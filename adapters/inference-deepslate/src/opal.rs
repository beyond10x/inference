//! An Opal session as an `inference-duplex` native speech model.
//!
//! [`DeepslateOpal`] opens a session over the WebSocket of [`crate::protocol`] and records what
//! the server sends through the contract's [`SessionLedger`], so it refuses what every
//! implementation refuses and yields the contract's events in order.
//!
//! # Opening
//!
//! [`DeepslateOpal::new`] refuses an endpoint it cannot use before anything connects
//! ([`Endpoint::url`]). [`NativeSpeechModel::open`] admits the session configuration against
//! [`capabilities`], resolves the key through the injected [`CredentialSource`] and refuses an
//! empty or blank one before connecting, connects with `Authorization: Bearer <key>`, sends
//! `InitializeSessionRequest` as the first message and, when the session offers tools,
//! `UpdateToolDefinitionsRequest`, then waits for `SessionReady`; no audio goes out before it,
//! as the documentation asks. The whole opening is bounded by [`Endpoint::open_timeout`]. A
//! refused handshake (HTTP 400, 401, 403 or 404) or a `SessionErrorNotification` of category
//! `ERROR_CONFIGURATION` before `SessionReady` fails the open as rejected; a close before it as a
//! closed transport, with [`DeepslateError::ClosedBeforeReady`]. Every way an open fails leaves
//! the reason in [`DeepslateOpal::open_error`].
//!
//! The `InitializeSessionRequest` carries the session's input and output formats as 16-bit PCM
//! audio lines, the voice activity detection of [`Vad::DOCUMENTED`] with the session's silence
//! as `stop_duration` and its prefix padding as `backbuffer_duration`, the instructions as the
//! system prompt and [`OpalConfig::temperature`]. A non-empty voice is a voice Deepslate hosts,
//! sent as the hosted provider's `voice_ref` in [`OpalConfig::tts_mode`]; an empty voice sends no
//! speech configuration, and the server then sends text and no audio. Each tool goes as a
//! `ToolDefinition`: its name, its description when it has one, and its schema as parameters
//! when the schema is a JSON object.
//!
//! # The connection
//!
//! Each session runs a runtime of one worker thread, on which one task owns the socket: it reads
//! every frame, writes what the session sends, sends `Ping` every [`Endpoint::ping_interval`],
//! and hands what it read to the session through a queue of [`INBOUND_DEPTH`] messages. A `Pong`
//! keeps the connection alive and is not handed over; a message the caller need not see
//! (`SessionReady`, `InferenceComplete`, and the members passed over) takes a slot only when one
//! is free at once. While it waits for a slot for a message the caller must see, it reads
//! nothing more, but still writes what the session sends, pings and obeys a stop. The session's
//! writes go through a queue of [`OUTBOUND_QUEUE`] messages, each send waiting at most
//! [`Endpoint::read_timeout`]. A frame larger than [`crate::MAX_MESSAGE_SIZE`] fails the session
//! as a transport error, and a connection that sends nothing, pongs included, for
//! [`Endpoint::read_timeout`] while the task reads fails it as stalled. Closing or dropping the
//! session sends a close frame and waits at most [`CLOSE_BOUND`] in all for it. Drive a session
//! from a blocking thread, never from inside an async runtime.
//!
//! # The mapping onto the contract
//!
//! * Opal's turn-taking is its own voice activity detection: [`TurnDetection::ServerVad`] is the
//!   only mode, tuned by silence and prefix padding; there is no documented mapping of a
//!   sensitivity onto its thresholds. `auto_response` and `auto_interrupt` must be on: audio goes
//!   out as `UserInput` with the `IMMEDIATE` trigger, which the documentation recommends for
//!   streaming audio and which interrupts the inference running. Commit, response creation,
//!   cancellation and truncation by the caller are refused as unsupported; the protocol has no
//!   message for the first three, and its playback report is not a truncation.
//! * Each `UserInput` carries as its packet id the input sample its audio starts at, the input
//!   sent before it; the definition leaves the numbering to the client. A `VadStateEvent` names
//!   the packet that triggered it, so its position is that sample, at most the input sent.
//!   A transition into `SPEECH` from `SILENCE` or `SPEECH_STARTING` is `SpeechStarted`; one
//!   into `SILENCE` from `SPEECH` or `SPEECH_ENDING` is `SpeechStopped` and the end of the user
//!   turn, `InputCommitted`. `UserTranscriptionResult` is `InputTranscribed`, whenever it comes.
//! * A response is one assistant turn, by its `turn_id`. It begins at `ResponseBegin`, or at
//!   the first audio chunk or tool call of a turn no response holds; a turn that begins while
//!   another's response is in progress completes that one first. Audio chunks without a turn
//!   belong to the turn of the latest `ResponseBegin`, as the documentation says. The audio of a
//!   response counts samples from its start; a chunk ending in half a sample keeps that byte for
//!   the next one. `ModelTextFragment` is its `OutputTranscriptDelta`; fragments of a turn no
//!   response holds yet are held for it, and those of a response that ended are dropped.
//!   `ResponseEnd` completes it. `InferenceComplete` is passed over, since speech may follow it.
//!   The text kept, of a response or held, is at most [`TEXT_LIMIT`] bytes; the bytes past it
//!   are counted ([`DeepslateOpalSession::text_dropped`]).
//! * `PlaybackClearBuffer`, which the server sends when the user starts speaking, cancels the
//!   response in progress (`ResponseCancelled`); its turn's audio that still arrives is dropped
//!   and counted ([`DeepslateOpalSession::late_audio_dropped`]).
//! * A `ToolCallRequest` for a tool the session offers is proposed on its turn's response
//!   (`SpeechToolCallProposed`); [`SpeechSessionHandle::submit_tool_result`] sends
//!   `ToolCallResponse` with the call's id, the result as its text (a JSON string as it is, any
//!   other value as its JSON text). An answer the caller gives while the call's response is
//!   still in progress makes what follows on that turn a new response, the one that answers the
//!   result; the call's response completes when it begins, or at the turn's `ResponseEnd` when
//!   nothing follows. A call of a tool the session does not offer is answered at once with
//!   [`tool_unavailable`], since the model waits for an answer to every request, and recorded
//!   in the ledger's dropped events as `ToolNotOffered`; the session goes on.
//! * `SessionErrorNotification` fails the session as a provider error
//!   ([`DeepslateError::Server`], with its category, message and trace id). A close by the
//!   server or a failed read fails it as a closed transport. [`DeepslateOpalSession::failure_cause`]
//!   keeps the reason.
//! * A text frame, or a binary frame that is not a `ClientBoundMessage`, is counted on the
//!   session ([`SessionLedger::unreadable_frames`]) and read past.
//! * Usage: no message carries any, so each response's usage record carries every count unknown.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use inference_duplex::conformance::{NativeDuplexEvidence, NoNativeDuplex};
use inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_duplex::inference_core::model::{
    Adapter, AdapterDescriptor, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_duplex::inference_tools::Json;
use inference_duplex::{
    NativeSpeechModel, OpenError, ResponseId, SessionConfig, SessionError, SessionEvent,
    SessionFailure, SessionFailureKind, SessionLedger, SpeechCallId, SpeechCapabilities,
    SpeechSessionHandle, SpeechSessionId, TuningSupport, TurnDetection,
};
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::{self, Bytes, Message};

use crate::config::{Endpoint, OpalConfig};
use crate::connection::{self, CLOSE_BOUND, Socket};
use crate::credential::{CredentialSource, DEEPSLATE_KEY_NAME, EnvironmentSource};
use crate::error::{ConfigError, DeepslateError};
use crate::protocol::{
    AudioLine, ClientBound, ErrorCategory, HostedTts, InitializeSession, ServiceBound,
    ToolDefinition, TriggerMode, Vad, VadState,
};

/// The backend an adapter record of the binding names.
pub const BACKEND: &str = "deepslate-opal";

/// The model the claims record names: the platform's default Opal model, unless the endpoint
/// pins one ([`Endpoint::with_model`]).
pub const MODEL: &str = "platform-default";

/// The sample rates the binding takes each way, mono 16-bit PCM: 16 kHz, which every example of
/// the documentation uses, 48 kHz, which its reconfiguration example uses, and 8 kHz, the lower
/// end of the range its error example names. The documentation lists no rates.
pub const SAMPLE_RATES: [u32; 3] = [8_000, 16_000, 48_000];

/// How many messages the connection task holds for the session.
pub const INBOUND_DEPTH: usize = 32;

/// How many messages the session holds for the connection task to write.
pub const OUTBOUND_QUEUE: usize = 64;

/// The most bytes of text kept for one response, and held for turns no response holds yet.
pub const TEXT_LIMIT: usize = 64 * 1024;

/// The most characters of a server error's message kept.
const MESSAGE_LIMIT: usize = 256;

fn pcm16(rate: u32) -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(rate).unwrap_or(NonZeroU32::MIN),
        channels: NonZeroU16::MIN,
    }
}

