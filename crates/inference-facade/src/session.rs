//! The `inference.session` domain of `ess/domains/session.yaml`: the session state machine.
//!
//! [`Sessions`] holds the sessions, each with the identity of its current run, zero or one, and
//! records every event it emits in order. The nouns are the domain's, in its terms: [`Session`],
//! [`SessionState`], the inputs, the events and the refusals. A [`Sessions`] from
//! [`Sessions::new`] gives each session a random identity; one from [`Sessions::counted_in`] gives
//! the next value of a counter, so that two runs of the same commands report the same identities,
//! as a store or a test needs. A refused command takes no identity.
//!
//! What a session reads of the `inference.graph` domain, which it references and does not hold,
//! it reads through [`Graph`] at the call: `OpenSession` the pipeline's existence, state and mode,
//! and `AttachRun` the pipeline of the run it attaches. A command refuses, stores nothing and
//! records no event in the order the specification answers it:
//!
//! * `OpenSession`: a pipeline no record carries, one that is not Planned, one of another mode.
//! * `AttachRun`: a session no record carries, a run whose pipeline is not the session's, a
//!   session that is Draining or Closed. The pipeline comparison is the specification's external
//!   refusal and answers before the held state, as ESS orders an external branch that moves
//!   nothing; a run the graph holds no record of is not refused, since the specification declares
//!   no refusal of it, and is attached by its identity.
//! * `MarkSessionReady`: a session no record carries, a session that is not Opened, an Opened
//!   session with no current run.
//! * `CloseSession` and `CompleteSessionClose`: a session no record carries, a session not in a
//!   state the move starts from.
//! * `SendInput` and `CommitInput`: a session no record carries, a session that is not Ready.
//!   Each accepted is a move from Ready to Ready, which `ess/domains/session.yaml` declares
//!   because ESS synthesizes no scenario for an accepting outcome that moves nothing; neither
//!   changes the session.
//!
//! Attaching moves the session to Opened from Opened or Ready, so a session that takes another
//! run is ready again only once `MarkSessionReady` says so. Two rules the specification leaves
//! `UNMAPPED` are not enforced, as it declares them: attaching the run a session already holds
//! moves it to Opened and records `RunAttached` again, and one run may be current in several
//! sessions. The view `inference.session.Sessions` is [`Sessions::sessions_view`], read at the
//! call.

use std::error::Error;
use std::fmt;

use uuid::Uuid;

use inference_core::id::{RunId, SessionId};
use inference_pipeline::{
    InteractionMode, PipelineId, PipelineNotFound, PipelineNotReady, PipelineState,
};

pub use crate::events::SessionEvent;
pub use crate::history::{
    HistoryEntry, HistoryEntryId, HistoryKind, HistoryRecorded, RecordHistory, RecordHistoryError,
};

/// What a session reads of a pipeline: its state and its interaction mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphPipeline {
    pub state: PipelineState,
    pub mode: InteractionMode,
}

/// What a session reads of the `inference.graph` domain, which it references and does not hold:
/// the pipelines a session is opened over and the runs it attaches.
pub trait Graph {
    /// The state and mode of the pipeline carrying `pipeline_id`; none when no record carries it.
    fn pipeline(&self, pipeline_id: PipelineId) -> Option<GraphPipeline>;

    /// The pipeline the run carrying `run_id` executes; none when no record carries the run.
    fn run_pipeline(&self, run_id: RunId) -> Option<PipelineId>;
}

/// `inference.session.Session.State`: Opened, then Ready once the application says so, then
/// Draining once closing starts, then Closed. Serialized as written in ESS (`Opened`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionState {
    /// The session exists; it is not ready for interaction yet.
    Opened,
    /// The session accepts interaction.
    Ready,
    /// The session admits no new interaction and drains what is in flight.
    Draining,
    /// The drain finished and the session's resources are released.
    Closed,
}

impl SessionState {
    /// Every state, in the order `ess/domains/session.yaml` lists them.
    pub const ALL: [Self; 4] = [Self::Opened, Self::Ready, Self::Draining, Self::Closed];

    /// The state as ESS writes it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Opened => "Opened",
            Self::Ready => "Ready",
            Self::Draining => "Draining",
            Self::Closed => "Closed",
        }
    }
}

