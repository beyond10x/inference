//! The session state machine of `ess/domains/session.yaml`, command by command: each accepting
//! branch and each refusal, read against a graph that holds pipelines and runs as a host would.

use std::collections::BTreeMap;

use inference_core::id::{RunId, SessionId};
use inference_facade::session::{
    AttachRun, AttachRunError, Graph, GraphPipeline, HistoryKind, HistoryRecorded, InputCommitted,
    InputSent, MarkSessionReadyError, ModeMismatch, NoCurrentRun, OpenSession, OpenSessionError,
    PipelineMismatch, PipelineSwitched, RecordHistory, RecordHistoryError, RunAttached,
    SessionClosed, SessionDraining, SessionEvent, SessionInterrupted, SessionMoveError,
    SessionNotFound, SessionOpened, SessionReady, SessionState, SessionStateConflict, Sessions,
    SessionsRow, SwitchPipeline, SwitchPipelineError,
};
use inference_pipeline::{
    InteractionMode, PipelineId, PipelineNotFound, PipelineNotReady, PipelineState,
};
use inference_tools::ToolCallId;
use uuid::Uuid;

/// The pipelines and runs a host holds, read by the session through [`Graph`].
#[derive(Default)]
struct Host {
    pipelines: BTreeMap<PipelineId, GraphPipeline>,
    runs: BTreeMap<RunId, PipelineId>,
}

impl Graph for Host {
    fn pipeline(&self, pipeline_id: PipelineId) -> Option<GraphPipeline> {
        self.pipelines.get(&pipeline_id).copied()
    }

    fn run_pipeline(&self, run_id: RunId) -> Option<PipelineId> {
        self.runs.get(&run_id).copied()
    }
}

const PLANNED: u128 = 1;
const SECOND: u128 = 2;
const DECLARED: u128 = 3;
const UNKNOWN: u128 = 9;
const OURS: u128 = 11;
const THEIRS: u128 = 12;
const UNHELD: u128 = 13;

fn pipeline(n: u128) -> PipelineId {
    PipelineId::from(Uuid::from_u128(n))
}

fn run(n: u128) -> RunId {
    RunId::from(Uuid::from_u128(n))
}

/// Two Planned batch pipelines, a Declared one, a run of the first and a run of the second.
fn host() -> Host {
    let mut host = Host::default();
    let batch = |state| GraphPipeline {
        state,
        mode: InteractionMode::Batch,
    };
    host.pipelines
        .insert(pipeline(PLANNED), batch(PipelineState::Planned));
    host.pipelines
        .insert(pipeline(SECOND), batch(PipelineState::Planned));
    host.pipelines
        .insert(pipeline(DECLARED), batch(PipelineState::Declared));
    host.runs.insert(run(OURS), pipeline(PLANNED));
    host.runs.insert(run(THEIRS), pipeline(SECOND));
    host
}

fn open(pipeline_id: PipelineId, mode: InteractionMode) -> OpenSession {
    OpenSession { pipeline_id, mode }
}

fn opened(sessions: &mut Sessions, host: &Host) -> SessionId {
    sessions
        .open_session(host, open(pipeline(PLANNED), InteractionMode::Batch))
        .expect("a Planned pipeline of the mode opens a session")
        .session_id
}

fn attach(sessions: &mut Sessions, host: &Host, session_id: SessionId, run_id: RunId) {
    sessions
        .attach_run(host, AttachRun { session_id, run_id })
        .expect("attached");
}

fn conflict(current: SessionState) -> SessionStateConflict {
    SessionStateConflict { current }
}

#[test]
fn open_session_opens_an_opened_session_with_no_run_over_a_planned_pipeline_of_the_mode() {
    let host = host();
    let mut sessions = Sessions::counted_in(5);
    let event = sessions
        .open_session(&host, open(pipeline(PLANNED), InteractionMode::Batch))
        .expect("opened");
    assert_eq!(
        event,
        SessionOpened {
            session_id: event.session_id,
            pipeline_id: pipeline(PLANNED),
        }
    );
    assert_eq!(
        event.session_id.to_string(),
        "00000005-0000-4000-8000-000000000001",
        "a counted store issues the first identity of its space"
    );
    assert_eq!(
        sessions.sessions_view(),
        vec![SessionsRow {
            session_id: event.session_id,
            pipeline_id: pipeline(PLANNED),
            mode: InteractionMode::Batch,
            run_id: None,
            state: SessionState::Opened,
        }]
    );
    assert_eq!(sessions.events(), [SessionEvent::SessionOpened(event)]);
}

