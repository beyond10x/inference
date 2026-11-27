//! Second adversary pass on the tool executor node's answer to a tool that panics: the retry
//! policy around a panic, a panic that carries no message, a future that panics on a later
//! poll, and a tool that never answers beside one that panics. The node is driven through its
//! `NodeImplementation` over endpoints of the test's own, as `tests/tool_executor_panic.rs`
//! drives it.

use std::collections::VecDeque;
use std::future::{self, Future};
use std::pin::pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use inference_core::endpoint::{EndpointFuture, InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::generation::{Generation, StartGeneration};
use inference_core::id::{NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::nodes::{
    ToolCallEvent, ToolCallMeasure, ToolCallUpdate, ToolExecutorNode, ToolPayload,
};
use inference_pipeline::{
    Catalog, Declaration, InteractionMode, NodeEndpoints, NodeImplementation, NodeInput,
    NodeOutput, NodeRole, PipelineSpec, PortId,
};
use inference_text::{OpenTurn, Span, Turn};
use inference_tools::{
    AuthorizeToolCall, CancellationSignal, DeclareTool, ExecutionFuture, RetryPolicy,
    SideEffectClass, Tool, ToolCall, ToolCallAuthorized, ToolCallId, ToolExecutor, ToolFailure,
    ToolFailureKind,
};
use serde_json::json;

/// How long a case waits for the node before it fails rather than hangs.
const PATIENCE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Payload {
    Tool(ToolCallUpdate),
    Control(ControlKind),
}

impl ToolPayload for Payload {
    fn into_tool_call(self) -> Result<ToolCallUpdate, Self> {
        match self {
            Self::Tool(update) => Ok(update),
            other => Err(other),
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

fn declared() -> (Declaration, NodeId) {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    let catalog = Fixture { profile };
    let spec = PipelineSpec {
        name: "tool executor panic, second pass".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let mut declaration = Declaration::new();
    let (_, node_id) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            pipeline.node("node", NodeRole::Tool, Vec::new()).id()
        })
        .expect("the profile is declared");
    (declaration, node_id)
}

/// A read-only tool: the class a retry policy attempts again.
fn tool() -> Tool {
    Tool::declare(DeclareTool {
        name: "lookup".to_owned(),
        description: String::new(),
        schema: json!({"type": "object"}),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0
}

fn authorized(tool: &Tool) -> ToolCall {
    let generation = Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    })
    .0;
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
    let proposed = ToolCall::propose(tool, &generation, &span, json!({}))
        .expect("an Active generation proposes on a committed span")
        .0;
    proposed
        .authorize(&AuthorizeToolCall {
            call_id: proposed.call_id(),
            confirmation_required: false,
            confirmed: false,
        })
        .expect("authorized")
        .0
}

fn envelope(kind: &str, payload: Payload) -> EventEnvelope<Payload> {
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id: RunId::generate(),
        session_id: None,
        stream_id: StreamId::generate(),
        sequence: 1,
        producer: NodeId::generate(),
        port: "authorized".to_owned(),
        kind: kind.to_owned(),
        generation_id: None,
        timestamp: 0,
        timebase: Timebase::WallTime,
        causation_id: None,
        payload,
    }
}

fn delivery(call: &ToolCall) -> EventEnvelope<Payload> {
    let update = ToolCallUpdate {
        call: call.clone(),
        event: ToolCallEvent::Authorized(ToolCallAuthorized {
            call_id: call.call_id(),
        }),
    };
    envelope(update.event.name(), Payload::Tool(update))
}

fn end_of_input() -> EventEnvelope<Payload> {
    envelope(
        ControlKind::Stop.as_str(),
        Payload::Control(ControlKind::Stop),
    )
}

/// The input edge: every envelope it was given, ready at once, then nothing, never closed.
struct Supply(VecDeque<EventEnvelope<Payload>>);

impl InputEndpoint<Payload> for Supply {
    fn recv(&mut self) -> EndpointFuture<'_, Option<EventEnvelope<Payload>>> {
        let next = self.0.pop_front();
        Box::pin(future::poll_fn(move |_| {
            next.clone()
                .map_or(Poll::Pending, |envelope| Poll::Ready(Some(envelope)))
        }))
    }
}

/// The output edge: it keeps the payload of every envelope sent on it.
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<Payload>>>);

impl Recorder {
    fn sent(&self) -> Vec<Payload> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl OutputEndpoint<Payload> for Recorder {
    fn send(
        &self,
        envelope: EventEnvelope<Payload>,
    ) -> EndpointFuture<'_, Result<(), SendError<Payload>>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(envelope.payload);
        Box::pin(future::ready(Ok(())))
    }
}

/// What one invocation of the scripted executor does.
#[derive(Debug, Clone, Copy)]
enum Step {
    /// Panics with a string literal as the tool is called.
    PanicCalled,
    /// Panics with a formatted message as its future is first polled.
    PanicFormatted,
    /// Panics with a payload that is no string.
    PanicNoMessage,
    /// Its future wakes itself and is pending once, then panics on its second poll.
    PanicSecondPoll,
    /// Answers an `execution_error`, the failure a retry policy takes as transient.
    Fail,
    /// Never answers and never wakes.
    Never,
    /// Answers `{"answer": 42}`.
    Answer,
}

/// An executor that takes the next step of its script at each invocation, and answers once the
/// script is spent.
struct Scripted {
    script: Mutex<VecDeque<Step>>,
    invoked: AtomicUsize,
}

impl Scripted {
    fn new(script: impl IntoIterator<Item = Step>) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into_iter().collect()),
            invoked: AtomicUsize::new(0),
        })
    }
}

