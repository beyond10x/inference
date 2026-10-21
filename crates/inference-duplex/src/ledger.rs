//! The record every implementation keeps of one open session.

use std::collections::VecDeque;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use inference_audio::{AudioFormat, AudioFrame, SampleClockPosition};
use inference_core::model::{Adapter, ModelId};
use inference_core::usage::{Quantity, QuantityKind, RecordUsage, UsageRecord};
use inference_tools::{Json, Tool};

use crate::{
    DeliverSessionEventError, InputAccepted, InputCommitted, InputTranscribed,
    NonPositiveIdleLimit, OpenError, OpenSpeechSession, OutputAudio, OutputTranscriptDelta,
    OutputTruncated, ProposeSpeechToolCall, ReceivedEventId, ReceivedEventKind,
    ReceivedSessionEvent, Response, ResponseId, ResponseIdReused, ResponseNotFound,
    ResponseStateConflict, ResponseUsage, ResponseUsageRecorded, SessionConfig, SessionEnd,
    SessionError, SessionEvent, SessionEventDelivered, SessionEventReceived, SessionFailure,
    SessionFailureKind, SpeechCallId, SpeechCallIdReused, SpeechCapabilities, SpeechSession,
    SpeechSessionId, SpeechSessionOpened, SpeechSessionStateConflict, SpeechStarted, SpeechStopped,
    SpeechToolCall, TranscriptionUsage, TranscriptionUsageRecorded, TuningSupport, Unsupported,
    UsageTotal,
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
///
/// Usage is recorded as `inference.usage` records, [`SessionEvent::UsageRecorded`], each naming
/// the session's speech session, adapter and the adapter's model, and no run, node or session
/// of a graph: a native speech session runs in none.
///
/// * Each response is recorded once, from the last quantities its provider reported, with every
///   token kind it left out `unknown`, and `requests` 1 as `measured`. The record is due when the
///   response has ended (completed, cancelled or failed) and its usage is settled: by a report of
///   its [`UsageTotal::Final`] total, or else by the creation of the next response or the end of
///   the session, since a provider whose reports are running totals may report after a response
///   ended. A response that ends settled is recorded just before its end event, and the session's
///   end settles the response it cancels or fails; one settled later is recorded by its final
///   report at once, by the next response just before that response's `ResponseCreated`, by the
///   session's end before the end events it records. A response with no usage reported is
///   recorded with every token count unknown.
/// * A transcription's usage is recorded when reported, and so is usage of the whole session
///   ([`SessionLedger::session_usage`]); neither names a response.
/// * Usage no response holds yet ([`SessionLedger::hold_usage`]) is recorded at the session's
///   end, before its end events, as usage of the session naming no response.
///
/// Records carry wall-clock times: the ledger reads the system clock once, at opening, and
/// places every instant it is given relative to it, so a mock running a clock of its own records
/// consistent times. A response starts when it is created and ends at the instant it ended; one
/// the caller cancelled, or a close or failure ended, ends at the latest instant the ledger was
/// given.
#[derive(Debug, Clone)]
pub struct SessionLedger {
    session: SpeechSession,
    model_id: ModelId,
    capabilities: SpeechCapabilities,
    tools: Vec<Tool>,
    responses: Vec<Response>,
    emitted: Vec<u64>,
    usage: Vec<Option<ResponseUsage>>,
    meters: Vec<Meter>,
    records: Vec<UsageRecord>,
    /// Usage the provider reported that no response holds yet ([`SessionLedger::hold_usage`]).
    held: Vec<Quantity>,
    /// The total of the session billed as a whole, recorded just before its end event
    /// ([`SessionLedger::session_usage_at_end`]), and whether the provider confirmed it final.
    at_end: Option<(Vec<Quantity>, bool)>,
    calls: Vec<SpeechToolCall>,
    liveness: Liveness,
    requested: bool,
    /// The bound was armed without an instant: it starts at the next liveness check.
    clock_pending: bool,
    /// How many frames the binding could not read, whole or in part
    /// ([`SessionLedger::unreadable_frame`]), and what the last one was.
    unreadable: (u64, Option<String>),
    /// The instant the session opened, and the wall clock then in Unix epoch milliseconds.
    opened: (Instant, i64),
    /// The latest instant the ledger was given.
    latest: Instant,
    /// The events recorded and not yet delivered, each with the received event it carries, if any.
    outbox: VecDeque<(SessionEvent, Option<ReceivedEventId>)>,
    /// The events the provider sent the open session, in the order received
    /// ([`SessionLedger::received_events`]).
    received: Vec<ReceivedSessionEvent>,
}

/// What the ledger keeps to record one response's usage.
#[derive(Debug, Clone)]
struct Meter {
    started: Instant,
    ended: Option<Instant>,
    /// The quantities the provider reported last.
    quantities: Vec<Quantity>,
    /// Whether no later report changes them.
    settled: bool,
    recorded: bool,
}

/// The token kinds every response record carries, in this order, `unknown` where the provider
/// left a count out.
const RESPONSE_TOKENS: [QuantityKind; 5] = [
    QuantityKind::InputTextTokens,
    QuantityKind::InputAudioTokens,
    QuantityKind::CachedInputTokens,
    QuantityKind::OutputTextTokens,
    QuantityKind::OutputAudioTokens,
];

/// The quantities of a response record: the token kinds of [`RESPONSE_TOKENS`], each as reported
/// or `unknown`, then any other kind reported, then `requests` 1 as measured unless reported.
fn response_quantities(reported: Vec<Quantity>) -> Vec<Quantity> {
    let mut quantities: Vec<Quantity> = RESPONSE_TOKENS.map(Quantity::unknown).to_vec();
    for quantity in reported {
        match quantities
            .iter_mut()
            .find(|known| known.kind() == quantity.kind())
        {
            Some(slot) => *slot = quantity,
            None => quantities.push(quantity),
        }
    }
    if !quantities
        .iter()
        .any(|quantity| quantity.kind() == QuantityKind::Requests)
    {
        quantities.push(Quantity::measured(QuantityKind::Requests, 1));
    }
    quantities
}

/// Milliseconds as the ESS `Integer`, saturated.
fn millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
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
        let ledger = Self::of(
            session,
            adapter.model_id(),
            capabilities.clone(),
            config.tools,
            config.idle_limit,
            now,
        );
        Ok((ledger, opened))
    }

    /// Runs `inference.duplex.OpenSpeechSession` with `record` as the command gives it, under
    /// `session_id`, for a store that holds sessions by their record rather than for a binding:
    /// the record is the whole configuration, so the capabilities are the ones it names, and the
    /// session offers no tool to call, since the record names its tools only by identity. The
    /// provider is heard from at `now`.
    ///
    /// # Errors
    ///
    /// [`NonPositiveIdleLimit`]; nothing is opened.
    pub fn open_record(
        session_id: SpeechSessionId,
        model_id: ModelId,
        record: OpenSpeechSession,
        now: Instant,
    ) -> Result<(Self, SpeechSessionOpened), NonPositiveIdleLimit> {
        let (session, opened) = SpeechSession::open_as(session_id, record)?;
        let config = session.config();
        let capabilities = SpeechCapabilities {
            input_formats: vec![config.input_format],
            output_formats: vec![config.output_format],
            turn_detection: vec![config.turn_detection],
            turn_tuning: TuningSupport::ALL,
            manual_commit: true,
            manual_response: true,
            manual_interrupt: true,
            client_cancel: true,
            truncation: true,
            tool_calling: true,
        };
        let idle_limit = Duration::from_millis(u64::try_from(config.idle_limit_ms).unwrap_or(0));
        let ledger = Self::of(session, model_id, capabilities, Vec::new(), idle_limit, now);
        Ok((ledger, opened))
    }

    /// The ledger of `session`, just opened at `now`.
    fn of(
        session: SpeechSession,
        model_id: ModelId,
        capabilities: SpeechCapabilities,
        tools: Vec<Tool>,
        idle_limit: Duration,
        now: Instant,
    ) -> Self {
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, millis);
        Self {
            session,
            model_id,
            capabilities,
            tools,
            responses: Vec::new(),
            emitted: Vec::new(),
            usage: Vec::new(),
            meters: Vec::new(),
            records: Vec::new(),
            held: Vec::new(),
            at_end: None,
            calls: Vec::new(),
            liveness: Liveness::new(idle_limit, now),
            requested: false,
            clock_pending: false,
            unreadable: (0, None),
            opened: (now, wall),
            latest: now,
            outbox: VecDeque::new(),
            received: Vec::new(),
        }
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

    /// The next recorded event not yet delivered. When it carries an event the provider sent the
    /// open session that is still held, the caller now has that one too:
    /// `inference.duplex.DeliverSessionEvent`.
    pub fn next_event(&mut self) -> Option<SessionEvent> {
        let (event, received_id) = self.outbox.pop_front()?;
        if let Some(received_id) = received_id {
            let _ = self.deliver_event(received_id);
        }
        Some(event)
    }

    /// Every event the provider sent the open session, in the order received: its output, its
    /// input transcripts, its usage, its tool calls and the ends of its responses, each held
    /// until the caller has it.
    #[must_use]
    pub fn received_events(&self) -> &[ReceivedSessionEvent] {
        &self.received
    }

    /// The provider sent the open session an event of `kind`, the `sequence`th it sent, under
    /// `received_id`: `inference.duplex.ReceiveSessionEvent`. It is held until the caller has it
    /// ([`Self::deliver_event`]); a close or failure delivers it before its end event. The ledger
    /// records the events it builds from the provider's traffic this way itself.
    ///
    /// # Errors
    ///
    /// [`SpeechSessionStateConflict`] when the session is not Open: what a provider sends after
    /// the end is not received. Nothing is held.
    pub fn receive_event(
        &mut self,
        received_id: ReceivedEventId,
        kind: ReceivedEventKind,
        sequence: i64,
    ) -> Result<SessionEventReceived, SpeechSessionStateConflict> {
        let (received, event) =
            ReceivedSessionEvent::receive_as(received_id, &self.session, kind, sequence)?;
        self.received.push(received);
        Ok(event)
    }

    /// The caller has the received event carrying `received_id`:
    /// `inference.duplex.DeliverSessionEvent`.
    ///
    /// # Errors
    ///
    /// [`DeliverSessionEventError::Unknown`] when the session received no such event, or
    /// [`DeliverSessionEventError::Conflict`] when it is no longer held; nothing moved.
    pub fn deliver_event(
        &mut self,
        received_id: ReceivedEventId,
    ) -> Result<SessionEventDelivered, DeliverSessionEventError> {
        let slot = self
            .received
            .iter_mut()
            .find(|received| received.received_id() == received_id)
            .ok_or(DeliverSessionEventError::Unknown { received_id })?;
        let (moved, event) = slot.deliver().map_err(DeliverSessionEventError::Conflict)?;
        *slot = moved;
        Ok(event)
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
        self.push(SessionEvent::InputAccepted(InputAccepted {
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
        self.tick(now);
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
        self.tick(now);
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
        self.cancel_response(response_id, self.latest)
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
        self.cancel_response(response_id, now)?;
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
        self.push(SessionEvent::OutputTruncated(OutputTruncated {
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
        self.push(SessionEvent::SpeechToolCallAnswered(event));
        if !self.capabilities.manual_response {
            if !self.awaiting() {
                self.clock_pending = true;
            }
            self.requested = true;
        }
        Ok(())
    }

    /// The caller closes the session: `inference.duplex.CloseSpeechSession`. The response in
    /// progress is cancelled first, its `ResponseCancelled` just before the total of the session
    /// ([`Self::session_usage_at_end`]) and `SpeechSessionClosed`; every response not yet
    /// recorded, and any usage held for no response, is recorded before both. Every event the
    /// provider sent that is still held is delivered before `SpeechSessionClosed`
    /// (`delivered_before: closed`), and stays in order before it.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`].
    pub fn close(&mut self) -> Result<(), SessionError> {
        let (moved, event) = self.session.close().map_err(SessionError::NotOpen)?;
        self.settle_for_end();
        if let Some(response_id) = self.in_progress() {
            self.cancel_response(response_id, self.latest)?;
        }
        self.record_at_end();
        self.deliver_held(SessionEnd::Closed);
        self.session = moved;
        self.requested = false;
        self.push(SessionEvent::SpeechSessionClosed(event));
        Ok(())
    }

    /// The session fails with `failure`: `inference.duplex.FailSpeechSession`, for a closed
    /// transport, an error the provider reported or a stall. The response in progress fails with
    /// it first, its `ResponseFailed` just before the total of the session
    /// ([`Self::session_usage_at_end`]) and `SpeechSessionFailed`; every response not yet
    /// recorded, and any usage held for no response, is recorded before both. Every event the
    /// provider sent that is still held is delivered before `SpeechSessionFailed`
    /// (`delivered_before: failed`), and stays in order before it.
    ///
    /// When the provider had sent frames the binding could not read
    /// ([`Self::unreadable_frame`]), the failure is reported as
    /// [`SessionFailureKind::ProviderError`] whatever its kind, its detail naming how many and
    /// the last one: a session the provider stopped speaking readably to has not just stalled or
    /// lost its transport.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`].
    pub fn fail(&mut self, failure: SessionFailure) -> Result<(), SessionError> {
        let failure = self.after_unreadable(failure);
        let (moved, event) = self
            .session
            .fail(failure.clone())
            .map_err(SessionError::NotOpen)?;
        self.settle_for_end();
        if let Some(response_id) = self.in_progress() {
            self.fail_response(response_id, failure, self.latest)?;
        }
        self.record_at_end();
        self.deliver_held(SessionEnd::Failed);
        self.session = moved;
        self.requested = false;
        self.push(SessionEvent::SpeechSessionFailed(event));
        Ok(())
    }

    /// Fails an open session as stalled when the caller awaits the provider and it has been
    /// silent for the whole bound at `now`; returns whether it did.
    pub fn check_liveness(&mut self, now: Instant) -> bool {
        self.tick(now);
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
                "the provider sent nothing readable for {} ms, the bound is {} ms",
                silent.as_millis(),
                self.liveness.limit.as_millis()
            ),
        })
        .is_ok()
    }

    /// The provider was heard from at `now`, whatever it sent.
    pub fn heard(&mut self, now: Instant) {
        self.tick(now);
        self.liveness.heard(now);
        self.clock_pending = false;
    }

    /// The provider sent, at `now`, a frame the binding could not read, whole or in part;
    /// `what` names it: its type, or the start of its text. The binding reads on past it, and
    /// it ends nothing, but it is no sign of life: the liveness bound runs on as if the provider
    /// had sent nothing. It is counted, and a later failure of the session names it
    /// ([`Self::fail`]).
    pub fn unreadable_frame(&mut self, what: impl Into<String>, now: Instant) {
        self.tick(now);
        self.unreadable.0 = self.unreadable.0.saturating_add(1);
        self.unreadable.1 = Some(what.into());
    }

    /// How many frames the binding could not read ([`Self::unreadable_frame`]).
    #[must_use]
    pub const fn unreadable_frames(&self) -> u64 {
        self.unreadable.0
    }

    /// What the last frame the binding could not read was.
    #[must_use]
    pub fn last_unreadable(&self) -> Option<&str> {
        self.unreadable.1.as_deref()
    }

    /// `failure` as the session reports it: a provider error naming the frames the binding
    /// could not read, when there were any.
    fn after_unreadable(&self, failure: SessionFailure) -> SessionFailure {
        let (count, Some(last)) = (self.unreadable.0, self.unreadable.1.as_deref()) else {
            return failure;
        };
        let frames = if count == 1 { "frame" } else { "frames" };
        SessionFailure {
            kind: SessionFailureKind::ProviderError,
            detail: format!(
                "{}; the provider had sent {count} {frames} the binding could not read, the last: {last}",
                failure.detail
            ),
        }
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
        self.push(SessionEvent::SpeechStarted(SpeechStarted {
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
        self.push(SessionEvent::SpeechStopped(SpeechStopped {
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
        self.push_received(
            ReceivedEventKind::InputTranscript,
            SessionEvent::InputTranscribed(InputTranscribed { session_id, text }),
        );
        Ok(())
    }

    /// The provider reported the usage of transcribing the input, in any state of the session:
    /// its `TranscriptionUsageRecorded` and its usage record, each count as reported.
    ///
    /// # Errors
    ///
    /// None: the session record exists for as long as the ledger does.
    pub fn transcription_usage(
        &mut self,
        usage: TranscriptionUsage,
        now: Instant,
    ) -> Result<(), SessionError> {
        self.transcription_usage_reported(Some(usage), usage.quantities(), now)
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
        self.settle_ended();
        self.responses.push(response);
        self.emitted.push(0);
        self.usage.push(None);
        self.meters.push(Meter {
            started: now,
            ended: None,
            quantities: Vec::new(),
            settled: false,
            recorded: false,
        });
        self.push(SessionEvent::ResponseCreated(event));
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
        self.push_received(
            ReceivedEventKind::OutputAudio,
            SessionEvent::OutputAudio(OutputAudio { response_id, frame }),
        );
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
        self.push_received(
            ReceivedEventKind::OutputTranscript,
            SessionEvent::OutputTranscriptDelta(OutputTranscriptDelta { response_id, text }),
        );
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
        self.ended(index, now);
        self.push_received(
            ReceivedEventKind::ResponseEnd,
            SessionEvent::ResponseCompleted(event),
        );
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
        self.fail_response(response_id, failure, now)?;
        self.heard(now);
        Ok(())
    }

    /// The provider ended a response at `now` with a frame whose outcome the binding could not
    /// read; `what` names what it lacked. The response fails as a provider error naming it, and
    /// the frame is counted as one the binding could not read ([`Self::unreadable_frame`]).
    /// Whether the provider meant the response to be done is unknown, so the caller still awaits
    /// it: the liveness bound runs on from `now` until the provider creates a response, and a
    /// provider that stays silent fails the session.
    ///
    /// # Errors
    ///
    /// [`SessionError::NotOpen`], [`SessionError::UnknownResponse`], then
    /// [`SessionError::ResponseNotInProgress`].
    pub fn response_ended_unread(
        &mut self,
        response_id: ResponseId,
        what: impl Into<String>,
        now: Instant,
    ) -> Result<(), SessionError> {
        self.require_open()?;
        let what = what.into();
        self.fail_response(
            response_id,
            SessionFailure {
                kind: SessionFailureKind::ProviderError,
                detail: format!("the provider ended the response unreadably: {what}"),
            },
            now,
        )?;
        self.heard(now);
        self.unreadable_frame(what, now);
        self.requested = true;
        Ok(())
    }

    /// The provider reported the usage of a response, in any state of the response or the
    /// session: the running total of the response so far, which replaces the one recorded
    /// before. It is for a provider that reports every count and whose report is the final
    /// total: until the response's usage is recorded, its record takes every count as
    /// reported ([`ResponseUsage::quantities`]), [`UsageTotal::Final`].
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
        self.record_response_usage(index, usage, now);
        if !self.meters[index].recorded {
            self.count(index, usage.quantities(), UsageTotal::Final);
        }
        Ok(())
    }

    /// Records `ResponseUsageRecorded` for the response at `index`.
    fn record_response_usage(&mut self, index: usize, usage: ResponseUsage, now: Instant) {
        self.heard(now);
        self.usage[index] = Some(usage);
        let response_id = self.responses[index].response_id();
        self.push_received(
            ReceivedEventKind::Usage,
            SessionEvent::ResponseUsageRecorded(ResponseUsageRecorded { response_id, usage }),
        );
    }

    /// The provider reported the usage of a response as `quantities`, built from its own fields
    /// so that a count it left out is unknown, and, where the contract's form can carry it, as
    /// `usage`, recorded as [`SessionLedger::response_usage`] records it. `total` says whether
    /// a later report may still replace it.
    ///
    /// # Errors
    ///
    /// [`SessionError::UnknownResponse`], then [`SessionError::ResponseNotInProgress`] when the
    /// response's usage was already recorded; `usage` is recorded all the same.
    pub fn response_usage_reported(
        &mut self,
        response_id: ResponseId,
        usage: Option<ResponseUsage>,
        quantities: Vec<Quantity>,
        total: UsageTotal,
        now: Instant,
    ) -> Result<(), SessionError> {
        let index = self.index_of(response_id)?;
        match usage {
            Some(usage) => self.record_response_usage(index, usage, now),
            None => self.heard(now),
        }
        if self.meters[index].recorded {
            return Err(SessionError::ResponseNotInProgress(ResponseStateConflict {
                current: self.responses[index].state(),
            }));
        }
        self.count(index, quantities, total);
        Ok(())
    }

    /// The provider reported the usage of transcribing the input as `quantities`, and, where the
    /// contract's form can carry it, as `usage`, recorded as
    /// [`SessionLedger::transcription_usage`] records it; in any state of the session.
    ///
    /// # Errors
    ///
    /// None: the session record exists for as long as the ledger does.
    pub fn transcription_usage_reported(
        &mut self,
        usage: Option<TranscriptionUsage>,
        quantities: Vec<Quantity>,
        now: Instant,
    ) -> Result<(), SessionError> {
        self.heard(now);
        let session_id = self.session.session_id();
        if let Some(usage) = usage {
            self.push_received(
                ReceivedEventKind::Usage,
                SessionEvent::TranscriptionUsageRecorded(TranscriptionUsageRecorded {
                    session_id,
                    usage,
                }),
            );
        }
        self.record_of_session(quantities, true, now, now);
        Ok(())
    }

    /// The provider reported usage of the session as a whole, not of one response, such as the
    /// seconds a provider bills per session; in any state of the session. `confirmed` is false
    /// when the provider has not confirmed the counts final.
    ///
    /// # Errors
    ///
    /// None: the session record exists for as long as the ledger does.
    pub fn session_usage(
        &mut self,
        quantities: Vec<Quantity>,
        confirmed: bool,
        now: Instant,
    ) -> Result<(), SessionError> {
        self.heard(now);
        self.record_of_session(quantities, confirmed, self.opened.0, now);
        Ok(())
    }

    /// The provider reported usage that no response holds yet, as `quantities`, built from its
    /// own fields: input reported before the response it counts toward begins, for one. They
    /// replace what was held before, and an empty list holds nothing; an adapter releases them
    /// so once a response takes them. What is held when the session ends, by a close, a failure
    /// or a stall, is recorded as usage of the session, naming no response, before the end
    /// events; held after the end, it is recorded at once.
    pub fn hold_usage(&mut self, quantities: Vec<Quantity>) {
        if self.session.require_open().is_err() {
            if !quantities.is_empty() {
                self.record_of_session(quantities, true, self.opened.0, self.latest);
            }
            return;
        }
        self.held = quantities;
    }

    /// The provider bills the session as a whole, such as seconds of session, and reported its
    /// total so far as `quantities`; `confirmed` is false while the provider has not confirmed
    /// them final. They replace the total given before and are recorded once, as usage of the
    /// session naming no response, when a close, a failure or a stall ends it: after the end
    /// events of its responses, just before its own. Given after the end, they record nothing
    /// and push nothing: the total was recorded once, at the end.
    pub fn session_usage_at_end(&mut self, quantities: Vec<Quantity>, confirmed: bool) {
        if self.session.require_open().is_ok() {
            self.at_end = Some((quantities, confirmed));
        }
    }

    /// Every usage record of the session, in the order they were recorded.
    #[must_use]
    pub fn usage_records(&self) -> &[UsageRecord] {
        &self.records
    }

    /// The response at `index` reported `quantities` as its usage: they replace the ones
    /// before, and it is recorded at once when it ended and they settle it.
    fn count(&mut self, index: usize, quantities: Vec<Quantity>, total: UsageTotal) {
        let meter = &mut self.meters[index];
        meter.quantities = quantities;
        meter.settled |= total == UsageTotal::Final;
        if meter.settled && meter.ended.is_some() {
            self.record_response(index);
        }
    }

    /// The response at `index` ended at `at`; settled, it is recorded now, before its end event.
    fn ended(&mut self, index: usize, at: Instant) {
        self.meters[index].ended = Some(at);
        if self.meters[index].settled {
            self.record_response(index);
        }
    }

    /// Records every response that ended and is not recorded yet: the next response, or the
    /// session's end, settles them.
    fn settle_ended(&mut self) {
        for index in 0..self.meters.len() {
            if self.meters[index].ended.is_some() {
                self.record_response(index);
            }
        }
    }

    /// The session ends: every response that ended is recorded, and the one in progress is
    /// settled, so that its end records it just before its end event.
    fn settle_for_end(&mut self) {
        self.settle_ended();
        for meter in &mut self.meters {
            meter.settled = true;
        }
        let held = std::mem::take(&mut self.held);
        if !held.is_empty() {
            self.record_of_session(held, true, self.opened.0, self.latest);
        }
    }

    /// Records the total of the session billed as a whole, if one was given, once.
    fn record_at_end(&mut self) {
        if let Some((quantities, confirmed)) = self.at_end.take() {
            self.record_of_session(quantities, confirmed, self.opened.0, self.latest);
        }
    }

    /// Records the usage of the response at `index`, once.
    fn record_response(&mut self, index: usize) {
        let meter = &mut self.meters[index];
        if meter.recorded {
            return;
        }
        meter.recorded = true;
        let quantities = response_quantities(std::mem::take(&mut meter.quantities));
        let (started, ended) = (meter.started, meter.ended.unwrap_or(self.latest));
        let mut usage = RecordUsage::new(
            self.session.adapter_id(),
            self.model_id,
            self.wall_ms(started),
            self.wall_ms(ended),
            quantities,
        );
        usage.speech_session_id = Some(self.session.session_id());
        usage.response_id = Some(self.responses[index].response_id());
        self.push_record(usage);
    }

    /// Records usage of the session that names no response, from `started` to `ended`.
    fn record_of_session(
        &mut self,
        quantities: Vec<Quantity>,
        confirmed: bool,
        started: Instant,
        ended: Instant,
    ) {
        let mut usage = RecordUsage::new(
            self.session.adapter_id(),
            self.model_id,
            self.wall_ms(started),
            self.wall_ms(ended),
            quantities,
        );
        usage.speech_session_id = Some(self.session.session_id());
        usage.r#final = confirmed;
        self.push_record(usage);
    }

    fn push_record(&mut self, usage: RecordUsage) {
        let (record, event) = UsageRecord::record(usage);
        self.records.push(record);
        self.push_received(ReceivedEventKind::Usage, SessionEvent::UsageRecorded(event));
    }

    /// Records `event` for the caller; it carries nothing the provider sent.
    fn push(&mut self, event: SessionEvent) {
        self.outbox.push_back((event, None));
    }

    /// Records `event`, built from what the provider sent, for the caller: while the session is
    /// open it is received as an event of `kind`, the next in sequence, and held until
    /// [`Self::next_event`] delivers it or the session's end does. After the end nothing is
    /// received, and the event carries none.
    fn push_received(&mut self, kind: ReceivedEventKind, event: SessionEvent) {
        let sequence = i64::try_from(self.received.len())
            .unwrap_or(i64::MAX)
            .saturating_add(1);
        let received_id = ReceivedEventId::generate();
        let received = self.receive_event(received_id, kind, sequence).ok();
        self.outbox
            .push_back((event, received.map(|received| received.received_id)));
    }

    /// The session ends with `end`: every event the provider sent that is still held is
    /// delivered before the end event, which the caller records next.
    fn deliver_held(&mut self, end: SessionEnd) {
        for slot in &mut self.received {
            if let Some(moved) = slot.delivered_before_end(end) {
                *slot = moved;
            }
        }
    }

    /// `at` on the wall clock, in Unix epoch milliseconds, placed relative to the opening.
    fn wall_ms(&self, at: Instant) -> i64 {
        let (opened, wall) = self.opened;
        match at.checked_duration_since(opened) {
            Some(after) => wall.saturating_add(millis(after)),
            None => wall.saturating_sub(millis(opened.duration_since(at))),
        }
    }

    /// The ledger was given `now`.
    fn tick(&mut self, now: Instant) {
        self.latest = self.latest.max(now);
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
        self.push_received(
            ReceivedEventKind::ToolCall,
            SessionEvent::SpeechToolCallProposed(event),
        );
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
        self.push(SessionEvent::InputCommitted(InputCommitted { session_id }));
    }

    fn cancel_response(
        &mut self,
        response_id: ResponseId,
        at: Instant,
    ) -> Result<(), SessionError> {
        let index = self.index_of(response_id)?;
        let (moved, event) = self.responses[index]
            .cancel()
            .map_err(SessionError::ResponseNotInProgress)?;
        self.responses[index] = moved;
        self.ended(index, at);
        self.push(SessionEvent::ResponseCancelled(event));
        Ok(())
    }

    fn fail_response(
        &mut self,
        response_id: ResponseId,
        failure: SessionFailure,
        at: Instant,
    ) -> Result<(), SessionError> {
        let index = self.index_of(response_id)?;
        let (moved, event) = self.responses[index]
            .fail(failure)
            .map_err(SessionError::ResponseNotInProgress)?;
        self.responses[index] = moved;
        self.ended(index, at);
        self.push(SessionEvent::ResponseFailed(event));
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
