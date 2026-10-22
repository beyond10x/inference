//! The four entities of `ess/domains/duplex.yaml` and their lifecycles. Each transition is a
//! method that returns the moved record and the event its command emits, or the error the
//! command declares; a refused move changes nothing. Which session, response, call or received
//! event an identity names is decided by a store or a [`SessionLedger`](crate::SessionLedger)
//! holding them.

use inference_audio::AudioFormat;
use inference_core::model::AdapterId;
use inference_tools::{Json, ToolId};

use crate::{
    NonPositiveIdleLimit, OpenSpeechSession, ProposeSpeechToolCall, ReceivedEventId,
    ReceivedEventKind, ReceivedSessionEventState, ReceivedSessionEventStateConflict,
    ResponseCancelled, ResponseCompleted, ResponseCreated, ResponseFailed, ResponseId,
    ResponseState, ResponseStateConflict, SessionEnd, SessionEventDelivered, SessionEventReceived,
    SessionFailure, SpeechCallId, SpeechSessionClosed, SpeechSessionFailed, SpeechSessionId,
    SpeechSessionOpened, SpeechSessionState, SpeechSessionStateConflict, SpeechToolCallAnswered,
    SpeechToolCallProposed, SpeechToolCallState, SpeechToolCallStateConflict, TurnDetection,
};

/// `inference.duplex.SpeechSession`: a native speech session over one adapter, with the
/// configuration it was opened with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechSession {
    session_id: SpeechSessionId,
    config: OpenSpeechSession,
    state: SpeechSessionState,
}

impl SpeechSession {
    /// Runs `inference.duplex.OpenSpeechSession` under `session_id`; whether the adapter exists is
    /// the store's to decide.
    ///
    /// # Errors
    ///
    /// [`NonPositiveIdleLimit`] carrying the submitted bound when it is below one millisecond;
    /// nothing is opened.
    pub fn open_as(
        session_id: SpeechSessionId,
        input: OpenSpeechSession,
    ) -> Result<(Self, SpeechSessionOpened), NonPositiveIdleLimit> {
        if input.idle_limit_ms <= 0 {
            return Err(NonPositiveIdleLimit {
                submitted: input.idle_limit_ms,
            });
        }
        let event = SpeechSessionOpened {
            session_id,
            adapter_id: input.adapter_id,
            turn_detection: input.turn_detection,
        };
        let session = Self {
            session_id,
            config: input,
            state: SpeechSessionState::Open,
        };
        Ok((session, event))
    }

    #[must_use]
    pub const fn session_id(&self) -> SpeechSessionId {
        self.session_id
    }

    #[must_use]
    pub const fn adapter_id(&self) -> AdapterId {
        self.config.adapter_id
    }

    /// The configuration the session was opened with, as `OpenSpeechSession` records it.
    #[must_use]
    pub const fn config(&self) -> &OpenSpeechSession {
        &self.config
    }

    #[must_use]
    pub const fn input_format(&self) -> &AudioFormat {
        &self.config.input_format
    }

    #[must_use]
    pub const fn output_format(&self) -> &AudioFormat {
        &self.config.output_format
    }

    #[must_use]
    pub const fn turn_detection(&self) -> TurnDetection {
        self.config.turn_detection
    }

    #[must_use]
    pub const fn state(&self) -> SpeechSessionState {
        self.state
    }

    /// Whether the session is Open, which every input, response and observation needs.
    ///
    /// # Errors
    ///
    /// [`SpeechSessionStateConflict`] carrying the held state when it is not Open.
    pub const fn require_open(&self) -> Result<(), SpeechSessionStateConflict> {
        match self.state {
            SpeechSessionState::Open => Ok(()),
            current => Err(SpeechSessionStateConflict { current }),
        }
    }

    /// Runs `inference.duplex.CloseSpeechSession`.
    ///
    /// # Errors
    ///
    /// [`SpeechSessionStateConflict`] carrying the held state when it is not Open.
    pub fn close(&self) -> Result<(Self, SpeechSessionClosed), SpeechSessionStateConflict> {
        self.require_open()?;
        let moved = Self {
            state: SpeechSessionState::Closed,
            ..self.clone()
        };
        Ok((
            moved,
            SpeechSessionClosed {
                session_id: self.session_id,
            },
        ))
    }

    /// Runs `inference.duplex.FailSpeechSession` with `failure`.
    ///
    /// # Errors
    ///
    /// [`SpeechSessionStateConflict`] carrying the held state when it is not Open.
    pub fn fail(
        &self,
        failure: SessionFailure,
    ) -> Result<(Self, SpeechSessionFailed), SpeechSessionStateConflict> {
        self.require_open()?;
        let moved = Self {
            state: SpeechSessionState::Failed,
            ..self.clone()
        };
        Ok((
            moved,
            SpeechSessionFailed {
                session_id: self.session_id,
                failure,
            },
        ))
    }
}

