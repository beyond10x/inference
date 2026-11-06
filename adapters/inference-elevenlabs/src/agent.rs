//! The wire protocol of an ElevenLabs agent conversation: where a session connects, what it
//! sends and reads, and what the documentation leaves open.
//!
//! Taken on 2026-11-05 from ElevenLabs' public documentation:
//!
//! - <https://elevenlabs.io/docs/agents-platform/api-reference/agents-platform/websocket>, the
//!   AsyncAPI reference the field names and enum values are read from;
//! - <https://elevenlabs.io/docs/eleven-agents/libraries/web-sockets>;
//! - <https://elevenlabs.io/docs/eleven-agents/customization/realtime-models>;
//! - <https://elevenlabs.io/docs/eleven-agents/customization/events/client-events>;
//! - <https://elevenlabs.io/docs/eleven-agents/customization/events/client-to-server-events>;
//! - <https://elevenlabs.io/docs/eleven-agents/overview>.
//!
//! # What the documentation states
//!
//! - **Endpoint.** `wss://api.elevenlabs.io/v1/convai/conversation?agent_id=<id>`; the same path
//!   on the residency hosts of [`Residency`]. A public agent needs only its id. A private agent
//!   is reached through a signed URL that `GET /v1/convai/conversation/get-signed-url` returns
//!   for an API key; the handshake carries no key. The binding has no HTTP client: the caller
//!   fetches the signed URL and hands it over as a [`SignedUrl`].
//! - **Client messages.** `conversation_initiation_client_data` on open; `user_audio_chunk`, a
//!   base64 chunk of input audio with no `type` and no format field; `pong` with the `event_id`
//!   of the `ping` it answers; `client_tool_result` with the call's id, a string result and
//!   `is_error`. The binding sends no `conversation_config_override`, `user_message`,
//!   `contextual_update` or `user_activity`.
//! - **Server messages.** `conversation_initiation_metadata` with the conversation id and the
//!   two audio formats; `audio` (`audio_base_64`, `event_id`); `user_transcript`;
//!   `agent_response` once a message is finished; `agent_response_correction`, the truncated
//!   text after an interruption; `interruption` with an `event_id`; `ping`; `vad_score`;
//!   `client_tool_call`; `context_usage`; `agent_chat_response_part`; `client_error` with a
//!   `code`, an `error_name` and an optional `message`. [`parse`] reads them into
//!   [`ServerMessage`].
//! - **Audio formats.** `pcm_8000`, `pcm_16000`, `pcm_22050`, `pcm_24000`, `pcm_44100`,
//!   `pcm_48000` and `ulaw_8000`. The agent's configuration chooses them; a session cannot.
//!
//! # What it leaves open, and what the binding decided
//!
//! - **Encoding.** Not stated. By the names, `pcm_*` is taken as signed 16-bit little-endian
//!   mono at the rate and `ulaw_8000` as G.711 μ-law mono ([`audio_format`]). An `audio` chunk
//!   of PCM ending in half a sample keeps that byte for the next chunk of the same response.
//! - **Initiation before audio.** Not stated; the guide streams audio without waiting. The
//!   binding sends the initiation first and sends no audio before the metadata: a session opens
//!   only once the metadata arrived and its formats are the session's.
//! - **Signed URL lifetime and reuse.** Not stated. A [`SignedUrl`] is used as given, for each
//!   session opened with it; it carries a token, so it is held as a credential: zeroized on drop,
//!   only its host in `Debug`, in no error.
//! - **`ping_ms`.** One page reads it as a delay before the pong, another as estimated latency.
//!   The binding answers every `ping` at once with its `event_id` and does not wait.
//! - **Interruption and `event_id`.** Whether an interruption's `event_id` orders against the
//!   audio's, and whether audio after it belongs to a new response, is not stated. The binding
//!   infers that `event_id` increases over a connection: audio whose `event_id` is at most that
//!   of the latest interruption belongs to the cut response and is dropped, later audio opens a
//!   new response.
//! - **When a message ends.** `agent_response` arrives "once the message is finished, usually
//!   after its audio started", and the documented order is audio, then `agent_response`. The
//!   binding takes `agent_response` as the end of its message, audio included.
//! - **Close codes and idle timeout.** Only 4300 is named, after a queue timeout. Any close is
//!   read as the end of the conversation, its code and reason kept. No idle timeout is stated:
//!   a connection that sends nothing, not even a `ping`, for [`AgentEndpoint::read_timeout`]
//!   is taken as stalled.
//! - **Message size.** Not stated. The binding reads messages and frames of at most
//!   [`MAX_MESSAGE_BYTES`] (1 MiB); audio chunks are far smaller, a second of `pcm_48000` being
//!   128 KiB once encoded.
//! - **Usage.** No message carries billing. `context_usage` is the size of the model's context,
//!   not usage, and is not recorded as such.

