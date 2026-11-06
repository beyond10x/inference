//! The errors of the binding: a configuration it refuses, and why an open or a session failed.

use std::fmt;
use std::time::Duration;

use crate::credential::CredentialError;
use crate::protocol::ErrorCategory;

/// Why an endpoint or a configuration was refused before anything connected. No variant
/// carries the URL or an id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// The base URL is not one the binding takes; the reason, never the URL.
    InvalidUrl(&'static str),
    /// The vendor id, the organization id or the model, named, is not a plain path segment.
    PathSegment(&'static str),
    /// The ping interval is zero.
    PingInterval,
    /// The environment variable, named, is not set.
    Missing(&'static str),
    /// The adapter record could not be declared with a concurrency of one.
    MaxConcurrency,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl(reason) => write!(f, "invalid base URL: {reason}"),
            Self::PathSegment(what) => write!(
                f,
                "the {what} must be 1 to 128 ASCII letters, digits, `-`, `_` or `.`"
            ),
            Self::PingInterval => f.write_str("the ping interval must not be zero"),
            Self::Missing(variable) => write!(f, "{variable} is not set"),
            Self::MaxConcurrency => f.write_str("the adapter record could not be declared"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Why an open or a session failed. No variant carries the credential or any part of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeepslateError {
    /// The injected source refused the credential name, or the credential it returned cannot be
    /// sent as a header value (`Invalid`).
    Credential(CredentialError),
    /// The opening handshake failed: refused by the server (HTTP 401 or 403 for a bad key or a
    /// model the organization may not use, 404 for an unknown model), not answered within the
    /// open timeout, or the connection could not be made. [`ConnectError::status`] tells them
    /// apart.
    Connect(ConnectError),
    /// Writing or reading the open connection failed, or a frame was larger than
    /// [`crate::MAX_MESSAGE_SIZE`].
    Transport(String),
    /// The server closed the connection, with the close code and reason when it gave one.
    Closed { code: Option<u16>, reason: String },
    /// The connection sent nothing within the read timeout, or a write did not finish within
    /// it, or `SessionReady` did not arrive within the open timeout; the bound it carries.
    Stalled(Duration),
    /// The server sent a `SessionErrorNotification`: its category, its message, at most 256
    /// characters, and its trace id when it gave one.
    Server {
        category: ErrorCategory,
        message: String,
        trace_id: Option<String>,
    },
    /// The server closed the connection before `SessionReady`.
    ClosedBeforeReady { code: Option<u16>, reason: String },
    /// The model asked for a tool the session does not offer.
    ToolNotOffered { name: String },
    /// The thread or runtime that drives the connection could not be started.
    Runtime(String),
    /// The endpoint was refused before connecting.
    Config(ConfigError),
}

impl fmt::Display for DeepslateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Credential(error) => write!(f, "credential refused: {error}"),
            Self::Connect(detail) => write!(f, "connection failed: {detail}"),
            Self::Transport(detail) => write!(f, "transport failed: {detail}"),
            Self::Closed { code, reason } => match code {
                Some(code) => write!(f, "the server closed the connection ({code}): {reason}"),
                None => f.write_str("the connection closed"),
            },
            Self::Stalled(waited) => write!(f, "nothing from the server within {waited:?}"),
            Self::Server {
                category,
                message,
                trace_id,
            } => match trace_id {
                Some(trace) => write!(f, "server error {category}: {message} (trace {trace})"),
                None => write!(f, "server error {category}: {message}"),
            },
            Self::ClosedBeforeReady { code, reason } => match code {
                Some(code) => write!(
                    f,
                    "the server closed the connection before SessionReady ({code}): {reason}"
                ),
                None => f.write_str("the server closed the connection before SessionReady"),
            },
            Self::ToolNotOffered { name } => write!(
                f,
                "the model asked for the tool `{name}`, which the session does not offer"
            ),
            Self::Runtime(detail) => write!(f, "the session's runtime failed: {detail}"),
            Self::Config(error) => write!(f, "configuration refused: {error}"),
        }
    }
}

impl std::error::Error for DeepslateError {}

/// Why the opening handshake failed: the HTTP status when the server answered the upgrade with
/// one, and a description that carries no part of the credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectError {
    pub status: Option<u16>,
    pub detail: String,
}

impl ConnectError {
    /// Whether the server refused the session: HTTP 400 (a header value), 401 or 403 (the key,
    /// or a model or feature the organization lacks) or 404 (an unknown model).
    #[must_use]
    pub fn is_refusal(&self) -> bool {
        matches!(self.status, Some(400 | 401 | 403 | 404))
    }
}

impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail)
    }
}
