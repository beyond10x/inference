//! The multi-stream text-to-speech WebSocket protocol, as this binding speaks it.
//!
//! # Source
//!
//! Read on 2026-11-06 from ElevenLabs' public API documentation, fetched without a key. None of
//! the pages carries a publication or revision date of its own, so the retrieval date is the
//! only date there is.
//!
//! - <https://elevenlabs.io/docs/api-reference/text-to-speech/v-1-text-to-speech-voice-id-multi-stream-input>,
//!   the AsyncAPI description of the multi-stream endpoint, which this binding speaks;
//! - <https://elevenlabs.io/docs/eleven-api/guides/how-to/websockets/multi-context-web-socket.md>,
//!   the multi-context guide, with its worked conversation;
//!
//! and, read on 2026-11-05 for the single-stream binding this one replaced and still the source
//! of what both endpoints share:
//!
//! - <https://elevenlabs.io/docs/api-reference/text-to-speech/v-1-text-to-speech-voice-id-stream-input>,
//!   the single-stream endpoint;
//! - <https://elevenlabs.io/docs/eleven-api/guides/how-to/websockets/realtime-tts.md>, the
//!   realtime guide;
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
//! One WebSocket per synthesis, opened when its first non-blank segment is spoken, to
//! `{base}/v1/text-to-speech/{voice_id}/multi-stream-input?model_id=…&output_format=…`, with
//! `auto_mode` and `inactivity_timeout` appended when they are configured. The API key travels in
//! the `xi-api-key` request header of the handshake and nowhere else; the documentation also
//! allows it in a context's first message, which this binding never does.
//!
//! # What the binding sends
//!
//! Each segment is one context of the connection, named `s0`, `s1`, … in the order the
//! synthesis speaks them, so no name is ever used twice on a connection:
//!
//! 1. The context's opening message: `text` a single space and the `context_id`, with the
//!    `voice_settings` that are configured ([`context_message`]). A single space is what the
//!    specification's connection initialization requires and is a text ending in one space, as
//!    its context initialization asks, so the message is both.
//! 2. The segment's text, its trailing whitespace replaced by exactly one space, with the
//!    `context_id` and `flush` set, so the endpoint generates what it holds at once
//!    ([`text_message`]).
//! 3. `close_context` for the context ([`close_context_message`]). The specification: "If it
//!    has already been set to flush it will continue flushing"; the guide's worked conversation
//!    flushes and closes a context the same way. The endpoint then finishes the audio of the
//!    context and marks its end with its final message.
//!
//! The end of the utterance sends `close_socket` ([`CLOSE_SOCKET`]), which "closes all contexts
//! and closes the entire WebSocket connection", then a close frame. Dropping the handle does the
//! same.
//!
//! # What the binding reads
//!
//! - Audio: an object whose `audio` is the base64 of a chunk in the requested output format and
//!   whose `contextId` names the context. `alignment` and `normalizedAlignment`, when present,
//!   are ignored.
//! - The final message of a context: `isFinal` (or `is_final`, see below) true, with its
//!   `contextId`. It ends the segment.
//! - Anything else ends the synthesis as failed ([`parse_server_message`]), and so does a
//!   message of a context the binding is not speaking and has not cancelled.
//!
//! # Cancel
//!
//! A discard of the segment being spoken, as a barge-in makes it, ends that context for the
//! binding: it sends nothing, since the context's `close_context` already went with its text and
//! the specification defines no second close of a closing context, and it drops every later
//! message of that context, its final message included, without reading it as audio. The
//! connection stays open, and the next segment opens the next context on it. Messages of a
//! cancelled context move no read deadline, so a server that keeps streaming one cannot hold the
//! next segment past the read timeout.
//!
//! A cancel stops nothing at the server: the context was flushed and closed when its text was
//! sent, and the specification says a context set to flush "will continue flushing" when closed.
//! So the endpoint still generates, sends and bills the whole cancelled segment; a barge-in saves
//! playback, not cost.
//!
//! # Points the documentation leaves open, and what the binding decided
//!
//! - **When a context's final message comes.** The schema says only that it "indicates this is
//!   the final message for the context". The binding closes each context right after its flush,
//!   as the guide's worked conversation does, and reads the final message as the end of the
//!   segment. Whether a flush alone, without a close, would bring one is not stated, so the
//!   binding never relies on it.
//! - **A second `close_context`.** Not described; a cancel therefore sends none (see Cancel).
//! - **The two initialization messages.** The specification lists a connection initialization
//!   (`text` must be a single space) and a context initialization (`text` should end with a
//!   single space) and does not say how the endpoint tells them apart. The opening message of
//!   every context is a single space with its `context_id`, which fits both.
//! - **`contextId` on server messages.** Optional in the schema; the guide's example falls back
//!   to a default context when it is absent. The binding always names its contexts, so a message
//!   without `contextId` is a protocol failure rather than a guess.
//! - **An idle connection.** Contexts time out after `inactivity_timeout` (20 s unless
//!   configured, at most 180 s). Whether the connection itself closes when no context is open is
//!   not stated. A synthesis can outlive a turn, so the gap between two segments can pass the
//!   timeout: when sending a context fails because the connection ended, or the connection ends
//!   before any message of the context arrives, the binding opens a new connection once and sends
//!   that context again on it; a second failure fails the synthesis. Contexts cancelled on the
//!   old connection are forgotten with it.
//! - **Five contexts per connection.** The guide caps concurrent contexts at five. The binding
//!   speaks one at a time; a cancelled context the endpoint is still flushing counts until its
//!   final message, so more than four barge-ins within one context's flush could meet the cap,
//!   whose answer the documentation does not give.
//! - **PCM sample width, byte order and channels.** Not stated for any `pcm_*` format. The
//!   binding takes them as signed 16-bit little-endian mono; a chunk with an odd byte count
//!   keeps its last byte for the next chunk, and a segment that ends on half a sample fails.
//! - **μ-law and A-law.** Named as 8 kHz; channels not stated. Taken as mono, one byte per
//!   sample, which the refusal of stereo effects for these formats implies.
//! - **`output_format` on the WebSocket.** The WebSocket pages give it no type or list; the
//!   binding sends the REST names it maps, as a query parameter.
//! - **Errors on the WebSocket.** No error frame or close code is defined. A JSON object that
//!   carries `message`, `error` or `detail` and no audio (no `audio`, or one that is null or
//!   empty) is read as the server's error; a close before the end of the utterance is a failure
//!   carrying the close code and reason.
//! - **Plaintext.** The key travels in a header, so `ws://` would send it in the clear. The
//!   binding takes `ws://` only to a loopback host (`127.0.0.0/8`, `::1`, `localhost`), as a
//!   local test server is, and `wss://` to any host.
//! - **The final flag's spelling.** The documentation contradicts itself: the AsyncAPI page
//!   names it `isFinal`, while the multi-context guide's Python and JavaScript examples both read
//!   `is_final`. The binding takes either as the final flag ([`FINAL_SPELLINGS`]); a message
//!   whose flag is true under either spelling ends its context.
//! - **The model.** The specification's default is a slower model than the latency guide
//!   recommends, so the binding always names one, `eleven_flash_v2_5` unless configured.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Map, Value, json};

