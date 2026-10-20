//! `inference.session`: the eleven session commands, the two views and the ten events of
//! `ess/domains/session.yaml`, answered by [`SessionDomain`]. The store records the session's own
//! events only; an event of the session vocabulary from another family is not the store's to
//! record, and observing one is an error.
//!
//! Each request field is read on its own ([`Input`]); the outcome, the error and the event come
//! from what the store returned. `OpenSession` reads the pipeline's existence, state and mode, and
//! `AttachRun` the pipeline of the run it attaches, from the `inference.graph` store, through
//! [`SessionDomain::open_session`] and [`SessionDomain::attach_run`].
//!
//! `AttachRun`'s `pipeline-mismatch` outcome is declared `external`: the store takes it when the
//! graph store holds the run and the run executes another pipeline, and a scenario may force it
//! ([`configure_external_outcome`]) on the next invocation, which then moves nothing and answers
//! `PipelineMismatch` naming the session and the run the request sent, for an unknown session and
//! in every state, as the forced outcomes of `inference.graph` answer.
//!
//! A command's consistency token is the target's ([`Stores::token`]), and a read demanding a token
//! the stores have not reached is an error ([`Stores::require`]). An observed event's position is
//! its place in the target's one publication order ([`Stores::session_position`]).
//!
//! [`SessionDomain::open_session`]: inference_testkit::session::SessionDomain::open_session
//! [`SessionDomain::attach_run`]: inference_testkit::session::SessionDomain::attach_run

use std::collections::BTreeMap;

use ess_conformance::scenario::{CommandRef, ErrorRef, EventRef, OutcomeRef};
use ess_conformance::target::{
    DeclaredErrorValue, EventObservationRequest, ExternalOutcomeControl, ObservedEvent,
    SemanticCommandRequest, SemanticCommandResult, SemanticViewRequest, SemanticViewResult,
    TargetError, ViewRow,
};
use ess_primitives::node::Node;
use inference_core::id::{RunId, SessionId};
use inference_pipeline::{InteractionMode, PipelineId, PipelineState};
use inference_testkit::session::{
    AttachRun, AttachRunError, HistoryEntry, HistoryKind, MarkSessionReadyError, OpenSession,
    OpenSessionError, RecordHistory, RecordHistoryError, SessionDomain, SessionEvent,
    SessionMoveError, SessionNotFound, SessionStateConflict, SessionsRow, SwitchPipeline,
    SwitchPipelineError,
};
use inference_tools::ToolCallId;

use super::{Input, Stores, integer_node, named_node, text_node, unreadable, unsupported};

/// The prefix of every name this module answers.
pub const DOMAIN: &str = "inference.session";

const OPEN_SESSION: &str = "inference.session.OpenSession";
const ATTACH_RUN: &str = "inference.session.AttachRun";
const MARK_SESSION_READY: &str = "inference.session.MarkSessionReady";
const CLOSE_SESSION: &str = "inference.session.CloseSession";
const COMPLETE_SESSION_CLOSE: &str = "inference.session.CompleteSessionClose";
const SEND_INPUT: &str = "inference.session.SendInput";
const COMMIT_INPUT: &str = "inference.session.CommitInput";
const RECORD_HISTORY: &str = "inference.session.RecordHistory";
const INTERRUPT: &str = "inference.session.Interrupt";
const CANCEL_SESSION: &str = "inference.session.CancelSession";
const SWITCH_PIPELINE: &str = "inference.session.SwitchPipeline";

/// The outcome of `AttachRun` that `ess/domains/session.yaml` declares external.
const PIPELINE_MISMATCH_OUTCOME: &str = "pipeline-mismatch";

/// The outcomes of `SwitchPipeline` that `ess/domains/session.yaml` declares external, which
/// `OpenSession` shares.
const UNKNOWN_PIPELINE: &str = "unknown-pipeline";
const NOT_PLANNED: &str = "not-planned";
const MODE_MISMATCH_OUTCOME: &str = "mode-mismatch";

const SESSION_OPENED: &str = "inference.session.SessionOpened";
const RUN_ATTACHED: &str = "inference.session.RunAttached";
const SESSION_READY: &str = "inference.session.SessionReady";
const SESSION_DRAINING: &str = "inference.session.SessionDraining";
const SESSION_CLOSED: &str = "inference.session.SessionClosed";
const INPUT_SENT: &str = "inference.session.InputSent";
const INPUT_COMMITTED: &str = "inference.session.InputCommitted";
const HISTORY_RECORDED: &str = "inference.session.HistoryRecorded";
const SESSION_INTERRUPTED: &str = "inference.session.SessionInterrupted";
const PIPELINE_SWITCHED: &str = "inference.session.PipelineSwitched";

