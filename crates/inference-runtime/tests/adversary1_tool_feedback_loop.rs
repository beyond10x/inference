//! Adversary pass 1 on the LLM to tool feedback loop (`story:tool-feedback-loop`): the loop's
//! pipeline of `tests/tool_feedback_loop.rs`, driven where its cases do not go.
//!
//! * A drain the caller asks for while a tool round is in flight: the run drains, "in-flight
//!   events complete" (`inference.graph.RunDraining`), so the round's outcome is reinjected into
//!   the generation before it completes.
//! * A second input arriving while the generation awaits the outcome of its first call: every
//!   call the executor ran has its outcome reinjected into the generation that proposed it.
//! * An input arriving after the iteration bound completed the generation: the run still
//!   completes rather than failing.

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant};

use inference_core::envelope::ControlKind;
use inference_core::id::{GenerationId, RunId};
use inference_core::model::{Adapter, AdapterId, Capability, ModelId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_llm::Generator;
use inference_pipeline::nodes::{
    AuthorityHandle, AuthorityOutcome, ToolAuthority, ToolCallUpdate, ToolExecutorNode, ToolPayload,
};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeImplementation,
    NodeRole, OverloadPolicy, PipelineSpec, PlannedPipeline, PortId, PortTraits, modality,
};
use inference_runtime::run::RunState;
use inference_runtime::{NodeId, RunController, RunError, RunRequests};
use inference_testkit::graph::TextSink;
use inference_testkit::graph::{RunLog, SourceHandle, TextSource};
use inference_testkit::llm::{
    Carried, MockConfiguration, MockGenerator, ModelHandle, ModelNode, ModelPayload,
};
use inference_testkit::stream::StreamEvent;
use inference_testkit::tools::{MockToolExecutor, Script};
use inference_text::{SpanId, TranscriptCommitted};
use inference_tools::{DeclareTool, Json, SideEffectClass, Tool};
use serde_json::json;

const PATIENCE: Duration = Duration::from_secs(10);

/// The executor's deadline in the cases whose executor overruns it: long enough that the round is
/// still in flight when the case acts, short enough to keep the case quick.
const OVERRUN: Duration = Duration::from_millis(400);

fn reply(input: &str) -> String {
    format!("looking up {input}")
}

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

#[derive(Debug, Clone, Copy)]
struct Ids {
    source: NodeId,
    model: NodeId,
    authority: NodeId,
    executor: NodeId,
    sink: NodeId,
    tool_call: PortId,
    tool_result: PortId,
    rejected: PortId,
}

/// The pipeline of `tests/tool_feedback_loop.rs`, its feedback loop on the edge from `tool_call`
/// into the authority bounded by `bound`, validated and planned.
fn planned(generator: &MockGenerator, bound: NonZeroUsize) -> (Declaration, PlannedPipeline, Ids) {
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
    let (pipeline_id, ids) = declaration
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
            pipeline.connect(&emitted, &prompt, four, OverloadPolicy::Pause);
            pipeline.connect(
                &answer,
                &collected,
                NonZeroUsize::new(16).expect("16"),
                OverloadPolicy::Fail,
            );
            pipeline.connect_feedback(&tool_call, &proposed, four, OverloadPolicy::Fail, bound);
            pipeline.connect(&authorized, &calls, four, OverloadPolicy::Fail);
            pipeline.connect(&outcomes, &tool_result, four, OverloadPolicy::Fail);
            pipeline.connect(&rejected, &tool_result, four, OverloadPolicy::Fail);
            pipeline
                .bind(&model, adapter_id)
                .expect("the catalog holds the mock LLM's adapter");
            (
                pipeline.pipeline_id(),
                Ids {
                    source: source.id(),
                    model: model.id(),
                    authority: authority.id(),
                    executor: executor.id(),
                    sink: sink.id(),
                    tool_call: tool_call.id(),
                    tool_result: tool_result.id(),
                    rejected: rejected.id(),
                },
            )
        })
        .map(|(_, built)| built)
        .expect("the profile is declared");
    let validated = declaration
        .validate(pipeline_id)
        .expect("a cycle through a bounded feedback edge is valid");
    let planned = declaration
        .plan(validated, &catalog)
        .expect("the LLM node is bound to an adapter that streams and calls tools");
    (declaration, planned, ids)
}

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

