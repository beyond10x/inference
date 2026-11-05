//! The single-stream text-to-speech WebSocket protocol, as this binding speaks it.
//!
//! # Source
//!
//! Read on 2026-11-05 from ElevenLabs' public API documentation. None of the pages carries a
//! publication or revision date of its own, so the retrieval date is the only date there is.
//!
//! - <https://elevenlabs.io/docs/api-reference/text-to-speech/v-1-text-to-speech-voice-id-stream-input>,
//!   the AsyncAPI description of the single-stream endpoint;
//! - <https://elevenlabs.io/docs/api-reference/text-to-speech/v-1-text-to-speech-voice-id-multi-stream-input>,
//!   the multi-stream endpoint, which this binding does not use;
//! - <https://elevenlabs.io/docs/eleven-api/guides/how-to/websockets/realtime-tts.md>, the
//!   realtime guide;
//! - <https://elevenlabs.io/docs/eleven-api/guides/how-to/websockets/multi-context-web-socket.md>,
//!   the multi-context guide;
//! - <https://elevenlabs.io/docs/eleven-api/guides/how-to/best-practices/latency-optimization.md>,
//!   the latency guide;
//! - <https://elevenlabs.io/docs/overview/models.md>, the models;
//! - <https://elevenlabs.io/docs/eleven-api/resources/errors.md>, the error envelope;
//! - <https://elevenlabs.io/docs/api-reference/authentication.md>, the API key header;
//! - <https://elevenlabs.io/docs/api-reference/tokens/create.md>, single-use tokens, not used;
//! - <https://elevenlabs.io/docs/api-reference/text-to-speech/convert.md>, the REST endpoint,
//!   the only page with a list of `output_format` values;
//! - <https://elevenlabs.io/docs/help-center/troubleshooting/what-audio-formats-do-you-support.md>,
//!   the audio formats.
//!
//! # The connection
//!
//! One WebSocket per segment, to
//! `{base}/v1/text-to-speech/{voice_id}/stream-input?model_id=…&output_format=…`, with
//! `auto_mode` and `inactivity_timeout` appended when they are configured. The API key travels in
//! the `xi-api-key` request header of the handshake and nowhere else; the documentation also
//! allows it in the first message, which this binding never does.
//!
//! # What the binding sends
//!
//! 1. The opening message: `text` is a single space, which the endpoint requires of the first
//!    message, with the `voice_settings` that are configured ([`init_message`]).
//! 2. The segment's text, its trailing whitespace replaced by exactly one space because the
//!    endpoint expects every text chunk to end in one, with `flush` set so the endpoint generates
//!    what it holds at once instead of waiting for its chunk schedule ([`text_message`]).
//! 3. The end of input: an empty `text` ([`END_OF_INPUT`]). After it the endpoint finishes the
//!    audio, sends the final message and closes.
//!
//! # What the binding reads
//!
//! - Audio: an object whose `audio` is the base64 of a chunk in the requested output format.
//!   `alignment` and `normalizedAlignment`, when present, are ignored.
//! - The final message: `isFinal` true, `audio` null. It ends the segment.
//! - Anything else ends the synthesis as failed ([`parse_server_message`]).
//!
//! # Points the documentation leaves open, and what the binding decided
//!
//! - **Where one segment's audio ends.** The single stream marks no boundary between flushes;
//!   the only end marker is the final message after the end of input, after which the endpoint
//!   closes. So a segment cannot share a connection with the next and still have its own audio:
//!   the binding opens one connection per segment. The multi-stream endpoint would mark each
//!   context's end, at the price of a protocol whose two initialization messages the
//!   documentation does not tell apart.
//! - **PCM sample width, byte order and channels.** Not stated for any `pcm_*` format. The
//!   binding takes them as signed 16-bit little-endian mono; a chunk with an odd byte count
//!   keeps its last byte for the next chunk, and a segment that ends on half a sample fails.
//! - **μ-law and A-law.** Named as 8 kHz; channels not stated. Taken as mono, one byte per
//!   sample, which the refusal of stereo effects for these formats implies.
//! - **`output_format` on the WebSocket.** The WebSocket pages give it no type or list; the
//!   binding sends the REST names it maps, as a query parameter.
//! - **Errors on the WebSocket.** No error frame or close code is defined. A JSON object that
//!   carries `message`, `error` or `detail` and no audio (no `audio`, or one that is null or
//!   empty) is read as the server's error; a close before the final message is a failure
//!   carrying the close code and reason.
//! - **Plaintext.** The key travels in a header, so `ws://` would send it in the clear. The
//!   binding takes `ws://` only to a loopback host (`127.0.0.0/8`, `::1`, `localhost`), as a
//!   local test server is, and `wss://` to any host.
//! - **Cancel.** The single stream has no cancel message, and closing it makes the endpoint
//!   generate whatever it still holds. The binding cancels by sending a close frame and dropping
//!   the connection without reading anything more from it.
//! - **`isFinal` spelling.** One guide reads `is_final`; both specifications and the realtime
//!   guide say `isFinal`, which is what the binding reads.
//! - **The model.** The specification's default is a slower model than the latency guide
//!   recommends, so the binding always names one, `eleven_flash_v2_5` unless configured.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Map, Value, json};