/// Every event this module observes.
const EVENTS: [&str; 10] = [
    SESSION_OPENED,
    RUN_ATTACHED,
    SESSION_READY,
    SESSION_DRAINING,
    SESSION_CLOSED,
    INPUT_SENT,
    INPUT_COMMITTED,
    HISTORY_RECORDED,
    SESSION_INTERRUPTED,
    PIPELINE_SWITCHED,
];

const PIPELINE_NOT_FOUND: &str = "inference.graph.PipelineNotFound";
const PIPELINE_NOT_READY: &str = "inference.graph.PipelineNotReady";
const MODE_MISMATCH: &str = "inference.session.ModeMismatch";
const SESSION_NOT_FOUND: &str = "inference.session.SessionNotFound";
const SESSION_STATE_CONFLICT: &str = "inference.session.SessionStateConflict";
const PIPELINE_MISMATCH: &str = "inference.session.PipelineMismatch";
const NO_CURRENT_RUN: &str = "inference.session.NoCurrentRun";

const SESSIONS: &str = "inference.session.Sessions";
const HISTORY: &str = "inference.session.History";

/// The session store of one scenario.
pub type State = SessionDomain;

pub fn execute_command(
    stores: &mut Stores,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    stores.publish();
    let forced = stores.forced(&request.command);
    let graph = &stores.graph;
    let domain = &mut stores.session;
    let name = request.command.to_string();
    let result = match name.as_str() {
        OPEN_SESSION => {
            let mut input = Input::of(request);
            let open = OpenSession {
                pipeline_id: input.named("pipeline_id")?,
                mode: input.named("mode")?,
            };
            input.finish()?;
            match domain.open_session(graph, open) {
                Ok(event) => accepted(
                    OPEN_SESSION,
                    "opened",
                    &SessionEvent::SessionOpened(event),
                    request,
                )?,
                Err(OpenSessionError::UnknownPipeline(error)) => {
                    unknown_pipeline(OPEN_SESSION, error.pipeline_id)?
                }
                Err(OpenSessionError::NotPlanned(error)) => {
                    not_planned(OPEN_SESSION, error.pipeline_id)?
                }
                Err(OpenSessionError::ModeMismatch(error)) => {
                    mode_mismatch(OPEN_SESSION, error.pipeline_id, error.mode)?
                }
            }
        }
        SWITCH_PIPELINE => {
            let mut input = Input::of(request);
            let switch = SwitchPipeline {
                session_id: input.named("session_id")?,
                pipeline_id: input.named("pipeline_id")?,
                mode: input.named("mode")?,
            };
            input.finish()?;
            let SwitchPipeline {
                pipeline_id, mode, ..
            } = switch;
            match forced {
                Some(force) if force == outcome(SWITCH_PIPELINE, UNKNOWN_PIPELINE) => {
                    unknown_pipeline(SWITCH_PIPELINE, pipeline_id)?
                }
                Some(force) if force == outcome(SWITCH_PIPELINE, NOT_PLANNED) => {
                    not_planned(SWITCH_PIPELINE, pipeline_id)?
                }
                Some(_) => mode_mismatch(SWITCH_PIPELINE, pipeline_id, mode)?,
                None => match domain.switch_pipeline(graph, switch) {
                    Ok(event) => accepted(
                        SWITCH_PIPELINE,
                        "switched",
                        &SessionEvent::PipelineSwitched(event),
                        request,
                    )?,
                    Err(SwitchPipelineError::NoSuchSession(error)) => {
                        no_such_session(SWITCH_PIPELINE, error)?
                    }
                    Err(SwitchPipelineError::UnknownPipeline(error)) => {
                        unknown_pipeline(SWITCH_PIPELINE, error.pipeline_id)?
                    }
                    Err(SwitchPipelineError::NotPlanned(error)) => {
                        not_planned(SWITCH_PIPELINE, error.pipeline_id)?
                    }
                    Err(SwitchPipelineError::ModeMismatch(error)) => {
                        mode_mismatch(SWITCH_PIPELINE, error.pipeline_id, error.mode)?
                    }
                    Err(SwitchPipelineError::WrongState(conflict)) => {
                        wrong_state(SWITCH_PIPELINE, conflict)
                    }
                },
            }
        }
        ATTACH_RUN => {
            let mut input = Input::of(request);
            let attach = AttachRun {
                session_id: input.named("session_id")?,
                run_id: input.named("run_id")?,
            };
            input.finish()?;
            if forced.is_some() {
                pipeline_mismatch(attach.session_id, attach.run_id)?
            } else {
                match domain.attach_run(graph, attach) {
                    Ok(event) => accepted(
                        ATTACH_RUN,
                        "attached",
                        &SessionEvent::RunAttached(event),
                        request,
                    )?,
                    Err(AttachRunError::NoSuchSession(error)) => {
                        no_such_session(ATTACH_RUN, error)?
                    }
                    Err(AttachRunError::PipelineMismatch(error)) => {
                        pipeline_mismatch(error.session_id, error.run_id)?
                    }
                    Err(AttachRunError::WrongState(conflict)) => wrong_state(ATTACH_RUN, conflict),
                }
            }
        }
        MARK_SESSION_READY => {
            let session_id = session_id(request)?;
            match domain.mark_session_ready(session_id) {
                Ok(event) => accepted(
                    MARK_SESSION_READY,
                    "ready",
                    &SessionEvent::SessionReady(event),
                    request,
                )?,
                Err(MarkSessionReadyError::NoSuchSession(error)) => {
                    no_such_session(MARK_SESSION_READY, error)?
                }
                Err(MarkSessionReadyError::NoCurrentRun(error)) => refused(
                    MARK_SESSION_READY,
                    "no-current-run",
                    DeclaredErrorValue::new(parse::<ErrorRef>(NO_CURRENT_RUN))
                        .with("session_id", named_node(&error.session_id)?),
                ),
                Err(MarkSessionReadyError::WrongState(conflict)) => {
                    wrong_state(MARK_SESSION_READY, conflict)
                }
            }
        }
        CLOSE_SESSION => {
            let session_id = session_id(request)?;
            let answer = domain
                .close_session(session_id)
                .map(SessionEvent::SessionDraining);
            moved(CLOSE_SESSION, "draining", answer, request)?
        }
        COMPLETE_SESSION_CLOSE => {
            let session_id = session_id(request)?;
            let answer = domain
                .complete_session_close(session_id)
                .map(SessionEvent::SessionClosed);
            moved(COMPLETE_SESSION_CLOSE, "closed", answer, request)?
        }
        RECORD_HISTORY => {
            let mut input = Input::of(request);
            let record = RecordHistory {
                session_id: input.named("session_id")?,
                run_id: input.named("run_id")?,
                position: input.integer("position")?,
                kind: history_kind(&mut input)?,
                text: input.text("text")?,
                call_id: optional_call(&mut input)?,
            };
            input.finish()?;
            match domain.record_history(record) {
                Ok(event) => accepted(
                    RECORD_HISTORY,
                    "recorded",
                    &SessionEvent::HistoryRecorded(event),
                    request,
                )?,
                Err(RecordHistoryError::NoSuchSession(error)) => {
                    no_such_session(RECORD_HISTORY, error)?
                }
                Err(RecordHistoryError::Closed(conflict)) => refused(
                    RECORD_HISTORY,
                    "closed",
                    DeclaredErrorValue::new(parse::<ErrorRef>(SESSION_STATE_CONFLICT))
                        .with("current", text_node(conflict.current.as_str())),
                ),
            }
        }
        INTERRUPT => {
            let session_id = session_id(request)?;
            let answer = domain
                .interrupt(session_id)
                .map(SessionEvent::SessionInterrupted);
            moved(INTERRUPT, "interrupted", answer, request)?
        }
        CANCEL_SESSION => {
            let session_id = session_id(request)?;
            let answer = domain
                .cancel_session(session_id)
                .map(SessionEvent::SessionClosed);
            moved(CANCEL_SESSION, "cancelled", answer, request)?
        }
        SEND_INPUT => {
            let session_id = session_id(request)?;
            let answer = domain.send_input(session_id).map(SessionEvent::InputSent);
            moved(SEND_INPUT, "sent", answer, request)?
        }
        COMMIT_INPUT => {
            let session_id = session_id(request)?;
            let answer = domain
                .commit_input(session_id)
                .map(SessionEvent::InputCommitted);
            moved(COMMIT_INPUT, "committed", answer, request)?
        }
        _ => return Err(unsupported(&name)),
    };
    stores.publish();
    Ok(result.with_consistency(stores.token()?))
}