use crate::config::{Endpoint, VoiceConfig, VoiceSettings};
use crate::error::{ConfigError, ElevenLabsError};

/// The path of the multi-stream endpoint under the base URL, `{voice_id}` filled in.
pub const PATH: &str = "/v1/text-to-speech/{voice_id}/multi-stream-input";

/// The message that closes every context and the connection, at the end of an utterance.
pub const CLOSE_SOCKET: &str = r#"{"close_socket":true}"#;

/// The spellings of the final flag the binding reads: `isFinal`, as the specification names it,
/// and `is_final`, as the multi-context guide's examples read it.
pub const FINAL_SPELLINGS: [&str; 2] = ["isFinal", "is_final"];

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
pub(crate) fn is_loopback(rest: &str) -> bool {
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

/// The opening message of the context `context_id`: `text` a single space, and the configured
/// `voice_settings`.
pub fn context_message(context_id: &str, settings: Option<&VoiceSettings>) -> String {
    let mut message = json!({"text": " ", "context_id": context_id});
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

/// The text message of one segment in the context `context_id`: its text, trailing whitespace
/// replaced by one space, with `flush` set. `None` for text that is blank, which the endpoint
/// reads as a keep-alive and would speak nothing of.
pub fn text_message(context_id: &str, text: &str) -> Option<String> {
    let trimmed = text.trim_end();
    if trimmed.trim_start().is_empty() {
        return None;
    }
    Some(
        json!({"text": format!("{trimmed} "), "context_id": context_id, "flush": true}).to_string(),
    )
}

/// The message that closes the context `context_id` once what it holds is flushed.
pub fn close_context_message(context_id: &str) -> String {
    json!({"context_id": context_id, "close_context": true}).to_string()
}

/// One message the server sent, as the binding reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerMessage {
    /// The decoded bytes of `audio`, when it is a non-empty string.
    pub audio: Option<Vec<u8>>,
    /// Whether the final flag, `isFinal` or `is_final`, is true.
    pub is_final: bool,
    /// The context it belongs to, `contextId`, when it names one.
    pub context_id: Option<String>,
}

/// Reads one text message of the server.
///
/// # Errors
/// [`ElevenLabsError::Server`] for an object carrying `message`, `error` or `detail` and no audio,
/// where audio is an `audio` that is a non-empty string, so an error whose `audio` is null or
/// empty is the server's error too; [`ElevenLabsError::Protocol`] for text that is not a JSON
/// object, an `audio` that is neither a string nor null or not base64, an `isFinal` or
/// `is_final` that is neither a boolean nor null, a `contextId` that is neither a string nor
/// null, and an object with neither `audio` nor a final flag. The error never quotes the
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
        if !object.contains_key("audio")
            && !FINAL_SPELLINGS
                .iter()
                .any(|spelling| object.contains_key(*spelling))
        {
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
    let mut is_final = false;
    for spelling in FINAL_SPELLINGS {
        match object.get(spelling) {
            None | Some(Value::Null) => {}
            Some(Value::Bool(flag)) => is_final |= *flag,
            Some(_) => {
                return Err(ElevenLabsError::Protocol(format!(
                    "`{spelling}` is neither a boolean nor null"
                )));
            }
        }
    }
    let context_id = match object.get("contextId") {
        None | Some(Value::Null) => None,
        Some(Value::String(context_id)) => Some(context_id.clone()),
        Some(_) => {
            return Err(ElevenLabsError::Protocol(
                "`contextId` is neither a string nor null".to_owned(),
            ));
        }
    };
    Ok(ServerMessage {
        audio,
        is_final,
        context_id,
    })
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
            "ws://127.0.0.1:9/v1/text-to-speech/voice1/multi-stream-input?model_id=eleven_flash_v2_5&output_format=ulaw_8000"
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
                is_final: false,
                context_id: None
            },
            "audio with a message is audio"
        );
        assert_eq!(
            parse_server_message(r#"{"audio":""}"#).unwrap(),
            ServerMessage {
                audio: None,
                is_final: false,
                context_id: None
            }
        );
    }

    #[test]
    fn the_opening_message_is_one_space_in_its_context_with_only_the_settings_set() {
        let parsed = |text: String| serde_json::from_str::<Value>(&text).unwrap();
        assert_eq!(
            parsed(context_message("s0", None)),
            json!({"text": " ", "context_id": "s0"})
        );
        let settings = VoiceSettings {
            stability: Some(0.5),
            use_speaker_boost: Some(false),
            ..VoiceSettings::default()
        };
        assert_eq!(
            parsed(context_message("s1", Some(&settings))),
            json!({"text": " ", "context_id": "s1", "voice_settings": {"stability": 0.5, "use_speaker_boost": false}})
        );
    }

    #[test]
    fn a_text_ends_in_exactly_one_space_with_flush_and_blank_text_is_not_sent() {
        let message: Value =
            serde_json::from_str(&text_message("s0", "Hello there.  \n").unwrap()).unwrap();
        assert_eq!(
            message,
            json!({"text": "Hello there. ", "context_id": "s0", "flush": true})
        );
        assert_eq!(text_message("s0", " \t "), None);
        assert_eq!(text_message("s0", ""), None);
    }

    #[test]
    fn a_context_closes_by_its_id_and_the_socket_by_close_socket() {
        let close: Value = serde_json::from_str(&close_context_message("s2")).unwrap();
        assert_eq!(close, json!({"context_id": "s2", "close_context": true}));
        let socket: Value = serde_json::from_str(CLOSE_SOCKET).unwrap();
        assert_eq!(socket, json!({"close_socket": true}));
    }

    #[test]
    fn the_final_flag_is_read_under_both_documented_spellings() {
        for message in [
            r#"{"isFinal":true,"contextId":"s0"}"#,
            r#"{"is_final":true,"contextId":"s0"}"#,
            r#"{"isFinal":false,"is_final":true,"contextId":"s0"}"#,
        ] {
            assert!(parse_server_message(message).unwrap().is_final, "{message}");
        }
        assert!(
            !parse_server_message(r#"{"is_final":null,"contextId":"s0"}"#)
                .unwrap()
                .is_final
        );
        assert!(matches!(
            parse_server_message(r#"{"is_final":"yes"}"#),
            Err(ElevenLabsError::Protocol(detail)) if detail.contains("is_final")
        ));
    }

    #[test]
    fn the_context_of_a_server_message_is_read_from_context_id() {
        assert_eq!(
            parse_server_message(r#"{"audio":"AAEC","contextId":"s3"}"#).unwrap(),
            ServerMessage {
                audio: Some(vec![0, 1, 2]),
                is_final: false,
                context_id: Some("s3".to_owned())
            }
        );
        assert_eq!(
            parse_server_message(r#"{"isFinal":true,"contextId":"s3"}"#).unwrap(),
            ServerMessage {
                audio: None,
                is_final: true,
                context_id: Some("s3".to_owned())
            }
        );
        assert!(matches!(
            parse_server_message(r#"{"isFinal":true,"contextId":3}"#),
            Err(ElevenLabsError::Protocol(_))
        ));
    }

    #[test]
    fn audio_and_the_final_message_are_read_and_everything_else_is_refused() {
        assert_eq!(
            parse_server_message(r#"{"audio":"AAEC","alignment":null}"#).unwrap(),
            ServerMessage {
                audio: Some(vec![0, 1, 2]),
                is_final: false,
                context_id: None
            }
        );
        assert_eq!(
            parse_server_message(r#"{"audio":null,"isFinal":true}"#).unwrap(),
            ServerMessage {
                audio: None,
                is_final: true,
                context_id: None
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
