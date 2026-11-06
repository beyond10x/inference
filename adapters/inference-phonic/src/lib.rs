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
