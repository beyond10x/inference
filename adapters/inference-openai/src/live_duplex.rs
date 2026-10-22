//! GPT-Live as an `inference-duplex` native speech model.
//!
//! [`OpenAiLive`] opens a [`LiveClient`] session from a contract configuration and records what
//! the provider sends through the contract's [`SessionLedger`], so it refuses what every
//! implementation refuses and yields the contract's events in order. Like the Realtime binding,
//! each session owns a single-threaded Tokio runtime and blocks on the client for each
//! operation: drive it from a blocking thread, never from inside an async runtime. Dropping a
//! session is safe anywhere.
//!
//! GPT-Live has no responses, commits, cancellation or truncation on its voice side (see
//! [`crate::live`]), so the mapping onto the response-centred contract is the binding's own:
//!
//! * One format serves both directions: PCM16 mono at 24 or 16 kHz, G.711 mu-law or A-law at
//!   8 kHz. A configuration whose output format differs from its input format is refused at
//!   open as [`Unsupported::OutputFormat`].
//! * Turn-taking is the model's own and is declared as [`TurnDetection::SemanticVad`], the only
//!   mode offered, with no tuning: GPT-Live decides when to speak from what it hears. So
//!   `auto_response` and `auto_interrupt` must be on; commit, response creation, cancellation
//!   and truncation by the caller are refused as [`Unsupported`] and nothing is sent.
//! * Input frames pass as they are. The session timeline advances with the input: two live
//!   sessions on 2026-10-20 returned one 100 ms output frame for each 100 ms input frame, so the
//!   caller keeps sending input, silence included, paced in real time, for as long as it wants
//!   output.
//! * Output audio arrives continuously, silent while the model does not speak. A response begins
//!   with the first output transcript fragment or the first voiced output frame (a sample at or
//!   above [`Segmentation::silence_peak`]) while none is in progress, and completes once it has
//!   emitted [`Segmentation::gap`] of silent output, counted in samples of the output, with no
//!   transcript fragment in between. Silent output between responses is not delivered: the
//!   contract has no place for audio outside a response. Frame positions count samples from the
//!   start of their response's audio.
//! * The API marks neither the end of a spoken answer nor an interruption. A response the
//!   listener's speech cut short therefore ends as completed, never as cancelled; the binding
//!   does not guess which it was. Whether input during output changes the output on the same
//!   response is what `conformance::native_duplex_evidence` measures on this segmentation; the
//!   hosted model gave that evidence, so [`claims`] include `native_duplex`.
//! * Input transcript fragments become one `InputTranscribed` each, in delivery order; the API
//!   has no complete per-turn transcript. Speech start and stop and commits are never reported.
//! * Tools go to a Responses backend ([`DEFAULT_BACKEND_MODEL`] unless
//!   [`OpenAiLive::with_backend`] names another) as function tools. A function call the backend
//!   completes is proposed on the response in progress, or on a new response when none is. The
//!   result goes back as a `function_call_output` item followed by `response.create`; the
//!   backend continues and the model speaks the outcome.
//! * Usage: the voice model is billed in seconds of session. [`OpenAiLiveSession::voice_seconds`]
//!   holds the latest running total and [`OpenAiLiveSession::final_usage`] what `session.closed`
//!   confirmed, `None` while nothing was reported. When the session ends (closed, ended by the
//!   server, or failed) they are one usage record of the session as a whole, naming no response,
//!   just before the end event:
//!   the billed seconds as `input_audio_ms` (the session timeline advances with the input, so
//!   they are the audio the model read), `final` only when `session.closed` confirmed them,
//!   unknown when nothing was reported. The token usage of each backend response is summed onto
//!   the response in progress, or the latest one, or, before the first response, onto the
//!   session, recorded at its end before the end event as usage naming no response; each sum
//!   as text tokens (the backend takes no audio),
//!   cached tokens inside them; a session without backend work records no `ResponseUsage`. In a
//!   response's usage record those sums are reported, its audio tokens unknown, and its input,
//!   cached or output tokens unknown once one backend response left them out; backend usage may
//!   arrive after a response's silence gap ended it, so the record waits for the next response
//!   or the session's end. In the contract's `ResponseUsage` a count the server left out is 0,
//!   its absence kept in [`OpenAiLiveSession::backend_usage`]. A backend usage is read field by
//!   field: each count it carries is summed, and one missing or in another shape is unknown. A
//!   backend usage missing its input or output tokens, a `usage` that is not an object and a
//!   backend response end whose `response` is missing or not an object each count their frame
//!   once as one the binding could not read in full; the last two count no usage.
//! * A refused handshake (HTTP 401 or 403) fails the open as rejected, any other failure to
//!   connect as a closed transport. A `session.closed` the caller did not ask for (expired, a
//!   safety filter, a remote hangup) fails the session as a provider error, `connection_lost` as
//!   a closed transport; a closed or failed connection fails it at once. `error` events do not
//!   end the session and are kept in [`OpenAiLiveSession::server_errors`]. A `session.closed`
//!   without its reason or seconds still ends the session, and a backend response end without
//!   its id still counts its usage.
//! * A message the client cannot read (not JSON, no `type`, a typed event without a field it
//!   needs) is dropped and reading goes on. It is counted on the session
//!   ([`SessionLedger::unreadable_frames`]), so is a `session.closed` lacking a field or a
//!   backend response end whose `response` or `usage` was read only in part, and none
//!   refreshes the liveness bound. A later failure of the session names how many there were and
//!   the last. Stalled turns into ProviderError only when an unreadable frame arrived after the
//!   last readable frame. A readable frame clears that state. The detail still names the total
//!   unreadable count. Every other failure keeps its kind, so a `connection_lost` without its
//!   seconds is still a closed transport. Every provider event the ledger refuses is dropped and
//!   counted in [`SessionLedger::dropped_count`]; [`SessionLedger::dropped_events`] keeps the
//!   most recent 64.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_duplex::conformance::{NativeDuplexEvidence, NoNativeDuplex};
use inference_duplex::{
    NativeSpeechModel, OpenError, ProviderFrame, Quantity, QuantityKind, ResponseId, ResponseUsage,
    SessionConfig as SpeechConfig, SessionError, SessionEvent, SessionFailure, SessionFailureKind,
    SessionLedger, SpeechCallId, SpeechCapabilities, SpeechSessionHandle, SpeechSessionId,
    TokenCounts, TuningSupport, TurnDetection, Unsupported, UsageTotal, reported,
};
use inference_tools::Json;
use tokio::runtime::Runtime;

