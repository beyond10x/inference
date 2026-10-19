//! The node implementations that carry no model: the tool authority ([`ToolAuthority`], role
//! `policy`) and the tool executor ([`ToolExecutorNode`], role `tool`) of
//! `docs/design/inference.md` §15.1, between which a proposed tool call is validated, authorized,
//! executed and answered with a result or a typed failure; and the endpoint controller, the voice
//! activity detector ([`vad::VoiceActivityDetector`]) and the turn policy
//! ([`turn_policy::TurnPolicy`]), both policy nodes of §9.1 (466), between which audio frames
//! become speech activity and speech activity becomes the turns of `inference.stream` (§11.1,
//! 606), with the turn policy as their single authority (§12.5, 699).
//!
//! A tool call travels along the edges as a [`ToolCallUpdate`], a Structured payload: the call as
//! the last command left it and the event that command emitted. A run carries one payload type on
//! every edge, so the nodes read and write it through [`ToolPayload`], which that payload
//! implements; a control event is told from user content by its envelope's `kind` alone. Every
//! envelope a node emits names the run, the node, its output port, a stream of the node's own
//! numbered from 1, the generation of the call and, as its cause, the envelope it answers.
//!
//! A started node exchanges on a thread of its own and polls its endpoints with an executor of its
//! own, so the runtime that drives it commits to none.

use std::fmt;
use std::future::{self, Future};
use std::pin::pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, JoinHandle, Thread};
use std::time::{SystemTime, UNIX_EPOCH};

use inference_core::endpoint::InputEndpoint;
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{NodeId, RunId, StreamId};
use inference_tools::{
    ToolCall, ToolCallAuthorized, ToolCallFailed, ToolCallProposed, ToolCallRejected,
    ToolCallResult, ToolCallStarted,
};

use crate::{NodeEndpoints, NodeError, NodeInput, NodeOutput};

mod tool_authority;
mod tool_executor;
/// The turn policy: speech activity in, the turn commands of `inference.stream` out.
pub mod turn_policy;
/// The voice activity detector: audio frames in, speech activity out, by an energy threshold.
pub mod vad;

pub use tool_authority::{AuthorityHandle, AuthorityOutcome, Decision, ToolAuthority};
pub use tool_executor::ToolExecutorNode;

/// An event of `inference.tools.ToolCall`'s lifecycle, as its command emitted it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCallEvent {
    /// `inference.tools.ToolCallProposed`.
    Proposed(ToolCallProposed),
    /// `inference.tools.ToolCallAuthorized`.
    Authorized(ToolCallAuthorized),
    /// `inference.tools.ToolCallRejected`.
    Rejected(ToolCallRejected),
    /// `inference.tools.ToolCallStarted`.
    Started(ToolCallStarted),
    /// `inference.tools.ToolCallResult`.
    Result(ToolCallResult),
    /// `inference.tools.ToolCallFailed`.
    Failed(ToolCallFailed),
}

impl ToolCallEvent {
    /// The event's declared name, the `kind` of the envelope that carries it, such as
    /// `inference.tools.ToolCallResult`.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Proposed(_) => "inference.tools.ToolCallProposed",
            Self::Authorized(_) => "inference.tools.ToolCallAuthorized",
            Self::Rejected(_) => "inference.tools.ToolCallRejected",
            Self::Started(_) => "inference.tools.ToolCallStarted",
            Self::Result(_) => "inference.tools.ToolCallResult",
            Self::Failed(_) => "inference.tools.ToolCallFailed",
        }
    }
}

/// One step of a tool call along the tool path: `call` as the command left it, and the `event`
/// the command emitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallUpdate {
    pub call: ToolCall,
    pub event: ToolCallEvent,
}

