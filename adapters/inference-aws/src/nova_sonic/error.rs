//! What can go wrong in a Nova Sonic session.

use std::fmt;
use std::time::Duration;

/// A failure of the client itself, as opposed to an error event the service sent.
#[derive(Debug, Clone, PartialEq)]
pub enum NovaSonicError {
    /// The session was quiet for its read timeout: no frame sent and none received.
    Stall {
        /// The read timeout that elapsed.
        after: Duration,
    },
    /// An output frame was not an event of the reference.
    Decode(String),
    /// The transport's input side is closed: the stream ended or was dropped.
    Closed,
    /// The stream could not be opened. The message carries no account identifier.
    Connect(String),
}

impl fmt::Display for NovaSonicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NovaSonicError::Stall { after } => {
                write!(f, "no frame either way within {} ms", after.as_millis())
            }
            NovaSonicError::Decode(message) => write!(f, "undecodable output frame: {message}"),
            NovaSonicError::Closed => write!(f, "the stream's input side is closed"),
            NovaSonicError::Connect(message) => write!(f, "the stream did not open: {message}"),
        }
    }
}

impl std::error::Error for NovaSonicError {}

/// The exception kinds `InvokeModelWithBidirectionalStream` reports on its output stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceErrorKind {
    /// `validationException`, HTTP 400.
    Validation,
    /// `throttlingException`, HTTP 429.
    Throttling,
    /// `modelTimeoutException`, HTTP 408: no input arrived within the service's timeout.
    ModelTimeout,
    /// `modelStreamErrorException`, HTTP 424.
    ModelStreamError,
    /// `internalServerException`, HTTP 500.
    InternalServer,
    /// `serviceUnavailableException`, HTTP 503.
    ServiceUnavailable,
    /// Any other error code, or a transport failure, kept by name.
    Other(String),
}

/// An error the service reported on the output stream, delivered as [`super::OutputEvent::Error`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceError {
    /// Which exception it was.
    pub kind: ServiceErrorKind,
    /// The service's message, with ARNs and account numbers removed.
    pub message: String,
}

impl ServiceError {
    /// An error of `kind` with `message`, redacted of ARNs and account numbers.
    pub fn new(kind: ServiceErrorKind, message: impl AsRef<str>) -> Self {
        ServiceError {
            kind,
            message: redact(message.as_ref()),
        }
    }
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for ServiceError {}

/// `message` with every ARN and every run of twelve or more digits replaced, so that no account
/// identifier, role or session name reaches a log: for a model id given as an inference-profile
/// ARN, or a message that might quote one.
pub fn redact(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut rest = message;
    while !rest.is_empty() {
        if rest.starts_with("arn:") && !out.ends_with(|c: char| c.is_ascii_alphanumeric()) {
            let end = rest
                .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ')' | ']'))
                .unwrap_or(rest.len());
            out.push_str("<arn>");
            rest = &rest[end..];
            continue;
        }
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits >= 12 {
            out.push_str("<id>");
            rest = &rest[digits..];
            continue;
        }
        if digits > 0 {
            out.push_str(&rest[..digits]);
            rest = &rest[digits..];
            continue;
        }
        let c = rest.chars().next().unwrap_or_default();
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}