/// The one field of a command that names a session and nothing else.
fn session_id(request: &SemanticCommandRequest) -> Result<SessionId, TargetError> {
    let mut input = Input::of(request);
    let session_id = input.named("session_id")?;
    input.finish()?;
    Ok(session_id)
}

/// Accepts a force of `AttachRun/pipeline-mismatch`, the external outcome of the commands this
/// module answers; refuses a force of another `AttachRun` outcome as an error, as no
/// specification-declared external outcome, and any other command's as unsupported.
pub fn configure_external_outcome(
    stores: &mut Stores,
    request: &ExternalOutcomeControl,
) -> Result<(), TargetError> {
    let command = request.force.command.to_string();
    if command != ATTACH_RUN && command != SWITCH_PIPELINE {
        return Err(unsupported(&request.force.to_string()));
    }
    let external = [
        outcome(ATTACH_RUN, PIPELINE_MISMATCH_OUTCOME),
        outcome(SWITCH_PIPELINE, UNKNOWN_PIPELINE),
        outcome(SWITCH_PIPELINE, NOT_PLANNED),
        outcome(SWITCH_PIPELINE, MODE_MISMATCH_OUTCOME),
    ];
    if !external.contains(&request.force) {
        return Err(unreadable(
            format!("forcing `{}`", request.force),
            "it is not an outcome `ess/domains/session.yaml` declares external",
        ));
    }
    stores.force(request.force.clone());
    Ok(())
}

