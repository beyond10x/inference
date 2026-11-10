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
//! every event of the session was delivered. A run that fails, or that another holder of its
//! requests cancels, while the session drains ends the drain all the same: the session delivers
//! `RunFailed` or `RunCancelled`, runs `CompleteSessionClose` and delivers `SessionClosed` last,
//! and does not stay Draining. An application that lets go of its handle lets go of what was in
//! flight and cancels the run, if it has not ended: the session closes once the run has ended,
//! by `CompleteSessionClose` when the session drains, otherwise by `CancelSession`.
//!
//! Two things the session keeps are bounded, whatever the length of the session and the rate of
//! its input. The state machine's event log ([`LiveSessions::events`]) counts consecutive inputs
//! of a session as one `InputSent` and keeps at most [`Sessions::EVENT_LOG_BOUND`] events, the
//! newest, releasing its oldest half when full ([`LiveSessions::events_released`]);
//! [`LiveSessions::inputs_sent`] counts every input a session's run took, and `InputSent` is
//! recorded only once the ingress took the input. The bookkeeping of the generations an interrupt
//! cuts is released once nothing of a generation can still arrive, its end read and the playback
//! of each of its segments ended, and is kept for at most 256 generations: past them the oldest
//! is let go. A cut generation let go of either way stays known as cut among the last 256 cut
//! generations let go, so that what of it arrives after its end, a refusal among it, stays cut;
//! a late event of a cut generation older than those is delivered, the one exception to the cut
//! ([`LiveSession::interrupt`]). What is not bounded: a session's history grows with its
//! turns, and its row and history stay for as long as the session's state machine lives, Closed
//! included, as the history contract requires ([`crate::history`]).
//!
//! The session records its history ([`crate::history`]) from what it accepts and delivers: the
//! committed input texts the application sent since its last commit as one user turn at
//! [`LiveSession::commit_input`], joined by a space; a committed transcript it delivers, a voice
//! turn's, as a user turn; a committed segment it delivers as an assistant segment; a tool result
//! it delivers as a tool result with its call. A delta or a hypothesis records nothing. A user
//! turn is open from its first [`LiveSession::send`] to its commit: a fact the session delivers
//! meanwhile is recorded after the turn, at the commit, so that the turn precedes the reply that
//! answers it, and the history's positions follow the order it was recorded in. A turn the
//! application does not commit, because it closes, cancels, switches or lets go of its handle,
//! is not recorded; the facts delivered while it was open are, then.
//!
//! A usage record a model node sends on an exposed `usage` port, or a subgraph node forwards on
//! one from its child run, is delivered as `UsageRecorded` and added to the session's usage view,
//! a total per run and one over the session ([`LiveSession::usage`], [`crate::usage`]), the
//! child's record under the run the session holds. A record sent on a `usage` port with no edge
//! and no exposure is counted there too and not delivered; with the feature `cost`, a session
//! priced with `LiveSession::price_with` also prices it. [`LiveSession::record_inputs`] taps the
//! ingresses of the session's run for a recorder of the runtime, so that the run can be replayed.
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

use inference_core::endpoint::{EndpointFuture, InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, SessionId, StreamId};
use inference_core::usage::UsageRecorded;
use inference_pipeline::{
    Declaration, InteractionMode, Node, NodeImplementation, PipelineId, PlannedPipeline,
};
use inference_runtime::record::Recorder;
use inference_runtime::run::{Run, RunCancelled, RunCompleted, RunFailed, RunState};
use inference_runtime::{
    CreateControllerError, ExposedEnds, ExposureWatch, Ingress, QueueWatch, RunController,
    RunError, RunRequests, uncounted_usage,
};
use inference_text::SegmentId;
use inference_tools::ToolCallId;

use crate::events::{SessionEvent, SessionPayload};
use crate::history::{HistoryEntry, HistoryKind, RecordHistory};
use crate::session::{
    AttachRun, AttachRunError, Graph, GraphPipeline, MarkSessionReadyError, OpenSession,
    OpenSessionError, Session, SessionMoveError, SessionNotFound, SessionState,
    SessionStateConflict, Sessions, SessionsRow,
};
use crate::usage::SessionUsage;

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

/// The data of `mutex` through an exclusive borrow, taking the data a panicking holder left.
pub(crate) fn get_mut<T>(mutex: &mut Mutex<T>) -> &mut T {
    mutex.get_mut().unwrap_or_else(PoisonError::into_inner)
}

/// The events of one session not yet received, and how far its run and its close have come.
#[derive(Default)]
struct Inbox {
    /// The session's own events, delivered before what the egresses hold.
    events: VecDeque<SessionEvent>,
    /// How the run ended and what that closed, delivered once every egress has ended.
    ending: VecDeque<SessionEvent>,
    waker: Option<Waker>,
    /// No run of the session sends anything more: its current run ended, or it has none and the
    /// run it left at a switch ended.
    ended: bool,
    /// The application let go of its handle: nothing more is delivered.
    released: bool,
    /// The application cancelled the session: once the run has ended, `CancelSession` closes it.
    cancelling: bool,
    /// `CompleteSessionClose` or `CancelSession` closed the session.
    finished: bool,
    /// The session's current run, whose end the session ends with; none while it switches.
    run: Option<RunId>,
    /// The run the session left at a switch has not ended yet.
    leaving: bool,
    /// How the run the session left at a switch ended, once it has.
    left: Option<RunState>,
    /// The events of the left run's end, held until its egresses have ended, so that they follow
    /// what those egresses still held.
    left_ending: VecDeque<SessionEvent>,
    /// How the session's current run ended, once it has.
    state: Option<RunState>,
}

impl Inbox {
    /// Whether no run of the session is in flight: it has no current run and the run it left, if
    /// any, has ended.
    const fn idle(&self) -> bool {
        self.run.is_none() && !self.leaving
    }

    /// Queues the left run's end among the session's own events, after what its egresses held.
    fn release_left(&mut self) {
        let ending: Vec<SessionEvent> = self.left_ending.drain(..).collect();
        self.events.extend(ending);
    }

    fn wake(&mut self) {
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
    }
}

/// The most generations a live session keeps the bookkeeping of: a memory backstop for
/// generations whose end the session never reads, such as those of a pipeline whose egress
/// carries no generation end or no playback. Past it the oldest generation is released. A cut
/// generation's cut outlives its release: the session remembers the last [`GENERATIONS_KEPT`]
/// cut generations it released, so that what of one arrives after its end, such as a refusal
/// `inference-llm` delivers after a cancel or a supersede, stays cut. Only a cut generation
/// older than those is no longer known as cut; that is the one case in which a late event of an
/// interrupted generation is delivered.
pub(crate) const GENERATIONS_KEPT: usize = 256;

