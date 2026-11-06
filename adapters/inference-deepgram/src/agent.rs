//! The wire protocol of a Deepgram Voice Agent session: where it connects, the `Settings` it
//! sends, what it reads, and what the documentation leaves open.
//!
//! Taken on 2026-11-05 from Deepgram's public documentation:
//!
//! - <https://developers.deepgram.com/docs/voice-agent>;
//! - <https://developers.deepgram.com/reference/voice-agent/voice-agent>, the AsyncAPI reference;
//! - <https://developers.deepgram.com/docs/configure-voice-agent>;
//! - <https://developers.deepgram.com/docs/voice-agent-tts-models>;
//! - <https://developers.deepgram.com/docs/twilio-and-deepgram-voice-agent>;
//! - <https://developers.deepgram.com/docs/voice-agent-message-flow>;
//! - <https://developers.deepgram.com/docs/voice-agent-media-inputs-outputs>;
//! - <https://developers.deepgram.com/docs/voice-agent-errors-warnings>;
//! - <https://developers.deepgram.com/docs/agent-keep-alive>.
//!
//! # What the documentation states
//!
//! - **Endpoint.** `wss://agent.deepgram.com/v1/agent/converse` ([`crate::AGENT_URL`],
//!   [`PATH`]), with an `Authorization` header on the handshake.
//! - **Order.** Connect, wait for `Welcome`, send `Settings` first, wait for `SettingsApplied`,
//!   then send audio. Audio before `SettingsApplied` is not allowed.
//! - **Settings** ([`settings`]): `audio.input` (`encoding`, `sample_rate`), `audio.output`
//!   (`encoding`, `sample_rate`, `container`), `agent.listen.provider`, `agent.think.provider`
//!   and `prompt`, `agent.speak.provider`, an optional `agent.greeting`, and client-side
//!   functions under `agent.think.functions`. Input and output take `linear16`, `mulaw` and
//!   `alaw`, among others; the sample rate is set explicitly, since the defaults disagree.
//! - **Audio.** Binary frames both ways, raw bytes in the declared format: no base64, no header,
//!   no id or sequence.
//! - **Client messages.** `KeepAlive` every 8 seconds, only while no audio is sent, with no
//!   reply; `FunctionCallResponse` with the request's `id`, the function's `name` and the
//!   `content`. The binding sends no `InjectUserMessage`, `InjectAgentMessage`, `UpdatePrompt`,
//!   `UpdateSpeak` or `ForceEndTurn`.
//! - **Server messages** ([`parse`]): `Welcome` with a `request_id`; `SettingsApplied`;
//!   `ConversationText` with a `role` (`user` or `assistant`) and `content`;
//!   `UserStartedSpeaking`; `AgentThinking`; `AgentStartedSpeaking` with latencies;
//!   `AgentAudioDone`; `FunctionCallRequest`; `FunctionCallCancelled`; `PromptUpdated`;
//!   `InjectionRefused`; `LatencyReport`; `History`; `Warning` and `Error`, each with a
//!   `description` and a `code`.
//! - **Barge-in.** `UserStartedSpeaking` means the user started talking and the server stopped
//!   the agent's turn; the client clears what it queued.
//! - **Errors.** Every `Error` is fatal and the connection usually closes soon after; `Warning`
//!   is not fatal.
//! - **Latencies** are seconds; the schema types them as strings with the title float.
//! - **Usage.** No server message carries usage or billing; billing is per minute.
//!
//! # What it leaves open, and what the binding decided
//!
//! - **The `Authorization` value.** Not stated. The binding sends `Token <key>`, as the live
//!   transcription endpoint takes it; the scheme is [`crate::AgentEndpoint::auth_scheme`].
//! - **The EU host.** The EU processing guide names `api.eu.deepgram.com` without the agent's
//!   path on it, so no EU agent URL is built in: [`crate::AgentEndpoint::with_url`] sets a host.
//! - **Reply correlation.** No turn, response or sequence id is on any message or frame; only
//!   order separates replies. A reply opens at `AgentStartedSpeaking`, or at a binary frame when
//!   none is open (a greeting has no `AgentStartedSpeaking`), and ends at `AgentAudioDone`, or at
//!   the next `AgentStartedSpeaking` once it has sent audio. `ConversationText` of the assistant
//!   arrives before `AgentStartedSpeaking`: it is held and given to the reply that opens next, or
//!   to the reply open when it arrives.
//! - **`AgentAudioDone` after a barge-in.** Not stated; both are accepted. After
//!   `UserStartedSpeaking` cuts a reply, its frames up to the next `AgentStartedSpeaking` or
//!   `AgentAudioDone` are dropped and counted.
//! - **Audio before `SettingsApplied`.** Whether it is dropped is not stated; the binding sends
//!   none: a session opens only once `SettingsApplied` arrived.
//! - **Close codes and the idle threshold.** Not stated. Any close is the end of the session,
//!   its code and reason kept; a close before `SettingsApplied` is its own error. The binding
//!   sends `KeepAlive` after [`crate::AgentEndpoint::keepalive_interval`] without audio, and a
//!   WebSocket ping after as long without reading anything; a connection that sends nothing, not
//!   even the pong RFC 6455 requires, for [`crate::AgentEndpoint::read_timeout`] is stalled.
//! - **Function declarations.** The notes name `agent.think.functions` without its fields. The
//!   binding declares each tool of the session as `name`, `description` and `parameters` (the
//!   tool's JSON schema). A `FunctionCallRequest` function without `client_side` is taken as
//!   server-side and passed over; `arguments` is a JSON string, read as JSON where it parses
//!   and kept as a string where it does not.
//! - **Message size.** Not stated. The binding reads messages and frames of at most
//!   [`MAX_MESSAGE_SIZE`] (4 MiB), the bound of the live transcription binding.
//! - **Output pacing.** Not stated; frames are delivered as they arrive.

