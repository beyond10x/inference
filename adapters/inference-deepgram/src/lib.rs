//! Deepgram streaming speech-to-text as a recogniser of the `inference-stt` contract.
//!
//! `inference-deepgram` is a hosted adapter of the inference framework, at locality
//! `external_provider`: the audio of every recognition leaves the machine.
//! [`DeepgramRecogniser`] implements [`inference_stt::Recogniser`] over the live transcription
//! WebSocket `/v1/listen`; [`listen`] says what it sends and reads, the documentation it was taken
//! from with its date, and what that documentation leaves open. The Flux endpoint `/v2/listen` is
//! not spoken here.
//!
//! A recognition opens its connection when its first frame is taken, asking for that frame's
//! encoding and rate: G.711 μ-law or A-law at 8 kHz, as telephony carries it, or 16-bit PCM at any
//! rate. Every frame is sent as soon as the handle is iterated after it was offered, and interim
//! results reach the caller before input ends, so the binding claims `incremental_input` and
//! `output_streaming` ([`claims`]), and `records/claims-stt.json` holds the same claims.
//!
//! # Transcripts
//!
//! - An interim `Results` (`is_final` false) opens a Hypothesis span with its transcript, or, when
//!   a span is open, revises it to read the transcript if that differs.
//! - An `is_final` `Results` brings the open span to its transcript and commits it.
//! - **End of turn.** `speech_final` true, or an `UtteranceEnd` with no `speech_final` before it,
//!   commits the open span, as the sherpa-onnx adapter's endpoint does. No other event marks it,
//!   and the contract is unchanged: completing the turn stays the turn policy's. Since a
//!   `speech_final` result commits its span, an `UtteranceEnd` after one finds no span open and
//!   does nothing. An `UtteranceEnd` whose `last_word_end` is below 0 (`-1`, already finalized)
//!   is ignored.
//! - A `Results` whose transcript is empty or blank emits nothing; when it is final it still
//!   commits a span left open.
//! - **Audio committed once.** The binding keeps where the audio of the committed spans ends
//!   (`start` plus `duration` of their results). A `Results` whose audio ends no later than that,
//!   while no span is open, is dropped: after an `UtteranceEnd` committed an interim span, the
//!   final result of the same audio does not commit the words again. A result without both
//!   numbers is never dropped.
//! - After `CloseStream`, the summary `Metadata` commits a span still open and ends the
//!   recognition; its usage record is then its last event. Whether a `Metadata` is the summary
//!   is decided by the thread driving the connection, from whether it has sent `CloseStream`; any
//!   other `Metadata` is ignored.
//!
//! # Usage
//!
//! The usage record is the contract's: `input_audio_ms`, the audio offered, `requests` 1, and
//! `local_compute_ms` unknown, since nothing is computed here. The seconds of audio the endpoint
//! reports in its summary `Metadata` are not a quantity the contract's record carries, so they are
//! not put in it; [`DeepgramRecognition::provider_duration`] gives them as reported.
//!
//! # Failure and cancel
//!
//! A refused handshake (HTTP 401 or 403), an `Error` message, a close before the summary
//! `Metadata` (1008 `DATA-0000`, 1011 `NET-0000`, `NET-0001`, `NET-0002` or any other), a text
//! message outside the protocol, and a server silent past the read timeout once input has ended
//! each fail the recognition with a [`DeepgramError`] ([`DeepgramRecognition::failure`]), never a
//! panic or a hang. The handshake is bounded by the connect timeout and every write by the read
//! timeout. **One total deadline** bounds the end: from sending `CloseStream`, the summary
//! `Metadata` must arrive within the read timeout, however many messages come before it; the
//! handle waits at most that plus [`CLOSE_BOUND`]. [`DeepgramRecognition::cancel`] and dropping
//! the handle close the connection and deliver nothing more, during the opening handshake too:
//! the attempt is dropped at once.
//!
//! # Bounds on what the server sends
//!
//! A message or frame larger than [`listen::MAX_MESSAGE_SIZE`] (4 MiB) fails the recognition as a
//! transport error instead of being buffered. At most [`listen::INBOUND_DEPTH`] (32) server
//! messages wait for the caller; while that many wait, the connection is not read until the
//! caller iterates, and `KeepAlive` and the caller's audio still go out.
//!
//! The connection is driven on a thread of its own with a runtime of its own, so the handle may
//! be iterated from any thread, an async worker among them; once input has ended an iteration
//! blocks while it waits for the server.
//!
//! # The key
//!
//! The key is never read by the binding itself: the caller injects a [`CredentialSource`] and
//! names the secret ([`EnvironmentSource::deepgram`] reads `DEEPGRAM_API_KEY`; with feature
//! `keychain`, `KeychainSource` reads the OS keychain through the `secrets` library). It is sent
//! as `Authorization: Token <key>` in the handshake, marked sensitive, and never kept, printed,
//! logged or put in an error. The WebSocket library below does log the raw handshake request at
//! trace level under targets beginning with `tungstenite`; an embedder that installs a logger
//! keeps those targets below trace. `ws://` is taken only to a loopback host.
//!
//! The live check is `tests/live.rs`, behind feature `live` and ignored: with a key with
//! speech-to-text access in `DEEPGRAM_API_KEY` (or the keychain) and a 16-bit mono WAV file in
//! `INFERENCE_DEEPGRAM_STT_CLIP`, `cargo test -p inference-deepgram --features live --test live
//! -- --ignored` transcribes it. It is billed.

mod config;
mod connection;
mod credential;
mod error;
pub mod listen;
mod recogniser;

pub use config::{
    DEFAULT_CONNECT_TIMEOUT, DEFAULT_KEEPALIVE_INTERVAL, DEFAULT_LANGUAGE, DEFAULT_MODEL,
    DEFAULT_READ_TIMEOUT, DEFAULT_URL, EU_URL, Endpoint, Endpointing, ListenConfig, Region,
    UTTERANCE_END_RANGE,
};
pub use connection::CLOSE_BOUND;
#[cfg(feature = "keychain")]
pub use credential::KeychainSource;
pub use credential::{
    Credential, CredentialError, CredentialFuture, CredentialSource, DEEPGRAM_KEY_NAME,
    DEEPGRAM_KEY_VARIABLE, EnvironmentSource,
};
pub use error::{CloseCause, ConfigError, ConnectError, DeepgramError};
pub use recogniser::{BACKEND, DeepgramRecogniser, DeepgramRecognition, claims};
