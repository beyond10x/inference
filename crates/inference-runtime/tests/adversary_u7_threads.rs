//! Adversary pass 1 on `story:waiting-sends-land-in-the-order-they-started-waiting`, under real
//! threads. Producer threads give up some sends after they started waiting, some after a place
//! was granted to them; a consumer reads to the end, or drains by `known_contents` and closes.
//! Each thread parks until its waker runs, so a lost wakeup or room the queue never grants hangs
//! a thread, and a deadline turns the hang into a failed case.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::Arc;
use std::sync::mpsc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use inference_core::endpoint::{InputEndpoint, KnownContents, OutputEndpoint, SendError};
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

const PRODUCERS: u32 = 4;
const ITEMS: u32 = 300;
const DEADLINE: Duration = Duration::from_secs(60);

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

type Payload = (u32, u32);

fn item(producer: NodeId, stream: StreamId, p: u32, n: u32) -> EventEnvelope<Payload> {
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
        generation_id: None,
        timestamp: i64::from(n),
        timebase: Timebase::WallTime,
        causation_id: None,
        payload: (p, n),
    }
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
        thread::park();
    }
}

/// How one send of a producer ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sent {
    Taken,
    GivenUp,
    Closed,
}

/// Sends `n`: every fifth send is impatient and is dropped if it still waits after two short
/// parks, granted a place by then or not; every other send waits until the queue answers.
fn send(sender: &QueueSender<Payload>, producer: NodeId, stream: StreamId, p: u32, n: u32) -> Sent {
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut future = sender.send(item(producer, stream, p, n));
    let impatient = (n * 7 + p).is_multiple_of(5);
    let mut tries = 0;
    loop {
        match future.as_mut().poll(&mut Context::from_waker(&waker)) {
            Poll::Ready(Ok(())) => return Sent::Taken,
            Poll::Ready(Err(SendError::Closed(envelope))) => {
                assert_eq!(
                    envelope.payload,
                    (p, n),
                    "a refusal carries its own envelope"
                );
                return Sent::Closed;
            }
            Poll::Ready(Err(other)) => panic!("the queue answered {other:?}"),
            Poll::Pending if impatient => {
                tries += 1;
                if tries > 2 {
                    return Sent::GivenUp;
                }
                thread::park_timeout(Duration::from_micros(u64::from(n % 50)));
            }
            Poll::Pending => thread::park(),
        }
    }
}

struct Outcome {
    sent: Vec<Vec<(u32, Sent)>>,
    received: Vec<Payload>,
    most_queued: usize,
}

/// `PRODUCERS` threads send `ITEMS` each on their own stream through clones of one sender. The
/// consumer reads until the edge ends, or, when `drain_after` is set, drains by `known_contents`
/// once it has that many, closes the queue to its senders and reads what is left.
fn run(capacity: usize, drain_after: Option<usize>) -> Outcome {
    let (sender, mut receiver): (QueueSender<Payload>, QueueReceiver<Payload>) = EdgeQueue::new(
        &edge(capacity, OverloadPolicy::Pause),
        HashMap::<GenerationId, i64>::new(),
    )
    .expect("a supported policy")
    .split();
    let (done, outcome) = mpsc::channel();
    thread::spawn(move || {
        let producers: Vec<_> = (0..PRODUCERS)
            .map(|p| {
                let sender = sender.clone();
                thread::spawn(move || {
                    let (producer, stream) = (NodeId::generate(), StreamId::generate());
                    (1..=ITEMS)
                        .map(|n| (n, send(&sender, producer, stream, p, n)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        drop(sender);
        let consumer = thread::spawn(move || {
            let mut received = Vec::new();
            let mut most_queued = 0;
            loop {
                most_queued = most_queued.max(receiver.len());
                if drain_after.is_some_and(|after| received.len() >= after) {
                    while receiver.known_contents() == KnownContents::NotEmpty {
                        let envelope = block_on(receiver.recv()).expect("NotEmpty hands one over");
                        received.push(envelope.payload);
                    }
                    let _ = receiver.close_to_senders();
                    while let Some(envelope) = block_on(receiver.recv()) {
                        received.push(envelope.payload);
                    }
                    break;
                }
                match block_on(receiver.recv()) {
                    Some(envelope) => received.push(envelope.payload),
                    None => break,
                }
                if received.len().is_multiple_of(13) {
                    thread::yield_now();
                }
            }
            (received, most_queued)
        });
        let sent = producers
            .into_iter()
            .map(|producer| producer.join().expect("a producer does not panic"))
            .collect();
        let (received, most_queued) = consumer.join().expect("the consumer does not panic");
        let _ = done.send(Outcome {
            sent,
            received,
            most_queued,
        });
    });
    outcome
        .recv_timeout(DEADLINE)
        .expect("a thread hung: a wakeup was lost or room was held that no send could use")
}

fn check(outcome: &Outcome, capacity: usize) {
    assert!(outcome.most_queued <= capacity);
    let received: HashSet<Payload> = outcome.received.iter().copied().collect();
    assert_eq!(
        received.len(),
        outcome.received.len(),
        "an envelope received twice"
    );
    for (p, sent) in outcome.sent.iter().enumerate() {
        let p = u32::try_from(p).expect("small");
        let taken: Vec<u32> = sent
            .iter()
            .filter(|(_, how)| *how == Sent::Taken)
            .map(|(n, _)| *n)
            .collect();
        let got: Vec<u32> = outcome
            .received
            .iter()
            .filter(|(from, _)| *from == p)
            .map(|(_, n)| *n)
            .collect();
        assert_eq!(
            got, taken,
            "producer {p}: every send answered Ok is received once, in its order, and nothing \
             given up or refused is"
        );
    }
}

#[test]
fn sends_given_up_after_a_grant_leave_no_stall_under_threads() {
    for capacity in [1, 2, 3] {
        let outcome = run(capacity, None);
        check(&outcome, capacity);
    }
}

#[test]
fn a_drain_by_known_contents_under_threads_delivers_every_taken_send() {
    for capacity in [1, 2] {
        let outcome = run(capacity, Some(usize::try_from(ITEMS).expect("small")));
        check(&outcome, capacity);
    }
}