#[test]
fn open_session_refuses_an_unknown_pipeline_then_one_not_planned_then_another_mode() {
    let host = host();
    let mut sessions = Sessions::counted_in(5);
    assert_eq!(
        sessions.open_session(&host, open(pipeline(UNKNOWN), InteractionMode::Duplex)),
        Err(OpenSessionError::UnknownPipeline(PipelineNotFound {
            pipeline_id: pipeline(UNKNOWN)
        }))
    );
    assert_eq!(
        sessions.open_session(&host, open(pipeline(DECLARED), InteractionMode::Duplex)),
        Err(OpenSessionError::NotPlanned(PipelineNotReady {
            pipeline_id: pipeline(DECLARED),
            required: PipelineState::Planned,
        })),
        "a pipeline that is not Planned is refused before its mode is read"
    );
    assert_eq!(
        sessions.open_session(&host, open(pipeline(PLANNED), InteractionMode::Duplex)),
        Err(OpenSessionError::ModeMismatch(ModeMismatch {
            pipeline_id: pipeline(PLANNED),
            mode: InteractionMode::Duplex,
        }))
    );
    assert!(sessions.events().is_empty(), "a refusal records no event");
    assert!(sessions.sessions_view().is_empty(), "and stores nothing");
    assert_eq!(
        opened(&mut sessions, &host).to_string(),
        "00000005-0000-4000-8000-000000000001",
        "the refusals took no identity"
    );
}

#[test]
fn sessions_from_new_take_random_identities() {
    let host = host();
    let mut sessions = Sessions::new();
    let first = opened(&mut sessions, &host);
    let second = opened(&mut sessions, &host);
    assert_ne!(first, second);
    assert_eq!(first.as_uuid().get_version_num(), 4);
}

#[test]
fn attach_run_makes_the_run_current_and_moves_a_ready_session_to_opened() {
    let host = host();
    let mut sessions = Sessions::counted_in(5);
    let session_id = opened(&mut sessions, &host);

    assert_eq!(
        sessions.attach_run(
            &host,
            AttachRun {
                session_id,
                run_id: run(OURS),
            }
        ),
        Ok(RunAttached {
            session_id,
            run_id: run(OURS),
        })
    );
    let session = sessions.get(session_id).expect("stored");
    assert_eq!(session.run_id(), Some(run(OURS)));
    assert_eq!(session.state(), SessionState::Opened);

    sessions.mark_session_ready(session_id).expect("ready");
    attach(&mut sessions, &host, session_id, run(UNHELD));
    let session = sessions.get(session_id).expect("stored");
    assert_eq!(
        session.run_id(),
        Some(run(UNHELD)),
        "a run the graph holds no record of has no pipeline to compare, and is attached"
    );
    assert_eq!(
        session.state(),
        SessionState::Opened,
        "readiness is asserted again for the new run"
    );
}

