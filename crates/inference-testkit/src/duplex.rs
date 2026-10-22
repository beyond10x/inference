//! The native speech sessions of `ess/domains/duplex.yaml`, held in memory, and behind the
//! `duplex` feature the scripted mock of the `inference-duplex` contract.
//!
//! [`DuplexDomain`] holds the sessions, responses, tool calls and received provider events and
//! records every event it emits in order. Each session and the provider events it received are
//! held by the production [`SessionLedger`] the bindings use, so receiving, delivering, closing
//! and failing run the ledger's code. Its commands run the [`SpeechSession`], [`Response`],
//! [`SpeechToolCall`] and [`ReceivedSessionEvent`] state machines of `inference-duplex` under
//! identities the domain issues from a counter rendered as a Uuid in its own [`IDENTITY_SPACE`],
//! `00000006-0000-4000-8000-000000000001` first; a call id, which is the provider's string, is
//! that Uuid written as a string.
//!
//! What only a store can decide is decided here: an adapter no record of the `inference.model`
//! store carries, a tool no record of the `inference.tools` store carries, a session, response,
//! call or received event no record here carries. Each command decides in the order its outcomes
//! are declared. Closing or failing a session delivers every event it received and still holds
//! before the end event. The views are [`DuplexDomain::speech_sessions_view`],
//! [`DuplexDomain::responses_view`], [`DuplexDomain::speech_tool_calls_view`] and
//! [`DuplexDomain::received_session_events_view`], all `read_your_writes`.
//!
//! [`SessionLedger`]: inference_duplex::SessionLedger
//! [`SpeechSession`]: inference_duplex::SpeechSession
//! [`Response`]: inference_duplex::Response
//! [`SpeechToolCall`]: inference_duplex::SpeechToolCall
//! [`ReceivedSessionEvent`]: inference_duplex::ReceivedSessionEvent

use uuid::Uuid;

use inference_core::model::AdapterId;
use inference_duplex::inference_audio::{AudioFormat, AudioFrame, SampleClockPosition};
use inference_duplex::{
    AdapterNotFound, InputAccepted, InputCommitted, InputTranscribed, NonPositiveIdleLimit,
    OpenSpeechSession, OutputAudio, OutputTranscriptDelta, OutputTruncated, ProposeSpeechToolCall,
    ReceivedEventId, ReceivedEventKind, ReceivedSessionEventState, Response, ResponseCancelled,
    ResponseCompleted, ResponseCreated, ResponseFailed, ResponseId, ResponseNotFound,
    ResponseState, ResponseStateConflict, ResponseUsage, ResponseUsageRecorded, SessionEnd,
    SessionError, SessionEvent, SessionEventDelivered, SessionEventReceived, SessionFailure,
    SessionLedger, SpeechCallId, SpeechSession, SpeechSessionClosed, SpeechSessionFailed,
    SpeechSessionId, SpeechSessionNotFound, SpeechSessionOpened, SpeechSessionState,
    SpeechSessionStateConflict, SpeechStarted, SpeechStopped, SpeechToolCall,
    SpeechToolCallAnswered, SpeechToolCallProposed, SpeechToolCallState,
    SpeechToolCallStateConflict, TranscriptionUsage, TranscriptionUsageRecorded, TurnDetection,
};
use inference_tools::{Json, ToolId, ToolNotFound};
use std::time::Instant;

use crate::model::ModelDomain;
use crate::tools::ToolsDomain;

pub use inference_duplex::DeliverSessionEventError;

/// The scripted mock native speech model.
#[cfg(feature = "duplex")]
pub mod mock;

/// The first eight hex digits of every identity the domain issues, which no other domain of the
/// testkit issues under (see [`crate::policy::IDENTITY_SPACE`]). Five is left to
/// `inference.session`.
pub const IDENTITY_SPACE: u32 = 6;

