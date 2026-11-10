//! A Deepgram Voice Agent session as an `inference-duplex` native speech model.
//!
//! [`DeepgramAgent`] opens a Voice Agent session over the WebSocket of [`crate::agent`] and
//! records what the agent sends through the contract's [`SessionLedger`], so it refuses what
//! every implementation refuses and yields the contract's events in order.
//!
//! # Opening
//!
//! [`DeepgramAgent::new`] refuses an endpoint or configuration it cannot use, before anything
//! connects: a base URL that is not `wss://` (or `ws://` to a loopback host), an `Authorization`
//! scheme that is not plain letters, a zero keep-alive interval, a blank provider.
//! [`NativeSpeechModel::open`] admits the session configuration, whose formats must be ones the
//! Voice Agent takes ([`agent::encoding`]), resolves the key through the injected
//! [`CredentialSource`] and refuses an empty or blank one before connecting, connects with
//! `Authorization: <scheme> <key>`, waits for `Welcome`, sends `Settings` once and waits for
//! `SettingsApplied`. The whole opening is bounded by [`AgentEndpoint::open_timeout`] and races
//! [`OpenCanceller::cancel`]: a cancel drops a handshake in progress, which closes the TCP
//! connection at once, or sends a close frame, bounded by [`CLOSE_BOUND`], once the WebSocket is
//! open. A cancel reaches only an open in progress when it is issued; one issued while none is
//! does nothing. A session voice longer than a provider field may be is refused before
//! connecting ([`DeepgramError::Config`]). A close before `SettingsApplied` fails the open as
//! [`DeepgramError::ClosedBeforeSettings`], an `Error` as [`DeepgramError::Server`] with its
//! code. Every way an open fails leaves the reason in [`DeepgramAgent::open_error`]. No audio is
//! sent before `SettingsApplied`.
//!
//! # The connection
//!
//! Each session runs a runtime of one worker thread, on which one task owns the socket: it reads
//! every message, writes what the session sends, sends `KeepAlive` after
//! [`AgentEndpoint::keepalive_interval`] without audio and a WebSocket ping after as long
//! without reading anything, and hands what it read to the session through a queue of
//! [`INBOUND_DEPTH`] messages. It reads each message before it waits for a slot, and answers a
//! WebSocket ping as it reads it; a message the caller need not see (`AgentThinking`,
//! `History`, the other passed-over types) takes a slot only when one is free at once, so a
//! flood of them never stops the task reading. While it waits for a slot for a message the
//! caller must see, it reads nothing more, but still writes what the session sends, sends
//! `KeepAlive` and pings, and obeys a stop. The session's
//! writes go through a queue of [`OUTBOUND_QUEUE`] messages, each send waiting at most
//! [`AgentEndpoint::read_timeout`]. A message or frame larger than [`agent::MAX_MESSAGE_SIZE`] fails the
//! session as a transport error, and a connection that sends nothing, pongs included, for
//! [`AgentEndpoint::read_timeout`] while the task reads fails it as stalled. Closing or dropping
//! the session sends a close frame and waits at most [`CLOSE_BOUND`] in all for it; dropping one
//! inside an async runtime does not wait, and the connection closes with the runtime. Drive a
//! session from a blocking thread, never from inside an async runtime.
//!
//! # The mapping onto the contract
//!
//! * The agent's turn-taking is its own: [`TurnDetection::ServerVad`] is the only mode, with no
//!   tuning; `auto_response` and `auto_interrupt` must be on. Commit, response creation,
//!   cancellation and truncation by the caller are refused as unsupported: the protocol has no
//!   message for them.
//! * `ConversationText` of the user is the end of a user turn: `InputCommitted`, then
//!   `InputTranscribed`. The API names no turn, so the binding names each user turn by the next
//!   `turn-N` of the ledger's per-session counter, taken at the commit and given to the
//!   transcript.
//! * A response begins at `AgentStartedSpeaking`, or at a binary frame or a client-side
//!   `FunctionCallRequest` while none is in progress. Its audio frames count samples from its
//!   start; `linear16` ending in half a sample keeps that byte for the next frame. It completes
//!   (`ResponseCompleted`) at `AgentAudioDone`, or at the next `AgentStartedSpeaking` once it has
//!   sent audio. `ConversationText` of the assistant is its `OutputTranscriptDelta`: given to the
//!   response in progress, or, since it arrives before `AgentStartedSpeaking`, held and given to
//!   the next one when none is. The latencies of `AgentStartedSpeaking` are kept on the session
//!   ([`DeepgramAgentSession::latency`]). Held words, and the words of each response, are kept
//!   and delivered up to [`agent::TEXT_LIMIT`] bytes; the bytes past it are counted
//!   ([`DeepgramAgentSession::text_dropped`]).
//! * Frames after `AgentAudioDone` with no `AgentStartedSpeaking` before them open a response
//!   of their own, which completes at the next `AgentStartedSpeaking`, `UserStartedSpeaking`,
//!   user `ConversationText`, a close by the caller, or [`TRAILING_QUIET`] (1 s, or half the idle
//!   limit when that is shorter) without a frame. No frame is dropped, and a silent agent
//!   leaves no response in progress, so the idle limit fails no healthy session. This rule is
//!   inferred, not stated ([`crate::agent`]).
//! * A `THINK_REQUEST_FAILED` or `SPEAK_REQUEST_FAILED` warning ends the reply it concerns: the
//!   response in progress fails (`ResponseFailed`) and its later frames are dropped as a cut
//!   reply's are; with none in progress, while a reply to the user's words is awaited, a
//!   response is created and fails at once, so the caller stops awaiting it. The session stays
//!   open and the warning is kept. That no reply follows these warnings is inferred.
//! * `UserStartedSpeaking` is the user talking over the agent: `SpeechStarted` at the input sent
//!   so far, then the response in progress ends as cancelled (`ResponseCancelled`). Its frames
//!   that still arrive, up to the next `AgentStartedSpeaking` or `AgentAudioDone`, are dropped
//!   and counted ([`DeepgramAgentSession::late_audio_dropped`]); either may come first, since
//!   whether `AgentAudioDone` follows a cut reply is not stated.
//! * A client-side `FunctionCallRequest` function the session offers as a tool is proposed on
//!   the response in progress (`SpeechToolCallProposed`);
//!   [`SpeechSessionHandle::submit_tool_result`] sends `FunctionCallResponse`, the result as its
//!   `content` (a JSON string as it is, any other value as its JSON text). One the session does
//!   not offer fails the session as a provider error naming it
//!   ([`DeepgramError::FunctionNotOffered`]). Server-side functions are passed over.
//! * `Error` fails the session as a provider error ([`DeepgramError::Server`], with its code);
//!   `Warning` is kept ([`DeepgramAgentSession::warnings`]) and is not fatal. A close by the
//!   server or a failed read fails the session as a closed transport
//!   ([`DeepgramError::Closed`], [`DeepgramError::Transport`]).
//!   [`DeepgramAgentSession::failure_cause`] keeps the reason.
//! * `AgentThinking`, `FunctionCallCancelled`, `PromptUpdated`, `InjectionRefused`,
//!   `LatencyReport`, `History`, a repeated `Welcome` or `SettingsApplied` and types the binding
//!   does not know are read and passed over; each is a sign of life. A text the binding cannot
//!   read is counted on the session ([`SessionLedger::unreadable_frames`]).
//! * Usage: no message carries any, so no response usage is reported and each response's usage
//!   record carries every count unknown.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
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
    SpeechSessionHandle, SpeechSessionId, TuningSupport, TurnDetection, Unsupported,
};
use tokio::runtime::Runtime;
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::{self, Bytes, Message};