impl fmt::Display for SessionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `inference.session.Session`: an optional application-level interaction lifetime above graph
/// runs, opened over one pipeline in its interaction mode, holding its current run, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    session_id: SessionId,
    pipeline_id: PipelineId,
    mode: InteractionMode,
    run_id: Option<RunId>,
    state: SessionState,
}

impl Session {
    #[must_use]
    pub const fn session_id(&self) -> SessionId {
        self.session_id
    }

    #[must_use]
    pub const fn pipeline_id(&self) -> PipelineId {
        self.pipeline_id
    }

    #[must_use]
    pub const fn mode(&self) -> InteractionMode {
        self.mode
    }

    /// The current run; none until a run is attached.
    #[must_use]
    pub const fn run_id(&self) -> Option<RunId> {
        self.run_id
    }

    #[must_use]
    pub const fn state(&self) -> SessionState {
        self.state
    }
}

/// `inference.session.OpenSession`: the input that opens a [`Session`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenSession {
    pub pipeline_id: PipelineId,
    pub mode: InteractionMode,
}

/// `inference.session.AttachRun`: the input that makes a run the session's current run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttachRun {
    pub session_id: SessionId,
    pub run_id: RunId,
}

/// `inference.session.SessionOpened`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionOpened {
    pub session_id: SessionId,
    pub pipeline_id: PipelineId,
}

/// `inference.session.RunAttached`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunAttached {
    pub session_id: SessionId,
    pub run_id: RunId,
}

/// `inference.session.SessionReady`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionReady {
    pub session_id: SessionId,
}

/// `inference.session.SessionDraining`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionDraining {
    pub session_id: SessionId,
}

/// `inference.session.SessionClosed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionClosed {
    pub session_id: SessionId,
}

/// `inference.session.InputSent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputSent {
    pub session_id: SessionId,
}

/// `inference.session.SwitchPipeline`: the input that moves a Ready session over to another
/// pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwitchPipeline {
    pub session_id: SessionId,
    pub pipeline_id: PipelineId,
    pub mode: InteractionMode,
}

/// `inference.session.PipelineSwitched`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipelineSwitched {
    pub session_id: SessionId,
    pub pipeline_id: PipelineId,
}

/// Why `inference.session.SwitchPipeline` switched nothing, in the order
/// [`Sessions::switch_pipeline`] decides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchPipelineError {
    /// The `no-such-session` outcome.
    NoSuchSession(SessionNotFound),
    /// The `unknown-pipeline` outcome.
    UnknownPipeline(PipelineNotFound),
    /// The `not-planned` outcome.
    NotPlanned(PipelineNotReady),
    /// The `mode-mismatch` outcome.
    ModeMismatch(ModeMismatch),
    /// The `wrong-state` outcome: the session is not Ready.
    WrongState(SessionStateConflict),
}

impl fmt::Display for SwitchPipelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchSession(error) => fmt::Display::fmt(error, f),
            Self::UnknownPipeline(error) => fmt::Display::fmt(error, f),
            Self::NotPlanned(error) => fmt::Display::fmt(error, f),
            Self::ModeMismatch(error) => fmt::Display::fmt(error, f),
            Self::WrongState(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for SwitchPipelineError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::NoSuchSession(error) => Some(error),
            Self::UnknownPipeline(error) => Some(error),
            Self::NotPlanned(error) => Some(error),
            Self::ModeMismatch(error) => Some(error),
            Self::WrongState(error) => Some(error),
        }
    }
}

/// `inference.session.SessionInterrupted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionInterrupted {
    pub session_id: SessionId,
}

/// `inference.session.InputCommitted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputCommitted {
    pub session_id: SessionId,
}

/// `inference.session.ModeMismatch`: the pipeline declares another interaction mode; no session
/// was opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModeMismatch {
    pub pipeline_id: PipelineId,
    pub mode: InteractionMode,
}

impl fmt::Display for ModeMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the pipeline {} declares another mode than {:?}",
            self.pipeline_id, self.mode
        )
    }
}

impl Error for ModeMismatch {}

/// `inference.session.SessionNotFound`: no session carries the identity the command named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionNotFound {
    pub session_id: SessionId,
}

