//! The wire protocol of a Phonic speech-to-speech conversation: where it connects, the `config`
//! it sends, the messages it reads, and what the documentation leaves open.
//!
//! Taken on 2026-11-06 from Phonic's public documentation, summarised with its URLs in this crate's
//! `protocol/README.md`:
//!
//! - <https://docs.phonic.co/api-reference/conversations/conversations>, the AsyncAPI reference;
//! - <https://docs.phonic.co/docs/build/with-web-sockets/via-web-sockets>;
//! - <https://docs.phonic.co/docs/build/tools/websocket_tools>;
//! - <https://docs.phonic.co/docs/platform/authentication/overview>.
//!
//! # What the documentation states
//!
//! - **Endpoint.** `wss://api.phonic.ai/v1/sts/ws` ([`DEFAULT_URL`], [`PATH`]), with
//!   `Authorization: Bearer <API key>` in the handshake.
//! - **Order.** `config` is the first message ([`config_message`]); its `input_format` and
//!   `output_format` choose the audio both ways, one of `pcm_44100`, `pcm_24000`, `pcm_16000`,
//!   `pcm_8000` and `mulaw_8000` ([`format_name`]).
//! - **Audio.** JSON text messages both ways, the audio base64 in `audio`. A client
//!   `audio_chunk` holds at most 40 ms; the binding sends at most [`CHUNK_MS`] (20 ms), as
//!   recommended ([`audio_chunks`]). A server `audio_chunk` carries the `text` of its audio.
//! - **Tools.** The session's tools go in `config.tools` as inline `custom_websocket` tools, each
//!   an OpenAI function schema with `strict: true`, executed `sync`. A `tool_call` is answered
//!   with `tool_call_output` ([`tool_output`]).
//! - **Server messages** ([`parse`]): `ready_to_start_conversation`, `conversation_created`,
//!   `input_text`, `input_cancelled`, `audio_chunk`, `user_started_speaking`,
//!   `user_finished_speaking`, `assistant_started_speaking`, `assistant_finished_speaking` (not
//!   sent when the user interrupted), `interrupted_response` (what was spoken before the
//!   interruption), `dtmf`, `tool_call`, `tool_call_output_processed`, `tool_call_interrupted`,
//!   `assistant_chose_not_to_respond`, `assistant_ended_conversation`, `error` and `warning`
//!   (non-fatal).
//! - **Usage.** No server message carries usage or billing.
//!
//! # What it leaves open, and what the binding decided
//!
//! - **`ready_to_start_conversation` before or after `config`.** Not stated. The binding sends
//!   `config` at once and no audio before it has read `ready_to_start_conversation`.
//! - **PCM byte order and the server's audio encoding.** Not stated beyond `Int16Array`. Taken
//!   to be 16-bit little-endian mono for `pcm_*`, the server's audio in the `output_format`.
//! - **Whether `error` is fatal.** Only `warning` is called non-fatal; an `error` fails the
//!   session (inferred).
//! - **Barge-in.** Whether `user_started_speaking` itself cuts the assistant is not stated. The
//!   response is cut at `interrupted_response`, the documented report of an interruption, and its
//!   audio after it, up to the next `assistant_started_speaking`, is dropped (inferred).
//! - **A response without `assistant_started_speaking`.** Not stated; audio while no response is
//!   in progress opens one.
//! - **Keepalive and idle close.** No keepalive message is documented; the server closes after
//!   `websocket_timeout_sec` (default 60) of inactivity. The binding sends a WebSocket ping
//!   after [`PhonicEndpoint::ping_interval`] without reading anything, and a connection that
//!   sends nothing, not even the pong RFC 6455 requires, for [`PhonicEndpoint::read_timeout`] is
//!   stalled.
//! - **Message size.** Not stated. The binding reads messages and frames of at most
//!   [`MAX_MESSAGE_BYTES`] (1 MiB).
//! - **`strict: true`.** Whether it requires OpenAI's strict schema form is not stated; the tool's
//!   schema is sent as it was declared.
//! - **Reconnection.** The SDKs reconnect with `reconnect_conv_id`; the reference names no such
//!   parameter, so the binding does not reconnect: a close ends the session.

