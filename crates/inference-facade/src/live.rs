//! A live session: a session bound to the run of its pipeline, which the application reaches only
//! through the pipeline's exposures (`docs/design/inference.md` 107, 511, 711-719).
//!
//! [`LiveSessions::open`] runs `OpenSession` over a planned pipeline, creates a run of it under
//! the [`RunController`] of `inference-runtime`, attaches the run (`AttachRun`), validates,
//! prepares and activates it, and marks the session ready (`MarkSessionReady`) once the run is
//! Active, and not before. It answers the application's handle, [`LiveSession`], and the run's
//! loop, [`SessionDriver`], which the application polls to drive the run on an executor of its
//! choosing: no signature here names one (665).
//!
//! The application then reaches the run through the outer ends of the pipeline's exposures,
//! which the session holds:
//!
//! | operation | what the session does |
//! |---|---|
//! | [`LiveSession::send`] | `SendInput`, then forwards the application's envelope to the ingress its `port` names, stamped with the session and the current run (310) |
//! | [`LiveSession::commit_input`] | `CommitInput`, then sends a control event of kind `commit` into every ingress |
//! | [`LiveSession::receive`] | the next [`SessionEvent`]: the session's own events first, then what the run sends on its egresses, converted by [`SessionPayload`] |
//! | [`LiveSession::close`] | `CloseSession` (Draining), then a control event of kind `stop` into every ingress, which ends the run's input |
//!
//! `SendInput` and `CommitInput` refuse a session that is not Ready, so a draining session admits
//! no new input. Once the run has drained and completed and the application received what the
//! egresses still held, the session runs `CompleteSessionClose`: the drain completes what is in
//! flight before the session closes, so nothing the run produced arrives after the close and
//! what it delivers still joins the history. The session delivers `RunCompleted` and then
//! `SessionClosed` last; [`LiveSession::receive`] answers none once every egress has ended and
//! every event of the session was delivered. An application that lets go of its handle lets go
//! of what was in flight, and the session closes once the run has completed; let go before the
//! run's output ended, the handle leaves the egresses without a consumer, which the runtime
//! reports as a node whose exchange failed.
//!
//! The session records its history ([`crate::history`]) from what it accepts and delivers: the
//! committed input texts the application sent since its last commit as one user turn at
//! [`LiveSession::commit_input`], joined by a space; a committed transcript it delivers, a voice
//! turn's, as a user turn; a committed segment it delivers as an assistant segment; a tool result
//! it delivers as a tool result with its call. A delta or a hypothesis records nothing.
//!
//! The control events the session writes travel on a stream of its own, produced under an
//! identity of its own, so that they never interleave with the sequence of the application's
//! streams.

use std::collections::{HashMap, HashSet, VecDeque};
use std::error::Error;
use std::fmt;
use std::future;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::{SystemTime, UNIX_EPOCH};

use inference_core::endpoint::{InputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, SessionId, StreamId};
use inference_pipeline::{
    Declaration, InteractionMode, Node, NodeImplementation, PipelineId, PlannedPipeline,
};
use inference_runtime::run::{Run, RunCancelled, RunCompleted, RunFailed, RunState};
use inference_runtime::{
    CreateControllerError, ExposureWatch, Ingress, QueueWatch, RunController, RunError, RunRequests,
};
use inference_tools::ToolCallId;

use crate::events::{SessionEvent, SessionPayload};
use crate::history::{HistoryEntry, HistoryKind, RecordHistory};
use crate::session::{
    AttachRun, AttachRunError, Graph, GraphPipeline, MarkSessionReadyError, OpenSession,
    OpenSessionError, Session, SessionMoveError, SessionNotFound, SessionState,
    SessionStateConflict, Sessions, SessionsRow,
};

/// The declaration a live session's pipeline was planned in, and the session's run once it
/// exists, read as the session's [`Graph`].
pub(crate) struct Planned<'a> {
    pub(crate) declaration: &'a Declaration,
    pub(crate) run: Option<&'a Run>,
}

impl Graph for Planned<'_> {
    fn pipeline(&self, pipeline_id: PipelineId) -> Option<GraphPipeline> {
        self.declaration
            .pipeline(pipeline_id)
            .map(|pipeline| GraphPipeline {
                state: pipeline.state(),
                mode: pipeline.mode(),
            })
    }

    fn run_pipeline(&self, run_id: RunId) -> Option<PipelineId> {
        self.run
            .filter(|run| run.run_id() == run_id)
            .map(Run::pipeline_id)
    }
}

