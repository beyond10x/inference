//! A Phonic conversation as an `inference-duplex` native speech model.
//!
//! [`PhonicSpeech`] opens a conversation over the WebSocket of [`crate::protocol`] and records
//! what Phonic sends through the contract's [`SessionLedger`], so it refuses what every
//! implementation refuses and yields the contract's events in order.
//!
//! # Opening
//!
//! [`PhonicSpeech::new`] refuses a base URL or a `config` name before anything connects.
//! [`NativeSpeechModel::open`] admits the configuration (a format Phonic does not take is
//! refused there as `Unsupported::InputFormat` or `Unsupported::OutputFormat`), resolves the
//! key, connects within [`PhonicEndpoint::connect_timeout`], sends `config` and waits within
//! [`PhonicEndpoint::ready_timeout`] for `ready_to_start_conversation`; no audio goes out before
//! it. The session's formats, voice (`voice_id`), instructions (`system_prompt`) and tools go in
//! `config`. Every way an open fails leaves the reason in [`PhonicSpeech::open_error`].
//!
//! # The connection
//!
//! Each session runs a runtime of one worker thread, on which one task owns the socket: it reads
//! every message, writes what the session sends, pings after [`PhonicEndpoint::ping_interval`]
//! without reading anything, and hands every message it read to the session through a queue of
//! [`INBOUND_QUEUE`] messages; while that queue is full it reads nothing more and counts no
//! stall. The session's writes go through a queue of [`OUTBOUND_QUEUE`] messages, each send
//! waiting at most [`PhonicEndpoint::read_timeout`]. A message or frame larger than
//! [`MAX_MESSAGE_BYTES`] fails the session as a provider error ([`PhonicError::Oversize`]), and
//! a connection that sends nothing, not even a pong, for [`PhonicEndpoint::read_timeout`] fails
//! it as stalled. Closing or dropping the session sends a close frame and waits at most
//! [`CLOSE_BOUND`] in all for it; dropping one inside an async runtime does not wait, and the
//! connection closes with the runtime. Drive a session from a blocking thread, never from inside
//! an async runtime: each operation blocks on the session's runtime.
//!
//! # The mapping onto the contract
//!
//! * Phonic's turn-taking is its own: [`TurnDetection::ServerVad`] is the only mode, with no
//!   tuning; `auto_response` and `auto_interrupt` must be on. Commit, response creation,
//!   cancellation and truncation by the caller are refused as [`Unsupported`](inference_duplex::Unsupported).
//! * Input frames go out as `audio_chunk` messages of at most [`crate::protocol::CHUNK_MS`].
//! * `user_started_speaking` and `user_finished_speaking` are `SpeechStarted` and
//!   `SpeechStopped` at the input sent so far; `input_text` is the end of a user turn:
//!   `InputCommitted`, then `InputTranscribed`.
//! * A response begins at `assistant_started_speaking` (completing one still in progress), or at
//!   an `audio_chunk` or a `tool_call` while none is in progress. Its audio frames count samples
//!   from its start; PCM ending in half a sample keeps that byte for the next chunk. The `text`
//!   of each `audio_chunk` is its `OutputTranscriptDelta`. `assistant_finished_speaking` and
//!   `assistant_ended_conversation` complete it.
//! * Barge-in: `interrupted_response` cancels the response in progress (`ResponseCancelled`),
//!   and its text, what was spoken before the cut, becomes that response's final transcript
//!   ([`PhonicSession::final_transcript`]). Audio after it, up to the next
//!   `assistant_started_speaking`, belongs to the cut response and is dropped
//!   ([`PhonicSession::late_audio_dropped`]). That rule is inferred ([`crate::protocol`]).
//! * A `tool_call` of a tool the session offers is proposed on the response in progress
//!   (`SpeechToolCallProposed`); [`SpeechSessionHandle::submit_tool_result`] sends
//!   `tool_call_output` with the result as it is. A call of a tool the session does not offer
//!   fails the session as a provider error naming it ([`PhonicError::ToolNotOffered`]).
//! * `error` fails the session as a provider error ([`PhonicError::Server`]); a close by the
//!   server or a failed read fails it as a closed transport ([`PhonicError::Closed`],
//!   [`PhonicError::Transport`]), the close code kept. [`PhonicSession::failure_cause`] keeps
//!   the reason. `warning` is counted ([`PhonicSession::warnings`]).
//! * `ready_to_start_conversation`, `conversation_created`, `input_cancelled`, `dtmf`,
//!   `tool_call_output_processed`, `tool_call_interrupted`, `assistant_chose_not_to_respond` and
//!   types the binding does not know are read and passed over; each is a sign of life. A message
//!   the binding cannot read (not JSON, no `type`, a field it needs missing, a binary frame) is
//!   counted on the session ([`SessionLedger::unreadable_frames`]) and is no sign of life.
//! * Usage: no message carries billing, so each response's usage record carries every count
//!   unknown.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use inference_duplex::conformance::{NativeDuplexEvidence, NoNativeDuplex};
use inference_duplex::inference_audio::{AudioFormat, AudioFrame, SampleClockPosition};
use inference_duplex::inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_duplex::inference_tools::Json;
use inference_duplex::{
    NativeSpeechModel, OpenError, ResponseId, SessionConfig, SessionError, SessionEvent,
    SessionFailure, SessionFailureKind, SessionLedger, SpeechCallId, SpeechCapabilities,
    SpeechSessionHandle, SpeechSessionId, TuningSupport, TurnDetection,
};
use tokio::net::TcpStream;
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{self, Bytes, Message};
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};
use zeroize::Zeroizing;

