//! The Realtime client as an `inference-duplex` native speech model.
//!
//! [`OpenAiRealtime`] opens a [`RealtimeClient`] session from a contract configuration and
//! records every server event through the contract's [`SessionLedger`], so it refuses what every
//! implementation refuses and yields the contract's events in order.
//!
//! The contract is synchronous: each session owns a single-threaded Tokio runtime and blocks on
//! the client for each operation. A session must therefore not be driven from inside an async
//! runtime; an async caller runs it on a blocking thread. Dropping a session is safe anywhere:
//! its runtime is shut down without blocking.
//!
//! How the provider maps onto the contract:
//!
//! * Formats pass as they are: PCM16 at 24 kHz (`audio/pcm`), G.711 mu-law (`audio/pcmu`) and
//!   A-law (`audio/pcma`) at 8 kHz, all mono. Output deltas become frames whose position counts
//!   samples from the start of their response's audio.
//! * Turn detection off is `turn_detection: null`; server and semantic detection carry
//!   `create_response` from `auto_response` and `interrupt_response` from `auto_interrupt`.
//!   Silence and prefix padding tune `server_vad` only, sensitivity tunes `semantic_vad` only (as
//!   its eagerness); a tuning the chosen mode cannot take is refused at open. With
//!   `auto_response` on, the binding sends `response.create` after each caller commit, in every
//!   turn detection mode: the API creates a response by itself only for a turn its own detection
//!   ends, never for a client `input_audio_buffer.commit`.
//! * Each `response.created` gets a new [`ResponseId`]; provider response ids map to it and are
//!   never reused. `response.done` ends the response as completed, cancelled (the caller's
//!   cancellation already recorded is not recorded again), or failed for `failed` and
//!   `incomplete`. Its `usage` is the response's final total. The usage record of the response
//!   takes each count from its own field (text, audio and cached input tokens, text and audio
//!   output tokens) and keeps a count the server left out unknown; a `response.done` without
//!   usage settles the response with every count unknown. A response the caller cancels is
//!   recorded when its `response.done` arrives, with the usage of what it generated, or, failing
//!   that, at the next response or the session's end. The contract's `ResponseUsage`, which has no
//!   unknown, still counts an absent detail as 0, or in text where the total says so.
//! * The server's `input_audio_buffer.committed` that acknowledges a caller's commit is not a
//!   second commit; one the server's own turn detection makes is. A caller's commit the server
//!   refuses (an `error` whose code begins `input_audio_buffer_commit`) is owed no
//!   acknowledgement.
//! * The usage of `conversation.item.input_audio_transcription.completed` is a usage record: in
//!   tokens, the input tokens as `input_audio_tokens` and the output tokens as
//!   `output_text_tokens`, a count the server left out unknown; in seconds, as `input_audio_ms`.
//!   A usage in tokens with both counts is also the contract's transcription usage; one in
//!   seconds, or missing a count, has no place in it.
//! * A handshake the server refuses with HTTP 401 or 403 fails the open as rejected; any other
//!   failure to connect as a closed transport.
//! * Truncation converts the heard sample to `audio_end_ms` of the response's audio item.
//! * Tool results go back as `function_call_output` items; the caller creates the response that
//!   uses them.
//! * A closed connection, a transport or protocol failure fails the session at once; the
//!   liveness bound fails it when the server is silent while a response is awaited. `error`
//!   events do not end the session; they are kept in [`OpenAiRealtimeSession::server_errors`].

use std::collections::HashMap;
use std::time::{Duration, Instant};

use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_duplex::conformance::{NativeDuplexEvidence, NoNativeDuplex};
use inference_duplex::{
    NativeSpeechModel, OpenError, Quantity, QuantityKind, ResponseId, ResponseUsage,
    SessionConfig as SpeechConfig, SessionError, SessionEvent, SessionFailure, SessionFailureKind,
    SessionLedger, SpeechCallId, SpeechCapabilities, SpeechSessionHandle, SpeechSessionId,
    TokenCounts, TranscriptionUsage, TuningOption, TuningSupport,
    TurnDetection as SpeechTurnDetection, TurnSensitivity, Unsupported, UsageTotal, reported,
};
use inference_tools::Json;
use tokio::runtime::Runtime;

