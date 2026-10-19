//! The tool executor node: a node of role `tool` with one input port for authorized calls and one
//! output port for their results and failures, generic over the executor contract of
//! `inference-tools` ([`ToolExecutor`]).
//!
//! For each authorized call its input edge delivers, it runs `inference.tools.StartToolCall` and
//! sends the started call on, executes it on its executor under the node's deadline (852), then
//! runs `inference.tools.CompleteToolCall` with the result or `inference.tools.FailToolCall`
//! with the typed failure and sends that on. One node serves every tool it was constructed with;
//! a call for another tool fails with an `execution_error` naming it, and the executor is not
//! invoked. A call still running at the deadline fails with `deadline_exceeded`, whatever the
//! executor does with the deadline it is given; a deadline later than the clock can represent
//! is no deadline.
//!
//! A call that failed transiently is attempted again by the node's [`RetryPolicy`], by the call's
//! side-effect class: every attempt presents the same call, under the one deadline, and a
//! non-idempotent write is attempted once (`docs/design/inference.md` 1007). Attempts are not
//! lifecycle transitions: the call is Started from its first attempt to its Completed or Failed.
//! The node starts each call once: an authorized call delivered again is started as the node
//! holds it, which the state machine refuses (`StartToolCall/outcome/wrong-state`); the refusal is
//! recorded on the [`ExecutorHandle`], nothing is executed or sent, and the node goes on. The
//! handle also records each ended call's [`ToolCallMeasure`], its latency, attempts and failure
//! (1025).
//!
//! On the end of input, a control event of kind `stop`, it passes the end of input on and ends;
//! once its input edge is closed it ends. Cancelled, it cancels the call it is executing, abandons
//! it and sends nothing more, the answer its executor gives after the cancel among it. An
//! executor that panics ends the node's exchange, and the node's drain fails naming the panic.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::future::{self, Future};
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::thread;
use std::time::{Duration, Instant};

use inference_core::endpoint::{InputEndpoint, OutputEndpoint};
use inference_core::envelope::{ControlKind, EventEnvelope};
use inference_core::id::{NodeId, RunId};
use inference_tools::{
    CancellationSignal, RetryPolicy, SideEffectClass, ToolCall, ToolCallId, ToolCallState,
    ToolCallStateConflict, ToolExecutor, ToolFailure, ToolFailureKind, ToolId,
};

use super::{
    Emitter, Next, Task, ToolCallEvent, ToolCallUpdate, ToolPayload, Watched, block_on, next,
    one_in_one_out, unpark_waker,
};
use crate::{Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation};

/// `inference.tools.StartToolCall/outcome/wrong-state`, as the node answered an authorized call
/// delivered again: `call` as the node holds it, and the conflict naming the state it holds.
/// Nothing moved and nothing was executed or sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartRefusal {
    pub call: ToolCall,
    pub refused: ToolCallStateConflict,
}

/// The tool call latency and failure measure (`docs/design/inference.md` 1025) of one call the
/// node started and ended: how many attempts its executor made, from the start to the end how
/// long it took, and the kind of the failure it ended with, if it failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallMeasure {
    pub call_id: ToolCallId,
    pub tool_id: ToolId,
    pub side_effect: SideEffectClass,
    pub attempts: u32,
    pub latency: Duration,
    pub failure: Option<ToolFailureKind>,
}

/// What the node was asked to do and what its exchange came to.
#[derive(Debug, Default)]
struct ExecutorState {
    cancelled: bool,
    failure: Option<String>,
    refusals: Vec<StartRefusal>,
    measures: Vec<ToolCallMeasure>,
}

/// The host's side of a [`ToolExecutorNode`]: the record of its refusals and of its measures.
/// Clones share one node.
#[derive(Debug, Clone)]
pub struct ExecutorHandle {
    state: Arc<Watched<ExecutorState>>,
}

impl ExecutorHandle {
    /// Every `StartToolCall` the node refused, in the order it refused them.
    #[must_use]
    pub fn refusals(&self) -> Vec<StartRefusal> {
        self.state.read(|state| state.refusals.clone())
    }