use crate::credential::{CredentialError, CredentialSource};
use crate::protocol::{
    self, MAX_MESSAGE_BYTES, PhonicConfig, PhonicEndpoint, PhonicError, ServerMessage,
};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The backend an adapter record of the binding names.
pub const BACKEND: &str = "phonic";

/// The most a cancel or an orderly close waits for the close frame to be written.
pub const CLOSE_BOUND: Duration = Duration::from_secs(1);

/// How many server messages the connection task holds for the session before it stops reading.
pub const INBOUND_QUEUE: usize = 256;

/// How many messages the session holds for the connection task to write.
pub const OUTBOUND_QUEUE: usize = 64;

/// What Phonic takes, as the duplex contract states it: the five documented formats each way;
/// its own turn-taking as `server_vad` without tuning; responses and interruption by Phonic
/// alone; no cancellation or truncation by the caller; client tools.
pub fn capabilities() -> SpeechCapabilities {
    SpeechCapabilities {
        input_formats: protocol::audio_formats(),
        output_formats: protocol::audio_formats(),
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
/// `records/claims-duplex.json`.
///
/// Not `native_duplex`: `conformance::native_duplex_evidence` over the fixture server, which
/// interrupts a response on speech over it as the documentation describes, ends in
/// [`NoNativeDuplex::Restarted`], the interrupted response cancelled and the answer in a new one.
/// Not `interruptible`: the caller cannot cancel a response.
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

/// Phonic as a native speech model.
pub struct PhonicSpeech {
    adapter: Adapter,
    url: String,
    endpoint: PhonicEndpoint,
    config: PhonicConfig,
    source: Arc<dyn CredentialSource>,
    key_name: String,
    capabilities: SpeechCapabilities,
    open_error: Option<PhonicError>,
}

impl std::fmt::Debug for PhonicSpeech {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PhonicSpeech")
            .field("adapter", &self.adapter.adapter_id())
            .field("url", &self.url)
            .field("config", &self.config)
            .field("key_name", &self.key_name)
            .field("open_error", &self.open_error)
            .finish_non_exhaustive()
    }
}

impl PhonicSpeech {
    /// Phonic at `endpoint` with `config`, its key the secret `key_name` of `source`, under an
    /// adapter record of its own claiming [`claims`].
    ///
    /// # Errors
    /// [`PhonicError::InvalidUrl`] ([`protocol::url`]) or [`PhonicError::InvalidConfig`]; nothing
    /// connects.
    pub fn new(
        endpoint: PhonicEndpoint,
        config: PhonicConfig,
        source: Arc<dyn CredentialSource>,
        key_name: impl Into<String>,
    ) -> Result<Self, PhonicError> {
        let url = protocol::url(&endpoint)?;
        config.check()?;
        let adapter = Adapter::declare(DeclareAdapter {
            model_id: ModelId::generate(),
            backend: BACKEND.to_owned(),
            contract_revision: "1".to_owned(),
            execution_form: ExecutionForm::HostedService,
            locality: Locality::ExternalProvider,
            capabilities: claims(),
            max_concurrency: 1,
        })
        .map(|(adapter, _)| adapter)
        .unwrap_or_else(|_| unreachable!("a concurrency of one is positive"));
        Ok(Self {
            adapter,
            url,
            endpoint,
            config,
            source,
            key_name: key_name.into(),
            capabilities: capabilities(),
            open_error: None,
        })
    }

    /// Phonic at `endpoint` with `config`, its key from `PHONIC_API_KEY`.
    ///
    /// # Errors
    /// As [`Self::new`].
    pub fn from_env(endpoint: PhonicEndpoint, config: PhonicConfig) -> Result<Self, PhonicError> {
        Self::new(
            endpoint,
            config,
            Arc::new(crate::EnvironmentSource::phonic()),
            crate::PHONIC_KEY_NAME,
        )
    }

    /// This model under an adapter record declared elsewhere, such as a model store's.
    pub fn with_adapter(mut self, adapter: Adapter) -> Self {
        self.adapter = adapter;
        self
    }

    /// Why the latest open failed; `None` after an open that succeeded or before any.
    pub fn open_error(&self) -> Option<&PhonicError> {
        self.open_error.as_ref()
    }

    /// `error` as the open's failure, kept in [`Self::open_error`].
    fn failed(&mut self, error: PhonicError) -> OpenError {
        let failure = SessionFailure {
            kind: open_failure_kind(&error),
            detail: error.to_string(),
        };
        self.open_error = Some(error);
        OpenError::Failed(failure)
    }
}

fn open_failure_kind(error: &PhonicError) -> SessionFailureKind {
    match error {
        PhonicError::Connect {
            status: Some(401 | 403),
            ..
        }
        | PhonicError::Credential(_) => SessionFailureKind::Rejected,
        PhonicError::Stalled(_) => SessionFailureKind::Stalled,
        PhonicError::Server { .. }
        | PhonicError::Oversize(_)
        | PhonicError::InvalidConfig(_)
        | PhonicError::ToolNotOffered { .. } => SessionFailureKind::ProviderError,
        _ => SessionFailureKind::TransportClosed,
    }
}

impl NativeSpeechModel for PhonicSpeech {
    type Session = PhonicSession;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn capabilities(&self) -> &SpeechCapabilities {
        &self.capabilities
    }

    fn open(&mut self, config: SessionConfig) -> Result<PhonicSession, OpenError> {
        self.open_error = None;
        let (mut ledger, _) = SessionLedger::open(
            SpeechSessionId::generate(),
            &self.adapter,
            &self.capabilities,
            config.clone(),
            Instant::now(),
        )?;
        let message = match protocol::config_message(&self.config, &config) {
            Ok(message) => message,
            Err(error) => return Err(self.failed(error)),
        };
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("phonic")
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => return Err(self.failed(PhonicError::Runtime(error.to_string()))),
        };
        let opened = runtime.block_on(connect(
            &self.url,
            &self.endpoint,
            self.source.as_ref(),
            &self.key_name,
            message,
        ));
        let (socket, conversation_id, early) = match opened {
            Ok(opened) => opened,
            Err(error) => {
                runtime.shutdown_background();
                return Err(self.failed(error));
            }
        };
        let (commands, command_rx) = mpsc::channel(OUTBOUND_QUEUE);
        let (inbound_tx, inbound) = mpsc::channel(INBOUND_QUEUE);
        let (stop, stop_rx) = oneshot::channel();
        let pump = runtime.spawn(pump(
            socket,
            command_rx,
            stop_rx,
            inbound_tx,
            Timers::new(&self.endpoint),
        ));
        ledger.heard(Instant::now());
        let mut session = PhonicSession {
            runtime: Some(runtime),
            commands: Some(commands),
            inbound,
            stop: Some(stop),
            pump: Some(pump),
            ledger,
            input_format: config.input_format,
            output_format: config.output_format,
            write_timeout: self.endpoint.read_timeout,
            conversation_id,
            current: None,
            emitted: 0,
            carry: Vec::new(),
            input_samples: 0,
            dropping: false,
            last_response: None,
            transcripts: HashMap::new(),
            late_audio: 0,
            warnings: 0,
            assistant_ended: false,
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

/// The bounds on what the binding reads: [`MAX_MESSAGE_BYTES`] per message and per frame.
fn socket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_MESSAGE_BYTES))
}