use crate::client::RealtimeClient;
use crate::config::{
    Eagerness, Endpoint, FunctionTool, OutputModality, SessionConfig, TurnDetection, WireFormat,
};
use crate::credential::CredentialSource;
use crate::error::RealtimeError;
use crate::event::{
    ResponseStatus, ServerError, ServerEvent, TranscriptionUsage as ClientTranscriptionUsage, Usage,
};

/// The input transcription model the binding asks for unless told otherwise.
pub const DEFAULT_TRANSCRIPTION_MODEL: &str = "gpt-4o-mini-transcribe";

/// The slowest output speed the API takes, `audio.output.speed`.
const MIN_SPEED: f32 = 0.25;

/// The fastest output speed the API takes, `audio.output.speed`.
const MAX_SPEED: f32 = 1.5;

/// The formats the Realtime API takes and gives, in the contract's terms.
fn formats() -> Vec<AudioFormat> {
    let mono = |encoding, rate| AudioFormat {
        encoding,
        sample_rate: std::num::NonZeroU32::new(rate).unwrap_or(std::num::NonZeroU32::MIN),
        channels: std::num::NonZeroU16::MIN,
    };
    vec![
        mono(AudioEncoding::PcmS16le, 24_000),
        mono(AudioEncoding::PcmMulaw, 8_000),
        mono(AudioEncoding::PcmAlaw, 8_000),
    ]
}

/// What the Realtime API takes, as the duplex contract states it: PCM16 at 24 kHz and G.711
/// mu-law and A-law at 8 kHz both ways, every turn detection mode with its tuning, manual
/// commit, responses and interruption, cancellation and truncation by the caller, and tools.
pub fn capabilities() -> SpeechCapabilities {
    SpeechCapabilities {
        input_formats: formats(),
        output_formats: formats(),
        turn_detection: SpeechTurnDetection::ALL.to_vec(),
        turn_tuning: TuningSupport::ALL,
        manual_commit: true,
        manual_response: true,
        manual_interrupt: true,
        client_cancel: true,
        truncation: true,
        tool_calling: true,
    }
}

/// The capabilities an adapter record of the binding claims. `native_duplex` is not among them:
/// the hosted model has not given the evidence `conformance::native_duplex_evidence` asks for.
pub fn claims() -> Vec<Capability> {
    vec![
        Capability::OutputStreaming,
        Capability::IncrementalInput,
        Capability::ConcurrentInputOutput,
        Capability::ToolCalling,
        Capability::Interruptible,
    ]
}

/// Whether [`claims`] agree with an outcome of `conformance::native_duplex_evidence`: the
/// binding claims `native_duplex` exactly when the model gave the evidence.
///
/// # Errors
/// The reason, when the outcome says nothing about the model: the session did not open, failed,
/// was refused before the input during output, or never answered. Comparing such an outcome
/// with the claims would pass vacuously.
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

/// The Realtime wire format of a contract audio format, when the API takes it.
pub fn wire_format(format: &AudioFormat) -> Option<WireFormat> {
    if format.channels.get() != 1 {
        return None;
    }
    match (format.encoding, format.sample_rate.get()) {
        (AudioEncoding::PcmS16le, 24_000) => Some(WireFormat::Pcm16),
        (AudioEncoding::PcmMulaw, 8_000) => Some(WireFormat::Pcmu),
        (AudioEncoding::PcmAlaw, 8_000) => Some(WireFormat::Pcma),
        _ => None,
    }
}

/// The Realtime API as a native speech model.
pub struct OpenAiRealtime {
    adapter: Adapter,
    endpoint: Endpoint,
    source: Box<dyn CredentialSource>,
    credential: String,
    transcription: Option<String>,
    speed: Option<f32>,
    capabilities: SpeechCapabilities,
}

impl std::fmt::Debug for OpenAiRealtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiRealtime")
            .field("adapter", &self.adapter.adapter_id())
            .field("endpoint", &self.endpoint)
            .field("transcription", &self.transcription)
            .field("speed", &self.speed)
            .finish_non_exhaustive()
    }
}