use std::fmt;
use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use inference_duplex::SessionConfig;
use inference_duplex::inference_audio::{AudioEncoding, AudioFormat};
use inference_duplex::inference_tools::Json;
use serde_json::{Map, Value, json};

use crate::credential::CredentialError;

/// The default base URL of the conversation endpoint.
pub const DEFAULT_URL: &str = "wss://api.phonic.ai";

/// The path of the conversation endpoint under the base URL.
pub const PATH: &str = "/v1/sts/ws";

/// The model the binding asks for unless configured otherwise.
pub const DEFAULT_MODEL: &str = "phonic-1";

/// The largest message, and the largest frame, the binding reads: 1 MiB.
pub const MAX_MESSAGE_BYTES: usize = 1 << 20;

/// The most milliseconds of audio one client `audio_chunk` carries; the documentation's bound
/// is 40.
pub const CHUNK_MS: u32 = 20;

/// The most characters of a server's error or warning message the binding keeps.
pub const SERVER_MESSAGE_LIMIT: usize = 256;

/// The most characters of a `config` field the binding sends that the documentation bounds
/// nowhere: the model, agent, project and voice names.
pub const NAME_LIMIT: usize = 128;

/// The longest `system_prompt` the documentation admits, in characters.
pub const SYSTEM_PROMPT_LIMIT: usize = 1_000_000;

/// How long the TCP connection, TLS and the WebSocket handshake may take together.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the binding waits for `ready_to_start_conversation` after sending `config`.
pub const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(10);

/// How long an open connection may send nothing at all before it is taken as stalled.
pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the binding reads nothing before it sends a WebSocket ping.
pub const DEFAULT_PING_INTERVAL: Duration = Duration::from_secs(10);

/// Where a session connects, and the bounds on opening and reading it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhonicEndpoint {
    /// Scheme and host (and port), no path or query: `wss://` to any host, `ws://` only to a
    /// loopback host, such as a local test server.
    pub url: String,
    /// The bound on the TCP connection, TLS and the WebSocket handshake together.
    pub connect_timeout: Duration,
    /// The bound on the wait for `ready_to_start_conversation` after `config`.
    pub ready_timeout: Duration,
    /// How long an open connection may send nothing before it is taken as stalled.
    pub read_timeout: Duration,
    /// How long the binding reads nothing before it pings.
    pub ping_interval: Duration,
}

impl Default for PhonicEndpoint {
    fn default() -> Self {
        Self::new()
    }
}

impl PhonicEndpoint {
    /// [`DEFAULT_URL`] with the default bounds.
    pub fn new() -> Self {
        Self {
            url: DEFAULT_URL.to_owned(),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            ready_timeout: DEFAULT_READY_TIMEOUT,
            read_timeout: DEFAULT_READ_TIMEOUT,
            ping_interval: DEFAULT_PING_INTERVAL,
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

    /// This endpoint with another bound on the wait for `ready_to_start_conversation`.
    pub fn with_ready_timeout(mut self, timeout: Duration) -> Self {
        self.ready_timeout = timeout;
        self
    }

    /// This endpoint with another read timeout.
    pub fn with_read_timeout(mut self, timeout: Duration) -> Self {
        self.read_timeout = timeout;
        self
    }

    /// This endpoint with another ping interval.
    pub fn with_ping_interval(mut self, interval: Duration) -> Self {
        self.ping_interval = interval;
        self
    }
}

/// What the `config` message names besides the session configuration: the model, and the agent
/// and project whose stored configuration applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhonicConfig {
    /// `model`; `None` omits it, so Phonic's current default applies.
    pub model: Option<String>,
    /// `agent`, the name of an agent whose configuration applies; `None` sends none.
    pub agent: Option<String>,
    /// `project`; `None` omits it, so `main` applies.
    pub project: Option<String>,
}

impl Default for PhonicConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl PhonicConfig {
    /// [`DEFAULT_MODEL`], no agent, the default project.
    pub fn new() -> Self {
        Self {
            model: Some(DEFAULT_MODEL.to_owned()),
            agent: None,
            project: None,
        }
    }

    /// This configuration with `model`, or with none.
    pub fn with_model(mut self, model: Option<String>) -> Self {
        self.model = model;
        self
    }

