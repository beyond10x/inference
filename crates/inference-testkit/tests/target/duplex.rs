//! `inference.duplex`: the native speech session commands, views and events of
//! `ess/domains/duplex.yaml`, answered by [`DuplexDomain`].
//!
//! Each request field is read on its own ([`Input`]); the outcome, the error and the event come
//! from what the store returned. `OpenSpeechSession` reads the adapters of the `inference.model`
//! store and `ProposeSpeechToolCall` the tools of the `inference.tools` store. A move naming a
//! session, response or call no record carries takes its `wrong-state` outcome, whose error then
//! carries no `current`.
//!
//! A format or a frame is read field by field. One whose sample rate, channel count, sample count
//! or position an `inference-audio` value cannot hold takes the `format-out-of-range` outcome,
//! after the outcomes declared before it, which read the stores; any other shape is an error. A
//! frame's `data` is ESS `Bytes`, the padded base64 string `inference-audio` reads and writes.
//! `Json` fields are carried as sent; usage and failure records are read with every field
//! declared and nothing else.
//!
//! A command's consistency token is the target's ([`Stores::token`]), and a read demanding a token
//! the stores have not reached is an error ([`Stores::require`]). An observed event's position is
//! its place in the target's one publication order ([`Stores::duplex_position`]).

use std::collections::BTreeMap;

use ess_conformance::scenario::{CommandRef, ErrorRef, EventRef, OutcomeRef};
use ess_conformance::target::{
    DeclaredErrorValue, EventObservationRequest, ObservedEvent, SemanticCommandRequest,
    SemanticCommandResult, SemanticViewRequest, SemanticViewResult, TargetError, ViewRow,
};
use ess_primitives::node::Node;
use inference_duplex::inference_audio::{AudioFormat, AudioFrame, SampleClockPosition};
use inference_duplex::{
    OpenSpeechSession, ProposeSpeechToolCall, ResponseId, ResponseState, ResponseUsage,
    SessionFailure, SpeechCallId, SpeechSessionId, SpeechSessionState, SpeechToolCallState,
    TranscriptionUsage,
};
use inference_testkit::duplex::{
    AnswerSpeechToolCallError, DuplexDomain, DuplexEvent, OpenSpeechSessionError,
    ProposeSpeechToolCallError, ResponseCommandError, ResponsesRow, SessionCommandError,
    SpeechSessionsRow, SpeechToolCallsRow,
};
use inference_tools::Json;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{
    Input, Stores, boolean_node, integer, integer_node, named_list_node, named_node, text_node,
    unreadable, unsupported,
};

/// The prefix of every name this module answers.
pub const DOMAIN: &str = "inference.duplex";

const OPEN_SPEECH_SESSION: &str = "inference.duplex.OpenSpeechSession";
const APPEND_INPUT_AUDIO: &str = "inference.duplex.AppendInputAudio";
const COMMIT_INPUT: &str = "inference.duplex.CommitInput";
const OBSERVE_SPEECH_STARTED: &str = "inference.duplex.ObserveSpeechStarted";
const OBSERVE_SPEECH_STOPPED: &str = "inference.duplex.ObserveSpeechStopped";
const TRANSCRIBE_INPUT: &str = "inference.duplex.TranscribeInput";
const RECORD_TRANSCRIPTION_USAGE: &str = "inference.duplex.RecordTranscriptionUsage";
const CREATE_RESPONSE: &str = "inference.duplex.CreateResponse";
const EMIT_OUTPUT_AUDIO: &str = "inference.duplex.EmitOutputAudio";
const EMIT_OUTPUT_TRANSCRIPT: &str = "inference.duplex.EmitOutputTranscript";
const COMPLETE_RESPONSE: &str = "inference.duplex.CompleteResponse";
const CANCEL_RESPONSE: &str = "inference.duplex.CancelResponse";
const FAIL_RESPONSE: &str = "inference.duplex.FailResponse";
const TRUNCATE_OUTPUT: &str = "inference.duplex.TruncateOutput";
const RECORD_RESPONSE_USAGE: &str = "inference.duplex.RecordResponseUsage";
const PROPOSE_SPEECH_TOOL_CALL: &str = "inference.duplex.ProposeSpeechToolCall";
const ANSWER_SPEECH_TOOL_CALL: &str = "inference.duplex.AnswerSpeechToolCall";
const CLOSE_SPEECH_SESSION: &str = "inference.duplex.CloseSpeechSession";
const FAIL_SPEECH_SESSION: &str = "inference.duplex.FailSpeechSession";

