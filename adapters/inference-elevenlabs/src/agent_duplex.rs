//! An ElevenLabs agent conversation as an `inference-duplex` native speech model.
//!
//! [`ElevenLabsAgent`] opens a conversation with one agent over the WebSocket of
//! [`crate::agent`] and records what the agent sends through the contract's [`SessionLedger`],
//! so it refuses what every implementation refuses and yields the contract's events in order.
//!
//! # Opening
//!
//! A public agent is named by its id ([`ElevenLabsAgent::public`]), on the host of an
//! [`AgentEndpoint`]; a private one by a [`SignedUrl`] the caller fetched
//! ([`ElevenLabsAgent::signed`]). An id or URL the binding refuses is refused there, before
//! anything connects. [`NativeSpeechModel::open`] admits the configuration, connects within
//! [`AgentEndpoint::connect_timeout`], sends `conversation_initiation_client_data` and waits
//! within [`AgentEndpoint::metadata_timeout`] for `conversation_initiation_metadata`, answering
//! pings meanwhile. Both waits race [`OpenCanceller::cancel`]: a cancel drops a handshake in
//! progress, which closes the TCP connection at once, or sends a close frame, bounded by
//! [`CLOSE_BOUND`], once the WebSocket is open. A cancel reaches only an open in progress when
//! it is issued; one issued while none is does nothing. The agent's configuration chooses its audio
//! formats and a session cannot; the session configuration states the formats it expects, and
//! an agent reporting others fails the open as [`Unsupported::InputFormat`] or
//! [`Unsupported::OutputFormat`], [`ElevenLabsAgent::open_error`] naming what it reported. No
//! audio is resampled. Every way an open fails leaves the reason in
//! [`ElevenLabsAgent::open_error`]. The voice and instructions of the configuration are not
//! sent: the agent's own apply.
//!
//! # The connection
//!
//! Each session runs a runtime of one worker thread, on which one task owns the socket: it
//! reads every message, answers each `ping` at once with a `pong` carrying its `event_id`
//! (whether or not the caller is reading; a `ping` takes no slot of the queue below), writes
//! what the session sends, and hands every other message it read to the session through a
//! queue of [`INBOUND_QUEUE`] messages, reading it before it waits for a slot; while that queue
//! is full it reads nothing more. The session's writes go through a queue of [`OUTBOUND_QUEUE`]
//! messages, each send waiting at most [`AgentEndpoint::read_timeout`]. A message or frame
//! larger than [`MAX_MESSAGE_BYTES`] fails the session as a provider error
//! ([`AgentError::Oversize`]), and a connection that sends nothing for
//! [`AgentEndpoint::read_timeout`] fails it as stalled. Closing or dropping the session sends a
//! close frame and waits at most [`CLOSE_BOUND`] in all for it; dropping one inside an async
//! runtime does not wait, and the connection closes with the runtime. Drive a session from a
//! blocking thread, never from inside an async runtime: each operation blocks on the session's
//! runtime.
//!
//! # The mapping onto the contract
//!
//! * The agent's turn-taking is its own: [`TurnDetection::ServerVad`] is the only mode, with no
//!   tuning; `auto_response` and `auto_interrupt` must be on. Commit, response creation,
//!   cancellation and truncation by the caller are refused as [`Unsupported`]: the protocol has
//!   no message for them.
//! * A `user_transcript` is the end of a user turn: `InputCommitted`, then `InputTranscribed`.
//! * A response begins with the first `audio` while none is in progress, or with an
//!   `agent_response` or a `client_tool_call` while none is. Its audio frames count samples from
//!   its start; PCM ending in half a sample keeps that byte for the next chunk. An
//!   `agent_response` gives the response its `OutputTranscriptDelta`, the whole text of the
//!   message, and leaves it in progress, since audio of the message may still follow it. The
//!   response completes (`ResponseCompleted`) at the first of: the user's next turn
//!   (`user_transcript`), audio whose `event_id` is greater than that `agent_response`'s (a
//!   new message, which opens its own response), the next `agent_response`, a close by the
//!   caller, or [`MESSAGE_QUIET`] (1 s, or half the idle limit when that is shorter) with no
//!   audio of it. Audio up to then stays on it, so none is dropped and no stray response
//!   opens; and a silent agent leaves no response in progress, so the idle limit fails no
//!   healthy session. An `interruption` cancels it like any response in progress, since its
//!   audio was cut. This rule is inferred, not stated ([`crate::agent`]). Its `response_id`, where present, is kept as the
//!   provider's name of the response ([`ElevenLabsAgentSession::provider_response_id`]).
//! * An `interruption` is the agent hearing the user over it: `SpeechStarted` at the input sent
//!   so far, then the response in progress ends as cancelled (`ResponseCancelled`). Audio whose
//!   `event_id` is at most the latest interruption's belongs to the cut response and is dropped
//!   ([`ElevenLabsAgentSession::late_audio_dropped`]); later audio opens a new response. That
//!   ordering is inferred, not stated ([`crate::agent`]).
//! * An `agent_response_correction` is the final transcript of the response an interruption cut:
//!   the one whose `response_id` it names, else the latest cut response, else the latest
//!   response. The contract delivers no transcript of a response that ended, so it is kept on
//!   the session ([`ElevenLabsAgentSession::final_transcript`]), which otherwise holds each
//!   response's `agent_response` text.
//! * A `client_tool_call` of a tool the session offers is proposed on the response in progress
//!   (`SpeechToolCallProposed`); [`SpeechSessionHandle::submit_tool_result`] sends
//!   `client_tool_result`, the result as a string (a JSON string as it is, any other value as
//!   its JSON text). A call of a tool the session does not offer fails the session as a
//!   provider error naming the tool ([`AgentError::ToolNotOffered`]) when the agent awaits its
//!   result, and is passed over when it does not.
//! * `client_error` fails the session as a provider error ([`AgentError::Client`]); a close by
//!   the server or a failed read fails it as a closed transport ([`AgentError::Closed`],
//!   [`AgentError::Transport`]). [`ElevenLabsAgentSession::failure_cause`] keeps the reason.
//! * `vad_score`, `context_usage`, `agent_chat_response_part`, a repeated metadata and types the
//!   binding does not know are read and passed over; each is a sign of life. A message the
//!   binding cannot read (not JSON, no `type`, a field it needs missing, a binary frame) is
//!   counted on the session ([`SessionLedger::unreadable_frames`]) and is no sign of life.
//! * Usage: no message carries billing, so no response usage is reported and each response's
//!   usage record carries every count unknown.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use inference_duplex::conformance::{NativeDuplexEvidence, NoNativeDuplex};
use inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_duplex::inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_duplex::inference_tools::Json;
use inference_duplex::{
    NativeSpeechModel, OpenError, ResponseId, SessionConfig, SessionError, SessionEvent,
    SessionFailure, SessionFailureKind, SessionLedger, SpeechCallId, SpeechCapabilities,
    SpeechSessionHandle, SpeechSessionId, TuningSupport, TurnDetection, Unsupported,
};
use tokio::net::TcpStream;
use tokio::runtime::Runtime;
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};