/// Locks `mutex`, taking the data a panicking holder left: every critical section here leaves it
/// whole.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The events of one session not yet received, and how far its run and its close have come.
#[derive(Default)]
struct Inbox {
    /// The session's own events, delivered before what the egresses hold.
    events: VecDeque<SessionEvent>,
    /// How the run ended and what that closed, delivered once every egress has ended.
    ending: VecDeque<SessionEvent>,
    waker: Option<Waker>,
    /// The run ended: no egress sends anything more.
    ended: bool,
    /// The run ended Completed, so a draining session closes once what was in flight was
    /// delivered.
    completed: bool,
    /// The application let go of its handle: nothing more is delivered.
    released: bool,
    /// The application cancelled the session: once the run has ended, `CancelSession` closes it.
    cancelling: bool,
    /// `CompleteSessionClose` or `CancelSession` was run.
    finished: bool,
    /// The session's current run, whose end the session ends with; none while it switches.
    run: Option<RunId>,
    /// How the run the session left at a switch ended, once it has.
    left: Option<RunState>,
    /// How the session's current run ended, once it has.
    state: Option<RunState>,
}

/// The generations a live session delivered an event of, and those an interrupt cut: the session
/// delivers nothing more of a cut generation.
#[derive(Debug, Default)]
pub(crate) struct Generations {
    delivered: HashSet<GenerationId>,
    cut: HashSet<GenerationId>,
}

/// What a live session shares with its driver: the session state machine and the session's
/// inbox.
pub(crate) struct Shared {
    pub(crate) sessions: Arc<Mutex<Sessions>>,
    pub(crate) session_id: SessionId,
    inbox: Mutex<Inbox>,
}

impl Shared {
    /// The session's current run is `run_id`: its end is the one the session ends with.
    pub(crate) fn follow(&self, run_id: RunId) {
        let mut inbox = lock(&self.inbox);
        inbox.run = Some(run_id);
        inbox.ended = false;
        inbox.completed = false;
        inbox.state = None;
    }

    /// The session leaves its current run at a switch: the run's end, when it comes, is
    /// delivered among the session's own events and ends nothing of the session.
    pub(crate) fn leave(&self) {
        let mut inbox = lock(&self.inbox);
        inbox.run = None;
        inbox.left = None;
        if let Some(state) = inbox.state.take() {
            let ending: Vec<SessionEvent> = inbox.ending.drain(..).collect();
            inbox.events.extend(ending);
            inbox.left = Some(state);
        }
    }