    /// The measure of every call the node ended, in the order it ended them.
    #[must_use]
    pub fn measures(&self) -> Vec<ToolCallMeasure> {
        self.state.read(|state| state.measures.clone())
    }
}

impl fmt::Debug for Watched<ExecutorState> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.read(|state| state.fmt(f))
    }
}

/// How one execution ended.
enum Within<T> {
    Done(T),
    Overran,
    Cancelled,
}

/// Polls `future` on this thread until it completes, `deadline` passes or `cancellation` is
/// cancelled, whichever comes first; with no deadline, until it completes or is cancelled.
fn within<F: Future>(
    future: F,
    deadline: Option<Instant>,
    cancellation: &CancellationSignal,
) -> Within<F::Output> {
    let waker = unpark_waker();
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
            return Within::Done(output);
        }
        if cancellation.is_cancelled() {
            return Within::Cancelled;
        }
        match deadline {
            Some(deadline) => {
                let now = Instant::now();
                if now >= deadline {
                    return Within::Overran;
                }
                thread::park_timeout(deadline - now);
            }
            None => thread::park(),
        }
    }
}

/// The latest instant at most `after` past `now` the clock can represent: `now + after`, or a
/// shorter span when that overflows.
fn latest(now: Instant, after: Duration) -> Instant {
    let mut after = after;
    loop {
        if let Some(instant) = now.checked_add(after) {
            return instant;
        }
        after /= 2;
    }
}

/// The tool executor node; see the module documentation.
pub struct ToolExecutorNode<E: ?Sized> {
    run_id: RunId,
    executor: Arc<E>,
    tools: Arc<HashSet<ToolId>>,
    deadline: Duration,
    retries: RetryPolicy,
    node_id: Option<NodeId>,
    state: Arc<Watched<ExecutorState>>,
    cancellation: CancellationSignal,
    task: Task,
}

impl<E: ?Sized> fmt::Debug for ToolExecutorNode<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolExecutorNode")
            .field("run_id", &self.run_id)
            .field("tools", &self.tools)
            .field("deadline", &self.deadline)
            .field("retries", &self.retries)
            .field("node_id", &self.node_id)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl<E: ToolExecutor + ?Sized + 'static> ToolExecutorNode<E> {
    /// An executor node of the run `run_id` executing the calls of `tools` on `executor`, each
    /// under `deadline` from its start.
    #[must_use]
    pub fn new(
        run_id: RunId,
        executor: Arc<E>,
        tools: impl IntoIterator<Item = ToolId>,
        deadline: Duration,
    ) -> Self {
        Self {
            run_id,
            executor,
            tools: Arc::new(tools.into_iter().collect()),
            deadline,
            retries: RetryPolicy::default(),
            node_id: None,
            state: Arc::new(Watched::new(ExecutorState::default())),
            cancellation: CancellationSignal::new(),
            task: Task::new(),
        }
    }

    /// The node with `policy`, which decides how often a call is attempted by its side-effect
    /// class; without one, every call is attempted once.
    #[must_use]
    pub const fn with_retries(mut self, policy: RetryPolicy) -> Self {
        self.retries = policy;
        self
    }

    /// The host's side of the node: the record of its refusals and measures.
    #[must_use]
    pub fn handle(&self) -> ExecutorHandle {
        ExecutorHandle {
            state: Arc::clone(&self.state),
        }
    }

    fn failure(&self) -> Result<(), NodeError> {
        self.state
            .read(|state| state.failure.clone())
            .map_or(Ok(()), |failure| Err(NodeError::new(failure)))
    }
}

/// One started exchange of a [`ToolExecutorNode`].
struct Exchanging<P, E: ?Sized> {
    executor: Arc<E>,
    tools: Arc<HashSet<ToolId>>,
    deadline: Duration,
    state: Arc<Watched<ExecutorState>>,
    cancellation: CancellationSignal,
    retries: RetryPolicy,
    /// Every call the node started, as it last left it.
    known: HashMap<ToolCallId, ToolCall>,
    input: Box<dyn InputEndpoint<P>>,
    output: Box<dyn OutputEndpoint<P>>,
    emitter: Emitter,
}