const SPEECH_SESSION_OPENED: &str = "inference.duplex.SpeechSessionOpened";
const INPUT_ACCEPTED: &str = "inference.duplex.InputAccepted";
const INPUT_COMMITTED: &str = "inference.duplex.InputCommitted";
const SPEECH_STARTED: &str = "inference.duplex.SpeechStarted";
const SPEECH_STOPPED: &str = "inference.duplex.SpeechStopped";
const INPUT_TRANSCRIBED: &str = "inference.duplex.InputTranscribed";
const TRANSCRIPTION_USAGE_RECORDED: &str = "inference.duplex.TranscriptionUsageRecorded";
const RESPONSE_CREATED: &str = "inference.duplex.ResponseCreated";
const OUTPUT_AUDIO: &str = "inference.duplex.OutputAudio";
const OUTPUT_TRANSCRIPT_DELTA: &str = "inference.duplex.OutputTranscriptDelta";
const RESPONSE_COMPLETED: &str = "inference.duplex.ResponseCompleted";
const RESPONSE_CANCELLED: &str = "inference.duplex.ResponseCancelled";
const RESPONSE_FAILED: &str = "inference.duplex.ResponseFailed";
const OUTPUT_TRUNCATED: &str = "inference.duplex.OutputTruncated";
const RESPONSE_USAGE_RECORDED: &str = "inference.duplex.ResponseUsageRecorded";
const SPEECH_TOOL_CALL_PROPOSED: &str = "inference.duplex.SpeechToolCallProposed";
const SPEECH_TOOL_CALL_ANSWERED: &str = "inference.duplex.SpeechToolCallAnswered";
const SPEECH_SESSION_CLOSED: &str = "inference.duplex.SpeechSessionClosed";
const SPEECH_SESSION_FAILED: &str = "inference.duplex.SpeechSessionFailed";

/// Every event this module observes.
const EVENTS: [&str; 19] = [
    SPEECH_SESSION_OPENED,
    INPUT_ACCEPTED,
    INPUT_COMMITTED,
    SPEECH_STARTED,
    SPEECH_STOPPED,
    INPUT_TRANSCRIBED,
    TRANSCRIPTION_USAGE_RECORDED,
    RESPONSE_CREATED,
    OUTPUT_AUDIO,
    OUTPUT_TRANSCRIPT_DELTA,
    RESPONSE_COMPLETED,
    RESPONSE_CANCELLED,
    RESPONSE_FAILED,
    OUTPUT_TRUNCATED,
    RESPONSE_USAGE_RECORDED,
    SPEECH_TOOL_CALL_PROPOSED,
    SPEECH_TOOL_CALL_ANSWERED,
    SPEECH_SESSION_CLOSED,
    SPEECH_SESSION_FAILED,
];

const ADAPTER_NOT_FOUND: &str = "inference.duplex.AdapterNotFound";
const NON_POSITIVE_IDLE_LIMIT: &str = "inference.duplex.NonPositiveIdleLimit";
const FORMAT_OUT_OF_RANGE: &str = "inference.duplex.FormatOutOfRange";
const SPEECH_SESSION_NOT_FOUND: &str = "inference.duplex.SpeechSessionNotFound";
const SPEECH_SESSION_STATE_CONFLICT: &str = "inference.duplex.SpeechSessionStateConflict";
const RESPONSE_NOT_FOUND: &str = "inference.duplex.ResponseNotFound";
const RESPONSE_STATE_CONFLICT: &str = "inference.duplex.ResponseStateConflict";
const SPEECH_TOOL_CALL_STATE_CONFLICT: &str = "inference.duplex.SpeechToolCallStateConflict";
const TOOL_NOT_FOUND: &str = "inference.tools.ToolNotFound";

const SPEECH_SESSIONS: &str = "inference.duplex.SpeechSessions";
const RESPONSES: &str = "inference.duplex.Responses";
const SPEECH_TOOL_CALLS: &str = "inference.duplex.SpeechToolCalls";

/// The session, response and call stores of one scenario.
pub type State = DuplexDomain;

