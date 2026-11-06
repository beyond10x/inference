//! Review fixes of the tool authority and the tool executor node
//! (`story:pipeline-tool-and-node-cancel-findings`): a confirmed call queued behind the answer of
//! a withdrawn one is authorized on the next poll (W5), the executor's read-ahead while a call
//! runs is bounded so its input edge applies backpressure (N16), and a cancel racing a delivered
//! call neither starts it nor polls its executor (N17). Each node is driven through its
//! `NodeImplementation`, as the run controller drives it, over endpoints of the test's own.

use std::collections::VecDeque;
use std::future::{self, Future};
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use inference_core::endpoint::{EndpointFuture, InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::generation::{Generation, StartGeneration};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::nodes::{
    AuthorityHandle, AuthorityOutcome, ToolAuthority, ToolCallEvent, ToolCallUpdate,
    ToolExecutorNode, ToolPayload,
};
use inference_pipeline::{
    Catalog, Declaration, InteractionMode, NodeEndpoints, NodeImplementation, NodeInput,
    NodeOutput, NodeRole, PipelineSpec, PortId,
};
use inference_text::{OpenTurn, Span, Turn};
use inference_tools::{
    AuthorizeToolCall, CancellationSignal, DeclareTool, ExecutionFuture, SideEffectClass, Tool,
    ToolCall, ToolCallAuthorized, ToolCallId, ToolCallProposed, ToolCallState, ToolExecutor,
};
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Payload {
    Tool(ToolCallUpdate),
    Control(ControlKind),
    Other,
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

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
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

/// A declaration of one node of `role`, to describe the node under test with.
fn declared(role: NodeRole) -> (Declaration, NodeId) {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    let catalog = Fixture { profile };
    let spec = PipelineSpec {
        name: "tool nodes".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let mut declaration = Declaration::new();
    let (_, node_id) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            pipeline.node("node", role, Vec::new()).id()
        })
        .expect("the profile is declared");
    (declaration, node_id)
}

fn tool(requires_confirmation: bool) -> Tool {
    Tool::declare(DeclareTool {
        name: "lookup".to_owned(),
        description: String::new(),
        schema: json!({"type": "object"}),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation,
    })
    .0
}

fn generation() -> Generation {
    Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    })
    .0
}

fn proposed(tool: &Tool, generation: &Generation) -> ToolCall {
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
    ToolCall::propose(tool, generation, &span, json!({}))
        .expect("an Active generation proposes on a committed span")
        .0
}

fn authorized(tool: &Tool) -> ToolCall {
    let proposed = proposed(tool, &generation());
    proposed
        .authorize(&AuthorizeToolCall {
            call_id: proposed.call_id(),
            confirmation_required: false,
            confirmed: false,
        })
        .expect("authorized")
        .0
}

fn envelope(
    kind: &str,
    generation_id: Option<GenerationId>,
    payload: Payload,
) -> EventEnvelope<Payload> {
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id: RunId::generate(),
        session_id: None,
        stream_id: StreamId::generate(),
        sequence: 1,
        producer: NodeId::generate(),
        port: "out".to_owned(),
        kind: kind.to_owned(),
        generation_id,
        timestamp: 0,
        timebase: Timebase::WallTime,
        causation_id: None,
        payload,
    }
}

fn update(call: &ToolCall, event: ToolCallEvent) -> EventEnvelope<Payload> {
    let update = ToolCallUpdate {
        call: call.clone(),
        event,
    };
    envelope(
        update.event.name(),
        Some(call.generation_id()),
        Payload::Tool(update),
    )
}

fn proposal(call: &ToolCall) -> EventEnvelope<Payload> {
    update(
        call,
        ToolCallEvent::Proposed(ToolCallProposed {
            call_id: call.call_id(),
            tool_id: call.tool_id(),
            generation_id: call.generation_id(),
        }),
    )
}

fn delivery(call: &ToolCall) -> EventEnvelope<Payload> {
    update(
        call,
        ToolCallEvent::Authorized(ToolCallAuthorized {
            call_id: call.call_id(),
        }),
    )
}

fn stop() -> EventEnvelope<Payload> {
    envelope(
        ControlKind::Stop.as_str(),
        None,
        Payload::Control(ControlKind::Stop),
    )
}

/// The output edge: always has room, and records the kind of every envelope it takes.
#[derive(Clone, Default)]
struct Taken(Arc<Mutex<Vec<String>>>);