impl<P: ToolPayload, E: ToolExecutor + ?Sized> Exchanging<P, E> {
    /// Sends `envelope` on, unless the node was cancelled: it then sends nothing more.
    fn send(&self, envelope: EventEnvelope<P>) -> Result<(), String> {
        if self.cancellation.is_cancelled() {
            return Ok(());
        }
        block_on(self.output.send(envelope)).map_err(|refused| refused.to_string())
    }

    /// The authorized call the content of an envelope carries, or a failure naming what arrived
    /// instead.
    fn authorized(payload: P, kind: &str) -> Result<ToolCall, String> {
        match payload.into_tool_call() {
            Ok(update) if update.call.state() == ToolCallState::Authorized => Ok(update.call),
            Ok(update) => Err(format!(
                "the tool executor takes authorized calls; call {} arrived {:?}",
                update.call.call_id(),
                update.call.state()
            )),
            Err(_) => Err(format!(
                "the tool executor takes authorized calls; an event of kind {kind} arrived"
            )),
        }
    }

    /// The result or failure of executing the started `call`, with the number of attempts its
    /// executor made, or nothing once the node was cancelled. Every attempt runs under the one
    /// deadline the call started with.
    fn execute(
        &self,
        call: &ToolCall,
    ) -> Option<(Result<inference_tools::Json, ToolFailure>, u32)> {
        if !self.tools.contains(&call.tool_id()) {
            let failure = ToolFailure {
                kind: ToolFailureKind::ExecutionError,
                detail: format!("this executor node serves no tool {}", call.tool_id()),
            };
            return Some((Err(failure), 0));
        }
        let now = Instant::now();
        let deadline = now.checked_add(self.deadline);
        let given = deadline.unwrap_or_else(|| latest(now, self.deadline));
        let limit = self.retries.attempts(call.side_effect());
        let mut attempts = 0;
        loop {
            attempts += 1;
            let execution = self.executor.execute(call, given, &self.cancellation);
            let outcome = match within(execution, deadline, &self.cancellation) {
                Within::Done(outcome) => outcome,
                Within::Overran => Err(ToolFailure {
                    kind: ToolFailureKind::DeadlineExceeded,
                    detail: format!("the call did not finish within {:?}", self.deadline),
                }),
                Within::Cancelled => return None,
            };
            let again = match &outcome {
                Err(failure) => {
                    attempts < limit
                        && self.retries.retries(failure)
                        && !self.cancellation.is_cancelled()
                        && deadline.is_none_or(|deadline| Instant::now() < deadline)
                }
                Ok(_) => false,
            };
            if !again {
                return Some((outcome, attempts));
            }
        }
    }

    /// Starts, executes and completes or fails the call the envelope `envelope` authorized. A
    /// call the node holds already is started as the node holds it, so the state machine refuses
    /// it; the refusal is recorded and nothing is executed or sent.
    fn call(&mut self, envelope: EventEnvelope<P>) -> Result<(), String> {
        let delivered = Self::authorized(envelope.payload, &envelope.kind)?;
        let call_id = delivered.call_id();
        let call = self.known.get(&call_id).cloned().unwrap_or(delivered);
        let (started, event) = match call.start() {
            Ok(moved) => moved,
            Err(refused) => {
                let refusal = StartRefusal { call, refused };
                self.state.update(|state| state.refusals.push(refusal));
                return Ok(());
            }
        };
        self.known.insert(call_id, started.clone());
        let begun = Instant::now();
        let update = ToolCallUpdate {
            call: started.clone(),
            event: ToolCallEvent::Started(event),
        };
        let started_envelope = self.emitter.update(update, envelope.event_id);
        let cause = started_envelope.event_id;
        self.send(started_envelope)?;
        let Some((outcome, attempts)) = self.execute(&started) else {
            return Ok(());
        };
        let measure = ToolCallMeasure {
            call_id,
            tool_id: started.tool_id(),
            side_effect: started.side_effect(),
            attempts,
            latency: begun.elapsed(),
            failure: outcome.as_ref().err().map(|failure| failure.kind),
        };
        self.state.update(|state| state.measures.push(measure));
        let update = match outcome {
            Ok(result) => {
                let (call, event) = started
                    .complete(result)
                    .map_err(|refused| refused.to_string())?;
                ToolCallUpdate {
                    call,
                    event: ToolCallEvent::Result(event),
                }
            }
            Err(failure) => {
                let (call, event) = started
                    .fail(failure)
                    .map_err(|refused| refused.to_string())?;
                ToolCallUpdate {
                    call,
                    event: ToolCallEvent::Failed(event),
                }
            }
        };
        self.known.insert(call_id, update.call.clone());
        let envelope = self.emitter.update(update, cause);
        self.send(envelope)
    }

