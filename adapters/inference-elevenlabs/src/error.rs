//! The errors of the binding: a configuration it refuses, and why a synthesis failed.

use std::fmt;
use std::time::Duration;

use crate::credential::CredentialError;

/// Why a configuration was refused before anything connected.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigError {
    /// The `output_format` is not one the binding takes, with why.
    UnsupportedOutputFormat { name: String, reason: &'static str },
    /// The voice id is empty, too long or not a plain URL path segment.
    InvalidVoiceId,
    /// The model id is empty, too long or not a plain query value.
    InvalidModelId,
    /// The `inactivity_timeout` is outside 1 to 180 seconds.
    InactivityTimeout(u32),
    /// The voice speed is outside 0.7 to 1.2.
    Speed(f64),
    /// The base URL is not `ws://` or `wss://`, or carries a query.
    InvalidUrl(String),
    /// The maximum concurrency is not positive.
    MaxConcurrency(i64),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedOutputFormat { name, reason } => {
                write!(f, "output format {name} is refused: {reason}")
            }
            Self::InvalidVoiceId => {
                f.write_str("the voice id must be 1 to 128 ASCII letters, digits, `-` or `_`")
            }
            Self::InvalidModelId => {
                f.write_str("the model id must be 1 to 128 ASCII letters, digits, `-`, `_` or `.`")
            }
            Self::InactivityTimeout(seconds) => write!(
                f,
                "inactivity timeout {seconds} s is outside 1 to 180 seconds"
            ),
            Self::Speed(speed) => write!(f, "voice speed {speed} is outside 0.7 to 1.2"),
            Self::InvalidUrl(detail) => write!(f, "invalid base URL: {detail}"),
            Self::MaxConcurrency(max) => write!(f, "maximum concurrency {max} is not positive"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Why a synthesis failed. No variant carries the credential or any part of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElevenLabsError {
    /// The injected source refused the credential name, or the credential it returned cannot be
    /// sent as a header value (`Invalid`).
    Credential(CredentialError),
    /// The opening handshake failed: refused by the server (HTTP 401 for a bad key), not
    /// answered within the connect timeout, or the connection could not be made.
    /// [`ConnectError::status`] tells them apart.
    Connect(ConnectError),
    /// Writing or reading the open connection failed.
    Transport(String),
    /// The server closed the connection before the segment's final message, with its close
    /// code and reason when it gave one.
    Closed { code: Option<u16>, reason: String },
    /// No message arrived for the read timeout, which this carries, or a write did not finish
    /// within it.
    Stalled(Duration),
    /// The server sent something that is not a message of the protocol: a binary frame, text
    /// that is not a JSON object, an `audio` that is not base64 or a string, an `isFinal` that
    /// is not a boolean, an object with neither, or PCM audio ending in half a sample.
    Protocol(String),
    /// The server answered with an error message instead of audio: the `message`, `error` or
    /// `detail` it sent, at most 256 characters of it.
    Server(String),
    /// The runtime the synthesis drives its connection on could not be built, or the thread
    /// that drives it on a caller's runtime thread could not be started or panicked.
    Runtime(String),
}

impl fmt::Display for ElevenLabsError {
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
            Self::Server(message) => write!(f, "server error: {message}"),
            Self::Runtime(detail) => write!(f, "the synthesis's runtime failed: {detail}"),
        }
    }
}

impl std::error::Error for ElevenLabsError {}

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