impl fmt::Display for SessionNotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no session carries the identity {}", self.session_id)
    }
}

impl Error for SessionNotFound {}

/// `inference.session.SessionStateConflict`: the session is not in a state the command acts
/// from; `current` is the state it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionStateConflict {
    pub current: SessionState,
}

impl fmt::Display for SessionStateConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the session is {}", self.current)
    }
}

impl Error for SessionStateConflict {}

/// `inference.session.PipelineMismatch`: the run executes another pipeline than the session's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipelineMismatch {
    pub session_id: SessionId,
    pub run_id: RunId,
}

impl fmt::Display for PipelineMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the run {} executes another pipeline than the session {}",
            self.run_id, self.session_id
        )
    }
}

impl Error for PipelineMismatch {}

/// `inference.session.NoCurrentRun`: the session references no run, so it does not become
/// ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoCurrentRun {
    pub session_id: SessionId,
}

impl fmt::Display for NoCurrentRun {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the session {} references no run", self.session_id)
    }
}

impl Error for NoCurrentRun {}

/// Why `inference.session.OpenSession` opened nothing, in the order [`Sessions::open_session`]
/// decides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenSessionError {
    /// The `unknown-pipeline` outcome.
    UnknownPipeline(PipelineNotFound),
    /// The `not-planned` outcome.
    NotPlanned(PipelineNotReady),
    /// The `mode-mismatch` outcome.
    ModeMismatch(ModeMismatch),
}

impl fmt::Display for OpenSessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownPipeline(error) => fmt::Display::fmt(error, f),
            Self::NotPlanned(error) => fmt::Display::fmt(error, f),
            Self::ModeMismatch(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for OpenSessionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::UnknownPipeline(error) => Some(error),
            Self::NotPlanned(error) => Some(error),
            Self::ModeMismatch(error) => Some(error),
        }
    }
}

/// Why `inference.session.AttachRun` attached nothing, in the order [`Sessions::attach_run`]
/// decides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachRunError {
    /// The `no-such-session` outcome.
    NoSuchSession(SessionNotFound),
    /// The `pipeline-mismatch` outcome.
    PipelineMismatch(PipelineMismatch),
    /// The `wrong-state` outcome: the session is Draining or Closed.
    WrongState(SessionStateConflict),
}

impl fmt::Display for AttachRunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchSession(error) => fmt::Display::fmt(error, f),
            Self::PipelineMismatch(error) => fmt::Display::fmt(error, f),
            Self::WrongState(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for AttachRunError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::NoSuchSession(error) => Some(error),
            Self::PipelineMismatch(error) => Some(error),
            Self::WrongState(error) => Some(error),
        }
    }
}

/// Why `inference.session.MarkSessionReady` moved nothing, in the order
/// [`Sessions::mark_session_ready`] decides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkSessionReadyError {
    /// The `no-such-session` outcome.
    NoSuchSession(SessionNotFound),
    /// The `no-current-run` outcome: the session is Opened and references no run.
    NoCurrentRun(NoCurrentRun),
    /// The `wrong-state` outcome: the session is not Opened.
    WrongState(SessionStateConflict),
}

impl fmt::Display for MarkSessionReadyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchSession(error) => fmt::Display::fmt(error, f),
            Self::NoCurrentRun(error) => fmt::Display::fmt(error, f),
            Self::WrongState(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for MarkSessionReadyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::NoSuchSession(error) => Some(error),
            Self::NoCurrentRun(error) => Some(error),
            Self::WrongState(error) => Some(error),
        }
    }
}

/// Why `inference.session.CloseSession` or `inference.session.CompleteSessionClose` moved nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMoveError {
    /// The `no-such-session` outcome.
    NoSuchSession(SessionNotFound),
    /// The `wrong-state` outcome.
    WrongState(SessionStateConflict),
}

impl fmt::Display for SessionMoveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchSession(error) => fmt::Display::fmt(error, f),
            Self::WrongState(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for SessionMoveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::NoSuchSession(error) => Some(error),
            Self::WrongState(error) => Some(error),
        }
    }
}

/// A row of the view `inference.session.Sessions`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionsRow {
    pub session_id: SessionId,
    pub pipeline_id: PipelineId,
    pub mode: InteractionMode,
    pub run_id: Option<RunId>,
    pub state: SessionState,
}

