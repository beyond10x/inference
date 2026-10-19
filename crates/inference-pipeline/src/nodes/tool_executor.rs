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
//! is no deadline. The node starts each call once: an authorized call it already started fails
//! the node (`StartToolCall/outcome/wrong-state`) and is not executed again.
//!
//! On the end of input, a control event of kind `stop`, it passes the end of input on and ends;
//! once its input edge is closed it ends. Cancelled, it cancels the call it is executing, abandons
//! it and sends nothing more, the answer its executor gives after the cancel among it. An
//! executor that panics ends the node's exchange, and the node's drain fails naming the panic.

use std::collections::HashSet;
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
    CancellationSignal, ToolCall, ToolCallId, ToolCallState, ToolExecutor, ToolFailure,
    ToolFailureKind, ToolId,
};

use super::{
    Emitter, Next, Task, ToolCallEvent, ToolCallUpdate, ToolPayload, Watched, block_on, next,
    one_in_one_out, unpark_waker,
};
use crate::{Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation};

/// What the node was asked to do and what its exchange came to.
#[derive(Debug, Default)]
struct ExecutorState {
    cancelled: bool,
    failure: Option<String>,
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
            node_id: None,
            state: Arc::new(Watched::new(ExecutorState::default())),
            cancellation: CancellationSignal::new(),
            task: Task::new(),
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
    /// Every call the node started.
    started: HashSet<ToolCallId>,
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

    /// The result or failure of executing the started `call`, or nothing once the node was
    /// cancelled.
    fn execute(&self, call: &ToolCall) -> Option<Result<inference_tools::Json, ToolFailure>> {
        if !self.tools.contains(&call.tool_id()) {
            return Some(Err(ToolFailure {
                kind: ToolFailureKind::ExecutionError,
                detail: format!("this executor node serves no tool {}", call.tool_id()),
            }));
        }
        let now = Instant::now();
        let deadline = now.checked_add(self.deadline);
        let given = deadline.unwrap_or_else(|| latest(now, self.deadline));
        let execution = self.executor.execute(call, given, &self.cancellation);
        match within(execution, deadline, &self.cancellation) {
            Within::Done(outcome) => Some(outcome),
            Within::Overran => Some(Err(ToolFailure {
                kind: ToolFailureKind::DeadlineExceeded,
                detail: format!("the call did not finish within {:?}", self.deadline),
            })),
            Within::Cancelled => None,
        }
    }

    /// Starts, executes and completes or fails the call the envelope `envelope` authorized.
    fn call(&mut self, envelope: EventEnvelope<P>) -> Result<(), String> {
        let call = Self::authorized(envelope.payload, &envelope.kind)?;
        if !self.started.insert(call.call_id()) {
            return Err(format!(
                "the tool executor started call {} already; it is not executed again",
                call.call_id()
            ));
        }
        let (started, event) = call.start().map_err(|refused| refused.to_string())?;
        let update = ToolCallUpdate {
            call: started.clone(),
            event: ToolCallEvent::Started(event),
        };
        let started_envelope = self.emitter.update(update, envelope.event_id);
        let cause = started_envelope.event_id;
        self.send(started_envelope)?;
        let Some(outcome) = self.execute(&started) else {
            return Ok(());
        };
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
                started: HashSet::new(),
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