    /// How the run the session left ended, once it has; pending, with `cx` woken, until then.
    pub(crate) fn poll_left(&self, cx: &Context<'_>) -> Poll<RunState> {
        let mut inbox = lock(&self.inbox);
        match inbox.left.take() {
            Some(state) => Poll::Ready(state),
            None => {
                inbox.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }

    /// The run `run_id` ended in `state`, `ending` its events: the end of the session's current
    /// run ([`Shared::end`]), or of a run it left, whose `ending` is delivered in order among the
    /// session's own events.
    fn run_ended(&self, run_id: RunId, state: RunState, ending: Vec<SessionEvent>) {
        {
            let mut inbox = lock(&self.inbox);
            if inbox.run != Some(run_id) {
                inbox.events.extend(ending);
                inbox.left = Some(state);
                if let Some(waker) = inbox.waker.take() {
                    waker.wake();
                }
                return;
            }
            inbox.state = Some(state);
        }
        self.end(ending, state == RunState::Completed);
    }

    /// Queues `event` for the application and wakes a waiting receive.
    pub(crate) fn record(&self, event: SessionEvent) {
        let mut inbox = lock(&self.inbox);
        inbox.events.push_back(event);
        if let Some(waker) = inbox.waker.take() {
            waker.wake();
        }
    }

    /// The run ended, `completed` or not: no egress sends anything more, and `ending` is
    /// delivered after what the egresses still hold. A session whose application let go of its
    /// handle has nothing left in flight, and one being cancelled delivers nothing more, so
    /// either finishes its close at once.
    fn end(&self, ending: impl IntoIterator<Item = SessionEvent>, completed: bool) {
        let now = {
            let mut inbox = lock(&self.inbox);
            inbox.ending.extend(ending);
            inbox.ended = true;
            inbox.completed = completed;
            if let Some(waker) = inbox.waker.take() {
                waker.wake();
            }
            inbox.released || inbox.cancelling
        };
        if now {
            self.finish();
        }
    }

    /// The application cancelled the session; a run that ended already finishes the close.
    fn cancel(&self) {
        let ended = {
            let mut inbox = lock(&self.inbox);
            inbox.cancelling = true;
            inbox.ended
        };
        if ended {
            self.finish();
        }
    }

    /// The application let go of its handle; a run that ended already finishes the close.
    fn release(&self) {
        let ended = {
            let mut inbox = lock(&self.inbox);
            inbox.released = true;
            inbox.ended
        };
        if ended {
            self.finish();
        }
    }

    /// Closes the session, once, after its run ended: `CancelSession` for a session being
    /// cancelled, whatever its run's end, otherwise `CompleteSessionClose` for a run that completed
    /// while the session drains, so that the drain completed what was in flight before the
    /// session closes. The `SessionClosed` is the last event delivered.
    fn finish(&self) {
        let cancelling = {
            let mut inbox = lock(&self.inbox);
            if inbox.finished || !(inbox.cancelling || inbox.completed) {
                return;
            }
            inbox.finished = true;
            inbox.cancelling
        };
        let closed = {
            let mut sessions = lock(&self.sessions);
            if cancelling {
                sessions.cancel_session(self.session_id)
            } else {
                sessions.complete_session_close(self.session_id)
            }
        };
        if let Ok(event) = closed {
            let mut inbox = lock(&self.inbox);
            inbox.ending.push_back(SessionEvent::SessionClosed(event));
            if let Some(waker) = inbox.waker.take() {
                waker.wake();
            }
        }
    }

    /// Once every egress has ended and what it held was delivered: the session's own next
    /// event, then the next event of the run's ending, closing the session first, or none once
    /// it was all received; pending, with the waker [`Shared::next`] registered, until the run
    /// has ended.
    fn after_egress(&self) -> Poll<Option<SessionEvent>> {
        {
            let mut inbox = lock(&self.inbox);
            if let Some(event) = inbox.events.pop_front() {
                return Poll::Ready(Some(event));
            }
            if !inbox.ended {
                return Poll::Pending;
            }
        }
        self.finish();
        Poll::Ready(lock(&self.inbox).ending.pop_front())
    }

    /// The next queued event; none, with `cx` woken on the next, when none is queued.
    fn next(&self, cx: &Context<'_>) -> Option<SessionEvent> {
        let mut inbox = lock(&self.inbox);
        let next = inbox.events.pop_front();
        if next.is_none() {
            inbox.waker = Some(cx.waker().clone());
        }
        next
    }

    fn state(&self) -> Option<SessionState> {
        lock(&self.sessions)
            .get(self.session_id)
            .map(Session::state)
    }
}

/// Why [`LiveSessions::open`] answered no live session.
#[derive(Debug)]
pub enum OpenError {
    /// `OpenSession` refused: no session was opened.
    Open(OpenSessionError),
    /// The run could not be created; the session was closed.
    Create(CreateControllerError),
    /// `AttachRun` refused; the session was closed.
    Attach(AttachRunError),
    /// The run did not validate, prepare or activate; the session was closed.
    Run(RunError),
    /// `MarkSessionReady` refused; the session was closed.
    Ready(MarkSessionReadyError),
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open(error) => write!(f, "the session was not opened: {error}"),
            Self::Create(error) => write!(f, "the session's run was not created: {error}"),
            Self::Attach(error) => write!(f, "the session's run was not attached: {error}"),
            Self::Run(error) => write!(f, "the session's run did not start: {error}"),
            Self::Ready(error) => write!(f, "the session did not become ready: {error}"),
        }
    }
}

impl Error for OpenError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Open(error) => Some(error),
            Self::Create(error) => Some(error),
            Self::Attach(error) => Some(error),
            Self::Run(error) => Some(error),
            Self::Ready(error) => Some(error),
        }
    }
}

/// Why [`LiveSession::send`] or [`LiveSession::commit_input`] forwarded nothing, or why the run
/// did not take what was forwarded.
#[derive(Debug)]
pub enum InputError<P> {
    /// `SendInput` or `CommitInput` refused: the session is not Ready. Nothing was forwarded.
    Refused(SessionMoveError),
    /// The pipeline exposes no ingress under the envelope's `port`. Nothing was forwarded.
    UnknownIngress(String),
    /// The ingress refused the envelope.
    Forward(Box<SendError<P>>),
}

impl<P> fmt::Display for InputError<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(error) => write!(f, "the session admits no input: {error}"),
            Self::UnknownIngress(name) => write!(f, "the pipeline exposes no ingress `{name}`"),
            Self::Forward(_) => f.write_str("the ingress refused the input"),
        }
    }
}