use std::num::{NonZeroU16, NonZeroU32};

use inference_duplex::SessionConfig;
use inference_duplex::inference_audio::{AudioEncoding, AudioFormat};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::error::ConfigError;
pub use crate::listen::{INBOUND_DEPTH, KEEP_ALIVE, MAX_MESSAGE_SIZE, SERVER_MESSAGE_LIMIT};

/// The path of the Voice Agent endpoint under the base URL.
pub const PATH: &str = "/v1/agent/converse";

/// The sample rates the binding offers for `linear16`, each way.
pub const LINEAR16_RATES: [u32; 7] = [8_000, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000];

/// One provider of the agent's `listen`, `think` or `speak` stage, as `Settings` names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentProvider {
    /// The provider's `type`: `deepgram`, `open_ai`, ...
    #[serde(rename = "type")]
    pub kind: String,
    /// The provider's `version`, where it takes one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub model: String,
}

impl AgentProvider {
    /// A provider of `kind` with `model` and no version.
    pub fn new(kind: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            version: None,
            model: model.into(),
        }
    }

    /// This provider at `version`.
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    fn valid(&self) -> bool {
        let field = |value: &str| !value.trim().is_empty() && value.chars().count() <= 128;
        field(&self.kind) && field(&self.model) && self.version.as_deref().is_none_or(field)
    }
}

/// What a session asks of the Voice Agent beyond its audio formats: the provider of each stage,
/// the prompt and the greeting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentConfig {
    /// Speech recognition: Deepgram Flux, `flux-general-en`, from [`AgentConfig::new`].
    pub listen: AgentProvider,
    /// The language model: OpenAI `gpt-4o-mini`, from [`AgentConfig::new`].
    pub think: AgentProvider,
    /// The prompt when the session's instructions are empty.
    pub prompt: Option<String>,
    /// Speech synthesis: Deepgram, `flux-alexis-en`, from [`AgentConfig::new`]. A session's
    /// non-empty voice replaces its model.
    pub speak: AgentProvider,
    /// What the agent says first, when set.
    pub greeting: Option<String>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentConfig {
    /// The providers the documentation's example names, no prompt and no greeting.
    pub fn new() -> Self {
        Self {
            listen: AgentProvider::new("deepgram", "flux-general-en").with_version("v2"),
            think: AgentProvider::new("open_ai", "gpt-4o-mini"),
            prompt: None,
            speak: AgentProvider::new("deepgram", "flux-alexis-en").with_version("v2"),
            greeting: None,
        }
    }

    /// This configuration with a greeting.
    pub fn with_greeting(mut self, greeting: impl Into<String>) -> Self {
        self.greeting = Some(greeting.into());
        self
    }

    /// This configuration with a prompt for sessions whose instructions are empty.
    pub fn with_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.prompt = Some(prompt.into());
        self
    }

    /// Checks each provider.
    ///
    /// # Errors
    /// [`ConfigError::AgentProvider`] for a provider whose `type`, `model` or `version` is
    /// blank or longer than 128 characters.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if [&self.listen, &self.think, &self.speak]
            .iter()
            .all(|provider| provider.valid())
        {
            Ok(())
        } else {
            Err(ConfigError::AgentProvider)
        }
    }
}

