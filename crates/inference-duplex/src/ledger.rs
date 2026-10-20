//! The record every implementation keeps of one open session.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use inference_audio::{AudioFormat, AudioFrame, SampleClockPosition};
use inference_core::model::Adapter;
use inference_tools::{Json, Tool};

use crate::{
    InputAccepted, InputCommitted, InputTranscribed, OpenError, OpenSpeechSession, OutputAudio,
    OutputTranscriptDelta, OutputTruncated, ProposeSpeechToolCall, Response, ResponseId,
    ResponseIdReused, ResponseNotFound, ResponseUsage, ResponseUsageRecorded, SessionConfig,
    SessionError, SessionEvent, SessionFailure, SessionFailureKind, SpeechCallId,
    SpeechCallIdReused, SpeechCapabilities, SpeechSession, SpeechSessionId, SpeechSessionOpened,
    SpeechStarted, SpeechStopped, SpeechToolCall, TranscriptionUsage, TranscriptionUsageRecorded,
    Unsupported,
};

/// The liveness bound of a session: when the provider was last heard from, and how long it may
/// stay silent. Time is passed in, never read here, so a mock can run it on a clock of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Liveness {
    limit: Duration,
    last_heard: Instant,
}

impl Liveness {
    /// A bound of `limit`, the provider heard from at `now`.
    #[must_use]
    pub const fn new(limit: Duration, now: Instant) -> Self {
        Self {
            limit,
            last_heard: now,
        }
    }

    #[must_use]
    pub const fn limit(&self) -> Duration {
        self.limit
    }

    /// The provider was heard from at `now`; an earlier instant than the last changes nothing.
    pub fn heard(&mut self, now: Instant) {
        self.last_heard = self.last_heard.max(now);
    }

    /// Whether the provider has been silent for the whole bound at `now`.
    #[must_use]
    pub fn expired(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.last_heard) >= self.limit
    }
}

/// One open session as every implementation records it: the [`SpeechSession`], its
/// [`Response`]s and [`SpeechToolCall`]s, the latest usage of each response, the tools it
/// offers, the liveness bound, and the events recorded and not yet delivered.
///
/// The caller's operations and the provider's events both go through it. Each checks what the
/// specification and this contract decide, records its event, or returns the [`SessionError`]
/// and changes nothing; an adapter drops a provider event the ledger refuses, such as late audio
/// of a cancelled response.
///
/// Identities are never reused: a response id the adapter issued, or a call id the provider
/// gave, that the session already holds is refused. Closing the session cancels the response
/// in progress and failing it fails that response, their events before the session's, and no
/// output is recorded after.
///
/// The liveness bound runs while the caller awaits the provider: from a request for a response,
/// from a commit of the input when `auto_response` is on, or, when the model creates every
/// response, from the answer to a call, until the response ends. A session nobody awaits
/// anything of is idle, not stalled.
#[derive(Debug, Clone)]
pub struct SessionLedger {
    session: SpeechSession,
    capabilities: SpeechCapabilities,
    tools: Vec<Tool>,
    responses: Vec<Response>,
    emitted: Vec<u64>,
    usage: Vec<Option<ResponseUsage>>,
    calls: Vec<SpeechToolCall>,
    liveness: Liveness,
    requested: bool,
    /// The bound was armed without an instant: it starts at the next liveness check.
    clock_pending: bool,
    outbox: VecDeque<SessionEvent>,
}

