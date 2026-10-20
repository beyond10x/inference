//! The LLM to tool feedback loop of a text-only pipeline (`story:tool-feedback-loop`, topology A
//! of `docs/design/inference.md` §14.1 with its optional tool call and result feedback, the
//! feedback pattern of §10.3 and its bounded iteration rule, 546).
//!
//! One pipeline on mocks is declared with the typed builder: source to the mock LLM node to sink,
//! and the loop from the LLM node's `tool_call` port through the tool authority and the tool
//! executor back to its `tool_result` port, with the authority's rejections returning to the same
//! port. Every edge of the loop is Structured, bounded and fails on overload (687). The edge from
//! `tool_call` into the authority is the feedback edge: a `FeedbackLoop` bounds it with the number
//! of tool rounds one generation may make, which the authority enforces.
//!
//! The cases check that the scripted proposal is executed and its result reinjected into the
//! generation that proposed, whose text after the result carries its identity and which reaches
//! Completed; that with an iteration bound of N the proposal N+1 of one generation is rejected
//! `exhausted` while the generation still completes; and that the pipeline validates with the
//! bound declared and is refused without it (`inference.graph.ValidatePipeline/outcome/rejected`,
//! check 5).

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant};

use inference_core::envelope::{ControlKind, EventEnvelope};
use inference_core::generation::GenerationState;
use inference_core::id::{GenerationId, RunId};
use inference_core::model::{Adapter, AdapterId, Capability, ModelId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::nodes::{
    AuthorityHandle, AuthorityOutcome, IterationRuleNotDeclared, ToolAuthority, ToolCallUpdate,
    ToolExecutorNode, ToolPayload,
};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, GraphDefect, GraphFinding, IncompatibleGraph,
    InteractionMode, Node, NodeImplementation, NodeRole, OverloadPolicy, PipelineId, PipelineSpec,
    PipelineState, PlannedPipeline, PortId, PortTraits, ValidatePipeline, ValidatePipelineError,
    modality,
};
use inference_runtime::run::{NodeInstanceState, RunState};
use inference_runtime::{NodeId, RunController, RunError, RunRequests};
use inference_testkit::graph::{RunLog, TextSink, TextSource};
use inference_testkit::llm::{
    Carried, MockConfiguration, MockGenerator, ModelHandle, ModelNode, ModelPayload,
};
use inference_testkit::stream::StreamEvent;
use inference_testkit::tools::{MockToolExecutor, Script};
use inference_text::{SpanId, TranscriptCommitted};
use inference_tools::{DeclareTool, Json, RejectionReason, SideEffectClass, Tool};
use serde_json::json;

/// How long a case waits for the run to end before it fails rather than hangs, and the
/// executor's deadline.
const PATIENCE: Duration = Duration::from_secs(10);

/// The one input the source holds.
const INPUT: &str = "the weather in Berlin";

/// The mock LLM's answer to an input text.
fn reply(input: &str) -> String {
    format!("looking up {input}")
}

/// What the edges of the pipeline carry: an `inference.stream` event, a step of a tool call,
/// Structured, or a control event.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Payload {
    Stream(StreamEvent),
    Tool(ToolCallUpdate),
    Control(ControlKind),
}

