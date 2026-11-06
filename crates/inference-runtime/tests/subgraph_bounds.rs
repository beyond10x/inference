//! A subgraph node's child thread ends with its parent and never spins
//! (`story:runtime-subgraph-and-recorder-bounds`):
//!
//! - dropping a parent controller while the child runs ends the child's thread within
//!   [`CHILD_DROP_BOUND`];
//! - a bridge whose parent edge under `fail` stays full waits on a timer between its sends instead
//!   of re-polling at once, so the child's thread does not spin;
//! - a usage send waiting on a parent edge nobody reads gives way once the node is stopped, after
//!   [`USAGE_STOP_GRACE`], so the node's `stop` returns, and the record given up is counted.

use std::collections::HashMap;
use std::future::{self, Future};
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use inference_core::endpoint::{EndpointFuture, OutputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_core::usage::UsageRecorded;
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeEndpoints, NodeError,
    NodeFuture, NodeImplementation, NodeInstanceId, NodeOutput, NodeRole, OverloadPolicy,
    PipelineSpec, PlannedPipeline, PortId, PortTraits, USAGE_PORT, modality,
};
use inference_runtime::subgraph::{CHILD_DROP_BOUND, USAGE_STOP_GRACE};
use inference_runtime::{RunController, SubgraphNode};
use inference_testkit::graph::PipelinePayload;
use inference_testkit::stream::StreamEvent;
use inference_text::{SpanId, TranscriptCommitted};

/// How much later than its bound a step may end before the case counts it as hung.
const MARGIN: Duration = Duration::from_secs(3);

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

struct Profiles(DeploymentProfile);

impl Catalog for Profiles {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.0.profile_id() == profile_id).then_some(&self.0)
    }

    fn adapter(&self, _: AdapterId) -> Option<&Adapter> {
        None
    }
}

fn catalog() -> Profiles {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
    })
    .expect("a row of the table");
    Profiles(profile)
}

const TEXT: PortTraits = PortTraits {
    essentiality: Essentiality::Essential,
    delivery: Delivery::Reliable,
    incremental: true,
    end_marker_required: true,
    provisional_allowed: false,
};

const RECORDS: PortTraits = PortTraits {
    incremental: false,
    ..TEXT
};

fn spec(name: &str) -> PipelineSpec {
    PipelineSpec {
        name: name.to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    }
}

fn one() -> NonZeroUsize {
    NonZeroUsize::new(1).expect("1")
}

fn envelope(
    run_id: RunId,
    sequence: i64,
    kind: &str,
    payload: PipelinePayload,
) -> EventEnvelope<PipelinePayload> {
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id,
        session_id: None,
        stream_id: StreamId::generate(),
        sequence,
        producer: NodeId::generate(),
        port: "out".to_owned(),
        kind: kind.to_owned(),
        generation_id: None,
        timestamp: 0,
        timebase: Timebase::WallTime,
        causation_id: None,
        payload,
    }
}

fn text(run_id: RunId, sequence: i64) -> EventEnvelope<PipelinePayload> {
    let event = StreamEvent::TranscriptCommitted(TranscriptCommitted {
        span_id: SpanId::generate(),
        text: sequence.to_string(),
    });
    let kind = event.name().to_owned();
    envelope(run_id, sequence, &kind, PipelinePayload::Stream(event))
}

/// What a child node does once started.
#[derive(Clone, Copy)]
enum Act {
    /// Sends nothing and keeps its output until it is dropped.
    Linger,
    /// Sends texts on its output from a thread of its own until a send is refused.
    Texts,
    /// Sends one usage record on its output from a thread of its own, then drops the output.
    OneUsage,
}

/// The one node of every child here; it holds `alive` until it is dropped.
struct ChildNode {
    run_id: RunId,
    act: Act,
    held: Option<NodeEndpoints<PipelinePayload>>,
    _alive: mpsc::Sender<()>,
}

impl NodeImplementation<PipelinePayload> for ChildNode {
    fn describe(&mut self, _: &Node) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn start(
        &mut self,
        mut endpoints: NodeEndpoints<PipelinePayload>,
    ) -> NodeFuture<'_, Result<(), NodeError>> {
        let run_id = self.run_id;
        match self.act {
            Act::Linger => self.held = Some(endpoints),
            Act::Texts => {
                let output = endpoints.outputs.remove(0).endpoint;
                thread::spawn(move || {
                    for sequence in 1.. {
                        if block_on(output.send(text(run_id, sequence))).is_err() {
                            return;
                        }
                    }
                });
            }
            Act::OneUsage => {
                let output = endpoints.outputs.remove(0).endpoint;
                thread::spawn(move || {
                    let _sent = block_on(output.send(envelope(
                        run_id,
                        1,
                        UsageRecorded::KIND,
                        PipelinePayload::Control(ControlKind::Commit),
                    )));
                });
            }
        }
        Box::pin(future::ready(Ok(())))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        self.held = None;
        Box::pin(future::ready(()))
    }
}

