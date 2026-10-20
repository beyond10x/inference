//! A subgraph node runs its child pipeline as a nested child run with the cancellation and
//! tracing boundaries kept (`story:subgraph-execution`, A-14 of `docs/design/inference.md`).
//!
//! The child is the A-02 text pipeline with exposures: the mock model node alone between the
//! ingress `user text` and the egress `reply`. The parent is a text source, a subgraph node over
//! that child with the ports `user text` and `reply`, and a text sink. A run of the parent
//! completes with the child's reply at the parent's sink: every envelope the child logged carries
//! the child run's identity, and every envelope that crossed to the parent carries the parent
//! run's identity, the subgraph node as producer and, as `causation_id`, the identity of the child
//! envelope it crossed for. Cancelling the parent while the child streams leaves the child run
//! Cancelled, every child instance Stopped and every child queue dropped before the subgraph node
//! stops, and so before the parent run is Cancelled. A child node that fails fails the parent run
//! with a reason naming the subgraph node and carrying the child's, which names the child node.

use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use inference_core::endpoint::SendError;
use inference_core::envelope::{EventEnvelope, EventId, Timebase, is_control_kind};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId, Capability, ModelId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_llm::Generator;
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeEndpoints, NodeError,
    NodeFuture, NodeImplementation, NodeRole, OverloadPolicy, PipelineSpec, PlannedPipeline,
    PortTraits, modality,
};
use inference_runtime::run::{NodeInstanceState, RunState};
use inference_runtime::{RunController, RunError, SubgraphHandle, SubgraphNode};
use inference_testkit::graph::{
    FallibleStep, PipelinePayload, RecordingNode, RunLog, SinkHandle, SourceHandle, TextSink,
    TextSource,
};
use inference_testkit::llm::{MockConfiguration, MockGenerator, ModelNode};
use inference_testkit::stream::StreamEvent;
use inference_text::{SegmentId, TextSegmentCommitted};

const PATIENCE: Duration = Duration::from_secs(10);
const INPUTS: [&str; 2] = ["1", "2"];

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

struct Fixture {
    profile: DeploymentProfile,
    adapter: Adapter,
}

impl Catalog for Fixture {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.profile.profile_id() == profile_id).then_some(&self.profile)
    }

    fn adapter(&self, adapter_id: AdapterId) -> Option<&Adapter> {
        (self.adapter.adapter_id() == adapter_id).then_some(&self.adapter)
    }
}

const TEXT: PortTraits = PortTraits {
    essentiality: Essentiality::Essential,
    delivery: Delivery::Reliable,
    incremental: true,
    end_marker_required: true,
    provisional_allowed: false,
};

fn spec(name: &str) -> PipelineSpec {
    PipelineSpec {
        name: name.to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: vec![Capability::OutputStreaming],
    }
}

fn reply(input: &str) -> String {
    format!("s{input}")
}

fn generator() -> MockGenerator {
    MockGenerator::new(
        MockConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        Vec::<String>::new(),
    )
}

/// The declaration holding the planned child and the planned parent.
struct Planned {
    declaration: Declaration,
    child: PlannedPipeline,
    parent: PlannedPipeline,
    source: NodeId,
    subgraph: NodeId,
    sink: NodeId,
}

fn planned(generator: &MockGenerator) -> Planned {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    let adapter_id = generator.adapter().adapter_id();
    let catalog = Fixture {
        profile,
        adapter: generator.adapter().clone(),
    };
    let mut declaration = Declaration::new();
    let (child_id, ()) = declaration
        .build(&catalog, profile_id, spec("exposed model"), |pipeline| {
            let model = pipeline.node("model", NodeRole::Model, vec![Capability::OutputStreaming]);
            let prompt = pipeline.input::<modality::Text>(&model, "prompt", TEXT);
            let answer = pipeline.output::<modality::Text>(&model, "answer", TEXT);
            pipeline.expose(
                &prompt,
                "user text",
                NonZeroUsize::new(4).expect("4"),
                OverloadPolicy::Pause,
            );
            pipeline.expose(
                &answer,
                "reply",
                NonZeroUsize::new(16).expect("16"),
                OverloadPolicy::Fail,
            );
            pipeline.bind(&model, adapter_id).expect("known");
        })
        .expect("known");
    let validated = declaration.validate(child_id).expect("valid");
    let child = declaration.plan(validated, &catalog).expect("planned");
    let (parent_id, (source, subgraph, sink)) = declaration
        .build(&catalog, profile_id, spec("parent"), |pipeline| {
            let source = pipeline.node("source", NodeRole::Source, Vec::new());
            let emitted = pipeline.output::<modality::Text>(&source, "text", TEXT);
            let subgraph = pipeline
                .subgraph("frontend", NodeRole::Transform, child_id)
                .expect("the child is Planned");
            let into = pipeline.input::<modality::Text>(&subgraph, "user text", TEXT);
            let out = pipeline.output::<modality::Text>(&subgraph, "reply", TEXT);
            let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
            let collected = pipeline.input::<modality::Text>(&sink, "text", TEXT);
            pipeline.connect(
                &emitted,
                &into,
                NonZeroUsize::new(4).expect("4"),
                OverloadPolicy::Pause,
            );
            pipeline.connect(
                &out,
                &collected,
                NonZeroUsize::new(16).expect("16"),
                OverloadPolicy::Fail,
            );
            (source.id(), subgraph.id(), sink.id())
        })
        .expect("known");
    let validated = declaration.validate(parent_id).expect("valid");
    let parent = declaration
        .plan(validated, &catalog)
        .expect("the subgraph node matches the child's exposures");
    Planned {
        declaration,
        child,
        parent,
        source,
        subgraph,
        sink,
    }
}

