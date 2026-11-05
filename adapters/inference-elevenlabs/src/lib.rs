//! ElevenLabs streaming text-to-speech as a synthesiser of the `inference-tts` contract.
//!
//! `inference-elevenlabs` is a hosted adapter of the inference framework, at locality
//! `external_provider`: the text of every segment leaves the machine. [`ElevenLabsSynthesiser`]
//! implements [`inference_tts::Synthesiser`] over the single-stream WebSocket
//! `/v1/text-to-speech/{voice_id}/stream-input`; [`protocol`] says what it sends and reads, the
//! documentation it was taken from with its date, and what that documentation leaves open.
//!
//! A synthesis takes the segments offered to it in order. Each segment gets a connection of its
//! own: the opening message, its text ending in one space with `flush` set, and the end of input;
//! the audio chunks that come back are emitted as frames of that segment while it is still
//! Queued, on one sample clock across the synthesis, and the final message completes it. A
//! segment is sent as soon as the handle is iterated after it was offered, before the utterance
//! ends. So the binding claims `incremental_input` and `output_streaming` ([`claims`]), and
//! `records/claims-tts.json` holds the same claims.
//!
//! Output formats ([`OutputFormat`]): `ulaw_8000` gives G.711 μ-law frames at 8 kHz, `alaw_8000`
//! A-law at 8 kHz, and `pcm_16000`, `pcm_22050`, `pcm_24000` and `pcm_44100` signed 16-bit
//! little-endian mono PCM at that rate. The documentation states neither bit depth, byte order
//! nor channels for `pcm_*`; 16-bit little-endian mono is this crate's assumption, and the live
//! example checks it. MP3, Opus and WAV are refused when the configuration is parsed.
//!
//! A cancel is a discard of the segment being spoken, as a barge-in makes it: the protocol has no
//! cancel message, so the binding closes that segment's connection and emits nothing more of it.
//! Dropping the handle closes its connection the same way. A connection that closes, stalls or
//! sends something outside the protocol fails the synthesis with an [`ElevenLabsError`]
//! ([`ElevenLabsSynthesis::failure`]), never a panic or a hang: the handshake is bounded by the
//! endpoint's connect timeout, every read and write by its read timeout.
//!
//! The handle is a blocking iterator, as the contract defines it. It drives its connection on a
//! runtime of its own, so it is iterated on a thread that runs no async runtime; called on one,
//! it fails with [`ElevenLabsError::InsideRuntime`] instead of panicking.
//!
//! The key is never read by the binding itself: the caller injects a [`CredentialSource`] and
//! names the secret ([`EnvironmentSource::elevenlabs`] reads `ELEVENLABS_API_KEY`; with feature
//! `keychain`, `KeychainSource` reads the OS keychain through the `secrets` library). It is sent
//! in the `xi-api-key` header of each handshake, marked sensitive, and never kept, printed,
//! logged or put in an error. The WebSocket library below does log the raw handshake request at
//! trace level under targets beginning with `tungstenite`; an embedder that installs a logger
//! keeps those targets below trace.
//!
//! The live check is an example, never a test: with a key with text-to-speech access in
//! `ELEVENLABS_API_KEY` (or the keychain) and a voice in `ELEVENLABS_VOICE_ID`,
//! `cargo run -p inference-elevenlabs --features live --example live` speaks one short utterance
//! in `ulaw_8000` and one in `pcm_24000`, checks the encoding, rate and frame lengths of what
//! comes back and prints the time to first audio. It is billed.

mod config;
mod connection;
mod credential;
mod error;
pub mod protocol;
mod synthesiser;

pub use config::{
    DEFAULT_CONNECT_TIMEOUT, DEFAULT_MODEL, DEFAULT_READ_TIMEOUT, DEFAULT_URL, Endpoint,
    MAX_INACTIVITY_TIMEOUT, OutputFormat, VoiceConfig, VoiceSettings,
};
pub use connection::CLOSE_BOUND;
#[cfg(feature = "keychain")]
pub use credential::KeychainSource;
pub use credential::{
    Credential, CredentialError, CredentialFuture, CredentialSource, ELEVENLABS_KEY_NAME,
    ELEVENLABS_KEY_VARIABLE, EnvironmentSource,
};
pub use error::{ConfigError, ConnectError, ElevenLabsError};
pub use synthesiser::{BACKEND, ElevenLabsSynthesis, ElevenLabsSynthesiser, claims};
