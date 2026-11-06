//! What the speech binding speaks with and where it sends it, and why either is refused.

use std::fmt;
use std::time::Duration;

use inference_tts::inference_audio::{AudioEncoding, AudioFormat, RESAMPLER_RATES};

use super::protocol::MAX_INPUT_CHARACTERS;
use crate::config::{DEFAULT_CONNECT_TIMEOUT, DEFAULT_READ_TIMEOUT};
use crate::credential::CredentialError;

/// The model a [`SpeechConfig`] names unless another is set.
pub const DEFAULT_MODEL: &str = "gpt-4o-mini-tts";

/// The hosted speech endpoint.
pub const DEFAULT_URL: &str = "https://api.openai.com/v1/audio/speech";

/// The slowest speed the endpoint takes.
pub const MIN_SPEED: f64 = 0.25;

/// The fastest speed the endpoint takes.
pub const MAX_SPEED: f64 = 4.0;

/// Where the binding sends its requests: the URL, the bound on making a connection and the bound
/// on each wait for the server, for the response's head and for every chunk of its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechEndpoint {
    pub url: String,
    pub connect_timeout: Duration,
    pub read_timeout: Duration,
}

impl Default for SpeechEndpoint {
    fn default() -> Self {
        Self::new()
    }
}

impl SpeechEndpoint {
    /// The hosted endpoint, [`DEFAULT_URL`], connecting within 10 seconds and waiting at most 30
    /// seconds for each read.
    pub fn new() -> Self {
        Self {
            url: DEFAULT_URL.to_owned(),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            read_timeout: DEFAULT_READ_TIMEOUT,
        }
    }

    /// This endpoint at another URL, such as a local test server: `https://`, or `http://` only
    /// to a loopback host, checked when a synthesiser is made.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// This endpoint with another bound on making a connection.
    #[must_use]
    pub fn with_connect_timeout(mut self, connect_timeout: Duration) -> Self {
        self.connect_timeout = connect_timeout;
        self
    }

    /// This endpoint with another bound on each wait for the server.
    #[must_use]
    pub fn with_read_timeout(mut self, read_timeout: Duration) -> Self {
        self.read_timeout = read_timeout;
        self
    }
}

/// The voice that speaks: a built-in voice by name (`alloy`, `marin`, `cedar`, …) or a custom
/// voice by its id (`voice_…`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpeechVoice {
    /// A built-in voice, sent as its name.
    BuiltIn(String),
    /// A custom voice, sent as `{ "id": … }`.
    Custom(String),
}

/// What a synthesis speaks with: the model, the voice, the format of the frames it emits and,
/// when set, instructions and a speed. The text of every segment is sent as given.
///
/// The endpoint streams 24 kHz 16-bit PCM. A frame of another rate is resampled by
/// `inference-audio`'s band-limited [`Resampler`](inference_tts::inference_audio::Resampler), so
/// the rate is one of its rates, 8, 16, 24 or 48 kHz; a μ-law or A-law frame is encoded by
/// `inference-audio`'s G.711 codec. Frames are mono.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeechConfig {
    pub model: String,
    pub voice: SpeechVoice,
    pub output: AudioFormat,
    pub instructions: Option<String>,
    pub speed: Option<f64>,
}

impl SpeechConfig {
    /// `voice` speaking [`DEFAULT_MODEL`] into frames of `output`, with no instructions and the
    /// endpoint's own speed.
    pub fn new(voice: SpeechVoice, output: AudioFormat) -> Self {
        Self {
            model: DEFAULT_MODEL.to_owned(),
            voice,
            output,
            instructions: None,
            speed: None,
        }
    }

    /// This configuration with another model.
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// This configuration with instructions for the voice, which `tts-1` and `tts-1-hd` ignore.
    #[must_use]
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    /// This configuration with a speed, from [`MIN_SPEED`] to [`MAX_SPEED`].
    #[must_use]
    pub fn with_speed(mut self, speed: f64) -> Self {
        self.speed = Some(speed);
        self
    }