use std::fmt;
use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use inference_duplex::inference_audio::{AudioEncoding, AudioFormat};
use serde_json::{Value, json};
use zeroize::Zeroizing;

/// The path of the conversation endpoint under the base URL.
pub const PATH: &str = "/v1/convai/conversation";

/// The largest message, and the largest frame, the binding reads: 1 MiB.
pub const MAX_MESSAGE_BYTES: usize = 1 << 20;

/// The most characters of a server's error message an [`AgentError`] keeps.
pub const SERVER_MESSAGE_LIMIT: usize = 256;

/// How long the TCP connection, TLS and the WebSocket handshake may take together.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the binding waits for `conversation_initiation_metadata` after the handshake.
pub const DEFAULT_METADATA_TIMEOUT: Duration = Duration::from_secs(10);

/// How long an open connection may send nothing at all before it is taken as stalled.
pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// The host a conversation's data is served from. Each is a base URL with the same path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Residency {
    /// `wss://api.elevenlabs.io`, the default.
    Global,
    /// `wss://api.us.elevenlabs.io`.
    Us,
    /// `wss://api.eu.residency.elevenlabs.io`.
    Eu,
    /// `wss://api.in.residency.elevenlabs.io`.
    India,
    /// `wss://api.sg.residency.elevenlabs.io`.
    Singapore,
}

impl Residency {
    /// The base URL of the residency's host.
    pub const fn base_url(self) -> &'static str {
        match self {
            Self::Global => "wss://api.elevenlabs.io",
            Self::Us => "wss://api.us.elevenlabs.io",
            Self::Eu => "wss://api.eu.residency.elevenlabs.io",
            Self::India => "wss://api.in.residency.elevenlabs.io",
            Self::Singapore => "wss://api.sg.residency.elevenlabs.io",
        }
    }
}

/// Where a public agent's sessions connect, and the bounds on opening and reading them. A
/// signed URL names its own host; only the bounds apply to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentEndpoint {
    /// Scheme and host (and port), no path or query: `wss://` to any host, `ws://` only to a
    /// loopback host, such as a local test server.
    pub url: String,
    /// The bound on the TCP connection, TLS and the WebSocket handshake together.
    pub connect_timeout: Duration,
    /// The bound on the wait for `conversation_initiation_metadata` after the handshake.
    pub metadata_timeout: Duration,
    /// How long an open connection may send nothing before it is taken as stalled.
    pub read_timeout: Duration,
}

impl Default for AgentEndpoint {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentEndpoint {
    /// The default host, [`Residency::Global`], with the default bounds.
    pub fn new() -> Self {
        Self::residency(Residency::Global)
    }

    /// The host of `residency`, with the default bounds.
    pub fn residency(residency: Residency) -> Self {
        Self {
            url: residency.base_url().to_owned(),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            metadata_timeout: DEFAULT_METADATA_TIMEOUT,
            read_timeout: DEFAULT_READ_TIMEOUT,
        }
    }

    /// This endpoint at another base URL, such as a local test server.
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// This endpoint with another connect timeout.
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// This endpoint with another bound on the wait for the metadata.
    pub fn with_metadata_timeout(mut self, timeout: Duration) -> Self {
        self.metadata_timeout = timeout;
        self
    }

