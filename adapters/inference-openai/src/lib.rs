//! A client for the OpenAI Realtime API over WebSocket.
//!
//! `inference-openai` is a hosted adapter of the inference framework. It opens one Realtime
//! session ([`RealtimeClient::connect`]) with a model id and a [`SessionConfig`], sends client
//! events (input audio, commit, response create and cancel, truncation of played audio, text items
//! and tool results) and receives server events as typed [`ServerEvent`] values. A server event
//! the client has no type for is kept as [`ServerEvent::Unknown`], never dropped. A read that waits
//! longer than the endpoint's read timeout is [`RealtimeError::Stalled`].
//!
//! Event shapes follow the published Realtime API reference, client events
//! (`https://developers.openai.com/api/reference/resources/realtime/client-events`) and server
//! events (`https://developers.openai.com/api/reference/resources/realtime/server-events`), in
//! the generally available interface.
//!
//! The client never reads a key itself: the caller injects a [`CredentialSource`] and names the
//! secret to resolve ([`EnvironmentSource`]; with feature `keychain`, `KeychainSource` over the OS
//! keychain through the `secrets` library). The credential is sent once, in the opening
//! handshake, and is not kept, printed or logged.
//!
//! Audio goes in either as PCM16 at 24 kHz, resampled from `inference-audio` frames where needed
//! ([`encode_pcm16_24k`]), or as G.711 mu-law or A-law at 8 kHz, passed through
//! ([`RealtimeClient::append_encoded`]).
//!
//! With feature `duplex` ([`duplex`]), the client is an `inference-duplex` native speech model:
//! [`duplex::OpenAiRealtime`] opens sessions through the contract and records every server
//! event through its `SessionLedger`.

mod audio;
mod client;
mod config;
mod credential;
#[cfg(feature = "duplex")]
pub mod duplex;
mod error;
mod event;

pub use audio::encode_pcm16_24k;
pub use client::RealtimeClient;
pub use config::{
    DEFAULT_MODEL, DEFAULT_READ_TIMEOUT, DEFAULT_URL, Eagerness, Endpoint, FunctionTool,
    MODEL_VARIABLE, OutputModality, SessionConfig, TurnDetection, WireFormat,
};
#[cfg(feature = "keychain")]
pub use credential::KeychainSource;
pub use credential::{
    Credential, CredentialError, CredentialFuture, CredentialSource, EnvironmentSource,
    OPENAI_KEY_NAME, OPENAI_KEY_VARIABLE,
};
pub use error::RealtimeError;
pub use event::{ResponseStatus, ServerError, ServerEvent, Usage};
