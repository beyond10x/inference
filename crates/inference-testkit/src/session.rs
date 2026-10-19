//! The `inference.session` domain of `ess/domains/session.yaml`, held in memory.
//!
//! [`SessionDomain`] holds the sessions, each with the identity of its current run, zero or one,
//! and records every event it emits in order. No contract crate declares a session yet, so the
//! nouns of the domain are declared here, in its terms: [`Session`], [`SessionState`], the inputs,
//! the events and the refusals. Identities are issued from a counter rendered as a Uuid in the
//! domain's own [`IDENTITY_SPACE`], `00000005-0000-4000-8000-000000000001` first, so that two runs
//! of the same commands report the same identities and no other domain issues one of them; a
//! refused command takes none.
//!
//! What a session reads of another domain it reads from the `inference.graph` store,
//! [`GraphDomain`], at the call: `OpenSession` the pipeline's existence, state and mode, and
//! `AttachRun`, behind the `run` feature, the pipeline of the run it attaches. A command refuses,
//! stores nothing and records no event in the order the specification answers it:
//!
//! * `OpenSession`: a pipeline no record carries, one that is not Planned, one of another mode.
//! * `AttachRun`: a session no record carries, a run whose pipeline is not the session's, a
//!   session that is Draining or Closed. The pipeline comparison is the specification's external
//!   refusal and answers before the held state, as ESS orders an external branch that moves
//!   nothing; a run no record of the graph store carries is not refused, since the specification
//!   declares no refusal of it, and is attached by its identity.
//! * `MarkSessionReady`: a session no record carries, an Opened session with no current run, a
//!   session that is not Opened.
//! * `CloseSession` and `CompleteSessionClose`: a session no record carries, a session not in a
//!   state the move starts from.
//!
//! Attaching moves the session to Opened from Opened or Ready, so a session that takes another
//! run is ready again only once `MarkSessionReady` says so. Two rules the specification leaves
//! `UNMAPPED` are not enforced, as it declares them: attaching the run a session already holds
//! moves it to Opened and records `RunAttached` again, and one run may be current in several
//! sessions. The view `inference.session.Sessions` is [`SessionDomain::sessions_view`], read at
//! the call.

use std::error::Error;
use std::fmt;

use uuid::Uuid;

use inference_core::id::{RunId, SessionId};
use inference_pipeline::{
    InteractionMode, PipelineId, PipelineNotFound, PipelineNotReady, PipelineState,
};

use crate::graph::GraphDomain;

/// The first eight hex digits of every identity the domain issues, which no other domain of the
/// testkit issues under (see [`crate::policy::IDENTITY_SPACE`]; `inference.tools` counts in 4).
pub const IDENTITY_SPACE: u32 = 5;

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

/// An event the domain emitted, in the order it emitted them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEvent {
    SessionOpened(SessionOpened),
    RunAttached(RunAttached),
    SessionReady(SessionReady),
    SessionDraining(SessionDraining),
    SessionClosed(SessionClosed),
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

/// Why `inference.session.OpenSession` opened nothing, in the order
/// [`SessionDomain::open_session`] decides it.
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

/// Why `inference.session.AttachRun` attached nothing, in the order
/// [`SessionDomain::attach_run`] decides it.
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
/// [`SessionDomain::mark_session_ready`] decides it.
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

/// `inference.session` in memory: the sessions, in the order they were opened, and the events the
/// domain emitted. A session stays in every state, Closed included.
#[derive(Debug, Clone, Default)]
pub struct SessionDomain {
    sessions: Vec<Session>,
    events: Vec<SessionEvent>,
    issued: u128,
}