    fn exchange(&mut self) -> Result<(), String> {
        loop {
            let state = Arc::clone(&self.state);
            let next = block_on(next(self.input.as_mut(), &state, |state| {
                state.cancelled.then_some(())
            }));
            match next {
                Next::Signal(()) | Next::Closed => return Ok(()),
                Next::Envelope(envelope) => match ControlKind::of(&envelope.kind) {
                    Some(ControlKind::Stop) => {
                        let end = self.emitter.control(ControlKind::Stop);
                        return self.send(end);
                    }
                    Some(_) => {}
                    None => self.call(envelope)?,
                },
            }
            if self.cancellation.is_cancelled() {
                return Ok(());
            }
        }
    }

    fn run(mut self) {
        let ended = self.exchange();
        self.state.update(|state| state.failure = ended.err());
    }
}

impl<P, E> NodeImplementation<P> for ToolExecutorNode<E>
where
    P: ToolPayload,
    E: ToolExecutor + ?Sized + 'static,
{
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.node_id = Some(node.node_id());
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn start(&mut self, endpoints: NodeEndpoints<P>) -> NodeFuture<'_, Result<(), NodeError>> {
        let started = one_in_one_out(self.node_id, endpoints).map(|(node_id, input, output)| {
            let exchanging = Exchanging {
                executor: Arc::clone(&self.executor),
                tools: Arc::clone(&self.tools),
                deadline: self.deadline,
                state: Arc::clone(&self.state),
                cancellation: self.cancellation.clone(),
                retries: self.retries,
                known: HashMap::new(),
                input: input.endpoint,
                emitter: Emitter::new(self.run_id, node_id, output.port_id.to_string()),
                output: output.endpoint,
            };
            self.task.spawn(move || exchanging.run());
        });
        Box::pin(future::ready(started))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(async move {
            self.task.finished().await.map_err(NodeError::new)?;
            self.failure()
        })
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        self.cancellation.cancel();
        self.state.update(|state| state.cancelled = true);
        self.task.unpark();
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(async move {
            let _already_reported_by_drain = self.task.finished().await;
        })
    }
}

#[cfg(test)]
mod tests {
    use std::future;
    use std::time::{Duration, Instant};

    use inference_tools::CancellationSignal;

    use super::{Within, latest, within};

    #[test]
    fn a_deadline_past_what_the_clock_represents_is_no_deadline_and_does_not_panic() {
        let now = Instant::now();
        assert!(now.checked_add(Duration::MAX).is_none());
        assert!(latest(now, Duration::MAX) > now);
        assert_eq!(
            latest(now, Duration::from_secs(1)),
            now + Duration::from_secs(1)
        );
        let answered = within(future::ready(7), None, &CancellationSignal::new());
        assert!(matches!(answered, Within::Done(7)));
        let overran = within(
            future::pending::<()>(),
            Some(now),
            &CancellationSignal::new(),
        );
        assert!(matches!(overran, Within::Overran));
    }
}
