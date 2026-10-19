//! Adversary pass 1 on the tool authority and the tool executor nodes of `inference-pipeline`
//! (`story:tool-authority-and-executor-nodes`): one call proposed twice, the confirmation hook
//! answered for a call that is already authorized, the schema subset against JSON Schema where
//! it reads a number or a malformed keyword, the keywords preparation refuses, an executor that
//! ignores its deadline, and a burst of proposals over an edge's capacity.
//!
//! Each run case declares source, authority, executor and sink with the typed builder as
//! `tests/tool_nodes.rs` does, and drives one run of it with [`RunController::drive`].

use std::collections::HashMap;
use std::future::{self, Future};
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant};

use inference_core::envelope::ControlKind;
use inference_core::generation::{Generation, StartGeneration};
use inference_core::id::{GenerationId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::nodes::{
    AuthorityHandle, AuthorityOutcome, ToolAuthority, ToolCallEvent, ToolCallUpdate,
    ToolExecutorNode, ToolPayload,
};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeImplementation,
    NodeRole, OverloadPolicy, PipelineSpec, PlannedPipeline, PortTraits, modality,
};
use inference_runtime::run::RunState;
use inference_runtime::{NodeId, RunController, RunError, RunRequests};
use inference_testkit::graph::{SinkHandle, SourceHandle, TextSink, TextSource};
use inference_testkit::tools::{MockToolExecutor, Script};
use inference_text::{OpenTurn, Span, Turn};
use inference_tools::{
    CancellationSignal, DeclareTool, ExecutionFuture, Json, SideEffectClass, Tool, ToolCall,
    ToolCallAuthorized, ToolCallFailed, ToolCallId, ToolCallProposed, ToolExecutor, ToolFailure,
    ToolFailureKind, ToolId,
};
use serde_json::json;

const PATIENCE: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Payload {
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
            other @ Self::Control(_) => Err(other),
        }
    }

    fn from_tool_call(update: ToolCallUpdate) -> Self {
        Self::Tool(update)
    }

    fn control(kind: ControlKind) -> Self {
        Self::Control(kind)
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
}

impl Catalog for Fixture {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.profile.profile_id() == profile_id).then_some(&self.profile)
    }

    fn adapter(&self, _: AdapterId) -> Option<&Adapter> {
        None
    }
}

#[derive(Debug, Clone, Copy)]
struct Nodes {
    source: NodeId,
    authority: NodeId,
    executor: NodeId,
    sink: NodeId,
}

/// Source to authority to executor to sink, Structured, reliable, every edge bounded at
/// `capacity`, one per edge in order, and failing on overload.
fn planned(capacity: [usize; 3]) -> (Declaration, PlannedPipeline, Nodes) {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    let catalog = Fixture { profile };
    let structured = PortTraits {
        essentiality: Essentiality::Essential,
        delivery: Delivery::Reliable,
        incremental: false,
        end_marker_required: true,
        provisional_allowed: false,
    };
    let spec = PipelineSpec {
        name: "tool calls".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let capacity = capacity.map(|n| NonZeroUsize::new(n).expect("a capacity above zero"));
    let mut declaration = Declaration::new();
    let (pipeline_id, nodes) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let source = pipeline.node("proposer", NodeRole::Source, Vec::new());
            let authority = pipeline.node("authority", NodeRole::Policy, Vec::new());
            let executor = pipeline.node("executor", NodeRole::Tool, Vec::new());
            let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
            let proposed = pipeline.output::<modality::Structured>(&source, "proposed", structured);
            let proposals =
                pipeline.input::<modality::Structured>(&authority, "proposed", structured);
            let authorized =
                pipeline.output::<modality::Structured>(&authority, "authorized", structured);
            let calls = pipeline.input::<modality::Structured>(&executor, "authorized", structured);
            let outcomes =
                pipeline.output::<modality::Structured>(&executor, "outcomes", structured);
            let collected = pipeline.input::<modality::Structured>(&sink, "outcomes", structured);
            pipeline.connect(&proposed, &proposals, capacity[0], OverloadPolicy::Fail);
            pipeline.connect(&authorized, &calls, capacity[1], OverloadPolicy::Fail);
            pipeline.connect(&outcomes, &collected, capacity[2], OverloadPolicy::Fail);
            Nodes {
                source: source.id(),
                authority: authority.id(),
                executor: executor.id(),
                sink: sink.id(),
            }
        })
        .expect("the profile is declared");
    let validated = declaration
        .validate(pipeline_id)
        .expect("source to authority to executor to sink is valid");
    let planned = declaration
        .plan(validated, &catalog)
        .expect("no node requires a capability");
    (declaration, planned, nodes)
}