/// An event the domain emitted, in the order it emitted them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DuplexEvent {
    SpeechSessionOpened(SpeechSessionOpened),
    InputAccepted(InputAccepted),
    InputCommitted(InputCommitted),
    SpeechStarted(SpeechStarted),
    SpeechStopped(SpeechStopped),
    InputTranscribed(InputTranscribed),
    TranscriptionUsageRecorded(TranscriptionUsageRecorded),
    ResponseCreated(ResponseCreated),
    OutputAudio(OutputAudio),
    OutputTranscriptDelta(OutputTranscriptDelta),
    ResponseCompleted(ResponseCompleted),
    ResponseCancelled(ResponseCancelled),
    ResponseFailed(ResponseFailed),
    OutputTruncated(OutputTruncated),
    ResponseUsageRecorded(ResponseUsageRecorded),
    SpeechToolCallProposed(SpeechToolCallProposed),
    SpeechToolCallAnswered(SpeechToolCallAnswered),
    SessionEventReceived(SessionEventReceived),
    SessionEventDelivered(SessionEventDelivered),
    SpeechSessionClosed(SpeechSessionClosed),
    SpeechSessionFailed(SpeechSessionFailed),
}

/// Why `inference.duplex.OpenSpeechSession` opened nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenSpeechSessionError {
    /// The `unknown-adapter` outcome.
    UnknownAdapter(AdapterNotFound),
    /// The `no-liveness-bound` outcome.
    NoLivenessBound(NonPositiveIdleLimit),
}

/// Why a command acting on a session changed nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionCommandError {
    /// The `unknown-session` outcome.
    Unknown(SpeechSessionNotFound),
    /// The `session-not-open` or `wrong-state` outcome.
    Conflict(SpeechSessionStateConflict),
}

/// Why a command acting on a response changed nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseCommandError {
    /// The `unknown-response` outcome, or for a move the `wrong-state` outcome with no state.
    Unknown(ResponseNotFound),
    /// The `not-in-progress` or `wrong-state` outcome.
    Conflict(ResponseStateConflict),
}

/// Why `inference.duplex.ProposeSpeechToolCall` proposed nothing, in the order it decides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposeSpeechToolCallError {
    /// The `unknown-response` outcome.
    UnknownResponse(ResponseNotFound),
    /// The `unknown-tool` outcome.
    UnknownTool(ToolNotFound),
    /// The `not-in-progress` outcome.
    NotInProgress(ResponseStateConflict),
}

/// Why `inference.duplex.AnswerSpeechToolCall` moved nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnswerSpeechToolCallError {
    /// No call carries the identity: the `wrong-state` outcome with no state.
    Unknown { call_id: SpeechCallId },
    /// The `wrong-state` outcome.
    Conflict(SpeechToolCallStateConflict),
}

/// A row of the view `inference.duplex.SpeechSessions`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechSessionsRow {
    pub session_id: SpeechSessionId,
    pub adapter_id: AdapterId,
    pub voice: String,
    pub instructions: String,
    pub input_format: AudioFormat,
    pub output_format: AudioFormat,
    pub turn_detection: TurnDetection,
    pub auto_response: bool,
    pub auto_interrupt: bool,
    pub tools: Vec<ToolId>,
    pub idle_limit_ms: i64,
    pub state: SpeechSessionState,
}

/// A row of the view `inference.duplex.Responses`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponsesRow {
    pub response_id: ResponseId,
    pub session_id: SpeechSessionId,
    pub state: ResponseState,
}

/// A row of the view `inference.duplex.SpeechToolCalls`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechToolCallsRow {
    pub call_id: SpeechCallId,
    pub response_id: ResponseId,
    pub tool_id: ToolId,
    pub arguments: Json,
    pub state: SpeechToolCallState,
}

/// A row of the view `inference.duplex.ReceivedSessionEvents`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceivedSessionEventsRow {
    pub received_id: ReceivedEventId,
    pub session_id: SpeechSessionId,
    pub kind: ReceivedEventKind,
    pub sequence: i64,
    pub delivered_before: Option<SessionEnd>,
    pub state: ReceivedSessionEventState,
}

/// `inference.duplex` in memory: its four entity stores and the events it emitted.
#[derive(Debug, Clone, Default)]
pub struct DuplexDomain {
    /// The ledger of each session, in the order opened: it holds the session and the events its
    /// provider sent it.
    sessions: Vec<SessionLedger>,
    responses: Vec<Response>,
    calls: Vec<SpeechToolCall>,
    /// Each received event by its session, in the order received.
    received: Vec<(SpeechSessionId, ReceivedEventId)>,
    events: Vec<DuplexEvent>,
    issued: u128,
}