    /// This configuration naming the agent `agent`.
    pub fn with_agent(mut self, agent: impl Into<String>) -> Self {
        self.agent = Some(agent.into());
        self
    }

    /// This configuration in the project `project`.
    pub fn with_project(mut self, project: impl Into<String>) -> Self {
        self.project = Some(project.into());
        self
    }

    /// Refuses a name that is blank, longer than [`NAME_LIMIT`] characters or holds a control
    /// character.
    pub(crate) fn check(&self) -> Result<(), PhonicError> {
        for (field, value) in [
            ("model", &self.model),
            ("agent", &self.agent),
            ("project", &self.project),
        ] {
            if let Some(value) = value
                && !name(value)
            {
                return Err(PhonicError::InvalidConfig(field));
            }
        }
        Ok(())
    }
}

/// Whether `value` is a name the binding sends: not blank, at most [`NAME_LIMIT`] characters,
/// no control character.
fn name(value: &str) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= NAME_LIMIT
        && !value.chars().any(char::is_control)
}

/// The URL a session connects to at `endpoint`.
///
/// # Errors
/// [`PhonicError::InvalidUrl`] for a base URL that is not `wss://`, or `ws://` to a loopback
/// host, or carries a user, path, query or fragment.
pub fn url(endpoint: &PhonicEndpoint) -> Result<String, PhonicError> {
    let base = endpoint.url.trim_end_matches('/');
    check_scheme(base).map_err(|reason| PhonicError::InvalidUrl(reason.to_owned()))?;
    let authority = base.split_once("://").map_or("", |(_, rest)| rest);
    if authority.is_empty() || authority.contains(['/', '?', '#', '@']) {
        return Err(PhonicError::InvalidUrl(
            "the base URL must be a scheme and a host, without user, path, query or fragment"
                .to_owned(),
        ));
    }
    Ok(format!("{base}{PATH}"))
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

/// Whether the authority at the start of `rest`, what follows `ws://`, names a loopback host.
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

/// The documented format names, each with its encoding and rate.
const FORMATS: [(&str, AudioEncoding, u32); 5] = [
    ("pcm_44100", AudioEncoding::PcmS16le, 44_100),
    ("pcm_24000", AudioEncoding::PcmS16le, 24_000),
    ("pcm_16000", AudioEncoding::PcmS16le, 16_000),
    ("pcm_8000", AudioEncoding::PcmS16le, 8_000),
    ("mulaw_8000", AudioEncoding::PcmMulaw, 8_000),
];

/// The format name of `format` in `config`, when Phonic takes it: mono, 16-bit PCM at 44.1, 24,
/// 16 or 8 kHz, or G.711 μ-law at 8 kHz.
pub fn format_name(format: AudioFormat) -> Option<&'static str> {
    if format.channels != NonZeroU16::MIN {
        return None;
    }
    FORMATS
        .iter()
        .find(|(_, encoding, rate)| {
            format.encoding == *encoding && format.sample_rate.get() == *rate
        })
        .map(|(name, _, _)| *name)
}

/// Every format Phonic takes, each way, in the contract's terms.
pub fn audio_formats() -> Vec<AudioFormat> {
    FORMATS
        .iter()
        .filter_map(|(_, encoding, rate)| {
            Some(AudioFormat {
                encoding: *encoding,
                sample_rate: NonZeroU32::new(*rate)?,
                channels: NonZeroU16::MIN,
            })
        })
        .collect()
}

/// The bytes of one sample of `format`.
pub(crate) fn sample_width(format: AudioFormat) -> usize {
    match format.encoding {
        AudioEncoding::PcmS16le => 2,
        _ => 1,
    }
}

