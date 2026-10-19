//! Backpressure remains bounded under slow consumers (§23.3 test 4): one queue per
//! `inference.graph.Edge`, built from an edge carrying the overload policy each case names, never
//! holds more than the edge's capacity, and each of the five supported policies does what its row
//! of the §12.3 table says; `cancel` and `supersede` are refused when the queue is built.

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::{Pin, pin};
use std::task::{Context, Poll, Waker};

use inference_core::endpoint::{InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::{EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId, Capability};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Edge, Essentiality, InteractionMode, NodeRole, OverloadPolicy,
    PipelineSpec, PortTraits, modality,
};
use inference_runtime::{EdgeQueue, QueueReceiver, UnsupportedOverload};

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

/// One declared edge of a two-node text pipeline, with `capacity` and `overload`.
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
    let edge = declaration.edges().next().expect("one edge").clone();
    assert_eq!((edge.capacity(), edge.overload()), (capacity, overload));
    edge
}

type Sender = Box<dyn OutputEndpoint<u32>>;

/// The two sides of the queue of `edge`, the sender as the trait object a node holds.
fn queue(edge: &Edge, epochs: HashMap<GenerationId, i64>) -> (Sender, QueueReceiver<u32>) {
    let (sender, receiver) = EdgeQueue::new(edge, epochs)
        .expect("a supported policy")
        .split();
    (Box::new(sender), receiver)
}

/// Item `n` of `stream` from `producer`, in `generation` if any.
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

fn poll_once<F: Future + ?Sized>(future: Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

/// The output of `future`, which must complete on its first poll.
fn now<T>(future: impl Future<Output = T>) -> T {
    match poll_once(pin!(future)) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("the future did not complete at once"),
    }
}

/// Every payload the receiver yields until it reports that nothing more can arrive.
fn drain(receiver: &mut QueueReceiver<u32>) -> Vec<u32> {
    let mut payloads = Vec::new();
    while let Some(envelope) = now(receiver.recv()) {
        payloads.push(envelope.payload);
    }
    payloads
}

#[test]
fn pause_holds_the_producer_until_the_consumer_takes_one() {
    let c = 8;
    let (sender, mut receiver) = queue(&edge(c, OverloadPolicy::Pause), HashMap::new());
    let (producer, stream) = (NodeId::generate(), StreamId::generate());
    let mut production = pin!(async move {
        for n in 1..=1000 {
            sender
                .send(item(producer, stream, None, n))
                .await
                .expect("pause never refuses");
        }
        drop(sender);
    });
    let mut produced = false;
    let mut consumed = Vec::new();
    for _tick in 0..2000 {
        if !produced {
            produced = poll_once(production.as_mut()).is_ready();
        }
        assert!(
            receiver.len() <= c,
            "{} queued, capacity {c}",
            receiver.len()
        );
        match now(receiver.recv()) {
            Some(envelope) => consumed.push(envelope.payload),
            None => break,
        }
    }
    assert!(produced);
    assert_eq!(consumed, (1..=1000).collect::<Vec<_>>());
    assert_eq!(receiver.discarded(), 0);
}

#[test]
fn fail_returns_the_refused_item_to_the_producer() {
    let c: u32 = 4;
    let (sender, mut receiver) = queue(&edge(4, OverloadPolicy::Fail), HashMap::new());
    let (producer, stream) = (NodeId::generate(), StreamId::generate());
    for n in 1..=c {
        now(sender.send(item(producer, stream, None, n))).expect("room for it");
    }
    let refused = now(sender.send(item(producer, stream, None, c + 1)));
    match refused {
        Err(SendError::Full(envelope)) => assert_eq!(envelope.payload, c + 1),
        other => panic!("expected the typed full error, got {other:?}"),
    }
    assert_eq!(receiver.len(), 4);
    drop(sender);
    assert_eq!(drain(&mut receiver), (1..=c).collect::<Vec<_>>());
    assert_eq!(receiver.discarded(), 0);
}

#[test]
fn drop_discards_the_arriving_item_without_blocking() {
    let c: u32 = 4;
    let (sender, mut receiver) = queue(&edge(4, OverloadPolicy::Drop), HashMap::new());
    let (producer, stream) = (NodeId::generate(), StreamId::generate());
    for n in 1..=2 * c {
        now(sender.send(item(producer, stream, None, n))).expect("drop never refuses");
        assert!(receiver.len() <= 4);
    }
    drop(sender);
    assert_eq!(drain(&mut receiver), (1..=c).collect::<Vec<_>>());
    assert_eq!(receiver.discarded(), u64::from(c));
}