#[test]
fn attach_run_refuses_an_unknown_session_then_another_pipeline_then_a_draining_or_closed_one() {
    let host = host();
    let mut sessions = Sessions::counted_in(5);
    let unknown = SessionId::from(Uuid::from_u128(UNKNOWN));
    assert_eq!(
        sessions.attach_run(
            &host,
            AttachRun {
                session_id: unknown,
                run_id: run(THEIRS),
            }
        ),
        Err(AttachRunError::NoSuchSession(SessionNotFound {
            session_id: unknown
        }))
    );

    let session_id = opened(&mut sessions, &host);
    let bystander = opened(&mut sessions, &host);
    let mismatch = Err(AttachRunError::PipelineMismatch(PipelineMismatch {
        session_id,
        run_id: run(THEIRS),
    }));
    let theirs = AttachRun {
        session_id,
        run_id: run(THEIRS),
    };
    let ours = AttachRun {
        session_id,
        run_id: run(OURS),
    };
    assert_eq!(sessions.attach_run(&host, theirs), mismatch);
    assert_eq!(sessions.get(session_id).and_then(|s| s.run_id()), None);
    sessions
        .attach_run(&host, ours)
        .expect("a run of the session's pipeline is attached");
    assert_eq!(
        sessions.get(bystander).and_then(|s| s.run_id()),
        None,
        "attaching to one session leaves another as it was"
    );

    sessions.close_session(session_id).expect("drains");
    let recorded = sessions.events().len();
    assert_eq!(
        sessions.attach_run(&host, theirs),
        mismatch,
        "the external refusal moves nothing, so it answers before the held state"
    );
    assert_eq!(
        sessions.attach_run(&host, ours),
        Err(AttachRunError::WrongState(conflict(SessionState::Draining)))
    );
    sessions.complete_session_close(session_id).expect("closes");
    assert_eq!(
        sessions.attach_run(&host, ours),
        Err(AttachRunError::WrongState(conflict(SessionState::Closed)))
    );
    assert_eq!(sessions.events().len(), recorded + 1);
    assert_eq!(
        sessions.get(session_id).and_then(|s| s.run_id()),
        Some(run(OURS))
    );
}

#[test]
fn the_two_unmapped_attach_rules_are_not_enforced_as_the_specification_declares() {
    let host = host();
    let mut sessions = Sessions::counted_in(5);
    let session_id = opened(&mut sessions, &host);
    let other = opened(&mut sessions, &host);

    attach(&mut sessions, &host, session_id, run(OURS));
    sessions.mark_session_ready(session_id).expect("ready");
    attach(&mut sessions, &host, session_id, run(OURS));
    assert_eq!(
        sessions.get(session_id).map(|s| s.state()),
        Some(SessionState::Opened),
        "a retried attach moves a Ready session to Opened (session.yaml UNMAPPED: retried attach)"
    );
    assert_eq!(
        sessions
            .events()
            .iter()
            .filter(|event| matches!(event, SessionEvent::RunAttached(_)))
            .count(),
        2,
        "and records RunAttached again"
    );
    attach(&mut sessions, &host, other, run(OURS));
    assert_eq!(
        sessions.get(other).and_then(|s| s.run_id()),
        Some(run(OURS)),
        "one run current in two sessions (session.yaml UNMAPPED: at most one session)"
    );
    assert_eq!(
        sessions.get(session_id).and_then(|s| s.run_id()),
        Some(run(OURS))
    );
}

#[test]
fn mark_session_ready_moves_an_opened_session_with_a_run_to_ready() {
    let host = host();
    let mut sessions = Sessions::counted_in(5);
    let session_id = opened(&mut sessions, &host);
    attach(&mut sessions, &host, session_id, run(OURS));
    assert_eq!(
        sessions.mark_session_ready(session_id),
        Ok(SessionReady { session_id })
    );
    assert_eq!(
        sessions.get(session_id).map(|s| s.state()),
        Some(SessionState::Ready)
    );
}

#[test]
fn mark_session_ready_refuses_an_unknown_session_one_with_no_run_and_one_not_opened() {
    let host = host();
    let mut sessions = Sessions::counted_in(5);
    let unknown = SessionId::from(Uuid::from_u128(UNKNOWN));
    assert_eq!(
        sessions.mark_session_ready(unknown),
        Err(MarkSessionReadyError::NoSuchSession(SessionNotFound {
            session_id: unknown
        }))
    );
    let session_id = opened(&mut sessions, &host);
    assert_eq!(
        sessions.mark_session_ready(session_id),
        Err(MarkSessionReadyError::NoCurrentRun(NoCurrentRun {
            session_id
        }))
    );
    attach(&mut sessions, &host, session_id, run(OURS));
    sessions.mark_session_ready(session_id).expect("ready");
    assert_eq!(
        sessions.mark_session_ready(session_id),
        Err(MarkSessionReadyError::WrongState(conflict(
            SessionState::Ready
        )))
    );
    sessions.close_session(session_id).expect("drains");
    assert_eq!(
        sessions.mark_session_ready(session_id),
        Err(MarkSessionReadyError::WrongState(conflict(
            SessionState::Draining
        ))),
        "a draining session is not Opened, whatever run it holds"
    );

    let never_ran = opened(&mut sessions, &host);
    sessions.close_session(never_ran).expect("drains");
    assert_eq!(
        sessions.mark_session_ready(never_ran),
        Err(MarkSessionReadyError::WrongState(conflict(
            SessionState::Draining
        ))),
        "a session that is not Opened is refused for its state, run or no run"
    );
}