/// Where [`Sessions`] takes the identities of the sessions it opens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Identities {
    /// A random (version 4) identity each time.
    #[default]
    Generated,
    /// The next value of a counter in the first eight hex digits' `space`,
    /// `{space:08x}-0000-4000-8000-000000000001` first; `issued` is the count issued so far.
    Counted { space: u32, issued: u128 },
}

/// The sessions, in the order they were opened, their history, in the order it was recorded,
/// and the events the domain emitted. A session stays in every state, Closed included, and its
/// history with it.
#[derive(Debug, Clone, Default)]
pub struct Sessions {
    sessions: Vec<Session>,
    history: Vec<HistoryEntry>,
    events: Vec<SessionEvent>,
    identities: Identities,
}

impl Sessions {
    /// No session, giving random identities.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// No session, giving counted identities whose first eight hex digits are `space`,
    /// `{space:08x}-0000-4000-8000-000000000001` first, so that stores counting in different
    /// spaces never issue the same identity.
    #[must_use]
    pub fn counted_in(space: u32) -> Self {
        Self {
            identities: Identities::Counted { space, issued: 0 },
            ..Self::default()
        }
    }

    /// Runs `inference.session.OpenSession` against the pipelines `graph` holds: an Opened
    /// session over the pipeline and mode, with no current run, and its [`SessionOpened`].
    ///
    /// # Errors
    ///
    /// In this order: [`OpenSessionError::UnknownPipeline`], [`OpenSessionError::NotPlanned`]
    /// requiring Planned, [`OpenSessionError::ModeMismatch`]. Nothing is stored, no event is
    /// recorded and no identity is issued.
    pub fn open_session(
        &mut self,
        graph: &(impl Graph + ?Sized),
        input: OpenSession,
    ) -> Result<SessionOpened, OpenSessionError> {
        let pipeline_id = input.pipeline_id;
        let pipeline = graph
            .pipeline(pipeline_id)
            .ok_or(OpenSessionError::UnknownPipeline(PipelineNotFound {
                pipeline_id,
            }))?;
        if pipeline.state != PipelineState::Planned {
            return Err(OpenSessionError::NotPlanned(PipelineNotReady {
                pipeline_id,
                required: PipelineState::Planned,
            }));
        }
        if pipeline.mode != input.mode {
            return Err(OpenSessionError::ModeMismatch(ModeMismatch {
                pipeline_id,
                mode: input.mode,
            }));
        }
        let session_id = SessionId::from(self.issue());
        self.sessions.push(Session {
            session_id,
            pipeline_id,
            mode: input.mode,
            run_id: None,
            state: SessionState::Opened,
        });
        let event = SessionOpened {
            session_id,
            pipeline_id,
        };
        self.events.push(SessionEvent::SessionOpened(event));
        Ok(event)
    }

    /// Runs `inference.session.AttachRun`: the run becomes the session's current run, replacing
    /// any other whatever its state, and the session is Opened.
    ///
    /// The run's pipeline is read from `graph`; a run `graph` holds no record of has no pipeline
    /// to compare, and is attached.
    ///
    /// # Errors
    ///
    /// In this order: [`AttachRunError::NoSuchSession`], [`AttachRunError::PipelineMismatch`] when
    /// `graph` holds the run and it executes another pipeline, [`AttachRunError::WrongState`] when
    /// the session is Draining or Closed. Nothing moves and no event is recorded.
    pub fn attach_run(
        &mut self,
        graph: &(impl Graph + ?Sized),
        input: AttachRun,
    ) -> Result<RunAttached, AttachRunError> {
        let AttachRun { session_id, run_id } = input;
        let session =
            self.slot(session_id)
                .ok_or(AttachRunError::NoSuchSession(SessionNotFound {
                    session_id,
                }))?;
        if graph
            .run_pipeline(run_id)
            .is_some_and(|pipeline| pipeline != session.pipeline_id)
        {
            return Err(AttachRunError::PipelineMismatch(PipelineMismatch {
                session_id,
                run_id,
            }));
        }
        if !matches!(session.state, SessionState::Opened | SessionState::Ready) {
            return Err(AttachRunError::WrongState(SessionStateConflict {
                current: session.state,
            }));
        }
        session.run_id = Some(run_id);
        session.state = SessionState::Opened;
        let event = RunAttached { session_id, run_id };
        self.events.push(SessionEvent::RunAttached(event));
        Ok(event)
    }

