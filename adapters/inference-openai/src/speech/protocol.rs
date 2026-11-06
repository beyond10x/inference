//! OpenAI's streamed speech endpoint, as this binding speaks it.
//!
//! # Source
//!
//! Read on 2026-11-06 from OpenAI's public API documentation, fetched without a key; neither page
//! carries a revision date of its own, so the retrieval date is the only date there is:
//!
//! - <https://developers.openai.com/api/reference/resources/audio/subresources/speech/methods/create>,
//!   the reference of `POST /audio/speech` (its Markdown form, the same URL ending in `.md`);
//! - <https://developers.openai.com/api/docs/guides/text-to-speech>, the text-to-speech guide
//!   (likewise), which states the `pcm` format and the streaming by chunked transfer encoding.
//!
//! Both pages say that `tts-1`, `tts-1-hd` and `gpt-4o-mini-tts` are deprecated and shut down on
//! January 6, 2027, and point new integrations at the Realtime API. The binding is for consumers
//! already on the speech endpoint.
//!
//! # The request
//!
//! One `POST` per segment to `https://api.openai.com/v1/audio/speech`, the key as
//! `Authorization: Bearer …` and nowhere else, and a JSON body ([`request_body`]):
//!
//! - `model`, the configured one, `gpt-4o-mini-tts` unless configured;
//! - `input`, the segment's text exactly as given; the reference caps it at 4096 characters;
//! - `voice`, a built-in voice by name or a custom voice as `{ "id": … }`;
//! - `response_format` `pcm`, which the guide describes as "the raw samples in 24kHz (16-bit
//!   signed, low-endian), without the header" and recommends, with `wav`, for the fastest
//!   response;
//! - `stream_format` `audio`, the raw bytes, which is the default; `sse` is not used;
//! - `instructions` and `speed`, only when configured: instructions at most 4096 characters,
//!   speed from 0.25 to 4.0.
//!
//! # The response
//!
//! A success streams the audio bytes as the body, with chunked transfer encoding, so audio is
//! read before the whole of it is generated. The binding reads the body chunk by chunk, each read
//! bounded by the read timeout, and the end of the body ends the segment. Any other status fails
//! the synthesis with the status.
//!
//! # Points the documentation leaves open, and what the binding decided
//!
//! - **Channels of `pcm`.** The guide states the rate, the width, the signedness and the byte
//!   order, not the channel count. The guide's own Java example plays the stream as one channel,
//!   so the binding reads it as mono; a body that ends on half a sample fails the synthesis.
//! - **What the 4096-character limit counts.** The reference gives `maxLength: 4096`, a JSON
//!   Schema bound, which counts Unicode code points. The binding counts the `char`s of the text
//!   and refuses a segment longer than that before sending it.
//! - **The error body.** Neither page describes the body of a refused request. The binding
//!   reports the status alone and does not read the body.
//! - **The content type.** The reference gives `application/octet-stream` for a success; the
//!   binding does not check it, since the request names the format.
//! - **Cancel.** There is no cancel request. The binding cancels by dropping the response and
//!   closing its connection; whether generation and billing stop with it is not documented.
//! - **`instructions` with `tts-1` and `tts-1-hd`.** The reference says they do not work with
//!   those models; it does not say whether the request is refused. The binding sends what is
//!   configured and leaves the answer to the server.
//! - **Plaintext.** The key travels in a header, so `http://` would send it in the clear. The
//!   binding takes `http://` only to a loopback host (`127.0.0.0/8`, `::1`, `localhost`), as a
//!   local test server is, and `https://` to any host.

use serde_json::{Map, Value};

use super::config::{SpeechConfig, SpeechVoice};

/// The most characters the reference allows in `input`, and in `instructions`.
pub const MAX_INPUT_CHARACTERS: usize = 4_096;

/// The sample rate of the `pcm` response format, in hertz.
pub const RESPONSE_RATE: u32 = 24_000;

/// The JSON body of the request that speaks `input` under `config`.
pub fn request_body(config: &SpeechConfig, input: &str) -> Value {
    let mut body = Map::new();
    body.insert("model".to_owned(), Value::from(config.model.as_str()));
    body.insert("input".to_owned(), Value::from(input));
    let voice = match &config.voice {
        SpeechVoice::BuiltIn(name) => Value::from(name.as_str()),
        SpeechVoice::Custom(id) => {
            let mut object = Map::new();
            object.insert("id".to_owned(), Value::from(id.as_str()));
            Value::Object(object)
        }
    };
    body.insert("voice".to_owned(), voice);
    body.insert("response_format".to_owned(), Value::from("pcm"));
    body.insert("stream_format".to_owned(), Value::from("audio"));
    if let Some(instructions) = &config.instructions {
        body.insert(
            "instructions".to_owned(),
            Value::from(instructions.as_str()),
        );
    }
    if let Some(speed) = config.speed {
        body.insert("speed".to_owned(), Value::from(speed));
    }
    Value::Object(body)
}