pub fn execute_command(
    stores: &mut Stores,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    stores.publish();
    let name = request.command.to_string();
    let command = name.as_str();
    let mut input = Input::of(request);
    let result = match command {
        OPEN_SPEECH_SESSION => {
            let adapter_id = input.named("adapter_id")?;
            let voice = input.text("voice")?;
            let instructions = input.text("instructions")?;
            let input_format = audio_format(command, input.node("input_format")?)?;
            let output_format = audio_format(command, input.node("output_format")?)?;
            let turn_detection = input.named("turn_detection")?;
            let auto_response = input.boolean("auto_response")?;
            let turn_silence_ms = optional_integer(command, input.optional("turn_silence_ms"))?;
            let turn_sensitivity = input
                .optional("turn_sensitivity")
                .map(|node| declared(command, "turn_sensitivity", node))
                .transpose()?;
            let turn_prefix_padding_ms =
                optional_integer(command, input.optional("turn_prefix_padding_ms"))?;
            let auto_interrupt = input.boolean("auto_interrupt")?;
            let tools = input.named_list("tools")?;
            let idle_limit_ms = input.integer("idle_limit_ms")?;
            input.finish()?;
            let (Some(input_format), Some(output_format)) = (input_format, output_format) else {
                let refusal = if stores.model.adapters().get(adapter_id).is_none() {
                    adapter_not_found(adapter_id)?
                } else {
                    out_of_range(command)
                };
                return answered(stores, refusal);
            };
            let record = OpenSpeechSession {
                adapter_id,
                voice,
                instructions,
                input_format,
                output_format,
                turn_detection,
                auto_response,
                turn_silence_ms,
                turn_sensitivity,
                turn_prefix_padding_ms,
                auto_interrupt,
                tools,
                idle_limit_ms,
            };
            match stores.duplex.open_speech_session(&stores.model, record) {
                Ok(event) => accepted(
                    command,
                    "opened",
                    &DuplexEvent::SpeechSessionOpened(event),
                    request,
                )?,
                Err(OpenSpeechSessionError::UnknownAdapter(error)) => {
                    adapter_not_found(error.adapter_id)?
                }
                Err(OpenSpeechSessionError::NoLivenessBound(error)) => refused(
                    command,
                    "no-liveness-bound",
                    DeclaredErrorValue::new(parse::<ErrorRef>(NON_POSITIVE_IDLE_LIMIT))
                        .with("submitted", integer_node(error.submitted)),
                ),
            }
        }
        APPEND_INPUT_AUDIO => {
            let session_id: SpeechSessionId = input.named("session_id")?;
            let frame = audio_frame(command, input.node("frame")?)?;
            input.finish()?;
            match frame {
                Some(frame) => session_answer(
                    command,
                    "accepted",
                    stores
                        .duplex
                        .append_input_audio(session_id, frame)
                        .map(DuplexEvent::InputAccepted),
                    request,
                )?,
                None => match stores.duplex.session(session_id) {
                    None => session_answer(
                        command,
                        "accepted",
                        Err(SessionCommandError::Unknown(
                            inference_duplex::SpeechSessionNotFound { session_id },
                        )),
                        request,
                    )?,
                    Some(session) => match session.require_open() {
                        Err(conflict) => session_answer(
                            command,
                            "accepted",
                            Err(SessionCommandError::Conflict(conflict)),
                            request,
                        )?,
                        Ok(()) => out_of_range(command),
                    },
                },
            }
        }
        COMMIT_INPUT => {
            let session_id = input.named("session_id")?;
            input.finish()?;
            let answer = stores.duplex.commit_input(session_id);
            session_answer(
                command,
                "committed",
                answer.map(DuplexEvent::InputCommitted),
                request,
            )?
        }
        OBSERVE_SPEECH_STARTED | OBSERVE_SPEECH_STOPPED => {
            let session_id = input.named("session_id")?;
            let position = position(&mut input, "position")?;
            input.finish()?;
            let answer = if command == OBSERVE_SPEECH_STARTED {
                stores
                    .duplex
                    .observe_speech_started(session_id, position)
                    .map(DuplexEvent::SpeechStarted)
            } else {
                stores
                    .duplex
                    .observe_speech_stopped(session_id, position)
                    .map(DuplexEvent::SpeechStopped)
            };
            session_answer(command, "observed", answer, request)?
        }
        TRANSCRIBE_INPUT => {
            let session_id = input.named("session_id")?;
            let text = input.text("text")?;
            input.finish()?;
            let answer = stores.duplex.transcribe_input(session_id, text);
            session_answer(
                command,
                "transcribed",
                answer.map(DuplexEvent::InputTranscribed),
                request,
            )?
        }
        RECORD_TRANSCRIPTION_USAGE => {
            let session_id = input.named("session_id")?;
            let usage: TranscriptionUsage = record(command, "usage", input.node("usage")?)?;
            input.finish()?;
            match stores.duplex.record_transcription_usage(session_id, usage) {
                Ok(event) => accepted(
                    command,
                    "recorded",
                    &DuplexEvent::TranscriptionUsageRecorded(event),
                    request,
                )?,
                Err(error) => session_not_found(command, error.session_id)?,
            }
        }
        CREATE_RESPONSE => {
            let session_id = input.named("session_id")?;
            input.finish()?;
            let answer = stores.duplex.create_response(session_id);
            session_answer(
                command,
                "created",
                answer.map(DuplexEvent::ResponseCreated),
                request,
            )?
        }
        EMIT_OUTPUT_AUDIO => {
            let response_id: ResponseId = input.named("response_id")?;
            let frame = audio_frame(command, input.node("frame")?)?;
            input.finish()?;
            match frame {
                Some(frame) => response_answer(
                    command,
                    "emitted",
                    stores
                        .duplex
                        .emit_output_audio(response_id, frame)
                        .map(DuplexEvent::OutputAudio),
                    request,
                )?,
                None => match stores.duplex.response(response_id) {
                    None => response_answer(
                        command,
                        "emitted",
                        Err(ResponseCommandError::Unknown(
                            inference_duplex::ResponseNotFound { response_id },
                        )),
                        request,
                    )?,
                    Some(response) => match response.require_in_progress() {
                        Err(conflict) => response_answer(
                            command,
                            "emitted",
                            Err(ResponseCommandError::Conflict(conflict)),
                            request,
                        )?,
                        Ok(()) => out_of_range(command),
                    },
                },
            }
        }
        EMIT_OUTPUT_TRANSCRIPT => {
            let response_id = input.named("response_id")?;
            let text = input.text("text")?;
            input.finish()?;
            let answer = stores.duplex.emit_output_transcript(response_id, text);
            response_answer(
                command,
                "emitted",
                answer.map(DuplexEvent::OutputTranscriptDelta),
                request,
            )?
        }
        COMPLETE_RESPONSE | CANCEL_RESPONSE | FAIL_RESPONSE => {
            let response_id = input.named("response_id")?;
            let (branch, answer) = match command {
                COMPLETE_RESPONSE => {
                    input.finish()?;
                    (
                        "completed",
                        stores
                            .duplex
                            .complete_response(response_id)
                            .map(DuplexEvent::ResponseCompleted),
                    )
                }
                CANCEL_RESPONSE => {
                    input.finish()?;
                    (
                        "cancelled",
                        stores
                            .duplex
                            .cancel_response(response_id)
                            .map(DuplexEvent::ResponseCancelled),
                    )
                }
                _ => {
                    let failure: SessionFailure =
                        record(command, "failure", input.node("failure")?)?;
                    input.finish()?;
                    (
                        "failed",
                        stores
                            .duplex
                            .fail_response(response_id, failure)
                            .map(DuplexEvent::ResponseFailed),
                    )
                }
            };
            match answer {
                Ok(event) => accepted(command, branch, &event, request)?,
                Err(ResponseCommandError::Unknown(_)) => {
                    wrong_state::<ResponseState>(command, RESPONSE_STATE_CONFLICT, None)?
                }
                Err(ResponseCommandError::Conflict(conflict)) => {
                    wrong_state(command, RESPONSE_STATE_CONFLICT, Some(conflict.current))?
                }
            }
        }
        TRUNCATE_OUTPUT | RECORD_RESPONSE_USAGE => {
            let response_id = input.named("response_id")?;
            let answer = if command == TRUNCATE_OUTPUT {
                let heard = position(&mut input, "heard")?;
                input.finish()?;
                stores
                    .duplex
                    .truncate_output(response_id, heard)
                    .map(DuplexEvent::OutputTruncated)
            } else {
                let usage: ResponseUsage = record(command, "usage", input.node("usage")?)?;
                input.finish()?;
                stores
                    .duplex
                    .record_response_usage(response_id, usage)
                    .map(DuplexEvent::ResponseUsageRecorded)
            };
            let branch = if command == TRUNCATE_OUTPUT {
                "truncated"
            } else {
                "recorded"
            };
            match answer {
                Ok(event) => accepted(command, branch, &event, request)?,
                Err(error) => response_not_found(command, error.response_id)?,
            }
        }
        PROPOSE_SPEECH_TOOL_CALL => {
            let propose = ProposeSpeechToolCall {
                response_id: input.named("response_id")?,
                tool_id: input.named("tool_id")?,
                arguments: json(input.node("arguments")?)?,
            };
            input.finish()?;
            match stores
                .duplex
                .propose_speech_tool_call(&stores.tools, propose)
            {
                Ok(event) => accepted(
                    command,
                    "proposed",
                    &DuplexEvent::SpeechToolCallProposed(event),
                    request,
                )?,
                Err(ProposeSpeechToolCallError::UnknownResponse(error)) => {
                    response_not_found(command, error.response_id)?
                }
                Err(ProposeSpeechToolCallError::UnknownTool(error)) => refused(
                    command,
                    "unknown-tool",
                    DeclaredErrorValue::new(parse::<ErrorRef>(TOOL_NOT_FOUND))
                        .with("tool_id", named_node(&error.tool_id)?),
                ),
                Err(ProposeSpeechToolCallError::NotInProgress(conflict)) => refused(
                    command,
                    "not-in-progress",
                    DeclaredErrorValue::new(parse::<ErrorRef>(RESPONSE_STATE_CONFLICT))
                        .with("current", named_node(&conflict.current)?),
                ),
            }
        }
        ANSWER_SPEECH_TOOL_CALL => {
            let call_id: SpeechCallId = input.named("call_id")?;
            let result = json(input.node("result")?)?;
            input.finish()?;
            match stores.duplex.answer_speech_tool_call(&call_id, result) {
                Ok(event) => accepted(
                    command,
                    "answered",
                    &DuplexEvent::SpeechToolCallAnswered(event),
                    request,
                )?,
                Err(AnswerSpeechToolCallError::Unknown { .. }) => {
                    wrong_state::<SpeechToolCallState>(
                        command,
                        SPEECH_TOOL_CALL_STATE_CONFLICT,
                        None,
                    )?
                }
                Err(AnswerSpeechToolCallError::Conflict(conflict)) => wrong_state(
                    command,
                    SPEECH_TOOL_CALL_STATE_CONFLICT,
                    Some(conflict.current),
                )?,
            }
        }
        CLOSE_SPEECH_SESSION | FAIL_SPEECH_SESSION => {
            let session_id = input.named("session_id")?;
            let (branch, answer) = if command == CLOSE_SPEECH_SESSION {
                input.finish()?;
                (
                    "closed",
                    stores
                        .duplex
                        .close_speech_session(session_id)
                        .map(DuplexEvent::SpeechSessionClosed),
                )
            } else {
                let failure: SessionFailure = record(command, "failure", input.node("failure")?)?;
                input.finish()?;
                (
                    "failed",
                    stores
                        .duplex
                        .fail_speech_session(session_id, failure)
                        .map(DuplexEvent::SpeechSessionFailed),
                )
            };
            match answer {
                Ok(event) => accepted(command, branch, &event, request)?,
                Err(SessionCommandError::Unknown(_)) => {
                    wrong_state::<SpeechSessionState>(command, SPEECH_SESSION_STATE_CONFLICT, None)?
                }
                Err(SessionCommandError::Conflict(conflict)) => wrong_state(
                    command,
                    SPEECH_SESSION_STATE_CONFLICT,
                    Some(conflict.current),
                )?,
            }
        }
        _ => return Err(unsupported(&name)),
    };
    answered(stores, result)
}