#[test]
fn coalesce_replaces_the_newest_item_of_the_same_stream() {
    let c: u32 = 4;
    let (sender, mut receiver) = queue(&edge(4, OverloadPolicy::Coalesce), HashMap::new());
    let producer = NodeId::generate();
    let (stream, other) = (StreamId::generate(), StreamId::generate());
    for n in 1..=2 * c {
        now(sender.send(item(producer, stream, None, n))).expect("coalesce never refuses");
        assert!(receiver.len() <= 4);
    }
    assert_eq!(receiver.discarded(), u64::from(c));
    now(sender.send(item(producer, other, None, 2 * c + 1))).expect("coalesce never refuses");
    assert_eq!(receiver.discarded(), u64::from(c + 1));
    drop(sender);
    let mut expected: Vec<u32> = (1..c).collect();
    expected.push(2 * c);
    assert_eq!(drain(&mut receiver), expected);
}

#[test]
fn drop_stale_epochs_discards_the_older_epoch_of_a_producer() {
    let (old, new) = (GenerationId::generate(), GenerationId::generate());
    let epochs = HashMap::from([(old, 1), (new, 2)]);
    let (sender, mut receiver) = queue(&edge(4, OverloadPolicy::DropStaleEpochs), epochs);
    let (producer, stream) = (NodeId::generate(), StreamId::generate());
    for n in 1..=2 {
        now(sender.send(item(producer, stream, Some(old), n))).expect("room for it");
    }
    now(sender.send(item(producer, stream, Some(new), 3))).expect("room for it");
    assert_eq!(receiver.len(), 1);
    assert_eq!(receiver.discarded(), 2);
    now(sender.send(item(producer, stream, Some(old), 4))).expect("discarded, not refused");
    assert_eq!(receiver.discarded(), 3);
    drop(sender);
    assert_eq!(drain(&mut receiver), vec![3]);
}

#[test]
fn drop_stale_epochs_never_discards_an_item_without_a_generation() {
    let (old, new) = (GenerationId::generate(), GenerationId::generate());
    let epochs = HashMap::from([(old, 1), (new, 2)]);
    let (sender, mut receiver) = queue(&edge(4, OverloadPolicy::DropStaleEpochs), epochs);
    let (producer, stream) = (NodeId::generate(), StreamId::generate());
    now(sender.send(item(producer, stream, Some(old), 1))).expect("room for it");
    now(sender.send(item(producer, stream, None, 2))).expect("room for it");
    now(sender.send(item(producer, stream, Some(new), 3))).expect("room for it");
    now(sender.send(item(producer, stream, None, 4))).expect("room for it");
    drop(sender);
    assert_eq!(drain(&mut receiver), vec![2, 3, 4]);
    assert_eq!(receiver.discarded(), 1);
}

#[test]
fn a_send_after_the_receiver_is_gone_returns_the_item() {
    let (sender, receiver) = queue(&edge(1, OverloadPolicy::Pause), HashMap::new());
    let (producer, stream) = (NodeId::generate(), StreamId::generate());
    now(sender.send(item(producer, stream, None, 1))).expect("room for it");
    let mut waiting = sender.send(item(producer, stream, None, 2));
    assert!(poll_once(waiting.as_mut()).is_pending());
    drop(receiver);
    match poll_once(waiting.as_mut()) {
        Poll::Ready(Err(SendError::Closed(envelope))) => assert_eq!(envelope.payload, 2),
        other => panic!("expected the typed closed error, got {other:?}"),
    }
    drop(waiting);
    let refused = now(sender.send(item(producer, stream, None, 3))).expect_err("closed");
    assert_eq!(refused.into_envelope().payload, 3);
}

#[test]
fn cancel_and_supersede_are_refused_when_the_queue_is_built() {
    for overload in [OverloadPolicy::Cancel, OverloadPolicy::Supersede] {
        let edge = edge(4, overload);
        let refused = EdgeQueue::<u32>::new(&edge, HashMap::new()).err();
        assert_eq!(
            refused,
            Some(UnsupportedOverload {
                edge_id: edge.edge_id(),
                overload,
            })
        );
    }
}
