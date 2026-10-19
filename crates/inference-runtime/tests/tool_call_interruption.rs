//! Interruption during a tool round trip (`story:tool-call-interruption`;
//! `docs/design/inference.md` 376, 632-641, 852, 856, 1175).
//!
//! The loop pipeline of `story:tool-feedback-loop` on mocks: source to the mock LLM node to sink,
//! and the loop from the LLM node's `tool_call` port through the tool authority and the tool
//! executor back to its `tool_result` port, the authority's rejections returning to the same port.
//! The executor answers to the authority: it starts each call on the record the authority keeps.
//! The mock LLM proposes at the end of its first model request; the test supersedes the
//! generation through the LLM node (`inference.stream.SupersedeGeneration`) with the call in each
//! of the four states in turn:
//!
//! | state at the supersede | how | the call ends |
//! |---|---|---|
//! | Proposed | a tool that requires confirmation, the hook left unanswered | Rejected `superseded`, the confirmation withdrawn |
//! | Authorized | two calls in one round, the first held Started by the mock executor | the second Rejected `superseded` before the first is released |
//! | Started | the held call, scripted to finish on release | Completed, after a `cancelled` entry in the mock's log |
//! | Completed | the call released and Completed before the supersede | Completed, unchanged |
//!
//! In every case no segment of the superseded generation is emitted after its
//! `GenerationSuperseded`, an outcome arriving after the supersede is not reinjected and issues no
//! model request, and the `inference.tools.ToolCalls` view, read from the record the authority and
//! the executor share, lists each call with its state. Check 1 is the Started case with the held
//! call scripted to fail on release.
//!
//! The last case closes a gap the tool nodes left (wave 14): a consumer node whose exchange fails
//! drops its input edge, the producer's end of input is refused as closed, and the run, which
//! never sees its input end, ends Failed through `fail_node` naming the node instead of waiting
//! for a cancel.

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant};

use inference_core::envelope::{ControlKind, EventEnvelope};
use inference_core::generation::{Generation, StartGeneration};
use inference_core::id::{GenerationId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId, Capability, ModelId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_llm::Generator;
use inference_llm::tools::ToolProposal;
use inference_pipeline::nodes::{
    AuthorityHandle, AuthorityOutcome, ExecutorHandle, ToolAuthority, ToolCallEvent,
    ToolCallUpdate, ToolExecutorNode, ToolPayload,
};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeImplementation,
    NodeRole, OverloadPolicy, PipelineId, PipelineSpec, PlannedPipeline, PortId, PortTraits,
    modality,
};
use inference_runtime::run::{NodeInstanceState, RunState};
use inference_runtime::{NodeId, RunController, RunError, RunRequests};
use inference_testkit::graph::{RunLog, SinkHandle, SourceHandle, TextSink, TextSource};
use inference_testkit::llm::{
    Carried, MockConfiguration, MockGenerator, ModelHandle, ModelNode, ModelPayload,
};
use inference_testkit::stream::StreamEvent;
use inference_testkit::tools::{Entry, MockToolExecutor, Script, ToolCallsRow};
use inference_text::{OpenTurn, Span, SpanId, TranscriptCommitted, Turn};
use inference_tools::{
    DeclareTool, Json, RejectionReason, SideEffectClass, Tool, ToolCall, ToolCallFailed,
    ToolCallId, ToolCallRejected, ToolCallResult, ToolCallState, ToolCallStateConflict,
    ToolFailure, ToolFailureKind,
};
use serde_json::json;

/// How long a case waits for what it waits on before it fails rather than hangs, and the
/// executor's deadline.
const PATIENCE: Duration = Duration::from_secs(10);

/// The one input the source holds.
const INPUT: &str = "book a table in Berlin";

/// The mock LLM's answer to an input text.
fn reply(input: &str) -> String {
    format!("on it: {input}")
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

/// Whether `ready` holds within [`PATIENCE`], asked every few milliseconds.
fn eventually(ready: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if ready() {
            return true;
        }
        thread::sleep(Duration::from_millis(2));
    }
    ready()
}

/// The profile a pipeline is declared under and, if it has a model node, the adapter it binds to.
struct Fixture {
    profile: DeploymentProfile,
    adapter: Option<Adapter>,
}