/// The result, placed and carrying the target's token.
fn answered(
    stores: &mut Stores,
    result: SemanticCommandResult,
) -> Result<SemanticCommandResult, TargetError> {
    stores.publish();
    Ok(result.with_consistency(stores.token()?))
}

/// The answer of a command acting on an open session: `branch` and its event, `unknown-session`
/// or `session-not-open`.
fn session_answer(
    command: &str,
    branch: &str,
    answer: Result<DuplexEvent, SessionCommandError>,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    match answer {
        Ok(event) => accepted(command, branch, &event, request),
        Err(SessionCommandError::Unknown(error)) => session_not_found(command, error.session_id),
        Err(SessionCommandError::Conflict(conflict)) => Ok(refused(
            command,
            "session-not-open",
            DeclaredErrorValue::new(parse::<ErrorRef>(SPEECH_SESSION_STATE_CONFLICT))
                .with("current", named_node(&conflict.current)?),
        )),
    }
}

/// The answer of a command acting on a response in progress: `branch` and its event,
/// `unknown-response` or `not-in-progress`.
fn response_answer(
    command: &str,
    branch: &str,
    answer: Result<DuplexEvent, ResponseCommandError>,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    match answer {
        Ok(event) => accepted(command, branch, &event, request),
        Err(ResponseCommandError::Unknown(error)) => response_not_found(command, error.response_id),
        Err(ResponseCommandError::Conflict(conflict)) => Ok(refused(
            command,
            "not-in-progress",
            DeclaredErrorValue::new(parse::<ErrorRef>(RESPONSE_STATE_CONFLICT))
                .with("current", named_node(&conflict.current)?),
        )),
    }
}