/// The payload of a run whose edges carry tool calls: a tool node reads a [`ToolCallUpdate`] out
/// of it and builds one, or a control event, into it.
pub trait ToolPayload: Send + Sized + 'static {
    /// The step of a tool call the payload carries; the payload itself when it carries none.
    ///
    /// # Errors
    ///
    /// The payload, unchanged, when it carries no tool call.
    fn into_tool_call(self) -> Result<ToolCallUpdate, Self>;

    /// The payload carrying `update`.
    fn from_tool_call(update: ToolCallUpdate) -> Self;

    /// The payload of the control event `kind`; its envelope's `kind` is [`ControlKind::as_str`].
    fn control(kind: ControlKind) -> Self;
}

/// A state shared between a node, its exchange thread and its handle: a change wakes the task
/// waiting on it.
pub(crate) struct Watched<S> {
    state: Mutex<(S, Option<Waker>)>,
}

impl<S> Watched<S> {
    pub(crate) const fn new(state: S) -> Self {
        Self {
            state: Mutex::new((state, None)),
        }
    }

    fn lock(&self) -> MutexGuard<'_, (S, Option<Waker>)> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Changes the state by `change` and wakes the task waiting on it.
    pub(crate) fn update<T>(&self, change: impl FnOnce(&mut S) -> T) -> T {
        let mut guard = self.lock();
        let answer = change(&mut guard.0);
        let waker = guard.1.take();
        drop(guard);
        if let Some(waker) = waker {
            waker.wake();
        }
        answer
    }

    /// Reads the state.
    pub(crate) fn read<T>(&self, read: impl FnOnce(&S) -> T) -> T {
        read(&self.lock().0)
    }

    /// Waits, as a task, until `ready` answers.
    pub(crate) async fn until<T>(&self, mut ready: impl FnMut(&mut S) -> Option<T>) -> T {
        future::poll_fn(|cx| {
            let mut guard = self.lock();
            match ready(&mut guard.0) {
                Some(answer) => Poll::Ready(answer),
                None => {
                    guard.1 = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        })
        .await
    }
}

/// What a node's exchange acts on next.
pub(crate) enum Next<P, T> {
    /// The input edge delivered an envelope.
    Envelope(EventEnvelope<P>),
    /// The input edge is closed: empty, with every sender gone.
    Closed,
    /// The shared state answered first.
    Signal(T),
}

/// The next envelope of `input`, unless `signal` answers of `watched` first.
pub(crate) async fn next<P, S, T>(
    input: &mut dyn InputEndpoint<P>,
    watched: &Watched<S>,
    mut signal: impl FnMut(&mut S) -> Option<T>,
) -> Next<P, T> {
    let mut recv = input.recv();
    future::poll_fn(|cx| {
        {
            let mut guard = watched.lock();
            if let Some(answer) = signal(&mut guard.0) {
                return Poll::Ready(Next::Signal(answer));
            }
            guard.1 = Some(cx.waker().clone());
        }
        recv.as_mut()
            .poll(cx)
            .map(|received| received.map_or(Next::Closed, Next::Envelope))
    })
    .await
}

pub(crate) struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// A waker that unparks this thread.
pub(crate) fn unpark_waker() -> Waker {
    Waker::from(Arc::new(Unpark(thread::current())))
}

/// Polls `future` on this thread until it completes.
pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
    let waker = unpark_waker();
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
            return output;
        }
        thread::park();
    }
}

/// Marks the exchange ended when the thread leaves its work, returning or unwinding.
struct Ended(Arc<Watched<bool>>);

impl Drop for Ended {
    fn drop(&mut self) {
        self.0.update(|ended| *ended = true);
    }
}

/// The exchange task of one node: the thread it runs on once started, and how it ended if it
/// panicked.
pub(crate) struct Task {
    ended: Arc<Watched<bool>>,
    thread: Option<JoinHandle<()>>,
    panicked: Option<String>,
}

impl fmt::Debug for Task {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Task")
            .field("panicked", &self.panicked)
            .finish_non_exhaustive()
    }
}

impl Task {
    pub(crate) fn new() -> Self {
        Self {
            ended: Arc::new(Watched::new(false)),
            thread: None,
            panicked: None,
        }
    }