impl From<ControlKind> for Payload {
    fn from(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

impl ToolPayload for Payload {
    fn into_tool_call(self) -> Result<ToolCallUpdate, Self> {
        match self {
            Self::Tool(update) => Ok(update),
            other @ (Self::Stream(_) | Self::Control(_)) => Err(other),
        }
    }

    fn from_tool_call(update: ToolCallUpdate) -> Self {
        Self::Tool(update)
    }

    fn control(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

impl ModelPayload for Payload {
    fn stream(event: StreamEvent) -> Self {
        Self::Stream(event)
    }

    fn carried(self) -> Carried {
        match self {
            Self::Stream(event) => Carried::Stream(event),
            Self::Tool(update) => Carried::Tool(update),
            Self::Control(kind) => Carried::Control(kind),
        }
    }
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

/// Where the pipeline declares a `FeedbackLoop`, if anywhere.
#[derive(Debug, Clone, Copy)]
enum Feedback {
    /// No feedback loop.
    Undeclared,
    /// On the edge from `tool_call` into the authority, with this iteration rule.
    OnProposals(NonZeroUsize),
    /// On the edge from the source into the LLM node, which is on no cycle.
    OnInput(NonZeroUsize),
}

/// The nodes and the ports a run of the pipeline names.
#[derive(Debug, Clone, Copy)]
struct Ids {
    pipeline: PipelineId,
    source: NodeId,
    model: NodeId,
    authority: NodeId,
    executor: NodeId,
    sink: NodeId,
    tool_call: PortId,
    tool_result: PortId,
    rejected: PortId,
}

impl Ids {
    fn nodes(self) -> [NodeId; 5] {
        [
            self.source,
            self.model,
            self.authority,
            self.executor,
            self.sink,
        ]
    }
}

/// Declares the pipeline with the typed builder, its LLM node requiring `tool_calling` and bound
/// to the adapter of `generator`, and the feedback loop `feedback`. The pipeline stays Declared.
fn declared(generator: &MockGenerator, feedback: Feedback) -> (Declaration, Fixture, Ids) {
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
    let structured = PortTraits {
        essentiality: Essentiality::Essential,
        delivery: Delivery::Reliable,
        incremental: false,
        end_marker_required: true,
        provisional_allowed: false,
    };
    let spec = PipelineSpec {
        name: "text with tools".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: vec![Capability::OutputStreaming],
    };
    let four = NonZeroUsize::new(4).expect("4");
    let mut declaration = Declaration::new();
    let (pipeline, ids) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let source = pipeline.node("source", NodeRole::Source, Vec::new());
            let model = pipeline.node(
                "model",
                NodeRole::Model,
                vec![Capability::OutputStreaming, Capability::ToolCalling],
            );
            let authority = pipeline.node("authority", NodeRole::Policy, Vec::new());
            let executor = pipeline.node("executor", NodeRole::Tool, Vec::new());
            let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
            let emitted = pipeline.output::<modality::Text>(&source, "text", text);
            let prompt = pipeline.input::<modality::Text>(&model, "prompt", text);
            let answer = pipeline.output::<modality::Text>(&model, "answer", text);
            let tool_call =
                pipeline.output::<modality::Structured>(&model, "tool_call", structured);
            let tool_result =
                pipeline.input::<modality::Structured>(&model, "tool_result", structured);
            let proposed =
                pipeline.input::<modality::Structured>(&authority, "proposed", structured);
            let authorized =
                pipeline.output::<modality::Structured>(&authority, "authorized", structured);
            let rejected =
                pipeline.output::<modality::Structured>(&authority, "rejected", structured);
            let calls = pipeline.input::<modality::Structured>(&executor, "authorized", structured);
            let outcomes =
                pipeline.output::<modality::Structured>(&executor, "outcomes", structured);
            let collected = pipeline.input::<modality::Text>(&sink, "text", text);
            match feedback {
                Feedback::OnInput(bound) => {
                    pipeline.connect_feedback(
                        &emitted,
                        &prompt,
                        four,
                        OverloadPolicy::Pause,
                        bound,
                    );
                }
                Feedback::Undeclared | Feedback::OnProposals(_) => {
                    pipeline.connect(&emitted, &prompt, four, OverloadPolicy::Pause);
                }
            }
            pipeline.connect(
                &answer,
                &collected,
                NonZeroUsize::new(16).expect("16"),
                OverloadPolicy::Fail,
            );
            match feedback {
                Feedback::OnProposals(bound) => {
                    pipeline.connect_feedback(
                        &tool_call,
                        &proposed,
                        four,
                        OverloadPolicy::Fail,
                        bound,
                    );
                }
                Feedback::Undeclared | Feedback::OnInput(_) => {
                    pipeline.connect(&tool_call, &proposed, four, OverloadPolicy::Fail);
                }
            }
            pipeline.connect(&authorized, &calls, four, OverloadPolicy::Fail);
            pipeline.connect(&outcomes, &tool_result, four, OverloadPolicy::Fail);
            pipeline.connect(&rejected, &tool_result, four, OverloadPolicy::Fail);
            pipeline
                .bind(&model, adapter_id)
                .expect("the catalog holds the mock LLM's adapter");
            Ids {
                pipeline: pipeline.pipeline_id(),
                source: source.id(),
                model: model.id(),
                authority: authority.id(),
                executor: executor.id(),
                sink: sink.id(),
                tool_call: tool_call.id(),
                tool_result: tool_result.id(),
                rejected: rejected.id(),
            }
        })
        .expect("the profile is declared");
    assert_eq!(pipeline, ids.pipeline);
    (declaration, catalog, ids)
}

/// The pipeline with its feedback loop bounded by `bound`, validated and planned.
fn planned(generator: &MockGenerator, bound: NonZeroUsize) -> (Declaration, PlannedPipeline, Ids) {
    let (mut declaration, catalog, ids) = declared(generator, Feedback::OnProposals(bound));
    let validated = declaration
        .validate(ids.pipeline)
        .expect("a cycle through a bounded feedback edge is valid");
    let planned = declaration
        .plan(validated, &catalog)
        .expect("the LLM node is bound to an adapter that streams and calls tools");
    (declaration, planned, ids)
}

/// The one tool of the cases: read-only, its schema requires a string `city`.
fn lookup() -> Tool {
    Tool::declare(DeclareTool {
        name: "lookup".to_owned(),
        schema: json!({
            "type": "object",
            "properties": {"city": {"type": "string"}},
            "required": ["city"],
            "additionalProperties": false
        }),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0
}

/// What the mock executor answers for [`lookup`].
fn weather() -> Json {
    json!({"temperature": 21})
}

/// A streaming mock LLM taking input while it generates, which proposes to look up Berlin at the
/// end of each of its first `proposals` model requests.
fn generator(tool: &Tool, proposals: usize) -> MockGenerator {
    MockGenerator::new(
        MockConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        Vec::<String>::new(),
    )
    .proposing(tool.tool_id(), json!({"city": "Berlin"}), proposals)
}

/// A run whose drive loop ended, with what the test holds of it.
struct Finished {
    state: RunState,
    instances: Vec<Option<NodeInstanceState>>,
    model: ModelHandle,
    authority: AuthorityHandle,
    log: RunLog,
    received: Vec<EventEnvelope<Payload>>,
}

/// Runs the pipeline planned with the iteration rule `bound` once: the source releases [`INPUT`]
/// and its end of input, the LLM node answers on a generator proposing `proposals` times, and the
/// drive loop runs until the run ends, or for [`PATIENCE`] before the case cancels it and fails.
fn run(bound: NonZeroUsize, proposals: usize) -> Finished {
    run_inputs(bound, proposals, &[INPUT])
}

/// The authority of the run `run_id` of `planned`, knowing `tool`, enforcing the iteration rule
/// the pipeline declares for it and sending its rejections back to the LLM node.
fn authority(
    run_id: RunId,
    tool: &Tool,
    declaration: &Declaration,
    planned: &PlannedPipeline,
    ids: Ids,
) -> ToolAuthority {
    ToolAuthority::new(run_id, [tool.clone()])
        .bounded_by(declaration, planned, ids.authority)
        .expect("the pipeline declares a feedback loop into the authority")
        .rejecting_on(ids.rejected)
}

/// As [`run`], the source releasing `inputs` at once, then its end of input.
fn run_inputs(bound: NonZeroUsize, proposals: usize, inputs: &[&str]) -> Finished {
    let tool = lookup();
    let generator = generator(&tool, proposals);
    let (declaration, planned, ids) = planned(&generator, bound);
    let run_id = RunId::generate();
    let log = RunLog::new();
    let held: Vec<_> = inputs
        .iter()
        .map(|text| {
            (
                "inference.stream.TranscriptCommitted".to_owned(),
                None,
                Payload::Stream(StreamEvent::TranscriptCommitted(TranscriptCommitted {
                    span_id: SpanId::generate(),
                    text: (*text).to_owned(),
                })),
            )
        })
        .collect();
    let source = TextSource::scripted(run_id, held);
    let model = ModelNode::carrying(run_id, generator, reply, log.clone()).with_tools(
        [tool.clone()],
        ids.tool_call,
        ids.tool_result,
    );
    let authority = authority(run_id, &tool, &declaration, &planned, ids);
    let executor = ToolExecutorNode::new(
        run_id,
        Arc::new(MockToolExecutor::new([(
            tool.tool_id(),
            Script::Result(weather()),
        )])),
        [tool.tool_id()],
        PATIENCE,
    );
    let sink = TextSink::<Payload>::default();
    let (source_handle, model_handle, authority_handle, sink_handle) = (
        source.handle(),
        model.handle(),
        authority.handle(),
        sink.handle(),
    );
    let mut implementations: HashMap<NodeId, Box<dyn NodeImplementation<Payload>>> =
        HashMap::from([
            (
                ids.source,
                Box::new(source) as Box<dyn NodeImplementation<Payload>>,
            ),
            (ids.model, Box::new(model)),
            (ids.authority, Box::new(authority)),
            (ids.executor, Box::new(executor)),
            (ids.sink, Box::new(sink)),
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
    .expect("the pipeline is planned and no edge cancels or supersedes");
    controller.validate().expect("a Created run validates");
    block_on(controller.prepare()).expect("every node prepares");
    block_on(controller.activate()).expect("every node starts");
    assert_eq!(controller.run().state(), RunState::Active);
    let requests = controller.requests();
    let drive: JoinHandle<(RunController<Payload>, Result<(), RunError>)> =
        thread::spawn(move || {
            let result = block_on(controller.drive());
            (controller, result)
        });
    for _ in inputs {
        assert!(source_handle.release(), "the source holds its input");
    }
    assert!(source_handle.release_end(), "the source admits its end");
    let ended = within(&drive, &requests);
    let (controller, result) = drive.join().expect("the drive loop does not panic");
    assert!(ended, "the run ended within {PATIENCE:?}");
    result.expect("the drive loop ends the run");
    Finished {
        state: controller.run().state(),
        instances: ids
            .nodes()
            .iter()
            .map(|node_id| controller.instance(*node_id).map(|i| i.state()))
            .collect(),
        model: model_handle,
        authority: authority_handle,
        log,
        received: sink_handle.received(),
    }
}

/// Whether the drive loop ended within [`PATIENCE`]; when it did not, the run is cancelled so
/// that the case fails instead of hanging.
fn within(
    drive: &JoinHandle<(RunController<Payload>, Result<(), RunError>)>,
    requests: &RunRequests,
) -> bool {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if drive.is_finished() {
            return true;
        }
        thread::sleep(Duration::from_millis(5));
    }
    requests.cancel();
    false
}

impl Finished {
    /// The generation the LLM node started, from its `GenerationStarted`.
    fn generation(&self) -> GenerationId {
        self.log
            .envelopes()
            .iter()
            .find_map(|envelope| match &envelope.payload {
                StreamEvent::GenerationStarted(started) => Some(started.generation_id),
                _ => None,
            })
            .expect("the LLM node started a generation")
    }

    /// The text of every `TextDelta` the LLM node emitted, with the generation its envelope names.
    fn deltas(&self) -> Vec<(String, Option<GenerationId>)> {
        self.log
            .envelopes()
            .into_iter()
            .filter_map(|envelope| match envelope.payload {
                StreamEvent::TextDelta(delta) => Some((delta.text, envelope.generation_id)),
                _ => None,
            })
            .collect()
    }

    /// The generations named by every `GenerationCompleted` the LLM node emitted.
    fn completed(&self) -> Vec<Option<GenerationId>> {
        self.log
            .envelopes()
            .into_iter()
            .filter(|envelope| matches!(envelope.payload, StreamEvent::GenerationCompleted(_)))
            .map(|envelope| envelope.generation_id)
            .collect()
    }

    /// The text of every `TextDelta` the sink received.
    fn delivered(&self) -> Vec<String> {
        self.received
            .iter()
            .filter_map(|envelope| match &envelope.payload {
                Payload::Stream(StreamEvent::TextDelta(delta)) => Some(delta.text.clone()),
                _ => None,
            })
            .collect()
    }

    /// The run is Completed and every node instance Stopped.
    fn assert_completed(&self) {
        assert_eq!(self.state, RunState::Completed);
        assert_eq!(
            self.instances,
            vec![Some(NodeInstanceState::Stopped); 5],
            "every node instance is Stopped"
        );
    }
}

#[test]
fn the_proposal_is_executed_and_its_result_reinjected_on_the_generation_that_proposed() {
    let finished = run(NonZeroUsize::new(2).expect("2"), 1);
    finished.assert_completed();
    let generation = finished.generation();
    let decisions = finished.authority.decisions();
    assert_eq!(decisions.len(), 1, "one proposal reached the authority");
    assert!(
        matches!(decisions[0].outcome, AuthorityOutcome::Authorized(_)),
        "{:?}",
        decisions[0].outcome
    );
    assert_eq!(
        decisions[0].call.generation_id(),
        generation,
        "the proposal belongs to the generation that made it"
    );
    let result = weather().to_string();
    assert_eq!(
        finished.deltas(),
        vec![
            (reply(INPUT), Some(generation)),
            (result.clone(), Some(generation)),
        ],
        "the text after the result contains it and carries the proposing generation"
    );
    assert_eq!(finished.delivered(), vec![reply(INPUT), result]);
    assert_eq!(finished.completed(), vec![Some(generation)]);
    assert_eq!(
        finished.model.generation_state(),
        Some(GenerationState::Completed)
    );
}

#[test]
fn the_proposal_past_the_iteration_bound_is_rejected_exhausted_and_the_generation_completes() {
    for bound in 1..=3 {
        let finished = run(NonZeroUsize::new(bound).expect("positive"), bound + 1);
        finished.assert_completed();
        let generation = finished.generation();
        let decisions = finished.authority.decisions();
        assert_eq!(
            decisions.len(),
            bound + 1,
            "bound {bound}: every proposal was decided"
        );
        for (number, decision) in decisions.iter().enumerate() {
            assert_eq!(decision.call.generation_id(), generation);
            if number < bound {
                assert!(
                    matches!(decision.outcome, AuthorityOutcome::Authorized(_)),
                    "bound {bound}, proposal {}: {:?}",
                    number + 1,
                    decision.outcome
                );
            } else {
                assert!(
                    matches!(
                        &decision.outcome,
                        AuthorityOutcome::Rejected(rejected)
                            if rejected.reason == RejectionReason::Exhausted
                    ),
                    "bound {bound}, proposal {}: {:?}",
                    number + 1,
                    decision.outcome
                );
            }
        }
        let result = weather().to_string();
        let mut expected = vec![(reply(INPUT), Some(generation))];
        expected.extend(std::iter::repeat_n((result, Some(generation)), bound));
        assert_eq!(
            finished.deltas(),
            expected,
            "bound {bound}: one result text per authorized round, none after the rejection"
        );
        assert_eq!(finished.completed(), vec![Some(generation)]);
        assert_eq!(
            finished.model.generation_state(),
            Some(GenerationState::Completed),
            "bound {bound}"
        );
    }
}

#[test]
fn the_pipeline_validates_with_the_bound_declared_and_is_refused_without_it() {
    let tool = lookup();
    let generator = generator(&tool, 1);
    let one = NonZeroUsize::MIN;

    let (mut bounded, _, ids) = declared(&generator, Feedback::OnProposals(one));
    assert_eq!(
        bounded.validate_pipeline(ValidatePipeline {
            pipeline_id: ids.pipeline
        }),
        Ok(inference_pipeline::PipelineValidated {
            pipeline_id: ids.pipeline
        })
    );

    for feedback in [Feedback::Undeclared, Feedback::OnInput(one)] {
        let (mut unbounded, _, ids) = declared(&generator, feedback);
        assert_eq!(
            unbounded.validate_pipeline(ValidatePipeline {
                pipeline_id: ids.pipeline
            }),
            Err(ValidatePipelineError::Rejected(IncompatibleGraph {
                findings: vec![GraphFinding {
                    defect: GraphDefect::UnboundedCycle,
                    node_id: Some(ids.model),
                    port_id: None,
                }],
            })),
            "{feedback:?}"
        );
        assert_eq!(
            unbounded.pipeline(ids.pipeline).map(|p| p.state()),
            Some(PipelineState::Declared),
            "{feedback:?}: the refused pipeline stays Declared"
        );
    }
}

#[test]
fn the_authority_enforces_the_iteration_rule_the_pipeline_declares() {
    let five = NonZeroUsize::new(5).expect("5");
    let finished = run(five, 6);
    finished.assert_completed();
    let outcomes: Vec<bool> = finished
        .authority
        .decisions()
        .iter()
        .map(|decision| matches!(decision.outcome, AuthorityOutcome::Authorized(_)))
        .collect();
    assert_eq!(
        outcomes,
        vec![true, true, true, true, true, false],
        "a declaration of 5 authorizes five rounds and rejects the sixth"
    );

    let tool = lookup();
    let generator = generator(&tool, 1);
    let (declaration, planned, ids) = planned(&generator, five);
    let run_id = RunId::generate();
    let mut disagreeing = authority(run_id, &tool, &declaration, &planned, ids)
        .with_iteration_bound(NonZeroUsize::new(6).expect("6"));
    let refused = block_on(NodeImplementation::<Payload>::prepare(&mut disagreeing));
    assert!(
        refused.is_err(),
        "an authority given another rule than the declared one refuses to prepare"
    );
    let mut agreeing =
        authority(run_id, &tool, &declaration, &planned, ids).with_iteration_bound(five);
    assert_eq!(
        block_on(NodeImplementation::<Payload>::prepare(&mut agreeing)),
        Ok(()),
        "the declared rule given again agrees with it"
    );
    assert_eq!(
        ToolAuthority::new(run_id, [tool])
            .bounded_by(&declaration, &planned, ids.model)
            .err(),
        Some(IterationRuleNotDeclared {
            pipeline_id: ids.pipeline,
            node_id: ids.model,
        }),
        "no feedback loop of the pipeline bounds an edge into the LLM node"
    );
}

#[test]
fn an_input_after_the_iteration_rule_completed_the_generation_is_dropped_and_recorded() {
    let finished = run_inputs(NonZeroUsize::MIN, 2, &[INPUT, "and in Paris"]);
    finished.assert_completed();
    assert_eq!(finished.model.failure(), None);
    assert_eq!(
        finished.model.dropped_inputs(),
        vec!["and in Paris".to_owned()],
        "the input deferred during the round is dropped once the rule completed the generation"
    );
    assert_eq!(finished.completed(), vec![Some(finished.generation())]);
}
