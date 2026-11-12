//! The realtime speech-to-text WebSocket protocol, as this binding speaks it.
//!
//! # Source
//!
//! Read on 2026-11-12 from ElevenLabs' public API documentation, fetched without a key: the
//! AsyncAPI description of the endpoint,
//! <https://elevenlabs.io/docs/api-reference/speech-to-text/v-1-speech-to-text-realtime>. The page
//! carries no publication or revision date of its own, so the retrieval date is the only date
//! there is. A summary of it, with that date, is kept beside the fixtures in
//! `tests/fixtures/stt-realtime-protocol.md`.
//!
//! # The connection
//!
//! One WebSocket per recognition, opened when its first frame is taken, to
//! `{base}/v1/speech-to-text/realtime?model_id=…&audio_format=…&commit_strategy=…&include_timestamps=true`,
//! with `language_code`, one `keyterms` per key term and, for the `vad` strategy, the VAD
//! settings that are configured appended ([`request_url`]). The API key travels in the
//! `xi-api-key` request header of the handshake and nowhere else; the documentation also allows a
//! single-use token in the `token` query parameter, which this binding never uses.
//!
//! # What the binding sends
//!
//! Each frame taken is one `input_audio_chunk` ([`audio_chunk`]): its bytes as `audio_base_64`,
//! `commit` false and `sample_rate` the configured format's rate. A commit is one chunk with no
//! audio (`audio_base_64` empty) and `commit` true: at the end of input, and whenever the caller
//! asks for one. The binding never sends `previous_text`.
//!
//! # What the binding reads
//!
//! Every server message is a JSON object whose `message_type` names it ([`parse_server_message`]):
//!
//! - `session_started`: read and ignored;
//! - `partial_transcript` with its `text`: the transcript so far of the audio not yet committed;
//! - `committed_transcript` with its `text`: the text of a commit;
//! - `committed_transcript_with_timestamps` with its `text` and `words`, each with `start` and
//!   `end` in seconds: the same commit with its word timings. The span's timing
//!   ([`SpanTiming`]) runs from the first `start` to the last `end` of its words of type `word`
//!   (or of no type), `spacing` and words without both numbers left out;
//! - `warning` with its `warning`: kept, never a failure;
//! - one of the fourteen error types ([`ERROR_TYPES`]) with its `error`: the recognition fails
//!   with the [`ElevenLabsError`] variant of that type;
//! - any other `message_type` that carries an `error`: [`ElevenLabsError::Server`]; any other
//!   without one, such as the messages of options the binding never sets (`diarized_transcript`,
//!   `speakers_merged`, `committed_transcript_entities`, `redacted_transcript`,
//!   `edited_transcript`): ignored.
//!
//! Anything else — a binary message, text that is not a JSON object, an object without a
//! `message_type` string, a transcript without a `text` string, `words` that are not an array,
//! a word time that is neither a number nor null — fails the recognition as
//! [`ElevenLabsError::Protocol`].
//!
//! The protocol reports no usage: the usage record's audio is what the binding sent, as the STT
//! contract measures it.
//!
//! # Points the documentation leaves open, and what the binding decided
//!
//! - **Which committed message to read.** The binding always asks for timestamps, and the
//!   documentation lists both `committed_transcript` and `committed_transcript_with_timestamps`
//!   without saying whether the endpoint then sends one or both. The binding commits a span on
//!   `committed_transcript_with_timestamps` only. A `committed_transcript` brings the open span to
//!   its text (or opens one) and commits nothing, so a server that sends both commits once.
//! - **Which commit a committed transcript answers.** Nothing in a committed message names the
//!   commit it answers. The binding counts the commits it has sent and takes each
//!   `committed_transcript_with_timestamps` as the answer to the oldest one outstanding. With the
//!   `manual` strategy only the binding commits, so the count is exact and the recognition ends
//!   at the answer to the end-of-input commit. With the `vad` strategy the endpoint also commits
//!   on its own, and a VAD commit still on its way when input ends is counted as the answer to
//!   the binding's, while the transcript of the audio after it is still to come. So with `vad`
//!   the recognition ends only once every commit is counted as answered, no span is open and
//!   the endpoint has then been silent for
//!   [`VAD_END_QUIET`](crate::VAD_END_QUIET) (500 ms); a partial transcript in that time opens a
//!   span and the wait goes on, within the read timeout. The fixture test of μ-law input meets
//!   exactly that order. A transcript that comes later than the quiet time after a VAD commit
//!   answered for the binding's is lost; the live test is where that shows.
//! - **A commit with nothing to commit.** Whether the endpoint answers a commit of audio already
//!   committed, or of silence, is not stated. Once input has ended, a server silent for the read
//!   timeout ends the recognition cleanly when no span is open, since nothing heard is left
//!   uncommitted, and fails it as [`ElevenLabsError::Stalled`] when one is.
//! - **An empty commit chunk.** `audio_base_64` is required and not documented as non-empty; the
//!   binding sends an empty one with `commit` true rather than holding a frame back to carry the
//!   flag.
//! - **Array query parameters.** `keyterms` is an array in the query; the binding sends one
//!   `keyterms` parameter per term, each percent-encoded.
//! - **PCM sample width, byte order and channels.** Not stated for `pcm_*`. The binding takes them
//!   as signed 16-bit little-endian mono, as the TTS formats of the same provider are taken.
//! - **Plaintext.** The key travels in a header, so `ws://` would send it in the clear. The binding
//!   takes `ws://` only to a loopback host, as the TTS binding does, and `wss://` to any host.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Map, Value, json};