fn declare(name: &str, schema: Json, side_effect: SideEffectClass, confirm: bool) -> Tool {
    Tool::declare(DeclareTool {
        name: name.to_owned(),
        schema,
        schema_revision: "1".to_owned(),
        side_effect,
        requires_confirmation: confirm,
    })
    .0
}

fn propose(tool: &Tool, arguments: Json) -> ToolCall {
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
    ToolCall::propose(tool, &generation, &span, arguments)
        .expect("an Active generation proposes on a committed span")
        .0
}

/// One run, Active, with its drive loop on a thread of its own.
struct Running {
    source: SourceHandle<Payload>,
    authority: AuthorityHandle,
    sink: SinkHandle<Payload>,
    requests: RunRequests,
    drive: JoinHandle<(RunController<Payload>, Result<(), RunError>)>,
    #[allow(dead_code)]
    nodes: Nodes,
}

/// Starts a run whose source holds `proposals`, whose authority knows `tools` and whose
/// executor node runs every one of them on `executor` under `deadline`.
fn start<E: ToolExecutor + ?Sized + 'static>(
    tools: &[Tool],
    executor: Arc<E>,
    proposals: &[ToolCall],
    deadline: Duration,
    capacity: [usize; 3],
) -> Running {
    let (declaration, planned, nodes) = planned(capacity);
    let run_id = RunId::generate();
    let held = proposals.iter().map(|call| {
        let update = ToolCallUpdate {
            call: call.clone(),
            event: ToolCallEvent::Proposed(ToolCallProposed {
                call_id: call.call_id(),
                tool_id: call.tool_id(),
                generation_id: call.generation_id(),
            }),
        };
        (
            update.event.name().to_owned(),
            Some(call.generation_id()),
            Payload::Tool(update),
        )
    });
    let source = TextSource::scripted(run_id, held.collect::<Vec<_>>());
    let authority = ToolAuthority::new(run_id, tools.iter().cloned());
    let executor = ToolExecutorNode::new(
        run_id,
        executor,
        tools.iter().map(Tool::tool_id).collect::<Vec<ToolId>>(),
        deadline,
    );
    let sink = TextSink::<Payload>::default();
    let (source_handle, authority_handle, sink_handle) =
        (source.handle(), authority.handle(), sink.handle());
    let mut implementations: HashMap<NodeId, Box<dyn NodeImplementation<Payload>>> =
        HashMap::from([
            (
                nodes.source,
                Box::new(source) as Box<dyn NodeImplementation<Payload>>,
            ),
            (nodes.authority, Box::new(authority)),
            (nodes.executor, Box::new(executor)),
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
    assert_eq!(controller.run().state(), RunState::Active);
    let requests = controller.requests();
    let drive = thread::spawn(move || {
        let result = block_on(controller.drive());
        (controller, result)
    });
    Running {
        source: source_handle,
        authority: authority_handle,
        sink: sink_handle,
        requests,
        drive,
        nodes,
    }
}

impl Running {
    fn release_all(&self) {
        while self.source.release() {}
        assert!(self.source.release_end(), "the source admits its end");
    }

    /// The run's final state, once its drive loop ended within `patience`; cancels the run and
    /// answers `None` when it did not end.
    fn ended_within(self, patience: Duration) -> Option<RunState> {
        let until = Instant::now() + patience;
        while !self.drive.is_finished() && Instant::now() < until {
            thread::sleep(Duration::from_millis(5));
        }
        let ended = self.drive.is_finished();
        if !ended {
            self.requests.cancel();
        }
        let (controller, _) = self.drive.join().expect("the drive loop does not panic");
        ended.then(|| controller.run().state())
    }

    /// Cancels the run and waits for its drive loop.
    fn cancel(self) {
        self.requests.cancel();
        let _ = self.drive.join().expect("the drive loop does not panic");
    }
}

fn invoked(mock: &MockToolExecutor) -> Vec<ToolCallId> {
    mock.invocations()
        .iter()
        .map(|invocation| invocation.call_id)
        .collect()
}

fn outcomes(authority: &AuthorityHandle) -> Vec<AuthorityOutcome> {
    authority
        .decisions()
        .iter()
        .map(|decision| decision.outcome.clone())
        .collect()
}

fn read_only(name: &str, schema: Json) -> Tool {
    declare(name, schema, SideEffectClass::ReadOnly, false)
}

/// 699 and `AuthorizeToolCall/outcome/wrong-state`: a call is authorized once. The same
/// proposal delivered twice (a proposer that resends) must not be authorized and executed twice.
#[test]
fn adversary1_one_call_proposed_twice_is_authorized_once_and_executed_once() {
    let lookup = read_only("lookup", json!({"type": "object"}));
    let mock = Arc::new(MockToolExecutor::new([(
        lookup.tool_id(),
        Script::Result(json!({"ok": true})),
    )]));
    let call = propose(&lookup, json!({}));
    let run = start(
        std::slice::from_ref(&lookup),
        Arc::clone(&mock),
        &[call.clone(), call.clone()],
        PATIENCE,
        [4; 3],
    );
    run.release_all();
    let authority = run.authority.clone();
    let _ = run.ended_within(PATIENCE);
    let authorized = outcomes(&authority)
        .iter()
        .filter(|outcome| {
            **outcome
                == AuthorityOutcome::Authorized(ToolCallAuthorized {
                    call_id: call.call_id(),
                })
        })
        .count();
    assert_eq!(
        (authorized, invoked(&mock)),
        (1, vec![call.call_id()]),
        "call {} authorized and executed once",
        call.call_id()
    );
}

/// The confirmation hook answered twice for one call: a non-idempotent write that requires
/// confirmation, proposed twice under one call_id, confirmed once and executed, must not be
/// confirmable, and executable, a second time.
#[test]
fn adversary1_a_confirmed_and_executed_call_cannot_be_confirmed_again() {
    let transfer = declare(
        "transfer",
        json!({"type": "object"}),
        SideEffectClass::NonIdempotentWrite,
        true,
    );
    let mock = Arc::new(MockToolExecutor::new([(
        transfer.tool_id(),
        Script::Result(json!({"transferred": true})),
    )]));
    let call = propose(&transfer, json!({"amount": 5}));
    let run = start(
        std::slice::from_ref(&transfer),
        Arc::clone(&mock),
        &[call.clone(), call.clone()],
        PATIENCE,
        [4; 3],
    );
    assert!(run.source.release());
    assert!(run.source.release());
    let until = Instant::now() + PATIENCE;
    while run.authority.held().is_empty() && Instant::now() < until {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(run.authority.confirm(call.call_id()), "the hook confirms");
    assert!(
        run.sink.wait_received(2, PATIENCE),
        "the transfer is started and completed"
    );
    let still_held: Vec<ToolCallId> = run.authority.held().iter().map(ToolCall::call_id).collect();
    let again = run.authority.confirm(call.call_id());
    let _ = run.sink.wait_received(4, Duration::from_millis(500));
    let executed = invoked(&mock);
    run.cancel();
    assert_eq!(
        (still_held, again, executed),
        (Vec::new(), false, vec![call.call_id()]),
        "after the transfer ran: nothing held, a second confirm refused, executed once"
    );
}

/// JSON Schema compares `enum` members as JSON values, numbers by mathematical value: 1.0 is
/// the enum member 1, as the subset's own `integer` already admits 3.0.
#[test]
fn adversary1_an_enum_admits_a_number_equal_to_a_member() {
    let pick = read_only(
        "pick",
        json!({
            "type": "object",
            "properties": {"n": {"enum": [1, "one"]}},
            "required": ["n"]
        }),
    );
    let mock = Arc::new(MockToolExecutor::new([(
        pick.tool_id(),
        Script::Result(json!({})),
    )]));
    let call = propose(&pick, json!({"n": 1.0}));
    let run = start(
        std::slice::from_ref(&pick),
        Arc::clone(&mock),
        std::slice::from_ref(&call),
        PATIENCE,
        [4; 3],
    );
    run.release_all();
    let authority = run.authority.clone();
    let _ = run.ended_within(PATIENCE);
    assert_eq!(
        outcomes(&authority),
        [AuthorityOutcome::Authorized(ToolCallAuthorized {
            call_id: call.call_id()
        })],
        "{{\"n\": 1.0}} against enum [1, \"one\"]"
    );
}

/// The module documentation: a schema the authority cannot validate in full fails preparation.
/// `required` that is not an array of names validates nothing in the subset, so `{}` passes a
/// schema whose author required `city`.
#[test]
fn adversary1_a_malformed_required_fails_preparation_rather_than_validating_nothing() {
    let lookup = read_only(
        "lookup",
        json!({
            "type": "object",
            "properties": {"city": {"type": "string"}},
            "required": "city"
        }),
    );
    let mut authority = ToolAuthority::new(RunId::generate(), [lookup]);
    let prepared = block_on(NodeImplementation::<Payload>::prepare(&mut authority));
    assert!(
        prepared.is_err(),
        "prepare admitted a schema whose `required` is a string"
    );
}

/// Guards `prepare` and the walk of `supported` into `additionalProperties`: a keyword outside
/// the subset fails preparation at the root and below `additionalProperties`.
#[test]
fn adversary1_preparation_refuses_a_keyword_outside_the_subset_at_any_depth() {
    for schema in [
        json!({"type": "string", "pattern": "^a"}),
        json!({"type": "object", "additionalProperties": {"type": "string", "pattern": "^a"}}),
        json!({"type": "array", "items": {"type": "object", "properties": {"n": {"minimum": 1}}}}),
    ] {
        let tool = read_only("t", schema.clone());
        let mut authority = ToolAuthority::new(RunId::generate(), [tool]);
        let prepared = block_on(NodeImplementation::<Payload>::prepare(&mut authority));
        assert!(prepared.is_err(), "prepare admitted {schema}");
    }
    let tool = read_only(
        "t",
        json!({"type": "object", "additionalProperties": false}),
    );
    let mut authority = ToolAuthority::new(RunId::generate(), [tool]);
    assert!(block_on(NodeImplementation::<Payload>::prepare(&mut authority)).is_ok());
}

/// An executor that never answers and ignores both its deadline and its cancellation.
struct Deaf;

impl ToolExecutor for Deaf {
    fn execute<'a>(
        &'a self,
        _: &'a ToolCall,
        _: Instant,
        _: &'a CancellationSignal,
    ) -> ExecutionFuture<'a> {
        Box::pin(future::pending())
    }
}

/// Guards the node's own deadline (852): the module documentation says a call still running
/// at the deadline fails `deadline_exceeded` whatever the executor does with it.
#[test]
fn adversary1_the_node_fails_a_call_whose_executor_ignores_the_deadline() {
    let slow = read_only("slow", json!({"type": "object"}));
    let call = propose(&slow, json!({}));
    let run = start(
        std::slice::from_ref(&slow),
        Arc::new(Deaf),
        std::slice::from_ref(&call),
        Duration::from_millis(50),
        [4; 3],
    );
    run.release_all();
    let sink = run.sink.clone();
    let state = run.ended_within(PATIENCE);
    let last = sink
        .received()
        .iter()
        .filter_map(|envelope| match &envelope.payload {
            Payload::Tool(update) => Some(update.event.clone()),
            Payload::Control(_) => None,
        })
        .next_back();
    assert_eq!(state, Some(RunState::Completed), "the run ends");
    assert!(
        matches!(
            last,
            Some(ToolCallEvent::Failed(ToolCallFailed {
                failure: ToolFailure {
                    kind: ToolFailureKind::DeadlineExceeded,
                    ..
                },
                ..
            }))
        ),
        "{last:?}"
    );
}

/// Eight read-only proposals released at once into a run whose edges have `capacity`: the run's
/// final state (`None`: not ended within [`PATIENCE`]), the authority's decisions and the
/// results the sink received.
fn burst(capacity: [usize; 3]) -> (Option<RunState>, usize, usize) {
    let lookup = read_only("lookup", json!({"type": "object"}));
    let mock = Arc::new(MockToolExecutor::new([(
        lookup.tool_id(),
        Script::Result(json!({})),
    )]));
    let calls: Vec<ToolCall> = (0..8).map(|_| propose(&lookup, json!({}))).collect();
    let run = start(
        std::slice::from_ref(&lookup),
        Arc::clone(&mock),
        &calls,
        PATIENCE,
        capacity,
    );
    run.release_all();
    let (sink, authority) = (run.sink.clone(), run.authority.clone());
    let state = run.ended_within(PATIENCE);
    let completed = sink
        .received()
        .iter()
        .filter(|envelope| envelope.kind == "inference.tools.ToolCallResult")
        .count();
    (state, authority.decisions().len(), completed)
}

/// Overload `fail` (687) on the edges out of the authority and the executor: a burst over the
/// capacity is delivered or fails the run explicitly, never hangs it or loses a call silently.
#[test]
fn adversary1_a_burst_over_the_tool_edges_is_delivered_or_fails_the_run() {
    for capacity in [[16, 1, 16], [16, 16, 1]] {
        let (state, decisions, completed) = burst(capacity);
        assert!(
            state == Some(RunState::Failed)
                || (state == Some(RunState::Completed) && completed == 8),
            "capacity {capacity:?}: run {state:?}, {decisions} decisions, {completed} of 8 results"
        );
    }
}

/// Overload `fail` (687) on the edge into the authority: a burst over its capacity is
/// delivered or fails the run explicitly, never hangs it.
#[test]
fn adversary1_a_burst_over_the_proposal_edge_is_delivered_or_fails_the_run() {
    let (state, decisions, completed) = burst([1, 16, 16]);
    assert!(
        state == Some(RunState::Failed) || (state == Some(RunState::Completed) && completed == 8),
        "capacity [1, 16, 16]: run {state:?} (None: not ended in {PATIENCE:?}), {decisions} \
         decisions, {completed} of 8 results"
    );
}

/// An executor adapter with a bug: it panics inside `execute`, after a pause long enough for
/// the end of input to be queued behind the call.
struct Panics;

impl ToolExecutor for Panics {
    fn execute<'a>(
        &'a self,
        _: &'a ToolCall,
        _: Instant,
        _: &'a CancellationSignal,
    ) -> ExecutionFuture<'a> {
        thread::sleep(Duration::from_millis(300));
        panic!("the adapter has a bug");
    }
}

/// 687, no silent loss: a started call whose executor panicked has no result and no failure,
/// so the run must not complete as if every call was answered.
#[test]
fn adversary1_a_started_call_whose_executor_panicked_does_not_complete_the_run() {
    let lookup = read_only("lookup", json!({"type": "object"}));
    let call = propose(&lookup, json!({}));
    let run = start(
        std::slice::from_ref(&lookup),
        Arc::new(Panics),
        std::slice::from_ref(&call),
        PATIENCE,
        [4; 3],
    );
    run.release_all();
    let sink = run.sink.clone();
    let state = run.ended_within(PATIENCE);
    let kinds: Vec<String> = sink
        .received()
        .iter()
        .map(|envelope| envelope.kind.clone())
        .collect();
    let answered = kinds.iter().any(|kind| {
        kind == "inference.tools.ToolCallResult" || kind == "inference.tools.ToolCallFailed"
    });
    assert!(
        state == Some(RunState::Failed) || answered,
        "the run ended {state:?}; the sink received {kinds:?}"
    );
}