/// Resolves the key, connects, sends `config` and waits for `ready_to_start_conversation`, each
/// wait bounded by `endpoint`. The conversation id and what arrived before readiness are
/// returned with the socket. A connection that fails after the handshake is closed before this
/// returns.
async fn connect(
    url: &str,
    endpoint: &PhonicEndpoint,
    source: &dyn CredentialSource,
    key_name: &str,
    config: String,
) -> Result<(Socket, Option<String>, Vec<ServerMessage>), PhonicError> {
    let mut socket = handshake(url, endpoint, source, key_name).await?;
    let ready_timeout = endpoint.ready_timeout;
    let waited = tokio::time::timeout(ready_timeout, await_ready(&mut socket, config))
        .await
        .unwrap_or(Err(PhonicError::Stalled(ready_timeout)));
    match waited {
        Ok((conversation_id, early)) => Ok((socket, conversation_id, early)),
        Err(error) => {
            close_socket(socket).await;
            Err(error)
        }
    }
}

/// The key resolved, then the TCP connection, TLS for `wss://` and the WebSocket handshake with
/// `Authorization: Bearer <key>`, marked sensitive, within the connect timeout. A blank key is
/// refused as invalid; the key is in no error.
async fn handshake(
    url: &str,
    endpoint: &PhonicEndpoint,
    source: &dyn CredentialSource,
    key_name: &str,
) -> Result<Socket, PhonicError> {
    let resolved = source
        .resolve(key_name)
        .await
        .map_err(PhonicError::Credential)?;
    if resolved.expose().iter().all(u8::is_ascii_whitespace) {
        return Err(PhonicError::Credential(CredentialError::Invalid));
    }
    let mut value = Zeroizing::new(b"Bearer ".to_vec());
    value.extend_from_slice(resolved.expose());
    drop(resolved);
    let mut header = HeaderValue::from_bytes(&value)
        .map_err(|_| PhonicError::Credential(CredentialError::Invalid))?;
    header.set_sensitive(true);
    let mut request = url
        .into_client_request()
        .map_err(|error| PhonicError::Connect {
            status: None,
            detail: error.to_string(),
        })?;
    request.headers_mut().insert("authorization", header);
    let connector = if url.starts_with("wss://") {
        Some(Connector::Rustls(tls_config()?))
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
        Err(_) => Err(PhonicError::Connect {
            status: None,
            detail: format!("no handshake within {:?}", endpoint.connect_timeout),
        }),
        Ok(Err(error)) => Err(handshake_error(&error)),
        Ok(Ok((socket, _))) => Ok(socket),
    }
}