use crate::config::{Endpoint, FunctionTool};
use crate::credential::CredentialSource;
use crate::duplex::{failure, milliseconds, without_waiting};
use crate::error::RealtimeError;
use crate::event::ServerError;
use crate::live::{
    BackendEvent, BackendUsage, Delegation, LIVE_MODEL, LiveClient, LiveClosed, LiveEvent,
    LiveFormat, LiveSessionConfig, ResponsesBackend,
};

/// The Responses backend model the binding delegates to unless told otherwise, the one the
/// delegation guide starts with.
pub const DEFAULT_BACKEND_MODEL: &str = "gpt-6-luna";

/// How output is cut into responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segmentation {
    /// The silent output, in time of the output audio, that ends a response.
    pub gap: Duration,
    /// The smallest absolute sample, on the 16-bit scale, of voiced output.
    pub silence_peak: u16,
}

impl Default for Segmentation {
    /// A gap of 800 ms, longer than a pause between sentences and shorter than a listener's
    /// question; voiced at 200, above the peaks of 2 to 45 the live sessions' silence had and far
    /// below their speech.
    fn default() -> Self {
        Self {
            gap: Duration::from_millis(800),
            silence_peak: 200,
        }
    }
}

/// The formats GPT-Live takes and gives over WebSocket, in the contract's terms.
fn formats() -> Vec<AudioFormat> {
    let mono = |encoding, rate| AudioFormat {
        encoding,
        sample_rate: std::num::NonZeroU32::new(rate).unwrap_or(std::num::NonZeroU32::MIN),
        channels: std::num::NonZeroU16::MIN,
    };
    vec![
        mono(AudioEncoding::PcmS16le, 24_000),
        mono(AudioEncoding::PcmS16le, 16_000),
        mono(AudioEncoding::PcmMulaw, 8_000),
        mono(AudioEncoding::PcmAlaw, 8_000),
    ]
}

/// What GPT-Live takes, as the duplex contract states it: its four formats (one per session,
/// both ways), its own turn-taking as `semantic_vad` without tuning, responses and interruption
/// by the model alone, no cancellation or truncation by the caller, and tools.
pub fn capabilities() -> SpeechCapabilities {
    SpeechCapabilities {
        input_formats: formats(),
        output_formats: formats(),
        turn_detection: vec![TurnDetection::SemanticVad],
        turn_tuning: TuningSupport::NONE,
        manual_commit: false,
        manual_response: false,
        manual_interrupt: false,
        client_cancel: false,
        truncation: false,
        tool_calling: true,
    }
}

/// The capabilities an adapter record of the binding claims, `native_duplex` among them: on
/// 2026-10-20 `conformance::native_duplex_evidence`, run through this binding with the default
/// [`Segmentation`] against hosted `gpt-live-1` (the ignored live test
/// `native_duplex_evidence_of_gpt_live`), recorded output before the input during output
/// (" Paris is") and, on the same response, output that took that input up (" … Lisbon is the
/// capital of Portugal …"). The evidence holds on this segmentation: a gap long enough to
/// bridge a pause in which the model yields to the listener would count a restart as one
/// response.
pub fn claims() -> Vec<Capability> {
    vec![
        Capability::OutputStreaming,
        Capability::IncrementalInput,
        Capability::ConcurrentInputOutput,
        Capability::NativeDuplex,
        Capability::ToolCalling,
        Capability::Interruptible,
    ]
}

/// Whether [`claims`] agree with an outcome of `conformance::native_duplex_evidence`.
///
/// # Errors
/// The reason, when the outcome says nothing about the model: the session did not open, failed,
/// was refused before the input during output, or never answered.
pub fn native_duplex_verdict(
    outcome: &Result<NativeDuplexEvidence, NoNativeDuplex>,
) -> Result<bool, String> {
    if let Err(
        failure @ (NoNativeDuplex::Open(_)
        | NoNativeDuplex::Failed(_)
        | NoNativeDuplex::Refused(_)
        | NoNativeDuplex::NoOutput
        | NoNativeDuplex::Silent),
    ) = outcome
    {
        return Err(format!("the model was never put to the test: {failure}"));
    }
    Ok(claims().contains(&Capability::NativeDuplex) == outcome.is_ok())
}

