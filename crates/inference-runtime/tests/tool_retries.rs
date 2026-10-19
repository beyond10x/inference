//! Safe tool retries and at-most-once start in the tool executor node of `inference-pipeline`
//! (`story:tool-safe-retries`; `docs/design/inference.md` 852, 854, 999, 1007, 1025, 1215).
//!
//! The executor node attempts a call again after a transient failure only when the call's
//! side-effect class allows it and its retry policy has attempts left, every attempt presenting
//! the same call; a non-idempotent write is attempted once. An authorized call delivered to the
//! node twice is executed once, and the second delivery is answered with
//! `StartToolCall/outcome/wrong-state`, recorded on the node's handle, while the run goes on.
//!
//! Each case declares source, executor and sink with the typed builder, every edge Structured,
//! bounded and failing on overload, plans it and drives one run of it with
//! [`RunController::drive`]. The source is the testkit's mock source scripted with authorized
//! calls, the executor runs on the testkit's mock executor, and the sink is its collecting sink.

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, JoinHandle, Thread};
use std::time::Duration;

use inference_core::envelope::{ControlKind, EventEnvelope};
use inference_core::generation::{Generation, StartGeneration};
use inference_core::id::{GenerationId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::nodes::{ToolCallEvent, ToolCallUpdate, ToolExecutorNode, ToolPayload};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeImplementation,
    NodeRole, OverloadPolicy, PipelineSpec, PlannedPipeline, PortTraits, modality,
};
use inference_runtime::run::{NodeInstanceState, RunState};
use inference_runtime::{NodeId, RunController, RunError};
use inference_testkit::graph::{SinkHandle, SourceHandle, TextSink, TextSource};
use inference_testkit::tools::{MockToolExecutor, Script};
use inference_text::{OpenTurn, Span, Turn};
use inference_tools::{
    AuthorizeToolCall, DeclareTool, RetryPolicy, SideEffectClass, Tool, ToolCall, ToolCallFailed,
    ToolCallId, ToolCallResult, ToolCallState, ToolCallStateConflict, ToolFailure, ToolFailureKind,
};
use serde_json::json;

/// The executor's deadline: no case here tests it.
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

/// The three nodes of the pipeline.
#[derive(Debug, Clone, Copy)]
struct Nodes {
    source: NodeId,
    executor: NodeId,
    sink: NodeId,
}

/// Source of authorized calls to executor to sink, every port Structured and reliable, every edge
/// bounded and failing on overload.
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
        name: "tool retries".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let capacity = NonZeroUsize::new(4).expect("4");
    let mut declaration = Declaration::new();
    let (pipeline_id, nodes) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let source = pipeline.node("authorizer", NodeRole::Source, Vec::new());
            let executor = pipeline.node("executor", NodeRole::Tool, Vec::new());
            let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
            let authorized =
                pipeline.output::<modality::Structured>(&source, "authorized", structured);
            let calls = pipeline.input::<modality::Structured>(&executor, "authorized", structured);
            let outcomes =
                pipeline.output::<modality::Structured>(&executor, "outcomes", structured);
            let collected = pipeline.input::<modality::Structured>(&sink, "outcomes", structured);
            pipeline.connect(&authorized, &calls, capacity, OverloadPolicy::Fail);
            pipeline.connect(&outcomes, &collected, capacity, OverloadPolicy::Fail);
            Nodes {
                source: source.id(),
                executor: executor.id(),
                sink: sink.id(),
            }
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

fn declare(name: &str, side_effect: SideEffectClass) -> Tool {
    Tool::declare(DeclareTool {
        name: name.to_owned(),
        schema: json!({"type": "object"}),
        schema_revision: "1".to_owned(),
        side_effect,
        requires_confirmation: false,
    })
    .0
}

/// The transient failure the mock answers before it succeeds.
fn hiccup() -> ToolFailure {
    ToolFailure {
        kind: ToolFailureKind::ExecutionError,
        detail: "the backend hiccuped".to_owned(),
    }
}

/// A call of `tool`, proposed by a generation of its own on a committed span and authorized.
fn authorized(tool: &Tool) -> ToolCall {
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
    let (proposed, _) = ToolCall::propose(tool, &generation, &span, json!({"n": 1}))
        .expect("an Active generation proposes on a committed span");
    proposed
        .authorize(&AuthorizeToolCall {
            call_id: proposed.call_id(),
            confirmation_required: false,
            confirmed: false,
        })
        .expect("a Proposed call needing no confirmation is authorized")
        .0
}