impl<P: fmt::Debug> Error for InputError<P> {}

/// Why [`LiveSession::close`] did not start the drain, or why an ingress did not take its end of
/// input.
#[derive(Debug)]
pub enum CloseError<P> {
    /// `CloseSession` refused: nothing moved.
    Refused(SessionMoveError),
    /// An ingress refused the end of input; the session is Draining all the same.
    Forward(Box<SendError<P>>),
}

impl<P> fmt::Display for CloseError<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(error) => write!(f, "the session does not close: {error}"),
            Self::Forward(_) => f.write_str("an ingress refused the end of input"),
        }
    }
}

impl<P: fmt::Debug> Error for CloseError<P> {}

/// The live sessions of an application, over one session state machine ([`Sessions`]).
#[derive(Debug, Clone, Default)]
pub struct LiveSessions {
    sessions: Arc<Mutex<Sessions>>,
}

impl LiveSessions {
    /// No session, giving random identities.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// No session, giving counted identities whose first eight hex digits are `space`
    /// ([`Sessions::counted_in`]).
    #[must_use]
    pub fn counted_in(space: u32) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(Sessions::counted_in(space))),
        }
    }

    /// The session carrying `session_id`, as it is now.
    #[must_use]
    pub fn session(&self, session_id: SessionId) -> Option<Session> {
        lock(&self.sessions).get(session_id).cloned()
    }

    /// The view `inference.session.Sessions`, read at the call.
    #[must_use]
    pub fn sessions_view(&self) -> Vec<SessionsRow> {
        lock(&self.sessions).sessions_view()
    }

    /// Every session event the state machine recorded, in order, across every session.
    #[must_use]
    pub fn events(&self) -> Vec<SessionEvent> {
        lock(&self.sessions).events().to_vec()
    }

    /// The history of the session `session_id`, in the order it was recorded, read at the call:
    /// it outlives the session's runs and lives as long as the session row does.
    #[must_use]
    pub fn history(&self, session_id: SessionId) -> Vec<HistoryEntry> {
        lock(&self.sessions).history(session_id).cloned().collect()
    }

    /// The view `inference.session.History`, read at the call.
    #[must_use]
    pub fn history_view(&self) -> Vec<HistoryEntry> {
        lock(&self.sessions).history_view().to_vec()
    }

    /// Opens a live session over the pipeline `planned`, planned in `declaration`, in `mode`: runs
    /// `OpenSession`, creates a run of the pipeline taking each node's implementation from
    /// `implement` (given the run's identity), attaches it, validates, prepares and activates it,
    /// takes the outer ends of its exposures and marks the session ready.
    ///
    /// # Errors
    ///
    /// [`OpenError::Open`] when `OpenSession` refuses; nothing was opened. Otherwise the step that
    /// failed; the session is then closed (`CloseSession`, `CompleteSessionClose`), and a run that
    /// was created is cancelled.
    pub async fn open<P: SessionPayload>(
        &self,
        declaration: &Declaration,
        planned: &PlannedPipeline,
        mode: InteractionMode,
        implement: impl FnMut(RunId, &Node) -> Box<dyn NodeImplementation<P>>,
    ) -> Result<(LiveSession<P>, SessionDriver<P>), OpenError> {
        let pipeline_id = planned.pipeline_id();
        let opened = lock(&self.sessions)
            .open_session(
                &Planned {
                    declaration,
                    run: None,
                },
                OpenSession { pipeline_id, mode },
            )
            .map_err(OpenError::Open)?;
        let shared = Arc::new(Shared {
            sessions: Arc::clone(&self.sessions),
            session_id: opened.session_id,
            inbox: Mutex::new(Inbox::default()),
        });
        shared.record(SessionEvent::SessionOpened(opened));

        let (mut controller, run_id) =
            run_session(&shared, declaration, planned, implement).await?;
        let ends = controller.exposed();
        let session = LiveSession {
            shared: Arc::clone(&shared),
            run_id,
            ingresses: ends.ingresses,
            egresses: ends
                .egresses
                .into_iter()
                .map(|egress| egress.endpoint)
                .collect(),
            producer: NodeId::generate(),
            stream_id: StreamId::generate(),
            sequence: AtomicI64::new(0),
            turn: Mutex::new(Vec::new()),
            generations: Mutex::new(Generations::default()),
            cancelled: AtomicBool::new(false),
            requests: controller.requests(),
            queues: controller.queues(),
            exposure_queues: controller.exposure_queues(),
        };
        Ok((session, SessionDriver { controller, shared }))
    }
}