impl OutputEndpoint<Payload> for Taken {
    fn send(
        &self,
        envelope: EventEnvelope<Payload>,
    ) -> EndpointFuture<'_, Result<(), SendError<Payload>>> {
        lock(&self.0).push(envelope.kind);
        Box::pin(future::ready(Ok(())))
    }
}

fn endpoints(
    input: impl InputEndpoint<Payload> + 'static,
    output: impl OutputEndpoint<Payload> + 'static,
) -> NodeEndpoints<Payload> {
    NodeEndpoints {
        inputs: vec![NodeInput {
            port_id: PortId::generate(),
            endpoint: Box::new(input),
        }],
        outputs: vec![NodeOutput {
            port_id: PortId::generate(),
            endpoint: Box::new(output),
        }],
    }
}

/// The authority's output edge in the W5 case: as [`Taken`], except that the send of a forwarded
/// interruption finds the edge full at its first poll and takes it at the next, waking nobody
/// itself. The hook's answers, given while the authority read the interruption, woke its thread,
/// and that wake is still unused; the thread parks once here, which uses it up and polls the send
/// again. What the authority does next it does on its own: nothing outside it wakes it again until
/// the test sends the end of input.
struct WaitsOnceOnInterruption(Taken);

impl OutputEndpoint<Payload> for WaitsOnceOnInterruption {
    fn send(
        &self,
        envelope: EventEnvelope<Payload>,
    ) -> EndpointFuture<'_, Result<(), SendError<Payload>>> {
        let mut full = envelope.kind == "inference.stream.GenerationCancelled";
        let mut envelope = Some(envelope);
        Box::pin(future::poll_fn(move |_| {
            if full {
                full = false;
                return Poll::Pending;
            }
            if let Some(envelope) = envelope.take() {
                lock(&self.0.0).push(envelope.kind);
            }
            Poll::Ready(Ok(()))
        }))
    }
}

/// What a scripted input hands over next: an envelope, or an envelope it hands over only after it
/// confirmed `confirm` through the authority's hook, on the same poll.
enum Step {
    Envelope(EventEnvelope<Payload>),
    Confirming(EventEnvelope<Payload>, AuthorityHandle, Vec<ToolCallId>),
}

/// The input edge: the steps it was given, then nothing until the test pushes more and wakes it.
#[derive(Clone, Default)]
struct Script(Arc<Mutex<(VecDeque<Step>, Option<Waker>)>>);