/// TLS with the ring provider and the webpki roots, independent of any process-wide default
/// provider.
fn tls_config() -> Result<Arc<rustls::ClientConfig>, PhonicError> {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|error| PhonicError::Connect {
        status: None,
        detail: error.to_string(),
    })?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(Arc::new(config))
}

/// The handshake failure of `error`, with the HTTP status when the server answered the upgrade
/// with one. The description is tungstenite's, which names the status and never the request.
fn handshake_error(error: &tungstenite::Error) -> PhonicError {
    let status = match error {
        tungstenite::Error::Http(response) => Some(response.status().as_u16()),
        _ => None,
    };
    PhonicError::Connect {
        status,
        detail: error.to_string(),
    }
}

/// Sends `config`, then reads until `ready_to_start_conversation`; an `error` or a close ends
/// the wait. The conversation id is kept, and what else arrives, at most [`INBOUND_QUEUE`]
/// messages.
async fn await_ready(
    socket: &mut Socket,
    config: String,
) -> Result<(Option<String>, Vec<ServerMessage>), PhonicError> {
    socket
        .send(Message::text(config))
        .await
        .map_err(|error| PhonicError::Transport(error.to_string()))?;
    let mut conversation_id = None;
    let mut early = Vec::new();
    loop {
        let text = match next_frame(socket).await {
            Frame::Text(text) => text,
            Frame::Binary(_) | Frame::Control => continue,
            Frame::Closed { code, reason } => return Err(PhonicError::Closed { code, reason }),
            Frame::Failed(error) => return Err(error),
        };
        match protocol::parse(&text) {
            Ok(ServerMessage::Ready) => return Ok((conversation_id, early)),
            Ok(ServerMessage::ConversationCreated {
                conversation_id: id,
            }) => conversation_id = Some(id),
            Ok(ServerMessage::Error { code, message }) => {
                return Err(PhonicError::Server { code, message });
            }
            Ok(message) if early.len() < INBOUND_QUEUE => early.push(message),
            Ok(_) | Err(_) => {}
        }
    }
}