use crate::config::{Endpoint, VoiceConfig, VoiceSettings};
use crate::error::{ConfigError, ElevenLabsError};

/// The path of the single-stream endpoint under the base URL, `{voice_id}` filled in.
pub const PATH: &str = "/v1/text-to-speech/{voice_id}/stream-input";

/// The message that ends the input of a connection.
pub const END_OF_INPUT: &str = r#"{"text":""}"#;

/// The most characters of a server's error message an error keeps.
pub const SERVER_MESSAGE_LIMIT: usize = 256;

/// The URL a synthesis connects to for `config` at `endpoint`.
///
/// # Errors
/// [`ConfigError::InvalidUrl`] when the base URL is not `wss://`, or `ws://` to a loopback host
/// (`127.0.0.0/8`, `::1`, `localhost`), or carries a query.
pub fn request_url(endpoint: &Endpoint, config: &VoiceConfig) -> Result<String, ConfigError> {
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
    let mut url = format!(
        "{base}{}?model_id={}&output_format={}",
        PATH.replace("{voice_id}", &config.voice_id),
        config.model_id,
        config.output_format.as_str()
    );
    if let Some(auto_mode) = config.auto_mode {
        url.push_str(&format!("&auto_mode={auto_mode}"));
    }
    if let Some(seconds) = config.inactivity_timeout {
        url.push_str(&format!("&inactivity_timeout={seconds}"));
    }
    Ok(url)
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

/// The opening message: `text` a single space, and the configured `voice_settings`.
pub fn init_message(settings: Option<&VoiceSettings>) -> String {
    let mut message = json!({"text": " "});
    if let Some(settings) = settings {
        let mut object = Map::new();
        let mut put = |name: &str, value: Option<Value>| {
            if let Some(value) = value {
                object.insert(name.to_owned(), value);
            }
        };
        put("stability", settings.stability.map(Value::from));
        put(
            "similarity_boost",
            settings.similarity_boost.map(Value::from),
        );
        put("style", settings.style.map(Value::from));
        put(
            "use_speaker_boost",
            settings.use_speaker_boost.map(Value::from),
        );
        put("speed", settings.speed.map(Value::from));
        message["voice_settings"] = Value::Object(object);
    }
    message.to_string()
}

/// The text message of one segment: its text, trailing whitespace replaced by one space, with
/// `flush` set. `None` for text that is blank, which would read as the end of input.
pub fn text_message(text: &str) -> Option<String> {
    let trimmed = text.trim_end();
    if trimmed.trim_start().is_empty() {
        return None;
    }
    Some(json!({"text": format!("{trimmed} "), "flush": true}).to_string())
}

/// One message the server sent, as the binding reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerMessage {
    /// The decoded bytes of `audio`, when it is a non-empty string.
    pub audio: Option<Vec<u8>>,
    /// Whether `isFinal` is true.
    pub is_final: bool,
}