/// The Voice Agent's `encoding` of audio in `format`: `linear16` at one of [`LINEAR16_RATES`],
/// `mulaw` or `alaw` at 8 kHz, mono; `None` for any other.
pub fn encoding(format: AudioFormat) -> Option<&'static str> {
    if format.channels.get() != 1 {
        return None;
    }
    let rate = format.sample_rate.get();
    match format.encoding {
        AudioEncoding::PcmS16le if LINEAR16_RATES.contains(&rate) => Some("linear16"),
        AudioEncoding::PcmMulaw if rate == 8_000 => Some("mulaw"),
        AudioEncoding::PcmAlaw if rate == 8_000 => Some("alaw"),
        _ => None,
    }
}

/// Every format the binding offers, each way.
pub fn audio_formats() -> Vec<AudioFormat> {
    let format = |encoding, rate| AudioFormat {
        encoding,
        sample_rate: NonZeroU32::new(rate).unwrap_or(NonZeroU32::MIN),
        channels: NonZeroU16::MIN,
    };
    let mut formats = vec![
        format(AudioEncoding::PcmMulaw, 8_000),
        format(AudioEncoding::PcmAlaw, 8_000),
    ];
    formats.extend(
        LINEAR16_RATES
            .iter()
            .map(|rate| format(AudioEncoding::PcmS16le, *rate)),
    );
    formats
}

#[derive(Serialize)]
struct Settings<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    audio: Audio,
    agent: Agent<'a>,
}

#[derive(Serialize)]
struct Audio {
    input: Input,
    output: Output,
}

#[derive(Serialize)]
struct Input {
    encoding: &'static str,
    sample_rate: u32,
}

#[derive(Serialize)]
struct Output {
    encoding: &'static str,
    sample_rate: u32,
    container: &'static str,
}

#[derive(Serialize)]
struct Agent<'a> {
    listen: Stage<'a>,
    think: Think<'a>,
    speak: Stage<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    greeting: Option<&'a str>,
}

#[derive(Serialize)]
struct Stage<'a> {
    provider: &'a AgentProvider,
}

#[derive(Serialize)]
struct Think<'a> {
    provider: &'a AgentProvider,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt: Option<&'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    functions: Vec<Function<'a>>,
}

#[derive(Serialize)]
struct Function<'a> {
    name: &'a str,
    description: &'a str,
    parameters: &'a Value,
}

/// The `Settings` message of a session opened with `session` under `config`: the session's
/// formats, its instructions as the prompt (else the configuration's), its voice as the speak
/// model when it names one, and its tools as client-side functions. `None` when a format is not
/// one [`encoding`] takes.
pub fn settings(config: &AgentConfig, session: &SessionConfig) -> Option<String> {
    let speak = if session.voice.trim().is_empty() {
        config.speak.clone()
    } else {
        AgentProvider {
            model: session.voice.clone(),
            ..config.speak.clone()
        }
    };
    let prompt = if session.instructions.trim().is_empty() {
        config.prompt.as_deref()
    } else {
        Some(session.instructions.as_str())
    };
    let message = Settings {
        kind: "Settings",
        audio: Audio {
            input: Input {
                encoding: encoding(session.input_format)?,
                sample_rate: session.input_format.sample_rate.get(),
            },
            output: Output {
                encoding: encoding(session.output_format)?,
                sample_rate: session.output_format.sample_rate.get(),
                container: "none",
            },
        },
        agent: Agent {
            listen: Stage {
                provider: &config.listen,
            },
            think: Think {
                provider: &config.think,
                prompt,
                functions: session
                    .tools
                    .iter()
                    .map(|tool| Function {
                        name: tool.name(),
                        description: tool.description(),
                        parameters: tool.schema(),
                    })
                    .collect(),
            },
            speak: Stage { provider: &speak },
            greeting: config.greeting.as_deref(),
        },
    };
    serde_json::to_string(&message).ok()
}