/// A child of one source node with one output `port` of modality text, or structured when it is
/// `usage`, exposed under the port's name, and a parent of the subgraph node alone, its output of
/// that name exposed under it too; both planned. Answers the declaration, the child, the parent,
/// the subgraph node and its output port.
fn declared(port: &str) -> (Declaration, PlannedPipeline, PlannedPipeline, Node, PortId) {
    let catalog = catalog();
    let mut declaration = Declaration::new();
    let usage = port == USAGE_PORT;
    let (child_id, ()) = declaration
        .build(
            &catalog,
            catalog.0.profile_id(),
            spec("child"),
            |pipeline| {
                let source = pipeline.node("source", NodeRole::Source, Vec::new());
                if usage {
                    let out = pipeline.output::<modality::Structured>(&source, port, RECORDS);
                    pipeline.expose(&out, port, one(), OverloadPolicy::Pause);
                } else {
                    let out = pipeline.output::<modality::Text>(&source, port, TEXT);
                    pipeline.expose(&out, port, one(), OverloadPolicy::Pause);
                }
            },
        )
        .expect("the profile is declared");
    let validated = declaration.validate(child_id).expect("the child is valid");
    let child = declaration
        .plan(validated, &catalog)
        .expect("the child plans");
    let (parent_id, (subgraph, out)) = declaration
        .build(
            &catalog,
            catalog.0.profile_id(),
            spec("parent"),
            |pipeline| {
                let subgraph = pipeline
                    .subgraph("frontend", NodeRole::Transform, child_id)
                    .expect("the child is Planned");
                let out = if usage {
                    let out = pipeline.output::<modality::Structured>(&subgraph, port, RECORDS);
                    pipeline.expose(&out, port, one(), OverloadPolicy::Pause);
                    out.id()
                } else {
                    let out = pipeline.output::<modality::Text>(&subgraph, port, TEXT);
                    pipeline.expose(&out, port, one(), OverloadPolicy::Pause);
                    out.id()
                };
                (subgraph.id(), out)
            },
        )
        .expect("the profile is declared");
    let validated = declaration
        .validate(parent_id)
        .expect("the parent is valid");
    let parent = declaration
        .plan(validated, &catalog)
        .expect("the subgraph node matches the child's exposures");
    let node = declaration
        .node(subgraph)
        .expect("the subgraph node is declared")
        .clone();
    (declaration, child, parent, node, out)
}

/// The subgraph node of `declaration` over `child`, whose one node does `act` and holds a sender
/// of the answered receiver until it is dropped.
fn subgraph_node(
    run_id: RunId,
    declaration: &Declaration,
    child: &PlannedPipeline,
    act: Act,
) -> (SubgraphNode<PipelinePayload>, mpsc::Receiver<()>) {
    let (alive, gone) = mpsc::channel();
    let node = SubgraphNode::new(
        run_id,
        declaration,
        child,
        HashMap::<GenerationId, i64>::new(),
        Box::new(move |run_id, _: &Node| {
            Box::new(ChildNode {
                run_id,
                act,
                held: None,
                _alive: alive.clone(),
            }) as Box<dyn NodeImplementation<PipelinePayload>>
        }),
    );
    (node, gone)
}

/// Acceptance: dropping a parent controller mid-run ends the child thread within the stated
/// bound; the child's node, which the child's thread holds until it returns, is dropped by then.
#[test]
fn dropping_a_parent_controller_mid_run_ends_the_child_thread_within_the_bound() {
    let (declaration, child, parent, _, _) = declared("out");
    let run_id = RunId::generate();
    let (node, gone) = subgraph_node(run_id, &declaration, &child, Act::Linger);
    let mut node = Some(node);
    let mut controller = RunController::create_as(
        run_id,
        &parent,
        &declaration,
        HashMap::<GenerationId, i64>::new(),
        |_: &Node| {
            Box::new(node.take().expect("one node")) as Box<dyn NodeImplementation<PipelinePayload>>
        },
    )
    .expect("planned");
    let ends = controller.exposed();
    controller.validate().expect("Created");
    block_on(controller.prepare()).expect("the child prepares");
    block_on(controller.activate()).expect("the child starts");
    assert_eq!(
        gone.recv_timeout(Duration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout),
        "the child runs"
    );

    let (dropped, dropping) = mpsc::channel();
    thread::spawn(move || {
        let started = Instant::now();
        drop(controller);
        let _ = dropped.send(started.elapsed());
    });
    let took = dropping.recv_timeout(CHILD_DROP_BOUND + MARGIN);
    assert!(
        took.is_ok(),
        "story:runtime-subgraph-and-recorder-bounds: dropping the parent controller did not \
         return within {:?}",
        CHILD_DROP_BOUND + MARGIN
    );
    assert_eq!(
        gone.recv_timeout(MARGIN),
        Err(mpsc::RecvTimeoutError::Disconnected),
        "story:runtime-subgraph-and-recorder-bounds: the child's thread did not end with its \
         parent: its node was not dropped within {MARGIN:?} of the parent's drop, which took \
         {took:?}"
    );
    drop(ends);
}

