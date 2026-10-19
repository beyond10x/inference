//! Adversary pass 1: the edge queue under real concurrency. Producer threads and a consumer thread
//! each drive their futures with a minimal `block_on` that parks the thread until its waker runs,
//! so a lost wakeup hangs a thread; every scenario runs under a deadline and a hang fails the
//! case instead of the suite.

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::Arc;
use std::sync::mpsc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use inference_core::endpoint::{InputEndpoint, OutputEndpoint};
use inference_core::envelope::{EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId, Capability};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Edge, Essentiality, InteractionMode, NodeRole, OverloadPolicy,
    PipelineSpec, PortTraits, modality,
};
use inference_runtime::{EdgeQueue, QueueReceiver, QueueSender};

/// The one profile the case declares, and no adapter: what `Declaration::build` reads.
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
const ITEMS: u32 = 500;
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

fn item(producer: NodeId, stream: StreamId, n: u32) -> EventEnvelope<(u32, u32)> {
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
        payload: (0, n),
    }
}

type Payload = (u32, u32);

struct Outcome {
    delivered: Vec<(u32, u32)>,
    most_queued: usize,
    discarded: u64,
}

/// `PRODUCERS` threads each send `ITEMS` items on their own stream through a clone of one sender;
/// one consumer thread reads until the edge ends, yielding now and then to stay slow.
fn run(capacity: usize, overload: OverloadPolicy) -> Outcome {
    let (sender, receiver): (QueueSender<Payload>, QueueReceiver<Payload>) = EdgeQueue::new(
        &edge(capacity, overload),
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
                    let mut most = 0;
                    for n in 1..=ITEMS {
                        let mut envelope = item(producer, stream, n);
                        envelope.payload = (p, n);
                        block_on(sender.send(envelope)).expect("never refused here");
                        most = most.max(sender.len());
                    }
                    most
                })
            })
            .collect();
        drop(sender);
        let consumer = thread::spawn(move || {
            let mut receiver = receiver;
            let mut delivered = Vec::new();
            let mut most = 0;
            while let Some(envelope) = {
                most = most.max(receiver.len());
                block_on(receiver.recv())
            } {
                delivered.push(envelope.payload);
                if delivered.len() % 7 == 0 {
                    thread::yield_now();
                }
            }
            (delivered, most, receiver.discarded())
        });
        let mut most_queued = 0;
        for producer in producers {
            most_queued = most_queued.max(producer.join().expect("producer"));
        }
        let (delivered, most, discarded) = consumer.join().expect("consumer");
        let _ = done.send(Outcome {
            delivered,
            most_queued: most_queued.max(most),
            discarded,
        });
    });
    outcome
        .recv_timeout(DEADLINE)
        .expect("a thread stayed parked: a wakeup was lost")
}

fn per_producer(delivered: &[(u32, u32)]) -> Vec<Vec<u32>> {
    let mut each = vec![Vec::new(); PRODUCERS as usize];
    for &(p, n) in delivered {
        each[p as usize].push(n);
    }
    each
}

#[test]
fn pause_under_concurrent_producers_loses_nothing_and_keeps_each_stream_in_order() {
    let outcome = run(3, OverloadPolicy::Pause);
    assert!(outcome.most_queued <= 3, "{} queued", outcome.most_queued);
    assert_eq!(outcome.discarded, 0);
    for stream in per_producer(&outcome.delivered) {
        assert_eq!(stream, (1..=ITEMS).collect::<Vec<_>>());
    }
}

#[test]
fn drop_stale_epochs_without_generations_waits_like_pause_under_concurrency() {
    let outcome = run(2, OverloadPolicy::DropStaleEpochs);
    assert!(outcome.most_queued <= 2, "{} queued", outcome.most_queued);
    assert_eq!(outcome.discarded, 0);
    for stream in per_producer(&outcome.delivered) {
        assert_eq!(stream, (1..=ITEMS).collect::<Vec<_>>());
    }
}

#[test]
fn drop_and_coalesce_under_concurrent_producers_account_for_every_item() {
    for overload in [OverloadPolicy::Drop, OverloadPolicy::Coalesce] {
        let outcome = run(3, overload);
        assert!(
            outcome.most_queued <= 3,
            "{overload:?}: {} queued",
            outcome.most_queued
        );
        let delivered = u64::try_from(outcome.delivered.len()).expect("small");
        assert_eq!(
            delivered + outcome.discarded,
            u64::from(PRODUCERS * ITEMS),
            "{overload:?}: delivered plus discarded is every item sent"
        );
        for stream in per_producer(&outcome.delivered) {
            assert!(
                stream.windows(2).all(|pair| pair[0] < pair[1]),
                "{overload:?}: a stream arrived out of order: {stream:?}"
            );
        }
    }
}
