//! Adversary pass 1 on `story:tool-safe-retries`: the guards of the executor node's retry loop
//! that the unit's own cases leave unexercised (a transient failure only, the one deadline, no
//! attempt after a cancel, the latency of every attempt), and redeliveries interleaved with a
//! second call of the same flaky tool.
//!
//! Each case drives one run of source to executor to sink, as `tests/tool_retries.rs` does, on
//! either the testkit's mock executor or a [`Recording`] executor answering a scripted step per
//! attempt and recording the deadline each attempt was given.

use std::collections::HashMap;
use std::future::{self, Future};
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant};

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
use inference_runtime::run::RunState;
use inference_runtime::{NodeId, RunController, RunError, RunRequests};
use inference_testkit::graph::{SinkHandle, SourceHandle, TextSink, TextSource};
use inference_testkit::tools::{MockToolExecutor, Script};
use inference_text::{OpenTurn, Span, Turn};
use inference_tools::{
    AuthorizeToolCall, CancellationSignal, DeclareTool, ExecutionFuture, Json, RetryPolicy,
    SideEffectClass, Tool, ToolCall, ToolCallAuthorized, ToolCallId, ToolCallState,
    ToolCallStateConflict, ToolExecutor, ToolFailure, ToolFailureKind,
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
    executor: NodeId,
    sink: NodeId,
}

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
        name: "adversary tool retries".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let capacity = NonZeroUsize::new(8).expect("8");
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
    let validated = declaration.validate(pipeline_id).expect("valid");
    let planned = declaration.plan(validated, &catalog).expect("planned");
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

fn failure(kind: ToolFailureKind) -> ToolFailure {
    ToolFailure {
        kind,
        detail: "scripted".to_owned(),
    }
}

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
        .expect("authorized")
        .0
}

/// What a [`Recording`] executor answers to one attempt.
#[derive(Debug, Clone)]
enum Step {
    /// Answers at once.
    Answer(Result<Json, ToolFailure>),
    /// Blocks the polling thread for the span, then answers.
    SleepThen(Duration, Result<Json, ToolFailure>),
    /// Answers nothing until the call is cancelled, then answers this: an executor reporting
    /// its abort as whatever kind it chose.
    UntilCancelledThen(Result<Json, ToolFailure>),
}

/// One attempt a [`Recording`] executor saw.
#[derive(Debug, Clone, Copy)]
struct Attempt {
    call_id: ToolCallId,
    generation_id: GenerationId,
    deadline: Instant,
    at: Instant,
}

/// An executor answering its `n`th attempt overall with `steps[n]`, the last step once they ran
/// out, and recording every attempt before it answers.
struct Recording {
    steps: Vec<Step>,
    log: Mutex<Vec<Attempt>>,
}

impl Recording {
    fn new(steps: Vec<Step>) -> Arc<Self> {
        Arc::new(Self {
            steps,
            log: Mutex::new(Vec::new()),
        })
    }

    fn attempts(&self) -> Vec<Attempt> {
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl ToolExecutor for Recording {
    fn execute<'a>(
        &'a self,
        call: &'a ToolCall,
        deadline: Instant,
        cancellation: &'a CancellationSignal,
    ) -> ExecutionFuture<'a> {
        let index = {
            let mut log = self.log.lock().unwrap_or_else(PoisonError::into_inner);
            log.push(Attempt {
                call_id: call.call_id(),
                generation_id: call.generation_id(),
                deadline,
                at: Instant::now(),
            });
            log.len() - 1
        };
        let step = self.steps[index.min(self.steps.len() - 1)].clone();
        match step {
            Step::Answer(answer) => Box::pin(future::ready(answer)),
            Step::SleepThen(span, answer) => Box::pin(future::poll_fn(move |_| {
                thread::sleep(span);
                Poll::Ready(answer.clone())
            })),
            Step::UntilCancelledThen(answer) => Box::pin(future::poll_fn(move |_| {
                if cancellation.is_cancelled() {
                    Poll::Ready(answer.clone())
                } else {
                    Poll::Pending
                }
            })),
        }
    }
}

struct Running {
    source: SourceHandle<Payload>,
    sink: SinkHandle<Payload>,
    requests: RunRequests,
    drive: JoinHandle<(RunController<Payload>, Result<(), RunError>)>,
}

