//! The live transcription WebSocket `/v1/listen`, as this binding speaks it.
//!
//! # Source
//!
//! Read on 2026-11-05 from Deepgram's public developer documentation. The pages carry no
//! revision date of their own, so the retrieval date is the only date there is.
//!
//! - <https://developers.deepgram.com/docs/live-streaming-audio>, the streaming guide;
//! - <https://developers.deepgram.com/reference/speech-to-text/listen-streaming>, the reference
//!   of the endpoint and its messages;
//! - <https://developers.deepgram.com/reference/regional-endpoints.md>, the EU host;
//! - <https://developers.deepgram.com/docs/encoding.md>, the encodings;
//! - <https://developers.deepgram.com/docs/audio-keep-alive.md>, `KeepAlive`;
//! - <https://developers.deepgram.com/docs/finalize.md>, `Finalize`, which this binding does
//!   not send;
//! - <https://developers.deepgram.com/docs/close-stream.md>, `CloseStream`;
//! - <https://developers.deepgram.com/docs/understanding-end-of-speech-detection.md>, how
//!   `speech_final` and `UtteranceEnd` end an utterance;
//! - <https://developers.deepgram.com/docs/endpointing.md>, `endpointing`;
//! - <https://developers.deepgram.com/docs/utterance-end.md>, `UtteranceEnd`.
//!
//! The Flux endpoint, `/v2/listen`, is another protocol with its own turn events; this binding
//! does not speak it.
//!
//! # The connection
//!
//! One WebSocket per recognition, opened when the first frame is taken, to
//! `{base}/v1/listen?model=…&language=…&encoding=…&sample_rate=…&channels=1&interim_results=true&smart_format=…`,
//! with `endpointing` and `utterance_end_ms` appended when configured ([`query`]). The encoding
//! and rate are the first frame's: G.711 μ-law at 8 kHz is `mulaw`, A-law at 8 kHz `alaw`, and
//! signed 16-bit little-endian PCM at any rate `linear16`. The key travels as
//! `Authorization: Token <key>` in the handshake and nowhere else.
//!
//! # What the binding sends
//!
//! - Each frame's bytes as one binary message, as it is taken; a frame with no bytes is not sent,
//!   since an empty binary message closes the stream.
//! - [`KEEP_ALIVE`] as a text message whenever no audio or `KeepAlive` was sent for the
//!   configured interval, until the end of input.
//! - [`CLOSE_STREAM`] once input has ended. The endpoint then sends what it still holds, one
//!   summary `Metadata`, and closes.
//!
//! # What the binding reads
//!
//! Text messages only, each a JSON object with a string `type` ([`parse_server_message`]):
//!
//! - `Results`: `channel.alternatives[0].transcript`, `is_final` and `speech_final`.
//! - `UtteranceEnd`: `last_word_end`.
//! - `Metadata`: `duration`, the seconds of audio the endpoint processed.
//! - `SpeechStarted`: read and ignored; the binding does not ask for it.
//! - `Error`: fails the recognition with its `code` and `description`.
//!
//! # Points the documentation leaves open, and what the binding decided
//!
//! - **The `Authorization` value.** The reference names the header and not its form; every
//!   example sends `Token <key>`, which the binding sends.
//! - **`Error` on an open stream.** Described as not fatal by itself; the binding fails the
//!   recognition on it all the same, since what follows it is not described either.
//! - **A message of an unknown `type`.** Not described; ignored, so a new informational message
//!   does not end a call. A known message missing what the binding reads is a protocol error.
//! - **The close code after `Metadata`.** Not stated. The binding closes the connection itself
//!   once it has read the summary `Metadata` after `CloseStream`, and reads nothing more; a close
//!   before that `Metadata`, whatever its code, fails the recognition. A `Metadata` the binding
//!   reads before it has sent `CloseStream` is not the summary and is ignored, however long it
//!   waits for the caller.
//! - **Encodings and rates.** A default encoding is not stated, so the binding always names one.
//!   μ-law and A-law are taken only at 8 kHz, the telephony rate; 32-bit PCM is refused, since
//!   whether `linear32` is integer or float is not stated; more than one channel is refused.
//! - **How long the endpoint may stay silent.** Not stated for an open stream, whose silence is
//!   the speaker's; the endpoint closes a stream without audio itself (`NET-0001`, `NET-0002`).
//!   Once `CloseStream` is sent, the summary `Metadata` must arrive within the read timeout,
//!   counted once from `CloseStream`: messages in between do not extend it.
//! - **Whether a final result can repeat audio already committed.** Not stated. A `Results`
//!   whose span (`start` plus `duration`) ends no later than the audio already committed, while
//!   no span is open, is dropped: after an `UtteranceEnd` committed an interim span, the final
//!   result of the same audio does not commit the words again.
//! - **The largest message.** Not stated. The binding reads messages and frames of at most
//!   [`MAX_MESSAGE_SIZE`] and holds at most [`INBOUND_DEPTH`] messages its caller has not taken.
//! - **The `NET-0002` timeout, the `UtteranceEnd` timer's reference, the billing unit, the
//!   longest connection and the largest chunk.** Not stated; the binding depends on none.