/// One run, Active, with its drive loop on a thread of its own.
struct Running {
    nodes: Nodes,
    source: SourceHandle<Payload>,
    sink: SinkHandle<Payload>,
    drive: JoinHandle<(RunController<Payload>, Result<(), RunError>)>,
}

/// Starts a run of `run_id` whose source holds `deliveries`, each an authorized call delivered in
/// that order, and whose executor node is `executor`.
fn start(
    run_id: RunId,
    deliveries: &[ToolCall],
    executor: ToolExecutorNode<MockToolExecutor>,
) -> Running {
    let (declaration, planned, nodes) = planned();
    let held = deliveries.iter().map(|call| {
        let update = ToolCallUpdate {
            call: call.clone(),
            event: ToolCallEvent::Authorized(inference_tools::ToolCallAuthorized {
                call_id: call.call_id(),
            }),
        };
        (
            update.event.name().to_owned(),
            Some(call.generation_id()),
            Payload::Tool(update),
        )
    });
    let source = TextSource::scripted(run_id, held.collect::<Vec<_>>());
    let sink = TextSink::<Payload>::default();
    let (source_handle, sink_handle) = (source.handle(), sink.handle());
    let mut implementations: HashMap<NodeId, Box<dyn NodeImplementation<Payload>>> =
        HashMap::from([
            (
                nodes.source,
                Box::new(source) as Box<dyn NodeImplementation<Payload>>,
            ),
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
    let drive = thread::spawn(move || {
        let result = block_on(controller.drive());
        (controller, result)
    });
    Running {
        nodes,
        source: source_handle,
        sink: sink_handle,
        drive,
    }
}

impl Running {
    /// Releases every delivery the source holds and its end of input, waits for the drive loop
    /// to end the run, checks that every node instance is Stopped, and answers the run's state
    /// and what the sink received.
    fn finished(self) -> (RunState, Vec<EventEnvelope<Payload>>) {
        while self.source.release() {}
        assert!(
            self.source.release_end(),
            "the source admits its end of input"
        );
        let (controller, result) = self.drive.join().expect("the drive loop does not panic");
        result.expect("the drive loop ends the run");
        for node_id in [self.nodes.source, self.nodes.executor, self.nodes.sink] {
            assert_eq!(
                controller
                    .instance(node_id)
                    .map(|instance| instance.state()),
                Some(NodeInstanceState::Stopped),
                "the instance of node {node_id}"
            );
        }
        (controller.run().state(), self.sink.received())
    }
}

/// The tool-call updates of `received` for the call `call_id`, in the order received, each with
/// the generation its envelope names.
fn steps(
    received: &[EventEnvelope<Payload>],
    call_id: ToolCallId,
) -> Vec<(ToolCallUpdate, Option<GenerationId>)> {
    received
        .iter()
        .filter_map(|envelope| match &envelope.payload {
            Payload::Tool(update) if update.call.call_id() == call_id => {
                Some((update.clone(), envelope.generation_id))
            }
            _ => None,
        })
        .collect()
}

/// The call and generation identities of the mock executor's invocations, in order.
fn invoked(mock: &MockToolExecutor) -> Vec<(ToolCallId, GenerationId)> {
    mock.invocations()
        .iter()
        .map(|invocation| (invocation.call_id, invocation.generation_id))
        .collect()
}

/// A mock answering `tool` with `failures` transient failures of a call, then a result.
fn flaky(tool: &Tool, failures: usize) -> Arc<MockToolExecutor> {
    Arc::new(MockToolExecutor::new([(
        tool.tool_id(),
        Script::FailThenSucceed {
            failures,
            failure: hiccup(),
            result: json!({"temperature": 21}),
        },
    )]))
}

#[test]
fn a_read_only_call_failing_once_completes_on_the_second_attempt_with_its_identity_unchanged() {
    let lookup = declare("lookup", SideEffectClass::ReadOnly);
    let mock = flaky(&lookup, 1);
    let call = authorized(&lookup);
    let run_id = RunId::generate();
    let executor = ToolExecutorNode::new(run_id, Arc::clone(&mock), [lookup.tool_id()], PATIENCE)
        .with_retries(RetryPolicy::up_to(3));
    let handle = executor.handle();
    let (state, received) = start(run_id, std::slice::from_ref(&call), executor).finished();

    assert_eq!(state, RunState::Completed);
    assert_eq!(
        invoked(&mock),
        [
            (call.call_id(), call.generation_id()),
            (call.call_id(), call.generation_id())
        ],
        "two entries in the mock log, both under the call's call_id and generation_id"
    );
    let steps = steps(&received, call.call_id());
    let events: Vec<_> = steps
        .iter()
        .map(|(update, _)| update.event.name())
        .collect();
    assert_eq!(
        events,
        [
            "inference.tools.ToolCallStarted",
            "inference.tools.ToolCallResult"
        ],
        "attempts are not lifecycle transitions: one start, one result"
    );
    let (completed, generation) = &steps[1];
    assert_eq!(
        completed.event,
        ToolCallEvent::Result(ToolCallResult {
            call_id: call.call_id(),
            result: json!({"temperature": 21}),
        })
    );
    assert_eq!(completed.call.state(), ToolCallState::Completed);
    assert_eq!(completed.call.call_id(), call.call_id());
    assert_eq!(completed.call.generation_id(), call.generation_id());
    assert_eq!(*generation, Some(call.generation_id()));
    let measures = handle.measures();
    assert_eq!(measures.len(), 1, "one measure per call");
    assert_eq!(measures[0].call_id, call.call_id());
    assert_eq!(measures[0].side_effect, SideEffectClass::ReadOnly);
    assert_eq!(measures[0].attempts, 2);
    assert_eq!(measures[0].failure, None);
    assert_eq!(handle.refusals(), []);
}

#[test]
fn a_non_idempotent_write_failing_once_is_failed_after_one_attempt() {
    let transfer = declare("transfer", SideEffectClass::NonIdempotentWrite);
    let mock = flaky(&transfer, 1);
    let call = authorized(&transfer);
    let run_id = RunId::generate();
    let executor = ToolExecutorNode::new(run_id, Arc::clone(&mock), [transfer.tool_id()], PATIENCE)
        .with_retries(RetryPolicy::up_to(3));
    let handle = executor.handle();
    let (state, received) = start(run_id, std::slice::from_ref(&call), executor).finished();

    assert_eq!(state, RunState::Completed, "a failed call fails no run");
    assert_eq!(
        invoked(&mock),
        [(call.call_id(), call.generation_id())],
        "one entry: a non-idempotent write is never attempted again automatically"
    );
    let steps = steps(&received, call.call_id());
    let events: Vec<_> = steps
        .iter()
        .map(|(update, _)| update.event.name())
        .collect();
    assert_eq!(
        events,
        [
            "inference.tools.ToolCallStarted",
            "inference.tools.ToolCallFailed"
        ]
    );
    assert_eq!(
        steps[1].0.event,
        ToolCallEvent::Failed(ToolCallFailed {
            call_id: call.call_id(),
            failure: hiccup(),
        }),
        "FailToolCall/outcome/failed with the failure the one attempt answered"
    );
    assert_eq!(steps[1].0.call.state(), ToolCallState::Failed);
    let measures = handle.measures();
    assert_eq!(measures.len(), 1);
    assert_eq!(measures[0].side_effect, SideEffectClass::NonIdempotentWrite);
    assert_eq!(measures[0].attempts, 1);
    assert_eq!(measures[0].failure, Some(ToolFailureKind::ExecutionError));
}

#[test]
fn an_idempotent_write_is_attempted_up_to_the_limit_and_fails_with_the_count_on_its_measure() {
    let upsert = declare("upsert", SideEffectClass::IdempotentWrite);
    let recovers = authorized(&upsert);
    let exhausts = authorized(&upsert);
    let mock = flaky(&upsert, 2);
    let run_id = RunId::generate();
    let executor = ToolExecutorNode::new(run_id, Arc::clone(&mock), [upsert.tool_id()], PATIENCE)
        .with_retries(RetryPolicy::up_to(3));
    let handle = executor.handle();
    let (state, received) = start(run_id, std::slice::from_ref(&recovers), executor).finished();
    assert_eq!(state, RunState::Completed);
    assert_eq!(invoked(&mock).len(), 3, "two failures, then the result");
    let last = steps(&received, recovers.call_id());
    assert_eq!(last[1].0.call.state(), ToolCallState::Completed);
    assert_eq!(handle.measures()[0].attempts, 3);

    let mock = flaky(&upsert, 5);
    let run_id = RunId::generate();
    let executor = ToolExecutorNode::new(run_id, Arc::clone(&mock), [upsert.tool_id()], PATIENCE)
        .with_retries(RetryPolicy::up_to(3));
    let handle = executor.handle();
    let (state, received) = start(run_id, std::slice::from_ref(&exhausts), executor).finished();
    assert_eq!(state, RunState::Completed);
    assert_eq!(
        invoked(&mock),
        [(exhausts.call_id(), exhausts.generation_id()); 3],
        "three attempts, the limit, every one under the same call_id"
    );
    let last = steps(&received, exhausts.call_id());
    assert_eq!(
        last[1].0.event,
        ToolCallEvent::Failed(ToolCallFailed {
            call_id: exhausts.call_id(),
            failure: hiccup(),
        })
    );
    let measure = &handle.measures()[0];
    assert_eq!(
        (measure.attempts, measure.failure),
        (3, Some(ToolFailureKind::ExecutionError))
    );
}

#[test]
fn an_authorized_call_delivered_twice_is_executed_once_and_refused_once_by_state() {
    let lookup = declare("lookup", SideEffectClass::ReadOnly);
    let mock = Arc::new(MockToolExecutor::new([(
        lookup.tool_id(),
        Script::Result(json!({"temperature": 21})),
    )]));
    let call = authorized(&lookup);
    let run_id = RunId::generate();
    let executor = ToolExecutorNode::new(run_id, Arc::clone(&mock), [lookup.tool_id()], PATIENCE);
    let handle = executor.handle();
    let (state, received) = start(run_id, &[call.clone(), call.clone()], executor).finished();

    assert_eq!(
        state,
        RunState::Completed,
        "a redelivery is refused and the run goes on"
    );
    assert_eq!(
        invoked(&mock),
        [(call.call_id(), call.generation_id())],
        "one entry in the mock log"
    );
    let events: Vec<_> = steps(&received, call.call_id())
        .iter()
        .map(|(update, _)| update.event.name())
        .collect();
    assert_eq!(
        events,
        [
            "inference.tools.ToolCallStarted",
            "inference.tools.ToolCallResult"
        ],
        "one StartToolCall accepted, nothing sent for the refused one"
    );
    let refusals = handle.refusals();
    assert_eq!(refusals.len(), 1, "one refusal");
    assert_eq!(refusals[0].call.call_id(), call.call_id());
    assert_eq!(
        refusals[0].refused,
        ToolCallStateConflict {
            current: ToolCallState::Completed
        },
        "StartToolCall/outcome/wrong-state: a Completed call refuses StartToolCall"
    );
    assert_eq!(handle.measures().len(), 1, "one execution measured");
    assert!(
        received
            .last()
            .is_some_and(|envelope| envelope.kind == ControlKind::Stop.as_str()),
        "the end of input reaches the sink last"
    );
}

#[test]
fn a_failed_call_delivered_again_is_refused_by_state_and_not_attempted_again() {
    let lookup = declare("lookup", SideEffectClass::ReadOnly);
    let mock = flaky(&lookup, 1);
    let call = authorized(&lookup);
    let run_id = RunId::generate();
    let executor = ToolExecutorNode::new(run_id, Arc::clone(&mock), [lookup.tool_id()], PATIENCE);
    let handle = executor.handle();
    let (state, _) = start(run_id, &[call.clone(), call.clone()], executor).finished();

    assert_eq!(state, RunState::Completed);
    assert_eq!(
        invoked(&mock).len(),
        1,
        "the default policy attempts once, and a redelivery of a Failed call is no retry"
    );
    let refusals = handle.refusals();
    assert_eq!(
        refusals
            .iter()
            .map(|refusal| refusal.refused)
            .collect::<Vec<_>>(),
        [ToolCallStateConflict {
            current: ToolCallState::Failed
        }]
    );
}

#[test]
fn the_retry_policy_attempts_a_non_idempotent_write_once_whatever_its_limit() {
    let policy = RetryPolicy::up_to(5);
    assert_eq!(policy.attempts(SideEffectClass::ReadOnly), 5);
    assert_eq!(policy.attempts(SideEffectClass::IdempotentWrite), 5);
    assert_eq!(policy.attempts(SideEffectClass::NonIdempotentWrite), 1);
    assert_eq!(RetryPolicy::up_to(0).attempts(SideEffectClass::ReadOnly), 1);
    assert_eq!(
        RetryPolicy::default().attempts(SideEffectClass::ReadOnly),
        1
    );
}