/// What Opal takes, as the duplex contract states it: mono 16-bit PCM at [`SAMPLE_RATES`] each
/// way; its own voice activity detection as `server_vad`, tuned by silence and prefix padding;
/// responses and interruption by the model alone; no cancellation or truncation by the caller;
/// tool calls.
#[must_use]
pub fn capabilities() -> SpeechCapabilities {
    let formats: Vec<AudioFormat> = SAMPLE_RATES.iter().map(|&rate| pcm16(rate)).collect();
    SpeechCapabilities {
        input_formats: formats.clone(),
        output_formats: formats,
        turn_detection: vec![TurnDetection::ServerVad],
        turn_tuning: TuningSupport {
            silence: true,
            sensitivity: false,
            prefix_padding: true,
        },
        manual_commit: false,
        manual_response: false,
        manual_interrupt: false,
        client_cancel: false,
        truncation: false,
        tool_calling: true,
    }
}

/// The capabilities an adapter record of the binding claims, the same as
/// `records/claims-opal.json`.
///
/// Not `native_duplex`: `conformance::native_duplex_evidence` over the fixture server, which
/// cuts a reply on speech over it as the `IMMEDIATE` trigger does, ends in
/// [`NoNativeDuplex::Restarted`], the cut reply cancelled and the answer in a new one. Not
/// `interruptible`: the caller cannot cancel a reply, and nothing states a latency for the
/// model's own barge-in.
#[must_use]
pub fn claims() -> Vec<Capability> {
    vec![
        Capability::OutputStreaming,
        Capability::IncrementalInput,
        Capability::ConcurrentInputOutput,
        Capability::ToolCalling,
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

/// Opal as a native speech model.
pub struct DeepslateOpal {
    adapter: Adapter,
    url: String,
    endpoint: Endpoint,
    config: OpalConfig,
    source: Arc<dyn CredentialSource>,
    credential: String,
    capabilities: SpeechCapabilities,
    open_error: Option<DeepslateError>,
}

impl std::fmt::Debug for DeepslateOpal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeepslateOpal")
            .field("adapter", &self.adapter.adapter_id())
            .field("url", &self.url)
            .field("credential", &self.credential)
            .field("open_error", &self.open_error)
            .finish_non_exhaustive()
    }
}