/// The Live format of a contract audio format, when GPT-Live takes it.
pub fn live_format(format: &AudioFormat) -> Option<LiveFormat> {
    if format.channels.get() != 1 {
        return None;
    }
    match (format.encoding, format.sample_rate.get()) {
        (AudioEncoding::PcmS16le, 24_000) => Some(LiveFormat::Pcm24k),
        (AudioEncoding::PcmS16le, 16_000) => Some(LiveFormat::Pcm16k),
        (AudioEncoding::PcmMulaw, 8_000) => Some(LiveFormat::Pcmu),
        (AudioEncoding::PcmAlaw, 8_000) => Some(LiveFormat::Pcma),
        _ => None,
    }
}

/// The largest absolute sample of `audio` in `format`, on the 16-bit scale.
fn peak(format: LiveFormat, audio: &[u8]) -> u16 {
    match format {
        LiveFormat::Pcm24k | LiveFormat::Pcm16k => audio
            .as_chunks::<2>()
            .0
            .iter()
            .map(|sample| i16::from_le_bytes(*sample).unsigned_abs())
            .max()
            .unwrap_or(0),
        LiveFormat::Pcmu => audio.iter().map(|&byte| mulaw(byte)).max().unwrap_or(0),
        LiveFormat::Pcma => audio.iter().map(|&byte| alaw(byte)).max().unwrap_or(0),
    }
}

/// The magnitude of a G.711 mu-law sample on the 16-bit scale.
fn mulaw(byte: u8) -> u16 {
    let code = !byte;
    let exponent = (code >> 4) & 0x07;
    let mantissa = u16::from(code & 0x0F);
    (((mantissa << 3) + 0x84) << exponent) - 0x84
}

/// The magnitude of a G.711 A-law sample on the 16-bit scale.
fn alaw(byte: u8) -> u16 {
    let code = byte ^ 0x55;
    let exponent = (code >> 4) & 0x07;
    let mantissa = u16::from(code & 0x0F);
    match exponent {
        0 => (mantissa << 4) + 8,
        _ => ((mantissa << 4) + 0x108) << (exponent - 1),
    }
}

/// GPT-Live as a native speech model.
pub struct OpenAiLive {
    adapter: Adapter,
    endpoint: Endpoint,
    source: Box<dyn CredentialSource>,
    credential: String,
    backend: ResponsesBackend,
    segmentation: Segmentation,
    capabilities: SpeechCapabilities,
}

impl std::fmt::Debug for OpenAiLive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiLive")
            .field("adapter", &self.adapter.adapter_id())
            .field("endpoint", &self.endpoint)
            .field("backend", &self.backend.model)
            .field("segmentation", &self.segmentation)
            .finish_non_exhaustive()
    }
}

impl OpenAiLive {
    /// A model connecting to `endpoint` (such as `Endpoint::live(LIVE_MODEL)`) with the secret
    /// `credential` names in `source`, under an adapter record of its own claiming [`claims`].
    pub fn new(
        endpoint: Endpoint,
        source: Box<dyn CredentialSource>,
        credential: impl Into<String>,
    ) -> Self {
        let adapter = Adapter::declare(DeclareAdapter {
            model_id: ModelId::generate(),
            backend: "openai-gpt-live".to_owned(),
            contract_revision: "1".to_owned(),
            execution_form: ExecutionForm::HostedService,
            locality: Locality::ExternalProvider,
            capabilities: claims(),
            max_concurrency: 1,
        })
        .map(|(adapter, _)| adapter)
        .unwrap_or_else(|_| unreachable!("a concurrency of one is positive"));
        Self {
            adapter,
            endpoint,
            source,
            credential: credential.into(),
            backend: ResponsesBackend::new(DEFAULT_BACKEND_MODEL),
            segmentation: Segmentation::default(),
            capabilities: capabilities(),
        }
    }

    /// The hosted `gpt-live-1` endpoint.
    pub fn hosted(source: Box<dyn CredentialSource>, credential: impl Into<String>) -> Self {
        Self::new(Endpoint::live(LIVE_MODEL), source, credential)
    }

    /// This model under an adapter record declared elsewhere, such as a model store's.
    pub fn with_adapter(mut self, adapter: Adapter) -> Self {
        self.adapter = adapter;
        self
    }

    /// This model with another Responses backend; a session's tools are added to its own.
    pub fn with_backend(mut self, backend: ResponsesBackend) -> Self {
        self.backend = backend;
        self
    }

    /// This model cutting output into responses by `segmentation`.
    pub fn with_segmentation(mut self, segmentation: Segmentation) -> Self {
        self.segmentation = segmentation;
        self
    }

    /// The `session.start` configuration a contract configuration maps to.
    ///
    /// # Errors
    /// [`Unsupported`] for an option the capabilities refuse, or an output format other than
    /// the input format.
    pub fn client_config(&self, config: &SpeechConfig) -> Result<LiveSessionConfig, Unsupported> {
        self.capabilities.admit(config)?;
        let format = live_format(&config.input_format)
            .ok_or(Unsupported::InputFormat(config.input_format))?;
        if config.output_format != config.input_format {
            return Err(Unsupported::OutputFormat(config.output_format));
        }
        let mut backend = self.backend.clone();
        backend
            .tools
            .extend(config.tools.iter().map(|tool| FunctionTool {
                name: tool.name().to_owned(),
                description: String::new(),
                parameters: tool.schema().clone(),
            }));
        if !backend.tools.is_empty() && backend.tool_choice.is_none() {
            backend.tool_choice = Some("auto".to_owned());
        }
        let text = |value: &str| (!value.is_empty()).then(|| value.to_owned());
        Ok(LiveSessionConfig {
            instructions: text(&config.instructions),
            voice: text(&config.voice),
            format,
            delegation: Delegation::Responses(backend),
        })
    }
}

