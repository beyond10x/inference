//! Sends that wait for room land in the order they started waiting
//! (`story:waiting-sends-land-in-the-order-they-started-waiting`).
//!
//! Room the queue makes goes to the oldest waiting send as a place in queue order. A place is
//! invisible to the receiver until its send lands there by its own poll, so the receiver never
//! waits on a caller. Each case drives the queue by hand, polling futures with wakers it
//! observes, so no case waits on time.

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use inference_core::endpoint::{
    ClosedToSenders, InputEndpoint, KnownContents, OutputEndpoint, SendError,
};
use inference_core::envelope::{EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId, Capability};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Edge, Essentiality, InteractionMode, NodeRole, OverloadPolicy,
    PipelineSpec, PortTraits, modality,
};
use inference_runtime::{EdgeQueue, QueueReceiver, QueueSender};

struct Profiles(DeploymentProfile);

impl Catalog for Profiles {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.0.profile_id() == profile_id).then_some(&self.0)
    }

    fn adapter(&self, _: AdapterId) -> Option<&Adapter> {
        None
    }
}

fn edge(capacity: usize, overload: OverloadPolicy) -> Edge {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
    })
    .expect("a row of the table");
    let traits = PortTraits {
        essentiality: Essentiality::Essential,
        delivery: Delivery::Reliable,
        incremental: true,
        end_marker_required: true,
        provisional_allowed: false,
    };
    let spec = PipelineSpec {
        name: "edge".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::Continuous,
        required_capabilities: vec![Capability::OutputStreaming],
    };
    let capacity = NonZeroUsize::new(capacity).expect("a positive capacity");
    let mut declaration = Declaration::new();
    let profile_id = profile.profile_id();
    let catalog = Profiles(profile);
    declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let source = pipeline.node("source", NodeRole::Source, Vec::new());
            let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
            let out = pipeline.output::<modality::Text>(&source, "out", traits);
            let into = pipeline.input::<modality::Text>(&sink, "in", traits);
            pipeline.connect(&out, &into, capacity, overload);
        })
        .expect("the profile is declared");
    declaration.edges().next().expect("one edge").clone()
}

fn queue(
    capacity: usize,
    overload: OverloadPolicy,
    epochs: HashMap<GenerationId, i64>,
) -> (QueueSender<u32>, QueueReceiver<u32>) {
    EdgeQueue::new(&edge(capacity, overload), epochs)
        .expect("a supported policy")
        .split()
}

fn item(
    producer: NodeId,
    stream: StreamId,
    generation: Option<GenerationId>,
    n: u32,
) -> EventEnvelope<u32> {
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id: RunId::generate(),
        session_id: None,
        stream_id: stream,
        sequence: i64::from(n),
        producer,
        port: "out".to_owned(),
        kind: "inference.stream.TextDelta".to_owned(),
        generation_id: generation,
        timestamp: i64::from(n),
        timebase: Timebase::WallTime,
        causation_id: None,
        payload: n,
    }
}

#[derive(Default)]
struct Flag(AtomicBool);

impl Wake for Flag {
    fn wake(self: Arc<Self>) {
        self.0.store(true, Ordering::Release);
    }
}

fn poll<F: Future + ?Sized>(future: Pin<&mut F>, waker: &Waker) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(waker))
}

fn now<T>(future: impl Future<Output = T>) -> T {
    match poll(pin!(future), Waker::noop()) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("the future did not complete at once"),
    }
}

/// The payload the receiver hands over now; panics if it would wait.
fn take(receiver: &mut QueueReceiver<u32>) -> Option<u32> {
    now(receiver.recv()).map(|envelope| envelope.payload)
}