use crate::agent::{self, AgentEndpoint, AgentError, MAX_MESSAGE_BYTES, ServerMessage, SignedUrl};
use crate::connection::{CLOSE_BOUND, tls_config};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The backend an adapter record of the binding names.
pub const AGENT_BACKEND: &str = "elevenlabs-agent";

/// The model the claims record names: the agent's configuration chooses it, not the binding.
pub const AGENT_MODEL: &str = "agent-configured";

/// How long a response whose `agent_response` arrived waits for more audio of its message
/// before it completes, unless half the session's idle limit is shorter.
pub const MESSAGE_QUIET: Duration = Duration::from_secs(1);

/// How many server messages the connection task holds for the session before it stops reading.
pub const INBOUND_QUEUE: usize = 256;

/// How many messages the session holds for the connection task to write.
pub const OUTBOUND_QUEUE: usize = 64;

/// What an agent takes, as the duplex contract states it: the seven formats an agent may
/// report, each way; its own turn-taking as `server_vad` without tuning; responses and
/// interruption by the agent alone; no cancellation or truncation by the caller; client tools.
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
/// `records/claims-agent.json`.
///
/// Not `native_duplex`: `conformance::native_duplex_evidence` over the fixture server, which
/// interrupts a response on speech over it as the documentation describes, ends in
/// [`NoNativeDuplex::Restarted`], the interrupted response cancelled and the answer in a new
/// one. Not `interruptible`: the caller cannot cancel a response, and nothing states a latency
/// for the agent's own interruption.
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
    fn opens(&self) -> std::sync::MutexGuard<'_, (u64, u64)> {
        self.opens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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

/// What a session connects to.
enum Target {
    /// A public agent: the URL naming it.
    Public(String),
    Signed(SignedUrl),
}