/// Creates a run of `planned`, taking each node's implementation from `implement`, attaches it
/// to the session (`AttachRun`), validates, prepares and activates it, and marks the session ready
/// (`MarkSessionReady`) once the run is Active. The run becomes the one the session's inbox ends
/// with.
///
/// # Errors
///
/// The step that failed; the session is then closed (`CloseSession`, `CompleteSessionClose`), and
/// a run that was created is cancelled.
pub(crate) async fn run_session<P: SessionPayload>(
    shared: &Shared,
    declaration: &Declaration,
    planned: &PlannedPipeline,
    mut implement: impl FnMut(RunId, &Node) -> Box<dyn NodeImplementation<P>>,
) -> Result<(RunController<P>, RunId), OpenError> {
    let run_id = RunId::generate();
    let mut controller = match RunController::create_as(
        run_id,
        planned,
        declaration,
        HashMap::<GenerationId, i64>::new(),
        |node| implement(run_id, node),
    ) {
        Ok(controller) => controller,
        Err(error) => {
            abandon(shared);
            return Err(OpenError::Create(error));
        }
    };
    shared.follow(run_id);
    let attached = lock(&shared.sessions).attach_run(
        &Planned {
            declaration,
            run: Some(controller.run()),
        },
        AttachRun {
            session_id: shared.session_id,
            run_id,
        },
    );
    match attached {
        Ok(event) => shared.record(SessionEvent::RunAttached(event)),
        Err(error) => {
            abandon(shared);
            return Err(OpenError::Attach(error));
        }
    }
    if let Err(error) = start(&mut controller).await {
        let _ = controller.cancel().await;
        abandon(shared);
        return Err(OpenError::Run(error));
    }
    let ready = lock(&shared.sessions).mark_session_ready(shared.session_id);
    match ready {
        Ok(event) => shared.record(SessionEvent::SessionReady(event)),
        Err(error) => {
            let _ = controller.cancel().await;
            abandon(shared);
            return Err(OpenError::Ready(error));
        }
    }
    Ok((controller, run_id))
}

/// Validates, prepares and activates the run.
async fn start<P: Send + 'static>(controller: &mut RunController<P>) -> Result<(), RunError> {
    controller.validate().map_err(RunError::Conflict)?;
    controller.prepare().await?;
    controller.activate().await
}

/// Closes a session whose run did not start: `CloseSession`, then `CompleteSessionClose`.
fn abandon(shared: &Shared) {
    let mut sessions = lock(&shared.sessions);
    let draining = sessions.close_session(shared.session_id);
    let closed = sessions.complete_session_close(shared.session_id);
    drop(sessions);
    if let Ok(event) = draining {
        shared.record(SessionEvent::SessionDraining(event));
    }
    shared.end(closed.map(SessionEvent::SessionClosed), false);
}

/// The application's handle on a live session: what it sends, commits and receives, and how it
/// closes.
pub struct LiveSession<P> {
    pub(crate) shared: Arc<Shared>,
    pub(crate) run_id: RunId,
    pub(crate) ingresses: Vec<Ingress<P>>,
    pub(crate) egresses: Vec<Box<dyn InputEndpoint<P>>>,
    pub(crate) producer: NodeId,
    pub(crate) stream_id: StreamId,
    pub(crate) sequence: AtomicI64,
    /// The committed input texts the application sent since its last commit: the user turn the
    /// next commit records.
    pub(crate) turn: Mutex<Vec<String>>,
    /// The generations the session delivered, and those it cut at an interrupt.
    pub(crate) generations: Mutex<Generations>,
    /// The application cancelled the session: nothing more is delivered.
    pub(crate) cancelled: AtomicBool,
    /// How the session asks the run loop to cancel the run.
    pub(crate) requests: RunRequests,
    pub(crate) queues: Vec<QueueWatch<P>>,
    pub(crate) exposure_queues: Vec<ExposureWatch<P>>,
}

impl<P: SessionPayload> LiveSession<P> {
    #[must_use]
    pub fn session_id(&self) -> SessionId {
        self.shared.session_id
    }

    /// The session's current run.
    #[must_use]
    pub const fn run_id(&self) -> RunId {
        self.run_id
    }

    /// A weak reference to the queue of each edge of the session's run; none keeps a queue
    /// alive.
    #[must_use]
    pub fn queues(&self) -> Vec<QueueWatch<P>> {
        self.queues.clone()
    }

    /// A weak reference to the queue of each exposure of the session's run; none keeps a queue
    /// alive.
    #[must_use]
    pub fn exposure_queues(&self) -> Vec<ExposureWatch<P>> {
        self.exposure_queues.clone()
    }

