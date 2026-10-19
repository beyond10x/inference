//! The `inference.session` domain of `ess/domains/session.yaml`, held in memory.
//!
//! The session state machine is `inference-facade`'s ([`Sessions`]); this module holds the store
//! the conformance target answers with, [`SessionDomain`], which runs every command through one
//! [`Sessions`] and carries no state machine of its own. The nouns of the domain are the facade's,
//! re-exported here: [`Session`], [`SessionState`], the inputs, the events and the refusals.
//! Identities are counted in the domain's own [`IDENTITY_SPACE`],
//! `00000005-0000-4000-8000-000000000001` first ([`Sessions::counted_in`]), so that two runs of
//! the same commands report the same identities and no other domain issues one of them; a refused
//! command takes none.
//!
//! What a session reads of another domain it reads from the `inference.graph` store,
//! [`GraphDomain`], through the facade's [`Graph`], which this module implements over it: a
//! pipeline's existence, state and mode from the declaration, and a run's pipeline from the runs
//! the store holds. The order in which each command refuses is the facade's
//! ([`inference_facade::session`]). The view `inference.session.Sessions` is
//! [`SessionDomain::sessions_view`], read at the call.
//!
//! The text pipeline's payload, [`PipelinePayload`], is a [`SessionPayload`]: a live session of
//! `inference-facade` over a text pipeline writes its control events as
//! [`PipelinePayload::Control`] and delivers each `inference.stream` event the session event
//! vocabulary carries as its [`SessionEvent`] variant; a control event, a speech, turn or queued
//! or discarded segment event has no variant and is not delivered.
//!
//! The module needs the `run` feature, which brings `inference-facade`: a crate that leaves `run`
//! off carries neither the runtime nor the session API.

#![cfg(feature = "run")]

use inference_core::envelope::ControlKind;
use inference_core::id::{RunId, SessionId};
pub use inference_facade::events::SessionPayload;
use inference_facade::session::GraphPipeline;
pub use inference_facade::session::{
    AttachRun, AttachRunError, Graph, HistoryEntry, HistoryEntryId, HistoryKind, HistoryRecorded,
    InputCommitted, InputSent, MarkSessionReadyError, ModeMismatch, NoCurrentRun, OpenSession,
    OpenSessionError, PipelineMismatch, PipelineSwitched, RecordHistory, RecordHistoryError,
    RunAttached, Session, SessionClosed, SessionDraining, SessionEvent, SessionInterrupted,
    SessionMoveError, SessionNotFound, SessionOpened, SessionReady, SessionState,
    SessionStateConflict, Sessions, SessionsRow, SwitchPipeline, SwitchPipelineError,
};
use inference_pipeline::PipelineId;

use crate::graph::{GraphDomain, PipelinePayload};
use crate::stream::StreamEvent;

impl SessionPayload for PipelinePayload {
    fn control(kind: ControlKind) -> Self {
        Self::Control(kind)
    }

    fn session_event(&self) -> Option<SessionEvent> {
        match self {
            Self::Stream(event) => stream_session_event(event),
            Self::Control(_) => None,
        }
    }
}

/// The session event an `inference.stream` event is delivered as: every event of the session
/// vocabulary the stream domain declares; none for a speech, turn, queued or discarded segment
/// event, which the vocabulary does not carry.
pub(crate) fn stream_session_event(event: &StreamEvent) -> Option<SessionEvent> {
    Some(match event.clone() {
        StreamEvent::TranscriptHypothesis(event) => SessionEvent::TranscriptHypothesis(event),
        StreamEvent::TranscriptRevision(event) => SessionEvent::TranscriptRevision(event),
        StreamEvent::TranscriptCommitted(event) => SessionEvent::TranscriptCommitted(event),
        StreamEvent::GenerationStarted(event) => SessionEvent::GenerationStarted(event),
        StreamEvent::GenerationCompleted(event) => SessionEvent::GenerationCompleted(event),
        StreamEvent::GenerationSuperseded(event) => SessionEvent::GenerationSuperseded(event),
        StreamEvent::GenerationCancelled(event) => SessionEvent::GenerationCancelled(event),
        StreamEvent::TextDelta(event) => SessionEvent::TextDelta(event),
        StreamEvent::TextSegmentCommitted(event) => SessionEvent::TextSegmentCommitted(event),
        StreamEvent::SynthesisCompleted(event) => SessionEvent::SynthesisCompleted(event),
        StreamEvent::AudioPlayed(event) => SessionEvent::AudioPlayed(event),
        StreamEvent::SpeechStarted(_)
        | StreamEvent::SpeechEnded(_)
        | StreamEvent::TurnOpened(_)
        | StreamEvent::TurnCompleted(_)
        | StreamEvent::SegmentQueued(_)
        | StreamEvent::SegmentDiscarded(_) => return None,
    })
}

/// The first eight hex digits of every identity the domain issues, which no other domain of the
/// testkit issues under (see [`crate::policy::IDENTITY_SPACE`]; `inference.tools` counts in 4).
pub const IDENTITY_SPACE: u32 = 5;

impl Graph for GraphDomain {
    fn pipeline(&self, pipeline_id: PipelineId) -> Option<GraphPipeline> {
        self.declaration()
            .pipeline(pipeline_id)
            .map(|pipeline| GraphPipeline {
                state: pipeline.state(),
                mode: pipeline.mode(),
            })
    }

    fn run_pipeline(&self, run_id: RunId) -> Option<PipelineId> {
        self.run(run_id)
            .map(inference_runtime::run::Run::pipeline_id)
    }
}

/// `inference.session` in memory: the facade's [`Sessions`], counting in [`IDENTITY_SPACE`], over
/// the `inference.graph` store each command reads.
#[derive(Debug, Clone)]
pub struct SessionDomain {
    sessions: Sessions,
}

