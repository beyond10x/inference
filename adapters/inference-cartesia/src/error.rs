//! The errors of the binding: a configuration it refuses, and why a synthesis failed.

use std::fmt;
use std::time::Duration;

use crate::credential::CredentialError;

/// Why a configuration was refused before anything connected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// The sample rate is not one the endpoint takes: 8000, 16000, 22050, 24000, 44100 or 48000.
    UnsupportedSampleRate(u32),
    /// The voice id is empty, too long or not ASCII letters, digits, `-` and `_`.
    InvalidVoiceId,
    /// The model id is empty, too long or not ASCII letters, digits, `-`, `_` and `.`.
    InvalidModelId,
    /// The language is empty, too long or not ASCII letters, `-` and `_`.
    InvalidLanguage,
    /// The base URL is not `wss://`, or `ws://` to a loopback host, or carries a query.
    InvalidUrl(String),
    /// The maximum concurrency is not positive.
    MaxConcurrency(i64),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSampleRate(rate) => write!(
                f,
                "sample rate {rate} Hz is not one of 8000, 16000, 22050, 24000, 44100 and 48000"
            ),
            Self::InvalidVoiceId => {
                f.write_str("the voice id must be 1 to 128 ASCII letters, digits, `-` or `_`")
            }
            Self::InvalidModelId => {
                f.write_str("the model id must be 1 to 128 ASCII letters, digits, `-`, `_` or `.`")
            }
            Self::InvalidLanguage => {
                f.write_str("the language must be 1 to 16 ASCII letters, `-` or `_`")
            }
            Self::InvalidUrl(detail) => write!(f, "invalid base URL: {detail}"),
            Self::MaxConcurrency(max) => write!(f, "maximum concurrency {max} is not positive"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Why a synthesis failed. No variant carries the credential or any part of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CartesiaError {
    /// The injected source refused the credential name, or the credential it returned cannot be
    /// sent as a header value (`Invalid`).
    Credential(CredentialError),
    /// The opening handshake failed: refused by the server (HTTP 401 for a bad key), not
    /// answered within the connect timeout, or the connection could not be made.
    /// [`ConnectError::status`] tells them apart.
    Connect(ConnectError),
    /// Writing or reading the open connection failed.
    Transport(String),
    /// The server closed the connection while a segment was being spoken, with its close code
    /// and reason when it gave one.
    Closed { code: Option<u16>, reason: String },
    /// No message arrived for the read timeout, which this carries, or a write did not finish
    /// within it.
    Stalled(Duration),
    /// The server sent something that is not a message of the protocol: a binary frame, text
    /// that is not a JSON object, an unknown `type`, a `data` that is not base64, a context the
    /// binding never opened, or PCM audio ending in part of a sample.
    Protocol(String),
    /// The server sent an `error` message: its `status_code`, its `error_code` when it gave one,
    /// and its `title` and `message`, at most 256 characters of each.
    Server(ServerError),
    /// The runtime the synthesis drives its connection on could not be built, or the thread
    /// that drives it on a caller's runtime thread could not be started or panicked.
    Runtime(String),
}

impl fmt::Display for CartesiaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Credential(error) => write!(f, "credential refused: {error}"),
            Self::Connect(detail) => write!(f, "connection failed: {detail}"),
            Self::Transport(detail) => write!(f, "transport failed: {detail}"),
            Self::Closed { code, reason } => match code {
                Some(code) => write!(f, "server closed the connection ({code}): {reason}"),
                None => write!(f, "server closed the connection"),
            },
            Self::Stalled(waited) => write!(f, "no server message within {waited:?}"),
            Self::Protocol(detail) => write!(f, "protocol violation: {detail}"),
            Self::Server(error) => write!(f, "server error: {error}"),
            Self::Runtime(detail) => write!(f, "the synthesis's runtime failed: {detail}"),
        }
    }
}

impl std::error::Error for CartesiaError {}

/// An `error` message of the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerError {
    /// `status_code`, an HTTP status, when it is one.
    pub status: Option<u16>,
    /// `error_code`, such as `quota_exceeded` or `concurrency_limited`, when the server gave one.
    pub code: Option<String>,
    pub title: String,
    pub message: String,
}

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(status) = self.status {
            write!(f, "{status} ")?;
        }
        if let Some(code) = &self.code {
            write!(f, "{code} ")?;
        }
        write!(f, "{}: {}", self.title, self.message)
    }
}

/// Why the opening handshake failed: the HTTP status when the server answered the upgrade with
/// one (401 or 403 for a refused key), and a description that carries no part of the credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectError {
    pub status: Option<u16>,
    pub detail: String,
}

impl ConnectError {
    /// Whether the server refused the credential: HTTP 401 or 403.
    pub fn is_refusal(&self) -> bool {
        matches!(self.status, Some(401 | 403))
    }
}

impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail)
    }
}
