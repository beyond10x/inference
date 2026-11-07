//! Deepslate Opal, an end-to-end speech-to-speech model hosted in the EU, as a native speech
//! model of the `inference-duplex` contract.
//!
//! `inference-deepslate` is a hosted adapter of the inference framework, at locality
//! `external_provider`: the audio of every session leaves the machine for Deepslate's realtime
//! endpoint, `wss://app.deepslate.eu/api/v1/vendors/<vendor>/organizations/<organization>/realtime`
//! ([`Endpoint`]). [`DeepslateOpal`] opens a session over that WebSocket and records what the
//! server sends through the contract's ledger; `opal` says how a session maps onto the
//! contract. [`protocol`] is the wire protocol, protocol buffers messages in binary frames, with
//! the documentation it was taken from and its date; `protocol/README.md` beside this crate
//! summarises the pages it was read from and what they leave open.
//!
//! Its claims are [`claims`], and `records/claims-opal.json` holds the same. `native_duplex` is
//! not among them: audio sent with the documented `IMMEDIATE` trigger interrupts the inference
//! running and starts a new one, so speech over an answer cuts it and the answer to it is a new
//! response ([`native_duplex_verdict`]).
//!
//! # The key
//!
//! The key is never read by the binding itself: the caller injects a [`CredentialSource`] and
//! names the secret ([`EnvironmentSource::deepslate`] reads `DEEPSLATE_API_KEY`; with feature
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
//!   when the handshake fails first;
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
//! `DEEPSLATE_API_KEY` (or the keychain), the vendor and organization in `DEEPSLATE_VENDOR_ID`
//! and `DEEPSLATE_ORGANIZATION_ID` and a hosted voice in `DEEPSLATE_VOICE_ID`,
//! `cargo test -p inference-deepslate --features live --test live -- --ignored --nocapture`
//! asks Opal a question. It is billed.

mod config;
mod connection;
mod credential;
mod error;
mod opal;
pub mod protocol;
mod wire;

pub use config::{
    BASE_URL, DEFAULT_OPEN_TIMEOUT, DEFAULT_PING_INTERVAL, DEFAULT_READ_TIMEOUT, Endpoint,
    MAX_PATH_SEGMENT, MODEL_VARIABLE, ORGANIZATION_VARIABLE, OpalConfig, VENDOR_VARIABLE,
};
pub use connection::{CLOSE_BOUND, MAX_MESSAGE_SIZE};
#[cfg(feature = "keychain")]
pub use credential::KeychainSource;
pub use credential::{
    Credential, CredentialError, CredentialFuture, CredentialSource, DEEPSLATE_KEY_NAME,
    DEEPSLATE_KEY_VARIABLE, EnvironmentSource,
};
pub use error::{ConfigError, ConnectError, DeepslateError};
pub use inference_duplex;
pub use opal::{
    BACKEND, DeepslateOpal, DeepslateOpalSession, INBOUND_DEPTH, MODEL, OUTBOUND_QUEUE,
    SAMPLE_RATES, TEXT_LIMIT, capabilities, claims, native_duplex_verdict, tool_unavailable,
};