impl NativeSpeechModel for OpenAiLive {
    type Session = OpenAiLiveSession;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn capabilities(&self) -> &SpeechCapabilities {
        &self.capabilities
    }

    fn open(&mut self, config: SpeechConfig) -> Result<OpenAiLiveSession, OpenError> {
        let client_config = self
            .client_config(&config)
            .map_err(OpenError::Unsupported)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| {
                OpenError::Failed(SessionFailure {
                    kind: SessionFailureKind::ProviderError,
                    detail: format!("no runtime for the session: {error}"),
                })
            })?;
        let client = runtime
            .block_on(LiveClient::connect(
                &self.endpoint,
                self.source.as_ref(),
                &self.credential,
                &client_config,
            ))
            .map_err(|error| OpenError::Failed(failure(&error)))?;
        let rate = u64::from(client_config.format.sample_rate());
        let gap_samples = u64::try_from(self.segmentation.gap.as_millis())
            .unwrap_or(u64::MAX)
            .saturating_mul(rate)
            / 1_000;
        let (mut ledger, _) = SessionLedger::open(
            SpeechSessionId::generate(),
            &self.adapter,
            &self.capabilities,
            config,
            Instant::now(),
        )?;
        ledger.session_usage_at_end(billed(None), false);
        Ok(OpenAiLiveSession {
            runtime: Some(runtime),
            client: Some(client),
            ledger,
            format: client_config.format,
            silence_peak: self.segmentation.silence_peak,
            gap_samples: gap_samples.max(1),
            current: None,
            silent_run: 0,
            emitted: HashMap::new(),
            usage: HashMap::new(),
            cached: HashMap::new(),
            input: HashMap::new(),
            output: HashMap::new(),
            backend_usage: Vec::new(),
            voice_seconds: None,
            final_usage: None,
            server_errors: Vec::new(),
        })
    }
}

/// How reading what already reached the binding ended.
enum Arrived {
    /// Nothing more was waiting.
    Nothing,
    /// The server's `session.closed`, unrecorded.
    Closed(LiveClosed),
    /// A read failed.
    Failed(RealtimeError),
}

/// The billed voice seconds as the session's usage: `input_audio_ms`, unknown while none were
/// reported.
fn billed(seconds: Option<f64>) -> Vec<Quantity> {
    vec![reported(
        QuantityKind::InputAudioMs,
        seconds.and_then(milliseconds),
    )]
}

/// Adds `count` to the sum kept under `id`: `None` once a count was left out, or once the sum
/// would pass `u64::MAX`, since the sum is then unknown.
fn known_sum(
    sums: &mut HashMap<Option<ResponseId>, Option<u64>>,
    id: Option<ResponseId>,
    count: Option<u64>,
) -> Option<u64> {
    *sums
        .entry(id)
        .and_modify(|sum| {
            *sum = sum
                .zip(count)
                .and_then(|(sum, count)| sum.checked_add(count));
        })
        .or_insert(count)
}

/// A backend response's usage summed onto a response, or onto the session under `None`, ready
/// for the ledger.
struct BackendSum {
    id: Option<ResponseId>,
    sum: ResponseUsage,
    quantities: Vec<Quantity>,
}

impl BackendSum {
    /// Records the sum as the response's running total, or holds it in the ledger for the
    /// session's end; what the ledger refuses is dropped.
    fn record(self, frame: &mut ProviderFrame<'_>, now: Instant) {
        match self.id {
            Some(id) => {
                let refused = frame.response_usage_reported(
                    id,
                    Some(self.sum),
                    self.quantities,
                    UsageTotal::Running,
                    now,
                );
                if let Err(refusal) = refused {
                    frame.dropped(refusal);
                }
            }
            None => frame.hold_usage(self.quantities),
        }
    }
}

/// One open GPT-Live session through the contract. It owns the runtime it blocks on; dropping
/// it shuts that runtime down without blocking.
pub struct OpenAiLiveSession {
    runtime: Option<Runtime>,
    client: Option<LiveClient>,
    ledger: SessionLedger,
    format: LiveFormat,
    silence_peak: u16,
    gap_samples: u64,
    /// The response in progress.
    current: Option<ResponseId>,
    /// Silent output samples the response in progress emitted since its last voice.
    silent_run: u64,
    /// The output samples each response emitted, the position of its next frame.
    emitted: HashMap<ResponseId, u64>,
    /// The backend usage summed onto each response, under `None` onto the session before the
    /// first response.
    usage: HashMap<Option<ResponseId>, ResponseUsage>,
    /// The cached tokens summed as `usage` is; `None` once a backend response left them out.
    cached: HashMap<Option<ResponseId>, Option<u64>>,
    /// The input tokens summed as `usage` is; `None` once a backend response left them out.
    input: HashMap<Option<ResponseId>, Option<u64>>,
    /// The output tokens summed as `usage` is; `None` once a backend response left them out.
    output: HashMap<Option<ResponseId>, Option<u64>>,
    backend_usage: Vec<BackendUsage>,
    voice_seconds: Option<f64>,
    final_usage: Option<LiveClosed>,
    server_errors: Vec<ServerError>,
}

impl Drop for OpenAiLiveSession {
    fn drop(&mut self) {
        drop(self.client.take());
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

impl std::fmt::Debug for OpenAiLiveSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiLiveSession")
            .field("session", self.ledger.session())
            .field("current", &self.current)
            .field("voice_seconds", &self.voice_seconds)
            .finish_non_exhaustive()
    }
}

