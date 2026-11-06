//! The text-to-speech WebSocket protocol, as this binding speaks it.
//!
//! `docs/protocol.md` in this crate names every page of Cartesia's public documentation it was
//! taken from, with the date it was fetched (2026-11-06), says what each gave in our words, and
//! lists what the documentation leaves open with what the binding decided.
//!
//! In short: one connection per synthesis to `{base}/tts/websocket?cartesia_version=…`, the key
//! in the `X-API-Key` header of the handshake. Each segment is one generation request on a
//! context of its own, `continue` false ([`generation_request`]); the server answers with
//! `chunk` messages carrying base64 audio and then `done` for that context. A cancel is
//! [`cancel_request`] for the context; the server may still finish a context it had begun, so
//! the binding drops whatever else comes of it.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Map, Value, json};

use crate::config::{API_VERSION, Endpoint, VoiceConfig};
use crate::error::{CartesiaError, ConfigError, ServerError};

/// The path of the endpoint under the base URL.
pub const PATH: &str = "/tts/websocket";

/// The most characters of a server's error title or message an error keeps.
pub const SERVER_MESSAGE_LIMIT: usize = 256;

/// The URL a synthesis connects to at `endpoint`.
///
/// # Errors
/// [`ConfigError::InvalidUrl`] when the base URL is not `wss://`, or `ws://` to a loopback host
/// (`127.0.0.0/8`, `::1`, `localhost`), or carries a query or fragment.
pub fn request_url(endpoint: &Endpoint) -> Result<String, ConfigError> {
    let base = endpoint.url.trim_end_matches('/');
    if let Some(rest) = base.strip_prefix("ws://") {
        if !is_loopback(rest) {
            return Err(ConfigError::InvalidUrl(
                "ws:// would send the key in the clear; it is taken only to a loopback host, use wss://"
                    .to_owned(),
            ));
        }
    } else if !base.starts_with("wss://") {
        return Err(ConfigError::InvalidUrl(
            "the scheme must be wss://, or ws:// to a loopback host".to_owned(),
        ));
    }
    if base.contains('?') || base.contains('#') {
        return Err(ConfigError::InvalidUrl(
            "the base URL carries a query or fragment; the binding adds its own".to_owned(),
        ));
    }
    Ok(format!("{base}{PATH}?cartesia_version={API_VERSION}"))
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

/// The generation request of one segment's `text` on `context_id`: the model, the voice, the raw
/// output format, the language when configured, and `continue` false, since the segment is the
/// context's whole transcript. `None` for text that is blank, which is not sent.
pub fn generation_request(config: &VoiceConfig, context_id: &str, text: &str) -> Option<String> {
    let transcript = text.trim();
    if transcript.is_empty() {
        return None;
    }
    let format = config.output_format;
    let mut message = json!({
        "model_id": config.model_id,
        "transcript": transcript,
        "voice": config.voice_id,
        "output_format": {
            "container": "raw",
            "encoding": format.encoding_name(),
            "sample_rate": format.sample_rate(),
        },
        "context_id": context_id,
        "continue": false,
    });
    if let Some(language) = &config.language {
        message["language"] = Value::from(language.as_str());
    }
    Some(message.to_string())
}

/// The cancel of `context_id`.
pub fn cancel_request(context_id: &str) -> String {
    json!({"context_id": context_id, "cancel": true}).to_string()
}

/// One message the server sent, as the binding reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerMessage {
    /// A `chunk`: the decoded audio, and whether it is the context's final chunk.
    Chunk {
        context_id: String,
        audio: Vec<u8>,
        done: bool,
    },
    /// `done`: the context's generation is complete.
    Done { context_id: String },
    /// An `error`, and the context it names when it names one.
    Error {
        context_id: Option<String>,
        error: ServerError,
    },
    /// `flush_done`, `timestamps` or `phoneme_timestamps`, which answer options the binding
    /// never sets.
    Ignored { context_id: String },
}

impl ServerMessage {
    /// The context the message names.
    pub fn context_id(&self) -> Option<&str> {
        match self {
            Self::Chunk { context_id, .. }
            | Self::Done { context_id }
            | Self::Ignored { context_id } => Some(context_id),
            Self::Error { context_id, .. } => context_id.as_deref(),
        }
    }
}

/// Reads one text message of the server.
///
/// # Errors
/// [`CartesiaError::Protocol`] for text that is not a JSON object, a `type` that is missing or
/// unknown, a missing `context_id` where the type requires one, or a chunk whose `data` is not a
/// base64 string or whose `done` is not a boolean. The error never quotes the message.
pub fn parse_server_message(text: &str) -> Result<ServerMessage, CartesiaError> {
    let Ok(Value::Object(object)) = serde_json::from_str::<Value>(text) else {
        return Err(CartesiaError::Protocol(format!(
            "a message of {} bytes that is not a JSON object",
            text.len()
        )));
    };
    let context = || match object.get("context_id") {
        Some(Value::String(context_id)) => Ok(context_id.clone()),
        _ => Err(CartesiaError::Protocol(
            "a message without a string `context_id`".to_owned(),
        )),
    };
    match object.get("type").and_then(Value::as_str) {
        Some("chunk") => {
            let Some(Value::String(encoded)) = object.get("data") else {
                return Err(CartesiaError::Protocol(
                    "a chunk whose `data` is not a string".to_owned(),
                ));
            };
            let audio = STANDARD.decode(encoded).map_err(|_| {
                CartesiaError::Protocol(format!(
                    "`data` of {} characters is not base64",
                    encoded.len()
                ))
            })?;
            let done = match object.get("done") {
                None | Some(Value::Null) => false,
                Some(Value::Bool(done)) => *done,
                Some(_) => {
                    return Err(CartesiaError::Protocol(
                        "a chunk whose `done` is not a boolean".to_owned(),
                    ));
                }
            };
            Ok(ServerMessage::Chunk {
                context_id: context()?,
                audio,
                done,
            })
        }
        Some("done") => Ok(ServerMessage::Done {
            context_id: context()?,
        }),
        Some("error") => Ok(ServerMessage::Error {
            context_id: context().ok(),
            error: server_error(&object),
        }),
        Some("flush_done" | "timestamps" | "phoneme_timestamps") => Ok(ServerMessage::Ignored {
            context_id: context()?,
        }),
        Some(_) => Err(CartesiaError::Protocol(
            "a message of an unknown `type`".to_owned(),
        )),
        None => Err(CartesiaError::Protocol(
            "a message without a string `type`".to_owned(),
        )),
    }
}