/// The accepting outcome `branch` of `command`, emitting `event`.
fn accepted(
    command: &str,
    branch: &str,
    event: &SessionEvent,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    Ok(SemanticCommandResult::took(outcome(command, branch))
        .emitting(session_event(event)?.in_activity(request.correlation.clone())))
}

/// The refusing outcome `branch` of `command`, carrying `error`.
fn refused(command: &str, branch: &str, error: DeclaredErrorValue) -> SemanticCommandResult {
    SemanticCommandResult::took(outcome(command, branch)).with_error(error)
}

/// The `no-such-session` outcome of `command`: `SessionNotFound`.
fn no_such_session(
    command: &str,
    error: SessionNotFound,
) -> Result<SemanticCommandResult, TargetError> {
    Ok(refused(
        command,
        "no-such-session",
        DeclaredErrorValue::new(parse::<ErrorRef>(SESSION_NOT_FOUND))
            .with("session_id", named_node(&error.session_id)?),
    ))
}

/// The `unknown-pipeline` outcome of `command`: `PipelineNotFound`.
fn unknown_pipeline(
    command: &str,
    pipeline_id: PipelineId,
) -> Result<SemanticCommandResult, TargetError> {
    Ok(refused(
        command,
        UNKNOWN_PIPELINE,
        DeclaredErrorValue::new(parse::<ErrorRef>(PIPELINE_NOT_FOUND))
            .with("pipeline_id", named_node(&pipeline_id)?),
    ))
}

/// The `not-planned` outcome of `command`: `PipelineNotReady`, requiring Planned.
fn not_planned(
    command: &str,
    pipeline_id: PipelineId,
) -> Result<SemanticCommandResult, TargetError> {
    Ok(refused(
        command,
        NOT_PLANNED,
        DeclaredErrorValue::new(parse::<ErrorRef>(PIPELINE_NOT_READY))
            .with("pipeline_id", named_node(&pipeline_id)?)
            .with("required", named_node(&PipelineState::Planned)?),
    ))
}

/// The `mode-mismatch` outcome of `command`: `ModeMismatch`.
fn mode_mismatch(
    command: &str,
    pipeline_id: PipelineId,
    mode: InteractionMode,
) -> Result<SemanticCommandResult, TargetError> {
    Ok(refused(
        command,
        MODE_MISMATCH_OUTCOME,
        DeclaredErrorValue::new(parse::<ErrorRef>(MODE_MISMATCH))
            .with("pipeline_id", named_node(&pipeline_id)?)
            .with("mode", named_node(&mode)?),
    ))
}

/// The `pipeline-mismatch` outcome of `AttachRun`: `PipelineMismatch`.
fn pipeline_mismatch(
    session_id: SessionId,
    run_id: RunId,
) -> Result<SemanticCommandResult, TargetError> {
    Ok(refused(
        ATTACH_RUN,
        PIPELINE_MISMATCH_OUTCOME,
        DeclaredErrorValue::new(parse::<ErrorRef>(PIPELINE_MISMATCH))
            .with("session_id", named_node(&session_id)?)
            .with("run_id", named_node(&run_id)?),
    ))
}