    /// This endpoint with another read timeout.
    pub fn with_read_timeout(mut self, timeout: Duration) -> Self {
        self.read_timeout = timeout;
        self
    }
}

/// The URL a session with the public agent `agent_id` connects to at `endpoint`.
///
/// # Errors
/// [`AgentError::InvalidAgentId`] for an id that is empty, longer than 128 characters or not
/// ASCII letters, digits, `-` and `_`; [`AgentError::InvalidUrl`] for a base URL that is not
/// `wss://`, or `ws://` to a loopback host, or carries a path, query or fragment.
pub fn public_url(endpoint: &AgentEndpoint, agent_id: &str) -> Result<String, AgentError> {
    let valid = !agent_id.is_empty()
        && agent_id.len() <= 128
        && agent_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid {
        return Err(AgentError::InvalidAgentId);
    }
    let base = endpoint.url.trim_end_matches('/');
    check_scheme(base).map_err(|reason| AgentError::InvalidUrl(reason.to_owned()))?;
    let authority = base.split_once("://").map_or("", |(_, rest)| rest);
    if authority.is_empty() || authority.contains(['/', '?', '#', '@']) {
        return Err(AgentError::InvalidUrl(
            "the base URL must be a scheme and a host, without user, path, query or fragment"
                .to_owned(),
        ));
    }
    Ok(format!("{base}{PATH}?agent_id={agent_id}"))
}

/// Whether `url` is `wss://`, or `ws://` to a loopback host; why not, when it is not.
fn check_scheme(url: &str) -> Result<(), &'static str> {
    if let Some(rest) = url.strip_prefix("ws://") {
        if is_loopback(rest) {
            return Ok(());
        }
        return Err(
            "ws:// is taken only to a loopback host (127.0.0.0/8, ::1, localhost); use wss://",
        );
    }
    if url.starts_with("wss://") {
        return Ok(());
    }
    Err("the scheme must be wss://, or ws:// to a loopback host")
}

/// Whether the authority at the start of `rest`, what follows `ws://`, names a loopback host:
/// `localhost`, an IPv4 address in `127.0.0.0/8` or `[::1]`, with or without a port. A user
/// part (`user@host`) is refused, since it could make a host read as another.
fn is_loopback(rest: &str) -> bool {
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return false;
    }
    let host = match authority.strip_prefix('[') {
        Some(bracketed) => match bracketed.split_once(']') {
            Some((host, after)) if after.is_empty() || after.starts_with(':') => host,
            _ => return false,
        },
        None => authority.split(':').next().unwrap_or_default(),
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// A signed URL of a private agent, as `get-signed-url` returns it. It carries a token, so it is
/// held like a credential: zeroized on drop, never in `Debug`, `Display` or an error; only its
/// host is shown.
pub struct SignedUrl {
    url: Zeroizing<String>,
    host: String,
}

impl SignedUrl {
    /// Takes `url` as a signed URL.
    ///
    /// # Errors
    /// [`AgentError::InvalidSignedUrl`] for a URL that is blank, holds whitespace or control
    /// characters, is not `wss://` (or `ws://` to a loopback host), has no host or no query.
    /// The error names none of the URL.
    pub fn new(url: impl Into<String>) -> Result<Self, AgentError> {
        let url = Zeroizing::new(url.into());
        if url.trim().is_empty() {
            return Err(AgentError::InvalidSignedUrl("the signed URL is blank"));
        }
        if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(AgentError::InvalidSignedUrl(
                "the signed URL holds whitespace or a control character",
            ));
        }
        check_scheme(&url).map_err(AgentError::InvalidSignedUrl)?;
        let rest = url.split_once("://").map_or("", |(_, rest)| rest);
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if authority.is_empty() || authority.contains('@') {
            return Err(AgentError::InvalidSignedUrl(
                "the signed URL has no host, or a user part",
            ));
        }
        if !rest.contains('?') {
            return Err(AgentError::InvalidSignedUrl("the signed URL has no query"));
        }
        let host = authority.to_owned();
        Ok(Self { url, host })
    }

    /// The host (and port) the URL names; the only part of it that may be shown.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The whole URL, token included.
    pub(crate) fn expose(&self) -> &str {
        &self.url
    }

    /// `detail` with every trace of the URL taken out: the URL itself and its query.
    pub(crate) fn redact(&self, detail: &str) -> String {
        let mut detail = detail.replace(self.url.as_str(), &format!("wss://{}/…", self.host));
        if let Some((_, query)) = self.url.split_once('?') {
            detail = detail.replace(query, "[redacted]");
            for pair in query.split('&') {
                if let Some((_, value)) = pair.split_once('=')
                    && value.len() >= 4
                {
                    detail = detail.replace(value, "[redacted]");
                }
            }
        }
        detail
    }
}

impl fmt::Debug for SignedUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignedUrl")
            .field("host", &self.host)
            .finish_non_exhaustive()
    }
}