impl SessionDomain {
    /// An empty domain.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs `inference.session.OpenSession` against the pipelines `graph` declares: an Opened
    /// session over the pipeline and mode, with no current run, and its [`SessionOpened`].
    ///
    /// # Errors
    ///
    /// In this order: [`OpenSessionError::UnknownPipeline`], [`OpenSessionError::NotPlanned`]
    /// requiring Planned, [`OpenSessionError::ModeMismatch`]. Nothing is stored, no event is
    /// recorded and no identity is issued.
    pub fn open_session(
        &mut self,
        graph: &GraphDomain,
        input: OpenSession,
    ) -> Result<SessionOpened, OpenSessionError> {
        let pipeline_id = input.pipeline_id;
        let pipeline =
            graph
                .declaration()
                .pipeline(pipeline_id)
                .ok_or(OpenSessionError::UnknownPipeline(PipelineNotFound {
                    pipeline_id,
                }))?;
        if pipeline.state() != PipelineState::Planned {
            return Err(OpenSessionError::NotPlanned(PipelineNotReady {
                pipeline_id,
                required: PipelineState::Planned,
            }));
        }
        if pipeline.mode() != input.mode {
            return Err(OpenSessionError::ModeMismatch(ModeMismatch {
                pipeline_id,
                mode: input.mode,
            }));
        }
        self.issued += 1;
        let session_id = SessionId::from(identity(self.issued));
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
    /// The run's pipeline is read from `graph`, which holds runs behind the `run` feature only; a
    /// run `graph` holds no record of has no pipeline to compare, and is attached.
    ///
    /// # Errors
    ///
    /// In this order: [`AttachRunError::NoSuchSession`], [`AttachRunError::PipelineMismatch`] when
    /// `graph` holds the run and it executes another pipeline, [`AttachRunError::WrongState`] when
    /// the session is Draining or Closed. Nothing moves and no event is recorded.
    pub fn attach_run(
        &mut self,
        graph: &GraphDomain,
        input: AttachRun,
    ) -> Result<RunAttached, AttachRunError> {
        let AttachRun { session_id, run_id } = input;
        let session =
            self.slot(session_id)
                .ok_or(AttachRunError::NoSuchSession(SessionNotFound {
                    session_id,
                }))?;
        if run_pipeline(graph, run_id).is_some_and(|pipeline| pipeline != session.pipeline_id) {
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
    /// [`MarkSessionReadyError::NoCurrentRun`] when the session is Opened and references no run,
    /// [`MarkSessionReadyError::WrongState`] when it is not Opened. Nothing moves and no event is
    /// recorded.
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
}

/// The pipeline of the run `graph` holds under `run_id`, if it holds one.
#[cfg(feature = "run")]
fn run_pipeline(graph: &GraphDomain, run_id: RunId) -> Option<PipelineId> {
    graph
        .run(run_id)
        .map(inference_runtime::run::Run::pipeline_id)
}

/// None: without the `run` feature the graph store holds no run.
#[cfg(not(feature = "run"))]
const fn run_pipeline(_graph: &GraphDomain, _run_id: RunId) -> Option<PipelineId> {
    None
}

/// The `count`th identity: a version 4, RFC 4122 variant Uuid whose low bits are `count` and whose
/// first eight hex digits are [`IDENTITY_SPACE`], `00000005-0000-4000-8000-000000000001` first.
fn identity(count: u128) -> Uuid {
    Uuid::from_u128(
        (u128::from(IDENTITY_SPACE) << 96) | 0x0000_0000_0000_4000_8000_0000_0000_0000 | count,
    )
}

#[cfg(test)]
mod tests {
    use inference_core::id::{RunId, SessionId};
    use inference_core::model::Locality;
    use inference_core::policy::{DeclareProfile, Egress, ProfileKind};
    use inference_pipeline::{
        DeclarePipeline, InteractionMode, PipelineId, PipelineNotFound, PipelineNotReady,
        PipelineState, PlanPipeline, ValidatePipeline,
    };
    use uuid::Uuid;

    use super::{
        AttachRun, AttachRunError, MarkSessionReadyError, ModeMismatch, NoCurrentRun, OpenSession,
        OpenSessionError, SessionDomain, SessionEvent, SessionMoveError, SessionNotFound,
        SessionState, SessionStateConflict,
    };
    use crate::graph::{GraphCatalog, GraphDomain};
    use crate::model::ModelDomain;
    use crate::policy::PolicyDomain;

    /// A graph store with a Planned batch pipeline and a Declared one.
    fn graph() -> (GraphDomain, PipelineId, PipelineId) {
        let (graph, planned, _, declared) = graphs();
        (graph, planned, declared)
    }

    /// A graph store with two Planned batch pipelines and a Declared one.
    fn graphs() -> (GraphDomain, PipelineId, PipelineId, PipelineId) {
        let mut policy = PolicyDomain::new();
        let profile_id = policy
            .declare_profile(DeclareProfile {
                kind: ProfileKind::OfflineStrict,
                egress: Egress::Denied,
                admitted: vec![Locality::SameProcess],
            })
            .expect("a row of the table")
            .profile_id;
        let model = ModelDomain::new();
        let catalog = GraphCatalog {
            policy: &policy,
            model: &model,
        };
        let mut graph = GraphDomain::new();
        let declare = |graph: &mut GraphDomain, name: &str| {
            graph
                .declare_pipeline(
                    DeclarePipeline {
                        name: name.to_owned(),
                        schema_revision: "1".to_owned(),
                        mode: InteractionMode::Batch,
                        profile_id,
                        required_capabilities: Vec::new(),
                    },
                    &catalog,
                )
                .expect("the profile is declared")
                .pipeline_id
        };
        let plan = |graph: &mut GraphDomain, pipeline_id| {
            graph
                .validate_pipeline(ValidatePipeline { pipeline_id })
                .expect("an empty pipeline validates");
            graph
                .plan_pipeline(PlanPipeline { pipeline_id }, &catalog)
                .expect("an empty pipeline plans");
        };
        let planned = declare(&mut graph, "planned");
        let second = declare(&mut graph, "second");
        let declared = declare(&mut graph, "declared");
        plan(&mut graph, planned);
        plan(&mut graph, second);
        (graph, planned, second, declared)
    }

    fn open(domain: &mut SessionDomain, graph: &GraphDomain, pipeline_id: PipelineId) -> SessionId {
        domain
            .open_session(
                graph,
                OpenSession {
                    pipeline_id,
                    mode: InteractionMode::Batch,
                },
            )
            .expect("a Planned pipeline of the mode opens a session")
            .session_id
    }

    #[test]
    fn identities_count_in_the_session_space_and_a_refusal_takes_none() {
        let (graph, planned, declared) = graph();
        let mut domain = SessionDomain::new();
        let unknown = PipelineId::from(Uuid::from_u128(9));
        let open = |pipeline_id, mode| OpenSession { pipeline_id, mode };

        assert_eq!(
            domain.open_session(&graph, open(unknown, InteractionMode::Batch)),
            Err(OpenSessionError::UnknownPipeline(PipelineNotFound {
                pipeline_id: unknown
            }))
        );
        assert_eq!(
            domain.open_session(&graph, open(declared, InteractionMode::Duplex)),
            Err(OpenSessionError::NotPlanned(PipelineNotReady {
                pipeline_id: declared,
                required: PipelineState::Planned,
            })),
            "a pipeline that is not Planned is refused before its mode is read"
        );
        assert_eq!(
            domain.open_session(&graph, open(planned, InteractionMode::Duplex)),
            Err(OpenSessionError::ModeMismatch(ModeMismatch {
                pipeline_id: planned,
                mode: InteractionMode::Duplex,
            }))
        );
        assert!(domain.events().is_empty(), "a refusal records no event");

        let session_id = domain
            .open_session(&graph, open(planned, InteractionMode::Batch))
            .expect("opens")
            .session_id;
        assert_eq!(
            session_id.to_string(),
            "00000005-0000-4000-8000-000000000001",
            "the refusals took no identity"
        );
        let session = domain.get(session_id).expect("stored");
        assert_eq!(session.state(), SessionState::Opened);
        assert_eq!(session.run_id(), None);
        assert_eq!(session.pipeline_id(), planned);
    }

    #[test]
    fn readiness_needs_a_current_run_and_the_lifecycle_runs_through_the_store() {
        let (graph, planned, _) = graph();
        let mut domain = SessionDomain::new();
        let session_id = open(&mut domain, &graph, planned);

        assert_eq!(
            domain.mark_session_ready(session_id),
            Err(MarkSessionReadyError::NoCurrentRun(NoCurrentRun {
                session_id
            }))
        );
        assert_eq!(
            domain.complete_session_close(session_id),
            Err(SessionMoveError::WrongState(SessionStateConflict {
                current: SessionState::Opened,
            }))
        );
        assert_eq!(domain.events().len(), 1);

        let run_id = RunId::from(Uuid::from_u128(7));
        domain
            .attach_run(&graph, AttachRun { session_id, run_id })
            .expect("a run the graph store holds no record of has no pipeline to compare");
        assert_eq!(
            domain.get(session_id).and_then(|s| s.run_id()),
            Some(run_id)
        );
        domain.mark_session_ready(session_id).expect("ready");
        assert_eq!(
            domain.mark_session_ready(session_id),
            Err(MarkSessionReadyError::WrongState(SessionStateConflict {
                current: SessionState::Ready,
            }))
        );

        let other = RunId::from(Uuid::from_u128(8));
        domain
            .attach_run(
                &graph,
                AttachRun {
                    session_id,
                    run_id: other,
                },
            )
            .expect("a Ready session takes another run");
        let session = domain.get(session_id).expect("stored");
        assert_eq!(session.run_id(), Some(other));
        assert_eq!(
            session.state(),
            SessionState::Opened,
            "readiness is asserted again for the new run"
        );

        domain.close_session(session_id).expect("drains");
        assert_eq!(
            domain.attach_run(&graph, AttachRun { session_id, run_id }),
            Err(AttachRunError::WrongState(SessionStateConflict {
                current: SessionState::Draining,
            }))
        );
        assert_eq!(
            domain.mark_session_ready(session_id),
            Err(MarkSessionReadyError::WrongState(SessionStateConflict {
                current: SessionState::Draining,
            })),
            "a draining session is not Opened, whatever run it holds"
        );
        domain.complete_session_close(session_id).expect("closes");
        assert_eq!(
            domain.close_session(session_id),
            Err(SessionMoveError::WrongState(SessionStateConflict {
                current: SessionState::Closed,
            }))
        );
        assert!(matches!(
            domain.events(),
            [
                SessionEvent::SessionOpened(_),
                SessionEvent::RunAttached(_),
                SessionEvent::SessionReady(_),
                SessionEvent::RunAttached(_),
                SessionEvent::SessionDraining(_),
                SessionEvent::SessionClosed(_),
            ]
        ));
        let row = domain.sessions_view()[0];
        assert_eq!(row.state, SessionState::Closed);
        assert_eq!(row.run_id, Some(other));
    }

    #[test]
    fn the_two_unmapped_attach_rules_are_not_enforced_as_the_specification_declares() {
        let (graph, planned, _) = graph();
        let mut domain = SessionDomain::new();
        let session_id = open(&mut domain, &graph, planned);
        let other = open(&mut domain, &graph, planned);
        let run_id = RunId::from(Uuid::from_u128(7));
        let attach = |session_id| AttachRun { session_id, run_id };

        domain
            .attach_run(&graph, attach(session_id))
            .expect("attached");
        domain.mark_session_ready(session_id).expect("ready");
        domain
            .attach_run(&graph, attach(session_id))
            .expect("a retried attach of the run the session holds is accepted");
        assert_eq!(
            domain.get(session_id).map(super::Session::state),
            Some(SessionState::Opened),
            "a retried attach moves a Ready session to Opened (session.yaml UNMAPPED: retried attach)"
        );
        assert_eq!(
            domain
                .events()
                .iter()
                .filter(|event| matches!(event, SessionEvent::RunAttached(_)))
                .count(),
            2,
            "and records RunAttached again"
        );

        domain
            .attach_run(&graph, attach(other))
            .expect("one run current in two sessions (session.yaml UNMAPPED: at most one session)");
        assert_eq!(domain.get(other).and_then(|s| s.run_id()), Some(run_id));
        assert_eq!(
            domain.get(session_id).and_then(|s| s.run_id()),
            Some(run_id)
        );
    }

    #[test]
    fn a_command_naming_no_session_is_refused_and_records_nothing() {
        let (graph, _, _) = graph();
        let mut domain = SessionDomain::new();
        let session_id = SessionId::from(Uuid::from_u128(9));
        let not_found = SessionNotFound { session_id };
        assert_eq!(
            domain.attach_run(
                &graph,
                AttachRun {
                    session_id,
                    run_id: RunId::from(Uuid::from_u128(7)),
                }
            ),
            Err(AttachRunError::NoSuchSession(not_found))
        );
        assert_eq!(
            domain.mark_session_ready(session_id),
            Err(MarkSessionReadyError::NoSuchSession(not_found))
        );
        assert_eq!(
            domain.close_session(session_id),
            Err(SessionMoveError::NoSuchSession(not_found))
        );
        assert_eq!(
            domain.complete_session_close(session_id),
            Err(SessionMoveError::NoSuchSession(not_found))
        );
        assert!(domain.events().is_empty());
    }

    #[cfg(feature = "run")]
    #[test]
    fn a_run_of_another_pipeline_is_refused_before_the_held_state() {
        use super::PipelineMismatch;

        let (mut graph, planned, second, _) = graphs();
        let ours = graph.create_run(planned).expect("planned").run_id;
        let theirs = graph.create_run(second).expect("planned").run_id;
        let mut domain = SessionDomain::new();
        let session_id = open(&mut domain, &graph, planned);
        let bystander = open(&mut domain, &graph, planned);
        let mismatch = AttachRunError::PipelineMismatch(PipelineMismatch {
            session_id,
            run_id: theirs,
        });

        assert_eq!(
            domain.attach_run(
                &graph,
                AttachRun {
                    session_id,
                    run_id: theirs,
                }
            ),
            Err(mismatch)
        );
        assert_eq!(domain.get(session_id).and_then(|s| s.run_id()), None);
        domain
            .attach_run(
                &graph,
                AttachRun {
                    session_id,
                    run_id: ours,
                },
            )
            .expect("a run of the session's pipeline is attached");
        assert_eq!(
            domain.get(bystander).and_then(|s| s.run_id()),
            None,
            "attaching to one session leaves another as it was"
        );

        domain.close_session(session_id).expect("drains");
        let recorded = domain.events().len();
        assert_eq!(
            domain.attach_run(
                &graph,
                AttachRun {
                    session_id,
                    run_id: theirs,
                }
            ),
            Err(mismatch),
            "the external refusal moves nothing, so it answers before the held state"
        );
        assert_eq!(
            domain.attach_run(
                &graph,
                AttachRun {
                    session_id,
                    run_id: ours,
                }
            ),
            Err(AttachRunError::WrongState(SessionStateConflict {
                current: SessionState::Draining,
            }))
        );
        assert_eq!(domain.events().len(), recorded);
        assert_eq!(domain.get(session_id).and_then(|s| s.run_id()), Some(ours));
    }
}