impl DuplexDomain {
    /// An empty domain.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs `inference.duplex.OpenSpeechSession` against the adapters `model` holds.
    ///
    /// # Errors
    ///
    /// [`OpenSpeechSessionError::UnknownAdapter`], then
    /// [`OpenSpeechSessionError::NoLivenessBound`]; nothing is stored and no identity is issued.
    pub fn open_speech_session(
        &mut self,
        model: &ModelDomain,
        input: OpenSpeechSession,
    ) -> Result<SpeechSessionOpened, OpenSpeechSessionError> {
        let Some(adapter) = model.adapters().get(input.adapter_id) else {
            return Err(OpenSpeechSessionError::UnknownAdapter(AdapterNotFound {
                adapter_id: input.adapter_id,
            }));
        };
        let session_id = SpeechSessionId::from(self.next());
        let (ledger, event) =
            SessionLedger::open_record(session_id, adapter.model_id(), input, Instant::now())
                .map_err(OpenSpeechSessionError::NoLivenessBound)?;
        self.issue();
        self.sessions.push(ledger);
        self.record(DuplexEvent::SpeechSessionOpened(event.clone()));
        Ok(event)
    }

    /// Runs `inference.duplex.AppendInputAudio`.
    ///
    /// # Errors
    ///
    /// [`SessionCommandError::Unknown`], then [`SessionCommandError::Conflict`].
    pub fn append_input_audio(
        &mut self,
        session_id: SpeechSessionId,
        frame: AudioFrame,
    ) -> Result<InputAccepted, SessionCommandError> {
        self.open_session(session_id)?;
        let event = InputAccepted { session_id, frame };
        self.record(DuplexEvent::InputAccepted(event.clone()));
        Ok(event)
    }

    /// Runs `inference.duplex.CommitInput`.
    ///
    /// # Errors
    ///
    /// [`SessionCommandError::Unknown`], then [`SessionCommandError::Conflict`].
    pub fn commit_input(
        &mut self,
        session_id: SpeechSessionId,
    ) -> Result<InputCommitted, SessionCommandError> {
        self.open_session(session_id)?;
        let event = InputCommitted { session_id };
        self.record(DuplexEvent::InputCommitted(event));
        Ok(event)
    }

    /// Runs `inference.duplex.ObserveSpeechStarted`.
    ///
    /// # Errors
    ///
    /// [`SessionCommandError::Unknown`], then [`SessionCommandError::Conflict`].
    pub fn observe_speech_started(
        &mut self,
        session_id: SpeechSessionId,
        position: SampleClockPosition,
    ) -> Result<SpeechStarted, SessionCommandError> {
        self.open_session(session_id)?;
        let event = SpeechStarted {
            session_id,
            position,
        };
        self.record(DuplexEvent::SpeechStarted(event));
        Ok(event)
    }

    /// Runs `inference.duplex.ObserveSpeechStopped`.
    ///
    /// # Errors
    ///
    /// [`SessionCommandError::Unknown`], then [`SessionCommandError::Conflict`].
    pub fn observe_speech_stopped(
        &mut self,
        session_id: SpeechSessionId,
        position: SampleClockPosition,
    ) -> Result<SpeechStopped, SessionCommandError> {
        self.open_session(session_id)?;
        let event = SpeechStopped {
            session_id,
            position,
        };
        self.record(DuplexEvent::SpeechStopped(event));
        Ok(event)
    }

    /// Runs `inference.duplex.TranscribeInput`.
    ///
    /// # Errors
    ///
    /// [`SessionCommandError::Unknown`], then [`SessionCommandError::Conflict`].
    pub fn transcribe_input(
        &mut self,
        session_id: SpeechSessionId,
        text: String,
    ) -> Result<InputTranscribed, SessionCommandError> {
        self.open_session(session_id)?;
        let event = InputTranscribed { session_id, text };
        self.record(DuplexEvent::InputTranscribed(event.clone()));
        Ok(event)
    }

    /// Runs `inference.duplex.RecordTranscriptionUsage`, in any state of the session.
    ///
    /// # Errors
    ///
    /// [`SpeechSessionNotFound`].
    pub fn record_transcription_usage(
        &mut self,
        session_id: SpeechSessionId,
        usage: TranscriptionUsage,
    ) -> Result<TranscriptionUsageRecorded, SpeechSessionNotFound> {
        self.session(session_id)
            .ok_or(SpeechSessionNotFound { session_id })?;
        let event = TranscriptionUsageRecorded { session_id, usage };
        self.record(DuplexEvent::TranscriptionUsageRecorded(event));
        Ok(event)
    }