/// Reads one text message of the server.
///
/// # Errors
/// [`ElevenLabsError::Server`] for an object carrying `message`, `error` or `detail` and no audio,
/// where audio is an `audio` that is a non-empty string, so an error whose `audio` is null or
/// empty is the server's error too; [`ElevenLabsError::Protocol`] for text that is not a JSON
/// object, an `audio` that is neither a string nor null or not base64, an `isFinal` that is
/// neither a boolean nor null, and an object with neither field. The error never quotes the
/// message itself, only the server's own error text.
pub fn parse_server_message(text: &str) -> Result<ServerMessage, ElevenLabsError> {
    let Ok(Value::Object(object)) = serde_json::from_str::<Value>(text) else {
        return Err(ElevenLabsError::Protocol(format!(
            "a message of {} bytes that is not a JSON object",
            text.len()
        )));
    };
    let carries_audio =
        matches!(object.get("audio"), Some(Value::String(encoded)) if !encoded.is_empty());
    if !carries_audio {
        if let Some(said) = ["message", "error", "detail"]
            .iter()
            .find_map(|name| object.get(*name).map(server_text))
        {
            return Err(ElevenLabsError::Server(said));
        }
        if !object.contains_key("audio") && !object.contains_key("isFinal") {
            return Err(ElevenLabsError::Protocol(
                "an object with neither `audio` nor `isFinal`".to_owned(),
            ));
        }
    }
    let audio = match object.get("audio") {
        None | Some(Value::Null) => None,
        Some(Value::String(encoded)) if encoded.is_empty() => None,
        Some(Value::String(encoded)) => Some(STANDARD.decode(encoded).map_err(|_| {
            ElevenLabsError::Protocol(format!(
                "`audio` of {} characters is not base64",
                encoded.len()
            ))
        })?),
        Some(_) => {
            return Err(ElevenLabsError::Protocol(
                "`audio` is neither a string nor null".to_owned(),
            ));
        }
    };
    let is_final = match object.get("isFinal") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(is_final)) => *is_final,
        Some(_) => {
            return Err(ElevenLabsError::Protocol(
                "`isFinal` is neither a boolean nor null".to_owned(),
            ));
        }
    };
    Ok(ServerMessage { audio, is_final })
}