/// The contract format of an agent audio format name, when the binding maps it.
pub fn audio_format(name: &str) -> Option<AudioFormat> {
    let (encoding, rate) = match name {
        "ulaw_8000" => (AudioEncoding::PcmMulaw, 8_000),
        "pcm_8000" => (AudioEncoding::PcmS16le, 8_000),
        "pcm_16000" => (AudioEncoding::PcmS16le, 16_000),
        "pcm_22050" => (AudioEncoding::PcmS16le, 22_050),
        "pcm_24000" => (AudioEncoding::PcmS16le, 24_000),
        "pcm_44100" => (AudioEncoding::PcmS16le, 44_100),
        "pcm_48000" => (AudioEncoding::PcmS16le, 48_000),
        _ => return None,
    };
    Some(AudioFormat {
        encoding,
        sample_rate: NonZeroU32::new(rate)?,
        channels: NonZeroU16::MIN,
    })
}

/// Every audio format an agent may report, in the contract's terms.
pub fn audio_formats() -> Vec<AudioFormat> {
    [
        "pcm_8000",
        "pcm_16000",
        "pcm_22050",
        "pcm_24000",
        "pcm_44100",
        "pcm_48000",
        "ulaw_8000",
    ]
    .into_iter()
    .filter_map(audio_format)
    .collect()
}

/// A message the server sent, as the binding reads it.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerMessage {
    /// `conversation_initiation_metadata`.
    Metadata {
        conversation_id: Option<String>,
        input_format: String,
        output_format: String,
    },
    /// `audio`: a chunk of agent audio.
    Audio {
        audio: Vec<u8>,
        event_id: Option<u64>,
    },
    /// `user_transcript`.
    UserTranscript { text: String },
    /// `agent_response`: the finished text of a message.
    AgentResponse {
        text: String,
        event_id: Option<u64>,
        response_id: Option<String>,
    },
    /// `agent_response_correction`: the text of a message an interruption cut short.
    AgentResponseCorrection {
        original: String,
        corrected: String,
        response_id: Option<String>,
    },
    /// `interruption`.
    Interruption { event_id: Option<u64> },
    /// `ping`, which the binding answers with a `pong` carrying its `event_id`.
    Ping { event_id: u64 },
    /// `client_tool_call`.
    ClientToolCall {
        tool_name: String,
        tool_call_id: String,
        parameters: Value,
        expects_response: bool,
    },
    /// `client_error`.
    ClientError {
        code: Option<u64>,
        name: String,
        message: String,
    },
    /// A message of a type the binding reads and does not use: `vad_score`, `context_usage`,
    /// `agent_chat_response_part`, or a type it does not know.
    Ignored { kind: String },
}

/// The message of a server text, or what made it unreadable.
///
/// # Errors
/// What the text lacked: it is not a JSON object, has no `type`, or a typed message misses a
/// field the binding needs.
pub fn parse(text: &str) -> Result<ServerMessage, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|_| "a text message that is not JSON".to_owned())?;
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "a message without a string `type`".to_owned())?;
    let missing = |field: &str| format!("`{kind}` lacks `{field}`");
    let event = |name: &str| value.get(name).ok_or_else(|| missing(name));
    let text_of = |object: &Value, field: &str| {
        object
            .get(field)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| missing(field))
    };
    let optional_text =
        |object: &Value, field: &str| object.get(field).and_then(Value::as_str).map(str::to_owned);
    let id_of = |object: &Value| object.get("event_id").and_then(Value::as_u64);
    Ok(match kind {
        "conversation_initiation_metadata" => {
            let event = event("conversation_initiation_metadata_event")?;
            ServerMessage::Metadata {
                conversation_id: optional_text(event, "conversation_id"),
                input_format: text_of(event, "user_input_audio_format")?,
                output_format: text_of(event, "agent_output_audio_format")?,
            }
        }
        "audio" => {
            let event = event("audio_event")?;
            let encoded = text_of(event, "audio_base_64")?;
            let audio = STANDARD
                .decode(encoded)
                .map_err(|_| "`audio` whose `audio_base_64` is not base64".to_owned())?;
            ServerMessage::Audio {
                audio,
                event_id: id_of(event),
            }
        }
        "user_transcript" => ServerMessage::UserTranscript {
            text: text_of(event("user_transcription_event")?, "user_transcript")?,
        },
        "agent_response" => {
            let event = event("agent_response_event")?;
            ServerMessage::AgentResponse {
                text: text_of(event, "agent_response")?,
                event_id: id_of(event),
                response_id: optional_text(event, "response_id"),
            }
        }
        "agent_response_correction" => {
            let event = event("agent_response_correction_event")?;
            ServerMessage::AgentResponseCorrection {
                original: optional_text(event, "original_agent_response").unwrap_or_default(),
                corrected: text_of(event, "corrected_agent_response")?,
                response_id: optional_text(event, "response_id"),
            }
        }
        "interruption" => ServerMessage::Interruption {
            event_id: value.get("interruption_event").and_then(id_of),
        },
        "ping" => ServerMessage::Ping {
            event_id: id_of(event("ping_event")?).ok_or_else(|| missing("ping_event.event_id"))?,
        },
        "client_tool_call" => {
            let call = event("client_tool_call")?;
            ServerMessage::ClientToolCall {
                tool_name: text_of(call, "tool_name")?,
                tool_call_id: text_of(call, "tool_call_id")?,
                parameters: call.get("parameters").cloned().unwrap_or(Value::Null),
                expects_response: call
                    .get("expects_response")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
            }
        }
        "client_error" => {
            let error = value.get("error_event").unwrap_or(&value);
            ServerMessage::ClientError {
                code: error.get("code").and_then(Value::as_u64),
                name: optional_text(error, "error_name").unwrap_or_default(),
                message: optional_text(error, "message")
                    .unwrap_or_default()
                    .chars()
                    .take(SERVER_MESSAGE_LIMIT)
                    .collect(),
            }
        }
        other => ServerMessage::Ignored {
            kind: other.to_owned(),
        },
    })
}