    /// Runs `inference.duplex.CreateResponse`.
    ///
    /// # Errors
    ///
    /// [`SessionCommandError::Unknown`], then [`SessionCommandError::Conflict`]; no identity is
    /// issued.
    pub fn create_response(
        &mut self,
        session_id: SpeechSessionId,
    ) -> Result<ResponseCreated, SessionCommandError> {
        let session = self
            .session(session_id)
            .ok_or(SessionCommandError::Unknown(SpeechSessionNotFound {
                session_id,
            }))?;
        let response_id = ResponseId::from(self.next());
        let (response, event) =
            Response::create_as(response_id, session).map_err(SessionCommandError::Conflict)?;
        self.issue();
        self.responses.push(response);
        self.record(DuplexEvent::ResponseCreated(event));
        Ok(event)
    }

    /// Runs `inference.duplex.EmitOutputAudio`.
    ///
    /// # Errors
    ///
    /// [`ResponseCommandError::Unknown`], then [`ResponseCommandError::Conflict`].
    pub fn emit_output_audio(
        &mut self,
        response_id: ResponseId,
        frame: AudioFrame,
    ) -> Result<OutputAudio, ResponseCommandError> {
        self.in_progress(response_id)?;
        let event = OutputAudio { response_id, frame };
        self.record(DuplexEvent::OutputAudio(event.clone()));
        Ok(event)
    }

    /// Runs `inference.duplex.EmitOutputTranscript`.
    ///
    /// # Errors
    ///
    /// [`ResponseCommandError::Unknown`], then [`ResponseCommandError::Conflict`].
    pub fn emit_output_transcript(
        &mut self,
        response_id: ResponseId,
        text: String,
    ) -> Result<OutputTranscriptDelta, ResponseCommandError> {
        self.in_progress(response_id)?;
        let event = OutputTranscriptDelta { response_id, text };
        self.record(DuplexEvent::OutputTranscriptDelta(event.clone()));
        Ok(event)
    }

    /// Runs `inference.duplex.CompleteResponse`.
    ///
    /// # Errors
    ///
    /// [`ResponseCommandError::Unknown`] or [`ResponseCommandError::Conflict`]; nothing moved.
    pub fn complete_response(
        &mut self,
        response_id: ResponseId,
    ) -> Result<ResponseCompleted, ResponseCommandError> {
        let event = self.move_response(response_id, Response::complete)?;
        self.record(DuplexEvent::ResponseCompleted(event));
        Ok(event)
    }

    /// Runs `inference.duplex.CancelResponse`.
    ///
    /// # Errors
    ///
    /// [`ResponseCommandError::Unknown`] or [`ResponseCommandError::Conflict`]; nothing moved.
    pub fn cancel_response(
        &mut self,
        response_id: ResponseId,
    ) -> Result<ResponseCancelled, ResponseCommandError> {
        let event = self.move_response(response_id, Response::cancel)?;
        self.record(DuplexEvent::ResponseCancelled(event));
        Ok(event)
    }

    /// Runs `inference.duplex.FailResponse`.
    ///
    /// # Errors
    ///
    /// [`ResponseCommandError::Unknown`] or [`ResponseCommandError::Conflict`]; nothing moved.
    pub fn fail_response(
        &mut self,
        response_id: ResponseId,
        failure: SessionFailure,
    ) -> Result<ResponseFailed, ResponseCommandError> {
        let event = self.move_response(response_id, |response| response.fail(failure))?;
        self.record(DuplexEvent::ResponseFailed(event.clone()));
        Ok(event)
    }

    /// Runs `inference.duplex.TruncateOutput`, in any state of the response.
    ///
    /// # Errors
    ///
    /// [`ResponseNotFound`].
    pub fn truncate_output(
        &mut self,
        response_id: ResponseId,
        heard: SampleClockPosition,
    ) -> Result<OutputTruncated, ResponseNotFound> {
        self.response(response_id)
            .ok_or(ResponseNotFound { response_id })?;
        let event = OutputTruncated { response_id, heard };
        self.record(DuplexEvent::OutputTruncated(event));
        Ok(event)
    }