/// The server's error text: a string as it is, an object's `message` or else the whole value,
/// cut to [`SERVER_MESSAGE_LIMIT`] characters.
fn server_text(value: &Value) -> String {
    let text = match value {
        Value::String(text) => text.clone(),
        Value::Object(object) => match object.get("message") {
            Some(Value::String(text)) => text.clone(),
            _ => value.to_string(),
        },
        other => other.to_string(),
    };
    text.chars().take(SERVER_MESSAGE_LIMIT).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::OutputFormat;

    #[test]
    fn the_url_names_the_voice_the_model_and_the_format_and_only_the_options_set() {
        let mut config = VoiceConfig::new("voice1", OutputFormat::Ulaw8000);
        let endpoint = Endpoint::new().with_url("ws://127.0.0.1:9/");
        assert_eq!(
            request_url(&endpoint, &config).unwrap(),
            "ws://127.0.0.1:9/v1/text-to-speech/voice1/stream-input?model_id=eleven_flash_v2_5&output_format=ulaw_8000"
        );
        config.auto_mode = Some(true);
        config.inactivity_timeout = Some(60);
        assert!(
            request_url(&endpoint, &config)
                .unwrap()
                .ends_with("&auto_mode=true&inactivity_timeout=60")
        );
        for bad in ["http://host", "wss://host?x=1"] {
            assert!(matches!(
                request_url(&Endpoint::new().with_url(bad), &config),
                Err(ConfigError::InvalidUrl(_))
            ));
        }
    }

    #[test]
    fn plaintext_is_taken_only_to_a_loopback_host() {
        let config = VoiceConfig::new("voice1", OutputFormat::Ulaw8000);
        for taken in [
            "ws://127.0.0.1:9",
            "ws://127.8.9.10",
            "ws://localhost:8080/",
            "ws://LOCALHOST",
            "ws://[::1]:9",
            "wss://api.elevenlabs.io",
            "wss://10.0.0.1:443",
        ] {
            assert!(
                request_url(&Endpoint::new().with_url(taken), &config).is_ok(),
                "{taken}"
            );
        }
        for refused in [
            "ws://api.elevenlabs.io",
            "ws://10.0.0.1:9",
            "ws://128.0.0.1",
            "ws://[::2]:9",
            "ws://localhost.example.com",
            "ws://127.0.0.1@evil.example",
            "ws://[::1]evil",
            "ws://",
        ] {
            match request_url(&Endpoint::new().with_url(refused), &config) {
                Err(ConfigError::InvalidUrl(detail)) => assert!(detail.contains("ws://")),
                other => panic!("{refused}: {other:?}"),
            }
        }
    }

    #[test]
    fn an_error_without_audio_is_the_servers_whatever_audio_says() {
        for message in [
            r#"{"audio":null,"error":"quota_exceeded"}"#,
            r#"{"audio":"","message":"quota_exceeded"}"#,
            r#"{"audio":null,"isFinal":false,"detail":"quota_exceeded"}"#,
            r#"{"error":"quota_exceeded"}"#,
        ] {
            assert_eq!(
                parse_server_message(message),
                Err(ElevenLabsError::Server("quota_exceeded".to_owned())),
                "{message}"
            );
        }
        assert_eq!(
            parse_server_message(r#"{"audio":"AAEC","message":"chunk"}"#).unwrap(),
            ServerMessage {
                audio: Some(vec![0, 1, 2]),
                is_final: false
            },
            "audio with a message is audio"
        );
        assert_eq!(
            parse_server_message(r#"{"audio":""}"#).unwrap(),
            ServerMessage {
                audio: None,
                is_final: false
            }
        );
    }

    #[test]
    fn the_opening_message_is_one_space_with_only_the_settings_set() {
        assert_eq!(init_message(None), r#"{"text":" "}"#);
        let settings = VoiceSettings {
            stability: Some(0.5),
            use_speaker_boost: Some(false),
            ..VoiceSettings::default()
        };
        let message: Value = serde_json::from_str(&init_message(Some(&settings))).unwrap();
        assert_eq!(
            message,
            json!({"text": " ", "voice_settings": {"stability": 0.5, "use_speaker_boost": false}})
        );
    }

    #[test]
    fn a_text_ends_in_exactly_one_space_with_flush_and_blank_text_is_not_sent() {
        let message: Value =
            serde_json::from_str(&text_message("Hello there.  \n").unwrap()).unwrap();
        assert_eq!(message, json!({"text": "Hello there. ", "flush": true}));
        assert_eq!(text_message(" \t "), None);
        assert_eq!(text_message(""), None);
    }

    #[test]
    fn audio_and_the_final_message_are_read_and_everything_else_is_refused() {
        assert_eq!(
            parse_server_message(r#"{"audio":"AAEC","alignment":null}"#).unwrap(),
            ServerMessage {
                audio: Some(vec![0, 1, 2]),
                is_final: false
            }
        );
        assert_eq!(
            parse_server_message(r#"{"audio":null,"isFinal":true}"#).unwrap(),
            ServerMessage {
                audio: None,
                is_final: true
            }
        );
        for refused in [
            "not json",
            "[1]",
            r#"{"audio":5}"#,
            r#"{"audio":"@@"}"#,
            r#"{"isFinal":"yes"}"#,
            r#"{"other":1}"#,
        ] {
            assert!(
                matches!(
                    parse_server_message(refused),
                    Err(ElevenLabsError::Protocol(_))
                ),
                "{refused}"
            );
        }
        assert_eq!(
            parse_server_message(r#"{"error":"quota_exceeded"}"#),
            Err(ElevenLabsError::Server("quota_exceeded".to_owned()))
        );
        assert_eq!(
            parse_server_message(r#"{"detail":{"message":"no"}}"#),
            Err(ElevenLabsError::Server("no".to_owned()))
        );
    }
}