impl Target {
    fn url(&self) -> &str {
        match self {
            Self::Public(url) => url,
            Self::Signed(signed) => signed.expose(),
        }
    }

    fn host(&self) -> &str {
        match self {
            Self::Public(url) => url
                .split_once("://")
                .map_or("", |(_, rest)| rest)
                .split(['/', '?'])
                .next()
                .unwrap_or_default(),
            Self::Signed(signed) => signed.host(),
        }
    }

    /// `detail` with nothing of a signed URL in it.
    fn redact(&self, detail: &str) -> String {
        match self {
            Self::Public(_) => detail.to_owned(),
            Self::Signed(signed) => signed.redact(detail),
        }
    }
}

/// An ElevenLabs agent as a native speech model.
pub struct ElevenLabsAgent {
    adapter: Adapter,
    target: Target,
    endpoint: AgentEndpoint,
    capabilities: SpeechCapabilities,
    cancel: Arc<CancelState>,
    generation: u64,
    open_error: Option<AgentError>,
}

impl std::fmt::Debug for ElevenLabsAgent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ElevenLabsAgent")
            .field("adapter", &self.adapter.adapter_id())
            .field("host", &self.target.host())
            .field("signed", &matches!(self.target, Target::Signed(_)))
            .field("open_error", &self.open_error)
            .finish_non_exhaustive()
    }
}

impl ElevenLabsAgent {
    /// The public agent `agent_id` on the host of `endpoint`, under an adapter record of its own
    /// claiming [`agent_claims`].
    ///
    /// # Errors
    /// [`AgentError::InvalidAgentId`] or [`AgentError::InvalidUrl`] ([`agent::public_url`]);
    /// nothing connects.
    pub fn public(
        agent_id: impl Into<String>,
        endpoint: AgentEndpoint,
    ) -> Result<Self, AgentError> {
        let url = agent::public_url(&endpoint, &agent_id.into())?;
        Ok(Self::of(Target::Public(url), endpoint))
    }

    /// The private agent `signed` names, with the bounds of `endpoint`; its URL is ignored.
    pub fn signed(signed: SignedUrl, endpoint: AgentEndpoint) -> Self {
        Self::of(Target::Signed(signed), endpoint)
    }

