//! The errors of the binding: a configuration it refuses, and why a recognition failed.

use std::fmt;
use std::time::Duration;

use crate::credential::CredentialError;

/// Why a configuration was refused before anything connected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// The model is empty, too long or not a plain query value.
    InvalidModel,
    /// The language is empty, too long or not a plain query value.
    InvalidLanguage,
    /// The `utterance_end_ms` is outside 1000 to 5000.
    UtteranceEndMs(u32),
    /// The keep-alive interval is zero.
    KeepAliveInterval,
    /// The base URL is not `wss://`, or `ws://` to a loopback host, or carries a query. The
    /// detail never carries the URL.
    InvalidUrl(String),
    /// The maximum concurrency is not positive.
    MaxConcurrency(i64),
    /// The Voice Agent's `Authorization` scheme is not 1 to 32 ASCII letters.
    AuthScheme,
    /// A Voice Agent provider's `type` or `model` is empty, or longer than 128 characters.
    AgentProvider,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidModel => {
                f.write_str("the model must be 1 to 128 ASCII letters, digits, `-`, `_` or `.`")
            }
            Self::InvalidLanguage => {
                f.write_str("the language must be 1 to 128 ASCII letters, digits or `-`")
            }
            Self::UtteranceEndMs(ms) => {
                write!(f, "utterance_end_ms {ms} is outside 1000 to 5000")
            }
            Self::KeepAliveInterval => f.write_str("the keep-alive interval must not be zero"),
            Self::InvalidUrl(detail) => write!(f, "invalid base URL: {detail}"),
            Self::MaxConcurrency(max) => write!(f, "maximum concurrency {max} is not positive"),
            Self::AuthScheme => {
                f.write_str("the Authorization scheme must be 1 to 32 ASCII letters")
            }
            Self::AgentProvider => f.write_str(
                "a Voice Agent provider's type and model must each be 1 to 128 characters",
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

/// What a close frame's reason says, by the codes the endpoint documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CloseCause {
    /// 1008 `DATA-0000`: the audio could not be decoded.
    UndecodableAudio,
    /// 1011 `NET-0000`: the server timed out.
    ServerTimeout,
    /// 1011 `NET-0001`: no frame arrived within ten seconds of opening.
    NoFrameAfterOpen,
    /// 1011 `NET-0002`: no audio arrived for the server's own timeout.
    NoAudio,
    /// Any other code or reason, or none.
    Other,
}

impl CloseCause {
    /// The cause of a close with `code` and `reason`: the documented reason must begin the
    /// reason text and come with its documented code.
    pub fn of(code: Option<u16>, reason: &str) -> Self {
        let begins = |prefix: &str| reason.trim_start().starts_with(prefix);
        match code {
            Some(1008) if begins("DATA-0000") => Self::UndecodableAudio,
            Some(1011) if begins("NET-0000") => Self::ServerTimeout,
            Some(1011) if begins("NET-0001") => Self::NoFrameAfterOpen,
            Some(1011) if begins("NET-0002") => Self::NoAudio,
            _ => Self::Other,
        }
    }
}

/// Why a recognition failed. No variant carries the credential or any part of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeepgramError {
    /// The injected source refused the credential name, or the credential it returned cannot be
    /// sent as a header value (`Invalid`).
    Credential(CredentialError),
    /// The opening handshake failed: refused by the server (HTTP 401 or 403 for a bad key), not
    /// answered within the connect timeout, or the connection could not be made.
    /// [`ConnectError::status`] tells them apart.
    Connect(ConnectError),
    /// Writing or reading the open connection failed.
    Transport(String),
    /// The connection closed before the summary `Metadata`, with the close code and reason when
    /// the server gave one, and what that reason means.
    Closed {
        code: Option<u16>,
        reason: String,
        cause: CloseCause,
    },
    /// The summary `Metadata` did not arrive within the total deadline after the end of input,
    /// which this carries, or a write did not finish within the read timeout.
    Stalled(Duration),
    /// The server sent something that is not a message of the protocol: a binary frame, text
    /// that is not a JSON object, an object without a string `type`, or a known message missing
    /// what the binding reads of it.
    Protocol(String),
    /// The server sent an `Error` message: its `code` when given and its `description`, or else
    /// its `message`, at most 256 characters of each.
    Server {
        code: Option<String>,
        description: String,
    },
    /// A frame's audio is not one the binding can describe to the endpoint: a format other than
    /// μ-law or A-law at 8 kHz or 16-bit PCM, more than one channel, or a format other than the
    /// recognition's first frame's.
    UnsupportedAudio(String),
    /// The thread or runtime that drives the connection could not be started.
    Runtime(String),
    /// The caller cancelled the opening of a Voice Agent session.
    Cancelled,
    /// The Voice Agent closed the connection before `SettingsApplied`, with the close code and
    /// reason when it gave one.
    ClosedBeforeSettings { code: Option<u16>, reason: String },
    /// The Voice Agent asked for a client-side function the session does not offer.
    FunctionNotOffered { name: String },
}

impl fmt::Display for DeepgramError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Credential(error) => write!(f, "credential refused: {error}"),
            Self::Connect(detail) => write!(f, "connection failed: {detail}"),
            Self::Transport(detail) => write!(f, "transport failed: {detail}"),
            Self::Closed { code, reason, .. } => match code {
                Some(code) => write!(f, "server closed the connection ({code}): {reason}"),
                None => f.write_str("the connection closed before the summary metadata"),
            },
            Self::Stalled(waited) => write!(f, "no server message within {waited:?}"),
            Self::Protocol(detail) => write!(f, "protocol violation: {detail}"),
            Self::Server { code, description } => match code {
                Some(code) => write!(f, "server error {code}: {description}"),
                None => write!(f, "server error: {description}"),
            },
            Self::UnsupportedAudio(detail) => write!(f, "unsupported audio: {detail}"),
            Self::Runtime(detail) => write!(f, "the recognition's runtime failed: {detail}"),
            Self::Cancelled => f.write_str("the caller cancelled the open"),
            Self::ClosedBeforeSettings { code, reason } => match code {
                Some(code) => write!(
                    f,
                    "the server closed the connection before SettingsApplied ({code}): {reason}"
                ),
                None => f.write_str("the server closed the connection before SettingsApplied"),
            },
            Self::FunctionNotOffered { name } => write!(
                f,
                "the agent asked for the client-side function `{name}`, which the session does not offer"
            ),
        }
    }
}

impl std::error::Error for DeepgramError {}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_documented_close_is_its_own_cause() {
        assert_eq!(
            CloseCause::of(Some(1008), "DATA-0000"),
            CloseCause::UndecodableAudio
        );
        assert_eq!(
            CloseCause::of(Some(1011), "NET-0000"),
            CloseCause::ServerTimeout
        );
        assert_eq!(
            CloseCause::of(Some(1011), "NET-0001 no audio"),
            CloseCause::NoFrameAfterOpen
        );
        assert_eq!(CloseCause::of(Some(1011), "NET-0002"), CloseCause::NoAudio);
        assert_eq!(CloseCause::of(Some(1008), "NET-0000"), CloseCause::Other);
        assert_eq!(CloseCause::of(Some(1000), ""), CloseCause::Other);
        assert_eq!(CloseCause::of(None, ""), CloseCause::Other);
    }
}