impl OpenAiRealtime {
    /// A model connecting to `endpoint` with the secret `credential` names in `source`, under an
    /// adapter record of its own claiming [`claims`].
    pub fn new(
        endpoint: Endpoint,
        source: Box<dyn CredentialSource>,
        credential: impl Into<String>,
    ) -> Self {
        let adapter = Adapter::declare(DeclareAdapter {
            model_id: ModelId::generate(),
            backend: "openai-realtime".to_owned(),
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
            transcription: Some(DEFAULT_TRANSCRIPTION_MODEL.to_owned()),
            speed: None,
            capabilities: capabilities(),
        }
    }

    /// This model under an adapter record declared elsewhere, such as a model store's.
    pub fn with_adapter(mut self, adapter: Adapter) -> Self {
        self.adapter = adapter;
        self
    }

    /// This model with another input transcription model, or none; without one the session
    /// yields no input transcripts.
    pub fn with_input_transcription(mut self, model: Option<String>) -> Self {
        self.transcription = model;
        self
    }

    /// This model speaking at `speed` times its own pace (`audio.output.speed`), held to the
    /// API's range of 0.25 to 1.5; `None`, or a speed that is not a number, leaves the server's
    /// default of 1.0.
    pub fn with_output_speed(mut self, speed: Option<f32>) -> Self {
        self.speed = speed
            .filter(|speed| !speed.is_nan())
            .map(|speed| speed.clamp(MIN_SPEED, MAX_SPEED));
        self
    }

    /// The client session configuration a contract configuration maps to.
    ///
    /// # Errors
    /// [`Unsupported`] for a format the API does not take, or a tuning the chosen turn detection
    /// mode cannot take.
    pub fn client_config(&self, config: &SpeechConfig) -> Result<SessionConfig, Unsupported> {
        let input_format = wire_format(&config.input_format)
            .ok_or(Unsupported::InputFormat(config.input_format))?;
        let output_format = wire_format(&config.output_format)
            .ok_or(Unsupported::OutputFormat(config.output_format))?;
        let tuning = config.turn_tuning;
        let millis = |duration: Duration| u32::try_from(duration.as_millis()).unwrap_or(u32::MAX);
        let turn_detection = match config.turn_detection {
            SpeechTurnDetection::Disabled => TurnDetection::Off,
            SpeechTurnDetection::ServerVad => {
                if tuning.sensitivity.is_some() {
                    return Err(Unsupported::TurnTuning(TuningOption::Sensitivity));
                }
                TurnDetection::Server {
                    threshold: None,
                    prefix_padding_ms: tuning.prefix_padding.map(millis),
                    silence_duration_ms: tuning.silence.map(millis),
                    create_response: config.auto_response,
                    interrupt_response: config.auto_interrupt,
                }
            }
            SpeechTurnDetection::SemanticVad => {
                if tuning.silence.is_some() {
                    return Err(Unsupported::TurnTuning(TuningOption::Silence));
                }
                if tuning.prefix_padding.is_some() {
                    return Err(Unsupported::TurnTuning(TuningOption::PrefixPadding));
                }
                TurnDetection::Semantic {
                    eagerness: match tuning.sensitivity {
                        None => Eagerness::Auto,
                        Some(TurnSensitivity::Low) => Eagerness::Low,
                        Some(TurnSensitivity::Medium) => Eagerness::Medium,
                        Some(TurnSensitivity::High) => Eagerness::High,
                    },
                    create_response: config.auto_response,
                    interrupt_response: config.auto_interrupt,
                }
            }
        };
        let text = |value: &str| (!value.is_empty()).then(|| value.to_owned());
        Ok(SessionConfig {
            instructions: text(&config.instructions),
            voice: text(&config.voice),
            output_modality: OutputModality::Audio,
            input_format,
            output_format,
            turn_detection,
            input_transcription: self.transcription.clone(),
            speed: self.speed,
            tools: config
                .tools
                .iter()
                .map(|tool| FunctionTool {
                    name: tool.name().to_owned(),
                    description: String::new(),
                    parameters: tool.schema().clone(),
                })
                .collect(),
        })
    }
}

impl NativeSpeechModel for OpenAiRealtime {
    type Session = OpenAiRealtimeSession;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn capabilities(&self) -> &SpeechCapabilities {
        &self.capabilities
    }