    fn of(target: Target, endpoint: AgentEndpoint) -> Self {
        let adapter = Adapter::declare(DeclareAdapter {
            model_id: ModelId::generate(),
            backend: AGENT_BACKEND.to_owned(),
            contract_revision: "1".to_owned(),
            execution_form: ExecutionForm::HostedService,
            locality: Locality::ExternalProvider,
            capabilities: agent_claims(),
            max_concurrency: 1,
        })
        .map(|(adapter, _)| adapter)
        .unwrap_or_else(|_| unreachable!("a concurrency of one is positive"));
        Self {
            adapter,
            target,
            endpoint,
            capabilities: agent_capabilities(),
            cancel: Arc::new(CancelState::default()),
            generation: 0,
            open_error: None,
        }
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
    pub fn open_error(&self) -> Option<&AgentError> {
        self.open_error.as_ref()
    }

    /// `error` as the open's failure, kept in [`Self::open_error`].
    fn failed(&mut self, error: AgentError) -> OpenError {
        let kind = match &error {
            AgentError::Connect {
                status: Some(401 | 403),
                ..
            } => SessionFailureKind::Rejected,
            AgentError::Stalled(_) => SessionFailureKind::Stalled,
            AgentError::Client { .. } | AgentError::Oversize(_) => {
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

impl NativeSpeechModel for ElevenLabsAgent {
    type Session = ElevenLabsAgentSession;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn capabilities(&self) -> &SpeechCapabilities {
        &self.capabilities
    }

    fn open(&mut self, config: SessionConfig) -> Result<ElevenLabsAgentSession, OpenError> {
        self.open_error = None;
        let (mut ledger, _) = SessionLedger::open(
            SpeechSessionId::generate(),
            &self.adapter,
            &self.capabilities,
            config.clone(),
            Instant::now(),
        )?;
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("elevenlabs-agent")
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => return Err(self.failed(AgentError::Runtime(error.to_string()))),
        };
        self.generation += 1;
        self.cancel.begin(self.generation);
        let opened = runtime.block_on(connect(
            &self.target,
            &self.endpoint,
            &self.cancel,
            self.generation,
        ));
        self.cancel.end();
        let (socket, metadata, early) = match opened {
            Ok(opened) => opened,
            Err(error) => return Err(self.failed(error)),
        };
        let input = agent::audio_format(&metadata.input);
        let output = agent::audio_format(&metadata.output);
        if input != Some(config.input_format) || output != Some(config.output_format) {
            runtime.block_on(close_socket(socket));
            runtime.shutdown_background();
            let refused = if input == Some(config.input_format) {
                Unsupported::OutputFormat(config.output_format)
            } else {
                Unsupported::InputFormat(config.input_format)
            };
            self.open_error = Some(AgentError::FormatMismatch {
                input: metadata.input,
                output: metadata.output,
            });
            return Err(OpenError::Unsupported(refused));
        }
        let (commands, command_rx) = mpsc::channel(OUTBOUND_QUEUE);
        let (inbound_tx, inbound) = mpsc::channel(INBOUND_QUEUE);
        let (stop, stop_rx) = oneshot::channel();
        let pump = runtime.spawn(pump(
            socket,
            command_rx,
            stop_rx,
            inbound_tx,
            self.endpoint.read_timeout,
        ));
        ledger.heard(Instant::now());
        let mut session = ElevenLabsAgentSession {
            runtime: Some(runtime),
            commands: Some(commands),
            inbound,
            stop: Some(stop),
            pump: Some(pump),
            ledger,
            output_format: config.output_format,
            write_timeout: self.endpoint.read_timeout,
            conversation_id: metadata.conversation_id,
            current: None,
            emitted: 0,
            carry: Vec::new(),
            input_samples: 0,
            interrupted_at: None,
            last_cut: None,
            last_response: None,
            provider_ids: HashMap::new(),
            transcripts: HashMap::new(),
            late_audio: 0,
            finishing: None,
            quiet: MESSAGE_QUIET.min(config.idle_limit / 2),
            failure: None,
            closing: false,
        };
        let now = Instant::now();
        for message in early {
            session.record(Inbound::Message(message), now);
        }
        Ok(session)
    }
}

/// A response whose message's `agent_response` arrived, waiting for the rest of its audio.
struct Finishing {
    event_id: Option<u64>,
    last: Instant,
}

/// What the metadata said.
struct Metadata {
    conversation_id: Option<String>,
    input: String,
    output: String,
}

/// The bounds on what the binding reads: [`MAX_MESSAGE_BYTES`] per message and per frame.
fn socket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_MESSAGE_BYTES))
}

/// Connects to `target`, sends the initiation and waits for the metadata, each wait bounded by
/// `endpoint` and raced against `cancel`. What arrived before the metadata, pings aside, is
/// returned with it. A connection that fails after the handshake is closed before this returns.
async fn connect(
    target: &Target,
    endpoint: &AgentEndpoint,
    cancel: &CancelState,
    generation: u64,
) -> Result<(Socket, Metadata, Vec<ServerMessage>), AgentError> {
    let mut socket = tokio::select! {
        biased;
        () = cancel.cancelled(generation) => return Err(AgentError::Cancelled),
        connected = handshake(target, endpoint) => connected?,
    };
    let metadata_timeout = endpoint.metadata_timeout;
    let waited = tokio::select! {
        biased;
        () = cancel.cancelled(generation) => Err(AgentError::Cancelled),
        waited = tokio::time::timeout(metadata_timeout, await_metadata(&mut socket)) => {
            waited.unwrap_or(Err(AgentError::Stalled(metadata_timeout)))
        }
    };
    match waited {
        Ok((metadata, early)) => Ok((socket, metadata, early)),
        Err(error) => {
            close_socket(socket).await;
            Err(error)
        }
    }
}

/// The TCP connection, TLS for `wss://` and the WebSocket handshake, within the connect timeout.
async fn handshake(target: &Target, endpoint: &AgentEndpoint) -> Result<Socket, AgentError> {
    let request = target
        .url()
        .into_client_request()
        .map_err(|error| AgentError::Connect {
            status: None,
            detail: target.redact(&error.to_string()),
        })?;
    let connector = if target.url().starts_with("wss://") {
        let tls = tls_config().map_err(|error| AgentError::Connect {
            status: None,
            detail: error.to_string(),
        })?;
        Some(Connector::Rustls(tls))
    } else {
        None
    };
    let connecting = tokio_tungstenite::connect_async_tls_with_config(
        request,
        Some(socket_config()),
        true,
        connector,
    );
    match tokio::time::timeout(endpoint.connect_timeout, connecting).await {
        Err(_) => Err(AgentError::Connect {
            status: None,
            detail: format!("no handshake within {:?}", endpoint.connect_timeout),
        }),
        Ok(Err(error)) => Err(handshake_error(&error, target)),
        Ok(Ok((socket, _))) => Ok(socket),
    }
}

/// Sends the initiation, then reads until the metadata, answering pings; a `client_error` or a
/// close ends the wait. What else arrives is kept, at most [`INBOUND_QUEUE`] messages.
async fn await_metadata(socket: &mut Socket) -> Result<(Metadata, Vec<ServerMessage>), AgentError> {
    socket
        .send(Message::text(agent::initiation()))
        .await
        .map_err(|error| AgentError::Transport(error.to_string()))?;
    let mut early = Vec::new();
    loop {
        let text = match next_frame(socket).await {
            Frame::Text(text) => text,
            Frame::Binary(_) => continue,
            Frame::Closed { code, reason } => return Err(AgentError::Closed { code, reason }),
            Frame::Failed(error) => return Err(error),
        };
        match agent::parse(&text) {
            Ok(ServerMessage::Metadata {
                conversation_id,
                input_format,
                output_format,
            }) => {
                let metadata = Metadata {
                    conversation_id,
                    input: input_format,
                    output: output_format,
                };
                return Ok((metadata, early));
            }
            Ok(ServerMessage::Ping { event_id }) => {
                socket
                    .send(Message::text(agent::pong(event_id)))
                    .await
                    .map_err(|error| AgentError::Transport(error.to_string()))?;
            }
            Ok(ServerMessage::ClientError {
                code,
                name,
                message,
            }) => {
                return Err(AgentError::Client {
                    code,
                    name,
                    message,
                });
            }
            Ok(message) if early.len() < INBOUND_QUEUE => early.push(message),
            Ok(_) | Err(_) => {}
        }
    }
}

/// The handshake failure of `error`, with the HTTP status when the server answered the upgrade
/// with one, and nothing of a signed URL.
fn handshake_error(error: &tungstenite::Error, target: &Target) -> AgentError {
    let status = match error {
        tungstenite::Error::Http(response) => Some(response.status().as_u16()),
        _ => None,
    };
    AgentError::Connect {
        status,
        detail: target.redact(&error.to_string()),
    }
}

/// Writes a close frame, waiting at most [`CLOSE_BOUND`], and drops the connection.
async fn close_socket(mut socket: Socket) {
    let _ = tokio::time::timeout(CLOSE_BOUND, socket.close(None)).await;
}

/// One frame the connection read, pings and pongs of the WebSocket itself passed over.
enum Frame {
    Text(String),
    Binary(usize),
    Closed { code: Option<u16>, reason: String },
    Failed(AgentError),
}

async fn next_frame(socket: &mut Socket) -> Frame {
    loop {
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
            Message::Text(text) => return Frame::Text(text.as_str().to_owned()),
            Message::Binary(data) => return Frame::Binary(data.len()),
            Message::Close(frame) => {
                return Frame::Closed {
                    code: frame.as_ref().map(|frame| u16::from(frame.code)),
                    reason: frame
                        .map(|frame| frame.reason.as_str().to_owned())
                        .unwrap_or_default(),
                };
            }
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
        }
    }
}