impl DeepslateOpal {
    /// Opal at `endpoint`, asked for `config`, its key the secret `credential` of `source`, under
    /// an adapter record of its own claiming [`claims`].
    ///
    /// # Errors
    /// The [`ConfigError`] of [`Endpoint::url`]; nothing connects.
    pub fn new(
        endpoint: Endpoint,
        config: OpalConfig,
        source: Arc<dyn CredentialSource>,
        credential: impl Into<String>,
    ) -> Result<Self, ConfigError> {
        let url = endpoint.url()?;
        let adapter = Adapter::declare(DeclareAdapter {
            model_id: ModelId::generate(),
            backend: BACKEND.to_owned(),
            contract_revision: "1".to_owned(),
            execution_form: ExecutionForm::HostedService,
            locality: Locality::ExternalProvider,
            capabilities: claims(),
            max_concurrency: 1,
            descriptor: AdapterDescriptor::default(),
        })
        .map(|(adapter, _)| adapter)
        .map_err(|_| ConfigError::MaxConcurrency)?;
        Ok(Self {
            adapter,
            url,
            endpoint,
            config,
            source,
            credential: credential.into(),
            capabilities: capabilities(),
            open_error: None,
        })
    }

    /// Opal at the endpoint the environment names ([`Endpoint::from_env`]), asked for `config`,
    /// its key read from `DEEPSLATE_API_KEY`.
    ///
    /// # Errors
    /// As [`Endpoint::from_env`] and [`DeepslateOpal::new`].
    pub fn from_env(config: OpalConfig) -> Result<Self, ConfigError> {
        Self::new(
            Endpoint::from_env()?,
            config,
            Arc::new(EnvironmentSource::deepslate()),
            DEEPSLATE_KEY_NAME,
        )
    }

    /// This model under an adapter record declared elsewhere, such as a model store's.
    #[must_use]
    pub fn with_adapter(mut self, adapter: Adapter) -> Self {
        self.adapter = adapter;
        self
    }

    /// Why the latest open failed; `None` after an open that succeeded or before any.
    #[must_use]
    pub fn open_error(&self) -> Option<&DeepslateError> {
        self.open_error.as_ref()
    }

    /// `error` as the open's failure, kept in [`Self::open_error`].
    fn failed(&mut self, error: DeepslateError) -> OpenError {
        let kind = match &error {
            DeepslateError::Connect(connect) if connect.is_refusal() => {
                SessionFailureKind::Rejected
            }
            DeepslateError::Credential(_)
            | DeepslateError::Config(_)
            | DeepslateError::Server {
                category: ErrorCategory::Configuration,
                ..
            } => SessionFailureKind::Rejected,
            DeepslateError::Stalled(_) => SessionFailureKind::Stalled,
            DeepslateError::Server { .. } => SessionFailureKind::ProviderError,
            _ => SessionFailureKind::TransportClosed,
        };
        let failure = SessionFailure {
            kind,
            detail: error.to_string(),
        };
        self.open_error = Some(error);
        OpenError::Failed(failure)
    }
}

/// The `InitializeSessionRequest` of a session opened with `config`.
fn initialize(opal: &OpalConfig, config: &SessionConfig) -> InitializeSession {
    let line = |format: AudioFormat| {
        AudioLine::pcm16(format.sample_rate.get(), u32::from(format.channels.get()))
    };
    let tuning = config.turn_tuning;
    InitializeSession {
        input: line(config.input_format),
        output: line(config.output_format),
        vad: Vad {
            stop_duration: tuning.silence.unwrap_or(Vad::DOCUMENTED.stop_duration),
            backbuffer_duration: tuning
                .prefix_padding
                .unwrap_or(Vad::DOCUMENTED.backbuffer_duration),
            ..Vad::DOCUMENTED
        },
        system_prompt: config.instructions.clone(),
        temperature: opal.temperature.unwrap_or(0.0),
        tts: (!config.voice.is_empty()).then(|| HostedTts {
            voice_id: config.voice.clone(),
            mode: opal.tts_mode,
        }),
        supports_playback_reporting: false,
    }
}

/// The `UpdateToolDefinitionsRequest` of a session opened with `config`, when it offers tools.
fn tool_definitions(config: &SessionConfig) -> Option<ServiceBound> {
    if config.tools.is_empty() {
        return None;
    }
    Some(ServiceBound::UpdateTools(
        config
            .tools
            .iter()
            .map(|tool| ToolDefinition {
                name: tool.name().to_owned(),
                description: (!tool.description().is_empty())
                    .then(|| tool.description().to_owned()),
                parameters: tool.schema().is_object().then(|| tool.schema().clone()),
            })
            .collect(),
    ))
}

impl NativeSpeechModel for DeepslateOpal {
    type Session = DeepslateOpalSession;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn capabilities(&self) -> &SpeechCapabilities {
        &self.capabilities
    }

    fn open(&mut self, config: SessionConfig) -> Result<DeepslateOpalSession, OpenError> {
        self.open_error = None;
        let (mut ledger, _) = SessionLedger::open(
            SpeechSessionId::generate(),
            &self.adapter,
            &self.capabilities,
            config.clone(),
            Instant::now(),
        )?;
        let mut opening =
            vec![ServiceBound::Initialize(initialize(&self.config, &config)).encode()];
        opening.extend(tool_definitions(&config).map(|tools| tools.encode()));
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("deepslate-opal")
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => return Err(self.failed(DeepslateError::Runtime(error.to_string()))),
        };
        let link = Link {
            url: &self.url,
            open_timeout: self.endpoint.open_timeout,
            source: self.source.as_ref(),
            credential: &self.credential,
        };
        let (socket, early) = match runtime.block_on(connect(&link, opening)) {
            Ok(opened) => opened,
            Err(error) => {
                runtime.shutdown_background();
                return Err(self.failed(error));
            }
        };
        let (commands, command_rx) = mpsc::channel(OUTBOUND_QUEUE);
        let (inbound_tx, inbound) = mpsc::channel(INBOUND_DEPTH);
        let (stop, stop_rx) = oneshot::channel();
        let pump = runtime.spawn(pump(
            socket,
            command_rx,
            stop_rx,
            inbound_tx,
            Timers::new(&self.endpoint),
        ));
        ledger.heard(Instant::now());
        let mut session = DeepslateOpalSession {
            runtime: Some(runtime),
            commands: Some(commands),
            inbound,
            stop: Some(stop),
            pump: Some(pump),
            ledger,
            output_format: config.output_format,
            write_timeout: self.endpoint.read_timeout,
            turns: HashMap::new(),
            last_turn: None,
            call_turns: HashMap::new(),
            answered: HashSet::new(),
            emitted: 0,
            carry: Vec::new(),
            input_samples: 0,
            held: BTreeMap::new(),
            held_bytes: 0,
            kept: HashMap::new(),
            text_dropped: 0,
            late_audio: 0,
            failure: None,
            closing: false,
        };
        let now = Instant::now();
        for inbound in early {
            session.record(inbound, now);
        }
        Ok(session)
    }
}

