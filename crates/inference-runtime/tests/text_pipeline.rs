//! A text-only pipeline of mock nodes, source to mock LLM to sink, runs to completion, cancels and
//! drains (`story:text-pipeline-end-to-end`, A-02 of `docs/design/inference.md`, and §23.3 test 7:
//! graph runs drain and release worker resources).
//!
//! One pipeline is declared with the typed builder, validated and planned, and three runs of it
//! are driven by [`RunController::drive`]: one to Completed on the source's end of input, one
//! cancelled while Active, one drained by the test before the end of input. After each run every
//! node instance is Stopped, and the weak references the test holds show that no exchange task
//! of the run and no edge queue survives it.

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::{Arc, Weak};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, JoinHandle, Thread};
use std::time::Duration;

use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase, is_control_kind};
use inference_core::generation::{GenerationState, GenerationStateConflict};
use inference_core::id::{GenerationId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId, Capability, ModelId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_llm::Generator;
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeEndpoints, NodeError,
    NodeFuture, NodeImplementation, NodeRole, OverloadPolicy, PipelineSpec, PlannedPipeline,
    PortTraits, modality,
};
use inference_runtime::queue::QueueWatch;
use inference_runtime::run::{NodeInstanceState, RunState};
use inference_runtime::{NodeId, RunController, RunError, RunRequests};
use inference_testkit::graph::{
    PipelinePayload, RunLog, SinkHandle, SourceHandle, TextSink, TextSource,
};
use inference_testkit::llm::{MockConfiguration, MockGenerator, ModelHandle, ModelNode};
use inference_testkit::stream::StreamEvent;
use inference_text::{SegmentId, SegmentState, TextSegmentCommitted};

/// How long a case waits for the exchange tasks before it fails rather than hangs.
const PATIENCE: Duration = Duration::from_secs(10);

/// The four inputs the source holds, numbered 1 to 4.
const INPUTS: [&str; 4] = ["1", "2", "3", "4"];

/// The mock LLM's answer to input `n`: segment `s<n>`.
fn reply(input: &str) -> String {
    format!("s{input}")
}

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Polls `future` on this thread until it completes: the runtime commits to no executor.
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

/// The profile the pipeline is declared under and the mock LLM's adapter it binds to.
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

/// The three nodes of the pipeline.
#[derive(Debug, Clone, Copy)]
struct Nodes {
    source: NodeId,
    model: NodeId,
    sink: NodeId,
}

/// Source to model to sink, declared with the typed builder, validated and planned. The edge
/// into the model pauses; the edge into the sink fails on overload, as committed text is
/// delivered or fails explicitly.
fn planned(generator: &MockGenerator) -> (Declaration, PlannedPipeline, Nodes) {
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
    let text = PortTraits {
        essentiality: Essentiality::Essential,
        delivery: Delivery::Reliable,
        incremental: true,
        end_marker_required: true,
        provisional_allowed: false,
    };
    let spec = PipelineSpec {
        name: "text only".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: vec![Capability::OutputStreaming],
    };
    let mut declaration = Declaration::new();
    let (pipeline_id, nodes) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let source = pipeline.node("source", NodeRole::Source, Vec::new());
            let model = pipeline.node("model", NodeRole::Model, vec![Capability::OutputStreaming]);
            let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
            let emitted = pipeline.output::<modality::Text>(&source, "text", text);
            let prompt = pipeline.input::<modality::Text>(&model, "prompt", text);
            let answer = pipeline.output::<modality::Text>(&model, "answer", text);
            let collected = pipeline.input::<modality::Text>(&sink, "text", text);
            pipeline.connect(
                &emitted,
                &prompt,
                NonZeroUsize::new(4).expect("4"),
                OverloadPolicy::Pause,
            );
            pipeline.connect(
                &answer,
                &collected,
                NonZeroUsize::new(16).expect("16"),
                OverloadPolicy::Fail,
            );
            pipeline
                .bind(&model, adapter_id)
                .expect("the catalog holds the mock LLM's adapter");
            Nodes {
                source: source.id(),
                model: model.id(),
                sink: sink.id(),
            }
        })
        .expect("the profile is declared");
    let validated = declaration
        .validate(pipeline_id)
        .expect("source to model to sink is valid");
    let planned = declaration
        .plan(validated, &catalog)
        .expect("the model is bound to an adapter that streams output");
    (declaration, planned, nodes)
}

/// One run of the pipeline, Active, with its drive loop on a thread of its own, and what the
/// test holds of it: the nodes' handles, the run's requests, its event log and the weak
/// references to its tasks and edge queues.
struct Running {
    run_id: RunId,
    source: SourceHandle,
    model: ModelHandle,
    sink: SinkHandle,
    log: RunLog,
    requests: RunRequests,
    tasks: Vec<Weak<()>>,
    queues: Vec<QueueWatch<PipelinePayload>>,
    driver: Weak<()>,
    drive: JoinHandle<(RunController<PipelinePayload>, Result<(), RunError>)>,
}