/// The answer to the client-side function call `id` of the function `name`.
pub fn function_response(id: &str, name: &str, content: &str) -> String {
    serde_json::json!({
        "type": "FunctionCallResponse",
        "id": id,
        "name": name,
        "content": content,
    })
    .to_string()
}

/// The latencies of an `AgentStartedSpeaking`, in seconds, each when it was a finite number or
/// a string that reads as one.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Latency {
    pub total: Option<f64>,
    pub tts: Option<f64>,
    pub ttt: Option<f64>,
}

/// A `Warning` the server sent: its `code` and `description`, at most
/// [`SERVER_MESSAGE_LIMIT`] characters of each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentWarning {
    pub code: Option<String>,
    pub description: String,
}

/// Who said the words of a `ConversationText`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    /// A role the documentation does not name.
    Other,
}

/// One function of a `FunctionCallRequest`.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionCall {
    pub id: String,
    pub name: String,
    /// The arguments, read as JSON where the string parses, else the string itself.
    pub arguments: Value,
    pub client_side: bool,
}

/// A message the server sent, as the binding reads it.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerMessage {
    Welcome {
        request_id: Option<String>,
    },
    SettingsApplied,
    ConversationText {
        role: Role,
        content: String,
    },
    UserStartedSpeaking,
    AgentStartedSpeaking {
        latency: Latency,
    },
    AgentAudioDone,
    FunctionCallRequest {
        functions: Vec<FunctionCall>,
    },
    Warning(AgentWarning),
    /// An `Error`: its `code` and `description`, at most [`SERVER_MESSAGE_LIMIT`] characters of
    /// each.
    Error {
        code: Option<String>,
        description: String,
    },
    /// A message the binding reads and does not act on: `AgentThinking`,
    /// `FunctionCallCancelled`, `PromptUpdated`, `InjectionRefused`, `LatencyReport`, `History`,
    /// or a type it does not know.
    Ignored {
        kind: String,
    },
}

/// The message of a server text, or what made it unreadable. What is returned never quotes the
/// text itself.
///
/// # Errors
/// What the text lacked: it is not a JSON object, has no string `type`, or a typed message
/// misses a field the binding needs.
pub fn parse(text: &str) -> Result<ServerMessage, String> {
    let Ok(Value::Object(object)) = serde_json::from_str::<Value>(text) else {
        return Err(format!(
            "a message of {} bytes that is not a JSON object",
            text.len()
        ));
    };
    let Some(Value::String(kind)) = object.get("type") else {
        return Err("an object without a string `type`".to_owned());
    };
    let string = |field: &str| object.get(field).and_then(Value::as_str);
    Ok(match kind.as_str() {
        "Welcome" => ServerMessage::Welcome {
            request_id: string("request_id").map(limited),
        },
        "SettingsApplied" => ServerMessage::SettingsApplied,
        "ConversationText" => ServerMessage::ConversationText {
            role: match string("role") {
                Some("user") => Role::User,
                Some("assistant") => Role::Assistant,
                _ => Role::Other,
            },
            content: string("content")
                .ok_or("a `ConversationText` without a string `content`")?
                .to_owned(),
        },
        "UserStartedSpeaking" => ServerMessage::UserStartedSpeaking,
        "AgentStartedSpeaking" => ServerMessage::AgentStartedSpeaking {
            latency: Latency {
                total: seconds(&object, "total_latency"),
                tts: seconds(&object, "tts_latency"),
                ttt: seconds(&object, "ttt_latency"),
            },
        },
        "AgentAudioDone" => ServerMessage::AgentAudioDone,
        "FunctionCallRequest" => ServerMessage::FunctionCallRequest {
            functions: functions(&object)?,
        },
        "Warning" => ServerMessage::Warning(AgentWarning {
            code: object.get("code").map(server_text),
            description: object
                .get("description")
                .map(server_text)
                .unwrap_or_default(),
        }),
        "Error" => ServerMessage::Error {
            code: object.get("code").map(server_text),
            description: ["description", "message"]
                .iter()
                .find_map(|name| object.get(*name).map(server_text))
                .unwrap_or_default(),
        },
        other => ServerMessage::Ignored {
            kind: limited(other),
        },
    })
}