    /// `send`: runs `SendInput` and forwards `envelope` to the ingress its `port` names, stamped
    /// with the session's identity and its current run.
    ///
    /// # Errors
    ///
    /// [`InputError::Refused`] when the session is not Ready, [`InputError::UnknownIngress`] when
    /// the pipeline exposes no ingress under `port`; nothing is forwarded and no event recorded.
    /// [`InputError::Forward`] when the ingress refused the envelope.
    pub async fn send(&self, mut envelope: EventEnvelope<P>) -> Result<(), InputError<P>> {
        let ingress = self
            .ingresses
            .iter()
            .find(|ingress| ingress.name == envelope.port);
        if ingress.is_none() && self.shared.state() == Some(SessionState::Ready) {
            return Err(InputError::UnknownIngress(envelope.port));
        }
        let sent = lock(&self.shared.sessions)
            .send_input(self.shared.session_id)
            .map_err(InputError::Refused)?;
        self.shared.record(SessionEvent::InputSent(sent));
        let Some(ingress) = ingress else {
            return Err(InputError::UnknownIngress(envelope.port));
        };
        envelope.session_id = Some(self.shared.session_id);
        envelope.run_id = self.run_id;
        let input = match envelope.payload.session_event() {
            Some(SessionEvent::TranscriptCommitted(committed)) => Some(committed.text),
            _ => None,
        };
        ingress
            .endpoint
            .send(envelope)
            .await
            .map_err(|error| InputError::Forward(Box::new(error)))?;
        if let Some(text) = input {
            lock(&self.turn).push(text);
        }
        Ok(())
    }

    /// `commit_input`: runs `CommitInput` and sends a control event of kind `commit` into every
    /// ingress.
    ///
    /// # Errors
    ///
    /// [`InputError::Refused`] when the session is not Ready; nothing is sent and no event
    /// recorded. [`InputError::Forward`] when an ingress refused the commit.
    pub async fn commit_input(&self) -> Result<(), InputError<P>> {
        let committed = lock(&self.shared.sessions)
            .commit_input(self.shared.session_id)
            .map_err(InputError::Refused)?;
        self.shared.record(SessionEvent::InputCommitted(committed));
        let turn = std::mem::take(&mut *lock(&self.turn));
        if !turn.is_empty() {
            self.remember(HistoryKind::UserTurn, turn.join(" "), None);
        }
        for ingress in &self.ingresses {
            ingress
                .endpoint
                .send(self.control(&ingress.name, ControlKind::Commit))
                .await
                .map_err(|error| InputError::Forward(Box::new(error)))?;
        }
        Ok(())
    }

    /// `close` by draining: runs `CloseSession`, then sends the end of input, a control event of
    /// kind `stop`, into every ingress and lets go of the ingresses. The run drains; once it has
    /// completed, the driver runs `CompleteSessionClose`.
    ///
    /// # Errors
    ///
    /// [`CloseError::Refused`] when the session is neither Opened nor Ready; nothing moves.
    /// [`CloseError::Forward`] when an ingress refused the end of input; the session is Draining
    /// and the ingresses are let go all the same.
    pub async fn close(&mut self) -> Result<(), CloseError<P>> {
        let draining = lock(&self.shared.sessions)
            .close_session(self.shared.session_id)
            .map_err(CloseError::Refused)?;
        self.shared.record(SessionEvent::SessionDraining(draining));
        let mut refused = None;
        for ingress in &self.ingresses {
            let stop = self.control(&ingress.name, ControlKind::Stop);
            if let Err(error) = ingress.endpoint.send(stop).await {
                refused.get_or_insert(error);
            }
        }
        self.ingresses.clear();
        refused.map_or(Ok(()), |error| Err(CloseError::Forward(Box::new(error))))
    }

    /// `interrupt`: runs `Interrupt`, then cuts every generation the session delivered an event
    /// of, so that nothing more of it is delivered whatever the nodes still compute, and sends a
    /// control event of kind `cancel` into every ingress. What a node does with the `cancel` is
    /// the node's; the session stays Ready, and a generation it has not delivered yet, the next
    /// one, is delivered.
    ///
    /// # Errors
    ///
    /// [`InputError::Refused`] when the session is not Ready; nothing is cut or sent and no event
    /// recorded. [`InputError::Forward`] when an ingress refused the `cancel`; the cut holds.
    pub async fn interrupt(&self) -> Result<(), InputError<P>> {
        let interrupted = lock(&self.shared.sessions)
            .interrupt(self.shared.session_id)
            .map_err(InputError::Refused)?;
        self.shared
            .record(SessionEvent::SessionInterrupted(interrupted));
        {
            let mut generations = lock(&self.generations);
            let delivered: Vec<GenerationId> = generations.delivered.drain().collect();
            generations.cut.extend(delivered);
        }
        for ingress in &self.ingresses {
            ingress
                .endpoint
                .send(self.control(&ingress.name, ControlKind::Cancel))
                .await
                .map_err(|error| InputError::Forward(Box::new(error)))?;
        }
        Ok(())
    }