/// The error an `error` message carries, its texts cut to [`SERVER_MESSAGE_LIMIT`] characters.
fn server_error(object: &Map<String, Value>) -> ServerError {
    let text = |name: &str| {
        object
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .chars()
            .take(SERVER_MESSAGE_LIMIT)
            .collect::<String>()
    };
    ServerError {
        status: object
            .get("status_code")
            .and_then(Value::as_f64)
            .filter(|status| status.fract() == 0.0 && (100.0..=599.0).contains(status))
            .map(|status| status as u16),
        code: object
            .get("error_code")
            .and_then(Value::as_str)
            .map(|code| code.chars().take(SERVER_MESSAGE_LIMIT).collect()),
        title: text("title"),
        message: text("message"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::OutputFormat;

    #[test]
    fn the_url_names_the_path_and_the_version_and_plaintext_only_to_loopback() {
        let endpoint = Endpoint::new().with_url("ws://127.0.0.1:9/");
        assert_eq!(
            request_url(&endpoint).unwrap(),
            "ws://127.0.0.1:9/tts/websocket?cartesia_version=2026-08-14"
        );
        assert_eq!(
            request_url(&Endpoint::new()).unwrap(),
            "wss://api.cartesia.ai/tts/websocket?cartesia_version=2026-08-14"
        );
        for taken in ["ws://localhost:8080", "ws://[::1]:9", "wss://10.0.0.1"] {
            assert!(
                request_url(&Endpoint::new().with_url(taken)).is_ok(),
                "{taken}"
            );
        }
        for refused in [
            "ws://api.cartesia.ai",
            "ws://127.0.0.1@evil.example",
            "ws://[::1]evil",
            "http://127.0.0.1",
            "wss://api.cartesia.ai?x=1",
        ] {
            assert!(
                matches!(
                    request_url(&Endpoint::new().with_url(refused)),
                    Err(ConfigError::InvalidUrl(_))
                ),
                "{refused}"
            );
        }
    }

    #[test]
    fn a_request_carries_the_format_the_context_and_continue_false_and_blank_text_is_not_sent() {
        let config = VoiceConfig::new("voice-1", OutputFormat::MULAW_8000).with_language("en");
        let message: Value =
            serde_json::from_str(&generation_request(&config, "s1", " Hello there. \n").unwrap())
                .unwrap();
        assert_eq!(
            message,
            json!({
                "model_id": "sonic-3.6",
                "transcript": "Hello there.",
                "voice": "voice-1",
                "output_format": {"container": "raw", "encoding": "pcm_mulaw", "sample_rate": 8000},
                "context_id": "s1",
                "continue": false,
                "language": "en",
            })
        );
        assert_eq!(generation_request(&config, "s1", " \t "), None);
        assert_eq!(
            serde_json::from_str::<Value>(&cancel_request("s1")).unwrap(),
            json!({"context_id": "s1", "cancel": true})
        );
    }

    #[test]
    fn every_documented_message_is_read_and_anything_else_is_refused() {
        assert_eq!(
            parse_server_message(
                r#"{"type":"chunk","data":"AAEC","done":false,"status_code":206,"step_time":1,"context_id":"s1"}"#
            )
            .unwrap(),
            ServerMessage::Chunk {
                context_id: "s1".to_owned(),
                audio: vec![0, 1, 2],
                done: false
            }
        );
        assert_eq!(
            parse_server_message(
                r#"{"type":"done","done":true,"status_code":206,"context_id":"s1"}"#
            )
            .unwrap(),
            ServerMessage::Done {
                context_id: "s1".to_owned()
            }
        );
        assert_eq!(
            parse_server_message(
                r#"{"type":"error","done":true,"status_code":429,"error_code":"concurrency_limited","title":"Too many","message":"Slow down.","context_id":"s1"}"#
            )
            .unwrap(),
            ServerMessage::Error {
                context_id: Some("s1".to_owned()),
                error: ServerError {
                    status: Some(429),
                    code: Some("concurrency_limited".to_owned()),
                    title: "Too many".to_owned(),
                    message: "Slow down.".to_owned(),
                }
            }
        );
        assert!(matches!(
            parse_server_message(
                r#"{"type":"timestamps","done":false,"status_code":206,"context_id":"s1"}"#
            ),
            Ok(ServerMessage::Ignored { .. })
        ));
        for refused in [
            "not json",
            "[1]",
            r#"{"type":"chunk","data":5,"context_id":"s1"}"#,
            r#"{"type":"chunk","data":"@@","context_id":"s1"}"#,
            r#"{"type":"chunk","data":"AA==","done":"yes","context_id":"s1"}"#,
            r#"{"type":"chunk","data":"AA=="}"#,
            r#"{"type":"something","context_id":"s1"}"#,
            r#"{"context_id":"s1"}"#,
        ] {
            assert!(
                matches!(
                    parse_server_message(refused),
                    Err(CartesiaError::Protocol(_))
                ),
                "{refused}"
            );
        }
    }
}