/// Creates, validates, prepares and activates a run of `planned` whose model node answers on
/// `generator`, emitting `late` after a cancel when it is given, and starts its drive loop.
fn start(
    declaration: &Declaration,
    planned: &PlannedPipeline,
    nodes: Nodes,
    generator: &MockGenerator,
    late: Option<&str>,
) -> Running {
    let run_id = RunId::generate();
    let log = RunLog::new();
    let source = TextSource::new(run_id, INPUTS, log.clone());
    let mut model = ModelNode::new(run_id, generator.clone(), reply, log.clone());
    if let Some(late) = late {
        model = model.emitting_after_cancel(late);
    }
    let sink = TextSink::new();
    let (source_handle, model_handle, sink_handle) =
        (source.handle(), model.handle(), sink.handle());
    let mut implementations: HashMap<NodeId, Box<dyn NodeImplementation<PipelinePayload>>> =
        HashMap::from([
            (
                nodes.source,
                Box::new(source) as Box<dyn NodeImplementation<PipelinePayload>>,
            ),
            (nodes.model, Box::new(model)),
            (nodes.sink, Box::new(sink)),
        ]);
    let mut controller = RunController::create_as(
        run_id,
        planned,
        declaration,
        HashMap::<GenerationId, i64>::new(),
        |node: &Node| {
            implementations
                .remove(&node.node_id())
                .expect("every node of the pipeline has a mock")
        },
    )
    .expect("the pipeline is planned and neither edge cancels or supersedes");
    assert_eq!(controller.run().run_id(), run_id);
    assert_eq!(controller.run().pipeline_id(), planned.pipeline_id());
    controller.validate().expect("a Created run validates");
    block_on(controller.prepare()).expect("every node prepares");
    block_on(controller.activate()).expect("every node starts");
    assert_eq!(controller.run().state(), RunState::Active);
    let requests = controller.requests();
    let queues = controller.queues();
    let tasks = vec![
        source_handle.task(),
        model_handle.task(),
        sink_handle.task(),
    ];
    let token = Arc::new(());
    let driver = Arc::downgrade(&token);
    let drive = thread::spawn(move || {
        let held = token;
        let result = block_on(controller.drive());
        drop(held);
        (controller, result)
    });
    Running {
        run_id,
        source: source_handle,
        model: model_handle,
        sink: sink_handle,
        log,
        requests,
        tasks,
        queues,
        driver,
        drive,
    }
}

/// Waits for the drive loop to end and returns the controller, after checking §23.3 test 7:
/// every node instance is Stopped, and no task of the run and no edge queue survives it.
fn finished(running: Running, nodes: Nodes) -> Finished {
    let Running {
        run_id,
        source,
        model,
        sink,
        log,
        requests: _,
        tasks,
        queues,
        driver,
        drive,
    } = running;
    let (controller, result) = drive.join().expect("the drive loop does not panic");
    result.expect("the drive loop ends the run");
    for node_id in [nodes.source, nodes.model, nodes.sink] {
        assert_eq!(
            controller
                .instance(node_id)
                .map(|instance| instance.state()),
            Some(NodeInstanceState::Stopped),
            "the instance of node {node_id}"
        );
    }
    assert_eq!(controller.instances().count(), 3);
    assert_eq!(driver.strong_count(), 0, "the drive task is gone");
    for (index, task) in tasks.iter().enumerate() {
        assert_eq!(task.strong_count(), 0, "exchange task {index} is gone");
    }
    assert_eq!(queues.len(), 2, "one queue per edge");
    for queue in &queues {
        assert!(
            queue.is_dropped(),
            "the queue of edge {} is dropped",
            queue.edge_id()
        );
    }
    Finished {
        run_id,
        source,
        model,
        sink,
        log,
        controller,
    }
}

/// A run whose drive loop ended, with what the test holds of it.
struct Finished {
    run_id: RunId,
    source: SourceHandle,
    model: ModelHandle,
    sink: SinkHandle,
    log: RunLog,
    controller: RunController<PipelinePayload>,
}

fn generator() -> MockGenerator {
    MockGenerator::new(
        MockConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        Vec::<String>::new(),
    )
}

/// The texts of every `TextDelta` the sink received.
fn deltas(sink: &SinkHandle) -> Vec<String> {
    sink.received()
        .into_iter()
        .filter_map(|envelope| match envelope.payload {
            PipelinePayload::Stream(StreamEvent::TextDelta(delta)) => Some(delta.text),
            _ => None,
        })
        .collect()
}