/// The message that opens a conversation; it overrides nothing of the agent's configuration.
pub fn initiation() -> String {
    json!({"type": "conversation_initiation_client_data"}).to_string()
}

/// A chunk of user audio in the agent's input format.
pub fn audio_chunk(audio: &[u8]) -> String {
    json!({"user_audio_chunk": STANDARD.encode(audio)}).to_string()
}

/// The answer to the `ping` carrying `event_id`.
pub fn pong(event_id: u64) -> String {
    json!({"type": "pong", "event_id": event_id}).to_string()
}

/// The result of the client tool call `tool_call_id`.
pub fn tool_result(tool_call_id: &str, result: &str, is_error: bool) -> String {
    json!({
        "type": "client_tool_result",
        "tool_call_id": tool_call_id,
        "result": result,
        "is_error": is_error,
    })
    .to_string()
}

/// What went wrong with an agent conversation. No variant carries a signed URL or its token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentError {
    /// The agent id is empty, longer than 128 characters or not ASCII letters, digits, `-`
    /// and `_`.
    InvalidAgentId,
    /// The signed URL was refused, with why.
    InvalidSignedUrl(&'static str),
    /// The base URL was refused, with why.
    InvalidUrl(String),
    /// The caller cancelled the open.
    Cancelled,
    /// The handshake failed: refused by the server, with its HTTP status, not answered within
    /// the connect timeout, or the connection could not be made.
    Connect { status: Option<u16>, detail: String },
    /// The agent's audio formats, as its metadata named them, are not the session's.
    FormatMismatch { input: String, output: String },
    /// Writing or reading the open connection failed.
    Transport(String),
    /// A message larger than [`MAX_MESSAGE_BYTES`] arrived.
    Oversize(String),
    /// The server closed the connection, with its close code and reason when it gave one.
    Closed { code: Option<u16>, reason: String },
    /// Nothing arrived within the bound this carries.
    Stalled(Duration),
    /// The server sent `client_error`.
    Client {
        code: Option<u64>,
        name: String,
        message: String,
    },
    /// The agent called a client tool the session does not offer and awaits its result.
    ToolNotOffered { name: String },
    /// The runtime the session drives its connection on could not be built.
    Runtime(String),
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAgentId => {
                f.write_str("the agent id must be 1 to 128 ASCII letters, digits, `-` or `_`")
            }
            Self::InvalidSignedUrl(reason) => write!(f, "invalid signed URL: {reason}"),
            Self::InvalidUrl(reason) => write!(f, "invalid base URL: {reason}"),
            Self::Cancelled => f.write_str("the caller cancelled the open"),
            Self::Connect { status, detail } => match status {
                Some(status) => write!(f, "the handshake was refused with HTTP {status}: {detail}"),
                None => write!(f, "the handshake failed: {detail}"),
            },
            Self::FormatMismatch { input, output } => write!(
                f,
                "the agent takes {input} and gives {output}, not the session's formats"
            ),
            Self::Transport(detail) => write!(f, "transport failed: {detail}"),
            Self::Oversize(detail) => write!(
                f,
                "a message beyond {MAX_MESSAGE_BYTES} bytes arrived: {detail}"
            ),
            Self::Closed { code, reason } => match code {
                Some(code) => write!(f, "the server closed the connection ({code}): {reason}"),
                None => f.write_str("the server closed the connection"),
            },
            Self::Stalled(waited) => write!(f, "nothing arrived within {waited:?}"),
            Self::Client {
                code,
                name,
                message,
            } => match code {
                Some(code) => write!(f, "client_error {code} {name}: {message}"),
                None => write!(f, "client_error {name}: {message}"),
            },
            Self::ToolNotOffered { name } => write!(
                f,
                "the agent awaits the result of the client tool `{name}`, which the session does not offer"
            ),
            Self::Runtime(detail) => write!(f, "the session's runtime failed: {detail}"),
        }
    }
}