/// Where and how a session connects.
struct Link<'a> {
    url: &'a str,
    open_timeout: Duration,
    source: &'a dyn CredentialSource,
    credential: &'a str,
}

/// Connects, sends `opening` and waits for `SessionReady`, all within the open timeout. A
/// connection that fails after the handshake is closed before this returns.
async fn connect(
    link: &Link<'_>,
    opening: Vec<Vec<u8>>,
) -> Result<(Socket, Vec<Inbound>), DeepslateError> {
    let bound = link.open_timeout;
    let deadline = tokio::time::Instant::now() + bound;
    let handshake = connection::open(link.url, link.source, link.credential);
    let mut socket = tokio::time::timeout_at(deadline, handshake)
        .await
        .unwrap_or(Err(DeepslateError::Stalled(bound)))?;
    let waited = tokio::time::timeout_at(deadline, await_ready(&mut socket, opening))
        .await
        .unwrap_or(Err(DeepslateError::Stalled(bound)));
    match waited {
        Ok(early) => Ok((socket, early)),
        Err(error) => {
            connection::close(socket).await;
            Err(error)
        }
    }
}

/// Sends `opening`, then reads until `SessionReady`. A `SessionErrorNotification` or a close
/// ends the wait. What else arrives is kept, at most [`INBOUND_DEPTH`] messages.
async fn await_ready(
    socket: &mut Socket,
    opening: Vec<Vec<u8>>,
) -> Result<Vec<Inbound>, DeepslateError> {
    for message in opening {
        socket
            .send(Message::binary(message))
            .await
            .map_err(|error| DeepslateError::Transport(error.to_string()))?;
    }
    let mut early = Vec::new();
    loop {
        let inbound = match next_frame(socket).await {
            Frame::Binary(bytes) => match ClientBound::decode(&bytes) {
                Ok(ClientBound::SessionReady) => return Ok(early),
                Ok(ClientBound::Error {
                    category,
                    message,
                    trace_id,
                }) => return Err(server_error(category, &message, trace_id)),
                Ok(message) if !must_see(&message) => continue,
                Ok(message) => Inbound::Message(message),
                Err(error) => Inbound::Unreadable(unreadable(&error)),
            },
            Frame::Text => Inbound::Unreadable("a text frame".to_owned()),
            Frame::Control => continue,
            Frame::Closed { code, reason } => {
                return Err(DeepslateError::ClosedBeforeReady { code, reason });
            }
            Frame::Failed(error) => return Err(error),
        };
        if early.len() < INBOUND_DEPTH {
            early.push(inbound);
        }
    }
}

/// The result the binding answers a call of the tool `name` with when the session does not
/// offer it: a short JSON error, as its text.
#[must_use]
pub fn tool_unavailable(name: &str) -> String {
    serde_json::json!({"error": format!("the tool `{name}` is not available in this session")})
        .to_string()
}

fn server_error(
    category: ErrorCategory,
    message: &str,
    trace_id: Option<String>,
) -> DeepslateError {
    DeepslateError::Server {
        category,
        message: message.chars().take(MESSAGE_LIMIT).collect(),
        trace_id: trace_id.map(|trace| trace.chars().take(MESSAGE_LIMIT).collect()),
    }
}

fn unreadable(error: &crate::protocol::ProtocolError) -> String {
    format!("a frame that is not a ClientBoundMessage: {error}")
}

/// One frame the connection read.
enum Frame {
    Binary(Bytes),
    /// A text frame: the protocol sends none.
    Text,
    /// A WebSocket ping or pong: a sign of life, nothing to hand over.
    Control,
    Closed {
        code: Option<u16>,
        reason: String,
    },
    Failed(DeepslateError),
}

async fn next_frame(socket: &mut Socket) -> Frame {
    let message = match socket.next().await {
        None => {
            return Frame::Closed {
                code: None,
                reason: String::new(),
            };
        }
        Some(Err(error)) => return Frame::Failed(read_error(error)),
        Some(Ok(message)) => message,
    };
    match message {
        Message::Binary(data) => Frame::Binary(data),
        Message::Text(_) => Frame::Text,
        Message::Close(frame) => Frame::Closed {
            code: frame.as_ref().map(|frame| u16::from(frame.code)),
            reason: frame
                .map(|frame| frame.reason.as_str().chars().take(MESSAGE_LIMIT).collect())
                .unwrap_or_default(),
        },
        Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => Frame::Control,
    }
}

fn read_error(error: tungstenite::Error) -> DeepslateError {
    match error {
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => {
            DeepslateError::Closed {
                code: None,
                reason: String::new(),
            }
        }
        other => DeepslateError::Transport(other.to_string()),
    }
}

/// Whether the caller must see `message`: everything but the messages the session passes over.
fn must_see(message: &ClientBound) -> bool {
    !matches!(
        message,
        ClientBound::Other(_)
            | ClientBound::Empty
            | ClientBound::SessionReady
            | ClientBound::InferenceComplete { .. }
            | ClientBound::Pong
    )
}