use crate::agent::{
    self, AgentConfig, AgentWarning, INBOUND_DEPTH, KEEP_ALIVE, Latency, Role, ServerMessage,
};
use crate::config::AgentEndpoint;
use crate::connection::{self, CLOSE_BOUND, Socket};
use crate::credential::{CredentialSource, DEEPGRAM_KEY_NAME, EnvironmentSource};
use crate::error::{ConfigError, DeepgramError};

/// The backend an adapter record of the binding names.
pub const AGENT_BACKEND: &str = "deepgram-voice-agent";

/// The model the claims record names: the session's `Settings` choose the models, not the
/// binding.
pub const AGENT_MODEL: &str = "settings-configured";

/// How many messages the session holds for the connection task to write.
pub const OUTBOUND_QUEUE: usize = 64;

/// The most warnings a session keeps; later ones are counted only.
pub const WARNINGS_KEPT: usize = 64;

/// How long a response of frames that came after `AgentAudioDone` waits for another frame
/// before it completes, unless half the session's idle limit is shorter.
pub const TRAILING_QUIET: Duration = Duration::from_secs(1);

/// The warning codes after which the binding takes the reply they concern as never coming.
const REPLY_FAILED: [&str; 2] = ["THINK_REQUEST_FAILED", "SPEAK_REQUEST_FAILED"];

/// What the Voice Agent takes, as the duplex contract states it: the formats of
/// [`agent::audio_formats`] each way; its own turn-taking as `server_vad` without tuning;
/// responses and interruption by the agent alone; no cancellation or truncation by the caller;
/// client-side functions.
pub fn agent_capabilities() -> SpeechCapabilities {
    SpeechCapabilities {
        input_formats: agent::audio_formats(),
        output_formats: agent::audio_formats(),
        turn_detection: vec![TurnDetection::ServerVad],
        turn_tuning: TuningSupport::NONE,
        manual_commit: false,
        manual_response: false,
        manual_interrupt: false,
        client_cancel: false,
        truncation: false,
        tool_calling: true,
    }
}

/// The capabilities an adapter record of the binding claims, the same as
/// `records/claims-voice-agent.json`.
///
/// Not `native_duplex`: `conformance::native_duplex_evidence` over the fixture server, which cuts
/// a reply on speech over it as the documentation describes, ends in
/// [`NoNativeDuplex::Restarted`], the cut reply cancelled and the answer in a new one. Not
/// `interruptible`: the caller cannot cancel a reply, and nothing states a latency for the
/// agent's own barge-in.
pub fn agent_claims() -> Vec<Capability> {
    vec![
        Capability::OutputStreaming,
        Capability::IncrementalInput,
        Capability::ConcurrentInputOutput,
        Capability::ToolCalling,
    ]
}

/// Whether [`agent_claims`] agree with an outcome of `conformance::native_duplex_evidence`.
///
/// # Errors
/// The reason, when the outcome says nothing about the model: the session did not open, failed,
/// was refused before the input during output, or never answered.
pub fn agent_native_duplex_verdict(
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
    Ok(agent_claims().contains(&Capability::NativeDuplex) == outcome.is_ok())
}

/// Cancels the open in progress on the model it came from. A cancel issued while no open is in
/// progress does nothing: it never reaches a later open.
#[derive(Debug, Clone)]
pub struct OpenCanceller(Arc<CancelState>);