/// The generations a live session delivered an event of, those an interrupt cut, and the
/// generation of each segment a delta named whose playback has not ended: the session delivers
/// nothing more of a cut generation, a segment's events among it.
///
/// A generation's bookkeeping is released once nothing of it can still arrive: it ended, by
/// `GenerationCompleted`, `GenerationSuperseded` or `GenerationCancelled`, and the playback of
/// every segment a delta of it named ended, by `AudioPlayed`, whether the generation was cut or
/// not. A generation's text ends before its audio is played, so one that ended with a segment not
/// yet played is kept. Past [`GENERATIONS_KEPT`] generations kept, the oldest is released. A cut
/// generation, once released, is remembered as cut among the last [`GENERATIONS_KEPT`] released,
/// and nothing of it is delivered or kept again.
#[derive(Debug, Default)]
pub(crate) struct Generations {
    delivered: HashSet<GenerationId>,
    cut: HashSet<GenerationId>,
    /// The cut generations whose bookkeeping was released, the oldest first, at most
    /// [`GENERATIONS_KEPT`].
    released_cut: VecDeque<GenerationId>,
    segments: HashMap<SegmentId, GenerationId>,
    /// The generations whose end the session read.
    ended: HashSet<GenerationId>,
    /// Every generation kept, the oldest first.
    kept: VecDeque<GenerationId>,
}

impl Generations {
    /// Reads an event of an envelope naming `generation`: whether it is delivered, which it is
    /// not when its generation was cut. Its generation is the envelope's, or the one the event
    /// names, or, for an event naming only its segment, the generation a delta of that segment
    /// named; a delta records the generation of its segment. The generation's end, or the
    /// playback of its last segment, releases its bookkeeping.
    fn admit(&mut self, generation: Option<GenerationId>, event: &SessionEvent) -> bool {
        let generation = generation.or_else(|| generation_of(event)).or_else(|| {
            segment_of(event).and_then(|segment_id| self.segments.get(&segment_id).copied())
        });
        let Some(generation_id) = generation else {
            return true;
        };
        if self.released_cut.contains(&generation_id) {
            return false;
        }
        self.keep(generation_id);
        if let SessionEvent::TextDelta(delta) = event {
            self.segments.insert(delta.segment_id, generation_id);
        }
        let admitted = !self.cut.contains(&generation_id);
        if admitted {
            self.delivered.insert(generation_id);
        }
        let last = match event {
            SessionEvent::GenerationCompleted(_)
            | SessionEvent::GenerationSuperseded(_)
            | SessionEvent::GenerationCancelled(_) => {
                self.ended.insert(generation_id);
                true
            }
            SessionEvent::AudioPlayed(played) => {
                self.segments.remove(&played.segment_id);
                true
            }
            _ => false,
        };
        if last
            && self.ended.contains(&generation_id)
            && !self.segments.values().any(|kept| *kept == generation_id)
        {
            self.release(generation_id);
        }
        admitted
    }

    /// Keeps the bookkeeping of `generation_id`; past [`GENERATIONS_KEPT`], the oldest
    /// generation's is released.
    fn keep(&mut self, generation_id: GenerationId) {
        if self.kept.contains(&generation_id) {
            return;
        }
        self.kept.push_back(generation_id);
        while self.kept.len() > GENERATIONS_KEPT {
            if let Some(oldest) = self.kept.front().copied() {
                self.release(oldest);
            }
        }
    }

    /// Releases every record of `generation_id`; a cut one is remembered as cut, past
    /// [`GENERATIONS_KEPT`] remembered the oldest forgotten.
    fn release(&mut self, generation_id: GenerationId) {
        self.delivered.remove(&generation_id);
        if self.cut.remove(&generation_id) {
            self.released_cut.push_back(generation_id);
            while self.released_cut.len() > GENERATIONS_KEPT {
                self.released_cut.pop_front();
            }
        }
        self.ended.remove(&generation_id);
        self.segments.retain(|_, kept| *kept != generation_id);
        self.kept.retain(|kept| *kept != generation_id);
    }

    /// Cuts every generation delivered so far.
    fn interrupt(&mut self) {
        let delivered: Vec<GenerationId> = self.delivered.drain().collect();
        self.cut.extend(delivered);
    }
}

/// The user turn the application is sending: the committed input texts it sent since its last
/// commit, and the facts the session delivered since the first of them, which join the history
/// after the turn.
#[derive(Debug, Default)]
pub(crate) struct Turn {
    texts: Vec<String>,
    held: Vec<Fact>,
}

/// A committed fact the session delivered while a user turn was open, under the run it came from.
#[derive(Debug)]
struct Fact {
    run_id: RunId,
    kind: HistoryKind,
    text: String,
    call_id: Option<ToolCallId>,
}

/// What a live session shares with its driver: the session state machine, the session's inbox
/// and its usage view.
pub(crate) struct Shared {
    pub(crate) sessions: Arc<Mutex<Sessions>>,
    pub(crate) session_id: SessionId,
    inbox: Mutex<Inbox>,
    usage: Mutex<SessionUsage>,
    /// The session's pricing; locked after `usage`, never before it.
    #[cfg(feature = "cost")]
    pricer: Mutex<crate::cost::Pricer>,
}

impl Shared {
    /// Counts `recorded`, read off an egress or a usage sink of the run `run_id`, in the
    /// session's usage view and, with the feature `cost`, prices it.
    fn count_usage(&self, run_id: RunId, recorded: &UsageRecorded) {
        let mut usage = lock(&self.usage);
        usage.add(run_id, recorded);
        #[cfg(feature = "cost")]
        lock(&self.pricer).read(&mut usage.cost, recorded);
    }

    /// Marks the total of the run `run_id`, and the session's, partial, naming each node of
    /// `nodes`, whose usage the session cannot read.
    fn mark_uncounted(&self, run_id: RunId, nodes: impl IntoIterator<Item = NodeId>) {
        let mut usage = lock(&self.usage);
        for node_id in nodes {
            usage.mark_uncounted(run_id, node_id);
        }
    }

    /// The session's current run is `run_id`: its end is the one the session ends with, and its
    /// usage has a total of its own.
    pub(crate) fn follow(&self, run_id: RunId) {
        lock(&self.usage).hold(run_id);
        let mut inbox = lock(&self.inbox);
        inbox.run = Some(run_id);
        inbox.ended = false;
        inbox.state = None;
    }