use inference_stt::inference_audio::{AudioEncoding, AudioFormat};
use serde_json::{Map, Value};

use crate::config::{Endpointing, ListenConfig};
use crate::error::DeepgramError;

/// The path of the live transcription endpoint under the base URL.
pub const PATH: &str = "/v1/listen";

/// The message that keeps an open stream alive while no audio is sent.
pub const KEEP_ALIVE: &str = r#"{"type":"KeepAlive"}"#;

/// The message that ends the input of a stream.
pub const CLOSE_STREAM: &str = r#"{"type":"CloseStream"}"#;

/// The largest message, and the largest frame, the binding reads: 4 MiB. A larger one fails the
/// recognition as a transport error instead of being buffered.
pub const MAX_MESSAGE_SIZE: usize = 4 << 20;

/// The most server messages a recognition holds that its caller has not taken yet. Once that
/// many are waiting, the binding stops reading the connection until the caller takes one.
pub const INBOUND_DEPTH: usize = 32;

/// The most characters of a server's error text an error keeps.
pub const SERVER_MESSAGE_LIMIT: usize = 256;

/// The `encoding` of audio in `format`.
///
/// # Errors
/// [`DeepgramError::UnsupportedAudio`] for more than one channel, μ-law or A-law at a rate other
/// than 8 kHz, and 32-bit float PCM.
pub fn encoding(format: AudioFormat) -> Result<&'static str, DeepgramError> {
    if format.channels.get() != 1 {
        return Err(DeepgramError::UnsupportedAudio(format!(
            "{} channels; the binding sends mono",
            format.channels
        )));
    }
    let rate = format.sample_rate.get();
    match format.encoding {
        AudioEncoding::PcmS16le => Ok("linear16"),
        AudioEncoding::PcmMulaw if rate == 8_000 => Ok("mulaw"),
        AudioEncoding::PcmAlaw if rate == 8_000 => Ok("alaw"),
        AudioEncoding::PcmMulaw | AudioEncoding::PcmAlaw => Err(DeepgramError::UnsupportedAudio(
            format!("G.711 at {rate} Hz; the binding takes it at 8000 Hz"),
        )),
        AudioEncoding::PcmF32le => Err(DeepgramError::UnsupportedAudio(
            "32-bit float PCM; the endpoint's 32-bit encoding is not described".to_owned(),
        )),
    }
}

/// The query of a recognition of audio in `format` under `config`.
///
/// # Errors
/// The [`DeepgramError::UnsupportedAudio`] of [`encoding`].
pub fn query(config: &ListenConfig, format: AudioFormat) -> Result<String, DeepgramError> {
    let mut query = format!(
        "model={}&language={}&encoding={}&sample_rate={}&channels=1&interim_results=true&smart_format={}",
        config.model,
        config.language,
        encoding(format)?,
        format.sample_rate,
        config.smart_format
    );
    match config.endpointing {
        Some(Endpointing::Disabled) => query.push_str("&endpointing=false"),
        Some(Endpointing::Millis(ms)) => query.push_str(&format!("&endpointing={ms}")),
        None => {}
    }
    if let Some(ms) = config.utterance_end_ms {
        query.push_str(&format!("&utterance_end_ms={ms}"));
    }
    Ok(query)
}

/// One message the server sent, as the binding reads it.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerMessage {
    /// A transcript of the audio span the endpoint is working on.
    Results {
        /// The first alternative's transcript; empty when nothing was heard.
        transcript: String,
        /// The span's transcript will not change.
        is_final: bool,
        /// Endpointing ended an utterance here.
        speech_final: bool,
        /// Where the audio span ends, in seconds: `start` plus `duration`, when both are numbers
        /// and the sum is finite.
        end: Option<f64>,
    },
    /// No word followed the last one for `utterance_end_ms`. `last_word_end` is the end of the
    /// last word in seconds, below 0 when the utterance was already finalized.
    UtteranceEnd { last_word_end: f64 },
    /// The summary after `CloseStream`, or any other `Metadata`: the seconds of audio processed,
    /// when given as a number.
    Metadata { duration: Option<f64> },
    /// A message the binding does not act on: `SpeechStarted`, or a `type` it does not know.
    Ignored,
}