    fn open(&mut self, config: SpeechConfig) -> Result<OpenAiRealtimeSession, OpenError> {
        self.capabilities
            .admit(&config)
            .map_err(OpenError::Unsupported)?;
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
            .block_on(RealtimeClient::connect(
                &self.endpoint,
                self.source.as_ref(),
                &self.credential,
                &client_config,
            ))
            .map_err(|error| OpenError::Failed(failure(&error)))?;
        let (ledger, _) = SessionLedger::open(
            SpeechSessionId::generate(),
            &self.adapter,
            &self.capabilities,
            config,
            Instant::now(),
        )?;
        Ok(OpenAiRealtimeSession {
            runtime: Some(runtime),
            client: Some(client),
            ledger,
            responses: HashMap::new(),
            audio_items: HashMap::new(),
            emitted: HashMap::new(),
            caller_commits: 0,
            server_errors: Vec::new(),
        })
    }
}

/// What `read` gives when it completes without waiting: it is polled once, and once more after
/// one turn of the runtime's drivers, so a message that already reached the socket is read.
/// `None` when it would still wait; `read` is then dropped, so it must be cancel safe, as reading
/// the next event is.
pub(crate) async fn without_waiting<F: std::future::Future>(read: F) -> Option<F::Output> {
    let mut read = std::pin::pin!(read);
    for turn in 0..2 {
        let polled =
            std::future::poll_fn(|context| std::task::Poll::Ready(read.as_mut().poll(context)))
                .await;
        if let std::task::Poll::Ready(output) = polled {
            return Some(output);
        }
        if turn == 0 {
            tokio::task::yield_now().await;
        }
    }
    None
}

/// The contract's failure of a client error.
pub(crate) fn failure(error: &RealtimeError) -> SessionFailure {
    let kind = match error {
        RealtimeError::Connect(refused) if refused.is_refusal() => SessionFailureKind::Rejected,
        RealtimeError::Closed { .. } | RealtimeError::Transport(_) | RealtimeError::Connect(_) => {
            SessionFailureKind::TransportClosed
        }
        RealtimeError::Stalled(_) => SessionFailureKind::Stalled,
        RealtimeError::Credential(_)
        | RealtimeError::InvalidEndpoint(_)
        | RealtimeError::Server(_)
        | RealtimeError::Audio(_) => SessionFailureKind::Rejected,
        RealtimeError::Protocol(_) => SessionFailureKind::ProviderError,
    };
    SessionFailure {
        kind,
        detail: error.to_string(),
    }
}

/// One open Realtime session through the contract.
///
/// It owns the runtime it blocks on, held until it is dropped. Dropping it shuts that runtime
/// down without blocking, so a session may be dropped anywhere, inside async code too.
pub struct OpenAiRealtimeSession {
    runtime: Option<Runtime>,
    client: Option<RealtimeClient>,
    ledger: SessionLedger,
    /// Provider response id to the contract's.
    responses: HashMap<String, ResponseId>,
    /// The audio item and content index of each response, for truncation.
    audio_items: HashMap<ResponseId, (String, u32)>,
    /// The output samples each response emitted, the position of its next frame.
    emitted: HashMap<ResponseId, u64>,
    /// Caller commits the server has not acknowledged yet.
    caller_commits: usize,
    server_errors: Vec<ServerError>,
}

impl Drop for OpenAiRealtimeSession {
    fn drop(&mut self) {
        drop(self.client.take());
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

impl std::fmt::Debug for OpenAiRealtimeSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiRealtimeSession")
            .field("session", self.ledger.session())
            .field("responses", &self.responses)
            .finish_non_exhaustive()
    }
}

impl OpenAiRealtimeSession {
    /// The `error` events the server sent during the session, which do not end it.
    pub fn server_errors(&self) -> &[ServerError] {
        &self.server_errors
    }

    /// The provider's id of a response.
    pub fn provider_response_id(&self, response_id: ResponseId) -> Option<&str> {
        self.responses
            .iter()
            .find(|(_, id)| **id == response_id)
            .map(|(provider, _)| provider.as_str())
    }