fn start<E: ToolExecutor + ?Sized + 'static>(
    run_id: RunId,
    deliveries: &[ToolCall],
    executor: ToolExecutorNode<E>,
) -> Running {
    let (declaration, planned, nodes) = planned();
    let held = deliveries.iter().map(|call| {
        let update = ToolCallUpdate {
            call: call.clone(),
            event: ToolCallEvent::Authorized(ToolCallAuthorized {
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
                .expect("every node has an implementation")
        },
    )
    .expect("planned");
    controller.validate().expect("a Created run validates");
    block_on(controller.prepare()).expect("every node prepares");
    block_on(controller.activate()).expect("every node starts");
    let requests = controller.requests();
    let drive = thread::spawn(move || {
        let result = block_on(controller.drive());
        (controller, result)
    });
    Running {
        source: source_handle,
        sink: sink_handle,
        requests,
        drive,
    }
}

impl Running {
    fn finished(self) -> (RunState, Vec<EventEnvelope<Payload>>) {
        while self.source.release() {}
        assert!(self.source.release_end(), "the source admits its end");
        let (controller, result) = self.drive.join().expect("the drive loop does not panic");
        result.expect("the drive loop ends the run");
        (controller.run().state(), self.sink.received())
    }
}

/// The names of the tool-call events the sink received for `call_id`, in order.
fn events(received: &[EventEnvelope<Payload>], call_id: ToolCallId) -> Vec<&'static str> {
    received
        .iter()
        .filter_map(|envelope| match &envelope.payload {
            Payload::Tool(update) if update.call.call_id() == call_id => Some(update.event.name()),
            _ => None,
        })
        .collect()
}

/// The state each tool-call update the sink received for `call_id` left the call in.
fn states(received: &[EventEnvelope<Payload>], call_id: ToolCallId) -> Vec<ToolCallState> {
    received
        .iter()
        .filter_map(|envelope| match &envelope.payload {
            Payload::Tool(update) if update.call.call_id() == call_id => Some(update.call.state()),
            _ => None,
        })
        .collect()
}

/// `docs/design/inference.md` 1007 and the `RetryPolicy` documentation: only `execution_error`
/// is transient. A `deadline_exceeded` or `cancelled` the executor answers itself, well before
/// the node's deadline and with no cancel of the node, is not attempted again.
#[test]
fn adversary1_a_failure_other_than_an_execution_error_is_not_attempted_again() {
    let policy = RetryPolicy::up_to(3);
    assert!(policy.retries(&failure(ToolFailureKind::ExecutionError)));
    assert!(!policy.retries(&failure(ToolFailureKind::DeadlineExceeded)));
    assert!(!policy.retries(&failure(ToolFailureKind::Cancelled)));

    for kind in [
        ToolFailureKind::DeadlineExceeded,
        ToolFailureKind::Cancelled,
    ] {
        let lookup = declare("lookup", SideEffectClass::ReadOnly);
        let recording = Recording::new(vec![
            Step::Answer(Err(failure(kind))),
            Step::Answer(Ok(json!({"ok": true}))),
        ]);
        let call = authorized(&lookup);
        let run_id = RunId::generate();
        let executor =
            ToolExecutorNode::new(run_id, Arc::clone(&recording), [lookup.tool_id()], PATIENCE)
                .with_retries(policy);
        let handle = executor.handle();
        let (state, received) = start(run_id, std::slice::from_ref(&call), executor).finished();
        assert_eq!(state, RunState::Completed);
        assert_eq!(recording.attempts().len(), 1, "{kind:?} is attempted once");
        assert_eq!(
            states(&received, call.call_id()),
            [ToolCallState::Started, ToolCallState::Failed],
            "{kind:?}"
        );
        let measures = handle.measures();
        assert_eq!(
            measures
                .iter()
                .map(|measure| (measure.attempts, measure.failure))
                .collect::<Vec<_>>(),
            [(1, Some(kind))]
        );
    }
}

/// Every attempt of one call is given the one deadline the call started with, and presents the
/// call's call_id and generation_id.
#[test]
fn adversary1_every_attempt_is_given_the_one_deadline_the_call_started_with() {
    let lookup = declare("lookup", SideEffectClass::ReadOnly);
    let hiccup = Err(failure(ToolFailureKind::ExecutionError));
    let recording = Recording::new(vec![
        Step::SleepThen(Duration::from_millis(25), hiccup.clone()),
        Step::SleepThen(Duration::from_millis(25), hiccup),
        Step::Answer(Ok(json!({"ok": true}))),
    ]);
    let call = authorized(&lookup);
    let run_id = RunId::generate();
    let executor =
        ToolExecutorNode::new(run_id, Arc::clone(&recording), [lookup.tool_id()], PATIENCE)
            .with_retries(RetryPolicy::up_to(3));
    let (state, received) = start(run_id, std::slice::from_ref(&call), executor).finished();
    assert_eq!(state, RunState::Completed);
    let attempts = recording.attempts();
    assert_eq!(attempts.len(), 3);
    assert!(
        attempts
            .iter()
            .all(|attempt| attempt.deadline == attempts[0].deadline),
        "one deadline for every attempt: {attempts:?}"
    );
    assert!(
        attempts
            .iter()
            .all(|attempt| (attempt.call_id, attempt.generation_id)
                == (call.call_id(), call.generation_id()))
    );
    assert_eq!(
        events(&received, call.call_id()),
        [
            "inference.tools.ToolCallStarted",
            "inference.tools.ToolCallResult"
        ]
    );
}

/// The measure's latency runs from the start to the end of the call, every attempt among it
/// (1025), and its attempts are the attempts the executor saw.
#[test]
fn adversary1_the_measure_covers_every_attempt() {
    let lookup = declare("lookup", SideEffectClass::IdempotentWrite);
    let hiccup = Err(failure(ToolFailureKind::ExecutionError));
    let recording = Recording::new(vec![
        Step::SleepThen(Duration::from_millis(40), hiccup.clone()),
        Step::SleepThen(Duration::from_millis(40), hiccup),
        Step::Answer(Ok(json!({"ok": true}))),
    ]);
    let call = authorized(&lookup);
    let run_id = RunId::generate();
    let executor =
        ToolExecutorNode::new(run_id, Arc::clone(&recording), [lookup.tool_id()], PATIENCE)
            .with_retries(RetryPolicy::up_to(4));
    let handle = executor.handle();
    let (state, _) = start(run_id, std::slice::from_ref(&call), executor).finished();
    assert_eq!(state, RunState::Completed);
    let measures = handle.measures();
    assert_eq!(measures.len(), 1);
    assert_eq!(
        (measures[0].attempts, measures[0].failure),
        (
            u32::try_from(recording.attempts().len()).expect("small"),
            None
        )
    );
    assert_eq!(measures[0].attempts, 3);
    assert!(
        measures[0].latency >= Duration::from_millis(80),
        "two attempts of 40ms each lie inside the call's latency: {:?}",
        measures[0].latency
    );
}

/// No attempt starts once the call's one deadline passed: an attempt that answers a transient
/// failure after the deadline is the last.
#[test]
fn adversary1_no_attempt_starts_after_the_shared_deadline() {
    let lookup = declare("lookup", SideEffectClass::ReadOnly);
    let recording = Recording::new(vec![
        Step::SleepThen(
            Duration::from_millis(120),
            Err(failure(ToolFailureKind::ExecutionError)),
        ),
        Step::Answer(Ok(json!({"ok": true}))),
    ]);
    let call = authorized(&lookup);
    let run_id = RunId::generate();
    let executor = ToolExecutorNode::new(
        run_id,
        Arc::clone(&recording),
        [lookup.tool_id()],
        Duration::from_millis(60),
    )
    .with_retries(RetryPolicy::up_to(5));
    let (state, received) = start(run_id, std::slice::from_ref(&call), executor).finished();
    assert_eq!(state, RunState::Completed);
    let attempts = recording.attempts();
    assert!(
        attempts.iter().all(|attempt| attempt.at < attempt.deadline),
        "no attempt started past the deadline: {attempts:?}"
    );
    assert_eq!(attempts.len(), 1);
    assert_eq!(
        states(&received, call.call_id()),
        [ToolCallState::Started, ToolCallState::Failed]
    );
}

/// No attempt starts once the node was cancelled, even when the executor reports its abort as a
/// transient `execution_error` rather than `cancelled`.
#[test]
fn adversary1_no_attempt_starts_after_the_run_is_cancelled() {
    let lookup = declare("lookup", SideEffectClass::ReadOnly);
    let recording = Recording::new(vec![
        Step::UntilCancelledThen(Err(failure(ToolFailureKind::ExecutionError))),
        Step::Answer(Ok(json!({"ok": true}))),
    ]);
    let call = authorized(&lookup);
    let run_id = RunId::generate();
    let executor =
        ToolExecutorNode::new(run_id, Arc::clone(&recording), [lookup.tool_id()], PATIENCE)
            .with_retries(RetryPolicy::up_to(5));
    let handle = executor.handle();
    let run = start(run_id, std::slice::from_ref(&call), executor);
    while run.source.release() {}
    let until = Instant::now() + PATIENCE;
    while recording.attempts().is_empty() && Instant::now() < until {
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        recording.attempts().len(),
        1,
        "the first attempt is running"
    );
    run.requests.cancel();
    let (controller, _) = run.drive.join().expect("the drive loop does not panic");
    assert_eq!(controller.run().state(), RunState::Cancelled);
    assert_eq!(
        recording.attempts().len(),
        1,
        "nothing is attempted after the cancel"
    );
    assert_eq!(
        events(&run.sink.received(), call.call_id()),
        ["inference.tools.ToolCallStarted"],
        "nothing is sent after the cancel"
    );
    assert!(
        handle
            .measures()
            .iter()
            .all(|measure| measure.attempts <= 1),
        "{:?}",
        handle.measures()
    );
}

/// A non-idempotent write is attempted once under the largest limit, by the node, with an
/// executor that fails it transiently every time.
#[test]
fn adversary1_a_non_idempotent_write_is_attempted_once_under_the_largest_limit() {
    let transfer = declare("transfer", SideEffectClass::NonIdempotentWrite);
    let recording = Recording::new(vec![Step::Answer(Err(failure(
        ToolFailureKind::ExecutionError,
    )))]);
    let call = authorized(&transfer);
    let run_id = RunId::generate();
    let executor = ToolExecutorNode::new(
        run_id,
        Arc::clone(&recording),
        [transfer.tool_id()],
        PATIENCE,
    )
    .with_retries(RetryPolicy::up_to(u32::MAX));
    let handle = executor.handle();
    let (state, received) = start(run_id, std::slice::from_ref(&call), executor).finished();
    assert_eq!(state, RunState::Completed);
    assert_eq!(recording.attempts().len(), 1);
    assert_eq!(
        states(&received, call.call_id()),
        [ToolCallState::Started, ToolCallState::Failed]
    );
    assert_eq!(handle.measures()[0].attempts, 1);
}

/// Two calls of one flaky tool, each delivered twice and interleaved: the mock counts failures
/// per call_id, each call completes on its second attempt, and each redelivery is one refusal.
#[test]
fn adversary1_two_calls_of_one_flaky_tool_interleaved_with_their_redeliveries() {
    let lookup = declare("lookup", SideEffectClass::ReadOnly);
    let mock = Arc::new(MockToolExecutor::new([(
        lookup.tool_id(),
        Script::FailThenSucceed {
            failures: 1,
            failure: failure(ToolFailureKind::ExecutionError),
            result: json!({"ok": true}),
        },
    )]));
    let (first, second) = (authorized(&lookup), authorized(&lookup));
    let run_id = RunId::generate();
    let executor = ToolExecutorNode::new(run_id, Arc::clone(&mock), [lookup.tool_id()], PATIENCE)
        .with_retries(RetryPolicy::up_to(2));
    let handle = executor.handle();
    let deliveries = [first.clone(), second.clone(), first.clone(), second.clone()];
    let (state, received) = start(run_id, &deliveries, executor).finished();
    assert_eq!(state, RunState::Completed);
    assert_eq!(
        mock.invocations()
            .iter()
            .map(|invocation| invocation.call_id)
            .collect::<Vec<_>>(),
        [
            first.call_id(),
            first.call_id(),
            second.call_id(),
            second.call_id()
        ]
    );
    for call in [&first, &second] {
        assert_eq!(
            states(&received, call.call_id()),
            [ToolCallState::Started, ToolCallState::Completed]
        );
    }
    assert_eq!(
        handle
            .refusals()
            .iter()
            .map(|refusal| (refusal.call.call_id(), refusal.refused))
            .collect::<Vec<_>>(),
        [
            (
                first.call_id(),
                ToolCallStateConflict {
                    current: ToolCallState::Completed
                }
            ),
            (
                second.call_id(),
                ToolCallStateConflict {
                    current: ToolCallState::Completed
                }
            ),
        ]
    );
    assert_eq!(
        handle
            .measures()
            .iter()
            .map(|measure| (measure.call_id, measure.attempts))
            .collect::<Vec<_>>(),
        [(first.call_id(), 2), (second.call_id(), 2)]
    );
}
