//! The errors of the Realtime client.

use std::fmt;
use std::time::Duration;

use crate::credential::CredentialError;
use crate::event::ServerError;

/// Why a Realtime operation failed. No variant carries the credential or any part of it.
#[derive(Debug)]
pub enum RealtimeError {
    /// The injected source refused the credential name, or the credential it returned cannot be
    /// sent as a bearer token (`Invalid`).
    Credential(CredentialError),
    /// The endpoint URL or the model id cannot form a request.
    InvalidEndpoint(String),
    /// The opening handshake failed: the server refused it (for example HTTP 401) or the
    /// connection could not be made.
    Connect(String),
    /// Reading or writing the open connection failed.
    Transport(String),
    /// The server closed the connection, with its close code and reason when it gave one.
    Closed { code: Option<u16>, reason: String },
    /// No server event arrived within the read timeout, which this carries.
    Stalled(Duration),
    /// The server sent something that is not a Realtime event: a binary frame, text that is not a
    /// JSON object with a `type`, or a typed event missing a field the reference requires.
    Protocol(String),
    /// The server answered session setup with an `error` event.
    Server(ServerError),
    /// The audio cannot be sent: its data does not match its format, or the session's input
    /// format does not take it.
    Audio(String),
}

impl fmt::Display for RealtimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Credential(error) => write!(f, "credential refused: {error}"),
            Self::InvalidEndpoint(detail) => write!(f, "invalid endpoint: {detail}"),
            Self::Connect(detail) => write!(f, "connection failed: {detail}"),
            Self::Transport(detail) => write!(f, "transport failed: {detail}"),
            Self::Closed { code, reason } => match code {
                Some(code) => write!(f, "server closed the connection ({code}): {reason}"),
                None => write!(f, "server closed the connection"),
            },
            Self::Stalled(waited) => write!(f, "no server event within {waited:?}"),
            Self::Protocol(detail) => write!(f, "protocol violation: {detail}"),
            Self::Server(error) => write!(f, "server error during setup: {error}"),
            Self::Audio(detail) => write!(f, "audio refused: {detail}"),
        }
    }
}

impl std::error::Error for RealtimeError {}