/// The `config` message of a session: `phonic`'s names, the session's formats, its voice and
/// instructions where they are not empty, and its tools as inline WebSocket tools.
///
/// # Errors
/// [`PhonicError::InvalidConfig`] naming the field the binding refuses: a format Phonic does
/// not take, a voice that is not a name, instructions beyond [`SYSTEM_PROMPT_LIMIT`].
pub fn config_message(
    phonic: &PhonicConfig,
    session: &SessionConfig,
) -> Result<String, PhonicError> {
    phonic.check()?;
    let input =
        format_name(session.input_format).ok_or(PhonicError::InvalidConfig("input_format"))?;
    let output =
        format_name(session.output_format).ok_or(PhonicError::InvalidConfig("output_format"))?;
    let mut config = Map::new();
    config.insert("type".to_owned(), json!("config"));
    for (field, value) in [
        ("agent", &phonic.agent),
        ("project", &phonic.project),
        ("model", &phonic.model),
    ] {
        if let Some(value) = value {
            config.insert(field.to_owned(), json!(value));
        }
    }
    config.insert("input_format".to_owned(), json!(input));
    config.insert("output_format".to_owned(), json!(output));
    if !session.voice.is_empty() {
        if !name(&session.voice) {
            return Err(PhonicError::InvalidConfig("voice_id"));
        }
        config.insert("voice_id".to_owned(), json!(session.voice));
    }
    if !session.instructions.is_empty() {
        if session.instructions.chars().count() > SYSTEM_PROMPT_LIMIT {
            return Err(PhonicError::InvalidConfig("system_prompt"));
        }
        config.insert("system_prompt".to_owned(), json!(session.instructions));
    }
    if !session.tools.is_empty() {
        let tools: Vec<Value> = session
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "custom_websocket",
                    "tool_schema": {
                        "type": "function",
                        "function": {
                            "name": tool.name(),
                            "description": tool.description(),
                            "parameters": tool.schema(),
                            "strict": true,
                        },
                    },
                    "execution_mode": "sync",
                })
            })
            .collect();
        config.insert("tools".to_owned(), Value::Array(tools));
    }
    Ok(Value::Object(config).to_string())
}

/// The `audio_chunk` messages carrying `audio` of `format`, each at most [`CHUNK_MS`] of it and
/// whole samples only.
pub fn audio_chunks(audio: &[u8], format: AudioFormat) -> Vec<String> {
    let width = sample_width(format);
    let samples = (format.sample_rate.get() * CHUNK_MS / 1_000).max(1) as usize;
    audio
        .chunks(samples * width)
        .map(|piece| json!({"type": "audio_chunk", "audio": STANDARD.encode(piece)}).to_string())
        .collect()
}

/// The answer to the `tool_call` named `tool_call_id`.
pub fn tool_output(tool_call_id: &str, output: &Json) -> String {
    json!({
        "type": "tool_call_output",
        "tool_call_id": tool_call_id,
        "output": output,
    })
    .to_string()
}

/// A message the server sent, as the binding reads it.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerMessage {
    /// `ready_to_start_conversation`.
    Ready,
    /// `conversation_created`.
    ConversationCreated { conversation_id: String },
    /// `input_text`: the transcript of the user's turn.
    InputText { text: String, language: String },
    /// `input_cancelled`: no speech was detected.
    InputCancelled,
    /// `audio_chunk`: assistant audio and its text.
    AudioChunk { audio: Vec<u8>, text: String },
    /// `user_started_speaking`.
    UserStartedSpeaking,
    /// `user_finished_speaking`.
    UserFinishedSpeaking,
    /// `assistant_started_speaking`.
    AssistantStartedSpeaking,
    /// `assistant_finished_speaking`.
    AssistantFinishedSpeaking,
    /// `interrupted_response`: what the assistant said before the user cut it.
    InterruptedResponse { text: String },
    /// `tool_call`.
    ToolCall {
        tool_call_id: String,
        tool_name: String,
        parameters: Value,
    },
    /// `assistant_ended_conversation`.
    AssistantEnded,
    /// `error`.
    Error {
        code: Option<String>,
        message: String,
    },
    /// `warning`.
    Warning {
        code: Option<String>,
        message: String,
    },
    /// A message the binding reads and does not use: `dtmf`, `tool_call_output_processed`,
    /// `tool_call_interrupted`, `assistant_chose_not_to_respond`, or a type it does not know.
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
    let text_of = |object: &Value, field: &str| {
        object
            .get(field)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| missing(field))
    };
    let notice = |field: &str| -> Result<(Option<String>, String), String> {
        let body = value.get(field).ok_or_else(|| missing(field))?;
        let message = text_of(body, "message")?
            .chars()
            .take(SERVER_MESSAGE_LIMIT)
            .collect();
        let code = body.get("code").and_then(Value::as_str).map(str::to_owned);
        Ok((code, message))
    };
    Ok(match kind {
        "ready_to_start_conversation" => ServerMessage::Ready,
        "conversation_created" => ServerMessage::ConversationCreated {
            conversation_id: text_of(&value, "conversation_id")?,
        },
        "input_text" => ServerMessage::InputText {
            text: text_of(&value, "text")?,
            language: value
                .get("language")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        },
        "input_cancelled" => ServerMessage::InputCancelled,
        "audio_chunk" => {
            let encoded = text_of(&value, "audio")?;
            let audio = STANDARD
                .decode(encoded)
                .map_err(|_| "`audio_chunk` whose `audio` is not base64".to_owned())?;
            ServerMessage::AudioChunk {
                audio,
                text: value
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            }
        }
        "user_started_speaking" => ServerMessage::UserStartedSpeaking,
        "user_finished_speaking" => ServerMessage::UserFinishedSpeaking,
        "assistant_started_speaking" => ServerMessage::AssistantStartedSpeaking,
        "assistant_finished_speaking" => ServerMessage::AssistantFinishedSpeaking,
        "interrupted_response" => ServerMessage::InterruptedResponse {
            text: value
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        },
        "tool_call" => ServerMessage::ToolCall {
            tool_call_id: text_of(&value, "tool_call_id")?,
            tool_name: text_of(&value, "tool_name")?,
            parameters: value.get("parameters").cloned().unwrap_or(Value::Null),
        },
        "assistant_ended_conversation" => ServerMessage::AssistantEnded,
        "error" => {
            let (code, message) = notice("error")?;
            ServerMessage::Error { code, message }
        }
        "warning" => {
            let (code, message) = notice("warning")?;
            ServerMessage::Warning { code, message }
        }
        other => ServerMessage::Ignored {
            kind: other.to_owned(),
        },
    })
}