/// Reads one text message of the server.
///
/// # Errors
/// [`DeepgramError::Server`] for an `Error` message; [`DeepgramError::Protocol`] for text that is
/// not a JSON object, an object without a string `type`, a `Results` without a string
/// `channel.alternatives[0].transcript` or with an `is_final` or `speech_final` that is neither
/// a boolean nor null, and an `UtteranceEnd` without a numeric `last_word_end`. The error never
/// quotes the message itself, only the server's own error text.
pub fn parse_server_message(text: &str) -> Result<ServerMessage, DeepgramError> {
    let Ok(Value::Object(object)) = serde_json::from_str::<Value>(text) else {
        return Err(DeepgramError::Protocol(format!(
            "a message of {} bytes that is not a JSON object",
            text.len()
        )));
    };
    let Some(Value::String(kind)) = object.get("type") else {
        return Err(DeepgramError::Protocol(
            "an object without a string `type`".to_owned(),
        ));
    };
    match kind.as_str() {
        "Results" => results(&object),
        "UtteranceEnd" => match object.get("last_word_end").and_then(Value::as_f64) {
            Some(last_word_end) => Ok(ServerMessage::UtteranceEnd { last_word_end }),
            None => Err(DeepgramError::Protocol(
                "an `UtteranceEnd` without a numeric `last_word_end`".to_owned(),
            )),
        },
        "Metadata" => Ok(ServerMessage::Metadata {
            duration: object.get("duration").and_then(Value::as_f64),
        }),
        "Error" => Err(DeepgramError::Server {
            code: object
                .get("code")
                .or_else(|| object.get("variant"))
                .map(server_text),
            description: ["description", "message"]
                .iter()
                .find_map(|name| object.get(*name).map(server_text))
                .unwrap_or_default(),
        }),
        _ => Ok(ServerMessage::Ignored),
    }
}

fn results(object: &Map<String, Value>) -> Result<ServerMessage, DeepgramError> {
    let transcript = object
        .get("channel")
        .and_then(|channel| channel.get("alternatives"))
        .and_then(|alternatives| alternatives.get(0))
        .and_then(|first| first.get("transcript"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            DeepgramError::Protocol(
                "a `Results` without a string `channel.alternatives[0].transcript`".to_owned(),
            )
        })?;
    let flag = |name: &str| match object.get(name) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(DeepgramError::Protocol(format!(
            "`{name}` is neither a boolean nor null"
        ))),
    };
    Ok(ServerMessage::Results {
        transcript: transcript.to_owned(),
        is_final: flag("is_final")?,
        speech_final: flag("speech_final")?,
        end: span_end(object),
    })
}

/// `start` plus `duration` of a `Results`, when both are numbers and the sum is finite.
fn span_end(object: &Map<String, Value>) -> Option<f64> {
    let start = object.get("start").and_then(Value::as_f64)?;
    let duration = object.get("duration").and_then(Value::as_f64)?;
    Some(start + duration).filter(|end| end.is_finite())
}

/// The server's text: a string as it is, else the whole value, cut to [`SERVER_MESSAGE_LIMIT`]
/// characters.
fn server_text(value: &Value) -> String {
    let text = match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    text.chars().take(SERVER_MESSAGE_LIMIT).collect()
}

#[cfg(test)]
mod tests {
    use std::num::{NonZeroU16, NonZeroU32};

    use super::*;

    fn format(encoding: AudioEncoding, rate: u32, channels: u16) -> AudioFormat {
        AudioFormat {
            encoding,
            sample_rate: NonZeroU32::new(rate).unwrap(),
            channels: NonZeroU16::new(channels).unwrap(),
        }
    }

