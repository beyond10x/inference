//! A queue's receiver closes it to its senders in one step and says what it still holds
//! ([`InputEndpoint::close_to_senders`]): every send that has not pushed its envelope, one still
//! waiting for room, is refused as [`SendError::Closed`] with its envelope at its next poll, a
//! new send at once, and the receiver takes what was pushed and then answers `None` without
//! waiting (`story:run-ingress-admitted-send-outlives-the-drain`).
//!
//! The single-threaded cases poll futures by hand, so no case waits on time. The threaded case
//! races the close against a send landing from another thread, in a thread of its own that fails
//! once [`PATIENCE`] passed.

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::{Pin, pin};
use std::sync::{Arc, Barrier, mpsc};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

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

/// How long the threaded case waits before it counts as hung.
const PATIENCE: Duration = Duration::from_secs(60);

/// How many rounds the threaded case races the close against a landing send, per policy.
const ROUNDS: usize = 4000;

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

/// Every payload the receiver hands over now and then its `None`; panics if it would wait.
fn take_to_none(receiver: &mut QueueReceiver<u32>) -> Vec<u32> {
    let mut taken = Vec::new();
    while let Some(envelope) = now(receiver.recv()) {
        taken.push(envelope.payload);
    }
    taken
}

/// The payload a send was refused with as closed; panics on any other answer.
fn refused_closed(answer: Poll<Result<(), SendError<u32>>>) -> u32 {
    match answer {
        Poll::Ready(Err(SendError::Closed(envelope))) => envelope.payload,
        other => panic!("the send answered {other:?}, not Closed with its envelope"),
    }
}

/// A queue of one holding `1`, and sends of `2` and `3` waiting for room: the take of `1` makes
/// room and wakes both, and the close comes before either is polled again, so both are refused
/// as closed with their own envelopes, a new send is refused at once, and the receiver answers
/// `None` without waiting.
#[test]
fn a_close_after_a_take_refuses_the_woken_sends_and_the_receiver_answers_none_at_once() {
    let stream = StreamId::generate();
    let (sender, mut receiver) = queue(1, OverloadPolicy::Pause);
    now(sender.send(item(stream, 1))).expect("the empty queue takes it");
    let mut woken = sender.send(item(stream, 2));
    assert!(poll(woken.as_mut(), Waker::noop()).is_pending());
    let mut waiting = sender.send(item(stream, 3));
    assert!(poll(waiting.as_mut(), Waker::noop()).is_pending());
    assert_eq!(
        now(receiver.recv()).map(|envelope| envelope.payload),
        Some(1),
        "the take of 1 makes room, and neither waiting send has pushed into it yet"
    );

    assert_eq!(
        receiver.close_to_senders(),
        ClosedToSenders::Empty,
        "no waiting send pushed its envelope, so the closed queue holds nothing"
    );
    assert_eq!(refused_closed(poll(woken.as_mut(), Waker::noop())), 2);
    assert_eq!(refused_closed(poll(waiting.as_mut(), Waker::noop())), 3);
    assert_eq!(
        refused_closed(poll(sender.send(item(stream, 4)).as_mut(), Waker::noop())),
        4
    );
    assert_eq!(take_to_none(&mut receiver), Vec::<u32>::new());
    assert_eq!(receiver.known_contents(), KnownContents::Empty);
    assert_eq!(receiver.discarded(), 0, "nothing was taken and discarded");
}

/// A queue of two holding `1` and `2`, with a send of `3` waiting for room: the close answers
/// that it holds envelopes, the receiver takes both and then answers `None` without waiting,
/// and the send of `3` is refused as closed with its envelope, even once the takes made room.
#[test]
fn a_close_holding_envelopes_hands_them_over_and_refuses_the_send_still_waiting() {
    let stream = StreamId::generate();
    let (sender, mut receiver) = queue(2, OverloadPolicy::Pause);
    now(sender.send(item(stream, 1))).expect("room");
    now(sender.send(item(stream, 2))).expect("room");
    let mut waiting = sender.send(item(stream, 3));
    assert!(poll(waiting.as_mut(), Waker::noop()).is_pending());

    assert_eq!(receiver.close_to_senders(), ClosedToSenders::Holding);
    assert_eq!(receiver.known_contents(), KnownContents::NotEmpty);
    assert_eq!(take_to_none(&mut receiver), [1, 2]);
    assert_eq!(refused_closed(poll(waiting.as_mut(), Waker::noop())), 3);
    assert_eq!(receiver.known_contents(), KnownContents::Empty);
}