fn a_run_completes_on_the_end_of_input_with_every_committed_segment_at_the_sink(
    declaration: &Declaration,
    planned: &PlannedPipeline,
    nodes: Nodes,
    generator: &MockGenerator,
) {
    let run = start(declaration, planned, nodes, generator, None);
    for _ in INPUTS {
        assert!(
            run.source.release(),
            "the source admits each input while Active"
        );
    }
    assert!(
        run.source.release_end(),
        "the source admits the end of input"
    );
    let run = finished(run, nodes);

    assert_eq!(run.controller.run().state(), RunState::Completed, "the run");
    assert_eq!(run.sink.committed(), ["s1", "s2", "s3", "s4"], "the sink");
    assert_eq!(run.source.emitted(), 4);
    assert_eq!(
        run.model.generation_state(),
        Some(GenerationState::Completed),
        "the model completed its generation on the end of input"
    );
    assert!(
        run.sink
            .received()
            .last()
            .is_some_and(|envelope| envelope.kind == ControlKind::Stop.as_str()),
        "the end of input reaches the sink last, as a control event"
    );
}

fn a_run_cancelled_while_active_delivers_nothing_after_the_cancel(
    declaration: &Declaration,
    planned: &PlannedPipeline,
    nodes: Nodes,
    generator: &MockGenerator,
) {
    let run = start(declaration, planned, nodes, generator, Some("s3"));
    assert!(run.source.release());
    assert!(run.source.release());
    assert!(
        run.sink.wait_committed(2, PATIENCE),
        "the sink holds s1 and s2 before the cancel"
    );
    run.requests.cancel();
    let run = finished(run, nodes);

    assert_eq!(run.controller.run().state(), RunState::Cancelled, "the run");
    assert_eq!(run.sink.committed(), ["s1", "s2"], "the sink");
    assert_eq!(deltas(&run.sink), ["s1", "s2"], "no delta after the cancel");
    assert_eq!(
        run.model.late_refusal(),
        Some(GenerationStateConflict {
            current: GenerationState::Cancelled,
        }),
        "the delta emitted after the cancel is refused stale-generation"
    );
    assert_eq!(
        run.model.generation_state(),
        Some(GenerationState::Cancelled),
        "the generation"
    );
    let segments = run.model.segments();
    assert_eq!(
        segments
            .iter()
            .map(|segment| segment.text().to_owned())
            .collect::<Vec<_>>(),
        ["s1", "s2"],
        "the late delta created no segment"
    );
    assert!(
        segments
            .iter()
            .all(|segment| segment.state() == SegmentState::Discarded),
        "the cancel discarded the generation's unplayed segments"
    );
}

fn a_run_drained_before_its_end_of_input_delivers_its_in_flight_segment_and_admits_no_more(
    declaration: &Declaration,
    planned: &PlannedPipeline,
    nodes: Nodes,
    generator: &MockGenerator,
) {
    let run = start(declaration, planned, nodes, generator, None);
    for _ in 0..3 {
        assert!(run.source.release());
    }
    assert!(run.source.wait_emitted(3, PATIENCE), "input 3 is emitted");
    run.requests.drain();
    let run = finished(run, nodes);

    assert!(
        !run.source.release(),
        "input 4 is not admitted after the drain"
    );
    assert!(!run.source.release_end(), "nor is the end of input");
    assert_eq!(run.source.emitted(), 3, "input 4 was never emitted");
    assert_eq!(run.controller.run().state(), RunState::Completed, "the run");
    assert_eq!(run.sink.committed(), ["s1", "s2", "s3"], "the sink");
    assert_eq!(
        run.model.generation_state(),
        Some(GenerationState::Completed),
        "the model completed its generation on the drain"
    );
}

/// The three runs of the story, in order, on one planned pipeline (`Run → Pipeline` is
/// many-to-one) and one mock generator, which refuses a second concurrent generation: each run
/// starts only if the one before it released its generation. Each helper checks its run's sink,
/// end states and resources.
#[test]
fn three_runs_of_one_planned_pipeline_complete_cancel_and_drain_and_release_their_resources() {
    let generator = generator();
    let (declaration, planned, nodes) = planned(&generator);
    a_run_completes_on_the_end_of_input_with_every_committed_segment_at_the_sink(
        &declaration,
        &planned,
        nodes,
        &generator,
    );
    a_run_cancelled_while_active_delivers_nothing_after_the_cancel(
        &declaration,
        &planned,
        nodes,
        &generator,
    );
    a_run_drained_before_its_end_of_input_delivers_its_in_flight_segment_and_admits_no_more(
        &declaration,
        &planned,
        nodes,
        &generator,
    );
}