/// What the connection task hands the session.
enum Inbound {
    Message(ClientBound),
    /// A frame the binding could not read, and what it was.
    Unreadable(String),
    /// The connection ended, and why.
    Ended(DeepslateError),
}

/// The connection task's clocks: when it last pinged and last read anything.
struct Timers {
    ping: Duration,
    read_timeout: Duration,
    pinged: tokio::time::Instant,
    read: tokio::time::Instant,
}

impl Timers {
    fn new(endpoint: &Endpoint) -> Self {
        let now = tokio::time::Instant::now();
        Self {
            ping: endpoint.ping_interval,
            read_timeout: endpoint.read_timeout,
            pinged: now,
            read: now,
        }
    }

    /// When something is next due: a `Ping`, or, while reading, the stall.
    fn next(&self, reading: bool) -> tokio::time::Instant {
        let next = self.pinged + self.ping;
        if reading {
            next.min(self.read + self.read_timeout)
        } else {
            next
        }
    }

    /// Sends what is due; a stall, when `reading` and nothing was read within the read timeout.
    async fn fire(&mut self, socket: &mut Socket, reading: bool) -> Result<(), DeepslateError> {
        let now = tokio::time::Instant::now();
        if reading && now >= self.read + self.read_timeout {
            return Err(DeepslateError::Stalled(self.read_timeout));
        }
        if now >= self.pinged + self.ping {
            write(socket, ServiceBound::Ping.encode(), self.read_timeout).await?;
            self.pinged = now;
        }
        Ok(())
    }
}

/// What woke the connection task while it read.
enum Woken {
    Stop,
    Command(Option<Vec<u8>>),
    Due,
    Frame(Frame),
}

/// Whether the connection task goes on after handing something over.
enum Handed {
    On,
    /// The session stopped the task or dropped its queues.
    Stopped,
    /// A write failed; the task ends.
    Failed,
}

/// Hands `item` to the session, waiting for a slot of the inbound queue; meanwhile it still
/// writes what the session sends, pings, and obeys a stop.
async fn hand_over(
    socket: &mut Socket,
    commands: &mut mpsc::Receiver<Vec<u8>>,
    stop: &mut oneshot::Receiver<()>,
    inbound: &mpsc::Sender<Inbound>,
    timers: &mut Timers,
    item: Inbound,
) -> Handed {
    loop {
        let written = tokio::select! {
            biased;
            _ = &mut *stop => return Handed::Stopped,
            permit = inbound.reserve() => {
                return match permit {
                    Ok(permit) => {
                        permit.send(item);
                        Handed::On
                    }
                    Err(_) => Handed::Stopped,
                };
            }
            command = commands.recv() => match command {
                Some(bytes) => write(socket, bytes, timers.read_timeout).await,
                None => return Handed::Stopped,
            },
            () = tokio::time::sleep_until(timers.next(false)) => timers.fire(socket, false).await,
        };
        if let Err(error) = written {
            let _ = inbound.try_send(Inbound::Ended(error));
            return Handed::Failed;
        }
    }
}

/// The connection task: reads, writes what the session sends, pings, until the session stops it
/// or drops its queue, or the connection ends. A `Pong` is read and taken as a sign of life of
/// the connection only; a message the caller need not see takes a slot only when one is free at
/// once. While no slot is free for a message the caller must see, the task reads nothing more,
/// and its timers no longer count a stall.
async fn pump(
    mut socket: Socket,
    mut commands: mpsc::Receiver<Vec<u8>>,
    mut stop: oneshot::Receiver<()>,
    inbound: mpsc::Sender<Inbound>,
    mut timers: Timers,
) {
    loop {
        let woken = tokio::select! {
            biased;
            _ = &mut stop => Woken::Stop,
            command = commands.recv() => Woken::Command(command),
            () = tokio::time::sleep_until(timers.next(true)) => Woken::Due,
            frame = next_frame(&mut socket) => Woken::Frame(frame),
        };
        let (item, ends) = match woken {
            Woken::Stop | Woken::Command(None) => break,
            Woken::Command(Some(bytes)) => {
                if let Err(error) = write(&mut socket, bytes, timers.read_timeout).await {
                    let _ = inbound.try_send(Inbound::Ended(error));
                    return;
                }
                continue;
            }
            Woken::Due => match timers.fire(&mut socket, true).await {
                Ok(()) => continue,
                Err(error) => (Inbound::Ended(error), true),
            },
            Woken::Frame(frame) => {
                timers.read = tokio::time::Instant::now();
                match frame {
                    Frame::Binary(bytes) => match ClientBound::decode(&bytes) {
                        Ok(ClientBound::Pong) => continue,
                        Ok(message) if !must_see(&message) => {
                            let _ = inbound.try_send(Inbound::Message(message));
                            continue;
                        }
                        Ok(message) => (Inbound::Message(message), false),
                        Err(error) => (Inbound::Unreadable(unreadable(&error)), false),
                    },
                    Frame::Text => (Inbound::Unreadable("a text frame".to_owned()), false),
                    Frame::Control => continue,
                    Frame::Closed { code, reason } => (
                        Inbound::Ended(DeepslateError::Closed { code, reason }),
                        true,
                    ),
                    Frame::Failed(error) => (Inbound::Ended(error), true),
                }
            }
        };
        let handed = hand_over(
            &mut socket,
            &mut commands,
            &mut stop,
            &inbound,
            &mut timers,
            item,
        )
        .await;
        match handed {
            Handed::On if !ends => timers.read = tokio::time::Instant::now(),
            Handed::On | Handed::Stopped => break,
            Handed::Failed => return,
        }
    }
    connection::close(socket).await;
}