/// Writes a close frame, waiting at most [`CLOSE_BOUND`], and drops the connection.
async fn close_socket(mut socket: Socket) {
    let _ = tokio::time::timeout(CLOSE_BOUND, socket.close(None)).await;
}

/// One frame the connection read.
enum Frame {
    Text(String),
    Binary(usize),
    /// A WebSocket ping or pong: a sign of life, nothing to hand over.
    Control,
    Closed {
        code: Option<u16>,
        reason: String,
    },
    Failed(PhonicError),
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
        Message::Binary(data) => Frame::Binary(data.len()),
        Message::Close(frame) => Frame::Closed {
            code: frame.as_ref().map(|frame| u16::from(frame.code)),
            reason: frame
                .map(|frame| frame.reason.as_str().to_owned())
                .unwrap_or_default(),
        },
        Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => Frame::Control,
    }
}

fn read_error(error: tungstenite::Error) -> PhonicError {
    match error {
        tungstenite::Error::Capacity(capacity) => PhonicError::Oversize(capacity.to_string()),
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => {
            PhonicError::Closed {
                code: None,
                reason: String::new(),
            }
        }
        other => PhonicError::Transport(other.to_string()),
    }
}

/// What the connection task hands the session.
enum Inbound {
    Message(ServerMessage),
    /// A message the binding could not read, and what it was.
    Unreadable(String),
    /// The connection ended, and why.
    Ended(PhonicError),
}

/// The connection task's clocks: when it last read anything and last pinged.
struct Timers {
    ping_interval: Duration,
    read_timeout: Duration,
    pinged: tokio::time::Instant,
    read: tokio::time::Instant,
}

impl Timers {
    fn new(endpoint: &PhonicEndpoint) -> Self {
        let now = tokio::time::Instant::now();
        Self {
            ping_interval: endpoint.ping_interval,
            read_timeout: endpoint.read_timeout,
            pinged: now,
            read: now,
        }
    }

    /// When something is next due: a ping, or, while reading, the stall.
    fn next(&self, reading: bool) -> tokio::time::Instant {
        let ping = self.pinged.max(self.read) + self.ping_interval;
        if reading {
            ping.min(self.read + self.read_timeout)
        } else {
            ping
        }
    }

    /// Pings when due; a stall, when `reading` and nothing was read within the read timeout.
    async fn fire(&mut self, socket: &mut Socket, reading: bool) -> Result<(), PhonicError> {
        let now = tokio::time::Instant::now();
        if reading && now >= self.read + self.read_timeout {
            return Err(PhonicError::Stalled(self.read_timeout));
        }
        if now >= self.pinged.max(self.read) + self.ping_interval {
            write(socket, Message::Ping(Bytes::new()), self.read_timeout).await?;
            self.pinged = now;
        }
        Ok(())
    }
}