fn adapter_not_found(
    adapter_id: inference_duplex::inference_core::model::AdapterId,
) -> Result<SemanticCommandResult, TargetError> {
    Ok(refused(
        OPEN_SPEECH_SESSION,
        "unknown-adapter",
        DeclaredErrorValue::new(parse::<ErrorRef>(ADAPTER_NOT_FOUND))
            .with("adapter_id", named_node(&adapter_id)?),
    ))
}

fn out_of_range(command: &str) -> SemanticCommandResult {
    refused(
        command,
        "format-out-of-range",
        DeclaredErrorValue::new(parse::<ErrorRef>(FORMAT_OUT_OF_RANGE)),
    )
}

fn session_not_found(
    command: &str,
    session_id: SpeechSessionId,
) -> Result<SemanticCommandResult, TargetError> {
    Ok(refused(
        command,
        "unknown-session",
        DeclaredErrorValue::new(parse::<ErrorRef>(SPEECH_SESSION_NOT_FOUND))
            .with("session_id", named_node(&session_id)?),
    ))
}

fn response_not_found(
    command: &str,
    response_id: ResponseId,
) -> Result<SemanticCommandResult, TargetError> {
    Ok(refused(
        command,
        "unknown-response",
        DeclaredErrorValue::new(parse::<ErrorRef>(RESPONSE_NOT_FOUND))
            .with("response_id", named_node(&response_id)?),
    ))
}

/// The accepting outcome `branch` of `command`, emitting `event`.
fn accepted(
    command: &str,
    branch: &str,
    event: &DuplexEvent,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    Ok(SemanticCommandResult::took(outcome(command, branch))
        .emitting(duplex_event(event)?.in_activity(request.correlation.clone())))
}

/// The refusing outcome `branch` of `command`, carrying `error`.
fn refused(command: &str, branch: &str, error: DeclaredErrorValue) -> SemanticCommandResult {
    SemanticCommandResult::took(outcome(command, branch)).with_error(error)
}

/// The `wrong-state` outcome of `command`: `error`, carrying the held state, or nothing when no
/// record carries the identity sent.
fn wrong_state<S: Serialize>(
    command: &str,
    error: &str,
    current: Option<S>,
) -> Result<SemanticCommandResult, TargetError> {
    let value = DeclaredErrorValue::new(parse::<ErrorRef>(error));
    let value = match current {
        Some(current) => value.with("current", named_node(&current)?),
        None => value,
    };
    Ok(refused(command, "wrong-state", value))
}