/// A send that waits for room is woken by the close, so that its next poll is refused without
/// waiting for a take that would never come.
#[test]
fn a_close_wakes_the_sends_waiting_for_room() {
    struct Count(std::sync::atomic::AtomicUsize);
    impl Wake for Count {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }
    }
    let stream = StreamId::generate();
    let (sender, mut receiver) = queue(1, OverloadPolicy::DropStaleEpochs);
    now(sender.send(item(stream, 1))).expect("room");
    let count = Arc::new(Count(std::sync::atomic::AtomicUsize::new(0)));
    let waker = Waker::from(Arc::clone(&count));
    let mut waiting = sender.send(item(stream, 2));
    assert!(poll(waiting.as_mut(), &waker).is_pending());

    assert_eq!(receiver.close_to_senders(), ClosedToSenders::Holding);
    assert_eq!(
        count.0.load(std::sync::atomic::Ordering::Acquire),
        1,
        "the close woke the waiting send"
    );
    assert_eq!(refused_closed(poll(waiting.as_mut(), &waker)), 2);
    assert_eq!(take_to_none(&mut receiver), [1]);
}

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
            return output;
        }
        thread::park_timeout(Duration::from_millis(50));
    }
}

/// Runs `case` in a thread of its own and fails once it took longer than [`PATIENCE`].
fn bounded<T: Send + 'static>(case: impl FnOnce() -> T + Send + 'static) -> T {
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let _test_waits_or_gave_up = done.send(case());
    });
    finished
        .recv_timeout(PATIENCE)
        .expect("the case finished within its time bound")
}

/// What a send answered: `Ok`, `Closed` with the payload of its envelope, or anything else.
enum Answer {
    Ok,
    Closed(u32),
    Other,
}

impl Answer {
    fn of(sent: Result<(), SendError<u32>>) -> Self {
        match sent {
            Ok(()) => Self::Ok,
            Err(SendError::Closed(envelope)) => Self::Closed(envelope.payload),
            Err(_) => Self::Other,
        }
    }
}

/// What the rounds of the threaded case came to.
#[derive(Debug, Default)]
struct Tally {
    delivered: usize,
    refused: usize,
    lost: usize,
}

/// A queue of one holding `1`; another thread sends `2`, which lands in the room the take of `1`
/// makes or waits for it, while this thread takes `1` and closes the queue to its senders, in
/// either order. Every round, the send of `2` is either received, its send answering `Ok`, or
/// refused as closed with its own envelope and not received: never `Ok` and discarded, and the
/// receiver never waits once the queue is closed.
#[test]
fn a_close_racing_a_landing_send_from_another_thread_delivers_or_refuses_it() {
    for policy in [OverloadPolicy::Pause, OverloadPolicy::DropStaleEpochs] {
        let tally = bounded(move || {
            let mut tally = Tally::default();
            for round in 0..ROUNDS {
                let stream = StreamId::generate();
                let (sender, mut receiver) = queue(1, policy);
                now(sender.send(item(stream, 1))).expect("the empty queue takes it");
                let start = Arc::new(Barrier::new(2));
                let racing = {
                    let start = Arc::clone(&start);
                    thread::spawn(move || {
                        start.wait();
                        Answer::of(block_on(sender.send(item(stream, 2))))
                    })
                };
                start.wait();
                let mut taken = Vec::new();
                if round % 2 == 0 {
                    taken.extend(now(receiver.recv()).map(|envelope| envelope.payload));
                }
                let closed = receiver.close_to_senders();
                assert_ne!(closed, ClosedToSenders::Unsupported);
                taken.extend(take_to_none(&mut receiver));
                let sent = racing.join().expect("the sending thread does not panic");
                assert_eq!(taken.first(), Some(&1), "{policy:?}: 1 is received first");
                let received_two = taken.contains(&2);
                match sent {
                    Answer::Ok if received_two => tally.delivered += 1,
                    Answer::Closed(2) if !received_two => tally.refused += 1,
                    Answer::Ok | Answer::Closed(_) | Answer::Other => tally.lost += 1,
                }
                assert_eq!(receiver.discarded(), 0, "{policy:?}: nothing was discarded");
            }
            tally
        });
        eprintln!(
            "{policy:?}: {ROUNDS} rounds, {} delivered, {} refused Closed with their envelope, {} \
             answered otherwise",
            tally.delivered, tally.refused, tally.lost
        );
        assert_eq!(
            tally.lost, 0,
            "{policy:?}: every send racing the close is delivered or refused as closed with its \
             envelope: {tally:?}"
        );
    }
}