/// The `wrong-state` outcome of `command`: `SessionStateConflict`, carrying the state the session
/// holds.
fn wrong_state(command: &str, conflict: SessionStateConflict) -> SemanticCommandResult {
    refused(
        command,
        "wrong-state",
        DeclaredErrorValue::new(parse::<ErrorRef>(SESSION_STATE_CONFLICT))
            .with("current", text_node(conflict.current.as_str())),
    )
}

/// The answer of a command whose refusals are `no-such-session` and `wrong-state` alone: `branch`
/// and its event, `no-such-session` or `wrong-state`.
fn moved(
    command: &str,
    branch: &str,
    answer: Result<SessionEvent, SessionMoveError>,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    match answer {
        Ok(event) => accepted(command, branch, &event, request),
        Err(SessionMoveError::NoSuchSession(error)) => no_such_session(command, error),
        Err(SessionMoveError::WrongState(conflict)) => Ok(wrong_state(command, conflict)),
    }
}

pub fn query_view(
    stores: &mut Stores,
    request: &SemanticViewRequest,
) -> Result<SemanticViewResult, TargetError> {
    let name = request.view.to_string();
    if name != SESSIONS && name != HISTORY {
        return Err(unsupported(&name));
    }
    stores.require(&name, &request.consistency)?;
    let rows = if name == SESSIONS {
        stores
            .session
            .sessions_view()
            .iter()
            .map(sessions_row)
            .collect::<Result<Vec<_>, _>>()?
    } else {
        stores
            .session
            .history_view()
            .iter()
            .map(history_row)
            .collect::<Result<Vec<_>, _>>()?
    };
    Ok(SemanticViewResult::of(rows))
}

fn history_row(entry: &HistoryEntry) -> Result<ViewRow, TargetError> {
    Ok(BTreeMap::from([
        ("entry_id".to_owned(), named_node(&entry.entry_id())?),
        ("session_id".to_owned(), named_node(&entry.session_id())?),
        ("run_id".to_owned(), named_node(&entry.run_id())?),
        ("position".to_owned(), integer_node(entry.position())),
        ("kind".to_owned(), text_node(entry.kind().as_str())),
        ("text".to_owned(), text_node(entry.text())),
        (
            "call_id".to_owned(),
            entry
                .call_id()
                .as_ref()
                .map_or(Ok(Node::Null), named_node)?,
        ),
    ]))
}

/// An `inference.session.HistoryKind` read from its ESS variant.
fn history_kind(input: &mut Input<'_>) -> Result<HistoryKind, TargetError> {
    let kind = input.text("kind")?;
    HistoryKind::ALL
        .into_iter()
        .find(|candidate| candidate.as_str() == kind)
        .ok_or_else(|| {
            unreadable(
                format!("reading `{RECORD_HISTORY}`"),
                format!("`kind` is not a history kind: {kind}"),
            )
        })
}

/// An `Optional<inference.tools.ToolCallId>`: absent when the input leaves it out or sends null.
fn optional_call(input: &mut Input<'_>) -> Result<Option<ToolCallId>, TargetError> {
    match input.optional("call_id") {
        None => Ok(None),
        Some(Node::Text(text)) => serde_json::from_value(serde_json::Value::String(text.clone()))
            .map(Some)
            .map_err(|error| {
                unreadable(
                    format!("reading `{RECORD_HISTORY}`"),
                    format!("`call_id` is not a tool call identity: {error}"),
                )
            }),
        other => Err(unreadable(
            format!("reading `{RECORD_HISTORY}`"),
            format!("`call_id` is not a tool call identity: {other:?}"),
        )),
    }
}

pub fn observe_events(
    stores: &mut Stores,
    request: &EventObservationRequest,
) -> Result<Vec<ObservedEvent>, TargetError> {
    let wanted = request.event.to_string();
    if !EVENTS.contains(&wanted.as_str()) {
        return Err(unsupported(&wanted));
    }
    stores.publish();
    let mut observed = Vec::new();
    for (index, event) in stores.session.events().iter().enumerate() {
        let event = session_event(event)?;
        if event.event == request.event {
            let position = stores.session_position(index)?;
            observed.push(event.in_activity(request.correlation.clone()).at(position));
        }
    }
    Ok(observed)
}