impl ToolExecutor for Scripted {
    fn execute<'a>(
        &'a self,
        _: &'a ToolCall,
        _: Instant,
        _: &'a CancellationSignal,
    ) -> ExecutionFuture<'a> {
        let number = self.invoked.fetch_add(1, Ordering::SeqCst) + 1;
        let step = self
            .script
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
            .unwrap_or(Step::Answer);
        match step {
            Step::PanicCalled => panic!("the tool has a defect"),
            Step::PanicFormatted => Box::pin(future::poll_fn(move |_| {
                panic!("the tool has a defect at invocation {number}")
            })),
            Step::PanicNoMessage => std::panic::panic_any(7_u32),
            Step::PanicSecondPoll => {
                let mut polled = false;
                Box::pin(future::poll_fn(move |cx| {
                    assert!(!polled, "the tool has a defect on its second poll");
                    polled = true;
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }))
            }
            Step::Fail => Box::pin(future::ready(Err(ToolFailure {
                kind: ToolFailureKind::ExecutionError,
                detail: "the backend was busy".to_owned(),
            }))),
            Step::Never => Box::pin(future::pending()),
            Step::Answer => Box::pin(future::ready(Ok(json!({"answer": 42})))),
        }
    }
}

/// What the node sent about `call_id`, as the names of the steps, with the kind and detail of
/// the failure of a failed call.
fn steps(sent: &[Payload], call_id: ToolCallId) -> Vec<String> {
    sent.iter()
        .filter_map(|payload| match payload {
            Payload::Tool(update) if update.call.call_id() == call_id => Some(update),
            _ => None,
        })
        .map(|update| match &update.event {
            ToolCallEvent::Started(_) => "started".to_owned(),
            ToolCallEvent::Result(result) => format!("result {}", result.result),
            ToolCallEvent::Failed(failed) => {
                format!(
                    "failed {:?}: {}",
                    failed.failure.kind, failed.failure.detail
                )
            }
            other => format!("{other:?}"),
        })
        .collect()
}

/// What a run of the node came to.
struct Ran {
    calls: Vec<ToolCall>,
    sent: Vec<Payload>,
    drained: Result<(), String>,
    measures: Vec<ToolCallMeasure>,
    failure: Option<String>,
    invoked: usize,
}