    /// The session leaves its current run at a switch: the run's end, when it comes, is
    /// delivered among the session's own events once the run's egresses have ended, after what
    /// they held. While the session has no current run, the left run's end counts as its run's.
    pub(crate) fn leave(&self) {
        let mut inbox = lock(&self.inbox);
        inbox.run = None;
        inbox.left = None;
        if let Some(state) = inbox.state.take() {
            let ending: Vec<SessionEvent> = inbox.ending.drain(..).collect();
            inbox.left_ending.extend(ending);
            inbox.left = Some(state);
            inbox.leaving = false;
        } else {
            inbox.leaving = true;
        }
    }

    /// Once the left run's egresses have ended: how the left run ended, its end queued among the
    /// session's own events; pending, with `cx` woken, until the run has ended.
    pub(crate) fn poll_left(&self, cx: &Context<'_>) -> Poll<RunState> {
        let mut inbox = lock(&self.inbox);
        match inbox.left.take() {
            Some(state) => {
                inbox.release_left();
                Poll::Ready(state)
            }
            None => {
                inbox.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }

    /// The run `run_id` ended in `state`, `ending` its events: the end of the session's current
    /// run ([`Shared::end`]), or of a run it left, whose `ending` is held until that run's
    /// egresses have ended. A left run's end, while the session has no current run, is the end
    /// the session's close waits for.
    fn run_ended(&self, run_id: RunId, state: RunState, ending: Vec<SessionEvent>) {
        let mut inbox = lock(&self.inbox);
        if inbox.run == Some(run_id) {
            inbox.state = Some(state);
            drop(inbox);
            self.end(ending);
            return;
        }
        inbox.left_ending.extend(ending);
        inbox.left = Some(state);
        inbox.leaving = false;
        let now = inbox.run.is_none() && {
            inbox.ended = true;
            inbox.released || inbox.cancelling
        };
        inbox.wake();
        drop(inbox);
        if now {
            self.finish();
        }
    }

    /// Runs `RecordHistory` for `fact` at the session's next position. The `HistoryRecorded` the
    /// state machine records is not delivered: history is read through the view. A Closed
    /// session records nothing.
    fn remember(&self, fact: Fact) {
        let mut sessions = lock(&self.sessions);
        let position = sessions.next_position(self.session_id);
        let _ = sessions.record_history(RecordHistory {
            session_id: self.session_id,
            run_id: fact.run_id,
            position,
            kind: fact.kind,
            text: fact.text,
            call_id: fact.call_id,
        });
    }

    /// Queues `event` for the application and wakes a waiting receive. A session that
    /// `CompleteSessionClose` or `CancelSession` closed queued `SessionClosed` last, so it
    /// queues nothing more: an interrupt whose sends finished after a close records no event.
    pub(crate) fn record(&self, event: SessionEvent) {
        let mut inbox = lock(&self.inbox);
        if inbox.finished {
            return;
        }
        inbox.events.push_back(event);
        if let Some(waker) = inbox.waker.take() {
            waker.wake();
        }
    }

    /// The run ended, `completed` or not: no egress sends anything more, and `ending` is
    /// delivered after what the egresses still hold. A session whose application let go of its
    /// handle has nothing left in flight, and one being cancelled delivers nothing more, so
    /// either finishes its close at once.
    fn end(&self, ending: impl IntoIterator<Item = SessionEvent>) {
        let now = {
            let mut inbox = lock(&self.inbox);
            inbox.ending.extend(ending);
            inbox.ended = true;
            if let Some(waker) = inbox.waker.take() {
                waker.wake();
            }
            inbox.released || inbox.cancelling
        };
        if now {
            self.finish();
        }
    }

    /// The application cancelled the session; a run that ended already, or no run in flight,
    /// finishes the close.
    fn cancel(&self) {
        if self.mark(|inbox| inbox.cancelling = true) {
            self.finish();
        }
    }

    /// The application let go of its handle; a run that ended already, or no run in flight,
    /// finishes the close.
    fn release(&self) {
        if self.mark(|inbox| inbox.released = true) {
            self.finish();
        }
    }

    /// The application closes the session by draining: with no run in flight, nothing is, and
    /// the close completes at once.
    fn closing(&self) {
        let idle = {
            let mut inbox = lock(&self.inbox);
            let idle = inbox.idle();
            if idle {
                inbox.ended = true;
            }
            idle
        };
        if idle {
            self.finish();
        }
    }

    /// Applies `how` to the inbox; whether the session's run has ended, which a session with no
    /// run in flight has.
    fn mark(&self, how: impl FnOnce(&mut Inbox)) -> bool {
        let mut inbox = lock(&self.inbox);
        how(&mut inbox);
        if inbox.idle() {
            inbox.ended = true;
        }
        inbox.ended
    }

    /// Closes the session, once, after its run ended: `CancelSession` for a session being
    /// cancelled, whatever its run's end, otherwise `CompleteSessionClose` for a session that
    /// drains, whether its run completed, failed or was cancelled by another holder of its
    /// requests: the drain finished what was in flight, or nothing is in flight any more. A
    /// session that does not drain is left as it is. A session whose application let go of its
    /// handle is closed by `CancelSession` when `CompleteSessionClose` does not close it. The
    /// `SessionClosed` is the last event delivered, after the run's end. The inbox stays locked
    /// from the check to the `SessionClosed` queued, so that a receive finishing the close as the
    /// driver does reads the `SessionClosed` the driver queued, and the lock order is the inbox's,
    /// then the state machine's.
    fn finish(&self) {
        let mut inbox = lock(&self.inbox);
        if inbox.finished || !(inbox.cancelling || inbox.released || inbox.ended) {
            return;
        }
        let closed = {
            let mut sessions = lock(&self.sessions);
            if inbox.cancelling {
                sessions.cancel_session(self.session_id)
            } else {
                match sessions.complete_session_close(self.session_id) {
                    Err(_) if inbox.released => sessions.cancel_session(self.session_id),
                    closed => closed,
                }
            }
        };
        if let Ok(event) = closed {
            inbox.finished = true;
            inbox.ending.push_back(SessionEvent::SessionClosed(event));
            inbox.wake();
        }
    }

    /// Once every egress has ended and what it held was delivered: the session's own next
    /// event, the end of a run it left among them, then the next event of the run's ending,
    /// closing the session first, or none once it was all received; pending, with the waker
    /// [`Shared::next`] registered, until the run has ended.
    fn after_egress(&self) -> Poll<Option<SessionEvent>> {
        {
            let mut inbox = lock(&self.inbox);
            inbox.release_left();
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

    /// The session events the state machine recorded, in order, across every session: at most
    /// [`Sessions::EVENT_LOG_BOUND`], the newest, consecutive inputs of one session counted as one
    /// `InputSent` ([`Sessions::events`]).
    #[must_use]
    pub fn events(&self) -> Vec<SessionEvent> {
        lock(&self.sessions).events().to_vec()
    }

    /// How many events the state machine's log released, the oldest first, to stay within its
    /// bound ([`Sessions::events_released`]); none of them is in [`LiveSessions::events`] any
    /// more.
    #[must_use]
    pub fn events_released(&self) -> u64 {
        lock(&self.sessions).events_released()
    }

    /// How many inputs the run of the session `session_id` took ([`Sessions::inputs_sent`]).
    #[must_use]
    pub fn inputs_sent(&self, session_id: SessionId) -> u64 {
        lock(&self.sessions).inputs_sent(session_id)
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
            usage: Mutex::new(SessionUsage::default()),
            #[cfg(feature = "cost")]
            pricer: Mutex::new(crate::cost::Pricer::default()),
        });
        shared.record(SessionEvent::SessionOpened(opened));

        let (mut controller, run_id) =
            run_session(&shared, declaration, planned, implement).await?;
        let (ingresses, egresses) = outlets(controller.exposed());
        let session = LiveSession {
            shared: Arc::clone(&shared),
            run_id,
            ingresses: Mutex::new(ingresses),
            receiving: Mutex::new(Receiving { egresses }),
            producer: NodeId::generate(),
            stream_id: StreamId::generate(),
            sequence: AtomicI64::new(0),
            turn: Mutex::new(Turn::default()),
            generations: Mutex::new(Generations::default()),
            cancelled: AtomicBool::new(false),
            requests: controller.requests(),
            queues: controller.queues(),
            exposure_queues: controller.exposure_queues(),
            recorded: None,
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
    shared.mark_uncounted(run_id, uncounted_usage(declaration, planned.pipeline_id()));
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
    shared.end(closed.map(SessionEvent::SessionClosed));
}

/// An end the session reads off its run: an egress, whose events it delivers, or the sink of a
/// `usage` port no edge leaves and no exposure carries, whose usage records it counts and does
/// not deliver, since the pipeline exposes no port they travel on.
pub(crate) struct Outlet<P> {
    pub(crate) endpoint: Box<dyn InputEndpoint<P>>,
    pub(crate) delivers: bool,
}

/// What only receiving touches: the ends the session reads, taken under its lock by
/// [`LiveSession::receive`] for one poll and never held across an await.
pub(crate) struct Receiving<P> {
    pub(crate) egresses: Vec<Outlet<P>>,
}

/// The ingresses of `ends`, each shared so that a send holds no lock while it awaits, and the
/// ends the session reads: its egresses, then its usage sinks.
pub(crate) fn outlets<P>(ends: ExposedEnds<P>) -> (Vec<Arc<Ingress<P>>>, Vec<Outlet<P>>) {
    let egresses = ends.egresses.into_iter().map(|egress| Outlet {
        endpoint: egress.endpoint,
        delivers: true,
    });
    let sinks = ends.usage.into_iter().map(|sink| Outlet {
        endpoint: sink.endpoint,
        delivers: false,
    });
    (
        ends.ingresses.into_iter().map(Arc::new).collect(),
        egresses.chain(sinks).collect(),
    )
}

/// The application's handle on a live session: what it sends, commits and receives, and how it
/// closes.
///
/// Every method but [`LiveSession::record_inputs`] and [`LiveSession::switch`] takes `&self`,
/// so one task can receive while another sends or closes through the same shared handle. The
/// receive side keeps its own state behind a lock that only a poll of `receive` takes, never
/// across an await; a send takes the ingresses out of their lock and awaits with no lock held.
pub struct LiveSession<P> {
    pub(crate) shared: Arc<Shared>,
    pub(crate) run_id: RunId,
    pub(crate) ingresses: Mutex<Vec<Arc<Ingress<P>>>>,
    pub(crate) receiving: Mutex<Receiving<P>>,
    pub(crate) producer: NodeId,
    pub(crate) stream_id: StreamId,
    pub(crate) sequence: AtomicI64,
    /// The committed input texts the application sent since its last commit and the position
    /// they took: the user turn the next commit records.
    pub(crate) turn: Mutex<Turn>,
    /// The generations the session delivered, and those it cut at an interrupt.
    pub(crate) generations: Mutex<Generations>,
    /// The application cancelled the session: nothing more is delivered.
    pub(crate) cancelled: AtomicBool,
    /// How the session asks the run loop to cancel the run.
    pub(crate) requests: RunRequests,
    pub(crate) queues: Vec<QueueWatch<P>>,
    pub(crate) exposure_queues: Vec<ExposureWatch<P>>,
    /// The run whose ingresses [`LiveSession::record_inputs`] tapped, if any.
    pub(crate) recorded: Option<RunId>,
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

    /// The session's usage view, read at the call: a total per run it held and one over the
    /// session ([`crate::usage`]).
    #[must_use]
    pub fn usage(&self) -> SessionUsage {
        lock(&self.shared.usage).clone()
    }

    /// Prices the session's usage with `pricing` from now on, and at the call the usage records
    /// it read before it was first priced; the records it priced already keep their cost records
    /// ([`crate::cost`]). What they cost is the `cost` of [`LiveSession::usage`].
    ///
    /// # Errors
    ///
    /// [`CurrencyMismatch`](crate::cost::CurrencyMismatch) when the session was priced before in
    /// another currency; the session keeps its pricing.
    #[cfg(feature = "cost")]
    pub fn price_with(
        &self,
        pricing: crate::cost::Pricing,
    ) -> Result<(), crate::cost::CurrencyMismatch> {
        let mut usage = lock(&self.shared.usage);
        lock(&self.shared.pricer).price_with(&mut usage.cost, pricing)
    }

    /// `send`: forwards `envelope` to the ingress its `port` names, stamped with the session's
    /// identity and its current run, and runs `SendInput` once the ingress took it, so that
    /// `InputSent` names only input the run took.
    ///
    /// # Errors
    ///
    /// [`InputError::Refused`] when the session is not Ready, [`InputError::UnknownIngress`] when
    /// the pipeline exposes no ingress under `port`; nothing is forwarded and no event recorded.
    /// [`InputError::Forward`] when the ingress refused the envelope; no event is recorded.
    /// [`InputError::Refused`] as well when the session was cancelled while the ingress took the
    /// envelope, which the cancelled run then discards; no event is recorded.
    ///
    /// `Ok` once the ingress took the envelope, also when the session moved to Draining, or on
    /// to Closed by that drain, while the ingress took it: the run took the input. `InputSent`
    /// is recorded only while the session is Ready, so an input taken after it left Ready is
    /// answered `Ok` with no `InputSent` and is not counted.
    pub async fn send(&self, mut envelope: EventEnvelope<P>) -> Result<(), InputError<P>> {
        let ingress = lock(&self.ingresses)
            .iter()
            .find(|ingress| ingress.name == envelope.port)
            .map(Arc::clone);
        lock(&self.shared.sessions)
            .admits_input(self.shared.session_id)
            .map_err(InputError::Refused)?;
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
        let sent = lock(&self.shared.sessions).send_input(self.shared.session_id);
        let sent = match sent {
            Ok(sent) => sent,
            Err(SessionMoveError::WrongState(conflict))
                if matches!(
                    conflict.current,
                    SessionState::Draining | SessionState::Closed
                ) && !self.cancelled.load(Ordering::Acquire) =>
            {
                return Ok(());
            }
            Err(error) => return Err(InputError::Refused(error)),
        };
        self.shared.record(SessionEvent::InputSent(sent));
        if let Some(text) = input {
            lock(&self.turn).texts.push(text);
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
        self.end_turn(true);
        for ingress in self.shared_ingresses() {
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
    /// completed, the driver runs `CompleteSessionClose`. A session with no run in flight, one
    /// whose switch did not drain or was let go of once the run it left ended, has nothing to
    /// drain: `CompleteSessionClose` runs at once. While the run it left is still in flight, that
    /// run's end is the one the close waits for.
    ///
    /// # Errors
    ///
    /// [`CloseError::Refused`] when the session is neither Opened nor Ready; nothing moves.
    /// [`CloseError::Forward`] when an ingress refused the end of input; the session is Draining
    /// and the ingresses are let go all the same.
    pub async fn close(&self) -> Result<(), CloseError<P>> {
        let draining = lock(&self.shared.sessions)
            .close_session(self.shared.session_id)
            .map_err(CloseError::Refused)?;
        self.shared.record(SessionEvent::SessionDraining(draining));
        self.end_turn(false);
        let ingresses = std::mem::take(&mut *lock(&self.ingresses));
        let mut refused = None;
        for ingress in ingresses {
            let stop = self.control(&ingress.name, ControlKind::Stop);
            if let Err(error) = ingress.endpoint.send(stop).await {
                refused.get_or_insert(error);
            }
        }
        self.shared.closing();
        refused.map_or(Ok(()), |error| Err(CloseError::Forward(Box::new(error))))
    }

    /// `interrupt`: runs `Interrupt`, then cuts every generation the session delivered an event
    /// of, so that nothing more of it is delivered whatever the nodes still compute, and sends a
    /// control event of kind `cancel` into every ingress. What a node does with the `cancel` is
    /// the node's; the session stays Ready, and a generation it has not delivered yet, the next
    /// one, is delivered.
    ///
    /// The session keeps what it needs to cut a generation until nothing of it can still arrive:
    /// the generation ended and the playback of each of its segments ended. It keeps that for at
    /// most 256 generations; past them it lets go of the oldest. A cut generation it let go of
    /// stays known as cut among the last 256 cut generations let go, so that an event of it
    /// arriving after its end, such as a refusal `inference-llm` delivers after a cancel or a
    /// supersede, is not delivered. A late event of a cut generation older than those is
    /// delivered; that is the one exception to the cut.
    ///
    /// `SessionInterrupted` is recorded once the `cancel` was sent into every ingress, so a
    /// reader that received it knows every ingress holds the `cancel` ahead of any input sent
    /// after it. It is recorded when an ingress refused the `cancel` too.
    ///
    /// # Errors
    ///
    /// [`InputError::Refused`] when the session is not Ready; nothing is cut or sent and no event
    /// recorded. [`InputError::Forward`] when an ingress refused the `cancel`; the cut holds and
    /// `SessionInterrupted` is recorded.
    pub async fn interrupt(&self) -> Result<(), InputError<P>> {
        let interrupted = lock(&self.shared.sessions)
            .interrupt(self.shared.session_id)
            .map_err(InputError::Refused)?;
        lock(&self.generations).interrupt();
        let mut refused = None;
        for ingress in self.shared_ingresses() {
            if let Err(error) = ingress
                .endpoint
                .send(self.control(&ingress.name, ControlKind::Cancel))
                .await
            {
                refused.get_or_insert(error);
            }
        }
        self.shared
            .record(SessionEvent::SessionInterrupted(interrupted));
        refused.map_or(Ok(()), |error| Err(InputError::Forward(Box::new(error))))
    }

    /// The current ingresses, cloned out of their lock so a send awaits with no lock held.
    fn shared_ingresses(&self) -> Vec<Arc<Ingress<P>>> {
        lock(&self.ingresses).clone()
    }

    /// `close` by cancelling: asks the run loop to cancel the session's run
    /// ([`RunRequests::cancel`]) and delivers nothing more of what the run sends. Once the run
    /// has ended, the session runs `CancelSession`, which delivers `SessionClosed` after how the
    /// run ended; no `SessionDraining` is delivered for a session that was not draining. A
    /// draining session whose drain does not finish is cut the same way. A session with no
    /// current run, after a switch that did not drain or was let go of, cancels the run it left:
    /// once that run has ended, or at once when it already has, `CancelSession` closes it.
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
        self.end_turn(false);
        self.cancelled.store(true, Ordering::Release);
        self.requests.cancel();
        self.shared.cancel();
        Ok(())
    }

    /// `receive`: the next event of the session. The session's own events come first, in the
    /// order the session recorded them; then what the run sends on its egresses, each payload
    /// converted by [`SessionPayload::session_event`] and skipped when it converts to none.
    /// None once every egress has ended, the run has ended and every event was received.
    ///
    /// The session's inbox has one waker slot: of several tasks receiving at once, only the last
    /// to poll is woken by the session's own events. Receive from one task.
    pub async fn receive(&self) -> Option<SessionEvent> {
        future::poll_fn(|cx| self.poll_receive(cx)).await
    }

    fn poll_receive(&self, cx: &mut Context<'_>) -> Poll<Option<SessionEvent>> {
        let mut receiving = lock(&self.receiving);
        let egresses = &mut receiving.egresses;
        loop {
            if let Some(event) = self.shared.next(cx) {
                return Poll::Ready(Some(event));
            }
            let mut progressed = false;
            let mut index = 0;
            while index < egresses.len() {
                let delivers = egresses[index].delivers;
                let polled = egresses[index].endpoint.recv().as_mut().poll(cx);
                match polled {
                    Poll::Ready(Some(envelope)) => {
                        progressed = true;
                        if let Some(event) = self.read(&envelope, delivers) {
                            return Poll::Ready(Some(event));
                        }
                    }
                    Poll::Ready(None) => {
                        progressed = true;
                        drop(egresses.remove(index));
                    }
                    Poll::Pending => index += 1,
                }
            }
            if egresses.is_empty() {
                return self.shared.after_egress();
            }
            if !progressed {
                return Poll::Pending;
            }
        }
    }

    /// What the session does with `envelope`, read off one of its ends: delivers it when the end
    /// is an egress ([`LiveSession::deliver`]); otherwise, off a usage sink, counts the usage
    /// record it carries under the envelope's run, as a delivered one is counted, and delivers
    /// nothing.
    pub(crate) fn read(&self, envelope: &EventEnvelope<P>, delivers: bool) -> Option<SessionEvent> {
        if delivers {
            return self.deliver(envelope);
        }
        if let Some(SessionEvent::UsageRecorded(recorded)) = envelope.payload.session_event() {
            self.shared.count_usage(envelope.run_id, &recorded);
        }
        None
    }

    /// The session event an envelope of an egress is delivered as, if any. A committed fact the
    /// session delivers joins its history: a voice turn's committed transcript as a user turn, a
    /// committed segment as an assistant segment, a refusal as an assistant refusal, a tool result
    /// with its call.
    ///
    /// Nothing is delivered once the session was cancelled, nor an event of a generation an
    /// interrupt cut: its generation is the envelope's, or the one the event names, or, for an
    /// event naming only its segment, a committed segment, its synthesis or its playback, the
    /// generation a delta of that segment named. An event that names neither a generation nor a
    /// segment a delta named belongs to no generation and no interrupt cuts it.
    ///
    /// A usage record is added to the usage view under the run whose egress carried it, the
    /// envelope's, whether or not it is delivered ([`crate::usage`]); it belongs to no generation
    /// for the cut, so an interrupt does not cut it.
    pub(crate) fn deliver(&self, envelope: &EventEnvelope<P>) -> Option<SessionEvent> {
        let event = envelope.payload.session_event()?;
        if let SessionEvent::UsageRecorded(recorded) = &event {
            self.shared.count_usage(envelope.run_id, recorded);
            return (!self.cancelled.load(Ordering::Acquire)).then_some(event);
        }
        if self.cancelled.load(Ordering::Acquire) {
            return None;
        }
        if !lock(&self.generations).admit(envelope.generation_id, &event) {
            return None;
        }
        match &event {
            SessionEvent::TranscriptCommitted(committed) => {
                self.remember(HistoryKind::UserTurn, committed.text.clone(), None);
            }
            SessionEvent::TextSegmentCommitted(segment) => {
                self.remember(HistoryKind::AssistantSegment, segment.text.clone(), None);
            }
            SessionEvent::GenerationRefused(refused) => {
                self.remember(HistoryKind::AssistantRefusal, refused.text.clone(), None);
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

    /// Records the fact `kind` of `text`, naming the session's current run ([`Shared::remember`]);
    /// while a user turn is open, once the turn was recorded ([`LiveSession::end_turn`]), so that
    /// the turn precedes the reply that answers it.
    fn remember(&self, kind: HistoryKind, text: String, call_id: Option<ToolCallId>) {
        let fact = Fact {
            run_id: self.run_id,
            kind,
            text,
            call_id,
        };
        let mut turn = lock(&self.turn);
        if turn.texts.is_empty() {
            self.shared.remember(fact);
        } else {
            turn.held.push(fact);
        }
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

impl<P: SessionPayload + Clone + fmt::Debug> LiveSession<P> {
    /// Taps the ingresses of the session's current run: from now on `recorder` records every
    /// envelope sent through them, the application's input and the session's control events, as
    /// an input of the run ([`Recorder::ingress`]), so that a replay can feed them into another
    /// run of the pipeline. With the nodes wrapped by the same recorder as the session creates
    /// them ([`Recorder::wrap`], in the `implement` given to [`LiveSessions::open`]), the
    /// recording holds the run's whole event path, its usage records among it. A switch gives the
    /// session another run, whose ingresses are not tapped until this is called again.
    ///
    /// # Errors
    ///
    /// [`AlreadyRecorded`] when the ingresses of the current run are tapped already, by this
    /// recorder or another: a second tap would record, and a replay feed, every input twice.
    /// Nothing is tapped again.
    pub fn record_inputs(&mut self, recorder: &Recorder<P>) -> Result<(), AlreadyRecorded> {
        if self.recorded == Some(self.run_id) {
            return Err(AlreadyRecorded {
                run_id: self.run_id,
            });
        }
        self.recorded = Some(self.run_id);
        for shared in get_mut(&mut self.ingresses) {
            let ingress = Arc::get_mut(shared)
                .expect("no send holds an ingress while the session is borrowed exclusively");
            let endpoint = std::mem::replace(&mut ingress.endpoint, Box::new(Detached));
            ingress.endpoint = recorder.ingress(ingress.name.clone(), endpoint);
        }
        Ok(())
    }
}

/// [`LiveSession::record_inputs`] refused: the ingresses of the run `run_id` are tapped already.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlreadyRecorded {
    pub run_id: RunId,
}

impl fmt::Display for AlreadyRecorded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the ingresses of run {} are recorded already; a second tap would record every \
             input twice",
            self.run_id
        )
    }
}

impl Error for AlreadyRecorded {}

/// The place an ingress's endpoint is held in while it is wrapped: it takes nothing.
struct Detached;

impl<P: Send + 'static> OutputEndpoint<P> for Detached {
    fn send(&self, envelope: EventEnvelope<P>) -> EndpointFuture<'_, Result<(), SendError<P>>> {
        Box::pin(future::ready(Err(SendError::Closed(envelope))))
    }
}

impl<P> LiveSession<P> {
    /// Ends the open user turn: records it, joined by a space, when it is `committed` and the
    /// application sent text in it, then the facts delivered while it was open, in order.
    pub(crate) fn end_turn(&self, committed: bool) {
        let mut turn = lock(&self.turn);
        let Turn { texts, held } = std::mem::take(&mut *turn);
        if committed && !texts.is_empty() {
            self.shared.remember(Fact {
                run_id: self.run_id,
                kind: HistoryKind::UserTurn,
                text: texts.join(" "),
                call_id: None,
            });
        }
        for fact in held {
            self.shared.remember(fact);
        }
    }
}

/// Letting go of the handle ends the open user turn uncommitted and cancels the session's run,
/// which then ends, so that the session closes: by `CompleteSessionClose` when its run completed
/// while it drains, otherwise by `CancelSession`.
impl<P> Drop for LiveSession<P> {
    fn drop(&mut self) {
        self.end_turn(false);
        self.requests.cancel();
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
                &lock(&self.ingresses)
                    .iter()
                    .map(|ingress| ingress.name.clone())
                    .collect::<Vec<_>>(),
            )
            .field("egresses", &lock(&self.receiving).egresses.len())
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
        SessionEvent::GenerationRefused(event) => Some(event.generation_id),
        SessionEvent::TextDelta(event) => Some(event.generation_id),
        _ => None,
    }
}

/// The segment an event names when it names no generation: a committed segment, its synthesis,
/// its playback.
const fn segment_of(event: &SessionEvent) -> Option<SegmentId> {
    match event {
        SessionEvent::TextSegmentCommitted(event) => Some(event.segment_id),
        SessionEvent::SynthesisCompleted(event) => Some(event.segment_id),
        SessionEvent::AudioPlayed(event) => Some(event.segment_id),
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
    /// event its egresses still hold. A run that ends while the session drains, whether it
    /// completed, failed or was cancelled by another holder of its requests, closes the session
    /// (`CompleteSessionClose`) once the application received what the egresses held, or at once
    /// when it let go of its handle, and `SessionClosed` is delivered last, after the run's end.
    /// Answers the run as it ended.
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

#[cfg(test)]
mod tests {
    use inference_core::generation::{
        GenerationCancelled, GenerationCompleted, GenerationRefused, GenerationStarted,
    };
    use inference_pipeline::PipelineState;
    use inference_text::{AudioPlayed, TextDelta};

    use super::*;
    use crate::session::SessionClosed;

    /// One Planned turn-based pipeline, which every run executes.
    struct OnePipeline(PipelineId);

    impl Graph for OnePipeline {
        fn pipeline(&self, pipeline_id: PipelineId) -> Option<GraphPipeline> {
            (pipeline_id == self.0).then_some(GraphPipeline {
                state: PipelineState::Planned,
                mode: InteractionMode::TurnBased,
            })
        }

        fn run_pipeline(&self, _: RunId) -> Option<PipelineId> {
            Some(self.0)
        }
    }

    /// What a live session shares with its driver once `close()` left it Draining, its run
    /// still in flight.
    fn draining() -> (Arc<Shared>, RunId) {
        let graph = OnePipeline(PipelineId::generate());
        let sessions = Arc::new(Mutex::new(Sessions::counted_in(3)));
        let session_id = lock(&sessions)
            .open_session(
                &graph,
                OpenSession {
                    pipeline_id: graph.0,
                    mode: InteractionMode::TurnBased,
                },
            )
            .expect("opens")
            .session_id;
        let shared = Arc::new(Shared {
            sessions: Arc::clone(&sessions),
            session_id,
            inbox: Mutex::new(Inbox::default()),
            usage: Mutex::new(SessionUsage::default()),
            #[cfg(feature = "cost")]
            pricer: Mutex::new(crate::cost::Pricer::default()),
        });
        let run_id = RunId::generate();
        shared.follow(run_id);
        {
            let mut sessions = lock(&sessions);
            sessions
                .attach_run(&graph, AttachRun { session_id, run_id })
                .expect("attached");
            sessions.mark_session_ready(session_id).expect("ready");
            sessions.close_session(session_id).expect("drains");
        }
        shared.closing();
        (shared, run_id)
    }

    /// Everything the session delivers once every egress has ended, to its last event.
    fn after_egress(shared: &Shared) -> Vec<SessionEvent> {
        let mut received = Vec::new();
        loop {
            match shared.after_egress() {
                Poll::Ready(Some(event)) => received.push(event),
                Poll::Ready(None) => return received,
                Poll::Pending => panic!("the run has ended: nothing is pending"),
            }
        }
    }

    /// `close()`, then another holder of the run's requests cancels it: the session delivers
    /// `RunCancelled`, then `SessionClosed` last, and is Closed.
    #[test]
    fn a_draining_session_whose_run_another_holder_cancelled_closes() {
        let (shared, run_id) = draining();
        shared.run_ended(
            run_id,
            RunState::Cancelled,
            vec![SessionEvent::RunCancelled(RunCancelled { run_id })],
        );
        assert_eq!(
            after_egress(&shared),
            [
                SessionEvent::RunCancelled(RunCancelled { run_id }),
                SessionEvent::SessionClosed(SessionClosed {
                    session_id: shared.session_id,
                }),
            ],
            "the run's end, then SessionClosed last"
        );
        assert_eq!(shared.state(), Some(SessionState::Closed));
    }

    fn delta(segment_id: SegmentId, generation_id: GenerationId) -> SessionEvent {
        SessionEvent::TextDelta(TextDelta {
            segment_id,
            generation_id,
            sequence: 1,
            text: "a".to_owned(),
        })
    }

    /// The start of `generation_id`, which names no segment.
    fn started(generation_id: GenerationId) -> SessionEvent {
        SessionEvent::GenerationStarted(GenerationStarted {
            generation_id,
            run_id: RunId::generate(),
            epoch: 1,
        })
    }

    /// One generation from its start to its completion, with one segment.
    fn generation(generations: &mut Generations) -> (GenerationId, SegmentId) {
        let (generation_id, segment_id) = (GenerationId::generate(), SegmentId::generate());
        for event in [
            SessionEvent::GenerationStarted(GenerationStarted {
                generation_id,
                run_id: RunId::generate(),
                epoch: 1,
            }),
            delta(segment_id, generation_id),
            SessionEvent::GenerationCompleted(GenerationCompleted {
                generation_id,
                stop_reason: None,
            }),
        ] {
            assert!(generations.admit(None, &event));
        }
        (generation_id, segment_id)
    }

    /// Whether `generations` keeps anything of `generation_id`.
    fn keeps(generations: &Generations, generation_id: GenerationId) -> bool {
        generations.delivered.contains(&generation_id)
            || generations.cut.contains(&generation_id)
            || generations
                .segments
                .values()
                .any(|kept| *kept == generation_id)
    }

    fn played(segment_id: SegmentId) -> SessionEvent {
        SessionEvent::AudioPlayed(AudioPlayed { segment_id })
    }

    /// A generation's bookkeeping is released once nothing of it can still arrive: it ended and
    /// its segment's playback ended, in either order.
    #[test]
    fn generation_bookkeeping_is_released_after_a_generation_ends() {
        let mut generations = Generations::default();
        let (first, segment) = generation(&mut generations);
        assert!(
            keeps(&generations, first),
            "an ended generation whose segment is not played yet is kept"
        );
        assert!(generations.admit(None, &played(segment)));
        assert!(
            !keeps(&generations, first),
            "an ended generation whose playback ended is released"
        );
        assert!(generations.kept.is_empty() && generations.ended.is_empty());

        let (second, segment) = (GenerationId::generate(), SegmentId::generate());
        assert!(generations.admit(None, &delta(segment, second)));
        assert!(generations.admit(None, &played(segment)));
        assert!(
            keeps(&generations, second),
            "a generation not ended is kept"
        );
        assert!(generations.admit(
            None,
            &SessionEvent::GenerationCompleted(GenerationCompleted {
                generation_id: second,
                stop_reason: None,
            }),
        ));
        assert!(
            !keeps(&generations, second),
            "played before it ended, released at its end"
        );

        let silent = GenerationId::generate();
        for event in [
            SessionEvent::GenerationStarted(GenerationStarted {
                generation_id: silent,
                run_id: RunId::generate(),
                epoch: 1,
            }),
            SessionEvent::GenerationCompleted(GenerationCompleted {
                generation_id: silent,
                stop_reason: None,
            }),
        ] {
            assert!(generations.admit(None, &event));
        }
        assert!(
            !keeps(&generations, silent),
            "a generation with no segment is released at its end"
        );
        assert!(generations.kept.is_empty());
    }

    /// A cut generation is kept, so that its late events stay cut, until nothing of it can still
    /// arrive; then it is released.
    #[test]
    fn a_cut_generation_is_released_once_its_end_and_playback_were_read() {
        let mut generations = Generations::default();
        let (cut, segment) = (GenerationId::generate(), SegmentId::generate());
        assert!(generations.admit(None, &delta(segment, cut)));
        generations.interrupt();
        assert!(!generations.admit(
            None,
            &SessionEvent::GenerationCancelled(GenerationCancelled { generation_id: cut }),
        ));
        assert!(keeps(&generations, cut), "its segment may still be played");
        assert!(
            !generations.admit(None, &delta(segment, cut)),
            "a late delta of the cut generation stays cut"
        );
        assert!(
            !generations.admit(None, &played(segment)),
            "its playback is cut"
        );
        assert!(!keeps(&generations, cut), "then it is released");
    }

    /// Generations whose end the session never reads, as on a pipeline whose egress carries no
    /// generation end and no playback, are kept within the backstop.
    #[test]
    fn generations_that_never_end_stay_within_the_backstop() {
        let mut generations = Generations::default();
        let first = GenerationId::generate();
        assert!(generations.admit(None, &delta(SegmentId::generate(), first)));
        for _ in 0..1000 {
            let generation_id = GenerationId::generate();
            assert!(generations.admit(None, &delta(SegmentId::generate(), generation_id)));
        }
        assert!(
            generations.kept.len() <= GENERATIONS_KEPT
                && generations.delivered.len() <= GENERATIONS_KEPT
                && generations.segments.len() <= GENERATIONS_KEPT,
            "{} kept, {} delivered, {} segments past the backstop of {GENERATIONS_KEPT}",
            generations.kept.len(),
            generations.delivered.len(),
            generations.segments.len()
        );
        assert!(!keeps(&generations, first), "the oldest is let go");
        generations.interrupt();
        assert!(generations.cut.len() <= GENERATIONS_KEPT);
    }

    /// An interrupt still cuts the playback of a generation that completed: an ended generation's
    /// bookkeeping outlives its last text event, since its audio is played after it.
    #[test]
    fn an_interrupt_cuts_the_playback_of_a_generation_that_completed() {
        let mut generations = Generations::default();
        let (_, segment_id) = generation(&mut generations);
        generations.interrupt();
        assert!(
            !generations.admit(None, &SessionEvent::AudioPlayed(AudioPlayed { segment_id })),
            "the completed generation's playback is cut"
        );
    }

    /// Every event of a cut generation that arrives after its end, its refusal or a late delta,
    /// stays cut once its bookkeeping was released, and is not kept again; the cut generations
    /// remembered so stay within the backstop.
    #[test]
    fn a_cut_generation_stays_cut_past_its_end_within_the_backstop() {
        let mut generations = Generations::default();
        let cut = GenerationId::generate();
        assert!(generations.admit(None, &started(cut)));
        generations.interrupt();
        assert!(!generations.admit(
            None,
            &SessionEvent::GenerationCancelled(GenerationCancelled { generation_id: cut }),
        ));
        assert!(
            !generations.admit(
                None,
                &SessionEvent::GenerationRefused(GenerationRefused {
                    generation_id: cut,
                    text: "no".to_owned(),
                }),
            ),
            "a refusal after the end of a cut generation stays cut"
        );
        assert!(
            !generations.admit(None, &delta(SegmentId::generate(), cut)),
            "so does a late delta"
        );
        assert!(
            !generations.kept.contains(&cut),
            "and nothing of it is kept again"
        );
        for _ in 0..=GENERATIONS_KEPT {
            let other = GenerationId::generate();
            assert!(generations.admit(None, &started(other)));
            generations.interrupt();
            assert!(!generations.admit(
                None,
                &SessionEvent::GenerationCompleted(GenerationCompleted {
                    generation_id: other,
                    stop_reason: None,
                }),
            ));
        }
        assert!(generations.released_cut.len() <= GENERATIONS_KEPT);
        assert!(
            !generations.released_cut.contains(&cut),
            "past the backstop the oldest cut generation is let go"
        );
    }

    /// A refusal names its generation and nothing else, so an envelope that names none is cut
    /// with the generation the refusal names, and a refusal of a generation not cut is delivered.
    #[test]
    fn an_interrupt_cuts_the_refusal_of_the_cut_generation() {
        let mut generations = Generations::default();
        let (cut, segment) = (GenerationId::generate(), SegmentId::generate());
        assert!(generations.admit(None, &delta(segment, cut)));
        generations.interrupt();
        let refused = |generation_id| {
            SessionEvent::GenerationRefused(GenerationRefused {
                generation_id,
                text: "no".to_owned(),
            })
        };
        assert!(
            !generations.admit(None, &refused(cut)),
            "the refusal of the cut generation is cut, story:facade-session-refusal-event"
        );
        assert!(
            generations.admit(None, &refused(GenerationId::generate())),
            "the refusal of another generation is delivered"
        );
    }
}