    /// `close` by cancelling: asks the run loop to cancel the session's run
    /// ([`RunRequests::cancel`]) and delivers nothing more of what the run sends. Once the run
    /// has ended, the session runs `CancelSession`, which delivers `SessionClosed` after how the
    /// run ended; no `SessionDraining` is delivered for a session that was not draining. A
    /// draining session whose drain does not finish is cut the same way.
    ///
    /// # Errors
    ///
    /// [`SessionMoveError::NoSuchSession`] or [`SessionMoveError::WrongState`] when the session
    /// is Closed; nothing is asked of the run.
    pub fn cancel(&self) -> Result<(), SessionMoveError> {
        match self.shared.state() {
            None => {
                return Err(SessionMoveError::NoSuchSession(SessionNotFound {
                    session_id: self.shared.session_id,
                }));
            }
            Some(SessionState::Closed) => {
                return Err(SessionMoveError::WrongState(SessionStateConflict {
                    current: SessionState::Closed,
                }));
            }
            Some(SessionState::Opened | SessionState::Ready | SessionState::Draining) => {}
        }
        self.cancelled.store(true, Ordering::Release);
        self.requests.cancel();
        self.shared.cancel();
        Ok(())
    }

    /// `receive`: the next event of the session. The session's own events come first, in the
    /// order the session recorded them; then what the run sends on its egresses, each payload
    /// converted by [`SessionPayload::session_event`] and skipped when it converts to none.
    /// None once every egress has ended, the run has ended and every event was received.
    pub async fn receive(&mut self) -> Option<SessionEvent> {
        future::poll_fn(|cx| self.poll_receive(cx)).await
    }

    fn poll_receive(&mut self, cx: &mut Context<'_>) -> Poll<Option<SessionEvent>> {
        loop {
            if let Some(event) = self.shared.next(cx) {
                return Poll::Ready(Some(event));
            }
            let mut progressed = false;
            let mut index = 0;
            while index < self.egresses.len() {
                let polled = self.egresses[index].recv().as_mut().poll(cx);
                match polled {
                    Poll::Ready(Some(envelope)) => {
                        progressed = true;
                        if let Some(event) = self.deliver(&envelope) {
                            return Poll::Ready(Some(event));
                        }
                    }
                    Poll::Ready(None) => {
                        progressed = true;
                        drop(self.egresses.remove(index));
                    }
                    Poll::Pending => index += 1,
                }
            }
            if self.egresses.is_empty() {
                return self.shared.after_egress();
            }
            if !progressed {
                return Poll::Pending;
            }
        }
    }

    /// The session event an envelope of an egress is delivered as, if any. A committed fact the
    /// session delivers joins its history: a voice turn's committed transcript as a user turn, a
    /// committed segment as an assistant segment, a tool result with its call.
    ///
    /// Nothing is delivered once the session was cancelled, nor an event of a generation an
    /// interrupt cut: its generation is the envelope's, or the one the event names.
    pub(crate) fn deliver(&self, envelope: &EventEnvelope<P>) -> Option<SessionEvent> {
        if self.cancelled.load(Ordering::Acquire) {
            return None;
        }
        let event = envelope.payload.session_event()?;
        if let Some(generation_id) = envelope.generation_id.or_else(|| generation_of(&event)) {
            let mut generations = lock(&self.generations);
            if generations.cut.contains(&generation_id) {
                return None;
            }
            generations.delivered.insert(generation_id);
        }
        match &event {
            SessionEvent::TranscriptCommitted(committed) => {
                self.remember(HistoryKind::UserTurn, committed.text.clone(), None);
            }
            SessionEvent::TextSegmentCommitted(segment) => {
                self.remember(HistoryKind::AssistantSegment, segment.text.clone(), None);
            }
            SessionEvent::ToolCallResult(result) => {
                self.remember(
                    HistoryKind::ToolResult,
                    result.result.to_string(),
                    Some(result.call_id),
                );
            }
            _ => {}
        }
        Some(event)
    }