/// A parent edge under `fail` that is always full: every send is refused `Full`, and counted.
struct AlwaysFull(Arc<AtomicUsize>);

impl OutputEndpoint<PipelinePayload> for AlwaysFull {
    fn send(
        &self,
        envelope: EventEnvelope<PipelinePayload>,
    ) -> EndpointFuture<'_, Result<(), SendError<PipelinePayload>>> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Box::pin(future::ready(Err(SendError::Full(envelope))))
    }
}

/// How long the case watches the bridge, and the most sends it may make meanwhile: a bridge
/// that yields once and retries at once makes one per poll of its thread, many thousands.
const WINDOW: Duration = Duration::from_millis(500);
const MOST_SENDS: usize = 200;

/// Acceptance: a slow parent consumer on a `fail` edge does not spin the child thread; the
/// bridge's sends over a fixed window stay bounded.
#[test]
fn a_full_fail_edge_leaving_the_subgraph_node_does_not_spin_the_child_thread() {
    let (declaration, child, _, declared, out) = declared("out");
    let (mut node, _gone) = subgraph_node(RunId::generate(), &declaration, &child, Act::Texts);
    let sends = Arc::new(AtomicUsize::new(0));
    block_on(node.describe(&declared));
    node.described(NodeInstanceId::generate());
    block_on(node.prepare()).expect("the child prepares");
    block_on(node.start(NodeEndpoints {
        inputs: Vec::new(),
        outputs: vec![NodeOutput {
            port_id: out,
            endpoint: Box::new(AlwaysFull(Arc::clone(&sends))),
        }],
    }))
    .expect("the child starts");

    let first = Instant::now() + Duration::from_secs(10);
    while sends.load(Ordering::Relaxed) == 0 && Instant::now() < first {
        thread::yield_now();
    }
    let before = sends.load(Ordering::Relaxed);
    assert!(before > 0, "the bridge sends to the full edge");
    thread::sleep(WINDOW);
    let during = sends.load(Ordering::Relaxed) - before;

    let (stopped, stopping) = mpsc::channel();
    thread::spawn(move || {
        block_on(node.cancel());
        block_on(node.stop());
        let _ = stopped.send(());
    });
    assert!(
        stopping.recv_timeout(MARGIN).is_ok(),
        "the cancel ends the bridge waiting for room"
    );
    assert!(
        during <= MOST_SENDS,
        "story:runtime-subgraph-and-recorder-bounds: the bridge sent {during} times into a full \
         `fail` edge within {WINDOW:?}; at most {MOST_SENDS} are allowed: it spins"
    );
}

/// A parent edge whose consumer holds its receiver and never reads: a send never completes.
struct NeverRead(Arc<Mutex<usize>>);

impl OutputEndpoint<PipelinePayload> for NeverRead {
    fn send(
        &self,
        _: EventEnvelope<PipelinePayload>,
    ) -> EndpointFuture<'_, Result<(), SendError<PipelinePayload>>> {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        Box::pin(future::pending())
    }
}

/// Acceptance: a cancel while a usage send waits ends `stop`: the usage send gives way once the
/// node is stopped, within [`USAGE_STOP_GRACE`], and the record it gave up is counted.
#[test]
fn a_cancel_while_a_usage_send_waits_on_an_unread_edge_ends_stop() {
    let (declaration, child, _, declared, usage) = declared(USAGE_PORT);
    let (mut node, _gone) = subgraph_node(RunId::generate(), &declaration, &child, Act::OneUsage);
    let handle = node.handle();
    let sends = Arc::new(Mutex::new(0));
    block_on(node.describe(&declared));
    node.described(NodeInstanceId::generate());
    block_on(node.prepare()).expect("the child prepares");
    block_on(node.start(NodeEndpoints {
        inputs: Vec::new(),
        outputs: vec![NodeOutput {
            port_id: usage,
            endpoint: Box::new(NeverRead(Arc::clone(&sends))),
        }],
    }))
    .expect("the child starts");
    let first = Instant::now() + Duration::from_secs(10);
    while *sends.lock().unwrap_or_else(PoisonError::into_inner) == 0 && Instant::now() < first {
        thread::yield_now();
    }
    assert_eq!(
        *sends.lock().unwrap_or_else(PoisonError::into_inner),
        1,
        "the bridge sends the usage record to the unread edge"
    );

    let (stopped, stopping) = mpsc::channel();
    thread::spawn(move || {
        block_on(node.cancel());
        let started = Instant::now();
        block_on(node.stop());
        let _ = stopped.send(started.elapsed());
    });
    let took = stopping.recv_timeout(USAGE_STOP_GRACE + MARGIN);
    assert!(
        took.is_ok(),
        "story:runtime-subgraph-and-recorder-bounds: the subgraph node's stop did not return \
         within {:?}: its usage send waits on an edge nobody reads",
        USAGE_STOP_GRACE + MARGIN
    );
    assert_eq!(
        handle.dropped_usage(),
        1,
        "the usage record given up at the stop is counted"
    );
}