/// Drives a node over `count` authorized calls and the end of input, on an executor running
/// `script`, with `retries` and `deadline`, and waits for its drain.
fn run(
    script: impl IntoIterator<Item = Step>,
    count: usize,
    retries: RetryPolicy,
    deadline: Duration,
) -> Ran {
    let lookup = tool();
    let calls: Vec<ToolCall> = (0..count).map(|_| authorized(&lookup)).collect();
    let (declaration, node_id) = declared();
    let executor = Scripted::new(script);
    let mut node = ToolExecutorNode::new(
        RunId::generate(),
        Arc::clone(&executor),
        [lookup.tool_id()],
        deadline,
    )
    .with_retries(retries);
    let handle = node.handle();
    let output = Recorder::default();
    block_on(NodeImplementation::<Payload>::describe(
        &mut node,
        declaration.node(node_id).expect("declared"),
    ));
    block_on(NodeImplementation::<Payload>::prepare(&mut node)).expect("prepares");
    let mut supply: VecDeque<_> = calls.iter().map(delivery).collect();
    supply.push_back(end_of_input());
    block_on(NodeImplementation::<Payload>::start(
        &mut node,
        NodeEndpoints {
            inputs: vec![NodeInput {
                port_id: PortId::generate(),
                endpoint: Box::new(Supply(supply)),
            }],
            outputs: vec![NodeOutput {
                port_id: PortId::generate(),
                endpoint: Box::new(output.clone()),
            }],
        },
    ))
    .expect("starts");

    let (told, drained) = mpsc::channel();
    thread::spawn(move || {
        let drained = block_on(NodeImplementation::<Payload>::drain(&mut node));
        let _ = told.send(drained.map_err(|error| error.to_string()));
    });
    let drained = drained
        .recv_timeout(PATIENCE)
        .expect("the node ends at the end of its input");
    Ran {
        calls,
        sent: output.sent(),
        drained,
        measures: handle.measures(),
        failure: handle.failure(),
        invoked: executor.invoked.load(Ordering::SeqCst),
    }
}