impl OpenCanceller {
    /// Cancels the open in progress, if one is.
    pub fn cancel(&self) {
        self.0.cancel();
    }
}

/// Which open is in progress and which one a cancel named, by generation; 0 is none.
#[derive(Debug, Default)]
struct CancelState {
    opens: Mutex<(u64, u64)>,
    notify: Notify,
}

impl CancelState {
    fn opens(&self) -> MutexGuard<'_, (u64, u64)> {
        self.opens.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The open of `generation` is in progress.
    fn begin(&self, generation: u64) {
        self.opens().0 = generation;
    }

    /// No open is in progress.
    fn end(&self) {
        self.opens().0 = 0;
    }

    /// Cancels the open in progress, if one is.
    fn cancel(&self) {
        let mut opens = self.opens();
        if opens.0 != 0 {
            opens.1 = opens.0;
            drop(opens);
            self.notify.notify_waiters();
        }
    }

    /// Resolves once the open of `generation` is cancelled.
    async fn cancelled(&self, generation: u64) {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.opens().1 == generation {
                return;
            }
            notified.await;
        }
    }
}

/// A Deepgram Voice Agent as a native speech model.
pub struct DeepgramAgent {
    adapter: Adapter,
    url: String,
    endpoint: AgentEndpoint,
    config: AgentConfig,
    source: Arc<dyn CredentialSource>,
    credential: String,
    capabilities: SpeechCapabilities,
    cancel: Arc<CancelState>,
    generation: u64,
    open_error: Option<DeepgramError>,
}

impl std::fmt::Debug for DeepgramAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeepgramAgent")
            .field("adapter", &self.adapter.adapter_id())
            .field("url", &self.url)
            .field("credential", &self.credential)
            .field("open_error", &self.open_error)
            .finish_non_exhaustive()
    }
}

impl DeepgramAgent {
    /// The Voice Agent at `endpoint`, asked for `config`, its key the secret `credential` of
    /// `source`, under an adapter record of its own claiming [`agent_claims`].
    ///
    /// # Errors
    /// The [`ConfigError`] of [`AgentEndpoint::base`] or [`AgentConfig::validate`]; nothing
    /// connects.
    pub fn new(
        endpoint: AgentEndpoint,
        config: AgentConfig,
        source: Arc<dyn CredentialSource>,
        credential: impl Into<String>,
    ) -> Result<Self, ConfigError> {
        let url = format!("{}{}", endpoint.base()?, agent::PATH);
        config.validate()?;
        let adapter = Adapter::declare(DeclareAdapter {
            model_id: ModelId::generate(),
            backend: AGENT_BACKEND.to_owned(),
            contract_revision: "1".to_owned(),
            execution_form: ExecutionForm::HostedService,
            locality: Locality::ExternalProvider,
            capabilities: agent_claims(),
            max_concurrency: 1,
            descriptor: AdapterDescriptor::default(),
        })
        .map(|(adapter, _)| adapter)
        .map_err(|_| ConfigError::MaxConcurrency(1))?;
        Ok(Self {
            adapter,
            url,
            endpoint,
            config,
            source,
            credential: credential.into(),
            capabilities: agent_capabilities(),
            cancel: Arc::new(CancelState::default()),
            generation: 0,
            open_error: None,
        })
    }

    /// The Voice Agent at `endpoint`, asked for `config`, its key read from `DEEPGRAM_API_KEY`.
    ///
    /// # Errors
    /// As [`DeepgramAgent::new`].
    pub fn from_env(endpoint: AgentEndpoint, config: AgentConfig) -> Result<Self, ConfigError> {
        Self::new(
            endpoint,
            config,
            Arc::new(EnvironmentSource::deepgram()),
            DEEPGRAM_KEY_NAME,
        )
    }

    /// This model under an adapter record declared elsewhere, such as a model store's.
    pub fn with_adapter(mut self, adapter: Adapter) -> Self {
        self.adapter = adapter;
        self
    }

    /// A handle that cancels this model's open in progress from another thread.
    pub fn canceller(&self) -> OpenCanceller {
        OpenCanceller(Arc::clone(&self.cancel))
    }

    /// Why the latest open failed; `None` after an open that succeeded or before any.
    pub fn open_error(&self) -> Option<&DeepgramError> {
        self.open_error.as_ref()
    }

