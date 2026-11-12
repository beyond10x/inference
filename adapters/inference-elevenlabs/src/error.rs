//! The errors of the binding: a configuration it refuses, and why a synthesis or a recognition
//! failed.

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
    /// The `audio_format` of a recognition is not one the realtime speech-to-text endpoint
    /// documents, with why.
    UnsupportedInputFormat { name: String, reason: &'static str },
    /// The language code is not two or three ASCII letters (ISO 639-1 or ISO 639-3).
    InvalidLanguageCode,
    /// A key term is blank.
    InvalidKeyterm,
    /// A VAD threshold is not a finite number of at least zero.
    VadSetting { name: &'static str, value: f64 },
    /// `ELEVENLABS_RESIDENCY` names no residency; carries what it was set to.
    Residency(String),
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
            Self::UnsupportedInputFormat { name, reason } => {
                write!(f, "input format {name} is refused: {reason}")
            }
            Self::InvalidLanguageCode => {
                f.write_str("the language code must be two or three ASCII letters")
            }
            Self::InvalidKeyterm => f.write_str("a key term is blank"),
            Self::VadSetting { name, value } => {
                write!(f, "{name} {value} is not a finite number of at least zero")
            }
            Self::Residency(value) => write!(
                f,
                "{} is {value}; it takes global, us, eu, in or sg, or is unset for global",
                crate::config::RESIDENCY_VARIABLE
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Why a synthesis or a recognition failed. No variant carries the credential or any part of it.
///
/// The variants from [`ElevenLabsError::ScribeError`] to [`ElevenLabsError::TranscriberError`]
/// are the fourteen error types of the realtime speech-to-text endpoint, one each
/// (`stt_protocol::ERROR_TYPES`, feature `stt`), each carrying
/// the endpoint's `error` text, at most 256 characters of it.
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
    /// that drives it on a caller's runtime thread could not be started or panicked; for a
    /// recognition, the runtime or the thread that drives its connection could not be started,
    /// or that thread stopped without a word.
    Runtime(String),
    /// A frame offered to a recognition is not in the configured input format, or its bytes do
    /// not hold its samples.
    UnsupportedAudio(String),
    /// `error`: a transcription error the endpoint names no narrower type for.
    ScribeError(String),
    /// `auth_error`: the endpoint refused the credential.
    AuthError(String),
    /// `quota_exceeded`: the account's quota is used up.
    QuotaExceeded(String),
    /// `commit_throttled`: commits came faster than the endpoint takes them.
    CommitThrottled(String),
    /// `unaccepted_terms`: the account has not accepted the terms the endpoint needs.
    UnacceptedTerms(String),
    /// `rate_limited`: requests came faster than the account may make them.
    RateLimited(String),
    /// `queue_overflow`: the endpoint's queue for the session overflowed.
    QueueOverflow(String),
    /// `resource_exhausted`: the endpoint has no capacity for the session.
    ResourceExhausted(String),
    /// `session_time_limit_exceeded`: the session ran past its time limit.
    SessionTimeLimitExceeded(String),
    /// `input_error`: the endpoint could not take the audio sent.
    InputError(String),
    /// `invalid_request`: the endpoint refused the request's parameters.
    InvalidRequest(String),
    /// `chunk_size_exceeded`: an audio chunk was larger than the endpoint takes.
    ChunkSizeExceeded(String),
    /// `insufficient_audio_activity`: the endpoint found too little speech in the audio.
    InsufficientAudioActivity(String),
    /// `transcriber_error`: the transcriber itself failed.
    TranscriberError(String),
}

impl ElevenLabsError {
    /// The `message_type` of the realtime speech-to-text error this is, one of the fourteen the
    /// endpoint documents; none for any other failure.
    pub fn scribe_error_type(&self) -> Option<&'static str> {
        Some(match self {
            Self::ScribeError(_) => "error",
            Self::AuthError(_) => "auth_error",
            Self::QuotaExceeded(_) => "quota_exceeded",
            Self::CommitThrottled(_) => "commit_throttled",
            Self::UnacceptedTerms(_) => "unaccepted_terms",
            Self::RateLimited(_) => "rate_limited",
            Self::QueueOverflow(_) => "queue_overflow",
            Self::ResourceExhausted(_) => "resource_exhausted",
            Self::SessionTimeLimitExceeded(_) => "session_time_limit_exceeded",
            Self::InputError(_) => "input_error",
            Self::InvalidRequest(_) => "invalid_request",
            Self::ChunkSizeExceeded(_) => "chunk_size_exceeded",
            Self::InsufficientAudioActivity(_) => "insufficient_audio_activity",
            Self::TranscriberError(_) => "transcriber_error",
            Self::Credential(_)
            | Self::Connect(_)
            | Self::Transport(_)
            | Self::Closed { .. }
            | Self::Stalled(_)
            | Self::Protocol(_)
            | Self::Server(_)
            | Self::Runtime(_)
            | Self::UnsupportedAudio(_) => return None,
        })
    }
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
            Self::Runtime(detail) => write!(f, "the runtime failed: {detail}"),
            Self::UnsupportedAudio(detail) => write!(f, "unsupported audio: {detail}"),
            Self::ScribeError(message)
            | Self::AuthError(message)
            | Self::QuotaExceeded(message)
            | Self::CommitThrottled(message)
            | Self::UnacceptedTerms(message)
            | Self::RateLimited(message)
            | Self::QueueOverflow(message)
            | Self::ResourceExhausted(message)
            | Self::SessionTimeLimitExceeded(message)
            | Self::InputError(message)
            | Self::InvalidRequest(message)
            | Self::ChunkSizeExceeded(message)
            | Self::InsufficientAudioActivity(message)
            | Self::TranscriberError(message) => write!(
                f,
                "speech-to-text {}: {message}",
                self.scribe_error_type().unwrap_or("error")
            ),
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