impl Script {
    fn push(&self, step: Step) {
        let waker = {
            let mut script = lock(&self.0);
            script.0.push_back(step);
            script.1.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

impl InputEndpoint<Payload> for Script {
    fn recv(&mut self) -> EndpointFuture<'_, Option<EventEnvelope<Payload>>> {
        let script = Arc::clone(&self.0);
        Box::pin(future::poll_fn(move |cx| {
            let mut guard = lock(&script);
            match guard.0.pop_front() {
                Some(Step::Envelope(envelope)) => Poll::Ready(Some(envelope)),
                Some(Step::Confirming(envelope, hook, calls)) => {
                    drop(guard);
                    for call_id in calls {
                        assert!(hook.confirm(call_id), "the call {call_id} is held");
                    }
                    Poll::Ready(Some(envelope))
                }
                None => {
                    guard.1 = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        }))
    }
}

/// Runs `work` on a thread of its own and answers its result, or nothing when it has not ended
/// within `within`.
fn ends_within<T: Send + 'static>(
    within: Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (sent, received) = mpsc::channel();
    thread::spawn(move || {
        let _ = sent.send(work());
    });
    received.recv_timeout(within).ok()
}

/// Whether `ready` holds within `within`, asked every few milliseconds.
fn eventually(within: Duration, ready: impl Fn() -> bool) -> bool {
    let until = Instant::now() + within;
    while Instant::now() < until {
        if ready() {
            return true;
        }
        thread::sleep(Duration::from_millis(5));
    }
    ready()
}

/// W5: calls `a` (generation one) and `b` (generation two) are held for confirmation; the host
/// confirms both, and the interruption of generation one withdraws `a` before the authority acts
/// on the answers. The answer of `a` names a call no longer held; `b`'s, queued behind it, is
/// acted on at the same poll, with no other wake, and the drain ends.
#[test]
fn a_confirmed_call_queued_behind_the_answer_of_a_withdrawn_one_is_authorized_and_drain_ends() {
    let confirming = tool(true);
    let (first, second) = (generation(), generation());
    let (a, b) = (
        proposed(&confirming, &first),
        proposed(&confirming, &second),
    );
    let (declaration, node_id) = declared(NodeRole::Policy);
    let mut authority = ToolAuthority::new(RunId::generate(), [confirming]);
    let hook = authority.handle();
    let script = Script::default();
    script.push(Step::Envelope(proposal(&a)));
    script.push(Step::Envelope(proposal(&b)));
    script.push(Step::Confirming(
        envelope(
            "inference.stream.GenerationCancelled",
            Some(first.generation_id()),
            Payload::Other,
        ),
        hook.clone(),
        vec![a.call_id(), b.call_id()],
    ));
    let taken = Taken::default();
    block_on(NodeImplementation::<Payload>::describe(
        &mut authority,
        declaration.node(node_id).expect("declared"),
    ));
    block_on(NodeImplementation::<Payload>::prepare(&mut authority)).expect("prepares");
    block_on(NodeImplementation::<Payload>::start(
        &mut authority,
        endpoints(script.clone(), WaitsOnceOnInterruption(taken.clone())),
    ))
    .expect("starts");

    let authorized_b = || {
        hook.decisions().iter().any(|decision| {
            decision.call.call_id() == b.call_id()
                && matches!(decision.outcome, AuthorityOutcome::Authorized(_))
        })
    };
    assert!(
        eventually(Duration::from_secs(3), authorized_b),
        "the confirmed call of generation two is authorized without another wake; decisions {:?}, \
         held {:?}",
        hook.decisions()
            .iter()
            .map(|decision| (decision.call.call_id(), decision.outcome.clone()))
            .collect::<Vec<_>>(),
        hook.held()
    );
    assert!(
        hook.calls()
            .iter()
            .any(|call| call.call_id() == a.call_id() && call.state() == ToolCallState::Rejected),
        "the withdrawn call is rejected superseded"
    );

    script.push(Step::Envelope(stop()));
    let drained = ends_within(Duration::from_secs(5), move || {
        block_on(NodeImplementation::<Payload>::drain(&mut authority))
    });
    assert_eq!(drained, Some(Ok(())), "the drain ends");
}

/// An executor whose execution never finishes on its own; it counts how often it was invoked.
#[derive(Default)]
struct Endless {
    invoked: AtomicUsize,
}

impl ToolExecutor for Endless {
    fn execute<'a>(
        &'a self,
        _: &'a ToolCall,
        _: Instant,
        _: &'a CancellationSignal,
    ) -> ExecutionFuture<'a> {
        self.invoked.fetch_add(1, Ordering::SeqCst);
        Box::pin(future::pending())
    }
}

/// The input edge of the backpressure case: `supply` envelopes, every one ready at once, counting
/// how many the node took.
struct Flood {
    supply: VecDeque<EventEnvelope<Payload>>,
    taken: Arc<AtomicUsize>,
}

impl InputEndpoint<Payload> for Flood {
    fn recv(&mut self) -> EndpointFuture<'_, Option<EventEnvelope<Payload>>> {
        let next = self.supply.pop_front();
        if next.is_some() {
            self.taken.fetch_add(1, Ordering::SeqCst);
        }
        Box::pin(future::poll_fn(move |_| {
            next.clone()
                .map_or(Poll::Pending, |envelope| Poll::Ready(Some(envelope)))
        }))
    }
}

/// How many of `sent` envelopes an executor node running its first call takes from its input
/// edge, built by `node`.
fn taken_while_running(
    sent: usize,
    node: impl FnOnce(Arc<Endless>, &Tool) -> ToolExecutorNode<Endless>,
) -> usize {
    let lookup = tool(false);
    let (declaration, node_id) = declared(NodeRole::Tool);
    let executor = Arc::new(Endless::default());
    let mut node = node(Arc::clone(&executor), &lookup);
    let taken = Arc::new(AtomicUsize::new(0));
    let supply: VecDeque<_> = (0..sent).map(|_| delivery(&authorized(&lookup))).collect();
    let flood = Flood {
        supply,
        taken: Arc::clone(&taken),
    };
    block_on(NodeImplementation::<Payload>::describe(
        &mut node,
        declaration.node(node_id).expect("declared"),
    ));
    block_on(NodeImplementation::<Payload>::prepare(&mut node)).expect("prepares");
    block_on(NodeImplementation::<Payload>::start(
        &mut node,
        endpoints(flood, Taken::default()),
    ))
    .expect("starts");
    assert!(
        eventually(Duration::from_secs(5), || executor
            .invoked
            .load(Ordering::SeqCst)
            == 1),
        "the first call runs"
    );
    thread::sleep(Duration::from_millis(200));
    let taken = taken.load(Ordering::SeqCst);
    block_on(NodeImplementation::<Payload>::cancel(&mut node));
    let stopped = ends_within(Duration::from_secs(5), move || {
        block_on(NodeImplementation::<Payload>::stop(&mut node));
    });
    assert!(stopped.is_some(), "the cancelled node stops");
    taken
}