    /// Runs one client operation; a failure of the connection fails the session.
    fn send<F>(&mut self, operation: F) -> Result<(), SessionError>
    where
        F: for<'a> FnOnce(
            &'a mut RealtimeClient,
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
            let error = self.record_arrived().unwrap_or(error);
            let _ = self.ledger.fail(failure(&error));
            return self
                .ledger
                .session()
                .require_open()
                .map_err(SessionError::NotOpen);
        }
        Ok(())
    }

    /// Records every server event that already reached the binding, without waiting for more,
    /// up to the first read that fails, whose error is returned; a session the ledger no longer
    /// holds open records nothing. A message that is not a Realtime event is no failed read:
    /// it is recorded as [`SpeechSessionHandle::next_event`] records it, and recording goes on.
    fn record_arrived(&mut self) -> Option<RealtimeError> {
        while self.ledger.session().require_open().is_ok() {
            let (Some(client), Some(runtime)) = (self.client.as_mut(), self.runtime.as_ref())
            else {
                return None;
            };
            match runtime.block_on(without_waiting(client.next_event())) {
                Some(Ok(event)) => self.record(event),
                Some(Err(RealtimeError::Protocol(_))) => self.undecodable(),
                None | Some(Err(RealtimeError::Stalled(_))) => return None,
                Some(Err(error)) => return Some(error),
            }
        }
        None
    }

    /// The server sent a message that is not a Realtime event. It ends nothing, since the client
    /// reads on: it is server traffic, and is dropped.
    fn undecodable(&mut self) {
        self.ledger.heard(Instant::now());
    }

    /// The contract's id of a provider response, when the session created one under it.
    fn response(&self, provider: &str) -> Option<ResponseId> {
        self.responses.get(provider).copied()
    }

    /// Records one server event through the ledger; what the ledger refuses is dropped.
    fn record(&mut self, event: ServerEvent) {
        let now = Instant::now();
        self.ledger.heard(now);
        let config = self.ledger.session().config().clone();
        let input_rate = u64::from(config.input_format.sample_rate.get());
        let at = |ms: u64| SampleClockPosition::new(ms.saturating_mul(input_rate) / 1_000);
        match event {
            ServerEvent::ResponseCreated { response_id } => {
                if self.responses.contains_key(&response_id) {
                    return;
                }
                let id = ResponseId::generate();
                if self.ledger.response_created(id, now).is_ok() {
                    self.responses.insert(response_id, id);
                    self.emitted.insert(id, 0);
                }
            }
            ServerEvent::AudioDelta {
                response_id,
                item_id,
                content_index,
                audio,
            } => {
                let Some(id) = self.response(&response_id) else {
                    return;
                };
                let format = config.output_format;
                let width = usize::try_from(format.encoding.width()).unwrap_or(1);
                let whole = audio.len() - audio.len() % width;
                let sample_count = u32::try_from(whole / width).unwrap_or(u32::MAX);
                let position = self.emitted.get(&id).copied().unwrap_or(0);
                let Some(position) = SampleClockPosition::new(position) else {
                    return;
                };
                let frame = AudioFrame {
                    format,
                    position,
                    sample_count,
                    data: audio[..whole].to_vec(),
                };
                if self.ledger.output_audio(id, frame, now).is_ok() {
                    *self.emitted.entry(id).or_insert(0) += u64::from(sample_count);
                    self.audio_items.insert(id, (item_id, content_index));
                }
            }
            ServerEvent::OutputTranscriptDelta {
                response_id, delta, ..
            }
            | ServerEvent::TextDelta {
                response_id, delta, ..
            } => {
                if let Some(id) = self.response(&response_id) {
                    let _ = self.ledger.output_transcript(id, delta, now);
                }
            }
            ServerEvent::InputTranscript {
                transcript, usage, ..
            } => {
                let _ = self.ledger.input_transcribed(transcript, now);
                let (contract, quantities) = match usage {
                    Some(ClientTranscriptionUsage::Tokens {
                        input_tokens,
                        output_tokens,
                        ..
                    }) => (
                        input_tokens
                            .zip(output_tokens)
                            .map(|(input, output)| TranscriptionUsage {
                                input_tokens: input,
                                output_tokens: output,
                            }),
                        vec![
                            reported(QuantityKind::InputAudioTokens, input_tokens),
                            reported(QuantityKind::OutputTextTokens, output_tokens),
                        ],
                    ),
                    Some(ClientTranscriptionUsage::Duration { seconds }) => (
                        None,
                        vec![reported(QuantityKind::InputAudioMs, milliseconds(seconds))],
                    ),
                    None => return,
                };
                let _ = self
                    .ledger
                    .transcription_usage_reported(contract, quantities, now);
            }
            ServerEvent::SpeechStarted { audio_start_ms, .. } => {
                if let Some(position) = at(audio_start_ms) {
                    let _ = self.ledger.speech_started(position, now);
                }
            }
            ServerEvent::SpeechStopped { audio_end_ms, .. } => {
                if let Some(position) = at(audio_end_ms) {
                    let _ = self.ledger.speech_stopped(position, now);
                }
            }
            ServerEvent::AudioCommitted { .. } => {
                if self.caller_commits > 0 {
                    self.caller_commits -= 1;
                } else {
                    let _ = self.ledger.input_committed(now);
                }
            }
            ServerEvent::FunctionCall {
                response_id,
                call_id,
                name,
                arguments,
                ..
            } => {
                if let Some(id) = self.response(&response_id) {
                    let arguments =
                        serde_json::from_str(&arguments).unwrap_or(Json::String(arguments));
                    let _ = self.ledger.tool_call(
                        SpeechCallId::new(call_id),
                        id,
                        &name,
                        arguments,
                        now,
                    );
                }
            }
            ServerEvent::ResponseDone {
                response_id,
                status,
                usage,
            } => {
                let Some(id) = self.response(&response_id) else {
                    return;
                };
                let _ = self.ledger.response_usage_reported(
                    id,
                    usage.as_ref().map(response_usage),
                    usage.as_ref().map(quantities).unwrap_or_default(),
                    UsageTotal::Final,
                    now,
                );
                let _ = match status {
                    ResponseStatus::Completed => self.ledger.response_completed(id, now),
                    ResponseStatus::Cancelled => self.ledger.response_cancelled(id, now),
                    ResponseStatus::InProgress => Ok(()),
                    ResponseStatus::Failed
                    | ResponseStatus::Incomplete
                    | ResponseStatus::Other(_) => self.ledger.response_failed(
                        id,
                        SessionFailure {
                            kind: SessionFailureKind::ProviderError,
                            detail: format!("the response ended {status:?}"),
                        },
                        now,
                    ),
                };
            }
            ServerEvent::Error(error) => {
                let refused_commit = error
                    .code
                    .as_deref()
                    .is_some_and(|code| code.starts_with("input_audio_buffer_commit"));
                if refused_commit && self.caller_commits > 0 {
                    self.caller_commits -= 1;
                }
                self.server_errors.push(error);
            }
            ServerEvent::SessionCreated { .. }
            | ServerEvent::SessionUpdated { .. }
            | ServerEvent::AudioDone { .. }
            | ServerEvent::OutputTranscriptDone { .. }
            | ServerEvent::InputTranscriptDelta { .. }
            | ServerEvent::ItemTruncated { .. }
            | ServerEvent::Unknown { .. } => {}
        }
    }
}