    /// Runs `inference.duplex.RecordResponseUsage`, in any state of the response.
    ///
    /// # Errors
    ///
    /// [`ResponseNotFound`].
    pub fn record_response_usage(
        &mut self,
        response_id: ResponseId,
        usage: ResponseUsage,
    ) -> Result<ResponseUsageRecorded, ResponseNotFound> {
        self.response(response_id)
            .ok_or(ResponseNotFound { response_id })?;
        let event = ResponseUsageRecorded { response_id, usage };
        self.record(DuplexEvent::ResponseUsageRecorded(event));
        Ok(event)
    }

    /// Runs `inference.duplex.ProposeSpeechToolCall` against the tools `tools` holds.
    ///
    /// # Errors
    ///
    /// In this order: [`ProposeSpeechToolCallError::UnknownResponse`],
    /// [`ProposeSpeechToolCallError::UnknownTool`], [`ProposeSpeechToolCallError::NotInProgress`];
    /// nothing is stored and no identity is issued.
    pub fn propose_speech_tool_call(
        &mut self,
        tools: &ToolsDomain,
        input: ProposeSpeechToolCall,
    ) -> Result<SpeechToolCallProposed, ProposeSpeechToolCallError> {
        let response = *self.response(input.response_id).ok_or(
            ProposeSpeechToolCallError::UnknownResponse(ResponseNotFound {
                response_id: input.response_id,
            }),
        )?;
        if tools.tools().get(input.tool_id).is_none() {
            return Err(ProposeSpeechToolCallError::UnknownTool(ToolNotFound {
                tool_id: input.tool_id,
            }));
        }
        let call_id = SpeechCallId::new(self.next().to_string());
        let (call, event) = SpeechToolCall::propose_as(call_id, &response, input)
            .map_err(ProposeSpeechToolCallError::NotInProgress)?;
        self.issue();
        self.calls.push(call);
        self.record(DuplexEvent::SpeechToolCallProposed(event.clone()));
        Ok(event)
    }

    /// Runs `inference.duplex.AnswerSpeechToolCall`.
    ///
    /// # Errors
    ///
    /// [`AnswerSpeechToolCallError::Unknown`] or [`AnswerSpeechToolCallError::Conflict`]; nothing
    /// moved.
    pub fn answer_speech_tool_call(
        &mut self,
        call_id: &SpeechCallId,
        result: Json,
    ) -> Result<SpeechToolCallAnswered, AnswerSpeechToolCallError> {
        let slot = self
            .calls
            .iter_mut()
            .find(|call| call.call_id() == call_id)
            .ok_or_else(|| AnswerSpeechToolCallError::Unknown {
                call_id: call_id.clone(),
            })?;
        let (moved, event) = slot
            .answer(result)
            .map_err(AnswerSpeechToolCallError::Conflict)?;
        *slot = moved;
        self.record(DuplexEvent::SpeechToolCallAnswered(event.clone()));
        Ok(event)
    }

    /// Runs `inference.duplex.CloseSpeechSession` through the session's [`SessionLedger`], which
    /// delivers every event the session still holds before `SpeechSessionClosed`.
    ///
    /// # Errors
    ///
    /// [`SessionCommandError::Unknown`] or [`SessionCommandError::Conflict`]; nothing moved.
    pub fn close_speech_session(
        &mut self,
        session_id: SpeechSessionId,
    ) -> Result<SpeechSessionClosed, SessionCommandError> {
        let event = self.end_session(session_id, SessionLedger::close, |event| match event {
            SessionEvent::SpeechSessionClosed(closed) => Some(closed),
            _ => None,
        })?;
        self.record(DuplexEvent::SpeechSessionClosed(event));
        Ok(event)
    }

    /// Runs `inference.duplex.FailSpeechSession` through the session's [`SessionLedger`], which
    /// delivers every event the session still holds before `SpeechSessionFailed`.
    ///
    /// # Errors
    ///
    /// [`SessionCommandError::Unknown`] or [`SessionCommandError::Conflict`]; nothing moved.
    pub fn fail_speech_session(
        &mut self,
        session_id: SpeechSessionId,
        failure: SessionFailure,
    ) -> Result<SpeechSessionFailed, SessionCommandError> {
        let event = self.end_session(
            session_id,
            |ledger| ledger.fail(failure),
            |event| match event {
                SessionEvent::SpeechSessionFailed(failed) => Some(failed),
                _ => None,
            },
        )?;
        self.record(DuplexEvent::SpeechSessionFailed(event.clone()));
        Ok(event)
    }