/// N16: while a call runs, the node reads ahead at most its read-ahead bound, so an upstream that
/// sends more than the edge holds waits for room instead of filling the node's memory.
#[test]
fn an_executor_running_a_call_takes_no_more_than_its_read_ahead_bound_from_its_input() {
    let sent = 1_000;
    let taken = taken_while_running(sent, |executor, lookup| {
        ToolExecutorNode::new(
            RunId::generate(),
            executor,
            [lookup.tool_id()],
            Duration::from_secs(60),
        )
    });
    assert!(
        taken <= 1 + 64,
        "the running call and at most the default read-ahead are taken, not {taken} of {sent}"
    );
    assert_eq!(
        ToolExecutorNode::<Endless>::DEFAULT_READ_AHEAD.get(),
        64,
        "the default bound the case above holds the node to"
    );

    let bound = NonZeroUsize::new(4).expect("not zero");
    let taken = taken_while_running(sent, |executor, lookup| {
        ToolExecutorNode::new(
            RunId::generate(),
            executor,
            [lookup.tool_id()],
            Duration::from_secs(60),
        )
        .with_read_ahead(bound)
    });
    assert_eq!(
        taken,
        1 + bound.get(),
        "the running call and exactly the read-ahead the node was given"
    );
}

/// The input edge of the cancel race: it tells the test when the node asks for its first
/// envelope, and hands the call over only once the test opens its gate.
struct Gated {
    call: Option<EventEnvelope<Payload>>,
    entered: Sender<()>,
    gate: Option<Receiver<()>>,
}

impl InputEndpoint<Payload> for Gated {
    fn recv(&mut self) -> EndpointFuture<'_, Option<EventEnvelope<Payload>>> {
        let call = self.call.take();
        let gate = self.gate.take();
        let entered = self.entered.clone();
        Box::pin(future::poll_fn(move |_| match &call {
            Some(call) => {
                let _ = entered.send(());
                if let Some(gate) = &gate {
                    let _ = gate.recv_timeout(Duration::from_secs(10));
                }
                Poll::Ready(Some(call.clone()))
            }
            None => Poll::Pending,
        }))
    }
}

/// N17: the call is delivered after the cancel was taken. The node neither moves it to Started in
/// the authority's record nor invokes its executor, and sends nothing.
#[test]
fn a_cancel_racing_a_delivered_call_starts_nothing_and_executes_nothing() {
    let lookup = tool(false);
    let call = authorized(&lookup);
    let (declaration, node_id) = declared(NodeRole::Tool);
    let authority = ToolAuthority::new(RunId::generate(), [lookup.clone()]).handle();
    let executor = Arc::new(Endless::default());
    let mut node = ToolExecutorNode::new(
        RunId::generate(),
        Arc::clone(&executor),
        [lookup.tool_id()],
        Duration::from_secs(60),
    )
    .answering_to(&authority);
    let (entered, entered_rx) = mpsc::channel();
    let (gate_tx, gate) = mpsc::channel();
    let input = Gated {
        call: Some(delivery(&call)),
        entered,
        gate: Some(gate),
    };
    let taken = Taken::default();
    block_on(NodeImplementation::<Payload>::describe(
        &mut node,
        declaration.node(node_id).expect("declared"),
    ));
    block_on(NodeImplementation::<Payload>::prepare(&mut node)).expect("prepares");
    block_on(NodeImplementation::<Payload>::start(
        &mut node,
        endpoints(input, taken.clone()),
    ))
    .expect("starts");
    entered_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the node asks for its first envelope");
    block_on(NodeImplementation::<Payload>::cancel(&mut node));
    gate_tx.send(()).expect("the input waits at its gate");
    let stopped = ends_within(Duration::from_secs(5), move || {
        block_on(NodeImplementation::<Payload>::stop(&mut node));
    });
    assert!(stopped.is_some(), "the cancelled node stops");

    assert_eq!(
        executor.invoked.load(Ordering::SeqCst),
        0,
        "the executor is not invoked after the cancel"
    );
    let started: Vec<_> = authority
        .calls()
        .into_iter()
        .filter(|call| call.state() == ToolCallState::Started)
        .collect();
    assert!(
        started.is_empty(),
        "no call is Started without its ToolCallStarted: {started:?}"
    );
    assert!(
        lock(&taken.0).is_empty(),
        "nothing is sent after the cancel"
    );
}