    /// Runs `RecordHistory` for the fact `kind` of `text`, at the session's next position, naming
    /// its current run. The `HistoryRecorded` the state machine records is not delivered: history
    /// is read through the view. A Closed session records nothing.
    fn remember(&self, kind: HistoryKind, text: String, call_id: Option<ToolCallId>) {
        let mut sessions = lock(&self.shared.sessions);
        let position = sessions.next_position(self.shared.session_id);
        let _ = sessions.record_history(RecordHistory {
            session_id: self.shared.session_id,
            run_id: self.run_id,
            position,
            kind,
            text,
            call_id,
        });
    }

    /// A control event of `kind` on the session's own stream, for the ingress `port`.
    pub(crate) fn control(&self, port: &str, kind: ControlKind) -> EventEnvelope<P> {
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: self.run_id,
            session_id: Some(self.shared.session_id),
            stream_id: self.stream_id,
            sequence: self.sequence.fetch_add(1, Ordering::Relaxed) + 1,
            producer: self.producer,
            port: port.to_owned(),
            kind: kind.as_str().to_owned(),
            generation_id: None,
            timestamp: now(),
            timebase: Timebase::WallTime,
            causation_id: None,
            payload: P::control(kind),
        }
    }
}

impl<P> Drop for LiveSession<P> {
    fn drop(&mut self) {
        self.shared.release();
    }
}

impl<P> fmt::Debug for LiveSession<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LiveSession")
            .field("session_id", &self.shared.session_id)
            .field("run_id", &self.run_id)
            .field(
                "ingresses",
                &self
                    .ingresses
                    .iter()
                    .map(|ingress| ingress.name.as_str())
                    .collect::<Vec<_>>(),
            )
            .field("egresses", &self.egresses.len())
            .finish_non_exhaustive()
    }
}

/// The generation an event names, for an envelope that names none.
const fn generation_of(event: &SessionEvent) -> Option<GenerationId> {
    match event {
        SessionEvent::GenerationStarted(event) => Some(event.generation_id),
        SessionEvent::GenerationCompleted(event) => Some(event.generation_id),
        SessionEvent::GenerationSuperseded(event) => Some(event.generation_id),
        SessionEvent::GenerationCancelled(event) => Some(event.generation_id),
        SessionEvent::TextDelta(event) => Some(event.generation_id),
        _ => None,
    }
}

/// Milliseconds on the wall clock since the Unix epoch; zero before it.
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// The loop of a live session's run, which the application polls to drive it.
pub struct SessionDriver<P> {
    controller: RunController<P>,
    shared: Arc<Shared>,
}

impl<P: SessionPayload> SessionDriver<P> {
    /// The driver of `controller`'s run, for the session `shared` holds.
    pub(crate) const fn new(controller: RunController<P>, shared: Arc<Shared>) -> Self {
        Self { controller, shared }
    }

    /// Drives the run to its end ([`RunController::drive`]), lets go of the run's queues and
    /// delivers how the run ended, `RunCompleted`, `RunCancelled` or `RunFailed`, after every
    /// event its egresses still hold. A run that completed while the session drains closes the
    /// session (`CompleteSessionClose`) once the application received what the egresses held, or
    /// at once when it let go of its handle, and `SessionClosed` is delivered last. Answers the
    /// run as it ended.
    ///
    /// # Errors
    ///
    /// The error of the run loop; the session's events are delivered as for a run that ended.
    pub async fn drive(self) -> Result<Run, RunError> {
        let Self {
            mut controller,
            shared,
        } = self;
        let driven = controller.drive().await;
        let run = controller.run().clone();
        drop(controller);
        let mut ending = Vec::new();
        let run_id = run.run_id();
        match (run.state(), run.failure()) {
            (RunState::Completed, _) => {
                ending.push(SessionEvent::RunCompleted(RunCompleted { run_id }));
            }
            (RunState::Cancelled, _) => {
                ending.push(SessionEvent::RunCancelled(RunCancelled { run_id }));
            }
            (RunState::Failed, Some(failure)) => {
                ending.push(SessionEvent::RunFailed(RunFailed {
                    run_id,
                    failure: failure.clone(),
                }));
            }
            _ => {}
        }
        shared.run_ended(run_id, run.state(), ending);
        driven.map(|()| run)
    }
}

impl<P: Send + 'static> fmt::Debug for SessionDriver<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionDriver")
            .field("session_id", &self.shared.session_id)
            .field("run_id", &self.controller.run().run_id())
            .finish_non_exhaustive()
    }
}