    /// Whether the endpoint can take this configuration.
    ///
    /// # Errors
    /// The first [`SpeechConfigError`] it meets: a blank model or voice, instructions longer than
    /// 4096 characters, a speed outside 0.25 to 4.0, or an output format that is not mono
    /// `pcm_s16le`, `pcm_mulaw` or `pcm_alaw` at 8, 16, 24 or 48 kHz.
    pub fn validate(&self) -> Result<(), SpeechConfigError> {
        if self.model.trim().is_empty() {
            return Err(SpeechConfigError::Model);
        }
        let voice = match &self.voice {
            SpeechVoice::BuiltIn(name) | SpeechVoice::Custom(name) => name,
        };
        if voice.trim().is_empty() {
            return Err(SpeechConfigError::Voice);
        }
        if let Some(instructions) = &self.instructions {
            let characters = instructions.chars().count();
            if characters > MAX_INPUT_CHARACTERS {
                return Err(SpeechConfigError::Instructions { characters });
            }
        }
        if let Some(speed) = self.speed
            && !(MIN_SPEED..=MAX_SPEED).contains(&speed)
        {
            return Err(SpeechConfigError::Speed(speed));
        }
        let output = self.output;
        let encoded = matches!(
            output.encoding,
            AudioEncoding::PcmS16le | AudioEncoding::PcmMulaw | AudioEncoding::PcmAlaw
        );
        if !encoded
            || output.channels.get() != 1
            || !RESAMPLER_RATES.contains(&output.sample_rate.get())
        {
            return Err(SpeechConfigError::Output(output));
        }
        Ok(())
    }
}

/// Why a speech synthesiser cannot be made. No variant carries a credential.
#[derive(Debug, Clone, PartialEq)]
pub enum SpeechConfigError {
    /// The model is blank.
    Model,
    /// The voice is blank.
    Voice,
    /// The instructions are longer than the endpoint takes; this many characters.
    Instructions { characters: usize },
    /// The speed is outside 0.25 to 4.0, or not a number.
    Speed(f64),
    /// No frame of this format can be made from the endpoint's PCM.
    Output(AudioFormat),
    /// The URL is not `https://`, or `http://` to a loopback host.
    InvalidEndpoint(String),
    /// The adapter record's maximum concurrency is not positive.
    MaxConcurrency(i64),
}

impl fmt::Display for SpeechConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Model => f.write_str("the model is blank"),
            Self::Voice => f.write_str("the voice is blank"),
            Self::Instructions { characters } => write!(
                f,
                "the instructions are {characters} characters long; the endpoint takes at most \
                 {MAX_INPUT_CHARACTERS}"
            ),
            Self::Speed(speed) => {
                write!(f, "the speed {speed} is outside {MIN_SPEED} to {MAX_SPEED}")
            }
            Self::Output(format) => write!(
                f,
                "no frame of {format:?} is made: the output is mono pcm_s16le, pcm_mulaw or \
                 pcm_alaw at 8, 16, 24 or 48 kHz"
            ),
            Self::InvalidEndpoint(detail) => write!(f, "invalid endpoint: {detail}"),
            Self::MaxConcurrency(value) => {
                write!(f, "the maximum concurrency {value} is not positive")
            }
        }
    }
}

impl std::error::Error for SpeechConfigError {}

/// Why a speech synthesis failed. No variant carries the credential or any part of it.
#[derive(Debug)]
pub enum SpeechError {
    /// The injected source refused the credential, or it cannot be sent as a bearer token.
    Credential(CredentialError),
    /// The segment's text is longer than one request takes: `characters` of at most `limit`.
    /// Nothing of it was sent.
    InputTooLong { characters: usize, limit: usize },
    /// The request could not be made or sent: no connection within the connect timeout, or a
    /// refused one.
    Connect(String),
    /// The server answered with a status other than success.
    Status(u16),
    /// Reading the response failed, or it ended before its end.
    Transport(String),
    /// No response head, or no chunk of its body, within the read timeout, which this carries.
    Stalled(Duration),
    /// The response is not what the endpoint documents: a body that ends on half a sample.
    Protocol(String),
    /// The runtime or the HTTP client that drives the requests could not be made.
    Runtime(String),
}

impl fmt::Display for SpeechError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Credential(error) => write!(f, "credential refused: {error}"),
            Self::InputTooLong { characters, limit } => write!(
                f,
                "the segment is {characters} characters long; one request takes at most {limit}"
            ),
            Self::Connect(detail) => write!(f, "the request could not be sent: {detail}"),
            Self::Status(status) => write!(f, "the server answered HTTP {status}"),
            Self::Transport(detail) => write!(f, "reading the response failed: {detail}"),
            Self::Stalled(waited) => write!(f, "nothing from the server within {waited:?}"),
            Self::Protocol(detail) => write!(f, "protocol violation: {detail}"),
            Self::Runtime(detail) => write!(f, "the runtime failed: {detail}"),
        }
    }
}

impl std::error::Error for SpeechError {}