impl Catalog for Fixture {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.profile.profile_id() == profile_id).then_some(&self.profile)
    }

    fn adapter(&self, adapter_id: AdapterId) -> Option<&Adapter> {
        self.adapter
            .as_ref()
            .filter(|adapter| adapter.adapter_id() == adapter_id)
    }
}

/// A profile admitting nothing, and a catalog of it and `adapter`.
fn fixture(adapter: Option<Adapter>) -> (ProfileId, Fixture) {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
    })
    .expect("a row of the table");
    (profile.profile_id(), Fixture { profile, adapter })
}

/// A Structured, reliable port that carries the end of input.
const STRUCTURED: PortTraits = PortTraits {
    essentiality: Essentiality::Essential,
    delivery: Delivery::Reliable,
    incremental: false,
    end_marker_required: true,
    provisional_allowed: false,
};

/// The nodes and the ports a run of the loop pipeline names.
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

/// The loop pipeline of `story:tool-feedback-loop`, its LLM node bound to the adapter of
/// `generator`, the edge from `tool_call` into the authority bounded by a feedback loop of four
/// rounds; validated and planned.
fn planned(generator: &MockGenerator) -> (Declaration, PlannedPipeline, Ids) {
    let (profile_id, catalog) = fixture(Some(generator.adapter().clone()));
    let adapter_id = generator.adapter().adapter_id();
    let text = PortTraits {
        incremental: true,
        ..STRUCTURED
    };
    let spec = PipelineSpec {
        name: "text with tools, interrupted".to_owned(),
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
                pipeline.output::<modality::Structured>(&model, "tool_call", STRUCTURED);
            let tool_result =
                pipeline.input::<modality::Structured>(&model, "tool_result", STRUCTURED);
            let proposed =
                pipeline.input::<modality::Structured>(&authority, "proposed", STRUCTURED);
            let authorized =
                pipeline.output::<modality::Structured>(&authority, "authorized", STRUCTURED);
            let rejected =
                pipeline.output::<modality::Structured>(&authority, "rejected", STRUCTURED);
            let calls = pipeline.input::<modality::Structured>(&executor, "authorized", STRUCTURED);
            let outcomes =
                pipeline.output::<modality::Structured>(&executor, "outcomes", STRUCTURED);
            let collected = pipeline.input::<modality::Text>(&sink, "text", text);
            pipeline.connect(&emitted, &prompt, four, OverloadPolicy::Pause);
            pipeline.connect(
                &answer,
                &collected,
                NonZeroUsize::new(16).expect("16"),
                OverloadPolicy::Fail,
            );
            pipeline.connect_feedback(&tool_call, &proposed, four, OverloadPolicy::Fail, four);
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
    let validated = declaration
        .validate(ids.pipeline)
        .expect("a cycle through a bounded feedback edge is valid");
    let planned = declaration
        .plan(validated, &catalog)
        .expect("the LLM node is bound to an adapter that streams and calls tools");
    (declaration, planned, ids)
}

/// A tool named `name` of `side_effect`, requiring confirmation when `confirmed` says so.
fn tool(name: &str, side_effect: SideEffectClass, confirmed: bool) -> Tool {
    Tool::declare(DeclareTool {
        name: name.to_owned(),
        schema: json!({
            "type": "object",
            "properties": {"city": {"type": "string"}},
            "required": ["city"],
            "additionalProperties": false
        }),
        schema_revision: "1".to_owned(),
        side_effect,
        requires_confirmation: confirmed,
    })
    .0
}

/// The proposal to call `tool` for `city`.
fn proposal(tool: &Tool, city: &str) -> ToolProposal {
    ToolProposal {
        tool_id: tool.tool_id(),
        arguments: json!({"city": city}),
    }
}

/// What the held tools answer on release.
fn booked() -> Json {
    json!({"booked": true})
}

/// The failure a held tool scripted to fail answers on release.
fn backend_down() -> ToolFailure {
    ToolFailure {
        kind: ToolFailureKind::ExecutionError,
        detail: "the booking backend went away".to_owned(),
    }
}

/// One run of the loop pipeline, Active, with its drive loop on a thread of its own.
struct Looping {
    ids: Ids,
    source: SourceHandle<Payload>,
    model: ModelHandle,
    authority: AuthorityHandle,
    executor: ExecutorHandle,
    mock: MockToolExecutor,
    sink: SinkHandle<Payload>,
    log: RunLog,
    requests: RunRequests,
    drive: JoinHandle<(RunController<Payload>, Result<(), RunError>)>,
}

/// Starts a run of the loop pipeline whose LLM node knows `tools` and whose generator ends its
/// first model request with the proposals of `round`, executed on `mock`; the source releases
/// [`INPUT`] at once and holds its end of input.
fn start(tools: &[Tool], round: Vec<ToolProposal>, mock: &MockToolExecutor) -> Looping {
    let generator = MockGenerator::new(
        MockConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        Vec::<String>::new(),
    )
    .proposing_round(round, 1);
    let (declaration, planned, ids) = planned(&generator);
    let run_id = RunId::generate();
    let log = RunLog::new();
    let source = TextSource::scripted(
        run_id,
        [(
            "inference.stream.TranscriptCommitted".to_owned(),
            None,
            Payload::Stream(StreamEvent::TranscriptCommitted(TranscriptCommitted {
                span_id: SpanId::generate(),
                text: INPUT.to_owned(),
            })),
        )],
    );
    let model = ModelNode::carrying(run_id, generator, reply, log.clone()).with_tools(
        tools.iter().cloned(),
        ids.tool_call,
        ids.tool_result,
    );
    let authority = ToolAuthority::new(run_id, tools.iter().cloned())
        .bounded_by(&declaration, &planned, ids.authority)
        .expect("the pipeline declares a feedback loop into the authority")
        .rejecting_on(ids.rejected);
    let executor = ToolExecutorNode::new(
        run_id,
        Arc::new(mock.clone()),
        tools.iter().map(Tool::tool_id),
        PATIENCE,
    )
    .answering_to(&authority.handle());
    let sink = TextSink::<Payload>::default();
    let looping_handles = (
        source.handle(),
        model.handle(),
        authority.handle(),
        executor.handle(),
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
    let drive = thread::spawn(move || {
        let result = block_on(controller.drive());
        (controller, result)
    });
    let (source, model, authority, executor, sink) = looping_handles;
    assert!(source.release(), "the source holds its input");
    Looping {
        ids,
        source,
        model,
        authority,
        executor,
        mock: mock.clone(),
        sink,
        log,
        requests,
        drive,
    }
}

impl Looping {
    /// The call the mock was invoked with `number`-th, once it was, within [`PATIENCE`].
    fn invoked(&self, number: usize) -> ToolCallId {
        assert!(
            eventually(|| self.mock.invocations().len() > number),
            "the mock executor was invoked {} times, not {}",
            self.mock.invocations().len(),
            number + 1
        );
        self.mock.invocations()[number].call_id
    }

    /// Runs `inference.stream.SupersedeGeneration` on the LLM node's generation, then waits until
    /// its `GenerationSuperseded` is in the run's log.
    fn supersede(&self) {
        assert!(self.model.supersede(), "the LLM node takes the supersede");
        assert!(
            eventually(|| superseded_at(&self.log.envelopes()).is_some()),
            "the LLM node emitted no GenerationSuperseded"
        );
    }

    /// Waits until the authority decided `call_id` Rejected `superseded`.
    fn rejected_superseded(&self, call_id: ToolCallId) -> bool {
        eventually(|| {
            self.authority.decisions().iter().any(|decision| {
                decision.call.call_id() == call_id
                    && matches!(
                        decision.outcome,
                        AuthorityOutcome::Rejected(ToolCallRejected {
                            reason: RejectionReason::Superseded,
                            ..
                        })
                    )
            })
        })
    }

    /// Releases the source's end of input, waits for the drive loop to end the run, or for
    /// [`PATIENCE`] before it cancels the run and the case fails, and takes what the run left.
    fn finish(self) -> Finished {
        assert!(self.source.release_end(), "the source admits its end");
        let ended = eventually(|| self.drive.is_finished());
        if !ended {
            self.requests.cancel();
        }
        let (controller, result) = self.drive.join().expect("the drive loop does not panic");
        assert!(ended, "the run ended within {PATIENCE:?}");
        result.expect("the drive loop ends the run");
        Finished {
            state: controller.run().state(),
            instances: self
                .ids
                .nodes()
                .iter()
                .map(|node_id| controller.instance(*node_id).map(|i| i.state()))
                .collect(),
            model: self.model,
            authority: self.authority,
            executor: self.executor,
            mock: self.mock,
            log: self.log,
            received: self.sink.received(),
        }
    }
}

/// A run of the loop pipeline whose drive loop ended, with what the test holds of it.
struct Finished {
    state: RunState,
    instances: Vec<Option<NodeInstanceState>>,
    model: ModelHandle,
    authority: AuthorityHandle,
    executor: ExecutorHandle,
    mock: MockToolExecutor,
    log: RunLog,
    received: Vec<EventEnvelope<Payload>>,
}

/// The position of the first `GenerationSuperseded` among `envelopes`, if there is one.
fn superseded_at(envelopes: &[EventEnvelope<StreamEvent>]) -> Option<usize> {
    envelopes
        .iter()
        .position(|envelope| matches!(envelope.payload, StreamEvent::GenerationSuperseded(_)))
}

/// Whether `event` is output of a generation: a delta, a committed segment or its completion.
fn output(event: &StreamEvent) -> bool {
    matches!(
        event,
        StreamEvent::TextDelta(_)
            | StreamEvent::TextSegmentCommitted(_)
            | StreamEvent::GenerationCompleted(_)
    )
}

impl Finished {
    /// The run is Completed and every node instance Stopped.
    fn assert_completed(&self) {
        assert_eq!(self.state, RunState::Completed);
        assert_eq!(
            self.instances,
            vec![Some(NodeInstanceState::Stopped); 5],
            "every node instance is Stopped"
        );
    }

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

    /// The generation was superseded, by `GenerationSuperseded` naming it, and neither the run's
    /// log nor the sink holds output of it after that event.
    fn assert_nothing_emitted_after_the_supersede(&self) {
        let generation = self.generation();
        let envelopes = self.log.envelopes();
        let at = superseded_at(&envelopes).expect("the generation was superseded");
        assert_eq!(envelopes[at].generation_id, Some(generation));
        let late: Vec<&str> = envelopes[at + 1..]
            .iter()
            .filter(|envelope| output(&envelope.payload))
            .map(|envelope| envelope.kind.as_str())
            .collect();
        assert!(late.is_empty(), "emitted after the supersede: {late:?}");
        let delivered = self
            .received
            .iter()
            .position(|envelope| {
                matches!(
                    envelope.payload,
                    Payload::Stream(StreamEvent::GenerationSuperseded(_))
                )
            })
            .expect("the sink received the GenerationSuperseded");
        let late: Vec<&str> = self.received[delivered + 1..]
            .iter()
            .filter(|envelope| matches!(&envelope.payload, Payload::Stream(event) if output(event)))
            .map(|envelope| envelope.kind.as_str())
            .collect();
        assert!(late.is_empty(), "delivered after the supersede: {late:?}");
        assert_eq!(self.model.failure(), None, "the LLM node does not fail");
    }

    /// The `inference.tools.ToolCalls` view of the run: one row per call the authority and the
    /// executor recorded, in the order they were proposed.
    fn view(&self) -> Vec<ToolCallsRow> {
        self.authority
            .calls()
            .iter()
            .map(|call| ToolCallsRow {
                call_id: call.call_id(),
                tool_id: call.tool_id(),
                generation_id: call.generation_id(),
                side_effect: call.side_effect(),
                basis_span_id: call.basis_span_id(),
                arguments: call.arguments().clone(),
                state: call.state(),
            })
            .collect()
    }

    /// The row of `call_id` in the view.
    fn row(&self, call_id: ToolCallId) -> ToolCallsRow {
        self.view()
            .into_iter()
            .find(|row| row.call_id == call_id)
            .unwrap_or_else(|| panic!("the view lists no call {call_id}"))
    }

    /// The call `call_id` as the shared record holds it.
    fn call(&self, call_id: ToolCallId) -> ToolCall {
        self.authority
            .calls()
            .into_iter()
            .find(|call| call.call_id() == call_id)
            .unwrap_or_else(|| panic!("the record holds no call {call_id}"))
    }

    /// The call's identity and the event of every outcome the LLM node took after its
    /// generation was no longer Active, none of them reinjected.
    fn not_reinjected(&self) -> Vec<(ToolCallId, ToolCallEvent)> {
        self.model
            .not_reinjected()
            .into_iter()
            .map(|update| (update.call.call_id(), update.event))
            .collect()
    }
}

/// The Proposed case: the call of a tool that requires confirmation, held by the authority with
/// the hook unanswered, is rejected `superseded` and its confirmation withdrawn.
fn superseded_while_proposed() -> (Finished, ToolCallId) {
    let book = tool("book", SideEffectClass::NonIdempotentWrite, true);
    let mock = MockToolExecutor::new([(book.tool_id(), Script::Result(booked()))]);
    let run = start(
        std::slice::from_ref(&book),
        vec![proposal(&book, "Berlin")],
        &mock,
    );
    assert!(
        eventually(|| run.authority.held().len() == 1),
        "the authority holds the call for confirmation"
    );
    let call_id = run.authority.held()[0].call_id();
    run.supersede();
    assert!(
        run.rejected_superseded(call_id),
        "the held call was not rejected superseded: {:?}",
        run.authority.decisions()
    );
    assert!(
        run.authority.held().is_empty(),
        "the confirmation is withdrawn"
    );
    assert!(
        !run.authority.confirm(call_id),
        "a withdrawn confirmation confirms nothing"
    );
    (run.finish(), call_id)
}

/// The Authorized case: two calls in one round, the first held Started by the mock, the second
/// Authorized when the supersede fires and Rejected `superseded` before the first is released.
fn superseded_while_authorized() -> (Finished, ToolCallId, ToolCallId) {
    let lookup = tool("lookup", SideEffectClass::ReadOnly, false);
    let mock = MockToolExecutor::new([(lookup.tool_id(), Script::Hold(Ok(booked())))]);
    let run = start(
        std::slice::from_ref(&lookup),
        vec![proposal(&lookup, "Berlin"), proposal(&lookup, "Paris")],
        &mock,
    );
    let first = run.invoked(0);
    assert!(
        eventually(|| run.authority.decisions().len() == 2),
        "both proposals were decided: {:?}",
        run.authority.decisions()
    );
    let decisions = run.authority.decisions();
    assert!(
        decisions
            .iter()
            .all(|decision| matches!(decision.outcome, AuthorityOutcome::Authorized(_))),
        "{decisions:?}"
    );
    assert_eq!(decisions[0].call.call_id(), first, "the first call is held");
    let second = decisions[1].call.call_id();
    run.supersede();
    assert!(
        run.rejected_superseded(second),
        "the second call was not rejected superseded: {:?}",
        run.authority.decisions()
    );
    assert!(
        eventually(|| run.mock.entries().contains(&Entry::Cancelled(first))),
        "the cancellation signal did not reach the held first call: {:?}",
        run.mock.entries()
    );
    assert!(run.mock.release(first), "the first call is held");
    (run.finish(), first, second)
}

/// The Started case: the held call is cancelled by the supersede, logged `cancelled` by the mock,
/// and released to answer `answer`.
fn superseded_while_started(answer: Result<Json, ToolFailure>) -> (Finished, ToolCallId) {
    let lookup = tool("lookup", SideEffectClass::ReadOnly, false);
    let mock = MockToolExecutor::new([(lookup.tool_id(), Script::Hold(answer))]);
    let run = start(
        std::slice::from_ref(&lookup),
        vec![proposal(&lookup, "Berlin")],
        &mock,
    );
    let call_id = run.invoked(0);
    run.supersede();
    assert!(
        eventually(|| run.mock.entries().contains(&Entry::Cancelled(call_id))),
        "the cancellation signal did not reach the held call: {:?}",
        run.mock.entries()
    );
    assert!(run.mock.release(call_id), "the call is held");
    (run.finish(), call_id)
}

/// The Completed case: a write released and Completed, its result reinjected, before the
/// supersede.
fn superseded_after_completion() -> (Finished, ToolCallId) {
    let transfer = tool("transfer", SideEffectClass::NonIdempotentWrite, false);
    let mock = MockToolExecutor::new([(transfer.tool_id(), Script::Hold(Ok(booked())))]);
    let run = start(
        std::slice::from_ref(&transfer),
        vec![proposal(&transfer, "Berlin")],
        &mock,
    );
    let call_id = run.invoked(0);
    assert!(run.mock.release(call_id), "the call is held");
    let result = booked().to_string();
    assert!(
        eventually(|| run.sink.received().iter().any(|envelope| matches!(
            &envelope.payload,
            Payload::Stream(StreamEvent::TextDelta(delta)) if delta.text == result
        ))),
        "the result was reinjected before the supersede"
    );
    run.supersede();
    (run.finish(), call_id)
}

#[test]
fn superseding_a_proposed_call_rejects_it_superseded_and_withdraws_its_confirmation() {
    let (finished, call_id) = superseded_while_proposed();
    finished.assert_completed();
    finished.assert_nothing_emitted_after_the_supersede();
    let generation = finished.generation();
    let rejected: Vec<_> = finished
        .authority
        .decisions()
        .into_iter()
        .filter(|decision| matches!(decision.outcome, AuthorityOutcome::Rejected(_)))
        .collect();
    assert_eq!(rejected.len(), 1);
    assert_eq!(
        rejected[0].outcome,
        AuthorityOutcome::Rejected(ToolCallRejected {
            call_id,
            reason: RejectionReason::Superseded,
        }),
        "RejectToolCall/outcome/rejected with the reason superseded"
    );
    assert_eq!(rejected[0].call.state(), ToolCallState::Rejected);
    assert_eq!(rejected[0].call.generation_id(), generation);
    assert_eq!(finished.row(call_id).state, ToolCallState::Rejected);
    assert!(
        finished.mock.invocations().is_empty(),
        "the call never reached the executor"
    );
    assert_eq!(
        finished.not_reinjected(),
        vec![(
            call_id,
            ToolCallEvent::Rejected(ToolCallRejected {
                call_id,
                reason: RejectionReason::Superseded,
            })
        )],
        "the rejection reached the LLM node and was not reinjected"
    );
    assert_eq!(
        finished.model.model_requests(),
        1,
        "the input's request only"
    );
}

#[test]
fn superseding_an_authorized_call_rejects_it_superseded_and_the_executor_drops_its_start() {
    let (finished, first, second) = superseded_while_authorized();
    finished.assert_completed();
    finished.assert_nothing_emitted_after_the_supersede();
    assert_eq!(finished.row(second).state, ToolCallState::Rejected);
    assert_eq!(
        finished.executor.refusals().len(),
        1,
        "{:?}",
        finished.executor.refusals()
    );
    let refusal = &finished.executor.refusals()[0];
    assert_eq!(refusal.call.call_id(), second);
    assert_eq!(
        refusal.refused,
        ToolCallStateConflict {
            current: ToolCallState::Rejected
        },
        "StartToolCall of the rejected call answers wrong-state, which the executor drops"
    );
    let invoked: Vec<ToolCallId> = finished
        .mock
        .invocations()
        .iter()
        .map(|invocation| invocation.call_id)
        .collect();
    assert_eq!(invoked, vec![first], "the rejected call was never executed");
    assert!(
        finished.mock.entries().contains(&Entry::Cancelled(first)),
        "the held first call was told of the supersede"
    );
    assert_eq!(finished.row(first).state, ToolCallState::Completed);
    let reinjected: Vec<ToolCallId> = finished
        .not_reinjected()
        .into_iter()
        .map(|(call_id, _)| call_id)
        .collect();
    assert_eq!(
        reinjected,
        vec![second, first],
        "neither outcome was reinjected"
    );
    assert_eq!(finished.model.model_requests(), 1);
}

#[test]
fn superseding_a_started_call_cancels_it_and_its_result_is_not_reinjected() {
    let (finished, call_id) = superseded_while_started(Ok(booked()));
    finished.assert_completed();
    finished.assert_nothing_emitted_after_the_supersede();
    assert_eq!(finished.row(call_id).state, ToolCallState::Completed);
    let entries = finished.mock.entries();
    let cancelled = entries
        .iter()
        .position(|entry| *entry == Entry::Cancelled(call_id))
        .expect("the mock logged the cancellation of the held call");
    let released = entries
        .iter()
        .position(|entry| *entry == Entry::Released(call_id, Ok(booked())))
        .expect("the mock logged the release");
    assert!(cancelled < released, "{entries:?}");
    assert_eq!(
        finished.not_reinjected(),
        vec![(
            call_id,
            ToolCallEvent::Result(ToolCallResult {
                call_id,
                result: booked(),
            })
        )],
        "the result reached the LLM node and was not reinjected"
    );
    assert_eq!(
        finished.model.model_requests(),
        1,
        "no model request after the result"
    );
}

#[test]
fn superseding_after_completion_leaves_the_completed_call_completed() {
    let (finished, call_id) = superseded_after_completion();
    finished.assert_completed();
    finished.assert_nothing_emitted_after_the_supersede();
    let call = finished.call(call_id);
    assert_eq!(call.state(), ToolCallState::Completed);
    assert_eq!(
        call.fail(backend_down()).err(),
        Some(ToolCallStateConflict {
            current: ToolCallState::Completed
        }),
        "ToolCall/state/Completed/refuses/FailToolCall: nothing rolls the write back"
    );
    assert!(
        !finished.mock.entries().contains(&Entry::Cancelled(call_id)),
        "a completed call is not cancelled"
    );
    assert!(
        finished
            .authority
            .decisions()
            .iter()
            .all(|decision| matches!(decision.outcome, AuthorityOutcome::Authorized(_))),
        "nothing was rejected: {:?}",
        finished.authority.decisions()
    );
    assert_eq!(finished.not_reinjected(), Vec::new());
    assert_eq!(
        finished.model.model_requests(),
        2,
        "the input's request and the result's, both before the supersede"
    );
}

#[test]
fn the_tool_calls_view_lists_the_four_calls_with_their_states() {
    let (proposed, proposed_call) = superseded_while_proposed();
    let (authorized, _, authorized_call) = superseded_while_authorized();
    let (started, started_call) = superseded_while_started(Ok(booked()));
    let (completed, completed_call) = superseded_after_completion();
    let view = [
        proposed.row(proposed_call),
        authorized.row(authorized_call),
        started.row(started_call),
        completed.row(completed_call),
    ];
    let states: Vec<ToolCallState> = view.iter().map(|row| row.state).collect();
    assert_eq!(
        states,
        vec![
            ToolCallState::Rejected,
            ToolCallState::Rejected,
            ToolCallState::Completed,
            ToolCallState::Completed,
        ]
    );
    for (row, finished) in view
        .iter()
        .zip([&proposed, &authorized, &started, &completed])
    {
        assert_eq!(row.generation_id, finished.generation());
        assert_eq!(
            row.arguments["city"],
            json!(if row.call_id == authorized_call {
                "Paris"
            } else {
                "Berlin"
            })
        );
    }
}

/// Check 1: the Started case with the held call scripted to fail on release.
#[test]
fn a_started_call_failing_on_release_after_the_cancel_ends_failed_and_is_not_reinjected() {
    let (finished, call_id) = superseded_while_started(Err(backend_down()));
    finished.assert_completed();
    finished.assert_nothing_emitted_after_the_supersede();
    assert_eq!(finished.row(call_id).state, ToolCallState::Failed);
    let entries = finished.mock.entries();
    let cancelled = entries
        .iter()
        .position(|entry| *entry == Entry::Cancelled(call_id))
        .expect("the mock logged the cancellation of the held call");
    let failed = entries
        .iter()
        .position(|entry| *entry == Entry::Released(call_id, Err(backend_down())))
        .expect("the mock logged the failure it answered on release");
    assert!(cancelled < failed, "{entries:?}");
    assert_eq!(
        finished.not_reinjected(),
        vec![(
            call_id,
            ToolCallEvent::Failed(ToolCallFailed {
                call_id,
                failure: backend_down(),
            })
        )],
        "the ToolCallFailed reached the LLM node and was not reinjected"
    );
    assert_eq!(finished.model.model_requests(), 1);
}

/// Source of tool-call steps to executor to sink, every port Structured, every edge bounded and
/// failing on overload.
fn executor_pipeline() -> (Declaration, PlannedPipeline, [NodeId; 3]) {
    let (profile_id, catalog) = fixture(None);
    let spec = PipelineSpec {
        name: "a failing consumer".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let capacity = NonZeroUsize::new(4).expect("4");
    let mut declaration = Declaration::new();
    let (pipeline_id, nodes) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let source = pipeline.node("proposer", NodeRole::Source, Vec::new());
            let executor = pipeline.node("executor", NodeRole::Tool, Vec::new());
            let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
            let calls = pipeline.output::<modality::Structured>(&source, "calls", STRUCTURED);
            let taken = pipeline.input::<modality::Structured>(&executor, "authorized", STRUCTURED);
            let outcomes =
                pipeline.output::<modality::Structured>(&executor, "outcomes", STRUCTURED);
            let collected = pipeline.input::<modality::Structured>(&sink, "outcomes", STRUCTURED);
            pipeline.connect(&calls, &taken, capacity, OverloadPolicy::Fail);
            pipeline.connect(&outcomes, &collected, capacity, OverloadPolicy::Fail);
            [source.id(), executor.id(), sink.id()]
        })
        .expect("the profile is declared");
    let validated = declaration
        .validate(pipeline_id)
        .expect("source to executor to sink is valid");
    let planned = declaration
        .plan(validated, &catalog)
        .expect("no node requires a capability");
    (declaration, planned, nodes)
}

/// A call of `tool`, proposed by a generation of its own on a committed span, and not authorized.
fn proposed(tool: &Tool) -> ToolCall {
    let (generation, _) = Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    });
    let (turn, _) = Turn::open(OpenTurn {
        run_id: RunId::generate(),
    });
    let (span, _) = Span::emit(
        &turn,
        turn.run_id(),
        StreamId::generate(),
        1,
        "a request".to_owned(),
    )
    .expect("an Open turn admits a span");
    let (span, _) = span.commit().expect("a Hypothesis commits");
    ToolCall::propose(tool, &generation, &span, json!({"city": "Berlin"}))
        .expect("an Active generation proposes on a committed span")
        .0
}