fn functions(object: &Map<String, Value>) -> Result<Vec<FunctionCall>, String> {
    let Some(Value::Array(functions)) = object.get("functions") else {
        return Err("a `FunctionCallRequest` without a `functions` array".to_owned());
    };
    functions
        .iter()
        .map(|function| {
            let text = |field: &str| {
                function
                    .get(field)
                    .and_then(Value::as_str)
                    .map(limited)
                    .ok_or_else(|| format!("a `FunctionCallRequest` function without `{field}`"))
            };
            let arguments = match function.get("arguments") {
                Some(Value::String(arguments)) => serde_json::from_str(arguments)
                    .unwrap_or_else(|_| Value::String(arguments.clone())),
                Some(other) => other.clone(),
                None => Value::Null,
            };
            Ok(FunctionCall {
                id: text("id")?,
                name: text("name")?,
                arguments,
                client_side: function
                    .get("client_side")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect()
}

/// A latency field in seconds: a number, or a string that reads as one; finite either way.
fn seconds(object: &Map<String, Value>, field: &str) -> Option<f64> {
    let value = match object.get(field)? {
        Value::Number(number) => number.as_f64()?,
        Value::String(text) => text.trim().parse().ok()?,
        _ => return None,
    };
    value.is_finite().then_some(value)
}

/// At most [`SERVER_MESSAGE_LIMIT`] characters of `text`.
fn limited(text: &str) -> String {
    text.chars().take(SERVER_MESSAGE_LIMIT).collect()
}

/// A server's text field as at most [`SERVER_MESSAGE_LIMIT`] characters: a string as it is, any
/// other value as its JSON text.
fn server_text(value: &Value) -> String {
    match value {
        Value::String(text) => limited(text),
        other => limited(&other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use inference_duplex::{TurnDetection, TurnTuning};

    use super::*;

    fn format(encoding: AudioEncoding, rate: u32) -> AudioFormat {
        AudioFormat {
            encoding,
            sample_rate: NonZeroU32::new(rate).unwrap(),
            channels: NonZeroU16::MIN,
        }
    }

    fn session(input: AudioFormat, output: AudioFormat) -> SessionConfig {
        SessionConfig {
            voice: String::new(),
            instructions: String::new(),
            input_format: input,
            output_format: output,
            turn_detection: TurnDetection::ServerVad,
            turn_tuning: TurnTuning::default(),
            auto_response: true,
            auto_interrupt: true,
            tools: Vec::new(),
            idle_limit: Duration::from_secs(5),
        }
    }

    #[test]
    fn latencies_are_read_from_numbers_and_strings() {
        let message = parse(
            r#"{"type":"AgentStartedSpeaking","total_latency":1.2,"tts_latency":"0.3","ttt_latency":"x"}"#,
        )
        .unwrap();
        assert_eq!(
            message,
            ServerMessage::AgentStartedSpeaking {
                latency: Latency {
                    total: Some(1.2),
                    tts: Some(0.3),
                    ttt: None,
                }
            }
        );
        let message = parse(r#"{"type":"AgentStartedSpeaking","total_latency":"NaN"}"#).unwrap();
        assert_eq!(
            message,
            ServerMessage::AgentStartedSpeaking {
                latency: Latency::default()
            }
        );
    }

    #[test]
    fn every_documented_server_message_is_read() {
        let cases = [
            (
                r#"{"type":"Welcome","request_id":"r1"}"#,
                ServerMessage::Welcome {
                    request_id: Some("r1".to_owned()),
                },
            ),
            (
                r#"{"type":"SettingsApplied"}"#,
                ServerMessage::SettingsApplied,
            ),
            (
                r#"{"type":"ConversationText","role":"assistant","content":"Hi.","conversational_behavior":"default"}"#,
                ServerMessage::ConversationText {
                    role: Role::Assistant,
                    content: "Hi.".to_owned(),
                },
            ),
            (
                r#"{"type":"UserStartedSpeaking"}"#,
                ServerMessage::UserStartedSpeaking,
            ),
            (
                r#"{"type":"AgentAudioDone"}"#,
                ServerMessage::AgentAudioDone,
            ),
            (
                r#"{"type":"Warning","description":"slow","code":"SLOW_THINK_REQUEST"}"#,
                ServerMessage::Warning(AgentWarning {
                    code: Some("SLOW_THINK_REQUEST".to_owned()),
                    description: "slow".to_owned(),
                }),
            ),
            (
                r#"{"type":"Error","description":"gone","code":"SERVER_GOING_AWAY"}"#,
                ServerMessage::Error {
                    code: Some("SERVER_GOING_AWAY".to_owned()),
                    description: "gone".to_owned(),
                },
            ),
            (
                r#"{"type":"LatencyReport","stt_latency":0.1}"#,
                ServerMessage::Ignored {
                    kind: "LatencyReport".to_owned(),
                },
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(parse(text), Ok(expected), "{text}");
        }
        assert_eq!(
            parse(
                r#"{"type":"FunctionCallRequest","functions":[{"id":"f1","name":"t","arguments":"{\"a\":1}","client_side":true},{"id":"f2","name":"u","arguments":"not json"}]}"#
            ),
            Ok(ServerMessage::FunctionCallRequest {
                functions: vec![
                    FunctionCall {
                        id: "f1".to_owned(),
                        name: "t".to_owned(),
                        arguments: serde_json::json!({"a": 1}),
                        client_side: true,
                    },
                    FunctionCall {
                        id: "f2".to_owned(),
                        name: "u".to_owned(),
                        arguments: Value::String("not json".to_owned()),
                        client_side: false,
                    },
                ]
            })
        );
        for unreadable in [
            "not json",
            "[1]",
            r#"{"no":"type"}"#,
            r#"{"type":"ConversationText","role":"user"}"#,
            r#"{"type":"FunctionCallRequest"}"#,
        ] {
            let error = parse(unreadable).expect_err(unreadable);
            assert!(!error.contains(unreadable), "{error}");
        }
    }

    #[test]
    fn server_text_is_bounded() {
        let long = "e".repeat(10_000);
        let Ok(ServerMessage::Error { description, .. }) = parse(&format!(
            r#"{{"type":"Error","description":"{long}","code":5}}"#
        )) else {
            panic!("an error");
        };
        assert_eq!(description.chars().count(), SERVER_MESSAGE_LIMIT);
    }

    #[test]
    fn the_formats_the_voice_agent_takes() {
        assert_eq!(
            encoding(format(AudioEncoding::PcmMulaw, 8_000)),
            Some("mulaw")
        );
        assert_eq!(
            encoding(format(AudioEncoding::PcmAlaw, 8_000)),
            Some("alaw")
        );
        assert_eq!(encoding(format(AudioEncoding::PcmMulaw, 16_000)), None);
        assert_eq!(
            encoding(format(AudioEncoding::PcmS16le, 24_000)),
            Some("linear16")
        );
        assert_eq!(encoding(format(AudioEncoding::PcmS16le, 11_025)), None);
        assert_eq!(encoding(format(AudioEncoding::PcmF32le, 16_000)), None);
        assert_eq!(audio_formats().len(), 9);
        assert!(audio_formats().into_iter().all(|f| encoding(f).is_some()));
    }

    #[test]
    fn settings_serialise_the_session_and_the_configuration() {
        let mut config = session(
            format(AudioEncoding::PcmMulaw, 8_000),
            format(AudioEncoding::PcmMulaw, 8_000),
        );
        config.voice = "aura-2-thalia-en".to_owned();
        config.instructions = "Be \"brief\".".to_owned();
        let text = settings(&AgentConfig::new(), &config).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["type"], "Settings");
        assert_eq!(
            value["audio"],
            serde_json::json!({
                "input": {"encoding": "mulaw", "sample_rate": 8000},
                "output": {"encoding": "mulaw", "sample_rate": 8000, "container": "none"},
            })
        );
        assert_eq!(
            value["agent"]["listen"]["provider"],
            serde_json::json!({"type": "deepgram", "version": "v2", "model": "flux-general-en"})
        );
        assert_eq!(value["agent"]["think"]["prompt"], "Be \"brief\".");
        assert!(value["agent"]["think"].get("functions").is_none());
        assert_eq!(
            value["agent"]["speak"]["provider"]["model"],
            "aura-2-thalia-en"
        );
        assert!(value["agent"].get("greeting").is_none());
        let unsupported = session(
            format(AudioEncoding::PcmF32le, 16_000),
            format(AudioEncoding::PcmS16le, 16_000),
        );
        assert_eq!(settings(&AgentConfig::new(), &unsupported), None);
    }

    #[test]
    fn a_blank_provider_is_refused() {
        assert_eq!(AgentConfig::new().validate(), Ok(()));
        let mut config = AgentConfig::new();
        config.think.model = " ".to_owned();
        assert_eq!(config.validate(), Err(ConfigError::AgentProvider));
    }
}