    /// Runs `work` on a thread of its own.
    pub(crate) fn spawn(&mut self, work: impl FnOnce() + Send + 'static) {
        let ended = Ended(Arc::clone(&self.ended));
        self.thread = Some(thread::spawn(move || {
            let _ended = ended;
            work();
        }));
    }

    /// Wakes the thread if it is parked.
    pub(crate) fn unpark(&self) {
        if let Some(thread) = &self.thread {
            thread.thread().unpark();
        }
    }

    /// Waits until the task ended, then joins its thread; a task never spawned has ended.
    ///
    /// # Errors
    ///
    /// The panic of the thread, described, when it panicked; every later call answers it again.
    pub(crate) async fn finished(&mut self) -> Result<(), String> {
        if let Some(thread) = self.thread.take() {
            self.ended.until(|ended| ended.then_some(())).await;
            if let Err(panic) = thread.join() {
                let message = panic
                    .downcast_ref::<&str>()
                    .map(|message| (*message).to_owned())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "a panic without a message".to_owned());
                self.panicked = Some(format!("the node's exchange panicked: {message}"));
            }
        }
        self.panicked.clone().map_or(Ok(()), Err)
    }
}

/// Wraps what one node emits on its output port in envelopes: one stream of its own, numbered
/// from 1.
pub(crate) struct Emitter {
    run_id: RunId,
    producer: NodeId,
    port: String,
    stream_id: StreamId,
    sequence: i64,
}

impl Emitter {
    pub(crate) fn new(run_id: RunId, producer: NodeId, port: String) -> Self {
        Self {
            run_id,
            producer,
            port,
            stream_id: StreamId::generate(),
            sequence: 0,
        }
    }

    fn envelope<P>(
        &mut self,
        kind: &str,
        update: Option<(&ToolCall, EventId)>,
        payload: P,
    ) -> EventEnvelope<P> {
        self.sequence += 1;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| {
                i64::try_from(since.as_micros()).unwrap_or(i64::MAX)
            });
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: self.run_id,
            session_id: None,
            stream_id: self.stream_id,
            sequence: self.sequence,
            producer: self.producer,
            port: self.port.clone(),
            kind: kind.to_owned(),
            generation_id: update.map(|(call, _)| call.generation_id()),
            timestamp,
            timebase: Timebase::WallTime,
            causation_id: update.map(|(_, cause)| cause),
            payload,
        }
    }

    /// The envelope of `update`, naming the call's generation and `cause`, the envelope it
    /// answers.
    pub(crate) fn update<P: ToolPayload>(
        &mut self,
        update: ToolCallUpdate,
        cause: EventId,
    ) -> EventEnvelope<P> {
        let kind = update.event.name();
        let call = update.call.clone();
        self.envelope(kind, Some((&call, cause)), P::from_tool_call(update))
    }

    /// The envelope of the control event `kind`, on the same stream.
    pub(crate) fn control<P: ToolPayload>(&mut self, kind: ControlKind) -> EventEnvelope<P> {
        self.envelope(kind.as_str(), None, P::control(kind))
    }
}

/// The node's identity and its one input and one output edge, once it was described and handed
/// exactly those; otherwise the error saying what it was handed instead.
pub(crate) fn one_in_one_out<P>(
    node_id: Option<NodeId>,
    mut endpoints: NodeEndpoints<P>,
) -> Result<(NodeId, NodeInput<P>, NodeOutput<P>), NodeError> {
    if endpoints.inputs.len() != 1 || endpoints.outputs.len() != 1 {
        return Err(NodeError::new(format!(
            "the node takes 1 input and 1 output edge and was handed {} and {}",
            endpoints.inputs.len(),
            endpoints.outputs.len()
        )));
    }
    let node_id =
        node_id.ok_or_else(|| NodeError::new("the node was started before it was described"))?;
    let input = endpoints.inputs.remove(0);
    let output = endpoints.outputs.remove(0);
    Ok((node_id, input, output))
}
