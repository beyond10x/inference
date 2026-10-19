//! Adversary pass 1: a task that waits on an edge queue is woken through its own waker when the
//! state it waits on changes, and the policies hold for more than one producer and for a full
//! queue under `drop_stale_epochs`. Every future here is polled with a counting waker, never a
//! no-op one, so a wake the queue forgets is a failed assertion.

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use inference_core::endpoint::{InputEndpoint, OutputEndpoint, SendError};
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

fn item(producer: NodeId, generation: Option<GenerationId>, n: u32) -> EventEnvelope<u32> {
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id: RunId::generate(),
        session_id: None,
        stream_id: StreamId::generate(),
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

/// A waker that counts how often it was woken.
#[derive(Default)]
struct Count(AtomicUsize);

impl Wake for Count {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct Task {
    count: Arc<Count>,
    waker: Waker,
}

impl Task {
    fn new() -> Self {
        let count = Arc::new(Count::default());
        let waker = Waker::from(Arc::clone(&count));
        Self { count, waker }
    }

    fn poll<F: Future + ?Sized>(&self, future: Pin<&mut F>) -> Poll<F::Output> {
        future.poll(&mut Context::from_waker(&self.waker))
    }

    fn woken(&self) -> usize {
        self.count.0.load(Ordering::SeqCst)
    }
}

fn now<T>(future: impl Future<Output = T>) -> T {
    match Task::new().poll(pin!(future)) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("the future did not complete at once"),
    }
}

#[test]
fn pause_wakes_the_waiting_producer_when_the_consumer_takes_one() {
    let (sender, mut receiver) = queue(1, OverloadPolicy::Pause, HashMap::new());
    let producer = NodeId::generate();
    now(sender.send(item(producer, None, 1))).expect("room for it");
    let task = Task::new();
    let mut waiting = sender.send(item(producer, None, 2));
    assert!(task.poll(waiting.as_mut()).is_pending());
    assert_eq!(task.woken(), 0);
    assert_eq!(now(receiver.recv()).map(|e| e.payload), Some(1));
    assert!(task.woken() > 0, "the waiting producer was never woken");
    assert!(matches!(task.poll(waiting.as_mut()), Poll::Ready(Ok(()))));
}

#[test]
fn a_send_wakes_the_waiting_consumer() {
    let (sender, mut receiver) = queue(2, OverloadPolicy::Pause, HashMap::new());
    let task = Task::new();
    let mut waiting = receiver.recv();
    assert!(task.poll(waiting.as_mut()).is_pending());
    now(sender.send(item(NodeId::generate(), None, 1))).expect("room for it");
    assert!(task.woken() > 0, "the waiting consumer was never woken");
    match task.poll(waiting.as_mut()) {
        Poll::Ready(Some(envelope)) => assert_eq!(envelope.payload, 1),
        other => panic!("expected item 1, got {other:?}"),
    }
}

#[test]
fn only_the_last_sender_ends_the_edge_and_wakes_the_consumer() {
    let (sender, mut receiver) = queue(2, OverloadPolicy::Pause, HashMap::new());
    let clone = sender.clone();
    let task = Task::new();
    let mut waiting = receiver.recv();
    assert!(task.poll(waiting.as_mut()).is_pending());
    drop(sender);
    assert!(
        task.poll(waiting.as_mut()).is_pending(),
        "a clone is still alive, the edge has not ended"
    );
    drop(clone);
    assert!(task.woken() > 0, "the consumer was not woken by the end");
    assert!(matches!(task.poll(waiting.as_mut()), Poll::Ready(None)));
}

#[test]
fn dropping_the_receiver_wakes_the_waiting_producer() {
    let (sender, receiver) = queue(1, OverloadPolicy::Pause, HashMap::new());
    let producer = NodeId::generate();
    now(sender.send(item(producer, None, 1))).expect("room for it");
    let task = Task::new();
    let mut waiting = sender.send(item(producer, None, 2));
    assert!(task.poll(waiting.as_mut()).is_pending());
    drop(receiver);
    assert!(task.woken() > 0, "the waiting producer was never woken");
    match task.poll(waiting.as_mut()) {
        Poll::Ready(Err(SendError::Closed(envelope))) => assert_eq!(envelope.payload, 2),
        other => panic!("expected the closed error, got {other:?}"),
    }
}

#[test]
fn drop_stale_epochs_waits_when_full_and_a_newer_epoch_wakes_the_stale_waiter() {
    let (old, new) = (GenerationId::generate(), GenerationId::generate());
    let (sender, mut receiver) = queue(
        2,
        OverloadPolicy::DropStaleEpochs,
        HashMap::from([(old, 1), (new, 2)]),
    );
    let producer = NodeId::generate();
    now(sender.send(item(producer, Some(old), 1))).expect("room for it");
    now(sender.send(item(producer, Some(old), 2))).expect("room for it");
    let task = Task::new();
    let mut waiting = sender.send(item(producer, Some(old), 3));
    assert!(
        task.poll(waiting.as_mut()).is_pending(),
        "a full queue of the current epoch holds the producer, as under pause"
    );
    assert_eq!((receiver.len(), receiver.discarded()), (2, 0));
    let other = sender.clone();
    now(other.send(item(producer, Some(new), 4))).expect("room after the purge");
    assert_eq!(receiver.discarded(), 2);
    assert!(task.woken() > 0, "the purge freed room and woke nobody");
    assert!(matches!(task.poll(waiting.as_mut()), Poll::Ready(Ok(()))));
    assert_eq!(receiver.discarded(), 3, "item 3 is of the stale epoch");
    drop(waiting);
    drop((sender, other));
    let mut delivered = Vec::new();
    while let Some(envelope) = now(receiver.recv()) {
        delivered.push(envelope.payload);
    }
    assert_eq!(delivered, vec![4]);
}

#[test]
fn drop_stale_epochs_compares_epochs_per_producer() {
    let (a5, a6, b1) = (
        GenerationId::generate(),
        GenerationId::generate(),
        GenerationId::generate(),
    );
    let (sender, mut receiver) = queue(
        4,
        OverloadPolicy::DropStaleEpochs,
        HashMap::from([(a5, 5), (a6, 6), (b1, 1)]),
    );
    let (a, b) = (NodeId::generate(), NodeId::generate());
    now(sender.send(item(a, Some(a5), 1))).expect("room for it");
    now(sender.send(item(b, Some(b1), 2))).expect("room for it");
    assert_eq!(
        receiver.discarded(),
        0,
        "b's epoch 1 is not stale for a's 5"
    );
    now(sender.send(item(a, Some(a6), 3))).expect("room for it");
    now(sender.send(item(b, Some(b1), 4))).expect("room for it");
    assert_eq!(receiver.discarded(), 1, "only a's epoch 5 is stale");
    drop(sender);
    let mut delivered = Vec::new();
    while let Some(envelope) = now(receiver.recv()) {
        delivered.push(envelope.payload);
    }
    assert_eq!(delivered, vec![2, 3, 4]);
}