    /// Runs `inference.session.MarkSessionReady`: Opened to Ready, for a session with a current
    /// run.
    ///
    /// # Errors
    ///
    /// In this order: [`MarkSessionReadyError::NoSuchSession`],
    /// [`MarkSessionReadyError::WrongState`] when the session is not Opened,
    /// [`MarkSessionReadyError::NoCurrentRun`] when it is Opened and references no run. Nothing
    /// moves and no event is recorded.
    pub fn mark_session_ready(
        &mut self,
        session_id: SessionId,
    ) -> Result<SessionReady, MarkSessionReadyError> {
        let session = self
            .slot(session_id)
            .ok_or(MarkSessionReadyError::NoSuchSession(SessionNotFound {
                session_id,
            }))?;
        if session.state != SessionState::Opened {
            return Err(MarkSessionReadyError::WrongState(SessionStateConflict {
                current: session.state,
            }));
        }
        if session.run_id.is_none() {
            return Err(MarkSessionReadyError::NoCurrentRun(NoCurrentRun {
                session_id,
            }));
        }
        session.state = SessionState::Ready;
        let event = SessionReady { session_id };
        self.events.push(SessionEvent::SessionReady(event));
        Ok(event)
    }

    /// Runs `inference.session.CloseSession`: Opened or Ready to Draining.
    ///
    /// # Errors
    ///
    /// [`SessionMoveError::NoSuchSession`], then [`SessionMoveError::WrongState`] when the session
    /// is Draining or Closed. Nothing moves and no event is recorded.
    pub fn close_session(
        &mut self,
        session_id: SessionId,
    ) -> Result<SessionDraining, SessionMoveError> {
        self.move_session(
            session_id,
            &[SessionState::Opened, SessionState::Ready],
            SessionState::Draining,
        )?;
        let event = SessionDraining { session_id };
        self.events.push(SessionEvent::SessionDraining(event));
        Ok(event)
    }

    /// Runs `inference.session.SendInput`: the `send` move from Ready to Ready, which admits
    /// input to the session's current run and moves nothing else.
    ///
    /// # Errors
    ///
    /// [`SessionMoveError::NoSuchSession`], then [`SessionMoveError::WrongState`] when the session
    /// is not Ready. Nothing moves and no event is recorded.
    pub fn send_input(&mut self, session_id: SessionId) -> Result<InputSent, SessionMoveError> {
        self.move_session(session_id, &[SessionState::Ready], SessionState::Ready)?;
        let event = InputSent { session_id };
        self.events.push(SessionEvent::InputSent(event));
        Ok(event)
    }

    /// Runs `inference.session.CommitInput`: the `commit` move from Ready to Ready, which admits a
    /// commit of the input to the session's current run and moves nothing else.
    ///
    /// # Errors
    ///
    /// [`SessionMoveError::NoSuchSession`], then [`SessionMoveError::WrongState`] when the session
    /// is not Ready. Nothing moves and no event is recorded.
    pub fn commit_input(
        &mut self,
        session_id: SessionId,
    ) -> Result<InputCommitted, SessionMoveError> {
        self.move_session(session_id, &[SessionState::Ready], SessionState::Ready)?;
        let event = InputCommitted { session_id };
        self.events.push(SessionEvent::InputCommitted(event));
        Ok(event)
    }