use crate::config::Endpoint;
use crate::error::{ConfigError, ElevenLabsError};
use crate::protocol::{SERVER_MESSAGE_LIMIT, is_loopback};
use crate::stt_config::{CommitStrategy, TranscriptionConfig};

/// The path of the realtime speech-to-text endpoint under the base URL.
pub const PATH: &str = "/v1/speech-to-text/realtime";

/// The `message_type` of every error the documentation lists, each read as its own
/// [`ElevenLabsError`] variant.
pub const ERROR_TYPES: [&str; 14] = [
    "error",
    "auth_error",
    "quota_exceeded",
    "commit_throttled",
    "unaccepted_terms",
    "rate_limited",
    "queue_overflow",
    "resource_exhausted",
    "session_time_limit_exceeded",
    "input_error",
    "invalid_request",
    "chunk_size_exceeded",
    "insufficient_audio_activity",
    "transcriber_error",
];

/// Where a committed span lies in the audio sent on its connection, in seconds from the first
/// sample sent, as the endpoint timed its words.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpanTiming {
    pub start: f64,
    pub end: f64,
}

/// The URL a recognition of `config` at `endpoint` connects to. It carries no key.
///
/// # Errors
/// [`ConfigError::InvalidUrl`] when the base URL is not `wss://`, or `ws://` to a loopback host,
/// or carries a query or fragment.
pub fn request_url(
    endpoint: &Endpoint,
    config: &TranscriptionConfig,
) -> Result<String, ConfigError> {
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
        "{base}{PATH}?model_id={}&audio_format={}&commit_strategy={}&include_timestamps=true",
        encode(&config.model_id),
        config.input_format.as_str(),
        config.commit_strategy.as_str()
    );
    if let Some(language) = &config.language_code {
        url.push_str(&format!("&language_code={}", encode(language)));
    }
    for term in &config.keyterms {
        url.push_str(&format!("&keyterms={}", encode(term)));
    }
    if let CommitStrategy::Vad(settings) = &config.commit_strategy {
        if let Some(value) = settings.threshold {
            url.push_str(&format!("&vad_threshold={value}"));
        }
        if let Some(value) = settings.silence_threshold_secs {
            url.push_str(&format!("&vad_silence_threshold_secs={value}"));
        }
        if let Some(value) = settings.min_speech_duration_ms {
            url.push_str(&format!("&min_speech_duration_ms={value}"));
        }
        if let Some(value) = settings.min_silence_duration_ms {
            url.push_str(&format!("&min_silence_duration_ms={value}"));
        }
    }
    Ok(url)
}

/// `value` percent-encoded for a query: every byte but ASCII letters, digits, `-`, `_`, `.` and
/// `~` as `%XX`.
fn encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// One `input_audio_chunk`: `audio` as base64, the `commit` flag and the `sample_rate`.
pub fn audio_chunk(audio: &[u8], sample_rate: u32, commit: bool) -> String {
    json!({
        "message_type": "input_audio_chunk",
        "audio_base_64": STANDARD.encode(audio),
        "commit": commit,
        "sample_rate": sample_rate,
    })
    .to_string()
}