    /// `error` as the open's failure, kept in [`Self::open_error`].
    fn failed(&mut self, error: DeepgramError) -> OpenError {
        let kind = match &error {
            DeepgramError::Connect(connect) if connect.is_refusal() => SessionFailureKind::Rejected,
            DeepgramError::Credential(_) | DeepgramError::Config(_) => SessionFailureKind::Rejected,
            DeepgramError::Stalled(_) => SessionFailureKind::Stalled,
            DeepgramError::Server { .. } | DeepgramError::Protocol(_) => {
                SessionFailureKind::ProviderError
            }
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

impl NativeSpeechModel for DeepgramAgent {
    type Session = DeepgramAgentSession;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn capabilities(&self) -> &SpeechCapabilities {
        &self.capabilities
    }

    fn open(&mut self, config: SessionConfig) -> Result<DeepgramAgentSession, OpenError> {
        self.open_error = None;
        let (mut ledger, _) = SessionLedger::open(
            SpeechSessionId::generate(),
            &self.adapter,
            &self.capabilities,
            config.clone(),
            Instant::now(),
        )?;
        let Some(settings) = agent::settings(&self.config, &config) else {
            return Err(OpenError::Unsupported(Unsupported::InputFormat(
                config.input_format,
            )));
        };
        if let Err(refused) = agent::validate_voice(&config.voice) {
            return Err(self.failed(DeepgramError::Config(refused)));
        }
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("deepgram-agent")
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => return Err(self.failed(DeepgramError::Runtime(error.to_string()))),
        };
        self.generation += 1;
        self.cancel.begin(self.generation);
        let link = Link {
            url: &self.url,
            endpoint: &self.endpoint,
            source: self.source.as_ref(),
            credential: &self.credential,
        };
        let opened = runtime.block_on(connect(&link, settings, &self.cancel, self.generation));
        self.cancel.end();
        let (socket, opening) = match opened {
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
        let mut session = DeepgramAgentSession {
            runtime: Some(runtime),
            commands: Some(commands),
            inbound,
            stop: Some(stop),
            pump: Some(pump),
            ledger,
            output_format: config.output_format,
            write_timeout: self.endpoint.read_timeout,
            request_id: opening.request_id,
            current: None,
            emitted: 0,
            carry: Vec::new(),
            input_samples: 0,
            dropping: false,
            late_audio: 0,
            pending_text: None,
            transcripts: HashMap::new(),
            latencies: HashMap::new(),
            warnings: Vec::new(),
            warnings_seen: 0,
            text_dropped: 0,
            after_done: false,
            trailing: None,
            quiet: TRAILING_QUIET.min(config.idle_limit / 2),
            awaiting_reply: false,
            calls: HashMap::new(),
            failure: None,
            closing: false,
        };
        let now = Instant::now();
        for inbound in opening.early {
            session.record(inbound, now);
        }
        Ok(session)
    }
}

/// Where and how a session connects.
struct Link<'a> {
    url: &'a str,
    endpoint: &'a AgentEndpoint,
    source: &'a dyn CredentialSource,
    credential: &'a str,
}

/// What the opening read besides `SettingsApplied`.
struct Opening {
    request_id: Option<String>,
    early: Vec<Inbound>,
}

/// Connects, waits for `Welcome`, sends `settings` and waits for `SettingsApplied`, all within
/// the open timeout and raced against `cancel`. A connection that fails after the handshake is
/// closed before this returns.
async fn connect(
    link: &Link<'_>,
    settings: String,
    cancel: &CancelState,
    generation: u64,
) -> Result<(Socket, Opening), DeepgramError> {
    let bound = link.endpoint.open_timeout;
    let deadline = tokio::time::Instant::now() + bound;
    let handshake = connection::open_with_scheme(
        link.url,
        link.source,
        link.credential,
        &link.endpoint.auth_scheme,
        bound,
    );
    let mut socket = tokio::select! {
        biased;
        () = cancel.cancelled(generation) => return Err(DeepgramError::Cancelled),
        connected = tokio::time::timeout_at(deadline, handshake) => {
            connected.unwrap_or(Err(DeepgramError::Stalled(bound)))?
        }
    };
    let waited = tokio::select! {
        biased;
        () = cancel.cancelled(generation) => Err(DeepgramError::Cancelled),
        waited = tokio::time::timeout_at(deadline, await_applied(&mut socket, settings)) => {
            waited.unwrap_or(Err(DeepgramError::Stalled(bound)))
        }
    };
    match waited {
        Ok(opening) => Ok((socket, opening)),
        Err(error) => {
            connection::close(socket).await;
            Err(error)
        }
    }
}

/// Reads until `Welcome`, sends `settings` once, then reads until `SettingsApplied`. An `Error`
/// or a close ends the wait. What else arrives is kept, at most [`INBOUND_DEPTH`] messages.
async fn await_applied(socket: &mut Socket, settings: String) -> Result<Opening, DeepgramError> {
    let mut settings = Some(settings);
    let mut opening = Opening {
        request_id: None,
        early: Vec::new(),
    };
    loop {
        let inbound = match next_frame(socket).await {
            Frame::Text(text) => match agent::parse(&text) {
                Ok(ServerMessage::Welcome { request_id }) => {
                    if let Some(settings) = settings.take() {
                        opening.request_id = request_id;
                        socket
                            .send(Message::text(settings))
                            .await
                            .map_err(|error| DeepgramError::Transport(error.to_string()))?;
                    }
                    continue;
                }
                Ok(ServerMessage::SettingsApplied) if settings.is_none() => return Ok(opening),
                Ok(ServerMessage::Error { code, description }) => {
                    return Err(DeepgramError::Server { code, description });
                }
                Ok(message) => Inbound::Message(message),
                Err(what) => Inbound::Unreadable(what),
            },
            Frame::Binary(audio) => Inbound::Audio(audio),
            Frame::Control => continue,
            Frame::Closed { code, reason } => {
                return Err(DeepgramError::ClosedBeforeSettings { code, reason });
            }
            Frame::Failed(error) => return Err(error),
        };
        if opening.early.len() < INBOUND_DEPTH {
            opening.early.push(inbound);
        }
    }
}

/// One frame the connection read.
enum Frame {
    Text(String),
    Binary(Bytes),
    /// A WebSocket ping or pong: a sign of life, nothing to hand over.
    Control,
    Closed {
        code: Option<u16>,
        reason: String,
    },
    Failed(DeepgramError),
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
        Message::Text(text) => Frame::Text(text.as_str().to_owned()),
        Message::Binary(data) => Frame::Binary(data),
        Message::Close(frame) => Frame::Closed {
            code: frame.as_ref().map(|frame| u16::from(frame.code)),
            reason: frame
                .map(|frame| frame.reason.as_str().chars().take(256).collect())
                .unwrap_or_default(),
        },
        Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => Frame::Control,
    }
}

fn read_error(error: tungstenite::Error) -> DeepgramError {
    match error {
        tungstenite::Error::Capacity(capacity) => DeepgramError::Transport(capacity.to_string()),
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => {
            DeepgramError::Closed {
                code: None,
                reason: String::new(),
                cause: crate::error::CloseCause::Other,
            }
        }
        other => DeepgramError::Transport(other.to_string()),
    }
}

/// What the connection task hands the session.
enum Inbound {
    Message(ServerMessage),
    /// A binary frame: agent audio.
    Audio(Bytes),
    /// A text the binding could not read, and what it was.
    Unreadable(String),
    /// The connection ended, and why.
    Ended(DeepgramError),
}

/// What the session hands the connection task to write.
enum Outbound {
    Text(String),
    Audio(Vec<u8>),
}

/// The connection task's clocks: when it last sent audio or a `KeepAlive`, last pinged, and
/// last read anything.
struct Timers {
    keepalive: Duration,
    read_timeout: Duration,
    sent: tokio::time::Instant,
    pinged: tokio::time::Instant,
    read: tokio::time::Instant,
}

impl Timers {
    fn new(endpoint: &AgentEndpoint) -> Self {
        let now = tokio::time::Instant::now();
        Self {
            keepalive: endpoint.keepalive_interval,
            read_timeout: endpoint.read_timeout,
            sent: now,
            pinged: now,
            read: now,
        }
    }

