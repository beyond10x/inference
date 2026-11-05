//! Adversary pass on `story:run-ingress-admitted-send-outlives-the-drain`, the queue's close to
//! its senders ([`InputEndpoint::close_to_senders`]) under every overload policy, a send given up
//! at each point around the close.
//!
//! Every case polls futures by hand on one thread, so none waits on time.

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
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

/// The policies a queue implements.
const POLICIES: [OverloadPolicy; 5] = [
    OverloadPolicy::Pause,
    OverloadPolicy::Fail,
    OverloadPolicy::Drop,
    OverloadPolicy::Coalesce,
    OverloadPolicy::DropStaleEpochs,
];

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

fn queue(capacity: usize, overload: OverloadPolicy) -> (QueueSender<u32>, QueueReceiver<u32>) {
    EdgeQueue::new(
        &edge(capacity, overload),
        HashMap::<GenerationId, i64>::new(),
    )
    .expect("a supported policy")
    .split()
}

fn item(stream: StreamId, n: u32) -> EventEnvelope<u32> {
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id: RunId::generate(),
        session_id: None,
        stream_id: stream,
        sequence: i64::from(n),
        producer: NodeId::generate(),
        port: "out".to_owned(),
        kind: "inference.stream.TextDelta".to_owned(),
        generation_id: None,
        timestamp: i64::from(n),
        timebase: Timebase::WallTime,
        causation_id: None,
        payload: n,
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

fn take_to_none(receiver: &mut QueueReceiver<u32>) -> Vec<u32> {
    let mut taken = Vec::new();
    while let Some(envelope) = now(receiver.recv()) {
        taken.push(envelope.payload);
    }
    taken
}

fn refused_closed(answer: Poll<Result<(), SendError<u32>>>) -> u32 {
    match answer {
        Poll::Ready(Err(SendError::Closed(envelope))) => envelope.payload,
        other => panic!("the send answered {other:?}, not Closed with its envelope"),
    }
}

struct Count(AtomicUsize);

impl Wake for Count {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::AcqRel);
    }
}

/// Under every policy, an empty queue and a full one closed to their senders refuse the next
/// send as closed with its own envelope, before the policy's own answer to a full queue (`Full`,
/// a discard, a coalesce) is reached, and hand over what they held, then `None`.
#[test]
fn every_policy_refuses_a_send_into_a_closed_queue_with_its_envelope_and_discards_nothing() {
    for policy in POLICIES {
        let stream = StreamId::generate();
        let (sender, mut receiver) = queue(1, policy);
        assert_eq!(
            receiver.close_to_senders(),
            ClosedToSenders::Empty,
            "{policy:?}"
        );
        assert_eq!(
            refused_closed(poll(sender.send(item(stream, 1)).as_mut(), Waker::noop())),
            1,
            "{policy:?}: an empty closed queue"
        );
        assert_eq!(take_to_none(&mut receiver), Vec::<u32>::new(), "{policy:?}");
        assert_eq!(receiver.discarded(), 0, "{policy:?}");

        let (sender, mut receiver) = queue(1, policy);
        now(sender.send(item(stream, 1))).expect("room");
        assert_eq!(
            receiver.close_to_senders(),
            ClosedToSenders::Holding,
            "{policy:?}"
        );
        assert_eq!(receiver.known_contents(), KnownContents::NotEmpty);
        assert_eq!(
            refused_closed(poll(sender.send(item(stream, 2)).as_mut(), Waker::noop())),
            2,
            "{policy:?}: a full closed queue refuses before its policy answers"
        );
        assert_eq!(
            take_to_none(&mut receiver),
            [1],
            "{policy:?}: the held envelope, unreplaced"
        );
        assert_eq!(receiver.discarded(), 0, "{policy:?}");
        assert_eq!(sender.discarded(), 0, "{policy:?}");
        assert_eq!(
            receiver.known_contents(),
            KnownContents::Empty,
            "{policy:?}"
        );
    }
}

/// A send given up at each point around the close pushes nothing: dropped before its first poll,
/// dropped while it waits for room, dropped once the close woke it and before its next poll, and
/// dropped once a take made room and before its next poll.
#[test]
fn a_send_given_up_at_each_point_around_the_close_delivers_nothing() {
    for policy in [OverloadPolicy::Pause, OverloadPolicy::DropStaleEpochs] {
        let stream = StreamId::generate();

        let (sender, mut receiver) = queue(1, policy);
        drop(sender.send(item(stream, 1)));
        assert_eq!(
            receiver.close_to_senders(),
            ClosedToSenders::Empty,
            "{policy:?}"
        );
        assert_eq!(take_to_none(&mut receiver), Vec::<u32>::new(), "{policy:?}");

        let (sender, mut receiver) = queue(1, policy);
        now(sender.send(item(stream, 1))).expect("room");
        let mut waiting = sender.send(item(stream, 2));
        assert!(poll(waiting.as_mut(), Waker::noop()).is_pending());
        drop(waiting);
        assert_eq!(
            receiver.close_to_senders(),
            ClosedToSenders::Holding,
            "{policy:?}"
        );
        assert_eq!(take_to_none(&mut receiver), [1], "{policy:?}");

        let (sender, mut receiver) = queue(1, policy);
        now(sender.send(item(stream, 1))).expect("room");
        let count = Arc::new(Count(AtomicUsize::new(0)));
        let waker = Waker::from(Arc::clone(&count));
        let mut woken = sender.send(item(stream, 2));
        assert!(poll(woken.as_mut(), &waker).is_pending());
        assert_eq!(
            receiver.close_to_senders(),
            ClosedToSenders::Holding,
            "{policy:?}"
        );
        assert_eq!(
            count.0.load(Ordering::Acquire),
            1,
            "{policy:?}: the close woke it"
        );
        drop(woken);
        assert_eq!(take_to_none(&mut receiver), [1], "{policy:?}");

        let (sender, mut receiver) = queue(1, policy);
        now(sender.send(item(stream, 1))).expect("room");
        let mut roomed = sender.send(item(stream, 2));
        assert!(poll(roomed.as_mut(), Waker::noop()).is_pending());
        assert_eq!(
            now(receiver.recv()).map(|envelope| envelope.payload),
            Some(1)
        );
        drop(roomed);
        assert_eq!(
            receiver.close_to_senders(),
            ClosedToSenders::Empty,
            "{policy:?}"
        );
        assert_eq!(take_to_none(&mut receiver), Vec::<u32>::new(), "{policy:?}");
        assert_eq!(
            refused_closed(poll(sender.send(item(stream, 3)).as_mut(), Waker::noop())),
            3,
            "{policy:?}"
        );
    }
}