const ANSWERED: [&str; 2] = ["started", r#"result {"answer":42}"#];

/// "A call that panicked is not attempted again, whatever the node's `RetryPolicy`": a read-only
/// call under a policy of three attempts, whose tool panics as it is called.
#[test]
fn a_call_whose_tool_panicked_as_it_was_called_is_not_attempted_again_under_three_attempts() {
    let ran = run(
        [Step::PanicCalled],
        2,
        RetryPolicy::up_to(3),
        Duration::from_secs(30),
    );

    assert_eq!(ran.drained, Ok(()), "{:#?}", ran.sent);
    assert_eq!(
        steps(&ran.sent, ran.calls[0].call_id()),
        [
            "started",
            "failed ExecutionError: the tool's executor panicked: the tool has a defect"
        ]
    );
    assert_eq!(steps(&ran.sent, ran.calls[1].call_id()), ANSWERED);
    assert_eq!(
        ran.invoked, 2,
        "one invocation of the call that panicked, one of the next"
    );
    let attempts: Vec<u32> = ran
        .measures
        .iter()
        .map(|measure| measure.attempts)
        .collect();
    assert_eq!(attempts, [1, 1]);
}

/// The same for a tool whose future panics with a formatted message, a `String` payload.
#[test]
fn a_call_whose_tools_future_panicked_is_not_attempted_again_under_three_attempts() {
    let ran = run(
        [Step::PanicFormatted],
        2,
        RetryPolicy::up_to(3),
        Duration::from_secs(30),
    );

    assert_eq!(ran.drained, Ok(()), "{:#?}", ran.sent);
    assert_eq!(
        steps(&ran.sent, ran.calls[0].call_id()),
        [
            "started",
            "failed ExecutionError: the tool's executor panicked: the tool has a defect at \
             invocation 1"
        ]
    );
    assert_eq!(steps(&ran.sent, ran.calls[1].call_id()), ANSWERED);
    assert_eq!(ran.invoked, 2);
    let attempts: Vec<u32> = ran
        .measures
        .iter()
        .map(|measure| measure.attempts)
        .collect();
    assert_eq!(attempts, [1, 1]);
}

/// A transient failure is attempted again as before; the attempt that panics is the last.
#[test]
fn a_call_that_failed_transiently_and_then_panicked_ends_failed_after_two_attempts() {
    let ran = run(
        [Step::Fail, Step::PanicCalled],
        2,
        RetryPolicy::up_to(3),
        Duration::from_secs(30),
    );

    assert_eq!(ran.drained, Ok(()), "{:#?}", ran.sent);
    assert_eq!(
        steps(&ran.sent, ran.calls[0].call_id()),
        [
            "started",
            "failed ExecutionError: the tool's executor panicked: the tool has a defect"
        ]
    );
    assert_eq!(steps(&ran.sent, ran.calls[1].call_id()), ANSWERED);
    assert_eq!(ran.invoked, 3);
    let attempts: Vec<u32> = ran
        .measures
        .iter()
        .map(|measure| measure.attempts)
        .collect();
    assert_eq!(attempts, [2, 1]);
}

/// What a retry policy meant before the panic catch: a transient failure of a read-only call is
/// attempted again, up to the limit, and the answer of a later attempt is the call's result.
#[test]
fn a_transient_failure_is_attempted_again_and_its_later_answer_is_the_result() {
    let ran = run(
        [Step::Fail, Step::Fail],
        1,
        RetryPolicy::up_to(3),
        Duration::from_secs(30),
    );

    assert_eq!(ran.drained, Ok(()), "{:#?}", ran.sent);
    assert_eq!(steps(&ran.sent, ran.calls[0].call_id()), ANSWERED);
    assert_eq!(ran.invoked, 3);
    assert_eq!(ran.measures[0].attempts, 3);
    assert_eq!(ran.measures[0].failure, None);
}

/// And a transient failure that stays past the limit ends the call failed with it.
#[test]
fn a_transient_failure_past_the_limit_fails_the_call_with_it() {
    let ran = run(
        [Step::Fail, Step::Fail, Step::Fail],
        2,
        RetryPolicy::up_to(3),
        Duration::from_secs(30),
    );

    assert_eq!(ran.drained, Ok(()), "{:#?}", ran.sent);
    assert_eq!(
        steps(&ran.sent, ran.calls[0].call_id()),
        ["started", "failed ExecutionError: the backend was busy"]
    );
    assert_eq!(steps(&ran.sent, ran.calls[1].call_id()), ANSWERED);
    assert_eq!(ran.invoked, 4);
}

/// A panic whose payload is no string: the failure says so, and the node goes on.
#[test]
fn a_panic_that_carries_no_message_fails_its_call_and_the_node_serves_the_next() {
    let ran = run(
        [Step::PanicNoMessage],
        2,
        RetryPolicy::default(),
        Duration::from_secs(30),
    );

    assert_eq!(ran.drained, Ok(()), "{:#?}", ran.sent);
    assert_eq!(
        steps(&ran.sent, ran.calls[0].call_id()),
        [
            "started",
            "failed ExecutionError: the tool's executor panicked: a panic without a message"
        ]
    );
    assert_eq!(steps(&ran.sent, ran.calls[1].call_id()), ANSWERED);
    assert_eq!(ran.failure, None);
}

/// A future that was pending once and panics on its second poll.
#[test]
fn a_future_that_panics_on_its_second_poll_fails_its_call_and_the_node_serves_the_next() {
    let ran = run(
        [Step::PanicSecondPoll],
        2,
        RetryPolicy::up_to(3),
        Duration::from_secs(30),
    );

    assert_eq!(ran.drained, Ok(()), "{:#?}", ran.sent);
    let failed = steps(&ran.sent, ran.calls[0].call_id());
    assert_eq!(failed.len(), 2, "{failed:?}");
    assert!(
        failed[1].starts_with("failed ExecutionError: the tool's executor panicked: ")
            && failed[1].contains("the tool has a defect on its second poll"),
        "{failed:?}"
    );
    assert_eq!(steps(&ran.sent, ran.calls[1].call_id()), ANSWERED);
    assert_eq!(ran.invoked, 2, "the call that panicked was invoked once");
}

/// A tool that never answers fails at the node's deadline, here 100 ms, the call after it
/// panics, and the third is served: neither failure keeps the node from the next call.
#[test]
fn a_tool_that_never_answers_and_one_that_panics_each_fail_their_call_alone() {
    let started = Instant::now();
    let ran = run(
        [Step::Never, Step::PanicCalled],
        3,
        RetryPolicy::up_to(3),
        Duration::from_millis(100),
    );

    assert_eq!(ran.drained, Ok(()), "{:#?}", ran.sent);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "the deadline of 100 ms ended the first call, not the patience of the test"
    );
    let overran = steps(&ran.sent, ran.calls[0].call_id());
    assert_eq!(overran.len(), 2, "{overran:?}");
    assert!(
        overran[1].starts_with("failed DeadlineExceeded: "),
        "{overran:?}"
    );
    assert_eq!(
        steps(&ran.sent, ran.calls[1].call_id()),
        [
            "started",
            "failed ExecutionError: the tool's executor panicked: the tool has a defect"
        ]
    );
    assert_eq!(steps(&ran.sent, ran.calls[2].call_id()), ANSWERED);
    assert_eq!(ran.invoked, 3);
    let failures: Vec<_> = ran.measures.iter().map(|measure| measure.failure).collect();
    assert_eq!(
        failures,
        [
            Some(ToolFailureKind::DeadlineExceeded),
            Some(ToolFailureKind::ExecutionError),
            None
        ]
    );
}