/// An event of the store as the suite observes it, every declared field carried.
fn session_event(event: &SessionEvent) -> Result<ObservedEvent, TargetError> {
    let session = |name: &str, session_id: SessionId| {
        Ok::<_, TargetError>(
            ObservedEvent::new(parse::<EventRef>(name))
                .with("session_id", named_node(&session_id)?),
        )
    };
    Ok(match event {
        SessionEvent::SessionOpened(event) => session(SESSION_OPENED, event.session_id)?
            .with("pipeline_id", named_node(&event.pipeline_id)?),
        SessionEvent::RunAttached(event) => {
            session(RUN_ATTACHED, event.session_id)?.with("run_id", named_node(&event.run_id)?)
        }
        SessionEvent::SessionReady(event) => session(SESSION_READY, event.session_id)?,
        SessionEvent::SessionDraining(event) => session(SESSION_DRAINING, event.session_id)?,
        SessionEvent::SessionClosed(event) => session(SESSION_CLOSED, event.session_id)?,
        SessionEvent::InputSent(event) => session(INPUT_SENT, event.session_id)?,
        SessionEvent::InputCommitted(event) => session(INPUT_COMMITTED, event.session_id)?,
        SessionEvent::SessionInterrupted(event) => session(SESSION_INTERRUPTED, event.session_id)?,
        SessionEvent::PipelineSwitched(event) => session(PIPELINE_SWITCHED, event.session_id)?
            .with("pipeline_id", named_node(&event.pipeline_id)?),
        SessionEvent::HistoryRecorded(event) => session(HISTORY_RECORDED, event.session_id)?
            .with("entry_id", named_node(&event.entry_id)?)
            .with("run_id", named_node(&event.run_id)?)
            .with("position", integer_node(event.position))
            .with("kind", text_node(event.kind.as_str())),
        other => {
            return Err(unreadable(
                format!("the session event {other:?}"),
                "the store records the session's own events only",
            ));
        }
    })
}

fn sessions_row(row: &SessionsRow) -> Result<ViewRow, TargetError> {
    Ok(BTreeMap::from([
        ("session_id".to_owned(), named_node(&row.session_id)?),
        ("pipeline_id".to_owned(), named_node(&row.pipeline_id)?),
        ("mode".to_owned(), named_node(&row.mode)?),
        (
            "run_id".to_owned(),
            row.run_id.as_ref().map_or(Ok(Node::Null), named_node)?,
        ),
        ("state".to_owned(), text_node(row.state.as_str())),
    ]))
}

fn outcome(command: &str, branch: &str) -> OutcomeRef {
    OutcomeRef::new(
        parse::<CommandRef>(command),
        branch
            .parse()
            .unwrap_or_else(|error| panic!("`{branch}` is an outcome name: {error}")),
    )
}