/// Writes `bytes` as one binary frame, waiting at most `bound`.
async fn write(socket: &mut Socket, bytes: Vec<u8>, bound: Duration) -> Result<(), DeepslateError> {
    match tokio::time::timeout(bound, socket.send(Message::binary(bytes))).await {
        Err(_) => Err(DeepslateError::Stalled(bound)),
        Ok(Err(error)) => Err(DeepslateError::Transport(error.to_string())),
        Ok(Ok(())) => Ok(()),
    }
}

/// Where the output of a turn goes.
enum Target {
    /// The response in progress that holds it.
    Live(ResponseId),
    /// The response in progress that holds it proposed a call the caller answered: what comes
    /// next for the turn is the answer, a new response.
    Answered(ResponseId),
    /// Its response ended: what comes for it is late.
    Ended,
    /// No response holds it yet.
    Unheld,
}

/// One open Opal session through the contract. It owns the runtime its connection runs on;
/// dropping it closes the connection, waiting at most [`CLOSE_BOUND`].
pub struct DeepslateOpalSession {
    runtime: Option<Runtime>,
    commands: Option<mpsc::Sender<Vec<u8>>>,
    inbound: mpsc::Receiver<Inbound>,
    stop: Option<oneshot::Sender<()>>,
    pump: Option<JoinHandle<()>>,
    ledger: SessionLedger,
    output_format: AudioFormat,
    write_timeout: Duration,
    /// The response of each turn that began.
    turns: HashMap<u32, ResponseId>,
    /// The turn of the latest `ResponseBegin`.
    last_turn: Option<u32>,
    /// The turn of each call proposed and not yet answered.
    call_turns: HashMap<String, u32>,
    /// Turns whose response in progress proposed a call the caller answered.
    answered: HashSet<u32>,
    /// The samples the response in progress emitted, the position of its next frame.
    emitted: u64,
    /// The half sample the last chunk of the response in progress ended in.
    carry: Vec<u8>,
    /// The input samples sent so far.
    input_samples: u64,
    /// Text of turns no response holds yet.
    held: BTreeMap<u32, String>,
    held_bytes: usize,
    /// The bytes of text delivered on each response.
    kept: HashMap<ResponseId, usize>,
    text_dropped: u64,
    late_audio: u64,
    failure: Option<DeepslateError>,
    /// The caller is closing: what would fail the session is not recorded.
    closing: bool,
}

impl std::fmt::Debug for DeepslateOpalSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeepslateOpalSession")
            .field("session", self.ledger.session())
            .field("in_progress", &self.ledger.in_progress())
            .field("failure", &self.failure)
            .finish_non_exhaustive()
    }
}