impl OpenAiLiveSession {
    /// A ledger refusal the binding drops: it changed nothing, and the ledger counts it
    /// ([`SessionLedger::dropped`]).
    fn or_drop(&mut self, result: Result<(), SessionError>) {
        if let Err(refusal) = result {
            self.ledger.dropped(refusal);
        }
    }

    /// The `error` events the server sent during the session, which do not end it.
    pub fn server_errors(&self) -> &[ServerError] {
        &self.server_errors
    }

    /// The token usage of every backend response, in the order it was reported.
    pub fn backend_usage(&self) -> &[BackendUsage] {
        &self.backend_usage
    }

    /// The voice seconds billed so far: the final count once `session.closed` confirmed it,
    /// otherwise the latest running total; `None` while the server reported none.
    pub fn voice_seconds(&self) -> Option<f64> {
        self.final_usage
            .as_ref()
            .and_then(|closed| closed.seconds)
            .or(self.voice_seconds)
    }

    /// What `session.closed` confirmed; `None` until it arrives, and for good when the
    /// connection ended without it.
    pub fn final_usage(&self) -> Option<&LiveClosed> {
        self.final_usage.as_ref()
    }

    /// The provider's id of the session, while its client is held.
    pub fn session_id(&self) -> Option<&str> {
        self.client.as_ref().and_then(LiveClient::session_id)
    }

    /// Runs one client operation; a failure of the connection fails the session.
    fn send<F>(&mut self, operation: F) -> Result<(), SessionError>
    where
        F: for<'a> FnOnce(
            &'a mut LiveClient,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), RealtimeError>> + 'a>,
        >,
    {
        let (Some(client), Some(runtime)) = (self.client.as_mut(), self.runtime.as_ref()) else {
            return self
                .ledger
                .session()
                .require_open()
                .map_err(SessionError::NotOpen);
        };
        if let Err(error) = runtime.block_on(operation(client)) {
            match self.record_arrived() {
                Arrived::Closed(LiveClosed { reason, seconds }) => {
                    self.record(LiveEvent::SessionClosed { reason, seconds });
                }
                Arrived::Failed(read) => {
                    let refused = self.ledger.fail(failure(&read));
                    self.or_drop(refused);
                }
                Arrived::Nothing => {
                    let refused = self.ledger.fail(failure(&error));
                    self.or_drop(refused);
                }
            }
            self.current = None;
            return self
                .ledger
                .session()
                .require_open()
                .map_err(SessionError::NotOpen);
        }
        Ok(())
    }

    /// Records every server event that already reached the binding, without waiting for more,
    /// up to the first that ends the session: `session.closed`, or a read that fails. That one
    /// is returned unrecorded; a session the ledger no longer holds open records nothing. A
    /// message that is not a GPT-Live event is no failed read: it is recorded as
    /// [`SpeechSessionHandle::next_event`] records it, and recording goes on.
    fn record_arrived(&mut self) -> Arrived {
        while self.ledger.session().require_open().is_ok() {
            let (Some(client), Some(runtime)) = (self.client.as_mut(), self.runtime.as_ref())
            else {
                break;
            };
            match runtime.block_on(without_waiting(client.next_event())) {
                Some(Ok(LiveEvent::SessionClosed { reason, seconds })) => {
                    return Arrived::Closed(LiveClosed { reason, seconds });
                }
                Some(Ok(event)) => self.record(event),
                Some(Err(error @ RealtimeError::Protocol(_))) => self.undecodable(&error),
                None | Some(Err(RealtimeError::Stalled(_))) => break,
                Some(Err(error)) => return Arrived::Failed(error),
            }
        }
        Arrived::Nothing
    }

    /// The server sent a message that is not a GPT-Live event. It ends nothing, since the client
    /// reads on, but it is no sign of life either: it is dropped, counted on the session as a
    /// frame the binding could not read, and named by a later failure of the session.
    fn undecodable(&mut self, error: &RealtimeError) {
        self.ledger
            .unreadable_frame(error.to_string(), Instant::now());
    }

    /// The response in progress, created when none is.
    fn begin(&mut self, now: Instant) -> Option<ResponseId> {
        if let Some(id) = self.current {
            return Some(id);
        }
        let id = ResponseId::generate();
        self.ledger.response_created(id, now).ok()?;
        self.current = Some(id);
        self.silent_run = 0;
        self.emitted.insert(id, 0);
        Some(id)
    }

    fn output(&mut self, audio: Vec<u8>, now: Instant) {
        let width = self.format.sample_width();
        let whole = audio.len() - audio.len() % width;
        if whole == 0 {
            return;
        }
        let samples = (whole / width) as u64;
        let voiced = peak(self.format, &audio[..whole]) >= self.silence_peak;
        let id = match self.current {
            Some(id) => id,
            None if voiced => match self.begin(now) {
                Some(id) => id,
                None => return,
            },
            None => return,
        };
        let position = self.emitted.get(&id).copied().unwrap_or(0);
        let (Some(position), Ok(sample_count)) =
            (SampleClockPosition::new(position), u32::try_from(samples))
        else {
            return;
        };
        let frame = AudioFrame {
            format: *self.ledger.session().output_format(),
            position,
            sample_count,
            data: audio[..whole].to_vec(),
        };
        if let Err(refusal) = self.ledger.output_audio(id, frame, now) {
            self.ledger.dropped(refusal);
            return;
        }
        *self.emitted.entry(id).or_insert(0) += samples;
        if voiced {
            self.silent_run = 0;
        } else {
            self.silent_run += samples;
            if self.silent_run >= self.gap_samples {
                let refused = self.ledger.response_completed(id, now);
                self.or_drop(refused);
                self.current = None;
                self.silent_run = 0;
            }
        }
    }