    #[test]
    fn the_query_names_the_model_the_audio_and_only_the_options_set() {
        let mut config = ListenConfig::new();
        assert_eq!(
            query(&config, format(AudioEncoding::PcmMulaw, 8_000, 1)).unwrap(),
            "model=nova-3&language=en&encoding=mulaw&sample_rate=8000&channels=1&interim_results=true&smart_format=false"
        );
        config.endpointing = Some(Endpointing::Millis(300));
        config.utterance_end_ms = Some(1_000);
        assert!(
            query(&config, format(AudioEncoding::PcmS16le, 16_000, 1))
                .unwrap()
                .ends_with("encoding=linear16&sample_rate=16000&channels=1&interim_results=true&smart_format=false&endpointing=300&utterance_end_ms=1000")
        );
        config.endpointing = Some(Endpointing::Disabled);
        assert!(
            query(&config, format(AudioEncoding::PcmAlaw, 8_000, 1))
                .unwrap()
                .contains("encoding=alaw&sample_rate=8000&")
        );
        assert!(
            query(&config, format(AudioEncoding::PcmAlaw, 8_000, 1))
                .unwrap()
                .contains("&endpointing=false")
        );
    }

    #[test]
    fn audio_the_binding_cannot_describe_is_refused() {
        for refused in [
            format(AudioEncoding::PcmMulaw, 16_000, 1),
            format(AudioEncoding::PcmAlaw, 16_000, 1),
            format(AudioEncoding::PcmF32le, 16_000, 1),
            format(AudioEncoding::PcmS16le, 16_000, 2),
        ] {
            assert!(
                matches!(encoding(refused), Err(DeepgramError::UnsupportedAudio(_))),
                "{refused:?}"
            );
        }
        assert_eq!(
            encoding(format(AudioEncoding::PcmS16le, 44_100, 1)).unwrap(),
            "linear16"
        );
    }

    #[test]
    fn the_end_of_a_results_span_is_its_start_plus_its_duration() {
        let end = |text: &str| match parse_server_message(text).unwrap() {
            ServerMessage::Results { end, .. } => end,
            other => panic!("{other:?}"),
        };
        let alternatives = r#""channel":{"alternatives":[{"transcript":"x"}]}"#;
        assert_eq!(
            end(&format!(
                r#"{{"type":"Results",{alternatives},"start":1.5,"duration":0.5}}"#
            )),
            Some(2.0)
        );
        assert_eq!(
            end(&format!(
                r#"{{"type":"Results",{alternatives},"start":1.5}}"#
            )),
            None
        );
        assert_eq!(
            end(&format!(
                r#"{{"type":"Results",{alternatives},"start":1e308,"duration":1e308}}"#
            )),
            None
        );
    }

    #[test]
    fn results_utterance_end_and_metadata_are_read() {
        assert_eq!(
            parse_server_message(
                r#"{"type":"Results","channel":{"alternatives":[{"transcript":"hi","confidence":0.9}]},"is_final":true,"speech_final":false}"#
            )
            .unwrap(),
            ServerMessage::Results {
                transcript: "hi".to_owned(),
                is_final: true,
                speech_final: false,
                end: None,
            }
        );
        assert_eq!(
            parse_server_message(
                r#"{"type":"Results","channel":{"alternatives":[{"transcript":""}]}}"#
            )
            .unwrap(),
            ServerMessage::Results {
                transcript: String::new(),
                is_final: false,
                speech_final: false,
                end: None,
            }
        );
        assert_eq!(
            parse_server_message(r#"{"type":"UtteranceEnd","channel":[0,1],"last_word_end":-1}"#)
                .unwrap(),
            ServerMessage::UtteranceEnd {
                last_word_end: -1.0
            }
        );
        assert_eq!(
            parse_server_message(r#"{"type":"Metadata","request_id":"r","duration":1.5}"#).unwrap(),
            ServerMessage::Metadata {
                duration: Some(1.5)
            }
        );
        assert_eq!(
            parse_server_message(r#"{"type":"SpeechStarted","channel":[0],"timestamp":0.1}"#)
                .unwrap(),
            ServerMessage::Ignored
        );
    }

    #[test]
    fn an_error_message_is_the_servers_and_everything_malformed_is_refused() {
        assert_eq!(
            parse_server_message(
                r#"{"type":"Error","variant":"x","code":"INVALID","description":"bad audio"}"#
            ),
            Err(DeepgramError::Server {
                code: Some("INVALID".to_owned()),
                description: "bad audio".to_owned()
            })
        );
        for refused in [
            "not json",
            "[1]",
            r#"{"kind":"Results"}"#,
            r#"{"type":5}"#,
            r#"{"type":"Results","channel":{"alternatives":[]}}"#,
            r#"{"type":"Results","channel":{"alternatives":[{"transcript":"x"}]},"is_final":"yes"}"#,
            r#"{"type":"UtteranceEnd"}"#,
        ] {
            assert!(
                matches!(
                    parse_server_message(refused),
                    Err(DeepgramError::Protocol(_))
                ),
                "{refused}"
            );
        }
    }
}