impl Drop for DeepslateOpalSession {
    fn drop(&mut self) {
        self.shutdown(true);
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

impl DeepslateOpalSession {
    /// How many audio chunks of a response that had ended, a cut one among them, were dropped.
    #[must_use]
    pub fn late_audio_dropped(&self) -> u64 {
        self.late_audio
    }

    /// How many bytes of text were neither kept nor delivered: past [`TEXT_LIMIT`], or of a
    /// response that had ended.
    #[must_use]
    pub fn text_dropped(&self) -> u64 {
        self.text_dropped
    }

    /// Why the session failed, once it did.
    #[must_use]
    pub fn failure_cause(&self) -> Option<&DeepslateError> {
        self.failure.as_ref()
    }

    fn or_drop(&mut self, result: Result<(), SessionError>) {
        if let Err(refusal) = result {
            self.ledger.dropped(refusal);
        }
    }

    fn require_open(&self) -> Result<(), SessionError> {
        self.ledger
            .session()
            .require_open()
            .map_err(SessionError::NotOpen)
    }

    /// Stops the connection task: it sends a close frame, bounded by [`CLOSE_BOUND`]. With
    /// `wait`, and outside an async runtime, waits for it at most [`CLOSE_BOUND`] in all.
    fn shutdown(&mut self, wait: bool) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.commands = None;
        let (Some(pump), Some(runtime)) = (self.pump.take(), self.runtime.as_ref()) else {
            return;
        };
        if wait && tokio::runtime::Handle::try_current().is_err() {
            let abort = pump.abort_handle();
            let waited = runtime.block_on(async { tokio::time::timeout(CLOSE_BOUND, pump).await });
            if waited.is_err() {
                abort.abort();
            }
        }
    }

    /// The session fails with `error`, unless the caller is closing it or it already ended.
    fn fail_with(&mut self, error: DeepslateError) {
        if self.closing || self.require_open().is_err() {
            return;
        }
        let kind = match &error {
            DeepslateError::Closed { .. } | DeepslateError::Transport(_) => {
                SessionFailureKind::TransportClosed
            }
            DeepslateError::Stalled(_) => SessionFailureKind::Stalled,
            _ => SessionFailureKind::ProviderError,
        };
        let refused = self.ledger.fail(SessionFailure {
            kind,
            detail: error.to_string(),
        });
        self.or_drop(refused);
        self.failure = Some(error);
        self.carry.clear();
        self.shutdown(false);
    }

    /// Where the output of `turn`, or of the latest turn when it names none, goes.
    fn target(&self, turn: Option<u32>) -> Target {
        let in_progress = self.ledger.in_progress();
        match turn.or(self.last_turn) {
            Some(turn) => match self.turns.get(&turn) {
                Some(id) if Some(*id) == in_progress && self.answered.contains(&turn) => {
                    Target::Answered(*id)
                }
                Some(id) if Some(*id) == in_progress => Target::Live(*id),
                Some(_) => Target::Ended,
                None => Target::Unheld,
            },
            None => in_progress.map_or(Target::Unheld, Target::Live),
        }
    }

    /// A new response for `turn`: the response in progress completes first, and text held for
    /// the turn goes to the new one.
    fn begin(&mut self, turn: Option<u32>, now: Instant) -> Option<ResponseId> {
        if let Some(previous) = self.ledger.in_progress() {
            let done = self.ledger.response_completed(previous, now);
            self.or_drop(done);
        }
        let id = ResponseId::generate();
        if let Err(refusal) = self.ledger.response_created(id, now) {
            self.ledger.dropped(refusal);
            return None;
        }
        self.emitted = 0;
        self.carry.clear();
        if let Some(turn) = turn {
            self.answered.remove(&turn);
            self.turns.insert(turn, id);
            self.last_turn = Some(turn);
            if let Some(text) = self.held.remove(&turn) {
                self.held_bytes -= text.len();
                self.said(id, &text, now);
            }
        }
        Some(id)
    }

    /// The response in progress for the output of `turn`, begun when no response holds the
    /// turn or when it is the answer to a call; `None` when the turn's response ended.
    fn live(&mut self, turn: Option<u32>, now: Instant) -> Option<ResponseId> {
        match self.target(turn) {
            Target::Live(id) => Some(id),
            Target::Ended => None,
            Target::Unheld | Target::Answered(_) => self.begin(turn.or(self.last_turn), now),
        }
    }

    /// The prefix of `text` that fits `room` bytes, on a character boundary.
    fn fitting(text: &str, room: usize) -> &str {
        let mut end = text.len().min(room);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        &text[..end]
    }

    /// `text` on the response `id`, as far as [`TEXT_LIMIT`] for the response allows.
    fn said(&mut self, id: ResponseId, text: &str, now: Instant) {
        let kept = self.kept.entry(id).or_default();
        let added = Self::fitting(text, TEXT_LIMIT.saturating_sub(*kept));
        *kept += added.len();
        self.text_dropped = self
            .text_dropped
            .saturating_add((text.len() - added.len()) as u64);
        if added.is_empty() {
            return;
        }
        let delivered = self.ledger.output_transcript(id, added.to_owned(), now);
        self.or_drop(delivered);
    }

    /// Holds `text` for `turn`, which no response holds yet, within [`TEXT_LIMIT`] for all
    /// turns held.
    fn hold(&mut self, turn: u32, text: &str) {
        let added = Self::fitting(text, TEXT_LIMIT.saturating_sub(self.held_bytes));
        self.text_dropped = self
            .text_dropped
            .saturating_add((text.len() - added.len()) as u64);
        self.held_bytes += added.len();
        self.held.entry(turn).or_default().push_str(added);
    }

    /// Emits `audio` on the response `id`, whole samples only; a half sample waits for the next
    /// chunk.
    fn output(&mut self, id: ResponseId, audio: &[u8], now: Instant) {
        let width = usize::from(self.output_format.channels.get()) * 2;
        let mut data = std::mem::take(&mut self.carry);
        data.extend_from_slice(audio);
        let whole = data.len() - data.len() % width;
        self.carry = data.split_off(whole);
        if data.is_empty() {
            return;
        }
        let samples = (data.len() / width) as u64;
        let (Some(position), Ok(sample_count)) = (
            SampleClockPosition::new(self.emitted),
            u32::try_from(samples),
        ) else {
            return;
        };
        let frame = AudioFrame {
            format: self.output_format,
            position,
            sample_count,
            data,
        };
        match self.ledger.output_audio(id, frame, now) {
            Ok(()) => self.emitted += samples,
            Err(refusal) => self.ledger.dropped(refusal),
        }
    }

    /// The input position a `VadStateEvent` names: the packet id, at most the input sent.
    fn position(&self, packet_id: u64) -> SampleClockPosition {
        SampleClockPosition::new(packet_id.min(self.input_samples))
            .unwrap_or(SampleClockPosition::MAX)
    }

    /// Records one thing the connection task handed over.
    fn record(&mut self, inbound: Inbound, now: Instant) {
        match inbound {
            Inbound::Unreadable(what) => self.ledger.unreadable_frame(what, now),
            Inbound::Ended(error) => self.fail_with(error),
            Inbound::Message(message) => {
                self.ledger.heard(now);
                self.message(message, now);
            }
        }
    }

    fn message(&mut self, message: ClientBound, now: Instant) {
        match message {
            ClientBound::ResponseBegin { turn_id } => {
                if !matches!(self.target(Some(turn_id)), Target::Live(_)) {
                    self.begin(Some(turn_id), now);
                }
                self.last_turn = Some(turn_id);
            }
            ClientBound::ModelAudioChunk { audio, turn_id } => match self.live(turn_id, now) {
                Some(id) => self.output(id, &audio, now),
                None => self.late_audio = self.late_audio.saturating_add(1),
            },
            ClientBound::ModelTextFragment { text, turn_id } => match self.target(Some(turn_id)) {
                Target::Live(id) => self.said(id, &text, now),
                Target::Answered(_) => {
                    if let Some(id) = self.begin(Some(turn_id), now) {
                        self.said(id, &text, now);
                    }
                }
                Target::Ended => {
                    self.text_dropped = self.text_dropped.saturating_add(text.len() as u64);
                }
                Target::Unheld => self.hold(turn_id, &text),
            },
            ClientBound::ResponseEnd { turn_id } => {
                if let Target::Live(id) | Target::Answered(id) = self.target(Some(turn_id)) {
                    self.answered.remove(&turn_id);
                    let done = self.ledger.response_completed(id, now);
                    self.or_drop(done);
                    self.carry.clear();
                }
            }
            ClientBound::PlaybackClearBuffer => {
                if let Some(id) = self.ledger.in_progress() {
                    let cancelled = self.ledger.response_cancelled(id, now);
                    self.or_drop(cancelled);
                    self.carry.clear();
                }
            }
            ClientBound::VadStateEvent {
                from,
                to,
                packet_id,
            } => {
                let position = self.position(packet_id);
                match (from, to) {
                    (VadState::Silence | VadState::SpeechStarting, VadState::Speech) => {
                        let started = self.ledger.speech_started(position, now);
                        self.or_drop(started);
                    }
                    (VadState::Speech | VadState::SpeechEnding, VadState::Silence) => {
                        let stopped = self.ledger.speech_stopped(position, now);
                        self.or_drop(stopped);
                        let committed = self.ledger.input_committed(now);
                        self.or_drop(committed);
                    }
                    _ => {}
                }
            }
            ClientBound::UserTranscription { text, .. } => {
                let heard = self.ledger.input_transcribed(text, now);
                self.or_drop(heard);
            }
            ClientBound::ToolCallRequest {
                id,
                name,
                parameters,
                turn_id,
            } => {
                let offered = self.ledger.tools().iter().any(|tool| tool.name() == name);
                if !offered {
                    let refusal = ServiceBound::ToolCallResponse {
                        id,
                        result: tool_unavailable(&name),
                    };
                    self.ledger.dropped(SessionError::ToolNotOffered { name });
                    let _ = self.send(refusal.encode());
                    return;
                }
                let response = match self.target(turn_id) {
                    Target::Live(response) => Some(response),
                    Target::Answered(_) | Target::Ended | Target::Unheld => {
                        self.begin(turn_id, now)
                    }
                };
                if let Some(response) = response {
                    let turn = self
                        .turns
                        .iter()
                        .find(|(_, held)| **held == response)
                        .map(|(turn, _)| *turn);
                    if let Some(turn) = turn {
                        self.call_turns.insert(id.clone(), turn);
                    }
                    let proposed = self.ledger.tool_call(
                        SpeechCallId::new(id),
                        response,
                        &name,
                        parameters,
                        now,
                    );
                    self.or_drop(proposed);
                }
            }
            ClientBound::Error {
                category,
                message,
                trace_id,
            } => self.fail_with(server_error(category, &message, trace_id)),
            ClientBound::SessionReady
            | ClientBound::InferenceComplete { .. }
            | ClientBound::Pong
            | ClientBound::Other(_)
            | ClientBound::Empty => {}
        }
    }

    /// Records what already reached the session, without waiting.
    fn record_arrived(&mut self) {
        while self.require_open().is_ok() {
            match self.inbound.try_recv() {
                Ok(inbound) => self.record(inbound, Instant::now()),
                Err(_) => break,
            }
        }
    }

    /// Hands `bytes` to the connection task; a task that is gone fails the session with what it
    /// said last.
    fn send(&mut self, bytes: Vec<u8>) -> Result<(), SessionError> {
        let sent = match (self.commands.as_ref(), self.runtime.as_ref()) {
            (Some(commands), Some(runtime)) => runtime.block_on(async {
                tokio::time::timeout(self.write_timeout, commands.send(bytes)).await
            }),
            _ => return self.require_open(),
        };
        match sent {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(_)) => {
                self.record_arrived();
                self.fail_with(DeepslateError::Transport("the connection ended".to_owned()));
            }
            Err(_) => self.fail_with(DeepslateError::Stalled(self.write_timeout)),
        }
        self.require_open()
    }
}