fn read_error(error: tungstenite::Error) -> AgentError {
    match error {
        tungstenite::Error::Capacity(capacity) => AgentError::Oversize(capacity.to_string()),
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => {
            AgentError::Closed {
                code: None,
                reason: String::new(),
            }
        }
        other => AgentError::Transport(other.to_string()),
    }
}

/// What the connection task hands the session.
enum Inbound {
    Message(ServerMessage),
    /// A message the binding could not read, and what it was.
    Unreadable(String),
    /// The connection ended, and why.
    Ended(AgentError),
}

/// What woke the connection task.
enum Woken {
    Stop,
    Command(Option<String>),
    Frame(Result<Frame, tokio::time::error::Elapsed>),
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
/// writes what the session sends and obeys a stop.
async fn hand_over(
    socket: &mut Socket,
    commands: &mut mpsc::Receiver<String>,
    stop: &mut oneshot::Receiver<()>,
    inbound: &mpsc::Sender<Inbound>,
    item: Inbound,
    read_timeout: Duration,
) -> Handed {
    loop {
        let command = tokio::select! {
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
            command = commands.recv() => command,
        };
        let Some(text) = command else {
            return Handed::Stopped;
        };
        if let Err(error) = write(socket, text, read_timeout).await {
            let _ = inbound.try_send(Inbound::Ended(error));
            return Handed::Failed;
        }
    }
}

/// The connection task: reads, answers pings, writes what the session sends, until the session
/// stops it or drops its queue, or the connection ends. A `ping` is answered as soon as it is
/// read and takes no slot of the inbound queue: the session is told of it only when a slot is
/// free at once, as a sign of life. Every other message is read before a slot is reserved for
/// it, and while none is free the task reads nothing more.
async fn pump(
    mut socket: Socket,
    mut commands: mpsc::Receiver<String>,
    mut stop: oneshot::Receiver<()>,
    inbound: mpsc::Sender<Inbound>,
    read_timeout: Duration,
) {
    let mut last = tokio::time::Instant::now();
    loop {
        let deadline = last + read_timeout;
        let woken = tokio::select! {
            biased;
            _ = &mut stop => Woken::Stop,
            command = commands.recv() => Woken::Command(command),
            frame = tokio::time::timeout_at(deadline, next_frame(&mut socket)) => Woken::Frame(frame),
        };
        let (item, ends) = match woken {
            Woken::Stop | Woken::Command(None) => break,
            Woken::Command(Some(text)) => {
                if let Err(error) = write(&mut socket, text, read_timeout).await {
                    let _ = inbound.try_send(Inbound::Ended(error));
                    return;
                }
                continue;
            }
            Woken::Frame(Err(_)) => (Inbound::Ended(AgentError::Stalled(read_timeout)), true),
            Woken::Frame(Ok(frame)) => {
                last = tokio::time::Instant::now();
                match frame {
                    Frame::Text(text) => match agent::parse(&text) {
                        Ok(ServerMessage::Ping { event_id }) => {
                            if let Err(error) =
                                write(&mut socket, agent::pong(event_id), read_timeout).await
                            {
                                let _ = inbound.try_send(Inbound::Ended(error));
                                return;
                            }
                            let _ = inbound
                                .try_send(Inbound::Message(ServerMessage::Ping { event_id }));
                            continue;
                        }
                        Ok(message) => (Inbound::Message(message), false),
                        Err(what) => (Inbound::Unreadable(what), false),
                    },
                    Frame::Binary(bytes) => (
                        Inbound::Unreadable(format!(
                            "a binary message of {bytes} bytes; the agent sends text"
                        )),
                        false,
                    ),
                    Frame::Closed { code, reason } => {
                        (Inbound::Ended(AgentError::Closed { code, reason }), true)
                    }
                    Frame::Failed(error) => (Inbound::Ended(error), true),
                }
            }
        };
        match hand_over(
            &mut socket,
            &mut commands,
            &mut stop,
            &inbound,
            item,
            read_timeout,
        )
        .await
        {
            Handed::On if !ends => {}
            Handed::On | Handed::Stopped => break,
            Handed::Failed => return,
        }
    }
    close_socket(socket).await;
}

/// Writes `text` as one text message, waiting at most `bound`.
async fn write(socket: &mut Socket, text: String, bound: Duration) -> Result<(), AgentError> {
    match tokio::time::timeout(bound, socket.send(Message::text(text))).await {
        Err(_) => Err(AgentError::Stalled(bound)),
        Ok(Err(error)) => Err(AgentError::Transport(error.to_string())),
        Ok(Ok(())) => Ok(()),
    }
}

/// One open conversation through the contract. It owns the runtime its connection runs on;
/// dropping it closes the connection, waiting at most [`CLOSE_BOUND`].
pub struct ElevenLabsAgentSession {
    runtime: Option<Runtime>,
    commands: Option<mpsc::Sender<String>>,
    inbound: mpsc::Receiver<Inbound>,
    stop: Option<oneshot::Sender<()>>,
    pump: Option<JoinHandle<()>>,
    ledger: SessionLedger,
    output_format: AudioFormat,
    write_timeout: Duration,
    conversation_id: Option<String>,
    /// The response in progress.
    current: Option<ResponseId>,
    /// The samples the response in progress emitted, the position of its next frame.
    emitted: u64,
    /// The half sample the last chunk of the response in progress ended in.
    carry: Vec<u8>,
    /// The input samples sent so far.
    input_samples: u64,
    /// The `event_id` of the latest interruption.
    interrupted_at: Option<u64>,
    last_cut: Option<ResponseId>,
    last_response: Option<ResponseId>,
    provider_ids: HashMap<ResponseId, String>,
    transcripts: HashMap<ResponseId, String>,
    late_audio: u64,
    /// The response in progress whose `agent_response` arrived: its message's `event_id` and
    /// when audio of it, or the `agent_response`, last arrived.
    finishing: Option<Finishing>,
    /// How long a finishing response waits for more audio of its message ([`MESSAGE_QUIET`],
    /// or half the idle limit when that is shorter).
    quiet: Duration,
    failure: Option<AgentError>,
    /// The caller is closing: what would fail the session is not recorded.
    closing: bool,
}

impl std::fmt::Debug for ElevenLabsAgentSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ElevenLabsAgentSession")
            .field("session", self.ledger.session())
            .field("conversation_id", &self.conversation_id)
            .field("current", &self.current)
            .field("failure", &self.failure)
            .finish_non_exhaustive()
    }
}