    /// Runs `inference.session.SwitchPipeline` against the pipelines `graph` holds: the `switch`
    /// move from Ready to Opened, over the other pipeline and mode, with no current run, so that
    /// readiness is asserted again for the next run. History stays with the session.
    ///
    /// The pipeline's refusals are the specification's external outcomes, which ESS synthesizes
    /// its accepting scenario around with an identity no pipeline carries; so, as `AttachRun`
    /// attaches a run `graph` holds no record of, a pipeline `graph` holds no record of is not
    /// refused here, and the refusals are taken for a pipeline it holds.
    /// [`crate::live::LiveSession::switch`] refuses an unknown pipeline itself, before anything
    /// moves.
    ///
    /// # Errors
    ///
    /// In this order: [`SwitchPipelineError::NoSuchSession`], then, for a pipeline `graph` holds,
    /// [`SwitchPipelineError::NotPlanned`] requiring Planned and
    /// [`SwitchPipelineError::ModeMismatch`], which the specification declares external and so
    /// answer before the held state, then [`SwitchPipelineError::WrongState`] when the session is
    /// not Ready. Nothing moves and no event is recorded.
    pub fn switch_pipeline(
        &mut self,
        graph: &(impl Graph + ?Sized),
        input: SwitchPipeline,
    ) -> Result<PipelineSwitched, SwitchPipelineError> {
        let SwitchPipeline {
            session_id,
            pipeline_id,
            mode,
        } = input;
        let state =
            self.get(session_id)
                .map(Session::state)
                .ok_or(SwitchPipelineError::NoSuchSession(SessionNotFound {
                    session_id,
                }))?;
        if let Some(pipeline) = graph.pipeline(pipeline_id) {
            if pipeline.state != PipelineState::Planned {
                return Err(SwitchPipelineError::NotPlanned(PipelineNotReady {
                    pipeline_id,
                    required: PipelineState::Planned,
                }));
            }
            if pipeline.mode != mode {
                return Err(SwitchPipelineError::ModeMismatch(ModeMismatch {
                    pipeline_id,
                    mode,
                }));
            }
        }
        if state != SessionState::Ready {
            return Err(SwitchPipelineError::WrongState(SessionStateConflict {
                current: state,
            }));
        }
        if let Some(session) = self.slot(session_id) {
            session.pipeline_id = pipeline_id;
            session.mode = mode;
            session.run_id = None;
            session.state = SessionState::Opened;
        }
        let event = PipelineSwitched {
            session_id,
            pipeline_id,
        };
        self.events.push(SessionEvent::PipelineSwitched(event));
        Ok(event)
    }

    /// Runs `inference.session.Interrupt`: the `interrupt` move from Ready to Ready, which admits
    /// the cut of the active generation and moves nothing else.
    ///
    /// # Errors
    ///
    /// [`SessionMoveError::NoSuchSession`], then [`SessionMoveError::WrongState`] when the session
    /// is not Ready. Nothing moves and no event is recorded.
    pub fn interrupt(
        &mut self,
        session_id: SessionId,
    ) -> Result<SessionInterrupted, SessionMoveError> {
        self.move_session(session_id, &[SessionState::Ready], SessionState::Ready)?;
        let event = SessionInterrupted { session_id };
        self.events.push(SessionEvent::SessionInterrupted(event));
        Ok(event)
    }

    /// Runs `inference.session.CancelSession`, the cancel branch of close: Opened, Ready or
    /// Draining to Closed, with the [`SessionClosed`] the drain's close emits.
    ///
    /// # Errors
    ///
    /// [`SessionMoveError::NoSuchSession`], then [`SessionMoveError::WrongState`] when the session
    /// is Closed. Nothing moves and no event is recorded.
    pub fn cancel_session(
        &mut self,
        session_id: SessionId,
    ) -> Result<SessionClosed, SessionMoveError> {
        self.move_session(
            session_id,
            &[
                SessionState::Opened,
                SessionState::Ready,
                SessionState::Draining,
            ],
            SessionState::Closed,
        )?;
        let event = SessionClosed { session_id };
        self.events.push(SessionEvent::SessionClosed(event));
        Ok(event)
    }

    /// Runs `inference.session.CompleteSessionClose`: Draining to Closed.
    ///
    /// # Errors
    ///
    /// [`SessionMoveError::NoSuchSession`], then [`SessionMoveError::WrongState`] when the session
    /// is not Draining. Nothing moves and no event is recorded.
    pub fn complete_session_close(
        &mut self,
        session_id: SessionId,
    ) -> Result<SessionClosed, SessionMoveError> {
        self.move_session(session_id, &[SessionState::Draining], SessionState::Closed)?;
        let event = SessionClosed { session_id };
        self.events.push(SessionEvent::SessionClosed(event));
        Ok(event)
    }