impl SpeechSessionHandle for DeepslateOpalSession {
    fn ledger(&self) -> &SessionLedger {
        &self.ledger
    }

    fn append_audio(&mut self, frame: AudioFrame) -> Result<(), SessionError> {
        let packet_id = self.input_samples;
        let samples = u64::from(frame.sample_count);
        let data = frame.data.clone();
        self.ledger.accept_input(frame)?;
        self.input_samples = self.input_samples.saturating_add(samples);
        if data.is_empty() {
            return Ok(());
        }
        self.send(
            ServiceBound::Audio {
                packet_id,
                mode: TriggerMode::Immediate,
                data,
            }
            .encode(),
        )
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
        let text = match &result {
            Json::String(text) => text.clone(),
            other => other.to_string(),
        };
        self.ledger.answer(call_id, result)?;
        if let Some(turn) = self.call_turns.remove(call_id.as_str())
            && matches!(self.target(Some(turn)), Target::Live(_))
        {
            self.answered.insert(turn);
        }
        self.send(
            ServiceBound::ToolCallResponse {
                id: call_id.as_str().to_owned(),
                result: text,
            }
            .encode(),
        )
    }

    /// Closes the session. What already reached the session is recorded first and delivered
    /// after the close, before `SpeechSessionClosed`, except what would fail the session; then
    /// the connection task sends a close frame, waited for at most [`CLOSE_BOUND`] in all.
    fn close(&mut self) -> Result<(), SessionError> {
        if self.require_open().is_ok() {
            self.closing = true;
            self.record_arrived();
            self.shutdown(true);
        }
        self.ledger.close()?;
        self.carry.clear();
        Ok(())
    }

    fn next_event(&mut self, wait: Duration) -> Option<SessionEvent> {
        let deadline = Instant::now().checked_add(wait);
        loop {
            if let Some(event) = self.ledger.next_event() {
                return Some(event);
            }
            let left = deadline.map_or(wait, |deadline| {
                deadline.saturating_duration_since(Instant::now())
            });
            if left.is_zero() || self.require_open().is_err() {
                self.ledger.check_liveness(Instant::now());
                return self.ledger.next_event();
            }
            let runtime = self.runtime.as_ref()?;
            let inbound = &mut self.inbound;
            let read = runtime.block_on(async { tokio::time::timeout(left, inbound.recv()).await });
            match read {
                Ok(Some(inbound)) => self.record(inbound, Instant::now()),
                Ok(None) => self.fail_with(DeepslateError::Closed {
                    code: None,
                    reason: "the connection task ended".to_owned(),
                }),
                Err(_) => {
                    self.ledger.check_liveness(Instant::now());
                }
            }
        }
    }
}