    /// A backend response ended. Its frame is recorded through a frame scope of its own
    /// ([`SessionLedger::provider_frame`]) that hears the provider when `heard`: what of its
    /// usage was read is summed ([`Self::backend_usage_summed`]), and a `response` or `usage`
    /// read only in part or not at all counts the frame once as one the binding could not read
    /// in full, after what it recorded; the liveness bound then stands where it stood before the
    /// frame ([`read_in_part`](inference_duplex::ProviderFrame::read_in_part)).
    fn backend_ended(
        &mut self,
        usage: Option<BackendUsage>,
        unreadable_usage: Option<String>,
        heard: bool,
        now: Instant,
    ) {
        let summed = usage.map(|usage| self.backend_usage_summed(usage));
        let mut frame = self.ledger.provider_frame();
        if heard {
            frame.heard(now);
        }
        if let Some(summed) = summed {
            summed.record(&mut frame, now);
        }
        if let Some(what) = unreadable_usage {
            frame.read_in_part(what, now);
        }
    }

    /// Sums a backend response's usage onto the response in progress or the latest one; before
    /// the first response, onto the session, held in the ledger for its end
    /// ([`BackendSum::record`]). A sum that would pass `u64::MAX` is unknown in the usage record
    /// ([`known_sum`]); the contract's `ResponseUsage`, which has no unknown, stops at
    /// `u64::MAX`.
    fn backend_usage_summed(&mut self, usage: BackendUsage) -> BackendSum {
        self.backend_usage.push(usage);
        let id = self.current.or_else(|| {
            self.ledger
                .responses()
                .last()
                .map(|response| response.response_id())
        });
        let sum = self.usage.entry(id).or_default();
        *sum = ResponseUsage {
            input: TokenCounts {
                text_tokens: sum
                    .input
                    .text_tokens
                    .saturating_add(usage.input_tokens.unwrap_or(0)),
                audio_tokens: 0,
                cached_tokens: sum
                    .input
                    .cached_tokens
                    .saturating_add(usage.cached_tokens.unwrap_or(0)),
            },
            output: TokenCounts {
                text_tokens: sum
                    .output
                    .text_tokens
                    .saturating_add(usage.output_tokens.unwrap_or(0)),
                audio_tokens: 0,
                cached_tokens: 0,
            },
        };
        let sum = *sum;
        let cached = known_sum(&mut self.cached, id, usage.cached_tokens);
        let input = known_sum(&mut self.input, id, usage.input_tokens);
        let output = known_sum(&mut self.output, id, usage.output_tokens);
        let quantities = vec![
            reported(QuantityKind::InputTextTokens, input),
            reported(QuantityKind::CachedInputTokens, cached),
            reported(QuantityKind::OutputTextTokens, output),
        ];
        BackendSum {
            id,
            sum,
            quantities,
        }
    }

    /// `session.closed` confirmed the billed voice seconds: they replace the running total the
    /// ledger records just before the session's end event. Its frame is recorded through a frame
    /// scope of its own ([`SessionLedger::provider_frame`]) that hears the provider when `heard`.
    /// One without its seconds confirms nothing, and one without its reason or seconds is
    /// counted as a frame the binding could not read in full, which leaves the liveness bound
    /// where it stood before the frame
    /// ([`read_in_part`](inference_duplex::ProviderFrame::read_in_part)).
    fn confirm(&mut self, closed: LiveClosed, heard: bool, now: Instant) {
        let missing = match (&closed.reason, closed.seconds) {
            (Some(_), Some(_)) => None,
            (None, Some(_)) => Some("the string field `reason`"),
            (Some(_), None) => Some("the number field `usage.seconds`"),
            (None, None) => Some("the fields `reason` and `usage.seconds`"),
        };
        let mut frame = self.ledger.provider_frame();
        if heard {
            frame.heard(now);
        }
        match missing {
            Some(missing) => frame.read_in_part(format!("`session.closed` lacks {missing}"), now),
            None => drop(frame),
        }
        if let Some(seconds) = closed.seconds {
            self.ledger
                .session_usage_at_end(billed(Some(seconds)), true);
        }
        self.final_usage = Some(closed);
    }

    /// What the server sent between the caller's close and `session.closed`: only its usage is
    /// recorded, the voice seconds as the running total and each backend response's tokens, and
    /// a message that is not a GPT-Live event is counted; nothing of it is delivered.
    fn closing(&mut self, event: Result<LiveEvent, RealtimeError>) {
        let now = Instant::now();
        match event {
            Ok(LiveEvent::UsageUpdated { seconds, .. }) => {
                self.voice_seconds = Some(seconds);
                self.ledger
                    .session_usage_at_end(billed(Some(seconds)), false);
            }
            Ok(LiveEvent::Backend {
                event:
                    BackendEvent::Completed {
                        usage,
                        unreadable_usage,
                        ..
                    }
                    | BackendEvent::Ended {
                        usage,
                        unreadable_usage,
                        ..
                    },
                ..
            }) => self.backend_ended(usage, unreadable_usage, false, now),
            Ok(_) => {}
            Err(error) => self.undecodable(&error),
        }
    }