/// One message the server sent, as the binding reads it.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerMessage {
    /// `session_started`.
    SessionStarted,
    /// `partial_transcript`, its `text`.
    Partial(String),
    /// `committed_transcript`, its `text`.
    Committed(String),
    /// `committed_transcript_with_timestamps`, its `text` and the timing of its words, none when
    /// no word carries both times.
    CommittedWithTimestamps {
        text: String,
        timing: Option<SpanTiming>,
    },
    /// `warning`, its `warning`.
    Warning(String),
    /// A message the binding does not act on.
    Ignored,
}

/// Reads one text message of the server.
///
/// # Errors
/// The [`ElevenLabsError`] variant of one of the [`ERROR_TYPES`], carrying its `error`;
/// [`ElevenLabsError::Server`] for another `message_type` carrying an `error`; and
/// [`ElevenLabsError::Protocol`] for text that is not a JSON object, an object without a
/// `message_type` string, a transcript without a `text` string, `words` that are neither an array
/// nor null, and a word that is not an object or whose time is neither a number nor null. The
/// error never quotes the message itself, only the server's own error text.
pub fn parse_server_message(text: &str) -> Result<ServerMessage, ElevenLabsError> {
    let Ok(Value::Object(object)) = serde_json::from_str::<Value>(text) else {
        return Err(ElevenLabsError::Protocol(format!(
            "a message of {} bytes that is not a JSON object",
            text.len()
        )));
    };
    let Some(Value::String(kind)) = object.get("message_type") else {
        return Err(ElevenLabsError::Protocol(
            "a message without a `message_type` string".to_owned(),
        ));
    };
    let said = || object.get("error").map(server_text).unwrap_or_default();
    if let Some(error) = scribe_error(kind, said()) {
        return Err(error);
    }
    let transcript = || match object.get("text") {
        Some(Value::String(text)) => Ok(text.clone()),
        _ => Err(ElevenLabsError::Protocol(format!(
            "a `{kind}` without a `text` string"
        ))),
    };
    Ok(match kind.as_str() {
        "session_started" => ServerMessage::SessionStarted,
        "partial_transcript" => ServerMessage::Partial(transcript()?),
        "committed_transcript" => ServerMessage::Committed(transcript()?),
        "committed_transcript_with_timestamps" => ServerMessage::CommittedWithTimestamps {
            text: transcript()?,
            timing: timing(&object)?,
        },
        "warning" => {
            ServerMessage::Warning(object.get("warning").map(server_text).unwrap_or_default())
        }
        _ if object.contains_key("error") => return Err(ElevenLabsError::Server(said())),
        _ => ServerMessage::Ignored,
    })
}

/// The timing of the `words` of a committed transcript: from the first `start` to the last `end`
/// of its words of type `word` or of no type that carry both.
fn timing(object: &Map<String, Value>) -> Result<Option<SpanTiming>, ElevenLabsError> {
    let words = match object.get("words") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Array(words)) => words,
        Some(_) => {
            return Err(ElevenLabsError::Protocol(
                "`words` is neither an array nor null".to_owned(),
            ));
        }
    };
    let mut span: Option<SpanTiming> = None;
    for word in words {
        let Value::Object(word) = word else {
            return Err(ElevenLabsError::Protocol(
                "a word that is not an object".to_owned(),
            ));
        };
        let time = |name: &str| match word.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Number(number)) => Ok(number.as_f64()),
            Some(_) => Err(ElevenLabsError::Protocol(format!(
                "a word's `{name}` is neither a number nor null"
            ))),
        };
        let (start, end) = (time("start")?, time("end")?);
        let spoken = matches!(word.get("type"), None | Some(Value::Null))
            || word.get("type").and_then(Value::as_str) == Some("word");
        if let (true, Some(start), Some(end)) = (spoken, start, end) {
            span = Some(match span {
                None => SpanTiming { start, end },
                Some(span) => SpanTiming {
                    start: span.start.min(start),
                    end: span.end.max(end),
                },
            });
        }
    }
    Ok(span)
}

