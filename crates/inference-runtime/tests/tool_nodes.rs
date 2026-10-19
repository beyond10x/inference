//! Tool calls through the tool authority and the tool executor nodes of `inference-pipeline`
//! (`story:tool-authority-and-executor-nodes`, §15.1 of `docs/design/inference.md`): a proposal
//! is validated against its tool's schema, authorized by the default policy or held for the
//! confirmation hook, executed by the testkit's mock executor and answered with a result or a
//! typed failure, correlated to the call and the generation that proposed it.
//!
//! Each case declares source, authority, executor and sink with the typed builder, every edge
//! Structured, bounded and failing on overload, plans it and drives one run of it with
//! [`RunController::drive`]. The source is the testkit's mock source scripted with proposals, the
//! sink its collecting sink.

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
use inference_runtime::run::{NodeInstanceState, RunState};
use inference_runtime::{NodeId, RunController, RunError, RunRequests};
use inference_testkit::graph::{SinkHandle, SourceHandle, TextSink, TextSource};
use inference_testkit::tools::{MockToolExecutor, Script};
use inference_text::{OpenTurn, Span, Turn};
use inference_tools::{
    ConfirmationRequired, DeclareTool, Json, RejectionReason, SideEffectClass, Tool, ToolCall,
    ToolCallAuthorized, ToolCallFailed, ToolCallId, ToolCallRejected, ToolCallResult,
    ToolCallState, ToolFailure, ToolFailureKind,
};
use serde_json::json;

/// How long a case waits for the exchange tasks before it fails rather than hangs, and the
/// executor's deadline where a case does not test the deadline.
const PATIENCE: Duration = Duration::from_secs(10);

/// What the edges of the test pipeline carry: a step of a tool call, Structured, or a control
/// event.
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

/// The profile the pipeline is declared under; no node is bound to an adapter.
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

/// The four nodes of the pipeline.
#[derive(Debug, Clone, Copy)]
struct Nodes {
    source: NodeId,
    authority: NodeId,
    executor: NodeId,
    sink: NodeId,
}