    /// Runs `inference.duplex.ReceiveSessionEvent`: the provider sent the open session an event
    /// of `kind`, the `sequence`th it sent.
    ///
    /// # Errors
    ///
    /// [`SessionCommandError::Unknown`], then [`SessionCommandError::Conflict`]; nothing is held
    /// and no identity is issued.
    pub fn receive_session_event(
        &mut self,
        session_id: SpeechSessionId,
        kind: ReceivedEventKind,
        sequence: i64,
    ) -> Result<SessionEventReceived, SessionCommandError> {
        let received_id = ReceivedEventId::from(self.next());
        let event = self
            .ledger(session_id)?
            .receive_event(received_id, kind, sequence)
            .map_err(SessionCommandError::Conflict)?;
        self.issue();
        self.received.push((session_id, received_id));
        self.record(DuplexEvent::SessionEventReceived(event));
        Ok(event)
    }

    /// Runs `inference.duplex.DeliverSessionEvent`: the caller has the event.
    ///
    /// # Errors
    ///
    /// [`DeliverSessionEventError::Unknown`] or [`DeliverSessionEventError::Conflict`]; nothing
    /// moved.
    pub fn deliver_session_event(
        &mut self,
        received_id: ReceivedEventId,
    ) -> Result<SessionEventDelivered, DeliverSessionEventError> {
        let session_id = self
            .received
            .iter()
            .find(|(_, received)| *received == received_id)
            .map(|(session_id, _)| *session_id)
            .ok_or(DeliverSessionEventError::Unknown { received_id })?;
        let event = self
            .sessions
            .iter_mut()
            .find(|ledger| ledger.session().session_id() == session_id)
            .ok_or(DeliverSessionEventError::Unknown { received_id })?
            .deliver_event(received_id)?;
        self.record(DuplexEvent::SessionEventDelivered(event));
        Ok(event)
    }

    /// The session carrying `session_id`, if one was opened.
    #[must_use]
    pub fn session(&self, session_id: SpeechSessionId) -> Option<&SpeechSession> {
        self.sessions
            .iter()
            .map(SessionLedger::session)
            .find(|session| session.session_id() == session_id)
    }

    /// The response carrying `response_id`, if one was created.
    #[must_use]
    pub fn response(&self, response_id: ResponseId) -> Option<&Response> {
        self.responses
            .iter()
            .find(|response| response.response_id() == response_id)
    }

    /// Every event emitted, in the order it was emitted.
    #[must_use]
    pub fn events(&self) -> &[DuplexEvent] {
        &self.events
    }

    /// The view `inference.duplex.SpeechSessions`: one row per session, in the order opened.
    #[must_use]
    pub fn speech_sessions_view(&self) -> Vec<SpeechSessionsRow> {
        self.sessions
            .iter()
            .map(SessionLedger::session)
            .map(|session| {
                let config = session.config();
                SpeechSessionsRow {
                    session_id: session.session_id(),
                    adapter_id: config.adapter_id,
                    voice: config.voice.clone(),
                    instructions: config.instructions.clone(),
                    input_format: config.input_format,
                    output_format: config.output_format,
                    turn_detection: config.turn_detection,
                    auto_response: config.auto_response,
                    auto_interrupt: config.auto_interrupt,
                    tools: config.tools.clone(),
                    idle_limit_ms: config.idle_limit_ms,
                    state: session.state(),
                }
            })
            .collect()
    }

    /// The view `inference.duplex.Responses`: one row per response, in the order created.
    #[must_use]
    pub fn responses_view(&self) -> Vec<ResponsesRow> {
        self.responses
            .iter()
            .map(|response| ResponsesRow {
                response_id: response.response_id(),
                session_id: response.session_id(),
                state: response.state(),
            })
            .collect()
    }

    /// The view `inference.duplex.SpeechToolCalls`: one row per call, in the order proposed.
    #[must_use]
    pub fn speech_tool_calls_view(&self) -> Vec<SpeechToolCallsRow> {
        self.calls
            .iter()
            .map(|call| SpeechToolCallsRow {
                call_id: call.call_id().clone(),
                response_id: call.response_id(),
                tool_id: call.tool_id(),
                arguments: call.arguments().clone(),
                state: call.state(),
            })
            .collect()
    }

