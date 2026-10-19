//! The native speech sessions of `ess/domains/duplex.yaml`, held in memory, and behind the
//! `duplex` feature the scripted mock of the `inference-duplex` contract.
//!
//! [`DuplexDomain`] holds the sessions, responses and tool calls and records every event it emits
//! in order. Its commands run the [`SpeechSession`], [`Response`] and [`SpeechToolCall`] state
//! machines of `inference-duplex` under identities the domain issues from a counter rendered as a
//! Uuid in its own [`IDENTITY_SPACE`], `00000006-0000-4000-8000-000000000001` first; a call id,
//! which is the provider's string, is that Uuid written as a string.
//!
//! What only a store can decide is decided here: an adapter no record of the `inference.model`
//! store carries, a tool no record of the `inference.tools` store carries, a session, response or
//! call no record here carries. Each command decides in the order its outcomes are declared. The
//! views are [`DuplexDomain::speech_sessions_view`], [`DuplexDomain::responses_view`] and
//! [`DuplexDomain::speech_tool_calls_view`], all `read_your_writes`.
//!
//! [`SpeechSession`]: inference_duplex::SpeechSession
//! [`Response`]: inference_duplex::Response
//! [`SpeechToolCall`]: inference_duplex::SpeechToolCall

use uuid::Uuid;

use inference_core::model::AdapterId;
use inference_duplex::inference_audio::{AudioFormat, AudioFrame, SampleClockPosition};
use inference_duplex::{
    AdapterNotFound, InputAccepted, InputCommitted, InputTranscribed, NonPositiveIdleLimit,
    OpenSpeechSession, OutputAudio, OutputTranscriptDelta, OutputTruncated, ProposeSpeechToolCall,
    Response, ResponseCancelled, ResponseCompleted, ResponseCreated, ResponseFailed, ResponseId,
    ResponseNotFound, ResponseState, ResponseStateConflict, ResponseUsage, ResponseUsageRecorded,
    SessionFailure, SpeechCallId, SpeechSession, SpeechSessionClosed, SpeechSessionFailed,
    SpeechSessionId, SpeechSessionNotFound, SpeechSessionOpened, SpeechSessionState,
    SpeechSessionStateConflict, SpeechStarted, SpeechStopped, SpeechToolCall,
    SpeechToolCallAnswered, SpeechToolCallProposed, SpeechToolCallState,
    SpeechToolCallStateConflict, TranscriptionUsage, TranscriptionUsageRecorded, TurnDetection,
};
use inference_tools::{Json, ToolId, ToolNotFound};

use crate::model::ModelDomain;
use crate::tools::ToolsDomain;

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

/// `inference.duplex` in memory: its three entity stores and the events it emitted.
#[derive(Debug, Clone, Default)]
pub struct DuplexDomain {
    sessions: Vec<SpeechSession>,
    responses: Vec<Response>,
    calls: Vec<SpeechToolCall>,
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
        if model.adapters().get(input.adapter_id).is_none() {
            return Err(OpenSpeechSessionError::UnknownAdapter(AdapterNotFound {
                adapter_id: input.adapter_id,
            }));
        }
        let session_id = SpeechSessionId::from(self.next());
        let (session, event) = SpeechSession::open_as(session_id, input)
            .map_err(OpenSpeechSessionError::NoLivenessBound)?;
        self.issue();
        self.sessions.push(session);
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

    /// Runs `inference.duplex.CloseSpeechSession`.
    ///
    /// # Errors
    ///
    /// [`SessionCommandError::Unknown`] or [`SessionCommandError::Conflict`]; nothing moved.
    pub fn close_speech_session(
        &mut self,
        session_id: SpeechSessionId,
    ) -> Result<SpeechSessionClosed, SessionCommandError> {
        let event = self.move_session(session_id, SpeechSession::close)?;
        self.record(DuplexEvent::SpeechSessionClosed(event));
        Ok(event)
    }

    /// Runs `inference.duplex.FailSpeechSession`.
    ///
    /// # Errors
    ///
    /// [`SessionCommandError::Unknown`] or [`SessionCommandError::Conflict`]; nothing moved.
    pub fn fail_speech_session(
        &mut self,
        session_id: SpeechSessionId,
        failure: SessionFailure,
    ) -> Result<SpeechSessionFailed, SessionCommandError> {
        let event = self.move_session(session_id, |session| session.fail(failure))?;
        self.record(DuplexEvent::SpeechSessionFailed(event.clone()));
        Ok(event)
    }

    /// The session carrying `session_id`, if one was opened.
    #[must_use]
    pub fn session(&self, session_id: SpeechSessionId) -> Option<&SpeechSession> {
        self.sessions
            .iter()
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

    fn record(&mut self, event: DuplexEvent) {
        self.events.push(event);
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

    fn move_session<E>(
        &mut self,
        session_id: SpeechSessionId,
        transition: impl FnOnce(
            &SpeechSession,
        ) -> Result<(SpeechSession, E), SpeechSessionStateConflict>,
    ) -> Result<E, SessionCommandError> {
        let slot = self
            .sessions
            .iter_mut()
            .find(|session| session.session_id() == session_id)
            .ok_or(SessionCommandError::Unknown(SpeechSessionNotFound {
                session_id,
            }))?;
        let (moved, event) = transition(slot).map_err(SessionCommandError::Conflict)?;
        *slot = moved;
        Ok(event)
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