    /// Records one server event through the ledger; what the ledger refuses is dropped. A frame
    /// read whole hears the provider; one that may be read only in part, a backend response's
    /// end or `session.closed`, is recorded through a frame scope of its own
    /// ([`SessionLedger::provider_frame`]), one frame at a time, and read only in part it leaves
    /// the liveness bound where it stood before it
    /// ([`read_in_part`](inference_duplex::ProviderFrame::read_in_part)).
    fn record(&mut self, event: LiveEvent) {
        let now = Instant::now();
        let framed = matches!(
            event,
            LiveEvent::Backend {
                event: BackendEvent::Completed { .. } | BackendEvent::Ended { .. },
                ..
            } | LiveEvent::SessionClosed { .. }
        );
        if !framed {
            self.ledger.heard(now);
        }
        match event {
            LiveEvent::OutputAudio { audio } => self.output(audio, now),
            LiveEvent::OutputTranscript { delta, .. } => {
                if let Some(id) = self.begin(now) {
                    self.silent_run = 0;
                    let refused = self.ledger.output_transcript(id, delta, now);
                    self.or_drop(refused);
                }
            }
            LiveEvent::InputTranscript { delta, .. } => {
                let refused = self.ledger.input_transcribed(delta, now);
                self.or_drop(refused);
            }
            LiveEvent::Backend { event, .. } => match event {
                BackendEvent::FunctionCall {
                    call_id,
                    name,
                    arguments,
                    ..
                } => {
                    if let Some(id) = self.begin(now) {
                        let arguments =
                            serde_json::from_str(&arguments).unwrap_or(Json::String(arguments));
                        let refused = self.ledger.tool_call(
                            SpeechCallId::new(call_id),
                            id,
                            &name,
                            arguments,
                            now,
                        );
                        self.or_drop(refused);
                    }
                }
                BackendEvent::Completed {
                    usage,
                    unreadable_usage,
                    ..
                }
                | BackendEvent::Ended {
                    usage,
                    unreadable_usage,
                    ..
                } => self.backend_ended(usage, unreadable_usage, true, now),
                BackendEvent::Created { .. } | BackendEvent::Other { .. } => {}
            },
            LiveEvent::UsageUpdated { seconds, .. } => {
                self.voice_seconds = Some(seconds);
                self.ledger
                    .session_usage_at_end(billed(Some(seconds)), false);
            }
            LiveEvent::SessionClosed { reason, seconds } => {
                let kind = if reason.as_deref() == Some("connection_lost") {
                    SessionFailureKind::TransportClosed
                } else {
                    SessionFailureKind::ProviderError
                };
                let detail = match &reason {
                    Some(reason) => format!("the provider ended the session: {reason}"),
                    None => "the provider ended the session without a reason".to_owned(),
                };
                self.confirm(LiveClosed { reason, seconds }, true, now);
                let refused = self.ledger.fail(SessionFailure { kind, detail });
                self.or_drop(refused);
                self.current = None;
                self.client = None;
            }
            LiveEvent::Error(error) => self.server_errors.push(error),
            LiveEvent::SessionStarted { .. }
            | LiveEvent::SessionUpdated { .. }
            | LiveEvent::DelegationCreated { .. }
            | LiveEvent::Acknowledged { .. }
            | LiveEvent::Info { .. }
            | LiveEvent::Unknown { .. } => {}
        }
    }
}

impl SpeechSessionHandle for OpenAiLiveSession {
    fn ledger(&self) -> &SessionLedger {
        &self.ledger
    }

    fn append_audio(&mut self, frame: AudioFrame) -> Result<(), SessionError> {
        let audio = frame.data.clone();
        self.ledger.accept_input(frame)?;
        self.send(|client| Box::pin(async move { client.append_audio(&audio).await }))
    }

    fn commit_input(&mut self) -> Result<(), SessionError> {
        self.ledger.commit_input(Instant::now())
    }

    fn create_response(&mut self) -> Result<(), SessionError> {
        self.ledger.request_response(Instant::now())
    }

    fn cancel_response(&mut self, response_id: ResponseId) -> Result<(), SessionError> {
        self.ledger.cancel(response_id)
    }

    fn truncate_output(
        &mut self,
        response_id: ResponseId,
        heard: SampleClockPosition,
    ) -> Result<(), SessionError> {
        self.ledger.truncate(response_id, heard)
    }

    fn submit_tool_result(
        &mut self,
        call_id: &SpeechCallId,
        result: Json,
    ) -> Result<(), SessionError> {
        let output = result.to_string();
        self.ledger.answer(call_id, result)?;
        let call_id = call_id.as_str().to_owned();
        self.send(|client| {
            Box::pin(async move { client.send_function_output(&call_id, &output).await })
        })?;
        self.send(|client| Box::pin(client.create_response()))
    }