/// An ESS `Optional<Integer>`: absent, or an integer within `i64`.
fn optional_integer(command: &str, node: Option<&Node>) -> Result<Option<i64>, TargetError> {
    node.map(|node| {
        integer(node).ok_or_else(|| {
            unreadable(
                format!("reading `{command}`"),
                format!("an optional integer is not one: {node:?}"),
            )
        })
    })
    .transpose()
}

/// A declared value written as its ESS string, such as an enum variant.
fn declared<T: DeserializeOwned>(
    command: &str,
    field: &str,
    node: &Node,
) -> Result<T, TargetError> {
    match node {
        Node::Text(text) => serde_json::from_value(Json::String(text.clone())).map_err(|error| {
            unreadable(
                format!("reading `{command}`"),
                format!("`{field}`: {error}"),
            )
        }),
        other => Err(unreadable(
            format!("reading `{command}`"),
            format!("`{field}` is not a declared value: {other:?}"),
        )),
    }
}

/// A sample clock position read from an ESS `Integer`; a negative one is an error.
fn position(input: &mut Input<'_>, field: &str) -> Result<SampleClockPosition, TargetError> {
    let value = input.integer(field)?;
    u64::try_from(value)
        .ok()
        .and_then(SampleClockPosition::new)
        .ok_or_else(|| unreadable(format!("reading `{field}`"), format!("{value} is negative")))
}

/// The fields of an ESS struct, each named once.
fn fields<'a>(
    command: &str,
    what: &str,
    node: &'a Node,
) -> Result<&'a BTreeMap<String, Node>, TargetError> {
    match node {
        Node::Map(fields) => Ok(fields),
        other => Err(unreadable(
            format!("reading `{command}`"),
            format!("`{what}` is not a record: {other:?}"),
        )),
    }
}

/// The integer of `field` of a struct, or an error naming it.
fn struct_integer(
    command: &str,
    what: &str,
    fields: &BTreeMap<String, Node>,
    field: &str,
) -> Result<i64, TargetError> {
    fields.get(field).and_then(integer).ok_or_else(|| {
        unreadable(
            format!("reading `{command}`"),
            format!(
                "`{what}.{field}` is not an integer: {:?}",
                fields.get(field)
            ),
        )
    })
}

/// Refuses a struct carrying a field outside `declared`.
fn only(
    command: &str,
    what: &str,
    fields: &BTreeMap<String, Node>,
    declared: &[&str],
) -> Result<(), TargetError> {
    match fields
        .keys()
        .find(|field| !declared.contains(&field.as_str()))
    {
        None => Ok(()),
        Some(extra) => Err(unreadable(
            format!("reading `{command}`"),
            format!("`{what}` carries the undeclared field `{extra}`"),
        )),
    }
}

/// An `inference.audio.AudioFormat`, or `None` when its sample rate or channel count is outside
/// the range an `AudioFormat` holds.
fn audio_format(command: &str, node: &Node) -> Result<Option<AudioFormat>, TargetError> {
    let what = "format";
    let fields = fields(command, what, node)?;
    only(
        command,
        what,
        fields,
        &["encoding", "sample_rate", "channels"],
    )?;
    let encoding = match fields.get("encoding") {
        Some(Node::Text(text)) => text.clone(),
        other => {
            return Err(unreadable(
                format!("reading `{command}`"),
                format!("`format.encoding` is not a string: {other:?}"),
            ));
        }
    };
    let sample_rate = struct_integer(command, what, fields, "sample_rate")?;
    let channels = struct_integer(command, what, fields, "channels")?;
    let (Ok(sample_rate @ 1..), Ok(channels @ 1..)) =
        (u32::try_from(sample_rate), u16::try_from(channels))
    else {
        return Ok(None);
    };
    let value = serde_json::json!({
        "encoding": encoding,
        "sample_rate": sample_rate,
        "channels": channels,
    });
    serde_json::from_value(value)
        .map(Some)
        .map_err(|error| unreadable(format!("reading `{command}`"), format!("`format`: {error}")))
}

/// An `inference.audio.AudioFrame`, or `None` when a number of it is outside the range an
/// `AudioFrame` holds.
fn audio_frame(command: &str, node: &Node) -> Result<Option<AudioFrame>, TargetError> {
    let what = "frame";
    let fields = fields(command, what, node)?;
    only(
        command,
        what,
        fields,
        &["format", "position", "sample_count", "data"],
    )?;
    let format_node = fields.get("format").ok_or_else(|| {
        unreadable(
            format!("reading `{command}`"),
            "`frame` carries no `format`",
        )
    })?;
    let format = audio_format(command, format_node)?;
    let position = struct_integer(command, what, fields, "position")?;
    let sample_count = struct_integer(command, what, fields, "sample_count")?;
    let data = match fields.get("data") {
        Some(Node::Text(text)) => text.clone(),
        other => {
            return Err(unreadable(
                format!("reading `{command}`"),
                format!("`frame.data` is not a base64 string: {other:?}"),
            ));
        }
    };
    let (Some(format), Ok(position), Ok(sample_count)) =
        (format, u64::try_from(position), u32::try_from(sample_count))
    else {
        return Ok(None);
    };
    let value = serde_json::json!({
        "format": format,
        "position": position,
        "sample_count": sample_count,
        "data": data,
    });
    serde_json::from_value(value)
        .map(Some)
        .map_err(|error| unreadable(format!("reading `{command}`"), format!("`frame`: {error}")))
}