impl std::error::Error for AgentError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_documented_server_message_is_read() {
        let audio = STANDARD.encode([1_u8, 2, 3, 4]);
        let cases = [
            (
                r#"{"type":"conversation_initiation_metadata","conversation_initiation_metadata_event":{"conversation_id":"c","agent_output_audio_format":"pcm_24000","user_input_audio_format":"ulaw_8000"}}"#.to_owned(),
                ServerMessage::Metadata {
                    conversation_id: Some("c".to_owned()),
                    input_format: "ulaw_8000".to_owned(),
                    output_format: "pcm_24000".to_owned(),
                },
            ),
            (
                format!(r#"{{"type":"audio","audio_event":{{"audio_base_64":"{audio}","event_id":5,"is_final":false}}}}"#),
                ServerMessage::Audio {
                    audio: vec![1, 2, 3, 4],
                    event_id: Some(5),
                },
            ),
            (
                r#"{"type":"ping","ping_event":{"event_id":9,"ping_ms":50}}"#.to_owned(),
                ServerMessage::Ping { event_id: 9 },
            ),
            (
                r#"{"type":"interruption","interruption_event":{"event_id":7}}"#.to_owned(),
                ServerMessage::Interruption { event_id: Some(7) },
            ),
            (
                r#"{"type":"vad_score","vad_score_event":{"vad_score":0.9}}"#.to_owned(),
                ServerMessage::Ignored {
                    kind: "vad_score".to_owned(),
                },
            ),
            (
                r#"{"type":"client_error","error_event":{"code":4000,"error_name":"bad"}}"#
                    .to_owned(),
                ServerMessage::ClientError {
                    code: Some(4000),
                    name: "bad".to_owned(),
                    message: String::new(),
                },
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(parse(&text), Ok(expected), "{text}");
        }
        for unreadable in [
            "not json",
            r#"{"no":"type"}"#,
            r#"{"type":"audio","audio_event":{"audio_base_64":"***"}}"#,
            r#"{"type":"ping","ping_event":{}}"#,
            r#"{"type":"agent_response"}"#,
        ] {
            assert!(parse(unreadable).is_err(), "{unreadable}");
        }
    }

    #[test]
    fn the_agent_formats_map_onto_mono_contract_formats() {
        assert_eq!(audio_formats().len(), 7);
        let ulaw = audio_format("ulaw_8000").unwrap();
        assert_eq!(ulaw.encoding, AudioEncoding::PcmMulaw);
        assert_eq!(ulaw.sample_rate.get(), 8_000);
        assert_eq!(
            audio_format("pcm_48000").unwrap().encoding,
            AudioEncoding::PcmS16le
        );
        assert!(audio_format("mp3_44100_128").is_none());
    }

    #[test]
    fn a_public_url_names_the_agent_on_the_residency_host() {
        let url = public_url(&AgentEndpoint::residency(Residency::Eu), "agent_1").unwrap();
        assert_eq!(
            url,
            "wss://api.eu.residency.elevenlabs.io/v1/convai/conversation?agent_id=agent_1"
        );
        for base in [
            "wss://host/path",
            "wss://host?x=1",
            "wss://",
            "wss://user@host",
        ] {
            assert!(
                matches!(
                    public_url(&AgentEndpoint::new().with_url(base), "a"),
                    Err(AgentError::InvalidUrl(_))
                ),
                "{base}"
            );
        }
    }

    #[test]
    fn redaction_takes_the_url_and_its_token_out() {
        let signed = SignedUrl::new(
            "wss://api.elevenlabs.io/v1/convai/conversation?agent_id=a&token=tok123",
        )
        .unwrap();
        let detail = signed.redact(
            "failed for wss://api.elevenlabs.io/v1/convai/conversation?agent_id=a&token=tok123 (tok123)",
        );
        assert!(!detail.contains("tok123"), "{detail}");
        assert!(detail.contains("api.elevenlabs.io"));
    }
}