/// Source to authority to executor to sink, every port Structured and reliable, every edge
/// bounded and failing on overload: a proposal or an outcome is delivered or fails explicitly.
fn planned() -> (Declaration, PlannedPipeline, Nodes) {
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
    let capacity = NonZeroUsize::new(4).expect("4");
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
            pipeline.connect(&proposed, &proposals, capacity, OverloadPolicy::Fail);
            pipeline.connect(&authorized, &calls, capacity, OverloadPolicy::Fail);
            pipeline.connect(&outcomes, &collected, capacity, OverloadPolicy::Fail);
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

fn declare(
    name: &str,
    schema: Json,
    side_effect: SideEffectClass,
    requires_confirmation: bool,
) -> Tool {
    Tool::declare(DeclareTool {
        name: name.to_owned(),
        schema,
        schema_revision: "1".to_owned(),
        side_effect,
        requires_confirmation,
    })
    .0
}

/// The declared tools of the cases and the mock executor's table for them.
struct World {
    /// Read-only; its schema requires a string `city` and nothing else.
    lookup: Tool,
    /// A non-idempotent write that requires confirmation.
    transfer: Tool,
    /// Read-only; the mock fails it.
    broken: Tool,
    /// Read-only; the mock overruns its deadline.
    slow: Tool,
    /// Read-only; the authority knows it and the executor node was not constructed with it.
    unserved: Tool,
    mock: Arc<MockToolExecutor>,
}

/// The failure the mock answers for [`World::broken`].
fn backend_down() -> ToolFailure {
    ToolFailure {
        kind: ToolFailureKind::ExecutionError,
        detail: "the backend is down".to_owned(),
    }
}

impl World {
    fn new() -> Self {
        let lookup = declare(
            "lookup",
            json!({
                "type": "object",
                "properties": {"city": {"type": "string"}},
                "required": ["city"],
                "additionalProperties": false
            }),
            SideEffectClass::ReadOnly,
            false,
        );
        let transfer = declare(
            "transfer",
            json!({
                "type": "object",
                "properties": {"amount": {"type": "integer"}},
                "required": ["amount"]
            }),
            SideEffectClass::NonIdempotentWrite,
            true,
        );
        let object = || json!({"type": "object"});
        let broken = declare("broken", object(), SideEffectClass::ReadOnly, false);
        let slow = declare("slow", object(), SideEffectClass::ReadOnly, false);
        let unserved = declare("unserved", object(), SideEffectClass::ReadOnly, false);
        let mock = Arc::new(MockToolExecutor::new([
            (lookup.tool_id(), Script::Result(json!({"temperature": 21}))),
            (
                transfer.tool_id(),
                Script::Result(json!({"transferred": true})),
            ),
            (broken.tool_id(), Script::Failure(backend_down())),
            (slow.tool_id(), Script::Overrun),
            (unserved.tool_id(), Script::Result(json!({}))),
        ]));
        Self {
            lookup,
            transfer,
            broken,
            slow,
            unserved,
            mock,
        }
    }

    /// A call of `tool` with `arguments`, proposed by a generation of its own on a committed span.
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

    /// Starts a run whose source holds `proposals`, whose authority knows every tool of the
    /// world and whose executor node serves every tool but [`World::unserved`] under `deadline`.
    fn start(&self, proposals: &[ToolCall], deadline: Duration) -> Running {
        let (declaration, planned, nodes) = planned();
        let run_id = RunId::generate();
        let held = proposals.iter().map(|call| {
            let event = inference_tools::ToolCallProposed {
                call_id: call.call_id(),
                tool_id: call.tool_id(),
                generation_id: call.generation_id(),
            };
            let update = ToolCallUpdate {
                call: call.clone(),
                event: ToolCallEvent::Proposed(event),
            };
            (
                update.event.name().to_owned(),
                Some(call.generation_id()),
                Payload::Tool(update),
            )
        });
        let source = TextSource::scripted(run_id, held.collect::<Vec<_>>());
        let authority = ToolAuthority::new(
            run_id,
            [
                self.lookup.clone(),
                self.transfer.clone(),
                self.broken.clone(),
                self.slow.clone(),
                self.unserved.clone(),
            ],
        );
        let executor = ToolExecutorNode::new(
            run_id,
            Arc::clone(&self.mock),
            [&self.lookup, &self.transfer, &self.broken, &self.slow].map(Tool::tool_id),
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
        Running {
            nodes,
            source: source_handle,
            authority: authority_handle,
            sink: sink_handle,
            requests,
            drive,
        }
    }
}

/// One run, Active, with its drive loop on a thread of its own.
struct Running {
    nodes: Nodes,
    source: SourceHandle<Payload>,
    authority: AuthorityHandle,
    sink: SinkHandle<Payload>,
    requests: RunRequests,
    drive: JoinHandle<(RunController<Payload>, Result<(), RunError>)>,
}

impl Running {
    /// Releases every proposal the source holds, then its end of input.
    fn release_all(&self) {
        while self.source.release() {}
        assert!(
            self.source.release_end(),
            "the source admits its end of input"
        );
    }

    /// Waits for the drive loop to end the run and checks that every node instance is Stopped.
    fn finished(self) -> Finished {
        let (controller, result) = self.drive.join().expect("the drive loop does not panic");
        result.expect("the drive loop ends the run");
        for node_id in [
            self.nodes.source,
            self.nodes.authority,
            self.nodes.executor,
            self.nodes.sink,
        ] {
            assert_eq!(
                controller
                    .instance(node_id)
                    .map(|instance| instance.state()),
                Some(NodeInstanceState::Stopped),
                "the instance of node {node_id}"
            );
        }
        Finished {
            state: controller.run().state(),
            authority: self.authority,
            received: self.sink.received(),
        }
    }
}

/// A run whose drive loop ended: its state, the authority's handle and what the sink received.
struct Finished {
    state: RunState,
    authority: AuthorityHandle,
    received: Vec<EventEnvelope<Payload>>,
}

/// The envelopes of `received` carrying a step of the call `call_id`, in the order received.
fn steps(received: &[EventEnvelope<Payload>], call_id: ToolCallId) -> Vec<EventEnvelope<Payload>> {
    received
        .iter()
        .filter(|envelope| {
            matches!(&envelope.payload, Payload::Tool(update) if update.call.call_id() == call_id)
        })
        .cloned()
        .collect()
}

fn update(envelope: &EventEnvelope<Payload>) -> &ToolCallUpdate {
    match &envelope.payload {
        Payload::Tool(update) => update,
        Payload::Control(kind) => panic!("a control event {kind:?} where a tool call was expected"),
    }
}

/// Checks that the sink received the call `call` started and then ended with `last`, each step
/// in an envelope of its event's kind naming the generation that proposed the call, the end of
/// input after them, and answers the call as `last` left it.
fn started_then(
    received: &[EventEnvelope<Payload>],
    call: &ToolCall,
    last: &ToolCallEvent,
) -> ToolCall {
    let steps = steps(received, call.call_id());
    let kinds: Vec<&str> = steps
        .iter()
        .map(|envelope| envelope.kind.as_str())
        .collect();
    assert_eq!(
        kinds,
        ["inference.tools.ToolCallStarted", last.name()],
        "the steps of call {}",
        call.call_id()
    );
    for envelope in &steps {
        assert_eq!(
            envelope.generation_id,
            Some(call.generation_id()),
            "every step names the generation that proposed the call"
        );
        assert!(
            envelope.causation_id.is_some(),
            "every step names its cause"
        );
        assert_eq!(update(envelope).call.call_id(), call.call_id());
        assert_eq!(update(envelope).call.generation_id(), call.generation_id());
    }
    assert_eq!(
        steps[1].causation_id,
        Some(steps[0].event_id),
        "the outcome answers the start"
    );
    assert_eq!(&update(&steps[1]).event, last);
    assert!(
        received
            .last()
            .is_some_and(|envelope| envelope.kind == ControlKind::Stop.as_str()),
        "the end of input reaches the sink last"
    );
    update(&steps[1]).call.clone()
}

/// The call identities of the mock executor's invocations, in order.
fn invoked(mock: &MockToolExecutor) -> Vec<ToolCallId> {
    mock.invocations()
        .iter()
        .map(|invocation| invocation.call_id)
        .collect()
}

/// Waits at most `patience` until `authority` holds a call.
fn wait_held(authority: &AuthorityHandle, patience: Duration) -> bool {
    let until = Instant::now() + patience;
    while authority.held().is_empty() {
        if Instant::now() >= until {
            return false;
        }
        thread::sleep(Duration::from_millis(1));
    }
    true
}

#[test]
fn a_held_call_the_hook_denies_is_rejected_denied_never_executed_and_no_longer_blocks_the_drain() {
    let world = World::new();
    let transfer = World::propose(&world.transfer, json!({"amount": 5}));
    let run = world.start(std::slice::from_ref(&transfer), PATIENCE);
    run.release_all();
    assert!(
        wait_held(&run.authority, PATIENCE),
        "the transfer is held for confirmation"
    );
    assert!(run.authority.deny(transfer.call_id()), "the hook denies");
    assert!(
        !run.authority.deny(transfer.call_id()),
        "a call answered once is not answered again"
    );
    assert!(
        !run.authority.confirm(transfer.call_id()),
        "a denied call cannot be confirmed"
    );
    let run = run.finished();

    assert_eq!(
        run.state,
        RunState::Completed,
        "the drain waited for no held call"
    );
    assert_eq!(run.authority.held(), [], "nothing is held any more");
    assert_eq!(
        run.authority
            .decisions()
            .iter()
            .map(|decision| (decision.outcome.clone(), decision.call.state()))
            .collect::<Vec<_>>(),
        [
            (
                AuthorityOutcome::Unconfirmed(ConfirmationRequired {
                    call_id: transfer.call_id()
                }),
                ToolCallState::Proposed
            ),
            (
                AuthorityOutcome::Rejected(ToolCallRejected {
                    call_id: transfer.call_id(),
                    reason: RejectionReason::Denied,
                }),
                ToolCallState::Rejected
            ),
        ],
        "RejectToolCall/outcome/rejected with reason denied, on the host's denial"
    );
    assert_eq!(invoked(&world.mock), [], "the transfer was never executed");
    assert_eq!(
        steps(&run.received, transfer.call_id()),
        [],
        "nothing of the transfer reached the sink"
    );
}

#[test]
fn a_read_only_proposal_with_valid_arguments_completes_with_a_result_correlated_to_call_and_generation()
 {
    let world = World::new();
    let call = World::propose(&world.lookup, json!({"city": "Berlin"}));
    let run = world.start(std::slice::from_ref(&call), PATIENCE);
    run.release_all();
    let run = run.finished();

    assert_eq!(run.state, RunState::Completed, "the run");
    let decisions = run.authority.decisions();
    assert_eq!(
        decisions
            .iter()
            .map(|decision| decision.outcome.clone())
            .collect::<Vec<_>>(),
        [AuthorityOutcome::Authorized(ToolCallAuthorized {
            call_id: call.call_id()
        })],
        "AuthorizeToolCall/outcome/authorized"
    );
    assert_eq!(decisions[0].call.state(), ToolCallState::Authorized);
    let result = ToolCallEvent::Result(ToolCallResult {
        call_id: call.call_id(),
        result: json!({"temperature": 21}),
    });
    let completed = started_then(&run.received, &call, &result);
    assert_eq!(
        completed.state(),
        ToolCallState::Completed,
        "StartToolCall/outcome/started, then CompleteToolCall/outcome/completed"
    );
    let invocations = world.mock.invocations();
    assert_eq!(
        invocations.len(),
        1,
        "one entry in the mock's invocation log"
    );
    assert_eq!(invocations[0].call_id, call.call_id());
    assert_eq!(invocations[0].generation_id, call.generation_id());
    assert_eq!(invocations[0].arguments, json!({"city": "Berlin"}));
}

#[test]
fn a_schema_invalid_proposal_is_rejected_schema_invalid_and_never_reaches_the_executor() {
    let world = World::new();
    let invalid = World::propose(&world.lookup, json!({"city": 7}));
    let undeclared = declare("undeclared", json!({}), SideEffectClass::ReadOnly, false);
    let unknown = World::propose(&undeclared, json!({}));
    let run = world.start(&[invalid.clone(), unknown.clone()], PATIENCE);
    run.release_all();
    let run = run.finished();

    assert_eq!(run.state, RunState::Completed, "the run");
    let decisions = run.authority.decisions();
    assert_eq!(
        decisions
            .iter()
            .map(|decision| (decision.outcome.clone(), decision.call.state()))
            .collect::<Vec<_>>(),
        [
            (
                AuthorityOutcome::Rejected(ToolCallRejected {
                    call_id: invalid.call_id(),
                    reason: RejectionReason::SchemaInvalid,
                }),
                ToolCallState::Rejected
            ),
            (
                AuthorityOutcome::Rejected(ToolCallRejected {
                    call_id: unknown.call_id(),
                    reason: RejectionReason::Denied,
                }),
                ToolCallState::Rejected
            ),
        ],
        "RejectToolCall/outcome/rejected: schema_invalid, and denied for a tool the authority \
         does not know"
    );
    assert_eq!(
        invoked(&world.mock),
        [],
        "the mock executor was never invoked"
    );
    assert!(
        run.received
            .iter()
            .all(|envelope| matches!(envelope.payload, Payload::Control(_))),
        "nothing of either call reached the executor node or the sink: {:?}",
        run.received
    );
}

#[test]
fn a_proposal_for_a_tool_that_requires_confirmation_stays_proposed_until_the_hook_confirms_it() {
    let world = World::new();
    let transfer = World::propose(&world.transfer, json!({"amount": 5}));
    let lookup = World::propose(&world.lookup, json!({"city": "Oslo"}));
    let run = world.start(&[transfer.clone(), lookup.clone()], PATIENCE);
    assert!(run.source.release(), "the transfer is proposed");
    assert!(run.source.release(), "then the lookup");
    assert!(
        run.sink.wait_received(2, PATIENCE),
        "the lookup, proposed after the transfer, is started and completed: the authority \
         went on past the call it holds"
    );

    let held = run.authority.held();
    assert_eq!(
        held.iter().map(ToolCall::call_id).collect::<Vec<_>>(),
        [transfer.call_id()],
        "the authority holds the transfer"
    );
    assert_eq!(held[0].state(), ToolCallState::Proposed, "held Proposed");
    assert_eq!(
        run.authority
            .decisions()
            .iter()
            .map(|decision| decision.outcome.clone())
            .collect::<Vec<_>>(),
        [
            AuthorityOutcome::Unconfirmed(ConfirmationRequired {
                call_id: transfer.call_id()
            }),
            AuthorityOutcome::Authorized(ToolCallAuthorized {
                call_id: lookup.call_id()
            }),
        ],
        "AuthorizeToolCall/outcome/unconfirmed for the transfer: the tool's \
         requires_confirmation reached the authorization"
    );
    assert_eq!(
        invoked(&world.mock),
        [lookup.call_id()],
        "the transfer was never executed"
    );
    assert_eq!(
        steps(&run.sink.received(), transfer.call_id()),
        [],
        "nothing of the transfer reached the sink"
    );

    assert!(
        !run.authority.confirm(lookup.call_id()),
        "a call that is not held cannot be confirmed"
    );
    assert!(
        run.authority.confirm(transfer.call_id()),
        "the hook confirms"
    );
    assert!(
        run.sink.wait_received(4, PATIENCE),
        "the confirmed transfer is started and completed"
    );
    run.release_all();
    let run = run.finished();

    assert_eq!(run.state, RunState::Completed, "the run");
    assert_eq!(run.authority.held(), [], "nothing is held any more");
    let decisions = run.authority.decisions();
    assert_eq!(
        decisions[2].outcome,
        AuthorityOutcome::Authorized(ToolCallAuthorized {
            call_id: transfer.call_id()
        }),
        "authorized once confirmed"
    );
    assert_eq!(decisions.len(), 3);
    let result = ToolCallEvent::Result(ToolCallResult {
        call_id: transfer.call_id(),
        result: json!({"transferred": true}),
    });
    let completed = started_then(&run.received, &transfer, &result);
    assert_eq!(completed.state(), ToolCallState::Completed);
    assert_eq!(
        invoked(&world.mock),
        [lookup.call_id(), transfer.call_id()],
        "the transfer was executed once, after the confirmation"
    );
}

#[test]
fn a_scripted_tool_failure_leaves_the_call_failed_with_a_tool_call_failed() {
    let world = World::new();
    let call = World::propose(&world.broken, json!({}));
    let run = world.start(std::slice::from_ref(&call), PATIENCE);
    run.release_all();
    let run = run.finished();

    assert_eq!(
        run.state,
        RunState::Completed,
        "a failed call does not fail the run"
    );
    let failed = ToolCallEvent::Failed(ToolCallFailed {
        call_id: call.call_id(),
        failure: backend_down(),
    });
    let ended = started_then(&run.received, &call, &failed);
    assert_eq!(
        ended.state(),
        ToolCallState::Failed,
        "FailToolCall/outcome/failed"
    );
    assert_eq!(invoked(&world.mock), [call.call_id()]);
}

#[test]
fn a_call_that_overruns_the_executor_nodes_deadline_fails_deadline_exceeded() {
    let world = World::new();
    let call = World::propose(&world.slow, json!({}));
    let run = world.start(std::slice::from_ref(&call), Duration::from_millis(50));
    run.release_all();
    let run = run.finished();

    assert_eq!(run.state, RunState::Completed, "the run");
    let steps = steps(&run.received, call.call_id());
    let last = update(steps.last().expect("the call has steps"));
    assert_eq!(last.call.state(), ToolCallState::Failed);
    assert!(
        matches!(
            &last.event,
            ToolCallEvent::Failed(ToolCallFailed {
                failure: ToolFailure {
                    kind: ToolFailureKind::DeadlineExceeded,
                    ..
                },
                ..
            })
        ),
        "{:?}",
        last.event
    );
    assert_eq!(invoked(&world.mock), [call.call_id()]);
}

#[test]
fn a_call_running_when_the_run_is_cancelled_has_nothing_sent_after_the_cancel() {
    let world = World::new();
    let call = World::propose(&world.slow, json!({}));
    let run = world.start(std::slice::from_ref(&call), PATIENCE);
    assert!(run.source.release(), "the slow call is proposed");
    assert!(
        run.sink.wait_received(1, PATIENCE),
        "the slow call is started"
    );
    run.requests.cancel();
    let run = run.finished();

    assert_eq!(run.state, RunState::Cancelled, "the run");
    assert_eq!(
        steps(&run.received, call.call_id())
            .iter()
            .map(|envelope| envelope.kind.clone())
            .collect::<Vec<_>>(),
        ["inference.tools.ToolCallStarted"],
        "the cancelled failure the mock answers once cancelled is not sent"
    );
    assert_eq!(invoked(&world.mock), [call.call_id()]);
}

#[test]
fn a_call_for_a_tool_the_executor_node_does_not_serve_fails_without_invoking_the_executor() {
    let world = World::new();
    let call = World::propose(&world.unserved, json!({}));
    let run = world.start(std::slice::from_ref(&call), PATIENCE);
    run.release_all();
    let run = run.finished();

    assert_eq!(run.state, RunState::Completed, "the run");
    let steps = steps(&run.received, call.call_id());
    let last = update(steps.last().expect("the call has steps"));
    assert_eq!(last.call.state(), ToolCallState::Failed);
    assert!(
        matches!(
            &last.event,
            ToolCallEvent::Failed(ToolCallFailed {
                failure: ToolFailure {
                    kind: ToolFailureKind::ExecutionError,
                    ..
                },
                ..
            })
        ),
        "{:?}",
        last.event
    );
    assert_eq!(invoked(&world.mock), [], "the executor was not invoked");
}