    /// When something is next due: a `KeepAlive`, a ping, or, while reading, the stall.
    fn next(&self, reading: bool) -> tokio::time::Instant {
        let mut next =
            (self.sent + self.keepalive).min(self.pinged.max(self.read) + self.keepalive);
        if reading {
            next = next.min(self.read + self.read_timeout);
        }
        next
    }

    /// Sends what is due; a stall, when `reading` and nothing was read within the read timeout.
    async fn fire(&mut self, socket: &mut Socket, reading: bool) -> Result<(), DeepgramError> {
        let now = tokio::time::Instant::now();
        if reading && now >= self.read + self.read_timeout {
            return Err(DeepgramError::Stalled(self.read_timeout));
        }
        if now >= self.sent + self.keepalive {
            write(socket, Message::text(KEEP_ALIVE), self.read_timeout).await?;
            self.sent = now;
        }
        if now >= self.pinged.max(self.read) + self.keepalive {
            write(socket, Message::Ping(Bytes::new()), self.read_timeout).await?;
            self.pinged = now;
        }
        Ok(())
    }

    /// Writes `outbound`; audio restarts the keep-alive clock.
    async fn send(&mut self, socket: &mut Socket, outbound: Outbound) -> Result<(), DeepgramError> {
        let message = match outbound {
            Outbound::Text(text) => Message::text(text),
            Outbound::Audio(audio) => {
                self.sent = tokio::time::Instant::now();
                Message::binary(audio)
            }
        };
        write(socket, message, self.read_timeout).await
    }
}

/// What woke the connection task while it read.
enum Woken {
    Stop,
    Command(Option<Outbound>),
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

/// Whether the caller must see `message`: everything but the types the session passes over.
fn must_see(message: &ServerMessage) -> bool {
    !matches!(
        message,
        ServerMessage::Ignored { .. }
            | ServerMessage::Welcome { .. }
            | ServerMessage::SettingsApplied
    )
}

/// Hands `item` to the session, waiting for a slot of the inbound queue; meanwhile it still
/// writes what the session sends, sends what the timers make due, and obeys a stop.
async fn hand_over(
    socket: &mut Socket,
    commands: &mut mpsc::Receiver<Outbound>,
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
                Some(outbound) => timers.send(socket, outbound).await,
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

/// The connection task: reads, writes what the session sends, keeps the connection alive,
/// until the session stops it or drops its queue, or the connection ends. Each message is read
/// before a slot of the inbound queue is reserved for it; a WebSocket ping is answered as it is
/// read and takes no slot, and a message the caller need not see takes one only when one is
/// free at once. While no slot is free for a message the caller must see, the task reads
/// nothing more, and its timers no longer count a stall.
async fn pump(
    mut socket: Socket,
    mut commands: mpsc::Receiver<Outbound>,
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
            Woken::Command(Some(outbound)) => {
                if let Err(error) = timers.send(&mut socket, outbound).await {
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
                    Frame::Text(text) => match agent::parse(&text) {
                        Ok(message) if !must_see(&message) => {
                            let _ = inbound.try_send(Inbound::Message(message));
                            continue;
                        }
                        Ok(message) => (Inbound::Message(message), false),
                        Err(what) => (Inbound::Unreadable(what), false),
                    },
                    Frame::Binary(audio) => (Inbound::Audio(audio), false),
                    Frame::Control => continue,
                    Frame::Closed { code, reason } => {
                        let cause = crate::error::CloseCause::of(code, &reason);
                        let closed = DeepgramError::Closed {
                            code,
                            reason,
                            cause,
                        };
                        (Inbound::Ended(closed), true)
                    }
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

/// Writes `message`, waiting at most `bound`.
async fn write(
    socket: &mut Socket,
    message: Message,
    bound: Duration,
) -> Result<(), DeepgramError> {
    match tokio::time::timeout(bound, socket.send(message)).await {
        Err(_) => Err(DeepgramError::Stalled(bound)),
        Ok(Err(error)) => Err(DeepgramError::Transport(error.to_string())),
        Ok(Ok(())) => Ok(()),
    }
}

/// One open Voice Agent session through the contract. It owns the runtime its connection runs
/// on; dropping it closes the connection, waiting at most [`CLOSE_BOUND`].
pub struct DeepgramAgentSession {
    runtime: Option<Runtime>,
    commands: Option<mpsc::Sender<Outbound>>,
    inbound: mpsc::Receiver<Inbound>,
    stop: Option<oneshot::Sender<()>>,
    pump: Option<JoinHandle<()>>,
    ledger: SessionLedger,
    output_format: AudioFormat,
    write_timeout: Duration,
    request_id: Option<String>,
    /// The response in progress.
    current: Option<ResponseId>,
    /// The samples the response in progress emitted, the position of its next frame.
    emitted: u64,
    /// The half sample the last frame of the response in progress ended in.
    carry: Vec<u8>,
    /// The input samples sent so far.
    input_samples: u64,
    /// A reply was cut: its frames are dropped until the next one starts or it is done.
    dropping: bool,
    late_audio: u64,
    /// Assistant words that arrived while no response was in progress.
    pending_text: Option<String>,
    transcripts: HashMap<ResponseId, String>,
    latencies: HashMap<ResponseId, Latency>,
    warnings: Vec<AgentWarning>,
    warnings_seen: u64,
    /// Bytes of assistant words past [`agent::TEXT_LIMIT`], neither kept nor delivered.
    text_dropped: u64,
    /// The last reply ended at `AgentAudioDone` and no `AgentStartedSpeaking` came since.
    after_done: bool,
    /// The response in progress was opened by frames after `AgentAudioDone`: when its last
    /// frame arrived.
    trailing: Option<Instant>,
    /// How long a trailing response waits for another frame ([`TRAILING_QUIET`], or half the
    /// idle limit when that is shorter).
    quiet: Duration,
    /// The user's words arrived and no response began since.
    awaiting_reply: bool,
    /// The function name of each call proposed, by its id.
    calls: HashMap<String, String>,
    failure: Option<DeepgramError>,
    /// The caller is closing: what would fail the session is not recorded.
    closing: bool,
}

impl std::fmt::Debug for DeepgramAgentSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeepgramAgentSession")
            .field("session", self.ledger.session())
            .field("request_id", &self.request_id)
            .field("current", &self.current)
            .field("failure", &self.failure)
            .finish_non_exhaustive()
    }
}

impl Drop for DeepgramAgentSession {
    fn drop(&mut self) {
        self.shutdown(true);
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

impl DeepgramAgentSession {
    /// The `request_id` of the `Welcome` that opened the session.
    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    /// The assistant's words of a response, as `ConversationText` gave them.
    pub fn final_transcript(&self, response_id: ResponseId) -> Option<&str> {
        self.transcripts.get(&response_id).map(String::as_str)
    }

    /// The latencies the `AgentStartedSpeaking` of a response reported.
    pub fn latency(&self, response_id: ResponseId) -> Option<Latency> {
        self.latencies.get(&response_id).copied()
    }

    /// How many binary frames were dropped as part of a reply a barge-in cut.
    pub fn late_audio_dropped(&self) -> u64 {
        self.late_audio
    }

    /// The warnings the server sent, the first [`WARNINGS_KEPT`] of them.
    pub fn warnings(&self) -> &[AgentWarning] {
        &self.warnings
    }

    /// How many warnings the server sent in all.
    pub fn warnings_seen(&self) -> u64 {
        self.warnings_seen
    }

    /// How many bytes of assistant words went past [`agent::TEXT_LIMIT`], held before a reply or
    /// said in one, and were neither kept nor delivered.
    pub fn text_dropped(&self) -> u64 {
        self.text_dropped
    }

    /// Why the session failed, once it did.
    pub fn failure_cause(&self) -> Option<&DeepgramError> {
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
    fn fail_with(&mut self, error: DeepgramError) {
        if self.closing || self.require_open().is_err() {
            return;
        }
        let kind = match &error {
            DeepgramError::Closed { .. } | DeepgramError::Transport(_) => {
                SessionFailureKind::TransportClosed
            }
            DeepgramError::Stalled(_) => SessionFailureKind::Stalled,
            _ => SessionFailureKind::ProviderError,
        };
        let refused = self.ledger.fail(SessionFailure {
            kind,
            detail: error.to_string(),
        });
        self.or_drop(refused);
        self.failure = Some(error);
        self.end_current();
        self.shutdown(false);
    }

    fn end_current(&mut self) {
        self.current = None;
        self.carry.clear();
        self.trailing = None;
    }

    /// The response in progress, created when none is; held assistant words go to a new one.
    fn begin(&mut self, now: Instant) -> Option<ResponseId> {
        if let Some(id) = self.current {
            return Some(id);
        }
        let id = ResponseId::generate();
        if let Err(refusal) = self.ledger.response_created(id, now) {
            self.ledger.dropped(refusal);
            return None;
        }
        self.current = Some(id);
        self.emitted = 0;
        self.carry.clear();
        self.awaiting_reply = false;
        if let Some(text) = self.pending_text.take() {
            self.said(id, text, now);
        }
        Some(id)
    }

    /// `text` joined to `kept` with a space, as far as [`agent::TEXT_LIMIT`] allows; the bytes
    /// past it are counted in [`Self::text_dropped`]. What was added is returned.
    fn append_bounded(kept: &mut String, text: &str, dropped: &mut u64) -> String {
        let separator = if kept.is_empty() { "" } else { " " };
        let room = agent::TEXT_LIMIT.saturating_sub(kept.len() + separator.len());
        let mut end = text.len().min(room);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        *dropped = dropped.saturating_add((text.len() - end) as u64);
        if end == 0 {
            return String::new();
        }
        kept.push_str(separator);
        kept.push_str(&text[..end]);
        text[..end].to_owned()
    }

    /// Holds the assistant's words `text` for the next response, within [`agent::TEXT_LIMIT`].
    fn hold(&mut self, text: &str) {
        let held = self.pending_text.get_or_insert_with(String::new);
        Self::append_bounded(held, text, &mut self.text_dropped);
    }

    /// The assistant's words `text` on the response `id`, within [`agent::TEXT_LIMIT`] for the
    /// response.
    fn said(&mut self, id: ResponseId, text: String, now: Instant) {
        let transcript = self.transcripts.entry(id).or_default();
        let added = Self::append_bounded(transcript, &text, &mut self.text_dropped);
        if added.is_empty() {
            return;
        }
        let delivered = self.ledger.output_transcript(id, added, now);
        self.or_drop(delivered);
    }

    /// Completes the response in progress if frames after `AgentAudioDone` opened it.
    fn finish_trailing(&mut self, now: Instant) {
        if self.trailing.is_some() {
            self.complete(now);
        }
    }

    /// When the trailing response completes for want of another frame, if one is in progress.
    fn quiet_due(&self) -> Option<Instant> {
        self.trailing.and_then(|last| last.checked_add(self.quiet))
    }

    /// Completes the trailing response once [`Self::quiet_due`] has passed at `now`.
    fn finish_if_quiet(&mut self, now: Instant) {
        if self.quiet_due().is_some_and(|due| now >= due) {
            self.complete(now);
        }
    }

    /// The reply a think or speak failure warning concerns will not come: the response in
    /// progress fails, its later frames are dropped; with none in progress while a reply to the
    /// user's words is awaited, one is created and fails at once.
    fn reply_failed(&mut self, warning: &AgentWarning, now: Instant) {
        let failure = SessionFailure {
            kind: SessionFailureKind::ProviderError,
            detail: format!(
                "the agent warned {}: {}",
                warning.code.as_deref().unwrap_or_default(),
                warning.description
            ),
        };
        let id = match self.current {
            Some(id) => {
                self.dropping = true;
                Some(id)
            }
            None if self.awaiting_reply => self.begin(now),
            None => None,
        };
        if let Some(id) = id {
            let failed = self.ledger.response_failed(id, failure, now);
            self.or_drop(failed);
            self.end_current();
        }
        self.awaiting_reply = false;
    }

    /// The response in progress completes.
    fn complete(&mut self, now: Instant) {
        if let Some(id) = self.current {
            let done = self.ledger.response_completed(id, now);
            self.or_drop(done);
            self.end_current();
        }
    }

    /// Emits `audio` on the response `id`, whole samples only; a half sample waits for the next
    /// frame.
    fn output(&mut self, id: ResponseId, audio: &[u8], now: Instant) {
        let width = match self.output_format.encoding {
            AudioEncoding::PcmS16le => 2,
            _ => 1,
        };
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

    /// Records one thing the connection task handed over.
    fn record(&mut self, inbound: Inbound, now: Instant) {
        match inbound {
            Inbound::Unreadable(what) => self.ledger.unreadable_frame(what, now),
            Inbound::Ended(error) => self.fail_with(error),
            Inbound::Audio(audio) => {
                self.ledger.heard(now);
                if self.dropping {
                    self.late_audio += 1;
                    return;
                }
                let trails = self.current.is_none() && self.after_done;
                if let Some(id) = self.begin(now) {
                    if trails || self.trailing.is_some() {
                        self.trailing = Some(now);
                    }
                    self.output(id, &audio, now);
                }
            }
            Inbound::Message(message) => {
                self.ledger.heard(now);
                self.message(message, now);
            }
        }
    }

    fn message(&mut self, message: ServerMessage, now: Instant) {
        match message {
            ServerMessage::ConversationText {
                role: Role::User,
                content,
            } => {
                self.finish_trailing(now);
                let item = self.ledger.assign_item();
                let committed = self.ledger.input_committed(item.clone(), now);
                self.or_drop(committed);
                let heard = self.ledger.input_transcribed(item, content, now);
                self.or_drop(heard);
                self.awaiting_reply = self.current.is_none();
            }
            ServerMessage::ConversationText {
                role: Role::Assistant,
                content,
            } => match self.current {
                Some(id) => self.said(id, content, now),
                None => self.hold(&content),
            },
            ServerMessage::UserStartedSpeaking => {
                self.finish_trailing(now);
                let position = SampleClockPosition::new(self.input_samples)
                    .unwrap_or(SampleClockPosition::MAX);
                let started = self.ledger.speech_started(position, now);
                self.or_drop(started);
                self.pending_text = None;
                if let Some(id) = self.current {
                    let cancelled = self.ledger.response_cancelled(id, now);
                    self.or_drop(cancelled);
                    self.dropping = true;
                    self.end_current();
                }
            }
            ServerMessage::AgentStartedSpeaking { latency } => {
                self.dropping = false;
                self.after_done = false;
                if self.current.is_some()
                    && (self.emitted > 0 || !self.carry.is_empty() || self.trailing.is_some())
                {
                    self.complete(now);
                }
                if let Some(id) = self.begin(now) {
                    self.latencies.insert(id, latency);
                }
            }
            ServerMessage::AgentAudioDone => {
                self.dropping = false;
                self.after_done = true;
                self.complete(now);
            }
            ServerMessage::FunctionCallRequest { functions } => {
                for function in functions
                    .into_iter()
                    .filter(|function| function.client_side)
                {
                    let offered = self
                        .ledger
                        .tools()
                        .iter()
                        .any(|tool| tool.name() == function.name);
                    if !offered {
                        self.fail_with(DeepgramError::FunctionNotOffered {
                            name: function.name,
                        });
                        return;
                    }
                    let Some(id) = self.begin(now) else { return };
                    let proposed = self.ledger.tool_call(
                        SpeechCallId::new(function.id.clone()),
                        id,
                        &function.name,
                        function.arguments,
                        now,
                    );
                    self.or_drop(proposed);
                    self.calls.insert(function.id, function.name);
                }
            }
            ServerMessage::Warning(warning) => {
                self.warnings_seen = self.warnings_seen.saturating_add(1);
                if warning
                    .code
                    .as_deref()
                    .is_some_and(|code| REPLY_FAILED.contains(&code))
                {
                    self.reply_failed(&warning, now);
                }
                if self.warnings.len() < WARNINGS_KEPT {
                    self.warnings.push(warning);
                }
            }
            ServerMessage::Error { code, description } => {
                self.fail_with(DeepgramError::Server { code, description });
            }
            ServerMessage::ConversationText { .. }
            | ServerMessage::Welcome { .. }
            | ServerMessage::SettingsApplied
            | ServerMessage::Ignored { .. } => {}
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

    /// Hands `outbound` to the connection task; a task that is gone fails the session with what
    /// it said last.
    fn send(&mut self, outbound: Outbound) -> Result<(), SessionError> {
        let sent = match (self.commands.as_ref(), self.runtime.as_ref()) {
            (Some(commands), Some(runtime)) => runtime.block_on(async {
                tokio::time::timeout(self.write_timeout, commands.send(outbound)).await
            }),
            _ => return self.require_open(),
        };
        match sent {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(_)) => {
                self.record_arrived();
                self.fail_with(DeepgramError::Transport("the connection ended".to_owned()));
            }
            Err(_) => self.fail_with(DeepgramError::Stalled(self.write_timeout)),
        }
        self.require_open()
    }
}

impl SpeechSessionHandle for DeepgramAgentSession {
    fn ledger(&self) -> &SessionLedger {
        &self.ledger
    }

    fn append_audio(&mut self, frame: AudioFrame) -> Result<(), SessionError> {
        let audio = frame.data.clone();
        let samples = u64::from(frame.sample_count);
        self.ledger.accept_input(frame)?;
        self.input_samples = self.input_samples.saturating_add(samples);
        if audio.is_empty() {
            return Ok(());
        }
        self.send(Outbound::Audio(audio))
    }

    fn commit_input(&mut self) -> Result<(), SessionError> {
        let item = self.ledger.assign_item();
        self.ledger.commit_input(item, Instant::now())
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
        let content = match &result {
            Json::String(text) => text.clone(),
            other => other.to_string(),
        };
        self.ledger.answer(call_id, result)?;
        let name = self
            .calls
            .get(call_id.as_str())
            .cloned()
            .unwrap_or_default();
        self.send(Outbound::Text(agent::function_response(
            call_id.as_str(),
            &name,
            &content,
        )))
    }

    /// Closes the session. What already reached the session is recorded first and delivered
    /// after the close, before `SpeechSessionClosed`, except what would fail the session; then
    /// the connection task sends a close frame, waited for at most [`CLOSE_BOUND`] in all.
    fn close(&mut self) -> Result<(), SessionError> {
        if self.require_open().is_ok() {
            self.closing = true;
            self.record_arrived();
            self.finish_trailing(Instant::now());
            self.shutdown(true);
        }
        self.ledger.close()?;
        self.end_current();
        Ok(())
    }

    fn next_event(&mut self, wait: Duration) -> Option<SessionEvent> {
        let deadline = Instant::now().checked_add(wait);
        loop {
            if let Some(event) = self.ledger.next_event() {
                return Some(event);
            }
            self.finish_if_quiet(Instant::now());
            if let Some(event) = self.ledger.next_event() {
                return Some(event);
            }
            let mut left = deadline.map_or(wait, |deadline| {
                deadline.saturating_duration_since(Instant::now())
            });
            if let Some(due) = self.quiet_due() {
                left = left.min(due.saturating_duration_since(Instant::now()));
            }
            if left.is_zero() || self.require_open().is_err() {
                self.ledger.check_liveness(Instant::now());
                return self.ledger.next_event();
            }
            let runtime = self.runtime.as_ref()?;
            let inbound = &mut self.inbound;
            let read = runtime.block_on(async { tokio::time::timeout(left, inbound.recv()).await });
            match read {
                Ok(Some(inbound)) => self.record(inbound, Instant::now()),
                Ok(None) => self.fail_with(DeepgramError::Closed {
                    code: None,
                    reason: "the connection task ended".to_owned(),
                    cause: crate::error::CloseCause::Other,
                }),
                Err(_) => {
                    self.finish_if_quiet(Instant::now());
                    self.ledger.check_liveness(Instant::now());
                }
            }
        }
    }
}