#[test]
fn every_event_a_node_emits_is_logged_in_its_envelope_and_the_run_refuses_none_of_them() {
    let generator = generator();
    let (declaration, planned, nodes) = planned(&generator);
    let run = start(&declaration, &planned, nodes, &generator, None);
    assert!(run.source.release());
    assert!(run.source.release_end());
    let run = finished(run, nodes);

    let envelopes = run.log.envelopes();
    let produced_by = |node_id: NodeId| {
        envelopes
            .iter()
            .filter(|envelope| envelope.producer == node_id)
            .map(|envelope| envelope.kind.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        produced_by(nodes.source),
        ["inference.stream.TranscriptCommitted"]
    );
    assert_eq!(
        produced_by(nodes.model),
        [
            "inference.stream.GenerationStarted",
            "inference.stream.TextDelta",
            "inference.stream.TextSegmentCommitted",
            "inference.stream.GenerationCompleted",
        ]
    );
    assert!(
        envelopes
            .iter()
            .all(|envelope| envelope.run_id == run.run_id)
    );
    let received: Vec<EventId> = run
        .sink
        .received()
        .iter()
        .filter(|envelope| !is_control_kind(&envelope.kind))
        .map(|envelope| envelope.event_id)
        .collect();
    let logged: Vec<EventId> = envelopes
        .iter()
        .filter(|envelope| envelope.producer == nodes.model)
        .map(|envelope| envelope.event_id)
        .collect();
    assert_eq!(
        received, logged,
        "the sink received the logged envelopes of the model"
    );

    assert_eq!(
        run.controller.refusals(),
        [],
        "the mocks number every stream from 1 up, so the run refused nothing"
    );
}

/// A model node that, when it starts, sends user content on its output edge at the sequences
/// `sequences`, all on one stream, going on past a refused send, then drops its endpoints without
/// reading its input.
struct Repeating {
    run_id: RunId,
    sequences: Vec<i64>,
    node_id: Option<NodeId>,
}

impl NodeImplementation<PipelinePayload> for Repeating {
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.node_id = Some(node.node_id());
        Box::pin(std::future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn start(
        &mut self,
        endpoints: NodeEndpoints<PipelinePayload>,
    ) -> NodeFuture<'_, Result<(), NodeError>> {
        let producer = self.node_id.expect("described before started");
        let (run_id, sequences) = (self.run_id, self.sequences.clone());
        Box::pin(async move {
            let stream_id = StreamId::generate();
            for sequence in sequences {
                let event = StreamEvent::TextSegmentCommitted(TextSegmentCommitted {
                    segment_id: SegmentId::generate(),
                    text: format!("at {sequence}"),
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
                let _refused_or_sent = endpoints.outputs[0].endpoint.send(envelope).await;
            }
            drop(endpoints);
            Ok(())
        })
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(std::future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(std::future::ready(()))
    }
}

#[test]
fn a_repeated_or_decreasing_sequence_a_node_emits_on_a_run_is_refused_and_never_reaches_the_sink() {
    let generator = generator();
    let (declaration, planned, nodes) = planned(&generator);
    let run_id = RunId::generate();
    let sink = TextSink::new();
    let sink_handle = sink.handle();
    let mut implementations: HashMap<NodeId, Box<dyn NodeImplementation<PipelinePayload>>> =
        HashMap::from([
            (
                nodes.source,
                Box::new(TextSource::new(run_id, INPUTS, RunLog::new()))
                    as Box<dyn NodeImplementation<PipelinePayload>>,
            ),
            (
                nodes.model,
                Box::new(Repeating {
                    run_id,
                    sequences: vec![1, 1, 0, 2],
                    node_id: None,
                }),
            ),
            (nodes.sink, Box::new(sink)),
        ]);
    let mut controller = RunController::create_as(
        run_id,
        &planned,
        &declaration,
        HashMap::<GenerationId, i64>::new(),
        |node: &Node| {
            implementations
                .remove(&node.node_id())
                .expect("every node of the pipeline has an implementation")
        },
    )
    .expect("the pipeline is planned");
    controller.validate().expect("a Created run validates");
    block_on(controller.prepare()).expect("every node prepares");
    block_on(controller.activate()).expect("every node starts");
    controller.requests().drain();
    block_on(controller.drive()).expect("the loop ends the run");

    assert_eq!(controller.run().state(), RunState::Completed);
    let delivered: Vec<i64> = sink_handle
        .received()
        .iter()
        .map(|envelope| envelope.sequence)
        .collect();
    assert_eq!(
        delivered,
        [1, 2],
        "the repeat and the decrease never reached the sink"
    );
    let refusals = controller.refusals();
    assert_eq!(
        refusals
            .iter()
            .map(|refusal| (refusal.producer, refusal.last, refusal.sequence))
            .collect::<Vec<_>>(),
        [(nodes.model, 1, 1), (nodes.model, 1, 0)],
        "the run refused the repeated and the decreasing sequence, naming the node"
    );
    assert!(
        refusals
            .iter()
            .all(|refusal| refusal.stream_id == refusals[0].stream_id)
    );
}