/// `inference.duplex.Response`: one response of a session. A cancelled response is never
/// resumed; the next answer is a new response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Response {
    response_id: ResponseId,
    session_id: SpeechSessionId,
    state: ResponseState,
}

impl Response {
    /// Runs `inference.duplex.CreateResponse` on `session` under `response_id`.
    ///
    /// # Errors
    ///
    /// [`SpeechSessionStateConflict`] carrying the session's state when it is not Open.
    pub fn create_as(
        response_id: ResponseId,
        session: &SpeechSession,
    ) -> Result<(Self, ResponseCreated), SpeechSessionStateConflict> {
        session.require_open()?;
        let event = ResponseCreated {
            response_id,
            session_id: session.session_id(),
        };
        let response = Self {
            response_id,
            session_id: session.session_id(),
            state: ResponseState::InProgress,
        };
        Ok((response, event))
    }

    #[must_use]
    pub const fn response_id(&self) -> ResponseId {
        self.response_id
    }

    #[must_use]
    pub const fn session_id(&self) -> SpeechSessionId {
        self.session_id
    }

    #[must_use]
    pub const fn state(&self) -> ResponseState {
        self.state
    }

    /// Whether the response is InProgress, which its output and its tool proposals need.
    ///
    /// # Errors
    ///
    /// [`ResponseStateConflict`] carrying the held state when it is not InProgress.
    pub const fn require_in_progress(&self) -> Result<(), ResponseStateConflict> {
        match self.state {
            ResponseState::InProgress => Ok(()),
            current => Err(ResponseStateConflict { current }),
        }
    }

    fn moved(&self, state: ResponseState) -> Result<Self, ResponseStateConflict> {
        self.require_in_progress()?;
        Ok(Self { state, ..*self })
    }

    /// Runs `inference.duplex.CompleteResponse`.
    ///
    /// # Errors
    ///
    /// [`ResponseStateConflict`] carrying the held state when it is not InProgress.
    pub fn complete(&self) -> Result<(Self, ResponseCompleted), ResponseStateConflict> {
        let moved = self.moved(ResponseState::Completed)?;
        Ok((
            moved,
            ResponseCompleted {
                response_id: self.response_id,
            },
        ))
    }

    /// Runs `inference.duplex.CancelResponse`.
    ///
    /// # Errors
    ///
    /// [`ResponseStateConflict`] carrying the held state when it is not InProgress.
    pub fn cancel(&self) -> Result<(Self, ResponseCancelled), ResponseStateConflict> {
        let moved = self.moved(ResponseState::Cancelled)?;
        Ok((
            moved,
            ResponseCancelled {
                response_id: self.response_id,
            },
        ))
    }

    /// Runs `inference.duplex.FailResponse` with `failure`.
    ///
    /// # Errors
    ///
    /// [`ResponseStateConflict`] carrying the held state when it is not InProgress.
    pub fn fail(
        &self,
        failure: SessionFailure,
    ) -> Result<(Self, ResponseFailed), ResponseStateConflict> {
        let moved = self.moved(ResponseState::Failed)?;
        Ok((
            moved,
            ResponseFailed {
                response_id: self.response_id,
                failure,
            },
        ))
    }
}

/// `inference.duplex.SpeechToolCall`: a call a response proposed inside the session, answered on
/// it by its call id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechToolCall {
    call_id: SpeechCallId,
    response_id: ResponseId,
    tool_id: ToolId,
    arguments: Json,
    state: SpeechToolCallState,
}

impl SpeechToolCall {
    /// Runs `inference.duplex.ProposeSpeechToolCall` by `response` under `call_id`; whether the
    /// tool exists is the store's to decide.
    ///
    /// # Errors
    ///
    /// [`ResponseStateConflict`] carrying the response's state when it is not InProgress.
    pub fn propose_as(
        call_id: SpeechCallId,
        response: &Response,
        input: ProposeSpeechToolCall,
    ) -> Result<(Self, SpeechToolCallProposed), ResponseStateConflict> {
        response.require_in_progress()?;
        let event = SpeechToolCallProposed {
            call_id: call_id.clone(),
            response_id: response.response_id(),
            tool_id: input.tool_id,
            arguments: input.arguments.clone(),
        };
        let call = Self {
            call_id,
            response_id: response.response_id(),
            tool_id: input.tool_id,
            arguments: input.arguments,
            state: SpeechToolCallState::Proposed,
        };
        Ok((call, event))
    }