/// What woke the connection task while it read.
enum Woken {
    Stop,
    Command(Option<String>),
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
/// writes what the session sends, pings when due, and obeys a stop.
async fn hand_over(
    socket: &mut Socket,
    commands: &mut mpsc::Receiver<String>,
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
                Some(text) => write(socket, Message::text(text), timers.read_timeout).await,
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
/// or drops its queue, or the connection ends. Each message is read before a slot of the inbound
/// queue is reserved for it; while no slot is free the task reads nothing more, and its timers
/// count no stall.
async fn pump(
    mut socket: Socket,
    mut commands: mpsc::Receiver<String>,
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
            Woken::Command(Some(text)) => {
                if let Err(error) =
                    write(&mut socket, Message::text(text), timers.read_timeout).await
                {
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
                    Frame::Text(text) => match protocol::parse(&text) {
                        Ok(message) => (Inbound::Message(message), false),
                        Err(what) => (Inbound::Unreadable(what), false),
                    },
                    Frame::Binary(bytes) => (
                        Inbound::Unreadable(format!(
                            "a binary message of {bytes} bytes; Phonic sends text"
                        )),
                        false,
                    ),
                    Frame::Control => continue,
                    Frame::Closed { code, reason } => {
                        (Inbound::Ended(PhonicError::Closed { code, reason }), true)
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
    close_socket(socket).await;
}

/// Writes `message`, waiting at most `bound`.
async fn write(socket: &mut Socket, message: Message, bound: Duration) -> Result<(), PhonicError> {
    match tokio::time::timeout(bound, socket.send(message)).await {
        Err(_) => Err(PhonicError::Stalled(bound)),
        Ok(Err(error)) => Err(PhonicError::Transport(error.to_string())),
        Ok(Ok(())) => Ok(()),
    }
}

/// One open conversation through the contract. It owns the runtime its connection runs on;
/// dropping it closes the connection, waiting at most [`CLOSE_BOUND`].
pub struct PhonicSession {
    runtime: Option<Runtime>,
    commands: Option<mpsc::Sender<String>>,
    inbound: mpsc::Receiver<Inbound>,
    stop: Option<oneshot::Sender<()>>,
    pump: Option<JoinHandle<()>>,
    ledger: SessionLedger,
    input_format: AudioFormat,
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
    /// An interruption cut the latest response: its audio is dropped until the next
    /// `assistant_started_speaking`.
    dropping: bool,
    last_response: Option<ResponseId>,
    transcripts: HashMap<ResponseId, String>,
    late_audio: u64,
    warnings: u64,
    assistant_ended: bool,
    failure: Option<PhonicError>,
    /// The caller is closing: what would fail the session is not recorded.
    closing: bool,
}

impl std::fmt::Debug for PhonicSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PhonicSession")
            .field("session", self.ledger.session())
            .field("conversation_id", &self.conversation_id)
            .field("current", &self.current)
            .field("failure", &self.failure)
            .finish_non_exhaustive()
    }
}

impl Drop for PhonicSession {
    fn drop(&mut self) {
        self.shutdown(true);
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

impl PhonicSession {
    /// The conversation id `conversation_created` named.
    pub fn conversation_id(&self) -> Option<&str> {
        self.conversation_id.as_deref()
    }

    /// The final transcript of a response: the text of its audio chunks, or, for a response an
    /// interruption cut, what `interrupted_response` says was spoken before the cut.
    pub fn final_transcript(&self, response_id: ResponseId) -> Option<&str> {
        self.transcripts.get(&response_id).map(String::as_str)
    }

    /// How many audio chunks were dropped as part of a response an interruption cut.
    pub fn late_audio_dropped(&self) -> u64 {
        self.late_audio
    }

    /// How many `warning` messages arrived.
    pub fn warnings(&self) -> u64 {
        self.warnings
    }

    /// Whether `assistant_ended_conversation` arrived.
    pub fn assistant_ended(&self) -> bool {
        self.assistant_ended
    }

    /// Why the session failed, once it did.
    pub fn failure_cause(&self) -> Option<&PhonicError> {
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

    /// The input sent so far, as a position of the input clock.
    fn input_position(&self) -> SampleClockPosition {
        SampleClockPosition::new(self.input_samples).unwrap_or(SampleClockPosition::MAX)
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
    fn fail_with(&mut self, error: PhonicError) {
        if self.closing || self.require_open().is_err() {
            return;
        }
        let kind = match &error {
            PhonicError::Closed { .. } | PhonicError::Transport(_) => {
                SessionFailureKind::TransportClosed
            }
            PhonicError::Stalled(_) => SessionFailureKind::Stalled,
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
    }

    /// Completes the response in progress, if one is.
    fn complete(&mut self, now: Instant) {
        if let Some(id) = self.current {
            let done = self.ledger.response_completed(id, now);
            self.or_drop(done);
        }
        self.end_current();
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
        let width = protocol::sample_width(self.output_format);
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
            ServerMessage::AssistantStartedSpeaking => {
                self.complete(now);
                self.dropping = false;
                self.begin(now);
            }
            ServerMessage::AudioChunk { audio, text } => {
                if self.dropping {
                    self.late_audio += 1;
                    return;
                }
                let Some(id) = self.begin(now) else { return };
                self.output(id, audio, now);
                if !text.is_empty() {
                    self.transcripts.entry(id).or_default().push_str(&text);
                    let said = self.ledger.output_transcript(id, text, now);
                    self.or_drop(said);
                }
            }
            ServerMessage::AssistantFinishedSpeaking => self.complete(now),
            ServerMessage::AssistantEnded => {
                self.assistant_ended = true;
                self.complete(now);
            }
            ServerMessage::UserStartedSpeaking => {
                let started = self.ledger.speech_started(self.input_position(), now);
                self.or_drop(started);
            }
            ServerMessage::UserFinishedSpeaking => {
                let stopped = self.ledger.speech_stopped(self.input_position(), now);
                self.or_drop(stopped);
            }
            ServerMessage::InterruptedResponse { text } => {
                let Some(id) = self.current.or(self.last_response) else {
                    return;
                };
                self.transcripts.insert(id, text);
                if self.current == Some(id) {
                    let cancelled = self.ledger.response_cancelled(id, now);
                    self.or_drop(cancelled);
                    self.end_current();
                    self.dropping = true;
                }
            }
            ServerMessage::InputText { text, .. } => {
                let committed = self.ledger.input_committed(now);
                self.or_drop(committed);
                let heard = self.ledger.input_transcribed(text, now);
                self.or_drop(heard);
            }
            ServerMessage::ToolCall {
                tool_call_id,
                tool_name,
                parameters,
            } => {
                let offered = self
                    .ledger
                    .tools()
                    .iter()
                    .any(|tool| tool.name() == tool_name);
                if !offered {
                    self.fail_with(PhonicError::ToolNotOffered { name: tool_name });
                    return;
                }
                self.dropping = false;
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
            ServerMessage::Error { code, message } => {
                self.fail_with(PhonicError::Server { code, message });
            }
            ServerMessage::Warning { .. } => self.warnings += 1,
            ServerMessage::ConversationCreated { conversation_id } => {
                self.conversation_id = Some(conversation_id);
            }
            ServerMessage::Ready
            | ServerMessage::InputCancelled
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
                self.fail_with(PhonicError::Transport("the connection ended".to_owned()));
            }
            Err(_) => self.fail_with(PhonicError::Stalled(self.write_timeout)),
        }
        self.require_open()
    }
}

impl SpeechSessionHandle for PhonicSession {
    fn ledger(&self) -> &SessionLedger {
        &self.ledger
    }

    fn append_audio(&mut self, frame: AudioFrame) -> Result<(), SessionError> {
        let chunks = protocol::audio_chunks(&frame.data, self.input_format);
        let samples = u64::from(frame.sample_count);
        self.ledger.accept_input(frame)?;
        self.input_samples = self.input_samples.saturating_add(samples);
        for chunk in chunks {
            self.send(chunk)?;
        }
        Ok(())
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
        let message = protocol::tool_output(call_id.as_str(), &result);
        self.ledger.answer(call_id, result)?;
        self.send(message)
    }

    /// Closes the conversation. What already reached the session is recorded first and
    /// delivered after the close, before `SpeechSessionClosed`, except what would fail the
    /// session; then the connection task sends a close frame, waited for at most
    /// [`CLOSE_BOUND`] in all.
    fn close(&mut self) -> Result<(), SessionError> {
        if self.require_open().is_ok() {
            self.closing = true;
            self.record_arrived();
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
                Ok(None) => self.fail_with(PhonicError::Closed {
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