    /// The view `inference.duplex.ReceivedSessionEvents`: one row per received event, in the
    /// order received.
    #[must_use]
    pub fn received_session_events_view(&self) -> Vec<ReceivedSessionEventsRow> {
        self.received
            .iter()
            .filter_map(|(session_id, received_id)| {
                self.sessions
                    .iter()
                    .find(|ledger| ledger.session().session_id() == *session_id)?
                    .received_events()
                    .iter()
                    .find(|received| received.received_id() == *received_id)
            })
            .map(|received| ReceivedSessionEventsRow {
                received_id: received.received_id(),
                session_id: received.session_id(),
                kind: received.kind(),
                sequence: received.sequence(),
                delivered_before: received.delivered_before(),
                state: received.state(),
            })
            .collect()
    }

    fn record(&mut self, event: DuplexEvent) {
        self.events.push(event);
    }

    /// The ledger of the session carrying `session_id`.
    fn ledger(
        &mut self,
        session_id: SpeechSessionId,
    ) -> Result<&mut SessionLedger, SessionCommandError> {
        self.sessions
            .iter_mut()
            .find(|ledger| ledger.session().session_id() == session_id)
            .ok_or(SessionCommandError::Unknown(SpeechSessionNotFound {
                session_id,
            }))
    }

    /// Ends the session carrying `session_id` through its ledger with `end`, and takes the end
    /// event `pick` finds among what the ledger recorded. The ledger refuses a session that is not
    /// Open; it holds no response of its own, so it refuses nothing else.
    fn end_session<E>(
        &mut self,
        session_id: SpeechSessionId,
        end: impl FnOnce(&mut SessionLedger) -> Result<(), SessionError>,
        pick: impl Fn(SessionEvent) -> Option<E>,
    ) -> Result<E, SessionCommandError> {
        let ledger = self.ledger(session_id)?;
        let state = ledger.session().state();
        end(ledger).map_err(|error| match error {
            SessionError::NotOpen(conflict) => SessionCommandError::Conflict(conflict),
            _ => SessionCommandError::Conflict(SpeechSessionStateConflict { current: state }),
        })?;
        std::iter::from_fn(|| ledger.next_event())
            .filter_map(pick)
            .last()
            .ok_or(SessionCommandError::Conflict(SpeechSessionStateConflict {
                current: state,
            }))
    }

    /// The session, which must exist and be Open.
    fn open_session(&self, session_id: SpeechSessionId) -> Result<(), SessionCommandError> {
        self.session(session_id)
            .ok_or(SessionCommandError::Unknown(SpeechSessionNotFound {
                session_id,
            }))?
            .require_open()
            .map_err(SessionCommandError::Conflict)
    }

    /// The response, which must exist and be InProgress.
    fn in_progress(&self, response_id: ResponseId) -> Result<(), ResponseCommandError> {
        self.response(response_id)
            .ok_or(ResponseCommandError::Unknown(ResponseNotFound {
                response_id,
            }))?
            .require_in_progress()
            .map_err(ResponseCommandError::Conflict)
    }

    fn move_response<E>(
        &mut self,
        response_id: ResponseId,
        transition: impl FnOnce(&Response) -> Result<(Response, E), ResponseStateConflict>,
    ) -> Result<E, ResponseCommandError> {
        let slot = self
            .responses
            .iter_mut()
            .find(|response| response.response_id() == response_id)
            .ok_or(ResponseCommandError::Unknown(ResponseNotFound {
                response_id,
            }))?;
        let (moved, event) = transition(slot).map_err(ResponseCommandError::Conflict)?;
        *slot = moved;
        Ok(event)
    }

    /// The next identity of the counter, taken.
    fn issue(&mut self) -> Uuid {
        self.issued += 1;
        Self::identity(self.issued)
    }

    /// The next identity of the counter, not taken; a refused command takes none.
    fn next(&self) -> Uuid {
        Self::identity(self.issued + 1)
    }

    /// The `count`th identity: a version 4, RFC 4122 variant Uuid whose low bits are `count` and
    /// whose first eight hex digits are [`IDENTITY_SPACE`].
    fn identity(count: u128) -> Uuid {
        Uuid::from_u128(
            (u128::from(IDENTITY_SPACE) << 96) | 0x0000_0000_0000_4000_8000_0000_0000_0000 | count,
        )
    }
}