/// A parent run whose subgraph node runs the child on `child_nodes`, validated and prepared.
struct Parent {
    controller: RunController<PipelinePayload>,
    run_id: RunId,
    source: SourceHandle,
    sink: SinkHandle,
    subgraph: SubgraphHandle,
}

fn parent(
    planned: &Planned,
    child_nodes: impl FnMut(RunId, &Node) -> Box<dyn NodeImplementation<PipelinePayload>>
    + Send
    + 'static,
) -> Result<Parent, RunError> {
    let run_id = RunId::generate();
    let source = TextSource::new(run_id, INPUTS, RunLog::new());
    let sink = TextSink::new();
    let subgraph = SubgraphNode::new(
        run_id,
        &planned.declaration,
        &planned.child,
        HashMap::<GenerationId, i64>::new(),
        Box::new(child_nodes),
    );
    let (source_handle, sink_handle, subgraph_handle) =
        (source.handle(), sink.handle(), subgraph.handle());
    let mut implementations: HashMap<NodeId, Box<dyn NodeImplementation<PipelinePayload>>> =
        HashMap::from([
            (
                planned.source,
                Box::new(source) as Box<dyn NodeImplementation<PipelinePayload>>,
            ),
            (planned.subgraph, Box::new(subgraph)),
            (planned.sink, Box::new(sink)),
        ]);
    let mut controller = RunController::create_as(
        run_id,
        &planned.parent,
        &planned.declaration,
        HashMap::<GenerationId, i64>::new(),
        |node: &Node| {
            implementations
                .remove(&node.node_id())
                .expect("every node of the parent has an implementation")
        },
    )
    .expect("planned");
    controller.validate().expect("Created");
    block_on(controller.prepare())?;
    Ok(Parent {
        controller,
        run_id,
        source: source_handle,
        sink: sink_handle,
        subgraph: subgraph_handle,
    })
}

/// The child's model, logging into `log`.
fn model(
    generator: MockGenerator,
    log: RunLog,
) -> impl FnMut(RunId, &Node) -> Box<dyn NodeImplementation<PipelinePayload>> + Send + 'static {
    move |run_id, _| {
        Box::new(ModelNode::new(
            run_id,
            generator.clone(),
            reply,
            log.clone(),
        ))
    }
}

#[test]
fn the_parent_completes_with_the_childs_reply_and_every_envelope_names_the_run_that_produced_it() {
    let generator = generator();
    let planned = planned(&generator);
    let child_log = RunLog::new();
    let mut parent = parent(&planned, model(generator, child_log.clone())).expect("prepares");
    let subgraph_instance = parent
        .controller
        .instance(planned.subgraph)
        .map(inference_runtime::NodeInstance::instance_id)
        .expect("described");
    let child_run = parent
        .subgraph
        .child_run_id()
        .expect("the subgraph node created its child run when it prepared");
    block_on(parent.controller.activate()).expect("every node starts");
    for _ in INPUTS {
        assert!(parent.source.release());
    }
    assert!(parent.source.release_end());
    block_on(parent.controller.drive()).expect("the loop ends the run");

    assert_eq!(parent.controller.run().state(), RunState::Completed);
    assert_eq!(parent.sink.committed(), ["s1", "s2"], "the child's reply");
    let at_stop = parent
        .subgraph
        .at_stop()
        .expect("the subgraph node stopped");
    assert_eq!(at_stop.run_id, child_run);
    assert_eq!(
        at_stop.parent,
        Some(subgraph_instance),
        "the child run names the subgraph node's instance as its parent"
    );
    assert_eq!(at_stop.state, RunState::Completed);
    assert!(
        at_stop
            .instances
            .iter()
            .all(|state| *state == NodeInstanceState::Stopped)
    );
    assert!(at_stop.queues_dropped, "every child queue is dropped");

    let logged = child_log.envelopes();
    assert!(!logged.is_empty());
    assert!(
        logged.iter().all(|envelope| envelope.run_id == child_run),
        "every envelope logged inside the child carries the child run's identity"
    );
    let logged_ids: BTreeSet<_> = logged.iter().map(|envelope| envelope.event_id).collect();
    let crossed: Vec<_> = parent
        .sink
        .received()
        .into_iter()
        .filter(|envelope| !is_control_kind(&envelope.kind))
        .collect();
    assert_eq!(crossed.len(), logged.len(), "every logged envelope crossed");
    for envelope in &crossed {
        assert_eq!(
            envelope.run_id, parent.run_id,
            "crossed into the parent run"
        );
        assert_eq!(
            envelope.producer, planned.subgraph,
            "the subgraph node emitted it"
        );
        assert!(
            envelope
                .causation_id
                .is_some_and(|cause| logged_ids.contains(&cause)),
            "its cause is the child envelope it crossed for"
        );
        assert!(
            !logged_ids.contains(&envelope.event_id),
            "a new event identity"
        );
    }
    for node_id in [planned.source, planned.subgraph, planned.sink] {
        assert_eq!(
            parent
                .controller
                .instance(node_id)
                .map(inference_runtime::NodeInstance::state),
            Some(NodeInstanceState::Stopped)
        );
    }
    assert!(
        parent
            .controller
            .queues()
            .iter()
            .all(|queue| queue.is_dropped())
    );
}