fn weather() -> Json {
    json!({"temperature": 21})
}

/// A run of the loop, Active, its drive loop on a thread of its own.
struct Running {
    source: SourceHandle<Payload>,
    model: ModelHandle,
    authority: AuthorityHandle,
    log: RunLog,
    requests: RunRequests,
    drive: JoinHandle<(RunController<Payload>, Result<(), RunError>)>,
    ids: Ids,
}

/// Starts a run whose source holds `inputs`, whose generator proposes at the end of each of its
/// first `proposals` model requests, whose authority enforces `bound` and whose executor answers
/// `script` under `deadline`.
fn start(
    inputs: &[&str],
    bound: usize,
    proposals: usize,
    script: Script,
    deadline: Duration,
) -> Running {
    let tool = lookup();
    let generator = MockGenerator::new(
        MockConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        Vec::<String>::new(),
    )
    .proposing(tool.tool_id(), json!({"city": "Berlin"}), proposals);
    let bound = NonZeroUsize::new(bound).expect("a positive bound");
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
    let authority = ToolAuthority::new(run_id, [tool.clone()])
        .with_iteration_bound(bound)
        .rejecting_on(ids.rejected);
    let executor = ToolExecutorNode::new(
        run_id,
        Arc::new(MockToolExecutor::new([(tool.tool_id(), script)])),
        [tool.tool_id()],
        deadline,
    );
    let sink = TextSink::<Payload>::default();
    let (source_handle, model_handle, authority_handle) =
        (source.handle(), model.handle(), authority.handle());
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
    .expect("the pipeline is planned");
    controller.validate().expect("a Created run validates");
    block_on(controller.prepare()).expect("every node prepares");
    block_on(controller.activate()).expect("every node starts");
    assert_eq!(controller.run().state(), RunState::Active);
    let requests = controller.requests();
    let drive = thread::spawn(move || {
        let result = block_on(controller.drive());
        (controller, result)
    });
    Running {
        source: source_handle,
        model: model_handle,
        authority: authority_handle,
        log,
        requests,
        drive,
        ids,
    }
}

/// Waits at most [`PATIENCE`] until `ready` holds.
fn wait_until(mut ready: impl FnMut() -> bool) -> bool {
    let until = Instant::now() + PATIENCE;
    while Instant::now() < until {
        if ready() {
            return true;
        }
        thread::sleep(Duration::from_millis(2));
    }
    false
}

/// What the run came to.
struct Ended {
    state: Option<RunState>,
    /// The run's failure reason, with the failed node named by its role in [`Ids`].
    failure: Option<String>,
    model: ModelHandle,
    authority: AuthorityHandle,
    log: RunLog,
}

impl Running {
    fn authorized(&self) -> usize {
        authorized(&self.authority)
    }

    /// The run's end, once its drive loop ended within [`PATIENCE`]; cancels it otherwise, and
    /// the state is then `None`.
    fn ended(self) -> Ended {
        let ended = wait_until(|| self.drive.is_finished());
        if !ended {
            self.requests.cancel();
        }
        let (controller, _) = self.drive.join().expect("the drive loop does not panic");
        let ids = self.ids;
        let role = |node_id: Option<NodeId>| {
            [
                (ids.source, "source"),
                (ids.model, "model"),
                (ids.authority, "authority"),
                (ids.executor, "executor"),
                (ids.sink, "sink"),
            ]
            .into_iter()
            .find(|(id, _)| Some(*id) == node_id)
            .map_or("no node", |(_, role)| role)
        };
        Ended {
            state: ended.then(|| controller.run().state()),
            failure: controller
                .run()
                .failure()
                .map(|failure| format!("{}: {}", role(failure.node_id), failure.detail)),
            model: self.model,
            authority: self.authority,
            log: self.log,
        }
    }
}