/// The quantities of a response total, each from its own field, unknown where the server left
/// it out.
fn quantities(usage: &Usage) -> Vec<Quantity> {
    vec![
        reported(QuantityKind::InputTextTokens, usage.input_text_tokens),
        reported(QuantityKind::InputAudioTokens, usage.input_audio_tokens),
        reported(QuantityKind::CachedInputTokens, usage.cached_input_tokens),
        reported(QuantityKind::OutputTextTokens, usage.output_text_tokens),
        reported(QuantityKind::OutputAudioTokens, usage.output_audio_tokens),
    ]
}

/// `seconds` in whole milliseconds, rounded, saturating at `u64::MAX`; `None` for a count that
/// is no duration (negative or not finite).
pub(crate) fn milliseconds(seconds: f64) -> Option<u64> {
    let millis = (seconds * 1_000.0).round();
    (millis.is_finite() && millis >= 0.0).then_some(millis as u64)
}

/// The contract's usage of a response total, which has no unknown: a detail the server left out
/// is counted as 0, the text tokens as the total's rest where the total says so.
fn response_usage(usage: &Usage) -> ResponseUsage {
    let input_audio = usage.input_audio_tokens.unwrap_or(0);
    let output_audio = usage.output_audio_tokens.unwrap_or(0);
    ResponseUsage {
        input: TokenCounts {
            text_tokens: usage
                .input_text_tokens
                .unwrap_or(usage.input_tokens.saturating_sub(input_audio)),
            audio_tokens: input_audio,
            cached_tokens: usage.cached_input_tokens.unwrap_or(0),
        },
        output: TokenCounts {
            text_tokens: usage
                .output_text_tokens
                .unwrap_or(usage.output_tokens.saturating_sub(output_audio)),
            audio_tokens: output_audio,
            cached_tokens: 0,
        },
    }
}