/// What went wrong with a Phonic conversation. No variant carries the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhonicError {
    /// The base URL was refused, with why.
    InvalidUrl(String),
    /// A field of `config` the binding refuses to send, by its name.
    InvalidConfig(&'static str),
    /// The key could not be had.
    Credential(CredentialError),
    /// The handshake failed: refused by the server, with its HTTP status, not answered within
    /// the connect timeout, or the connection could not be made.
    Connect { status: Option<u16>, detail: String },
    /// Writing or reading the open connection failed.
    Transport(String),
    /// A message larger than [`MAX_MESSAGE_BYTES`] arrived.
    Oversize(String),
    /// The server closed the connection, with its close code and reason when it gave one.
    Closed { code: Option<u16>, reason: String },
    /// Nothing arrived within the bound this carries.
    Stalled(Duration),
    /// The server sent `error`.
    Server {
        code: Option<String>,
        message: String,
    },
    /// The server called a tool the session does not offer.
    ToolNotOffered { name: String },
    /// The runtime the session drives its connection on could not be built.
    Runtime(String),
}

impl fmt::Display for PhonicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl(reason) => write!(f, "invalid base URL: {reason}"),
            Self::InvalidConfig(field) => write!(f, "the config field `{field}` is refused"),
            Self::Credential(error) => write!(f, "the key could not be had: {error}"),
            Self::Connect { status, detail } => match status {
                Some(status) => write!(f, "the handshake was refused with HTTP {status}: {detail}"),
                None => write!(f, "the handshake failed: {detail}"),
            },
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
            Self::Server { code, message } => match code {
                Some(code) => write!(f, "error {code}: {message}"),
                None => write!(f, "error: {message}"),
            },
            Self::ToolNotOffered { name } => write!(
                f,
                "the assistant called the tool `{name}`, which the session does not offer"
            ),
            Self::Runtime(detail) => write!(f, "the session's runtime failed: {detail}"),
        }
    }
}