#[test]
fn close_session_drains_an_opened_or_ready_session_and_refuses_the_rest() {
    let host = host();
    let mut sessions = Sessions::counted_in(5);
    let unknown = SessionId::from(Uuid::from_u128(UNKNOWN));
    assert_eq!(
        sessions.close_session(unknown),
        Err(SessionMoveError::NoSuchSession(SessionNotFound {
            session_id: unknown
        }))
    );

    let never_ready = opened(&mut sessions, &host);
    assert_eq!(
        sessions.close_session(never_ready),
        Ok(SessionDraining {
            session_id: never_ready
        }),
        "a session that never became ready may be closed"
    );
    let ready = opened(&mut sessions, &host);
    attach(&mut sessions, &host, ready, run(OURS));
    sessions.mark_session_ready(ready).expect("ready");
    assert_eq!(
        sessions.close_session(ready),
        Ok(SessionDraining { session_id: ready })
    );
    assert_eq!(
        sessions.get(ready).map(|s| s.state()),
        Some(SessionState::Draining)
    );
    assert_eq!(
        sessions.close_session(ready),
        Err(SessionMoveError::WrongState(conflict(
            SessionState::Draining
        )))
    );
    sessions.complete_session_close(ready).expect("closes");
    assert_eq!(
        sessions.close_session(ready),
        Err(SessionMoveError::WrongState(conflict(SessionState::Closed)))
    );
}

#[test]
fn complete_session_close_closes_a_draining_session_and_refuses_the_rest() {
    let host = host();
    let mut sessions = Sessions::counted_in(5);
    let unknown = SessionId::from(Uuid::from_u128(UNKNOWN));
    assert_eq!(
        sessions.complete_session_close(unknown),
        Err(SessionMoveError::NoSuchSession(SessionNotFound {
            session_id: unknown
        }))
    );
    let session_id = opened(&mut sessions, &host);
    assert_eq!(
        sessions.complete_session_close(session_id),
        Err(SessionMoveError::WrongState(conflict(SessionState::Opened)))
    );
    attach(&mut sessions, &host, session_id, run(OURS));
    sessions.mark_session_ready(session_id).expect("ready");
    assert_eq!(
        sessions.complete_session_close(session_id),
        Err(SessionMoveError::WrongState(conflict(SessionState::Ready)))
    );
    sessions.close_session(session_id).expect("drains");
    assert_eq!(
        sessions.complete_session_close(session_id),
        Ok(SessionClosed { session_id })
    );
    assert_eq!(
        sessions.complete_session_close(session_id),
        Err(SessionMoveError::WrongState(conflict(SessionState::Closed)))
    );
    assert_eq!(
        sessions.sessions_view()[0].state,
        SessionState::Closed,
        "a Closed session stays in the view"
    );
}

