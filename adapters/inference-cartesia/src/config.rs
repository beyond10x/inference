//! Where to connect and what to synthesise with.

use std::fmt;
use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use inference_tts::inference_audio::{AudioEncoding, AudioFormat};

use crate::error::ConfigError;

/// The model the binding asks for unless told otherwise: Sonic 3.6, which follows the most
/// recent stable snapshot.
pub const DEFAULT_MODEL: &str = "sonic-3.6";

/// The hosted API's base URL; the binding adds `/tts/websocket`.
pub const DEFAULT_URL: &str = "wss://api.cartesia.ai";

/// The API version the binding was written against, sent as the `cartesia_version` query
/// parameter of every connection.
pub const API_VERSION: &str = "2026-08-14";

/// How long a connection may go without a server message while a segment is being spoken before
/// the synthesis fails as stalled. Every read waits at most this long from the last message read
/// or request sent, and every write at most this long.
pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the TCP connection, TLS and the WebSocket handshake may take together.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The sample rates the endpoint takes, in Hz.
pub const SAMPLE_RATES: [u32; 6] = [8_000, 16_000, 22_050, 24_000, 44_100, 48_000];

/// Where a synthesis connects: the base URL and the bounds on the handshake and on every read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// Scheme and host (and port), no path or query: `wss://` to any host, `ws://` only to a
    /// loopback host, since the key would travel in the clear.
    pub url: String,
    pub read_timeout: Duration,
    pub connect_timeout: Duration,
}

impl Default for Endpoint {
    fn default() -> Self {
        Self::new()
    }
}

impl Endpoint {
    /// The hosted endpoint, with the default bounds.
    pub fn new() -> Self {
        Self {
            url: DEFAULT_URL.to_owned(),
            read_timeout: DEFAULT_READ_TIMEOUT,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
        }
    }

    /// This endpoint at another base URL, such as a local test server.
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// This endpoint with another read timeout.
    pub fn with_read_timeout(mut self, read_timeout: Duration) -> Self {
        self.read_timeout = read_timeout;
        self
    }

    /// This endpoint with another connect timeout.
    pub fn with_connect_timeout(mut self, connect_timeout: Duration) -> Self {
        self.connect_timeout = connect_timeout;
        self
    }
}

/// The raw `output_format` of every frame: an encoding of the contract, which are exactly the
/// four the endpoint takes, at one of [`SAMPLE_RATES`], taken as mono.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OutputFormat {
    encoding: AudioEncoding,
    sample_rate: u32,
}

impl OutputFormat {
    /// G.711 μ-law at 8 kHz, the telephony format.
    pub const MULAW_8000: Self = Self {
        encoding: AudioEncoding::PcmMulaw,
        sample_rate: 8_000,
    };

    /// G.711 A-law at 8 kHz, the telephony format outside North America and Japan.
    pub const ALAW_8000: Self = Self {
        encoding: AudioEncoding::PcmAlaw,
        sample_rate: 8_000,
    };

    /// Signed 16-bit little-endian PCM at 16 kHz.
    pub const PCM_S16LE_16000: Self = Self {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: 16_000,
    };

    /// Signed 16-bit little-endian PCM at 24 kHz.
    pub const PCM_S16LE_24000: Self = Self {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: 24_000,
    };

    /// `encoding` at `sample_rate` Hz.
    ///
    /// # Errors
    /// [`ConfigError::UnsupportedSampleRate`] for a rate not among [`SAMPLE_RATES`].
    pub fn new(encoding: AudioEncoding, sample_rate: u32) -> Result<Self, ConfigError> {
        if !SAMPLE_RATES.contains(&sample_rate) {
            return Err(ConfigError::UnsupportedSampleRate(sample_rate));
        }
        Ok(Self {
            encoding,
            sample_rate,
        })
    }

    /// The encoding.
    pub const fn encoding(self) -> AudioEncoding {
        self.encoding
    }

    /// The sample rate in Hz.
    pub const fn sample_rate(self) -> u32 {
        self.sample_rate
    }

    /// The name the endpoint takes in `output_format.encoding`.
    pub const fn encoding_name(self) -> &'static str {
        match self.encoding {
            AudioEncoding::PcmS16le => "pcm_s16le",
            AudioEncoding::PcmF32le => "pcm_f32le",
            AudioEncoding::PcmMulaw => "pcm_mulaw",
            AudioEncoding::PcmAlaw => "pcm_alaw",
        }
    }

    /// The contract format of the frames the binding emits: mono, at the rate, in the encoding.
    pub const fn audio_format(self) -> AudioFormat {
        AudioFormat {
            encoding: self.encoding,
            sample_rate: match NonZeroU32::new(self.sample_rate) {
                Some(rate) => rate,
                None => NonZeroU32::MIN,
            },
            channels: NonZeroU16::MIN,
        }
    }
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.encoding_name(), self.sample_rate)
    }
}

/// What a synthesis asks the endpoint for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceConfig {
    /// The voice: 1 to 128 ASCII letters, digits, `-` and `_`.
    pub voice_id: String,
    /// The model, [`DEFAULT_MODEL`] from [`VoiceConfig::new`]: 1 to 128 ASCII letters, digits,
    /// `-`, `_` and `.`.
    pub model_id: String,
    pub output_format: OutputFormat,
    /// The `language` of every request, a base code such as `en` or a locale such as `en-GB`;
    /// `None` leaves it out.
    pub language: Option<String>,
}

impl VoiceConfig {
    /// `voice_id` in `output_format` with [`DEFAULT_MODEL`] and no language.
    pub fn new(voice_id: impl Into<String>, output_format: OutputFormat) -> Self {
        Self {
            voice_id: voice_id.into(),
            model_id: DEFAULT_MODEL.to_owned(),
            output_format,
            language: None,
        }
    }

    /// This configuration with another model.
    pub fn with_model(mut self, model_id: impl Into<String>) -> Self {
        self.model_id = model_id.into();
        self
    }

    /// This configuration with a language.
    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = Some(language.into());
        self
    }

    /// Checks the identifiers the requests carry.
    ///
    /// # Errors
    /// [`ConfigError::InvalidVoiceId`], [`ConfigError::InvalidModelId`] or
    /// [`ConfigError::InvalidLanguage`].
    pub fn validate(&self) -> Result<(), ConfigError> {
        let token = |value: &str, limit: usize, allowed: fn(char) -> bool| {
            !value.is_empty() && value.len() <= limit && value.chars().all(allowed)
        };
        if !token(&self.voice_id, 128, |c| {
            c.is_ascii_alphanumeric() || c == '-' || c == '_'
        }) {
            return Err(ConfigError::InvalidVoiceId);
        }
        if !token(&self.model_id, 128, |c| {
            c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'
        }) {
            return Err(ConfigError::InvalidModelId);
        }
        if let Some(language) = &self.language
            && !token(language, 16, |c| {
                c.is_ascii_alphabetic() || c == '-' || c == '_'
            })
        {
            return Err(ConfigError::InvalidLanguage);
        }
        Ok(())
    }
}