fn authorized(authority: &AuthorityHandle) -> usize {
    authority
        .decisions()
        .iter()
        .filter(|decision| matches!(decision.outcome, AuthorityOutcome::Authorized(_)))
        .count()
}

impl Ended {
    /// The text of every `TextDelta` the LLM node emitted.
    fn deltas(&self) -> Vec<String> {
        self.log
            .envelopes()
            .into_iter()
            .filter_map(|envelope| match envelope.payload {
                StreamEvent::TextDelta(delta) => Some(delta.text),
                _ => None,
            })
            .collect()
    }

    /// How many texts the LLM node emitted for an outcome it integrated: a result's JSON or a
    /// failure's detail.
    fn integrated(&self) -> usize {
        let result = weather().to_string();
        self.deltas()
            .iter()
            .filter(|text| **text == result || text.starts_with("the tool failed:"))
            .count()
    }
}

#[test]
fn a_drain_while_a_tool_round_is_in_flight_reinjects_its_outcome_before_the_run_completes() {
    let running = start(&["the weather in Berlin"], 2, 1, Script::Overrun, OVERRUN);
    assert!(running.source.release(), "the source holds its input");
    assert!(
        wait_until(|| running.authorized() == 1),
        "the proposal was authorized and is with the executor"
    );
    running.requests.drain();
    let ended = running.ended();
    assert_eq!(
        ended.state,
        Some(RunState::Completed),
        "the drained run completes; run failure: {:?}",
        ended.failure
    );
    assert_eq!(
        ended.integrated(),
        1,
        "the outcome of the round in flight at the drain is reinjected; deltas: {:?}",
        ended.deltas()
    );
}

#[test]
fn a_second_input_during_a_tool_round_loses_no_outcome_of_a_call_the_executor_ran() {
    let running = start(
        &["the weather in Berlin", "and in Paris"],
        3,
        2,
        Script::Overrun,
        OVERRUN,
    );
    assert!(running.source.release(), "the source holds its first input");
    assert!(
        wait_until(|| running.authorized() == 1),
        "the first proposal was authorized and is with the executor"
    );
    assert!(
        running.source.release(),
        "the source holds its second input"
    );
    assert!(running.source.release_end(), "the source admits its end");
    let ended = running.ended();
    assert_eq!(
        ended.state,
        Some(RunState::Completed),
        "run failure: {:?}",
        ended.failure
    );
    let executed = authorized(&ended.authority);
    assert_eq!(
        ended.integrated(),
        executed,
        "every one of the {executed} authorized calls has its outcome reinjected; deltas: {:?}",
        ended.deltas()
    );
}

#[test]
fn an_input_after_the_bound_completed_the_generation_does_not_fail_the_run() {
    let running = start(
        &["the weather in Berlin", "and in Paris"],
        1,
        2,
        Script::Result(weather()),
        PATIENCE,
    );
    assert!(running.source.release(), "the source holds its first input");
    let log = running.log.clone();
    assert!(
        wait_until(|| log
            .envelopes()
            .iter()
            .any(|envelope| matches!(envelope.payload, StreamEvent::GenerationCompleted(_)))),
        "the exhausted rejection completed the generation"
    );
    assert!(
        running.source.release(),
        "the source holds its second input"
    );
    assert!(running.source.release_end(), "the source admits its end");
    let ended = running.ended();
    assert_eq!(
        ended.model.failure(),
        None,
        "an input after the termination rule fired does not fail the model node"
    );
    assert_eq!(ended.state, Some(RunState::Completed));
}