/// A record of integers, strings and declared values read through its serde form: every number
/// of it must be integral, and its type refuses any field it does not declare.
fn record<T: DeserializeOwned>(command: &str, what: &str, node: &Node) -> Result<T, TargetError> {
    let value = integral(json(node)?).ok_or_else(|| {
        unreadable(
            format!("reading `{command}`"),
            format!("`{what}` carries a number that is not an integer: {node:?}"),
        )
    })?;
    serde_json::from_value(value)
        .map_err(|error| unreadable(format!("reading `{command}`"), format!("`{what}`: {error}")))
}

/// `value` with every number written as the integer it is, or `None` if one is not integral.
fn integral(value: Json) -> Option<Json> {
    Some(match value {
        Json::Number(number) => {
            if number.is_i64() || number.is_u64() {
                Json::Number(number)
            } else {
                let float = number.as_f64()?;
                let node = Node::Number(ess_primitives::facts::Number::new(float).ok()?);
                Json::from(integer(&node)?)
            }
        }
        Json::Array(items) => Json::Array(items.into_iter().map(integral).collect::<Option<_>>()?),
        Json::Object(fields) => Json::Object(
            fields
                .into_iter()
                .map(|(field, value)| integral(value).map(|value| (field, value)))
                .collect::<Option<_>>()?,
        ),
        other => other,
    })
}

/// An ESS `Json` value read from the node the request carries.
fn json(node: &Node) -> Result<Json, TargetError> {
    serde_json::to_value(node)
        .map_err(|error| unreadable("reading a `Json` value", format!("{error}: {node:?}")))
}

/// The node of a value through its serde form: a `Json` value, a format, a frame, a usage or
/// failure record.
fn json_node<T: Serialize>(value: &T) -> Result<Node, TargetError> {
    let value = serde_json::to_value(value)
        .map_err(|error| unreadable("writing a value", error.to_string()))?;
    serde_json::from_value(value.clone())
        .map_err(|error| unreadable("writing a value", format!("{error}: {value}")))
}

pub fn query_view(
    stores: &mut Stores,
    request: &SemanticViewRequest,
) -> Result<SemanticViewResult, TargetError> {
    let name = request.view.to_string();
    if ![SPEECH_SESSIONS, RESPONSES, SPEECH_TOOL_CALLS].contains(&name.as_str()) {
        return Err(unsupported(&name));
    }
    stores.require(&name, &request.consistency)?;
    let domain = &stores.duplex;
    let rows = match name.as_str() {
        SPEECH_SESSIONS => domain
            .speech_sessions_view()
            .iter()
            .map(speech_sessions_row)
            .collect::<Result<Vec<_>, _>>()?,
        RESPONSES => domain
            .responses_view()
            .iter()
            .map(responses_row)
            .collect::<Result<Vec<_>, _>>()?,
        _ => domain
            .speech_tool_calls_view()
            .iter()
            .map(speech_tool_calls_row)
            .collect::<Result<Vec<_>, _>>()?,
    };
    Ok(SemanticViewResult::of(rows))
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
    for (index, event) in stores.duplex.events().iter().enumerate() {
        let event = duplex_event(event)?;
        if event.event == request.event {
            let position = stores.duplex_position(index)?;
            observed.push(event.in_activity(request.correlation.clone()).at(position));
        }
    }
    Ok(observed)
}

