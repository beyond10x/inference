//! A client for the OpenAI Realtime API over WebSocket.
//!
//! `inference-openai` is a hosted adapter of the inference framework. It opens one Realtime
//! session ([`RealtimeClient::connect`]) with a model id and a [`SessionConfig`], sends client
//! events (input audio, commit, response create and cancel, truncation of played audio, text items
//! and tool results) and receives server events as typed [`ServerEvent`] values. A server event
//! the client has no type for is kept as [`ServerEvent::Unknown`], never dropped. No message for
//! the endpoint's read timeout, measured from the last message read across every read, is
//! [`RealtimeError::Stalled`].
//!
//! Two more deadlines bound the transport, held in [`Deadlines`]: the whole opening handshake
//! (TCP, TLS and the HTTP upgrade) takes at most [`Deadlines::connect_timeout`]
//! ([`DEFAULT_CONNECT_TIMEOUT`], 10 seconds), past which connecting fails as
//! [`RealtimeError::Connect`] without a status; and each write of a client event or a ping takes
//! at most [`Deadlines::write_timeout`] ([`DEFAULT_WRITE_TIMEOUT`], 10 seconds), past which it
//! fails as [`RealtimeError::Transport`], so a peer that stops reading cannot hold a session
//! operation. [`RealtimeClient::connect_with`] and, with their features, `LiveClient::connect_with`
//! and the duplex bindings' `with_connect_timeout` and `with_write_timeout` set them. The URL is `wss://`, or `ws://` only to a loopback host (`localhost`,
//! `127.0.0.0/8`, `[::1]`): `ws://` would send the key in the clear, so any other URL is refused
//! as [`RealtimeError::InvalidEndpoint`] before the credential is resolved or anything connects.
//!
//! Event shapes follow the published Realtime API reference, client events
//! (`https://developers.openai.com/api/reference/resources/realtime/client-events`) and server
//! events (`https://developers.openai.com/api/reference/resources/realtime/server-events`), in
//! the generally available interface.
//!
//! The client never reads a key itself: the caller injects a [`CredentialSource`] and names the
//! secret to resolve ([`EnvironmentSource`]; with feature `keychain`, `KeychainSource` over the OS
//! keychain through the `secrets` library). The credential is sent once, in the opening
//! handshake, and this crate does not keep, print or log it.
//!
//! The WebSocket library below it does log it: `tungstenite` writes the raw handshake request,
//! the `Authorization: Bearer` header with the key included, at trace level through the `log`
//! facade, under targets beginning with `tungstenite`. A library cannot cap the logger of the
//! program that embeds it, so an embedder that installs a logger must keep those targets below
//! trace, for example with the filter `tungstenite=debug` (`RUST_LOG` for `env_logger`, or an
//! `EnvFilter` directive for `tracing-subscriber` with its `log` bridge).
//!
//! Audio goes in either as PCM16 at 24 kHz, resampled from `inference-audio` frames where needed
//! ([`encode_pcm16_24k`]), or as G.711 mu-law or A-law at 8 kHz, passed through
//! ([`RealtimeClient::append_encoded`]).
//!
//! [`RealtimeClient::into_split`] splits a session into a [`RealtimeSender`] and a
//! [`RealtimeReceiver`] that two tasks own, so server events are read while a write is pending.
//!
//! With feature `duplex` ([`duplex`]), the client is an `inference-duplex` native speech model:
//! [`duplex::OpenAiRealtime`] opens sessions through the contract and records every server
//! event through its `SessionLedger`.
//!
//! With feature `gpt-live`, `live` is a client for GPT-Live (`v1/live/sessions`, model
//! `gpt-live-1`), OpenAI's full-duplex voice model that delegates reasoning and tools to a
//! backend; it shares the credential sources and the transport. With `gpt-live` and `duplex`,
//! `live_duplex::OpenAiLive` binds it to the same duplex contract, so the two can be swapped and
//! compared.
//!
//! With feature `speech`, `speech::OpenAiSpeech` is a synthesiser of the `inference-tts` contract
//! over the streamed speech endpoint (`POST /v1/audio/speech`), its frames resampled and, where
//! asked, G.711-encoded by `inference-audio`; it shares the credential sources.

mod audio;
mod client;
mod config;
mod credential;
#[cfg(feature = "duplex")]
pub mod duplex;
mod error;
mod event;
#[cfg(feature = "gpt-live")]
pub mod live;
#[cfg(all(feature = "gpt-live", feature = "duplex"))]
pub mod live_duplex;
#[cfg(feature = "speech")]
pub mod speech;

pub use audio::encode_pcm16_24k;
pub use client::{
    ConversationItem, ItemOptions, RealtimeClient, RealtimeReceiver, RealtimeSender,
    ResponseConversation, ResponseOptions, ReuniteError,
};
pub use config::{
    DEFAULT_CONNECT_TIMEOUT, DEFAULT_MODEL, DEFAULT_READ_TIMEOUT, DEFAULT_URL,
    DEFAULT_WRITE_TIMEOUT, Deadlines, Eagerness, Endpoint, FunctionTool, MODEL_VARIABLE,
    NoiseReduction, OutputModality, SessionConfig, ToolChoice, TurnDetection, WireFormat,
};
#[cfg(feature = "keychain")]
pub use credential::KeychainSource;
pub use credential::{
    Credential, CredentialError, CredentialFuture, CredentialSource, EnvironmentSource,
    OPENAI_KEY_NAME, OPENAI_KEY_VARIABLE,
};
pub use error::{ConnectError, RealtimeError};
pub use event::{ResponseStatus, ServerError, ServerEvent, TranscriptionUsage, Usage};