    /// Moves the session `session_id` names to `to` if it holds one of `from`.
    fn move_session(
        &mut self,
        session_id: SessionId,
        from: &[SessionState],
        to: SessionState,
    ) -> Result<(), SessionMoveError> {
        let session = self
            .slot(session_id)
            .ok_or(SessionMoveError::NoSuchSession(SessionNotFound {
                session_id,
            }))?;
        if !from.contains(&session.state) {
            return Err(SessionMoveError::WrongState(SessionStateConflict {
                current: session.state,
            }));
        }
        session.state = to;
        Ok(())
    }

    fn slot(&mut self, session_id: SessionId) -> Option<&mut Session> {
        self.sessions
            .iter_mut()
            .find(|session| session.session_id == session_id)
    }

    /// The next identity; only a command that creates takes one.
    fn issue(&mut self) -> Uuid {
        match &mut self.identities {
            Identities::Generated => Uuid::new_v4(),
            Identities::Counted { space, issued } => {
                *issued += 1;
                Uuid::from_u128(
                    (u128::from(*space) << 96)
                        | 0x0000_0000_0000_4000_8000_0000_0000_0000
                        | *issued,
                )
            }
        }
    }

    /// The session carrying `session_id`, if one was opened.
    #[must_use]
    pub fn get(&self, session_id: SessionId) -> Option<&Session> {
        self.sessions
            .iter()
            .find(|session| session.session_id == session_id)
    }

    /// Every session, in the order they were opened, in every state.
    pub fn sessions(&self) -> impl Iterator<Item = &Session> {
        self.sessions.iter()
    }

    /// Every event emitted, in the order it was emitted.
    #[must_use]
    pub fn events(&self) -> &[SessionEvent] {
        &self.events
    }

    /// The view `inference.session.Sessions`: one row per session, in the order they were opened.
    #[must_use]
    pub fn sessions_view(&self) -> Vec<SessionsRow> {
        self.sessions
            .iter()
            .map(|session| SessionsRow {
                session_id: session.session_id,
                pipeline_id: session.pipeline_id,
                mode: session.mode,
                run_id: session.run_id,
                state: session.state,
            })
            .collect()
    }

    /// Runs `inference.session.RecordHistory`: the fact is the next entry of the session's
    /// history, under a new identity, and its [`HistoryRecorded`]. A session records in every
    /// state but Closed.
    ///
    /// # Errors
    ///
    /// [`RecordHistoryError::NoSuchSession`], then [`RecordHistoryError::Closed`] when the
    /// session is Closed. Nothing is stored, no event is recorded and no identity is issued.
    pub fn record_history(
        &mut self,
        input: RecordHistory,
    ) -> Result<HistoryRecorded, RecordHistoryError> {
        let session_id = input.session_id;
        let session = self
            .get(session_id)
            .ok_or(RecordHistoryError::NoSuchSession(SessionNotFound {
                session_id,
            }))?;
        if session.state == SessionState::Closed {
            return Err(RecordHistoryError::Closed(SessionStateConflict {
                current: SessionState::Closed,
            }));
        }
        let entry_id = HistoryEntryId::from(self.issue());
        let event = HistoryRecorded {
            entry_id,
            session_id,
            run_id: input.run_id,
            position: input.position,
            kind: input.kind,
        };
        self.history.push(HistoryEntry {
            entry_id,
            session_id,
            run_id: input.run_id,
            position: input.position,
            kind: input.kind,
            text: input.text,
            call_id: input.call_id,
        });
        self.events.push(SessionEvent::HistoryRecorded(event));
        Ok(event)
    }

    /// The position the next entry of the session `session_id` takes: one past its last, 1 for
    /// a session with no history.
    #[must_use]
    pub fn next_position(&self, session_id: SessionId) -> i64 {
        let recorded = self
            .history
            .iter()
            .filter(|entry| entry.session_id == session_id)
            .count();
        i64::try_from(recorded).map_or(i64::MAX, |recorded| recorded + 1)
    }

    /// The history of the session `session_id`, in the order it was recorded.
    pub fn history(&self, session_id: SessionId) -> impl Iterator<Item = &HistoryEntry> {
        self.history
            .iter()
            .filter(move |entry| entry.session_id == session_id)
    }

    /// The view `inference.session.History`: every entry of every session, in the order it was
    /// recorded.
    #[must_use]
    pub fn history_view(&self) -> &[HistoryEntry] {
        &self.history
    }
}