    #[must_use]
    pub const fn call_id(&self) -> &SpeechCallId {
        &self.call_id
    }

    #[must_use]
    pub const fn response_id(&self) -> ResponseId {
        self.response_id
    }

    #[must_use]
    pub const fn tool_id(&self) -> ToolId {
        self.tool_id
    }

    #[must_use]
    pub const fn arguments(&self) -> &Json {
        &self.arguments
    }

    #[must_use]
    pub const fn state(&self) -> SpeechToolCallState {
        self.state
    }

    /// Runs `inference.duplex.AnswerSpeechToolCall` with `result`.
    ///
    /// # Errors
    ///
    /// [`SpeechToolCallStateConflict`] carrying the held state when it is not Proposed.
    pub fn answer(
        &self,
        result: Json,
    ) -> Result<(Self, SpeechToolCallAnswered), SpeechToolCallStateConflict> {
        if self.state != SpeechToolCallState::Proposed {
            return Err(SpeechToolCallStateConflict {
                current: self.state,
            });
        }
        let event = SpeechToolCallAnswered {
            call_id: self.call_id.clone(),
            result,
        };
        let moved = Self {
            state: SpeechToolCallState::Answered,
            ..self.clone()
        };
        Ok((moved, event))
    }
}

/// `inference.duplex.ReceivedSessionEvent`: an event a provider sent an open session, held until
/// the caller has it. A close or failure of the session delivers every event still held before its
/// own end event, so nothing received before the end is lost to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceivedSessionEvent {
    received_id: ReceivedEventId,
    session_id: SpeechSessionId,
    kind: ReceivedEventKind,
    sequence: i64,
    delivered_before: Option<SessionEnd>,
    state: ReceivedSessionEventState,
}

impl ReceivedSessionEvent {
    /// Runs `inference.duplex.ReceiveSessionEvent` on `session` under `received_id`.
    ///
    /// # Errors
    ///
    /// [`SpeechSessionStateConflict`] carrying the session's state when it is not Open: what a
    /// provider sends after the end is not received.
    pub const fn receive_as(
        received_id: ReceivedEventId,
        session: &SpeechSession,
        kind: ReceivedEventKind,
        sequence: i64,
    ) -> Result<(Self, SessionEventReceived), SpeechSessionStateConflict> {
        if let Err(conflict) = session.require_open() {
            return Err(conflict);
        }
        let session_id = session.session_id();
        let event = SessionEventReceived {
            received_id,
            session_id,
            kind,
            sequence,
        };
        let received = Self {
            received_id,
            session_id,
            kind,
            sequence,
            delivered_before: None,
            state: ReceivedSessionEventState::Received,
        };
        Ok((received, event))
    }

    #[must_use]
    pub const fn received_id(&self) -> ReceivedEventId {
        self.received_id
    }

    #[must_use]
    pub const fn session_id(&self) -> SpeechSessionId {
        self.session_id
    }

    #[must_use]
    pub const fn kind(&self) -> ReceivedEventKind {
        self.kind
    }

    #[must_use]
    pub const fn sequence(&self) -> i64 {
        self.sequence
    }

    /// The end event this one was delivered before, when a close or failure delivered it.
    #[must_use]
    pub const fn delivered_before(&self) -> Option<SessionEnd> {
        self.delivered_before
    }

    #[must_use]
    pub const fn state(&self) -> ReceivedSessionEventState {
        self.state
    }

    /// Runs `inference.duplex.DeliverSessionEvent`: the caller has the event.
    ///
    /// # Errors
    ///
    /// [`ReceivedSessionEventStateConflict`] carrying the held state when it is not Received.
    pub const fn deliver(
        &self,
    ) -> Result<(Self, SessionEventDelivered), ReceivedSessionEventStateConflict> {
        if !matches!(self.state, ReceivedSessionEventState::Received) {
            return Err(ReceivedSessionEventStateConflict {
                current: self.state,
            });
        }
        let event = SessionEventDelivered {
            received_id: self.received_id,
        };
        let moved = Self {
            state: ReceivedSessionEventState::Delivered,
            ..*self
        };
        Ok((moved, event))
    }

    /// The effect of `CloseSpeechSession` or `FailSpeechSession` on an event of its session: one
    /// still Received is delivered before `end`; one already Delivered is left alone (`None`).
    #[must_use]
    pub const fn delivered_before_end(&self, end: SessionEnd) -> Option<Self> {
        match self.state {
            ReceivedSessionEventState::Received => Some(Self {
                delivered_before: Some(end),
                state: ReceivedSessionEventState::Delivered,
                ..*self
            }),
            ReceivedSessionEventState::Delivered => None,
        }
    }
}
