//! Cartesia Sonic streaming text-to-speech as a synthesiser of the `inference-tts` contract.
//!
//! `inference-cartesia` is a hosted adapter of the inference framework, at locality
//! `external_provider`: the text of every segment leaves the machine. [`CartesiaSynthesiser`]
//! implements [`inference_tts::Synthesiser`] over the WebSocket `/tts/websocket`; [`protocol`]
//! says what it sends and reads, and `docs/protocol.md` names the documentation it was taken
//! from with its date and what that documentation leaves open.
//!
//! A synthesis holds one connection, opened when its first segment is sent and kept for the
//! segments after it. Each segment is one generation request on a context of its own; the audio
//! chunks of that context are emitted as frames of the segment while it is still Queued, on one
//! sample clock across the synthesis, and the context's `done` completes it. A segment is sent
//! as soon as the handle is iterated after it was offered, before the utterance ends, once the
//! segment before it is done. So the binding claims `incremental_input` and `output_streaming`
//! ([`claims`]), and `records/claims-tts.json` holds the same claims.
//!
//! Output formats ([`OutputFormat`]): every encoding of the contract, `pcm_s16le`, `pcm_f32le`,
//! `pcm_mulaw` and `pcm_alaw`, at 8000, 16000, 22050, 24000, 44100 or 48000 Hz, taken as mono;
//! [`OutputFormat::MULAW_8000`] and [`OutputFormat::ALAW_8000`] are the telephony formats.
//!
//! A cancel is a discard of the segment being spoken, as a barge-in makes it: the binding sends
//! the protocol's cancel for its context, emits nothing more of it, and drops every later message
//! of that context, which the server may go on sending; the next segment goes out at once on the
//! same connection. Dropping the handle closes its connection. A connection that closes, stalls
//! or sends something outside the protocol fails the synthesis with a [`CartesiaError`]
//! ([`CartesiaSynthesis::failure`]), never a panic or a hang: the handshake is bounded by the
//! endpoint's connect timeout, every read and write by its read timeout, a cancel's write by
//! [`CLOSE_BOUND`].
//!
//! The handle drives its connection on a runtime of its own, with one thread. Iterated, as the
//! contract defines it, it blocks the calling thread while it waits for the server, on any
//! thread, an async worker included, which it then holds up. Its async form,
//! [`SynthesisHandle::event`](inference_tts::SynthesisHandle::event), waits without blocking, so
//! an async caller awaits that instead.
//!
//! The key is never read by the binding itself: the caller injects a [`CredentialSource`] and
//! names the secret ([`EnvironmentSource::cartesia`] reads `CARTESIA_API_KEY`; with feature
//! `keychain`, `KeychainSource` reads the OS keychain through the `secrets` library). It is sent
//! in the `X-API-Key` header of the handshake, marked sensitive, and never kept, printed, logged
//! or put in an error. The WebSocket library below does log the raw handshake request at trace
//! level under targets beginning with `tungstenite`; an embedder that installs a logger keeps
//! those targets below trace.
//!
//! The key is copied only where the handshake needs it, and no field of the binding holds it past
//! the handshake. Every copy:
//!
//! - the [`Credential`] the source returns, zeroized on drop. The `X-API-Key` header value is the
//!   credential's own buffer, not a copy of it, and the buffer is zeroized when tungstenite has
//!   turned the request into bytes, or when the handshake fails or times out first;
//! - those bytes: tungstenite, the WebSocket library, writes the request, header included, into a
//!   buffer of its own that it frees without zeroizing once the request is sent, and the smaller
//!   buffers that one outgrew while being written may hold part of it too. The binding cannot
//!   reach them short of a WebSocket client of its own. They are freed memory of this process
//!   until the allocator reuses it, which matters only to someone who can already read this
//!   process's memory or a core dump of it, and the binding accepts that;
//! - for `wss://`, the TLS record rustls copies the request into and encrypts in place, so what it
//!   keeps is ciphertext;
//! - the trace-level log record named above, when a logger lets it through.
//!
//! The live test is ignored and behind feature `live`: with a key in `CARTESIA_API_KEY` (or the
//! keychain) and a voice in `CARTESIA_VOICE_ID`,
//! `cargo test -p inference-cartesia --features live --test live -- --ignored --nocapture`
//! speaks one short utterance in μ-law at 8 kHz and checks what comes back. It is billed.

mod config;
mod connection;
mod credential;
mod error;
pub mod protocol;
mod synthesiser;
mod wake;

pub use config::{
    API_VERSION, DEFAULT_CONNECT_TIMEOUT, DEFAULT_MODEL, DEFAULT_READ_TIMEOUT, DEFAULT_URL,
    Endpoint, OutputFormat, SAMPLE_RATES, VoiceConfig,
};
pub use connection::CLOSE_BOUND;
#[cfg(feature = "keychain")]
pub use credential::KeychainSource;
pub use credential::{
    CARTESIA_KEY_NAME, CARTESIA_KEY_VARIABLE, Credential, CredentialError, CredentialFuture,
    CredentialSource, EnvironmentSource,
};
pub use error::{CartesiaError, ConfigError, ConnectError, ServerError};
pub use synthesiser::{BACKEND, CartesiaSynthesis, CartesiaSynthesiser, claims};