    /// Closes the session gracefully, confirming its final usage. The server events that already
    /// reached the binding are recorded first, so their usage counts and they are delivered after
    /// the close, before `SpeechSessionClosed`; a `session.closed` among them confirms the final
    /// usage, and from it or a read that fails nothing is recorded, so an open session closes
    /// rather than fails. A message that is not a GPT-Live event is no failed read: it is
    /// counted, the client reads on, and what follows it is recorded too. Otherwise the close asks
    /// the server for `session.closed` and awaits it, for [`crate::live::CLOSE_TIMEOUT`] at most
    /// in all when the endpoint's read timeout is shorter; the seconds it carries are recorded
    /// just before `SpeechSessionClosed`. Of what the server sends meanwhile only its usage is
    /// recorded and nothing is delivered; without `session.closed` the session still closes, with
    /// the usage seen so far, unconfirmed.
    fn close(&mut self) -> Result<(), SessionError> {
        if self.ledger.session().require_open().is_ok() {
            match self.record_arrived() {
                Arrived::Closed(closed) => {
                    self.confirm(closed, false, Instant::now());
                    self.client = None;
                }
                Arrived::Failed(_) => self.client = None,
                Arrived::Nothing => {}
            }
            let mut seen = Vec::new();
            let closed = match (self.client.take(), self.runtime.as_ref()) {
                (Some(client), Some(runtime)) => runtime
                    .block_on(client.close_seeing(|event| seen.push(event)))
                    .ok(),
                _ => None,
            };
            for event in seen {
                self.closing(event);
            }
            if let Some(closed) = closed {
                self.confirm(closed, false, Instant::now());
            }
        }
        self.ledger.close()?;
        self.current = None;
        Ok(())
    }

    fn next_event(&mut self, wait: Duration) -> Option<SessionEvent> {
        let deadline = Instant::now() + wait;
        loop {
            if let Some(event) = self.ledger.next_event() {
                return Some(event);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || self.ledger.session().require_open().is_err() {
                self.ledger.check_liveness(Instant::now());
                return self.ledger.next_event();
            }
            let (Some(client), Some(runtime)) = (self.client.as_mut(), self.runtime.as_ref())
            else {
                return None;
            };
            let read =
                runtime.block_on(async { tokio::time::timeout(left, client.next_event()).await });
            match read {
                Ok(Ok(event)) => self.record(event),
                Ok(Err(error @ RealtimeError::Protocol(_))) => {
                    self.undecodable(&error);
                    self.ledger.check_liveness(Instant::now());
                }
                Ok(Err(RealtimeError::Stalled(_))) | Err(_) => {
                    self.ledger.check_liveness(Instant::now());
                }
                Ok(Err(error)) => {
                    let refused = self.ledger.fail(failure(&error));
                    self.or_drop(refused);
                    self.current = None;
                    self.client = None;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sum that a count would carry past `u64::MAX` is unknown from then on, never a number
    /// stopped at `u64::MAX`; a count left out makes it unknown too.
    #[test]
    fn a_sum_past_u64_max_is_unknown_from_then_on() {
        let mut sums = HashMap::new();
        assert_eq!(
            known_sum(&mut sums, None, Some(u64::MAX - 1)),
            Some(u64::MAX - 1)
        );
        assert_eq!(known_sum(&mut sums, None, Some(2)), None);
        assert_eq!(known_sum(&mut sums, None, Some(0)), None);
        let mut sums = HashMap::new();
        assert_eq!(known_sum(&mut sums, None, Some(3)), Some(3));
        assert_eq!(known_sum(&mut sums, None, None), None);
        assert_eq!(known_sum(&mut sums, None, Some(4)), None);
    }

    /// A backend sum onto a response whose usage is already recorded is refused, and the refusal
    /// is dropped and counted. The binding cannot reach this today: the ledger refuses only an
    /// unknown or recorded response (`SessionLedger::response_usage_reported`), the sum goes to
    /// the newest response, which an open session records only once a newer one is created
    /// (`SessionLedger::response_created`), and GPT-Live takes no cancellation from the caller
    /// ([`capabilities`]). This test is what holds the branch.
    #[test]
    fn a_sum_onto_a_recorded_response_is_dropped_and_counted() {
        let pcm24 = AudioFormat {
            encoding: AudioEncoding::PcmS16le,
            sample_rate: std::num::NonZeroU32::new(24_000).unwrap(),
            channels: std::num::NonZeroU16::MIN,
        };
        let config = SpeechConfig {
            voice: "marin".to_owned(),
            instructions: "answer briefly".to_owned(),
            input_format: pcm24,
            output_format: pcm24,
            turn_detection: TurnDetection::SemanticVad,
            turn_tuning: inference_duplex::TurnTuning::default(),
            auto_response: true,
            auto_interrupt: true,
            tools: Vec::new(),
            idle_limit: Duration::from_secs(5),
        };
        let (adapter, _) = Adapter::declare(DeclareAdapter {
            model_id: ModelId::generate(),
            backend: "openai-gpt-live".to_owned(),
            contract_revision: "1".to_owned(),
            execution_form: ExecutionForm::HostedService,
            locality: Locality::ExternalProvider,
            capabilities: claims(),
            max_concurrency: 1,
        })
        .unwrap();
        let now = Instant::now();
        let (mut ledger, _) = SessionLedger::open(
            SpeechSessionId::generate(),
            &adapter,
            &capabilities(),
            config,
            now,
        )
        .unwrap();
        let (first, second) = (ResponseId::generate(), ResponseId::generate());
        ledger.response_created(first, now).unwrap();
        ledger.response_completed(first, now).unwrap();
        ledger.response_created(second, now).unwrap();
        assert!(
            ledger
                .usage_records()
                .iter()
                .any(|record| record.response_id() == Some(first)),
            "creating the next response did not record the first one's usage"
        );
        let before = ledger.dropped_count();
        let sum = BackendSum {
            id: Some(first),
            sum: ResponseUsage::default(),
            quantities: Vec::new(),
        };
        sum.record(&mut ledger.provider_frame(), now);
        assert_eq!(ledger.dropped_count(), before + 1);
        assert!(
            matches!(
                ledger.dropped_events().last(),
                Some(SessionError::ResponseNotInProgress(_))
            ),
            "{:?}",
            ledger.dropped_events()
        );
    }
}