impl std::error::Error for PhonicError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_documented_server_message_is_read() {
        let audio = STANDARD.encode([1_u8, 2, 3, 4]);
        let cases = [
            (
                r#"{"type":"ready_to_start_conversation"}"#.to_owned(),
                ServerMessage::Ready,
            ),
            (
                r#"{"type":"conversation_created","conversation_id":"c1"}"#.to_owned(),
                ServerMessage::ConversationCreated {
                    conversation_id: "c1".to_owned(),
                },
            ),
            (
                r#"{"type":"input_text","language":"en","text":"hi"}"#.to_owned(),
                ServerMessage::InputText {
                    text: "hi".to_owned(),
                    language: "en".to_owned(),
                },
            ),
            (
                format!(
                    r#"{{"type":"audio_chunk","audio":"{audio}","text":"Hel","timings":{{"x":1.5}}}}"#
                ),
                ServerMessage::AudioChunk {
                    audio: vec![1, 2, 3, 4],
                    text: "Hel".to_owned(),
                },
            ),
            (
                r#"{"type":"interrupted_response","text":"It is"}"#.to_owned(),
                ServerMessage::InterruptedResponse {
                    text: "It is".to_owned(),
                },
            ),
            (
                r#"{"type":"tool_call","tool_call_id":"t1","tool_name":"now","parameters":{"a":1}}"#
                    .to_owned(),
                ServerMessage::ToolCall {
                    tool_call_id: "t1".to_owned(),
                    tool_name: "now".to_owned(),
                    parameters: json!({"a": 1}),
                },
            ),
            (
                r#"{"type":"error","error":{"message":"bad","code":"x"}}"#.to_owned(),
                ServerMessage::Error {
                    code: Some("x".to_owned()),
                    message: "bad".to_owned(),
                },
            ),
            (
                r#"{"type":"warning","warning":{"message":"careful"}}"#.to_owned(),
                ServerMessage::Warning {
                    code: None,
                    message: "careful".to_owned(),
                },
            ),
            (
                r#"{"type":"dtmf","digits":"12"}"#.to_owned(),
                ServerMessage::Ignored {
                    kind: "dtmf".to_owned(),
                },
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(parse(&text), Ok(expected), "{text}");
        }
        for unreadable in [
            "not json",
            r#"{"no":"type"}"#,
            r#"{"type":"audio_chunk","audio":"***","text":""}"#,
            r#"{"type":"audio_chunk"}"#,
            r#"{"type":"tool_call","tool_call_id":"t"}"#,
            r#"{"type":"error"}"#,
        ] {
            assert!(parse(unreadable).is_err(), "{unreadable}");
        }
    }

    #[test]
    fn the_documented_formats_map_onto_mono_contract_formats() {
        let formats = audio_formats();
        assert_eq!(formats.len(), 5);
        let names: Vec<_> = formats.iter().filter_map(|f| format_name(*f)).collect();
        assert_eq!(
            names,
            [
                "pcm_44100",
                "pcm_24000",
                "pcm_16000",
                "pcm_8000",
                "mulaw_8000"
            ]
        );
        let stereo = AudioFormat {
            channels: NonZeroU16::new(2).unwrap(),
            ..formats[0]
        };
        assert_eq!(format_name(stereo), None);
    }

    #[test]
    fn audio_is_cut_into_chunks_of_at_most_twenty_milliseconds() {
        let format = audio_formats()[2];
        let chunks = audio_chunks(&[0; 3_300], format);
        let sizes: Vec<_> = chunks
            .iter()
            .map(|chunk| {
                let value: Value = serde_json::from_str(chunk).unwrap();
                STANDARD
                    .decode(value["audio"].as_str().unwrap())
                    .unwrap()
                    .len()
            })
            .collect();
        assert_eq!(sizes, [640, 640, 640, 640, 640, 100]);
    }

    #[test]
    fn a_base_url_with_a_path_or_a_plaintext_remote_host_is_refused() {
        assert_eq!(
            url(&PhonicEndpoint::new()).unwrap(),
            "wss://api.phonic.ai/v1/sts/ws"
        );
        for base in [
            "wss://host/path",
            "wss://host?x=1",
            "wss://",
            "wss://user@host",
            "ws://api.phonic.ai",
            "ws://127.0.0.1@evil.example",
            "http://127.0.0.1:9",
        ] {
            assert!(
                matches!(
                    url(&PhonicEndpoint::new().with_url(base)),
                    Err(PhonicError::InvalidUrl(_))
                ),
                "{base}"
            );
        }
        for base in ["ws://127.0.0.1:9", "ws://[::1]:9", "ws://localhost:9"] {
            assert!(url(&PhonicEndpoint::new().with_url(base)).is_ok(), "{base}");
        }
    }
}