/// The wave-14 gap: the executor's exchange fails on a call it was handed Proposed and drops its
/// input, so the source's end of input is refused as closed and the run's input never ends. The
/// run ends Failed through `fail_node`, naming the executor, without a cancel.
#[test]
fn a_consumer_whose_exchange_failed_ends_the_run_failed_naming_it_without_a_cancel() {
    let lookup = tool("lookup", SideEffectClass::ReadOnly, false);
    let call = proposed(&lookup);
    let (declaration, planned, [source_id, executor_id, sink_id]) = executor_pipeline();
    let run_id = RunId::generate();
    let update = ToolCallUpdate {
        call: call.clone(),
        event: ToolCallEvent::Proposed(inference_tools::ToolCallProposed {
            call_id: call.call_id(),
            tool_id: call.tool_id(),
            generation_id: call.generation_id(),
        }),
    };
    let source = TextSource::scripted(
        run_id,
        [(
            update.event.name().to_owned(),
            Some(call.generation_id()),
            Payload::Tool(update),
        )],
    );
    let executor = ToolExecutorNode::new(
        run_id,
        Arc::new(MockToolExecutor::new([(
            lookup.tool_id(),
            Script::Result(booked()),
        )])),
        [lookup.tool_id()],
        PATIENCE,
    );
    let (source_handle, executor_handle) = (source.handle(), executor.handle());
    let mut implementations: HashMap<NodeId, Box<dyn NodeImplementation<Payload>>> =
        HashMap::from([
            (
                source_id,
                Box::new(source) as Box<dyn NodeImplementation<Payload>>,
            ),
            (executor_id, Box::new(executor)),
            (sink_id, Box::new(TextSink::<Payload>::default())),
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
    let requests = controller.requests();
    let drive = thread::spawn(move || {
        let result = block_on(controller.drive());
        (controller, result)
    });
    assert!(source_handle.release(), "the source holds the call");
    assert!(
        eventually(|| executor_handle.failure().is_some()),
        "the executor's exchange failed on the Proposed call"
    );
    assert!(source_handle.release_end(), "the source admits its end");
    let ended = eventually(|| drive.is_finished());
    if !ended {
        requests.cancel();
    }
    let (controller, result) = drive.join().expect("the drive loop does not panic");
    assert!(ended, "the run ended within {PATIENCE:?} without a cancel");
    assert_eq!(controller.run().state(), RunState::Failed);
    let failure = controller
        .run()
        .failure()
        .expect("a Failed run carries its reason");
    assert_eq!(
        failure.node_id,
        Some(executor_id),
        "the reason names the node"
    );
    assert_eq!(
        Some(failure.detail.as_str()),
        executor_handle.failure().as_deref(),
        "and the error it failed with"
    );
    assert!(
        matches!(&result, Err(RunError::NodeFailed(reason)) if reason == failure),
        "{result:?}"
    );
    assert_eq!(
        controller.instance(executor_id).map(|i| i.state()),
        Some(NodeInstanceState::Failed)
    );
    for node_id in [source_id, sink_id] {
        assert_eq!(
            controller.instance(node_id).map(|i| i.state()),
            Some(NodeInstanceState::Stopped),
            "node {node_id} is wound down"
        );
    }
}