#[test]
fn every_accepted_command_records_its_event_in_order_and_no_refusal_records_one() {
    let host = host();
    let mut sessions = Sessions::counted_in(5);
    let session_id = opened(&mut sessions, &host);
    let _ = sessions.mark_session_ready(session_id);
    attach(&mut sessions, &host, session_id, run(OURS));
    let _ = sessions.attach_run(
        &host,
        AttachRun {
            session_id,
            run_id: run(THEIRS),
        },
    );
    sessions.mark_session_ready(session_id).expect("ready");
    let _ = sessions.complete_session_close(session_id);
    sessions.close_session(session_id).expect("drains");
    let _ = sessions.close_session(session_id);
    sessions.complete_session_close(session_id).expect("closes");
    assert_eq!(
        sessions.events(),
        [
            SessionEvent::SessionOpened(SessionOpened {
                session_id,
                pipeline_id: pipeline(PLANNED),
            }),
            SessionEvent::RunAttached(RunAttached {
                session_id,
                run_id: run(OURS),
            }),
            SessionEvent::SessionReady(SessionReady { session_id }),
            SessionEvent::SessionDraining(SessionDraining { session_id }),
            SessionEvent::SessionClosed(SessionClosed { session_id }),
        ]
    );
}

/// Sending or committing on a session, through `send` or `commit`.
type InputCommand = fn(&mut Sessions, SessionId) -> Result<SessionEvent, SessionMoveError>;

fn send(sessions: &mut Sessions, session_id: SessionId) -> Result<SessionEvent, SessionMoveError> {
    sessions.send_input(session_id).map(SessionEvent::InputSent)
}

fn commit(
    sessions: &mut Sessions,
    session_id: SessionId,
) -> Result<SessionEvent, SessionMoveError> {
    sessions
        .commit_input(session_id)
        .map(SessionEvent::InputCommitted)
}

#[test]
fn send_input_and_commit_input_are_admitted_by_a_ready_session_and_leave_it_ready() {
    let host = host();
    let mut sessions = Sessions::counted_in(5);
    let session_id = opened(&mut sessions, &host);
    attach(&mut sessions, &host, session_id, run(OURS));
    sessions.mark_session_ready(session_id).expect("ready");
    let recorded = sessions.events().len();

    assert_eq!(
        sessions.send_input(session_id),
        Ok(InputSent { session_id })
    );
    assert_eq!(
        sessions.commit_input(session_id),
        Ok(InputCommitted { session_id })
    );
    assert_eq!(
        sessions.get(session_id).map(|s| s.state()),
        Some(SessionState::Ready),
        "send and commit move Ready to Ready"
    );
    assert_eq!(
        sessions.get(session_id).and_then(|s| s.run_id()),
        Some(run(OURS))
    );
    assert_eq!(
        sessions.events()[recorded..],
        [
            SessionEvent::InputSent(InputSent { session_id }),
            SessionEvent::InputCommitted(InputCommitted { session_id }),
        ]
    );
}

#[test]
fn send_input_and_commit_input_refuse_an_unknown_session_and_one_that_is_not_ready() {
    for input in [send as InputCommand, commit] {
        let host = host();
        let mut sessions = Sessions::counted_in(5);
        let unknown = SessionId::from(Uuid::from_u128(UNKNOWN));
        assert_eq!(
            input(&mut sessions, unknown),
            Err(SessionMoveError::NoSuchSession(SessionNotFound {
                session_id: unknown
            }))
        );

        let session_id = opened(&mut sessions, &host);
        attach(&mut sessions, &host, session_id, run(OURS));
        assert_eq!(
            input(&mut sessions, session_id),
            Err(SessionMoveError::WrongState(conflict(SessionState::Opened))),
            "an Opened session is not ready for interaction"
        );
        sessions.mark_session_ready(session_id).expect("ready");
        sessions.close_session(session_id).expect("drains");
        assert_eq!(
            input(&mut sessions, session_id),
            Err(SessionMoveError::WrongState(conflict(
                SessionState::Draining
            ))),
            "a draining session admits no new interaction"
        );
        sessions.complete_session_close(session_id).expect("closes");
        assert_eq!(
            input(&mut sessions, session_id),
            Err(SessionMoveError::WrongState(conflict(SessionState::Closed)))
        );
        assert!(
            !sessions.events().iter().any(|event| matches!(
                event,
                SessionEvent::InputSent(_) | SessionEvent::InputCommitted(_)
            )),
            "a refusal records no event"
        );
    }
}

fn entry(session_id: SessionId, kind: HistoryKind, text: &str, position: i64) -> RecordHistory {
    RecordHistory {
        session_id,
        run_id: run(OURS),
        position,
        kind,
        text: text.to_owned(),
        call_id: None,
    }
}