impl SessionLedger {
    /// Runs `inference.duplex.OpenSpeechSession` for `adapter` under `session_id` with `config`,
    /// once `capabilities` admit it; the provider is heard from at `now`.
    ///
    /// # Errors
    ///
    /// [`OpenError::Unsupported`] as [`SpeechCapabilities::admit`] decides, then
    /// [`OpenError::NoLivenessBound`]; nothing is opened.
    pub fn open(
        session_id: SpeechSessionId,
        adapter: &Adapter,
        capabilities: &SpeechCapabilities,
        config: SessionConfig,
        now: Instant,
    ) -> Result<(Self, SpeechSessionOpened), OpenError> {
        capabilities
            .admit(&config)
            .map_err(OpenError::Unsupported)?;
        let millis = |duration: Duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX);
        let record = OpenSpeechSession {
            adapter_id: adapter.adapter_id(),
            voice: config.voice.clone(),
            instructions: config.instructions.clone(),
            input_format: config.input_format,
            output_format: config.output_format,
            turn_detection: config.turn_detection,
            auto_response: config.auto_response,
            turn_silence_ms: config.turn_tuning.silence.map(millis),
            turn_sensitivity: config.turn_tuning.sensitivity,
            turn_prefix_padding_ms: config.turn_tuning.prefix_padding.map(millis),
            auto_interrupt: config.auto_interrupt,
            tools: config.tool_ids(),
            idle_limit_ms: millis(config.idle_limit),
        };
        let (session, opened) =
            SpeechSession::open_as(session_id, record).map_err(OpenError::NoLivenessBound)?;
        let ledger = Self {
            session,
            capabilities: capabilities.clone(),
            tools: config.tools,
            responses: Vec::new(),
            emitted: Vec::new(),
            usage: Vec::new(),
            calls: Vec::new(),
            liveness: Liveness::new(config.idle_limit, now),
            requested: false,
            clock_pending: false,
            outbox: VecDeque::new(),
        };
        Ok((ledger, opened))
    }

    #[must_use]
    pub const fn session(&self) -> &SpeechSession {
        &self.session
    }

    #[must_use]
    pub const fn capabilities(&self) -> &SpeechCapabilities {
        &self.capabilities
    }

    /// The tools the session offers.
    #[must_use]
    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    /// Every response, in the order they were created.
    #[must_use]
    pub fn responses(&self) -> &[Response] {
        &self.responses
    }

    /// The response carrying `response_id`, if the session created one.
    #[must_use]
    pub fn response(&self, response_id: ResponseId) -> Option<&Response> {
        self.responses
            .iter()
            .find(|response| response.response_id() == response_id)
    }

    /// The latest usage recorded for the response, the running total of it so far.
    #[must_use]
    pub fn usage(&self, response_id: ResponseId) -> Option<ResponseUsage> {
        let index = self
            .responses
            .iter()
            .position(|response| response.response_id() == response_id)?;
        self.usage[index]
    }

    /// The response in progress, if one is.
    #[must_use]
    pub fn in_progress(&self) -> Option<ResponseId> {
        self.responses
            .iter()
            .find(|response| response.require_in_progress().is_ok())
            .map(Response::response_id)
    }

    /// Every call, in the order they were proposed.
    #[must_use]
    pub fn calls(&self) -> &[SpeechToolCall] {
        &self.calls
    }

    /// The call carrying `call_id`, if a response proposed one.
    #[must_use]
    pub fn call(&self, call_id: &SpeechCallId) -> Option<&SpeechToolCall> {
        self.calls.iter().find(|call| call.call_id() == call_id)
    }

    #[must_use]
    pub const fn liveness(&self) -> &Liveness {
        &self.liveness
    }

    /// How many recorded events are not yet delivered.
    #[must_use]
    pub fn undelivered(&self) -> usize {
        self.outbox.len()
    }

    /// The next recorded event not yet delivered.
    pub fn next_event(&mut self) -> Option<SessionEvent> {
        self.outbox.pop_front()
    }

    /// The caller sends `frame`: `inference.duplex.AppendInputAudio`, accepted whether or not a
    /// response is in progress.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`], then [`SessionError::FormatMismatch`].
    pub fn accept_input(&mut self, frame: AudioFrame) -> Result<(), SessionError> {
        self.require_open()?;
        Self::same_format(*self.session.input_format(), frame.format)?;
        let session_id = self.session.session_id();
        self.outbox
            .push_back(SessionEvent::InputAccepted(InputAccepted {
                session_id,
                frame,
            }));
        Ok(())
    }

    /// The caller commits the input at `now`: `inference.duplex.CommitInput`. With
    /// `auto_response` on it is a request for a response, and the liveness bound runs from `now`.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`], then [`SessionError::Unsupported`] when the model takes no
    /// commit.
    pub fn commit_input(&mut self, now: Instant) -> Result<(), SessionError> {
        self.require_open()?;
        if !self.capabilities.manual_commit {
            return Err(SessionError::Unsupported(Unsupported::ManualCommit));
        }
        self.record_commit(now);
        Ok(())
    }

    /// Whether the caller may ask for a response now; the response itself is recorded by
    /// [`SessionLedger::response_created`] when the provider starts it. The liveness bound runs
    /// from `now`.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`], [`SessionError::Unsupported`] when the model creates every
    /// response itself, then [`SessionError::ResponseInProgress`].
    pub fn request_response(&mut self, now: Instant) -> Result<(), SessionError> {
        self.require_open()?;
        if !self.capabilities.manual_response {
            return Err(SessionError::Unsupported(Unsupported::ManualResponse));
        }
        self.require_none_in_progress()?;
        self.await_response(now);
        Ok(())
    }

    /// The caller cancels the response, barge-in decided by the caller:
    /// `inference.duplex.CancelResponse`. Its later output is refused from now on.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`], [`SessionError::Unsupported`] with
    /// [`Unsupported::ClientCancel`] when the model takes no cancellation from the caller,
    /// [`SessionError::UnknownResponse`], then [`SessionError::ResponseNotInProgress`].
    pub fn cancel(&mut self, response_id: ResponseId) -> Result<(), SessionError> {
        self.require_open()?;
        if !self.capabilities.client_cancel {
            return Err(SessionError::Unsupported(Unsupported::ClientCancel));
        }
        self.cancel_response(response_id)
    }

    /// The provider cancelled the response by its own interruption:
    /// `inference.duplex.CancelResponse`. Its later output is refused from now on.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`], [`SessionError::UnknownResponse`], then
    /// [`SessionError::ResponseNotInProgress`].
    pub fn response_cancelled(
        &mut self,
        response_id: ResponseId,
        now: Instant,
    ) -> Result<(), SessionError> {
        self.require_open()?;
        self.cancel_response(response_id)?;
        self.heard(now);
        Ok(())
    }

    /// The caller cuts the response's audio at `heard`: `inference.duplex.TruncateOutput`.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`], [`SessionError::Unsupported`] when the model truncates nothing,
    /// [`SessionError::UnknownResponse`], then [`SessionError::BeyondOutput`] when `heard` lies
    /// past the samples the response emitted.
    pub fn truncate(
        &mut self,
        response_id: ResponseId,
        heard: SampleClockPosition,
    ) -> Result<(), SessionError> {
        self.require_open()?;
        if !self.capabilities.truncation {
            return Err(SessionError::Unsupported(Unsupported::Truncation));
        }
        let index = self.index_of(response_id)?;
        let emitted = self.emitted[index];
        if heard.get() > emitted {
            return Err(SessionError::BeyondOutput { heard, emitted });
        }
        self.outbox
            .push_back(SessionEvent::OutputTruncated(OutputTruncated {
                response_id,
                heard,
            }));
        Ok(())
    }

    /// The caller answers a call: `inference.duplex.AnswerSpeechToolCall`. A model that creates
    /// every response owes a new one that answers the result, so the liveness bound runs, as
    /// after a commit, until that response begins, also when the answer comes before the
    /// response that proposed the call ended. Time is not passed here: with no response in
    /// progress the bound runs from the next [`SessionLedger::check_liveness`], so the time the
    /// caller took to compute the result does not count.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`], [`SessionError::UnknownCall`], then
    /// [`SessionError::CallConflict`].
    pub fn answer(&mut self, call_id: &SpeechCallId, result: Json) -> Result<(), SessionError> {
        self.require_open()?;
        let slot = self
            .calls
            .iter_mut()
            .find(|call| call.call_id() == call_id)
            .ok_or_else(|| SessionError::UnknownCall {
                call_id: call_id.clone(),
            })?;
        let (moved, event) = slot.answer(result).map_err(SessionError::CallConflict)?;
        *slot = moved;
        self.outbox
            .push_back(SessionEvent::SpeechToolCallAnswered(event));
        if !self.capabilities.manual_response {
            if !self.awaiting() {
                self.clock_pending = true;
            }
            self.requested = true;
        }
        Ok(())
    }

    /// The caller closes the session: `inference.duplex.CloseSpeechSession`. The response in
    /// progress is cancelled first, its `ResponseCancelled` before `SpeechSessionClosed`.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`].
    pub fn close(&mut self) -> Result<(), SessionError> {
        let (moved, event) = self.session.close().map_err(SessionError::NotOpen)?;
        if let Some(response_id) = self.in_progress() {
            self.cancel_response(response_id)?;
        }
        self.session = moved;
        self.requested = false;
        self.outbox
            .push_back(SessionEvent::SpeechSessionClosed(event));
        Ok(())
    }

    /// The session fails with `failure`: `inference.duplex.FailSpeechSession`, for a closed
    /// transport, an error the provider reported or a stall. The response in progress fails with
    /// it first, its `ResponseFailed` before `SpeechSessionFailed`.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`].
    pub fn fail(&mut self, failure: SessionFailure) -> Result<(), SessionError> {
        let (moved, event) = self
            .session
            .fail(failure.clone())
            .map_err(SessionError::NotOpen)?;
        if let Some(response_id) = self.in_progress() {
            self.fail_response(response_id, failure)?;
        }
        self.session = moved;
        self.requested = false;
        self.outbox
            .push_back(SessionEvent::SpeechSessionFailed(event));
        Ok(())
    }

    /// Fails an open session as stalled when the caller awaits the provider and it has been
    /// silent for the whole bound at `now`; returns whether it did.
    pub fn check_liveness(&mut self, now: Instant) -> bool {
        if self.session.require_open().is_err() || !self.awaiting() {
            return false;
        }
        if self.clock_pending {
            self.heard(now);
            return false;
        }
        if !self.liveness.expired(now) {
            return false;
        }
        let silent = now.saturating_duration_since(self.liveness.last_heard);
        self.fail(SessionFailure {
            kind: SessionFailureKind::Stalled,
            detail: format!(
                "the provider sent nothing for {} ms, the bound is {} ms",
                silent.as_millis(),
                self.liveness.limit.as_millis()
            ),
        })
        .is_ok()
    }

    /// The provider was heard from at `now`, whatever it sent.
    pub fn heard(&mut self, now: Instant) {
        self.liveness.heard(now);
        self.clock_pending = false;
    }

    /// The provider's turn detection committed the input. With `auto_response` on it owes a
    /// response from `now`.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`].
    pub fn input_committed(&mut self, now: Instant) -> Result<(), SessionError> {
        self.require_open()?;
        self.heard(now);
        self.record_commit(now);
        Ok(())
    }

    /// The provider detected speech starting at `position` of the input.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`].
    pub fn speech_started(
        &mut self,
        position: SampleClockPosition,
        now: Instant,
    ) -> Result<(), SessionError> {
        self.require_open()?;
        self.heard(now);
        let session_id = self.session.session_id();
        self.outbox
            .push_back(SessionEvent::SpeechStarted(SpeechStarted {
                session_id,
                position,
            }));
        Ok(())
    }

    /// The provider detected speech stopping at `position` of the input.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`].
    pub fn speech_stopped(
        &mut self,
        position: SampleClockPosition,
        now: Instant,
    ) -> Result<(), SessionError> {
        self.require_open()?;
        self.heard(now);
        let session_id = self.session.session_id();
        self.outbox
            .push_back(SessionEvent::SpeechStopped(SpeechStopped {
                session_id,
                position,
            }));
        Ok(())
    }

    /// The provider transcribed the input.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`].
    pub fn input_transcribed(&mut self, text: String, now: Instant) -> Result<(), SessionError> {
        self.require_open()?;
        self.heard(now);
        let session_id = self.session.session_id();
        self.outbox
            .push_back(SessionEvent::InputTranscribed(InputTranscribed {
                session_id,
                text,
            }));
        Ok(())
    }

    /// The provider reported the usage of transcribing the input, in any state of the session.
    ///
    /// # Errors
    ///
    /// None: the session record exists for as long as the ledger does.
    pub fn transcription_usage(
        &mut self,
        usage: TranscriptionUsage,
        now: Instant,
    ) -> Result<(), SessionError> {
        self.heard(now);
        let session_id = self.session.session_id();
        self.outbox
            .push_back(SessionEvent::TranscriptionUsageRecorded(
                TranscriptionUsageRecorded { session_id, usage },
            ));
        Ok(())
    }

    /// The provider started a response, which the adapter identifies by `response_id`, an
    /// identity it issues once.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`], [`SessionError::ResponseIdReused`], then
    /// [`SessionError::ResponseInProgress`].
    pub fn response_created(
        &mut self,
        response_id: ResponseId,
        now: Instant,
    ) -> Result<(), SessionError> {
        self.require_open()?;
        if self.response(response_id).is_some() {
            return Err(SessionError::ResponseIdReused(ResponseIdReused {
                response_id,
            }));
        }
        self.require_none_in_progress()?;
        let (response, event) =
            Response::create_as(response_id, &self.session).map_err(SessionError::NotOpen)?;
        self.heard(now);
        self.requested = false;
        self.responses.push(response);
        self.emitted.push(0);
        self.usage.push(None);
        self.outbox.push_back(SessionEvent::ResponseCreated(event));
        Ok(())
    }

    /// The provider sent audio of a response.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`], [`SessionError::UnknownResponse`],
    /// [`SessionError::ResponseNotInProgress`], then [`SessionError::FormatMismatch`] against the
    /// session's output format.
    pub fn output_audio(
        &mut self,
        response_id: ResponseId,
        frame: AudioFrame,
        now: Instant,
    ) -> Result<(), SessionError> {
        let index = self.in_progress_index(response_id)?;
        Self::same_format(*self.session.output_format(), frame.format)?;
        self.heard(now);
        self.emitted[index] += u64::from(frame.sample_count);
        self.outbox
            .push_back(SessionEvent::OutputAudio(OutputAudio {
                response_id,
                frame,
            }));
        Ok(())
    }

    /// The provider sent transcript of a response's audio.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`], [`SessionError::UnknownResponse`], then
    /// [`SessionError::ResponseNotInProgress`].
    pub fn output_transcript(
        &mut self,
        response_id: ResponseId,
        text: String,
        now: Instant,
    ) -> Result<(), SessionError> {
        self.in_progress_index(response_id)?;
        self.heard(now);
        self.outbox
            .push_back(SessionEvent::OutputTranscriptDelta(OutputTranscriptDelta {
                response_id,
                text,
            }));
        Ok(())
    }

    /// The provider finished a response.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`], [`SessionError::UnknownResponse`], then
    /// [`SessionError::ResponseNotInProgress`].
    pub fn response_completed(
        &mut self,
        response_id: ResponseId,
        now: Instant,
    ) -> Result<(), SessionError> {
        self.require_open()?;
        let index = self.index_of(response_id)?;
        let (moved, event) = self.responses[index]
            .complete()
            .map_err(SessionError::ResponseNotInProgress)?;
        self.heard(now);
        self.responses[index] = moved;
        self.outbox
            .push_back(SessionEvent::ResponseCompleted(event));
        Ok(())
    }

    /// The provider failed a response.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`], [`SessionError::UnknownResponse`], then
    /// [`SessionError::ResponseNotInProgress`].
    pub fn response_failed(
        &mut self,
        response_id: ResponseId,
        failure: SessionFailure,
        now: Instant,
    ) -> Result<(), SessionError> {
        self.require_open()?;
        self.fail_response(response_id, failure)?;
        self.heard(now);
        Ok(())
    }

    /// The provider reported the usage of a response, in any state of the response or the
    /// session: the running total of the response so far, which replaces the one recorded
    /// before.
    ///
    /// # Errors
    ///
    /// [`SessionError::UnknownResponse`].
    pub fn response_usage(
        &mut self,
        response_id: ResponseId,
        usage: ResponseUsage,
        now: Instant,
    ) -> Result<(), SessionError> {
        let index = self.index_of(response_id)?;
        self.heard(now);
        self.usage[index] = Some(usage);
        self.outbox
            .push_back(SessionEvent::ResponseUsageRecorded(ResponseUsageRecorded {
                response_id,
                usage,
            }));
        Ok(())
    }

    /// A response proposed a call of the session's tool named `tool`, with JSON arguments, under
    /// the provider's `call_id`.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`], [`SessionError::UnknownResponse`],
    /// [`SessionError::ResponseNotInProgress`], [`SessionError::CallIdReused`], then
    /// [`SessionError::ToolNotOffered`].
    pub fn tool_call(
        &mut self,
        call_id: SpeechCallId,
        response_id: ResponseId,
        tool: &str,
        arguments: Json,
        now: Instant,
    ) -> Result<(), SessionError> {
        let index = self.in_progress_index(response_id)?;
        if self.call(&call_id).is_some() {
            return Err(SessionError::CallIdReused(SpeechCallIdReused { call_id }));
        }
        let tool_id = self
            .tools
            .iter()
            .find(|offered| offered.name() == tool)
            .map(Tool::tool_id)
            .ok_or_else(|| SessionError::ToolNotOffered {
                name: tool.to_owned(),
            })?;
        let (call, event) = SpeechToolCall::propose_as(
            call_id,
            &self.responses[index],
            ProposeSpeechToolCall {
                response_id,
                tool_id,
                arguments,
            },
        )
        .map_err(SessionError::ResponseNotInProgress)?;
        self.heard(now);
        self.calls.push(call);
        self.outbox
            .push_back(SessionEvent::SpeechToolCallProposed(event));
        Ok(())
    }

    /// Whether the caller awaits the provider: a response requested or in progress.
    fn awaiting(&self) -> bool {
        self.requested || self.in_progress().is_some()
    }

    /// The caller awaits a response from `now`, unless it already awaits one.
    fn await_response(&mut self, now: Instant) {
        if !self.awaiting() {
            self.heard(now);
        }
        self.requested = true;
    }

    fn record_commit(&mut self, now: Instant) {
        if self.session.config().auto_response && self.in_progress().is_none() {
            self.await_response(now);
        }
        let session_id = self.session.session_id();
        self.outbox
            .push_back(SessionEvent::InputCommitted(InputCommitted { session_id }));
    }

    fn cancel_response(&mut self, response_id: ResponseId) -> Result<(), SessionError> {
        let index = self.index_of(response_id)?;
        let (moved, event) = self.responses[index]
            .cancel()
            .map_err(SessionError::ResponseNotInProgress)?;
        self.responses[index] = moved;
        self.outbox
            .push_back(SessionEvent::ResponseCancelled(event));
        Ok(())
    }

    fn fail_response(
        &mut self,
        response_id: ResponseId,
        failure: SessionFailure,
    ) -> Result<(), SessionError> {
        let index = self.index_of(response_id)?;
        let (moved, event) = self.responses[index]
            .fail(failure)
            .map_err(SessionError::ResponseNotInProgress)?;
        self.responses[index] = moved;
        self.outbox.push_back(SessionEvent::ResponseFailed(event));
        Ok(())
    }

    fn require_open(&self) -> Result<(), SessionError> {
        self.session.require_open().map_err(SessionError::NotOpen)
    }

    fn require_none_in_progress(&self) -> Result<(), SessionError> {
        match self.in_progress() {
            Some(response_id) => Err(SessionError::ResponseInProgress { response_id }),
            None => Ok(()),
        }
    }

    fn same_format(expected: AudioFormat, found: AudioFormat) -> Result<(), SessionError> {
        if expected == found {
            Ok(())
        } else {
            Err(SessionError::FormatMismatch { expected, found })
        }
    }

    fn index_of(&self, response_id: ResponseId) -> Result<usize, SessionError> {
        self.responses
            .iter()
            .position(|response| response.response_id() == response_id)
            .ok_or(SessionError::UnknownResponse(ResponseNotFound {
                response_id,
            }))
    }

    /// The response, which must be InProgress in an open session.
    fn in_progress_index(&self, response_id: ResponseId) -> Result<usize, SessionError> {
        self.require_open()?;
        let index = self.index_of(response_id)?;
        self.responses[index]
            .require_in_progress()
            .map_err(SessionError::ResponseNotInProgress)?;
        Ok(index)
    }
}