#[test]
fn cancelling_the_parent_while_the_child_streams_ends_the_child_before_the_parent() {
    let generator = generator();
    let planned = planned(&generator);
    let mut parent = parent(&planned, model(generator, RunLog::new())).expect("prepares");
    block_on(parent.controller.activate()).expect("every node starts");
    let requests = parent.controller.requests();
    let sink = parent.sink.clone();
    let source = parent.source.clone();
    let mut controller = parent.controller;
    let drive = thread::spawn(move || {
        let result = block_on(controller.drive());
        (controller, result)
    });
    for _ in INPUTS {
        assert!(source.release());
    }
    assert!(
        sink.wait_committed(2, PATIENCE),
        "the child streamed both replies to the parent"
    );
    requests.cancel();
    let (controller, result) = drive.join().expect("no panic");
    result.expect("the loop cancels the run");

    assert_eq!(controller.run().state(), RunState::Cancelled);
    let at_stop = parent
        .subgraph
        .at_stop()
        .expect("the subgraph node stopped before the parent was Cancelled");
    assert_eq!(at_stop.state, RunState::Cancelled, "the child run");
    assert!(
        at_stop
            .instances
            .iter()
            .all(|state| *state == NodeInstanceState::Stopped),
        "every child instance: {:?}",
        at_stop.instances
    );
    assert!(at_stop.queues_dropped, "every child queue is dropped");
    assert!(controller.queues().iter().all(|queue| queue.is_dropped()));
}

#[test]
fn a_child_node_that_fails_fails_the_parent_naming_the_subgraph_node_and_the_child_node() {
    let generator = generator();
    let planned = planned(&generator);
    let child_model = planned
        .declaration
        .nodes()
        .find(|node| node.pipeline_id() == planned.child.pipeline_id())
        .map(Node::node_id)
        .expect("the child's model");
    let refused = parent(&planned, |_, _| {
        Box::new(RecordingNode::failing_at(
            FallibleStep::Prepare,
            NodeError::new("the model is out of memory"),
        ))
    });
    let Err(RunError::NodeFailed(failure)) = refused else {
        panic!("the parent fails to prepare");
    };
    assert_eq!(
        failure.node_id,
        Some(planned.subgraph),
        "names the subgraph node"
    );
    assert!(
        failure.detail.contains(&child_model.to_string())
            && failure.detail.contains("the model is out of memory"),
        "carries the child's reason, naming the child node: {}",
        failure.detail
    );
}

/// A child node that, once started, sends text on its one output without end, on a thread of its
/// own, until it is cancelled or its output refuses; its input is held and never read.
#[derive(Default)]
struct Endless {
    run_id: Option<RunId>,
    node_id: Option<NodeId>,
    cancelled: Arc<AtomicBool>,
    sent: Arc<AtomicUsize>,
    thread: Option<thread::JoinHandle<()>>,
}