impl Drop for ElevenLabsAgentSession {
    fn drop(&mut self) {
        self.shutdown(true);
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

impl ElevenLabsAgentSession {
    /// The conversation id the metadata named.
    pub fn conversation_id(&self) -> Option<&str> {
        self.conversation_id.as_deref()
    }

    /// The provider's `response_id` of a response, when its `agent_response` named one.
    pub fn provider_response_id(&self, response_id: ResponseId) -> Option<&str> {
        self.provider_ids.get(&response_id).map(String::as_str)
    }

    /// The final transcript of a response: its `agent_response` text, or the corrected text
    /// once an `agent_response_correction` landed on it.
    pub fn final_transcript(&self, response_id: ResponseId) -> Option<&str> {
        self.transcripts.get(&response_id).map(String::as_str)
    }

    /// How many audio chunks were dropped as part of a response an interruption cut.
    pub fn late_audio_dropped(&self) -> u64 {
        self.late_audio
    }

    /// Why the session failed, once it did.
    pub fn failure_cause(&self) -> Option<&AgentError> {
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
    fn fail_with(&mut self, error: AgentError) {
        if self.closing || self.require_open().is_err() {
            return;
        }
        let kind = match &error {
            AgentError::Closed { .. } | AgentError::Transport(_) => {
                SessionFailureKind::TransportClosed
            }
            AgentError::Stalled(_) => SessionFailureKind::Stalled,
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
        self.finishing = None;
    }

    /// Completes the response whose message finished, if one is in progress.
    fn finish(&mut self, now: Instant) {
        if self.finishing.is_none() {
            return;
        }
        if let Some(id) = self.current {
            let done = self.ledger.response_completed(id, now);
            self.or_drop(done);
        }
        self.end_current();
    }

    /// When the finishing response completes for want of more audio, if one is in progress.
    fn quiet_due(&self) -> Option<Instant> {
        self.finishing
            .as_ref()
            .and_then(|finishing| finishing.last.checked_add(self.quiet))
    }

    /// Completes the finishing response once [`Self::quiet_due`] has passed at `now`.
    fn finish_if_quiet(&mut self, now: Instant) {
        if self.quiet_due().is_some_and(|due| now >= due) {
            self.finish(now);
        }
    }

    /// The response in progress, created when none is.
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
        self.last_response = Some(id);
        Some(id)
    }

    /// Emits `audio` on the response `id`, whole samples only; a half sample waits for the next
    /// chunk.
    fn output(&mut self, id: ResponseId, audio: Vec<u8>, now: Instant) {
        let width = match self.output_format.encoding {
            AudioEncoding::PcmS16le => 2,
            _ => 1,
        };
        let mut data = std::mem::take(&mut self.carry);
        data.extend_from_slice(&audio);
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
            Inbound::Message(message) => {
                self.ledger.heard(now);
                self.message(message, now);
            }
        }
    }

    fn message(&mut self, message: ServerMessage, now: Instant) {
        match message {
            ServerMessage::Audio { audio, event_id } => {
                if let (Some(cut), Some(id)) = (self.interrupted_at, event_id)
                    && id <= cut
                {
                    self.late_audio += 1;
                    return;
                }
                let newer = match (&self.finishing, event_id) {
                    (
                        Some(Finishing {
                            event_id: Some(done),
                            ..
                        }),
                        Some(id),
                    ) => id > *done,
                    _ => false,
                };
                if newer {
                    self.finish(now);
                } else if let Some(finishing) = self.finishing.as_mut() {
                    finishing.last = now;
                }
                if let Some(id) = self.begin(now) {
                    self.output(id, audio, now);
                }
            }
            ServerMessage::UserTranscript { text } => {
                self.finish(now);
                let committed = self.ledger.input_committed(now);
                self.or_drop(committed);
                let heard = self.ledger.input_transcribed(text, now);
                self.or_drop(heard);
            }
            ServerMessage::AgentResponse {
                text,
                event_id,
                response_id,
            } => {
                self.finish(now);
                let Some(id) = self.begin(now) else { return };
                let said = self.ledger.output_transcript(id, text.clone(), now);
                self.or_drop(said);
                self.transcripts.insert(id, text);
                if let Some(provider) = response_id {
                    self.provider_ids.insert(id, provider);
                }
                self.finishing = Some(Finishing {
                    event_id,
                    last: now,
                });
            }
            ServerMessage::AgentResponseCorrection {
                corrected,
                response_id,
                ..
            } => {
                let named = response_id.as_ref().and_then(|provider| {
                    self.provider_ids
                        .iter()
                        .find(|(_, named)| *named == provider)
                        .map(|(id, _)| *id)
                });
                if let Some(id) = named.or(self.last_cut).or(self.last_response) {
                    self.transcripts.insert(id, corrected);
                }
            }
            ServerMessage::Interruption { event_id } => {
                if let Some(event_id) = event_id {
                    self.interrupted_at = Some(
                        self.interrupted_at
                            .map_or(event_id, |cut| cut.max(event_id)),
                    );
                }
                let position = SampleClockPosition::new(self.input_samples)
                    .unwrap_or(SampleClockPosition::MAX);
                let started = self.ledger.speech_started(position, now);
                self.or_drop(started);
                if let Some(id) = self.current {
                    let cancelled = self.ledger.response_cancelled(id, now);
                    self.or_drop(cancelled);
                    self.last_cut = Some(id);
                    self.end_current();
                }
            }
            ServerMessage::ClientToolCall {
                tool_name,
                tool_call_id,
                parameters,
                expects_response,
            } => {
                let offered = self
                    .ledger
                    .tools()
                    .iter()
                    .any(|tool| tool.name() == tool_name);
                if !offered {
                    if expects_response {
                        self.fail_with(AgentError::ToolNotOffered { name: tool_name });
                    }
                    return;
                }
                let Some(id) = self.begin(now) else { return };
                let proposed = self.ledger.tool_call(
                    SpeechCallId::new(tool_call_id),
                    id,
                    &tool_name,
                    parameters,
                    now,
                );
                self.or_drop(proposed);
            }
            ServerMessage::ClientError {
                code,
                name,
                message,
            } => self.fail_with(AgentError::Client {
                code,
                name,
                message,
            }),
            ServerMessage::Metadata { .. }
            | ServerMessage::Ping { .. }
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

    /// Hands `text` to the connection task; a task that is gone fails the session with what it
    /// said last.
    fn send(&mut self, text: String) -> Result<(), SessionError> {
        let sent = match (self.commands.as_ref(), self.runtime.as_ref()) {
            (Some(commands), Some(runtime)) => runtime.block_on(async {
                tokio::time::timeout(self.write_timeout, commands.send(text)).await
            }),
            _ => return self.require_open(),
        };
        match sent {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(_)) => {
                self.record_arrived();
                self.fail_with(AgentError::Transport("the connection ended".to_owned()));
            }
            Err(_) => self.fail_with(AgentError::Stalled(self.write_timeout)),
        }
        self.require_open()
    }
}

impl SpeechSessionHandle for ElevenLabsAgentSession {
    fn ledger(&self) -> &SessionLedger {
        &self.ledger
    }

    fn append_audio(&mut self, frame: AudioFrame) -> Result<(), SessionError> {
        let chunk = agent::audio_chunk(&frame.data);
        let samples = u64::from(frame.sample_count);
        self.ledger.accept_input(frame)?;
        self.input_samples = self.input_samples.saturating_add(samples);
        self.send(chunk)
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
        self.send(agent::tool_result(call_id.as_str(), &text, false))
    }

    /// Closes the conversation. What already reached the session is recorded first and
    /// delivered after the close, before `SpeechSessionClosed`, except what would fail the
    /// session; then the connection task sends a close frame, waited for at most
    /// [`CLOSE_BOUND`] in all.
    fn close(&mut self) -> Result<(), SessionError> {
        if self.require_open().is_ok() {
            self.closing = true;
            self.record_arrived();
            self.finish(Instant::now());
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
                Ok(None) => self.fail_with(AgentError::Closed {
                    code: None,
                    reason: "the connection task ended".to_owned(),
                }),
                Err(_) => {
                    self.finish_if_quiet(Instant::now());
                    self.ledger.check_liveness(Instant::now());
                }
            }
        }
    }
}