#[test]
fn record_history_records_an_entry_in_every_state_but_closed_and_keeps_it_after_the_close() {
    let host = host();
    let mut sessions = Sessions::counted_in(5);
    let session_id = opened(&mut sessions, &host);
    let other = opened(&mut sessions, &host);

    let first = sessions
        .record_history(entry(session_id, HistoryKind::UserTurn, "hello", 1))
        .expect("an Opened session records");
    assert_eq!(
        first,
        HistoryRecorded {
            entry_id: first.entry_id,
            session_id,
            run_id: run(OURS),
            position: 1,
            kind: HistoryKind::UserTurn,
        }
    );
    assert_eq!(
        first.entry_id.to_string(),
        "00000005-0000-4000-8000-000000000003",
        "an entry takes the next identity of the session space"
    );
    attach(&mut sessions, &host, session_id, run(OURS));
    sessions.mark_session_ready(session_id).expect("ready");
    assert_eq!(sessions.next_position(session_id), 2);
    sessions
        .record_history(entry(session_id, HistoryKind::AssistantSegment, "hi", 2))
        .expect("a Ready session records");
    sessions
        .record_history(entry(other, HistoryKind::UserTurn, "elsewhere", 1))
        .expect("another session records its own");
    sessions.close_session(session_id).expect("drains");
    let result = RecordHistory {
        call_id: Some(ToolCallId::from(Uuid::from_u128(21))),
        ..entry(session_id, HistoryKind::ToolResult, "{\"ok\":true}", 3)
    };
    sessions
        .record_history(result)
        .expect("a draining session completes what is in flight");
    sessions.complete_session_close(session_id).expect("closes");

    let history: Vec<_> = sessions.history(session_id).cloned().collect();
    assert_eq!(
        history
            .iter()
            .map(|entry| (entry.position(), entry.kind(), entry.text(), entry.run_id()))
            .collect::<Vec<_>>(),
        [
            (1, HistoryKind::UserTurn, "hello", run(OURS)),
            (2, HistoryKind::AssistantSegment, "hi", run(OURS)),
            (3, HistoryKind::ToolResult, "{\"ok\":true}", run(OURS)),
        ],
        "a Closed session keeps its history, in the order it was recorded"
    );
    assert_eq!(
        history[2].call_id(),
        Some(ToolCallId::from(Uuid::from_u128(21)))
    );
    assert_eq!(sessions.next_position(other), 2);
    assert_eq!(sessions.history_view().len(), 4);
    assert!(
        sessions
            .events()
            .iter()
            .filter(|event| matches!(event, SessionEvent::HistoryRecorded(_)))
            .count()
            == 4
    );
}

#[test]
fn record_history_refuses_an_unknown_session_then_a_closed_one() {
    let host = host();
    let mut sessions = Sessions::counted_in(5);
    let unknown = SessionId::from(Uuid::from_u128(UNKNOWN));
    assert_eq!(
        sessions.record_history(entry(unknown, HistoryKind::UserTurn, "x", 1)),
        Err(RecordHistoryError::NoSuchSession(SessionNotFound {
            session_id: unknown
        }))
    );
    let session_id = opened(&mut sessions, &host);
    sessions.close_session(session_id).expect("drains");
    sessions.complete_session_close(session_id).expect("closes");
    let recorded = sessions.events().len();
    assert_eq!(
        sessions.record_history(entry(session_id, HistoryKind::UserTurn, "x", 1)),
        Err(RecordHistoryError::Closed(conflict(SessionState::Closed)))
    );
    assert!(sessions.history_view().is_empty(), "nothing is recorded");
    assert_eq!(sessions.events().len(), recorded, "and no event");
    let next = opened(&mut sessions, &host);
    assert_eq!(
        next.to_string(),
        "00000005-0000-4000-8000-000000000002",
        "a refusal takes no identity"
    );
}