/// Two waiting sends on one stream each get a place; the younger one is polled first. The
/// receiver takes envelopes in the order the sends started waiting, so the stream must arrive as
/// 3 then 4.
#[test]
fn two_reserved_sends_land_in_the_order_they_parked() {
    let (sender, mut receiver) = queue(2, OverloadPolicy::Pause, HashMap::new());
    let (producer, stream) = (NodeId::generate(), StreamId::generate());
    now(sender.send(item(producer, stream, None, 1))).expect("room");
    now(sender.send(item(producer, stream, None, 2))).expect("room");
    let mut older = sender.send(item(producer, stream, None, 3));
    let mut younger = sender.send(item(producer, stream, None, 4));
    assert!(poll(older.as_mut(), Waker::noop()).is_pending());
    assert!(poll(younger.as_mut(), Waker::noop()).is_pending());
    assert_eq!(take(&mut receiver), Some(1));
    assert_eq!(take(&mut receiver), Some(2));
    let younger_sent = poll(younger.as_mut(), Waker::noop());
    let older_sent = poll(older.as_mut(), Waker::noop());
    assert!(
        matches!(younger_sent, Poll::Ready(Ok(()))) && matches!(older_sent, Poll::Ready(Ok(()))),
        "both sends held room: {younger_sent:?}, {older_sent:?}"
    );
    let received = vec![take(&mut receiver), take(&mut receiver)];
    assert_eq!(
        received,
        [Some(3), Some(4)],
        "the send of 3 started waiting before the send of 4 on the same stream"
    );
}

/// Under `drop_stale_epochs`, a raised epoch discards queued envelopes and so makes room while a
/// send waits: a send arriving after the waiting one must not be taken before it.
#[test]
fn a_send_arriving_after_a_parked_one_is_not_taken_before_it_once_a_raise_made_room() {
    let (old, new) = (GenerationId::generate(), GenerationId::generate());
    let epochs = HashMap::from([(old, 1), (new, 2)]);
    let (sender, mut receiver) = queue(2, OverloadPolicy::DropStaleEpochs, epochs);
    let (raising, parking, arriving) = (NodeId::generate(), NodeId::generate(), NodeId::generate());
    let stream = StreamId::generate();
    now(sender.send(item(raising, stream, Some(old), 1))).expect("room");
    now(sender.send(item(raising, stream, Some(old), 2))).expect("room");
    let mut parked = sender.send(item(parking, StreamId::generate(), None, 10));
    assert!(poll(parked.as_mut(), Waker::noop()).is_pending());
    now(sender.send(item(raising, stream, Some(new), 3))).expect("the raise discards 1 and 2");
    let mut later = sender.send(item(arriving, StreamId::generate(), None, 20));
    let later_sent = poll(later.as_mut(), Waker::noop());
    let mut received = Vec::new();
    for _ in 0..3 {
        let _ = poll(parked.as_mut(), Waker::noop());
        let _ = poll(later.as_mut(), Waker::noop());
        if let Some(payload) = take(&mut receiver) {
            received.push(payload);
        }
    }
    let at = |payload| received.iter().position(|taken| *taken == payload);
    assert!(
        at(10).is_some() && at(20).is_some() && at(10) < at(20),
        "the send of 20 arrived after the send of 10 parked, answered {later_sent:?} at once, and \
         the consumer received {received:?}"
    );
}