/// The variant of the documented error type `kind`, carrying `message`; none for another type.
fn scribe_error(kind: &str, message: String) -> Option<ElevenLabsError> {
    Some(match kind {
        "error" => ElevenLabsError::ScribeError(message),
        "auth_error" => ElevenLabsError::AuthError(message),
        "quota_exceeded" => ElevenLabsError::QuotaExceeded(message),
        "commit_throttled" => ElevenLabsError::CommitThrottled(message),
        "unaccepted_terms" => ElevenLabsError::UnacceptedTerms(message),
        "rate_limited" => ElevenLabsError::RateLimited(message),
        "queue_overflow" => ElevenLabsError::QueueOverflow(message),
        "resource_exhausted" => ElevenLabsError::ResourceExhausted(message),
        "session_time_limit_exceeded" => ElevenLabsError::SessionTimeLimitExceeded(message),
        "input_error" => ElevenLabsError::InputError(message),
        "invalid_request" => ElevenLabsError::InvalidRequest(message),
        "chunk_size_exceeded" => ElevenLabsError::ChunkSizeExceeded(message),
        "insufficient_audio_activity" => ElevenLabsError::InsufficientAudioActivity(message),
        "transcriber_error" => ElevenLabsError::TranscriberError(message),
        _ => return None,
    })
}

/// The server's text: a string as it is, anything else as JSON, cut to
/// [`SERVER_MESSAGE_LIMIT`] characters.
fn server_text(value: &Value) -> String {
    let text = match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    text.chars().take(SERVER_MESSAGE_LIMIT).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_documented_error_type_maps_to_its_variant_and_back() {
        for kind in ERROR_TYPES {
            let error = scribe_error(kind, "said".to_owned()).expect("a documented type");
            assert_eq!(error.scribe_error_type(), Some(kind));
            let message = json!({"message_type": kind, "error": "said"}).to_string();
            assert_eq!(parse_server_message(&message), Err(error));
        }
        assert_eq!(scribe_error("warning", String::new()), None);
    }

    #[test]
    fn a_chunk_carries_base64_audio_the_commit_flag_and_the_rate() {
        let chunk: Value = serde_json::from_str(&audio_chunk(&[0, 1, 2], 8_000, false)).unwrap();
        assert_eq!(
            chunk,
            json!({"message_type": "input_audio_chunk", "audio_base_64": "AAEC", "commit": false, "sample_rate": 8000})
        );
        let commit: Value = serde_json::from_str(&audio_chunk(&[], 16_000, true)).unwrap();
        assert_eq!(commit["audio_base_64"], "");
        assert_eq!(commit["commit"], true);
    }

    #[test]
    fn the_timing_of_a_committed_transcript_spans_its_timed_words_only() {
        let message = json!({
            "message_type": "committed_transcript_with_timestamps",
            "text": "a b",
            "words": [
                {"text": " ", "start": 0.0, "end": 0.1, "type": "spacing"},
                {"text": "a", "start": 0.1, "end": 0.2, "type": "word"},
                {"text": "b", "start": 0.3, "end": null, "type": "word"},
                {"text": "c", "start": 0.4, "end": 0.6}
            ]
        })
        .to_string();
        assert_eq!(
            parse_server_message(&message).unwrap(),
            ServerMessage::CommittedWithTimestamps {
                text: "a b".to_owned(),
                timing: Some(SpanTiming {
                    start: 0.1,
                    end: 0.6
                })
            }
        );
        let untimed = json!({"message_type": "committed_transcript_with_timestamps", "text": "x", "words": null}).to_string();
        assert!(matches!(
            parse_server_message(&untimed).unwrap(),
            ServerMessage::CommittedWithTimestamps { timing: None, .. }
        ));
    }

    #[test]
    fn messages_outside_the_protocol_are_refused_and_unknown_ones_ignored() {
        for refused in [
            "not json",
            "[1]",
            r#"{"text":"no type"}"#,
            r#"{"message_type":"partial_transcript"}"#,
            r#"{"message_type":"committed_transcript_with_timestamps","text":"x","words":5}"#,
            r#"{"message_type":"committed_transcript_with_timestamps","text":"x","words":[{"start":"0"}]}"#,
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
            parse_server_message(
                r#"{"message_type":"speakers_merged","from_speaker_id":"a","to_speaker_id":"b"}"#
            ),
            Ok(ServerMessage::Ignored)
        );
        assert_eq!(
            parse_server_message(r#"{"message_type":"new_error_kind","error":"no"}"#),
            Err(ElevenLabsError::Server("no".to_owned()))
        );
        assert_eq!(
            parse_server_message(r#"{"message_type":"warning","warning":"slow"}"#),
            Ok(ServerMessage::Warning("slow".to_owned()))
        );
    }
}