impl NodeImplementation<PipelinePayload> for Endless {
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.node_id = Some(node.node_id());
        Box::pin(std::future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn start(
        &mut self,
        mut endpoints: NodeEndpoints<PipelinePayload>,
    ) -> NodeFuture<'_, Result<(), NodeError>> {
        let output = endpoints.outputs.remove(0).endpoint;
        let held = endpoints.inputs;
        let (run_id, producer) = (
            self.run_id.expect("made for its run"),
            self.node_id.expect("described"),
        );
        let (cancelled, sent) = (Arc::clone(&self.cancelled), Arc::clone(&self.sent));
        self.thread = Some(thread::spawn(move || {
            let _held = held;
            let stream_id = StreamId::generate();
            let mut sequence = 0;
            while !cancelled.load(Ordering::Acquire) {
                sequence += 1;
                let event = StreamEvent::TextSegmentCommitted(TextSegmentCommitted {
                    segment_id: SegmentId::generate(),
                    text: format!("chunk {sequence}"),
                });
                let envelope = EventEnvelope {
                    schema_version: 1,
                    event_id: EventId::generate(),
                    run_id,
                    session_id: None,
                    stream_id,
                    sequence,
                    producer,
                    port: "answer".to_owned(),
                    kind: event.name().to_owned(),
                    generation_id: None,
                    timestamp: 0,
                    timebase: Timebase::WallTime,
                    causation_id: None,
                    payload: PipelinePayload::Stream(event),
                };
                let mut envelope = envelope;
                loop {
                    match block_on(output.send(envelope)) {
                        Ok(()) => break,
                        Err(SendError::Full(refused)) if !cancelled.load(Ordering::Acquire) => {
                            envelope = refused;
                            thread::yield_now();
                        }
                        Err(_) => return,
                    }
                }
                sent.fetch_add(1, Ordering::AcqRel);
            }
        }));
        Box::pin(std::future::ready(Ok(())))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        self.cancelled.store(true, Ordering::Release);
        Box::pin(std::future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        if let Some(thread) = self.thread.take() {
            let _ended = thread.join();
        }
        Box::pin(std::future::ready(()))
    }
}

#[test]
fn cancelling_the_parent_while_the_child_is_still_streaming_ends_the_child_first() {
    let generator = generator();
    let planned = planned(&generator);
    let sent = Arc::new(AtomicUsize::new(0));
    let child_sent = Arc::clone(&sent);
    let mut parent = parent(&planned, move |run_id, _: &Node| {
        Box::new(Endless {
            run_id: Some(run_id),
            sent: Arc::clone(&child_sent),
            ..Endless::default()
        })
    })
    .expect("prepares");
    block_on(parent.controller.activate()).expect("every node starts");
    let requests = parent.controller.requests();
    let sink = parent.sink.clone();
    let mut controller = parent.controller;
    let drive = thread::spawn(move || {
        let result = block_on(controller.drive());
        (controller, result)
    });
    assert!(
        sink.wait_received(3, PATIENCE),
        "the child streams into the parent's sink"
    );
    let before = sent.load(Ordering::Acquire);
    assert!(
        sink.wait_received(before + 2, PATIENCE),
        "and keeps streaming while the parent is Active"
    );
    requests.cancel();
    let (controller, result) = drive.join().expect("no panic");
    result.expect("the loop cancels the run");

    assert_eq!(controller.run().state(), RunState::Cancelled);
    let at_stop = parent
        .subgraph
        .at_stop()
        .expect("the subgraph node stopped before the parent was Cancelled");
    assert_eq!(at_stop.state, RunState::Cancelled, "the child run");
    assert_eq!(
        at_stop.instances,
        [NodeInstanceState::Stopped],
        "the streaming child node"
    );
    assert!(at_stop.queues_dropped, "every child queue is dropped");
    assert!(controller.queues().iter().all(|queue| queue.is_dropped()));
    assert!(
        sink.received().len() <= sent.load(Ordering::Acquire),
        "nothing reached the parent that the child did not send"
    );
}

#[test]
fn a_refused_start_leaves_the_child_run_wound_down_and_recorded() {
    let generator = generator();
    let planned = planned(&generator);
    let mut parent = parent(&planned, |_, _: &Node| {
        Box::new(RecordingNode::failing_at(
            FallibleStep::Start,
            NodeError::new("the model cannot start"),
        ))
    })
    .expect("prepares");
    let started = block_on(parent.controller.activate());
    let Err(RunError::NodeFailed(failure)) = started else {
        panic!("the parent's start is refused: {started:?}");
    };
    assert_eq!(failure.node_id, Some(planned.subgraph));
    assert!(failure.detail.contains("the model cannot start"));
    assert_eq!(parent.controller.run().state(), RunState::Failed);
    let at_stop = parent
        .subgraph
        .at_stop()
        .expect("the refused start recorded its child run");
    assert_eq!(at_stop.state, RunState::Failed, "the child run");
    assert!(
        at_stop.instances.iter().all(|state| state.is_terminal()),
        "no child instance is left running: {:?}",
        at_stop.instances
    );
    assert!(at_stop.queues_dropped, "every child queue is dropped");
}