/// A place a take made for a waiting send is not content until that send lands in it: the queue
/// reads empty, a receive waits, and a drain closes without waiting for the caller.
#[test]
fn a_granted_place_that_has_not_landed_reads_empty() {
    let (sender, mut receiver) = queue(1, OverloadPolicy::Pause, HashMap::new());
    let (producer, stream) = (NodeId::generate(), StreamId::generate());
    now(sender.send(item(producer, stream, None, 1))).expect("room");
    let mut waiting = sender.send(item(producer, stream, None, 2));
    assert!(poll(waiting.as_mut(), Waker::noop()).is_pending());
    assert_eq!(take(&mut receiver), Some(1));
    assert_eq!(
        receiver.known_contents(),
        KnownContents::Empty,
        "the place made for the waiting send holds nothing yet"
    );
    assert_eq!(receiver.len(), 0);
    assert!(poll(receiver.recv().as_mut(), Waker::noop()).is_pending());
    assert!(matches!(
        poll(waiting.as_mut(), Waker::noop()),
        Poll::Ready(Ok(()))
    ));
    assert_eq!(receiver.known_contents(), KnownContents::NotEmpty);
    assert_eq!(receiver.len(), 1);
    assert_eq!(take(&mut receiver), Some(2));

    let mut again = sender.send(item(producer, stream, None, 3));
    let mut third = sender.send(item(producer, stream, None, 4));
    now(sender.send(item(producer, stream, None, 5))).expect("room");
    assert!(poll(again.as_mut(), Waker::noop()).is_pending());
    assert!(poll(third.as_mut(), Waker::noop()).is_pending());
    assert_eq!(take(&mut receiver), Some(5));
    assert_eq!(
        receiver.close_to_senders(),
        ClosedToSenders::Empty,
        "a granted place that has not landed is not held"
    );
    assert_eq!(take(&mut receiver), None);
    match poll(again.as_mut(), Waker::noop()) {
        Poll::Ready(Err(SendError::Closed(envelope))) => assert_eq!(envelope.payload, 3),
        other => panic!("the send holding a place answered {other:?}"),
    }
}

/// The residual the design decides: a send whose place the receiver reaches before that send
/// polls again loses the place, not its envelope, and lands after the younger envelopes.
#[test]
fn a_place_the_receiver_reaches_before_its_send_polls_is_passed() {
    let (sender, mut receiver) = queue(2, OverloadPolicy::Pause, HashMap::new());
    let (producer, stream) = (NodeId::generate(), StreamId::generate());
    now(sender.send(item(producer, stream, None, 1))).expect("room");
    now(sender.send(item(producer, stream, None, 2))).expect("room");
    let mut older = sender.send(item(producer, stream, None, 3));
    let mut younger = sender.send(item(producer, stream, None, 4));
    assert!(poll(older.as_mut(), Waker::noop()).is_pending());
    assert!(poll(younger.as_mut(), Waker::noop()).is_pending());
    assert_eq!(take(&mut receiver), Some(1));
    assert_eq!(take(&mut receiver), Some(2));
    assert!(matches!(
        poll(younger.as_mut(), Waker::noop()),
        Poll::Ready(Ok(()))
    ));
    assert_eq!(
        take(&mut receiver),
        Some(4),
        "the receiver passes the older send's place and does not wait for it"
    );
    assert!(matches!(
        poll(older.as_mut(), Waker::noop()),
        Poll::Ready(Ok(()))
    ));
    assert_eq!(take(&mut receiver), Some(3));
    assert_eq!(receiver.discarded(), 0);
}

/// The receiver's drop refuses a send that holds a place and had not landed, as closed with its
/// envelope; nothing unlanded is counted as discarded.
#[test]
fn the_receivers_drop_refuses_a_granted_unlanded_send_as_closed_with_its_envelope() {
    let (sender, mut receiver) = queue(1, OverloadPolicy::Pause, HashMap::new());
    let (producer, stream) = (NodeId::generate(), StreamId::generate());
    now(sender.send(item(producer, stream, None, 1))).expect("room");
    let woken = Arc::new(Flag::default());
    let waker = Waker::from(Arc::clone(&woken));
    let mut waiting = sender.send(item(producer, stream, None, 2));
    assert!(poll(waiting.as_mut(), &waker).is_pending());
    assert_eq!(take(&mut receiver), Some(1));
    woken.0.store(false, Ordering::Release);
    drop(receiver);
    assert!(
        woken.0.load(Ordering::Acquire),
        "the receiver's drop woke the send"
    );
    match poll(waiting.as_mut(), Waker::noop()) {
        Poll::Ready(Err(SendError::Closed(envelope))) => assert_eq!(envelope.payload, 2),
        other => panic!("the send holding a place answered {other:?}"),
    }
    assert_eq!(sender.discarded(), 0, "nothing unlanded is discarded");
}