impl SpeechSessionHandle for OpenAiRealtimeSession {
    fn ledger(&self) -> &SessionLedger {
        &self.ledger
    }

    fn append_audio(&mut self, frame: AudioFrame) -> Result<(), SessionError> {
        let audio = frame.data.clone();
        self.ledger.accept_input(frame)?;
        self.send(|client| Box::pin(async move { client.append_encoded(&audio).await }))
    }

    fn commit_input(&mut self) -> Result<(), SessionError> {
        self.ledger.commit_input(Instant::now())?;
        self.caller_commits += 1;
        self.send(|client| Box::pin(client.commit_audio()))?;
        if self.ledger.session().config().auto_response {
            self.send(|client| Box::pin(client.create_response()))?;
        }
        Ok(())
    }

    fn create_response(&mut self) -> Result<(), SessionError> {
        self.ledger.request_response(Instant::now())?;
        self.send(|client| Box::pin(client.create_response()))
    }

    fn cancel_response(&mut self, response_id: ResponseId) -> Result<(), SessionError> {
        self.ledger.cancel(response_id)?;
        let provider = self.provider_response_id(response_id).map(str::to_owned);
        self.send(|client| {
            Box::pin(async move { client.cancel_response(provider.as_deref()).await })
        })
    }

    fn truncate_output(
        &mut self,
        response_id: ResponseId,
        heard: SampleClockPosition,
    ) -> Result<(), SessionError> {
        self.ledger.truncate(response_id, heard)?;
        let Some((item_id, content_index)) = self.audio_items.get(&response_id).cloned() else {
            return Ok(());
        };
        let rate = u64::from(self.ledger.session().output_format().sample_rate.get());
        let audio_end_ms =
            u32::try_from(heard.get().saturating_mul(1_000) / rate).unwrap_or(u32::MAX);
        self.send(|client| {
            Box::pin(async move { client.truncate(&item_id, content_index, audio_end_ms).await })
        })
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
        })
    }

    /// Closes the session and the connection. The server events that already reached the
    /// binding are recorded first, so their usage counts and they are delivered after the close,
    /// before `SpeechSessionClosed`; from the first read that fails among them nothing is
    /// recorded, so an open session closes rather than fails. A message that is not a Realtime
    /// event is no failed read: the client reads on, so what follows it is recorded too. Nothing
    /// the server sends after the close is delivered.
    fn close(&mut self) -> Result<(), SessionError> {
        if self.ledger.session().require_open().is_ok() && self.record_arrived().is_some() {
            self.client = None;
        }
        self.ledger.close()?;
        if let (Some(client), Some(runtime)) = (self.client.take(), self.runtime.as_ref()) {
            let _ = runtime.block_on(client.close());
        }
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
                Ok(Err(RealtimeError::Protocol(_))) => self.undecodable(),
                Ok(Err(RealtimeError::Stalled(_))) | Err(_) => {
                    self.ledger.check_liveness(Instant::now());
                }
                Ok(Err(error)) => {
                    let _ = self.ledger.fail(failure(&error));
                    self.client = None;
                }
            }
        }
    }
}