impl Default for SessionDomain {
    fn default() -> Self {
        Self {
            sessions: Sessions::counted_in(IDENTITY_SPACE),
        }
    }
}

impl SessionDomain {
    /// An empty domain.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs `inference.session.OpenSession` ([`Sessions::open_session`]) against the pipelines
    /// `graph` declares.
    ///
    /// # Errors
    ///
    /// The refusals of [`Sessions::open_session`], in its order.
    pub fn open_session(
        &mut self,
        graph: &GraphDomain,
        input: OpenSession,
    ) -> Result<SessionOpened, OpenSessionError> {
        self.sessions.open_session(graph, input)
    }

    /// Runs `inference.session.AttachRun` ([`Sessions::attach_run`]), reading the run's pipeline
    /// from the runs `graph` holds.
    ///
    /// # Errors
    ///
    /// The refusals of [`Sessions::attach_run`], in its order.
    pub fn attach_run(
        &mut self,
        graph: &GraphDomain,
        input: AttachRun,
    ) -> Result<RunAttached, AttachRunError> {
        self.sessions.attach_run(graph, input)
    }

    /// Runs `inference.session.MarkSessionReady` ([`Sessions::mark_session_ready`]).
    ///
    /// # Errors
    ///
    /// The refusals of [`Sessions::mark_session_ready`], in its order.
    pub fn mark_session_ready(
        &mut self,
        session_id: SessionId,
    ) -> Result<SessionReady, MarkSessionReadyError> {
        self.sessions.mark_session_ready(session_id)
    }

    /// Runs `inference.session.CloseSession` ([`Sessions::close_session`]).
    ///
    /// # Errors
    ///
    /// The refusals of [`Sessions::close_session`], in its order.
    pub fn close_session(
        &mut self,
        session_id: SessionId,
    ) -> Result<SessionDraining, SessionMoveError> {
        self.sessions.close_session(session_id)
    }

    /// Runs `inference.session.SendInput` ([`Sessions::send_input`]).
    ///
    /// # Errors
    ///
    /// The refusals of [`Sessions::send_input`], in its order.
    pub fn send_input(&mut self, session_id: SessionId) -> Result<InputSent, SessionMoveError> {
        self.sessions.send_input(session_id)
    }

    /// Runs `inference.session.CommitInput` ([`Sessions::commit_input`]).
    ///
    /// # Errors
    ///
    /// The refusals of [`Sessions::commit_input`], in its order.
    pub fn commit_input(
        &mut self,
        session_id: SessionId,
    ) -> Result<InputCommitted, SessionMoveError> {
        self.sessions.commit_input(session_id)
    }

    /// Runs `inference.session.SwitchPipeline` ([`Sessions::switch_pipeline`]) against the
    /// pipelines `graph` declares.
    ///
    /// # Errors
    ///
    /// The refusals of [`Sessions::switch_pipeline`], in its order.
    pub fn switch_pipeline(
        &mut self,
        graph: &GraphDomain,
        input: SwitchPipeline,
    ) -> Result<PipelineSwitched, SwitchPipelineError> {
        self.sessions.switch_pipeline(graph, input)
    }

    /// Runs `inference.session.Interrupt` ([`Sessions::interrupt`]).
    ///
    /// # Errors
    ///
    /// The refusals of [`Sessions::interrupt`], in its order.
    pub fn interrupt(
        &mut self,
        session_id: SessionId,
    ) -> Result<SessionInterrupted, SessionMoveError> {
        self.sessions.interrupt(session_id)
    }

    /// Runs `inference.session.CancelSession` ([`Sessions::cancel_session`]).
    ///
    /// # Errors
    ///
    /// The refusals of [`Sessions::cancel_session`], in its order.
    pub fn cancel_session(
        &mut self,
        session_id: SessionId,
    ) -> Result<SessionClosed, SessionMoveError> {
        self.sessions.cancel_session(session_id)
    }

    /// Runs `inference.session.CompleteSessionClose` ([`Sessions::complete_session_close`]).
    ///
    /// # Errors
    ///
    /// The refusals of [`Sessions::complete_session_close`], in its order.
    pub fn complete_session_close(
        &mut self,
        session_id: SessionId,
    ) -> Result<SessionClosed, SessionMoveError> {
        self.sessions.complete_session_close(session_id)
    }

    /// The session carrying `session_id`, if one was opened.
    #[must_use]
    pub fn get(&self, session_id: SessionId) -> Option<&Session> {
        self.sessions.get(session_id)
    }

    /// Every session, in the order they were opened, in every state.
    pub fn sessions(&self) -> impl Iterator<Item = &Session> {
        self.sessions.sessions()
    }

    /// Every event emitted, in the order it was emitted.
    #[must_use]
    pub fn events(&self) -> &[SessionEvent] {
        self.sessions.events()
    }

    /// The view `inference.session.Sessions`: one row per session, in the order they were opened.
    #[must_use]
    pub fn sessions_view(&self) -> Vec<SessionsRow> {
        self.sessions.sessions_view()
    }

    /// Runs `inference.session.RecordHistory` ([`Sessions::record_history`]).
    ///
    /// # Errors
    ///
    /// The refusals of [`Sessions::record_history`], in its order.
    pub fn record_history(
        &mut self,
        input: RecordHistory,
    ) -> Result<HistoryRecorded, RecordHistoryError> {
        self.sessions.record_history(input)
    }

    /// The view `inference.session.History`: every entry of every session, in the order it was
    /// recorded.
    #[must_use]
    pub fn history_view(&self) -> &[HistoryEntry] {
        self.sessions.history_view()
    }
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