#[test]
fn interrupt_is_admitted_by_a_ready_session_leaves_it_ready_and_refuses_every_other() {
    let host = host();
    let mut sessions = Sessions::counted_in(5);
    let unknown = SessionId::from(Uuid::from_u128(UNKNOWN));
    assert_eq!(
        sessions.interrupt(unknown),
        Err(SessionMoveError::NoSuchSession(SessionNotFound {
            session_id: unknown
        }))
    );
    let session_id = opened(&mut sessions, &host);
    attach(&mut sessions, &host, session_id, run(OURS));
    assert_eq!(
        sessions.interrupt(session_id),
        Err(SessionMoveError::WrongState(conflict(SessionState::Opened)))
    );
    sessions.mark_session_ready(session_id).expect("ready");
    assert_eq!(
        sessions.interrupt(session_id),
        Ok(SessionInterrupted { session_id })
    );
    assert_eq!(
        sessions.get(session_id).map(|s| s.state()),
        Some(SessionState::Ready),
        "an interrupt moves Ready to Ready"
    );
    assert_eq!(
        sessions.events().last(),
        Some(&SessionEvent::SessionInterrupted(SessionInterrupted {
            session_id
        }))
    );
    sessions.close_session(session_id).expect("drains");
    assert_eq!(
        sessions.interrupt(session_id),
        Err(SessionMoveError::WrongState(conflict(
            SessionState::Draining
        )))
    );
    sessions.complete_session_close(session_id).expect("closes");
    assert_eq!(
        sessions.interrupt(session_id),
        Err(SessionMoveError::WrongState(conflict(SessionState::Closed)))
    );
}

#[test]
fn cancel_session_closes_an_opened_ready_or_draining_session_and_refuses_a_closed_one() {
    let host = host();
    let mut sessions = Sessions::counted_in(5);
    let unknown = SessionId::from(Uuid::from_u128(UNKNOWN));
    assert_eq!(
        sessions.cancel_session(unknown),
        Err(SessionMoveError::NoSuchSession(SessionNotFound {
            session_id: unknown
        }))
    );

    let never_ready = opened(&mut sessions, &host);
    let ready = opened(&mut sessions, &host);
    attach(&mut sessions, &host, ready, run(OURS));
    sessions.mark_session_ready(ready).expect("ready");
    let draining = opened(&mut sessions, &host);
    sessions.close_session(draining).expect("drains");

    for session_id in [never_ready, ready, draining] {
        let recorded = sessions.events().len();
        assert_eq!(
            sessions.cancel_session(session_id),
            Ok(SessionClosed { session_id })
        );
        assert_eq!(
            sessions.get(session_id).map(|s| s.state()),
            Some(SessionState::Closed)
        );
        assert_eq!(
            sessions.events()[recorded..],
            [SessionEvent::SessionClosed(SessionClosed { session_id })],
            "the cancel emits the close and nothing else"
        );
        assert_eq!(
            sessions.cancel_session(session_id),
            Err(SessionMoveError::WrongState(conflict(SessionState::Closed)))
        );
    }
    assert!(
        !sessions.events().iter().any(|event| matches!(
            event,
            SessionEvent::SessionDraining(SessionDraining { session_id }) if *session_id == ready
        )),
        "a cancelled Ready session never drained"
    );
}

fn switch(session_id: SessionId, pipeline: u128, mode: InteractionMode) -> SwitchPipeline {
    SwitchPipeline {
        session_id,
        pipeline_id: self::pipeline(pipeline),
        mode,
    }
}