/// An event of the store as the suite observes it, every declared field carried.
fn duplex_event(event: &DuplexEvent) -> Result<ObservedEvent, TargetError> {
    let new = |name: &str| ObservedEvent::new(parse::<EventRef>(name));
    Ok(match event {
        DuplexEvent::SpeechSessionOpened(event) => new(SPEECH_SESSION_OPENED)
            .with("session_id", named_node(&event.session_id)?)
            .with("adapter_id", named_node(&event.adapter_id)?)
            .with("turn_detection", named_node(&event.turn_detection)?),
        DuplexEvent::InputAccepted(event) => new(INPUT_ACCEPTED)
            .with("session_id", named_node(&event.session_id)?)
            .with("frame", json_node(&event.frame)?),
        DuplexEvent::InputCommitted(event) => {
            new(INPUT_COMMITTED).with("session_id", named_node(&event.session_id)?)
        }
        DuplexEvent::SpeechStarted(event) => new(SPEECH_STARTED)
            .with("session_id", named_node(&event.session_id)?)
            .with("position", json_node(&event.position)?),
        DuplexEvent::SpeechStopped(event) => new(SPEECH_STOPPED)
            .with("session_id", named_node(&event.session_id)?)
            .with("position", json_node(&event.position)?),
        DuplexEvent::InputTranscribed(event) => new(INPUT_TRANSCRIBED)
            .with("session_id", named_node(&event.session_id)?)
            .with("text", text_node(&event.text)),
        DuplexEvent::TranscriptionUsageRecorded(event) => new(TRANSCRIPTION_USAGE_RECORDED)
            .with("session_id", named_node(&event.session_id)?)
            .with("usage", json_node(&event.usage)?),
        DuplexEvent::ResponseCreated(event) => new(RESPONSE_CREATED)
            .with("response_id", named_node(&event.response_id)?)
            .with("session_id", named_node(&event.session_id)?),
        DuplexEvent::OutputAudio(event) => new(OUTPUT_AUDIO)
            .with("response_id", named_node(&event.response_id)?)
            .with("frame", json_node(&event.frame)?),
        DuplexEvent::OutputTranscriptDelta(event) => new(OUTPUT_TRANSCRIPT_DELTA)
            .with("response_id", named_node(&event.response_id)?)
            .with("text", text_node(&event.text)),
        DuplexEvent::ResponseCompleted(event) => {
            new(RESPONSE_COMPLETED).with("response_id", named_node(&event.response_id)?)
        }
        DuplexEvent::ResponseCancelled(event) => {
            new(RESPONSE_CANCELLED).with("response_id", named_node(&event.response_id)?)
        }
        DuplexEvent::ResponseFailed(event) => new(RESPONSE_FAILED)
            .with("response_id", named_node(&event.response_id)?)
            .with("failure", json_node(&event.failure)?),
        DuplexEvent::OutputTruncated(event) => new(OUTPUT_TRUNCATED)
            .with("response_id", named_node(&event.response_id)?)
            .with("heard", json_node(&event.heard)?),
        DuplexEvent::ResponseUsageRecorded(event) => new(RESPONSE_USAGE_RECORDED)
            .with("response_id", named_node(&event.response_id)?)
            .with("usage", json_node(&event.usage)?),
        DuplexEvent::SpeechToolCallProposed(event) => new(SPEECH_TOOL_CALL_PROPOSED)
            .with("call_id", named_node(&event.call_id)?)
            .with("response_id", named_node(&event.response_id)?)
            .with("tool_id", named_node(&event.tool_id)?)
            .with("arguments", json_node(&event.arguments)?),
        DuplexEvent::SpeechToolCallAnswered(event) => new(SPEECH_TOOL_CALL_ANSWERED)
            .with("call_id", named_node(&event.call_id)?)
            .with("result", json_node(&event.result)?),
        DuplexEvent::SpeechSessionClosed(event) => {
            new(SPEECH_SESSION_CLOSED).with("session_id", named_node(&event.session_id)?)
        }
        DuplexEvent::SpeechSessionFailed(event) => new(SPEECH_SESSION_FAILED)
            .with("session_id", named_node(&event.session_id)?)
            .with("failure", json_node(&event.failure)?),
    })
}

fn speech_sessions_row(row: &SpeechSessionsRow) -> Result<ViewRow, TargetError> {
    Ok(BTreeMap::from([
        ("session_id".to_owned(), named_node(&row.session_id)?),
        ("adapter_id".to_owned(), named_node(&row.adapter_id)?),
        ("voice".to_owned(), text_node(&row.voice)),
        ("instructions".to_owned(), text_node(&row.instructions)),
        ("input_format".to_owned(), json_node(&row.input_format)?),
        ("output_format".to_owned(), json_node(&row.output_format)?),
        (
            "turn_detection".to_owned(),
            named_node(&row.turn_detection)?,
        ),
        ("auto_response".to_owned(), boolean_node(row.auto_response)),
        (
            "auto_interrupt".to_owned(),
            boolean_node(row.auto_interrupt),
        ),
        ("tools".to_owned(), named_list_node(&row.tools)?),
        ("idle_limit_ms".to_owned(), integer_node(row.idle_limit_ms)),
        ("state".to_owned(), named_node(&row.state)?),
    ]))
}

fn responses_row(row: &ResponsesRow) -> Result<ViewRow, TargetError> {
    Ok(BTreeMap::from([
        ("response_id".to_owned(), named_node(&row.response_id)?),
        ("session_id".to_owned(), named_node(&row.session_id)?),
        ("state".to_owned(), named_node(&row.state)?),
    ]))
}

fn speech_tool_calls_row(row: &SpeechToolCallsRow) -> Result<ViewRow, TargetError> {
    Ok(BTreeMap::from([
        ("call_id".to_owned(), named_node(&row.call_id)?),
        ("response_id".to_owned(), named_node(&row.response_id)?),
        ("tool_id".to_owned(), named_node(&row.tool_id)?),
        ("arguments".to_owned(), json_node(&row.arguments)?),
        ("state".to_owned(), named_node(&row.state)?),
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