/// Parses a name this module writes as a literal; a malformed one is a defect here.
fn parse<T>(name: &str) -> T
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    name.parse()
        .unwrap_or_else(|error| panic!("`{name}` is a well-formed name: {error}"))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ess_conformance::scenario::{CommandRef, EventRef, OutcomeRef, ViewRef};
    use ess_conformance::target::{
        Deadline, EventObservationRequest, ExternalOutcomeControl, ObservedEvent,
        SemanticCommandRequest, SemanticCommandResult, SemanticViewRequest, TargetError,
    };
    use ess_primitives::consistency::QueryConsistency;
    use ess_primitives::ids::CorrelationId;
    use ess_primitives::node::Node;
    use ess_primitives::time::Timestamp;
    use inference_core::model::Locality;
    use inference_core::policy::{DeclareProfile, Egress, ProfileKind};
    use inference_pipeline::{DeclarePipeline, InteractionMode, PlanPipeline, ValidatePipeline};
    use inference_testkit::graph::GraphCatalog;

    use super::super::Stores;
    use super::{configure_external_outcome, execute_command, observe_events, query_view};

    const UNKNOWN: &str = "00000005-0000-4000-8000-6eced0bc8ba0";

    fn correlation() -> CorrelationId {
        CorrelationId::new("session").expect("a correlation id")
    }

    fn text(value: &str) -> Node {
        Node::Text(value.to_owned())
    }

    fn command(name: &str, input: &[(&str, Node)]) -> SemanticCommandRequest {
        SemanticCommandRequest {
            command: name.parse::<CommandRef>().expect("a command name"),
            actor: None,
            caller: None,
            input: input
                .iter()
                .map(|(field, value)| ((*field).to_owned(), value.clone()))
                .collect(),
            correlation: correlation(),
        }
    }

    fn run(stores: &mut Stores, name: &str, input: &[(&str, Node)]) -> SemanticCommandResult {
        execute_command(stores, &command(name, input))
            .unwrap_or_else(|error| panic!("`{name}` answers: {error}"))
    }

    fn branch(result: &SemanticCommandResult) -> String {
        result
            .outcome
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default()
    }

    fn sessions(stores: &mut Stores) -> Vec<BTreeMap<String, Node>> {
        query_view(
            stores,
            &SemanticViewRequest {
                view: "inference.session.Sessions"
                    .parse::<ViewRef>()
                    .expect("a view name"),
                params: BTreeMap::new(),
                consistency: QueryConsistency::Current,
                correlation: correlation(),
                deadline: Deadline::at(Timestamp::from_epoch_millis(u64::MAX)),
            },
        )
        .unwrap_or_else(|error| panic!("`Sessions` reads: {error}"))
        .rows
    }

    fn observed(stores: &mut Stores, name: &str) -> Vec<ObservedEvent> {
        observe_events(
            stores,
            &EventObservationRequest {
                event: name.parse::<EventRef>().expect("an event name"),
                correlation: correlation(),
                deadline: Deadline::at(Timestamp::from_epoch_millis(u64::MAX)),
            },
        )
        .unwrap_or_else(|error| panic!("`{name}` is observed: {error}"))
    }

    /// Two Planned batch pipelines in the graph store, declared and planned past the target, and
    /// a run of each; returns the pipelines and the runs as the runner captures them.
    fn planned(stores: &mut Stores) -> [Node; 4] {
        let profile_id = stores
            .policy
            .declare_profile(DeclareProfile {
                kind: ProfileKind::OfflineStrict,
                egress: Egress::Denied,
                admitted: vec![Locality::SameProcess],
            })
            .expect("a row of the table")
            .profile_id;
        let mut ids = Vec::new();
        for name in ["first", "second"] {
            let catalog = GraphCatalog {
                policy: &stores.policy,
                model: &stores.model,
            };
            let pipeline_id = stores
                .graph
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
                .expect("declared")
                .pipeline_id;
            stores
                .graph
                .validate_pipeline(ValidatePipeline { pipeline_id })
                .expect("validates");
            stores
                .graph
                .plan_pipeline(PlanPipeline { pipeline_id }, &catalog)
                .expect("plans");
            ids.push(pipeline_id);
        }
        let first = stores.graph.create_run(ids[0]).expect("planned").run_id;
        let second = stores.graph.create_run(ids[1]).expect("planned").run_id;
        [
            text(&ids[0].to_string()),
            text(&ids[1].to_string()),
            text(&first.to_string()),
            text(&second.to_string()),
        ]
    }

    fn opened(stores: &mut Stores, pipeline_id: Node) -> Node {
        let result = run(
            stores,
            "inference.session.OpenSession",
            &[("pipeline_id", pipeline_id), ("mode", text("batch"))],
        );
        assert_eq!(
            branch(&result),
            "inference.session.OpenSession/opened",
            "{result:?}"
        );
        result.direct_events[0].payload["session_id"].clone()
    }

    fn attach(stores: &mut Stores, session_id: &Node, run_id: &Node) -> SemanticCommandResult {
        run(
            stores,
            "inference.session.AttachRun",
            &[
                ("session_id", session_id.clone()),
                ("run_id", run_id.clone()),
            ],
        )
    }

    #[test]
    fn a_session_holds_its_current_run_in_the_view_and_the_events() {
        let mut stores = Stores::default();
        let [first, _, run_id, _] = planned(&mut stores);
        let session_id = opened(&mut stores, first.clone());
        assert_eq!(
            session_id,
            text("00000005-0000-4000-8000-000000000001"),
            "the session counts in its own space"
        );
        let row = &sessions(&mut stores)[0];
        assert_eq!(row["run_id"], Node::Null, "no current run yet");
        assert_eq!(row["state"], text("Opened"));
        assert_eq!(row["mode"], text("batch"));
        assert_eq!(row["pipeline_id"], first);

        let refused = run(
            &mut stores,
            "inference.session.MarkSessionReady",
            &[("session_id", session_id.clone())],
        );
        assert_eq!(
            branch(&refused),
            "inference.session.MarkSessionReady/no-current-run"
        );
        let carried = refused.error.expect("a declared error");
        assert_eq!(carried.error.to_string(), "inference.session.NoCurrentRun");
        assert_eq!(carried.fields["session_id"], session_id);

        let attached = attach(&mut stores, &session_id, &run_id);
        assert_eq!(
            branch(&attached),
            "inference.session.AttachRun/attached",
            "{attached:?}"
        );
        let ready = run(
            &mut stores,
            "inference.session.MarkSessionReady",
            &[("session_id", session_id.clone())],
        );
        assert_eq!(branch(&ready), "inference.session.MarkSessionReady/ready");
        let row = &sessions(&mut stores)[0];
        assert_eq!(row["run_id"], run_id);
        assert_eq!(row["state"], text("Ready"));

        let events = observed(&mut stores, "inference.session.RunAttached");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].payload["run_id"], run_id);
        assert_eq!(events[0].payload["session_id"], session_id);
        assert_eq!(
            events[0].sequence,
            Some(u64::try_from(stores.graph.events().len() + 3).expect("small")),
            "placed after the policy and graph events recorded before it and the session's opening"
        );
    }

    #[test]
    fn a_run_of_another_pipeline_is_refused_and_moves_nothing() {
        let mut stores = Stores::default();
        let [first, _, _, foreign] = planned(&mut stores);
        let session_id = opened(&mut stores, first);
        let recorded = stores.session.events().len();
        let result = attach(&mut stores, &session_id, &foreign);
        assert_eq!(
            branch(&result),
            "inference.session.AttachRun/pipeline-mismatch"
        );
        let carried = result.error.expect("a declared error");
        assert_eq!(
            carried.error.to_string(),
            "inference.session.PipelineMismatch"
        );
        assert_eq!(carried.fields["session_id"], session_id);
        assert_eq!(carried.fields["run_id"], foreign);
        assert!(result.direct_events.is_empty());
        assert_eq!(stores.session.events().len(), recorded);
        assert_eq!(sessions(&mut stores)[0]["run_id"], Node::Null);
    }

    #[test]
    fn a_forced_mismatch_answers_for_an_unknown_session_and_lapses() {
        let mut stores = Stores::default();
        let force = |stores: &mut Stores, branch: &str| {
            configure_external_outcome(
                stores,
                &ExternalOutcomeControl {
                    force: OutcomeRef::new(
                        "inference.session.AttachRun"
                            .parse::<CommandRef>()
                            .expect("a command name"),
                        branch.parse().expect("an outcome name"),
                    ),
                    correlation: correlation(),
                },
            )
        };
        assert!(matches!(
            force(&mut stores, "attached"),
            Err(TargetError::Unavailable { .. })
        ));
        force(&mut stores, "pipeline-mismatch").expect("pipeline-mismatch is external");
        let forced = attach(&mut stores, &text(UNKNOWN), &text(UNKNOWN));
        assert_eq!(
            branch(&forced),
            "inference.session.AttachRun/pipeline-mismatch"
        );
        let carried = forced.error.expect("a declared error");
        assert_eq!(carried.fields["session_id"], text(UNKNOWN));
        assert_eq!(carried.fields["run_id"], text(UNKNOWN));

        let lapsed = attach(&mut stores, &text(UNKNOWN), &text(UNKNOWN));
        assert_eq!(
            branch(&lapsed),
            "inference.session.AttachRun/no-such-session",
            "the force lapses with the invocation it answered"
        );
        assert!(stores.session.events().is_empty());
    }

    #[test]
    fn an_undeclared_or_malformed_field_is_an_error_and_moves_nothing() {
        let mut stores = Stores::default();
        let [first, _, run_id, _] = planned(&mut stores);
        let session_id = opened(&mut stores, first.clone());
        let recorded = stores.session.events().len();
        for (name, input) in [
            (
                "inference.session.OpenSession",
                vec![
                    ("pipeline_id", first.clone()),
                    ("mode", text("batch")),
                    ("run_id", run_id.clone()),
                ],
            ),
            (
                "inference.session.OpenSession",
                vec![("pipeline_id", first), ("mode", text("chatty"))],
            ),
            (
                "inference.session.AttachRun",
                vec![("session_id", session_id.clone())],
            ),
            (
                "inference.session.AttachRun",
                vec![
                    ("session_id", session_id.clone()),
                    ("run_id", run_id.clone()),
                    ("pipeline_id", text(UNKNOWN)),
                ],
            ),
            (
                "inference.session.CloseSession",
                vec![
                    ("session_id", session_id.clone()),
                    ("cancel", Node::Bool(true)),
                ],
            ),
            (
                "inference.session.MarkSessionReady",
                vec![("session_id", Node::Bool(true))],
            ),
        ] {
            let answer = execute_command(&mut stores, &command(name, &input));
            assert!(
                matches!(answer, Err(TargetError::Unavailable { .. })),
                "{name} {input:?}: {answer:?}"
            );
        }
        assert_eq!(stores.session.events().len(), recorded);
        assert_eq!(sessions(&mut stores)[0]["state"], text("Opened"));
    }
}