#[test]
fn switch_pipeline_moves_a_ready_session_over_to_another_planned_pipeline_with_no_run() {
    let host = host();
    let mut sessions = Sessions::counted_in(5);
    let session_id = opened(&mut sessions, &host);
    attach(&mut sessions, &host, session_id, run(OURS));
    sessions.mark_session_ready(session_id).expect("ready");
    sessions
        .record_history(entry(session_id, HistoryKind::UserTurn, "before", 1))
        .expect("recorded");

    assert_eq!(
        sessions.switch_pipeline(&host, switch(session_id, SECOND, InteractionMode::Batch)),
        Ok(PipelineSwitched {
            session_id,
            pipeline_id: pipeline(SECOND),
        })
    );
    let session = sessions.get(session_id).expect("stored");
    assert_eq!(session.pipeline_id(), pipeline(SECOND));
    assert_eq!(session.mode(), InteractionMode::Batch);
    assert_eq!(session.run_id(), None, "the current run is cleared");
    assert_eq!(
        session.state(),
        SessionState::Opened,
        "readiness is asserted again for the next run"
    );
    assert_eq!(
        sessions.events().last(),
        Some(&SessionEvent::PipelineSwitched(PipelineSwitched {
            session_id,
            pipeline_id: pipeline(SECOND),
        }))
    );
    assert_eq!(
        sessions.history(session_id).count(),
        1,
        "history stays with the session"
    );
    attach(&mut sessions, &host, session_id, run(THEIRS));
    sessions
        .mark_session_ready(session_id)
        .expect("a run of the new pipeline makes it ready again");
}

#[test]
fn switch_pipeline_refuses_an_unknown_session_then_the_pipeline_as_open_does_then_a_state_but_ready()
 {
    let mut host = host();
    host.pipelines.insert(
        pipeline(UNHELD),
        GraphPipeline {
            state: PipelineState::Planned,
            mode: InteractionMode::Duplex,
        },
    );
    let mut sessions = Sessions::counted_in(5);
    let unknown = SessionId::from(Uuid::from_u128(UNKNOWN));
    assert_eq!(
        sessions.switch_pipeline(&host, switch(unknown, UNKNOWN, InteractionMode::Batch)),
        Err(SwitchPipelineError::NoSuchSession(SessionNotFound {
            session_id: unknown
        })),
        "the session's existence answers first"
    );

    let session_id = opened(&mut sessions, &host);
    assert_eq!(
        sessions.switch_pipeline(&host, switch(session_id, UNKNOWN, InteractionMode::Batch)),
        Err(SwitchPipelineError::WrongState(conflict(
            SessionState::Opened
        ))),
        "a pipeline the graph holds no record of is not refused here, as AttachRun attaches a \
         run it holds no record of (session.yaml UNMAPPED: unknown pipeline); the state answers"
    );
    assert_eq!(
        sessions.switch_pipeline(&host, switch(session_id, DECLARED, InteractionMode::Duplex)),
        Err(SwitchPipelineError::NotPlanned(PipelineNotReady {
            pipeline_id: pipeline(DECLARED),
            required: PipelineState::Planned,
        })),
        "a pipeline that is not Planned is refused before its mode is read"
    );
    assert_eq!(
        sessions.switch_pipeline(&host, switch(session_id, UNHELD, InteractionMode::Batch)),
        Err(SwitchPipelineError::ModeMismatch(ModeMismatch {
            pipeline_id: pipeline(UNHELD),
            mode: InteractionMode::Batch,
        }))
    );
    assert_eq!(
        sessions.switch_pipeline(&host, switch(session_id, SECOND, InteractionMode::Batch)),
        Err(SwitchPipelineError::WrongState(conflict(
            SessionState::Opened
        ))),
        "only a Ready session switches"
    );
    attach(&mut sessions, &host, session_id, run(OURS));
    sessions.mark_session_ready(session_id).expect("ready");
    sessions.close_session(session_id).expect("drains");
    assert_eq!(
        sessions.switch_pipeline(&host, switch(session_id, SECOND, InteractionMode::Batch)),
        Err(SwitchPipelineError::WrongState(conflict(
            SessionState::Draining
        )))
    );
    sessions.complete_session_close(session_id).expect("closes");
    let recorded = sessions.events().len();
    assert_eq!(
        sessions.switch_pipeline(&host, switch(session_id, SECOND, InteractionMode::Batch)),
        Err(SwitchPipelineError::WrongState(conflict(
            SessionState::Closed
        )))
    );
    assert_eq!(
        sessions.events().len(),
        recorded,
        "a refusal records nothing"
    );
    assert_eq!(
        sessions.get(session_id).map(|s| s.pipeline_id()),
        Some(pipeline(PLANNED))
    );
}
