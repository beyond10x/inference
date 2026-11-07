//! Phonic speech-to-speech as a native speech model of the `inference-duplex` contract.
//!
//! `inference-phonic` is a hosted adapter of the inference framework, at locality
//! `external_provider`: the audio of every session leaves the machine. [`PhonicSpeech`] opens a
//! conversation over Phonic's realtime WebSocket `/v1/sts/ws` with the `phonic-1` model unless
//! configured otherwise; [`protocol`] says what it sends and reads, the documentation it was taken
//! from with its date (summarised in `protocol/README.md`), and what that documentation
//! leaves open. `duplex` says how a conversation maps onto the contract.
//!
//! The session configuration chooses the formats both ways, each one Phonic takes: 16-bit PCM
//! at 44.1, 24, 16 or 8 kHz, or G.711 μ-law at 8 kHz; no audio is resampled. Phonic's turn-taking
//! and barge-in are its own. Its claims are [`claims`], and `records/claims-duplex.json` holds the
//! same; `native_duplex` is not among them, since Phonic answers speech over a response by
//! interrupting it.
//!
//! The key is never read by the binding itself: the caller injects a [`CredentialSource`] and
//! names the secret ([`EnvironmentSource::phonic`] reads `PHONIC_API_KEY`; with feature
//! `keychain`, `KeychainSource` reads the OS keychain through the `secrets` library). It is sent
//! as `Authorization: Bearer <key>` in the handshake, marked sensitive, and never kept, printed,
//! logged or put in an error. The WebSocket library below does log the raw handshake request at
//! trace level under targets beginning with `tungstenite`; an embedder that installs a logger
//! keeps those targets below trace. `ws://` is taken only to a loopback host.
//!
//! The key is copied only where the handshake needs it, and no field of the binding holds it past
//! the handshake. Every copy:
//!
//! - the [`Credential`] the source returns, zeroized on drop;
//! - for each handshake, one zeroizing buffer holding `Bearer <key>`, built from the credential,
//!   which is dropped at once. The `Authorization` header value is that buffer itself, not a copy
//!   of it, and the buffer is zeroized when tungstenite has turned the request into bytes, or
//!   when the handshake fails or times out first;
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
//! The live check is `tests/live.rs`, behind feature `live` and ignored: with a key in
//! `PHONIC_API_KEY` (or the keychain), `cargo test -p inference-phonic --features live --test
//! live -- --ignored` holds one spoken exchange. It is billed.

mod credential;
mod duplex;
pub mod protocol;

#[cfg(feature = "keychain")]
pub use credential::KeychainSource;
pub use credential::{
    Credential, CredentialError, CredentialFuture, CredentialSource, EnvironmentSource,
    PHONIC_KEY_NAME, PHONIC_KEY_VARIABLE,
};
pub use duplex::{
    BACKEND, CLOSE_BOUND, INBOUND_QUEUE, OUTBOUND_QUEUE, PhonicSession, PhonicSpeech, capabilities,
    claims, native_duplex_verdict,
};
pub use inference_duplex;
pub use protocol::{DEFAULT_MODEL, DEFAULT_URL, PhonicConfig, PhonicEndpoint, PhonicError};
