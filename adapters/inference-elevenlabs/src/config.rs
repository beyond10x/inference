//! Where to connect and what to synthesise with.

use std::fmt;
use std::num::{NonZeroU16, NonZeroU32};
use std::str::FromStr;
use std::time::Duration;

use inference_tts::inference_audio::{AudioEncoding, AudioFormat};

use crate::error::ConfigError;

/// The model the binding asks for unless told otherwise: the one the latency guide recommends
/// for low latency. The endpoint's own default is another, slower one, so the binding always
/// names a model.
pub const DEFAULT_MODEL: &str = "eleven_flash_v2_5";

/// The hosted API's base URL; the binding adds `/v1/text-to-speech/{voice_id}/stream-input`.
pub const DEFAULT_URL: &str = "wss://api.elevenlabs.io";

/// How long a connection may go without a server message before the synthesis fails as
/// stalled. Every read waits at most this long from the last message read on the connection
/// (the end of the handshake before the first), and every write at most this long.
pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the TCP connection, TLS and the WebSocket handshake may take together.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The largest `inactivity_timeout` the endpoint takes, in seconds.
pub const MAX_INACTIVITY_TIMEOUT: u32 = 180;

/// Where a synthesis connects: the base URL and the bounds on the handshake and on every read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// Scheme and host (and port), no path, query or trailing slash needed: `wss://` to any host,
    /// `ws://` only to a loopback host, since the key would travel in the clear.
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

    /// This endpoint at another base URL, such as a regional host or a local test server.
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

/// An `output_format` the binding takes, each an uncompressed format the TTS contract carries
/// as it arrives. MP3, Opus and WAV are not among them: [`OutputFormat::from_str`] refuses them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutputFormat {
    /// `ulaw_8000`: G.711 μ-law, 8 kHz, one byte per sample.
    Ulaw8000,
    /// `alaw_8000`: G.711 A-law, 8 kHz, one byte per sample.
    Alaw8000,
    /// `pcm_16000`: PCM, 16 kHz, taken as signed 16-bit little-endian mono.
    Pcm16000,
    /// `pcm_22050`: PCM, 22.05 kHz, taken as signed 16-bit little-endian mono.
    Pcm22050,
    /// `pcm_24000`: PCM, 24 kHz, taken as signed 16-bit little-endian mono.
    Pcm24000,
    /// `pcm_44100`: PCM, 44.1 kHz, taken as signed 16-bit little-endian mono.
    Pcm44100,
}

impl OutputFormat {
    /// Every format the binding takes.
    pub const ALL: [Self; 6] = [
        Self::Ulaw8000,
        Self::Alaw8000,
        Self::Pcm16000,
        Self::Pcm22050,
        Self::Pcm24000,
        Self::Pcm44100,
    ];

    /// The name the endpoint takes in its `output_format` query parameter.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ulaw8000 => "ulaw_8000",
            Self::Alaw8000 => "alaw_8000",
            Self::Pcm16000 => "pcm_16000",
            Self::Pcm22050 => "pcm_22050",
            Self::Pcm24000 => "pcm_24000",
            Self::Pcm44100 => "pcm_44100",
        }
    }

    /// The contract format of the frames the binding emits in this output format: mono, at the
    /// format's rate, μ-law, A-law or signed 16-bit little-endian PCM.
    pub const fn audio_format(self) -> AudioFormat {
        let (encoding, rate) = match self {
            Self::Ulaw8000 => (AudioEncoding::PcmMulaw, 8_000),
            Self::Alaw8000 => (AudioEncoding::PcmAlaw, 8_000),
            Self::Pcm16000 => (AudioEncoding::PcmS16le, 16_000),
            Self::Pcm22050 => (AudioEncoding::PcmS16le, 22_050),
            Self::Pcm24000 => (AudioEncoding::PcmS16le, 24_000),
            Self::Pcm44100 => (AudioEncoding::PcmS16le, 44_100),
        };
        AudioFormat {
            encoding,
            sample_rate: match NonZeroU32::new(rate) {
                Some(rate) => rate,
                None => NonZeroU32::MIN,
            },
            channels: NonZeroU16::MIN,
        }
    }
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for OutputFormat {
    type Err = ConfigError;

    /// The format of an `output_format` name.
    ///
    /// # Errors
    /// [`ConfigError::UnsupportedOutputFormat`] for any other name: MP3 and Opus, which are
    /// compressed, WAV, which is a container, and PCM rates the binding does not map.
    fn from_str(name: &str) -> Result<Self, ConfigError> {
        if let Some(format) = Self::ALL.into_iter().find(|format| format.as_str() == name) {
            return Ok(format);
        }
        let reason = if name.starts_with("mp3_") || name.starts_with("opus_") {
            "a compressed format, which the TTS contract does not carry"
        } else if name.starts_with("wav_") {
            "a container format; the binding takes raw samples"
        } else {
            "not an output format the binding maps"
        };
        Err(ConfigError::UnsupportedOutputFormat {
            name: name.to_owned(),
            reason,
        })
    }
}

/// The `voice_settings` of the first message. `None` fields are left out, so the voice's own
/// settings apply.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct VoiceSettings {
    pub stability: Option<f64>,
    pub similarity_boost: Option<f64>,
    pub style: Option<f64>,
    pub use_speaker_boost: Option<bool>,
    /// 0.7 to 1.2.
    pub speed: Option<f64>,
}

/// What a synthesis asks the endpoint for.
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceConfig {
    /// The voice, a path segment of the URL: ASCII letters, digits, `-` and `_` only.
    pub voice_id: String,
    /// The model, [`DEFAULT_MODEL`] from [`VoiceConfig::new`]: ASCII letters, digits, `-`, `_`
    /// and `.` only.
    pub model_id: String,
    pub output_format: OutputFormat,
    pub voice_settings: Option<VoiceSettings>,
    /// The `auto_mode` query parameter; `None` leaves it out.
    pub auto_mode: Option<bool>,
    /// The `inactivity_timeout` query parameter in seconds, 1 to [`MAX_INACTIVITY_TIMEOUT`];
    /// `None` leaves it out.
    pub inactivity_timeout: Option<u32>,
}

impl VoiceConfig {
    /// `voice_id` in `output_format` with [`DEFAULT_MODEL`] and nothing else set.
    pub fn new(voice_id: impl Into<String>, output_format: OutputFormat) -> Self {
        Self {
            voice_id: voice_id.into(),
            model_id: DEFAULT_MODEL.to_owned(),
            output_format,
            voice_settings: None,
            auto_mode: None,
            inactivity_timeout: None,
        }
    }

    /// This configuration with another model.
    pub fn with_model(mut self, model_id: impl Into<String>) -> Self {
        self.model_id = model_id.into();
        self
    }

    /// Checks what the URL and the endpoint's documented ranges need.
    ///
    /// # Errors
    /// [`ConfigError::InvalidVoiceId`], [`ConfigError::InvalidModelId`],
    /// [`ConfigError::InactivityTimeout`] or [`ConfigError::Speed`].
    pub fn validate(&self) -> Result<(), ConfigError> {
        let token = |value: &str, extra: &[char]| {
            !value.is_empty()
                && value.len() <= 128
                && value.chars().all(|c| {
                    c.is_ascii_alphanumeric() || c == '-' || c == '_' || extra.contains(&c)
                })
        };
        if !token(&self.voice_id, &[]) {
            return Err(ConfigError::InvalidVoiceId);
        }
        if !token(&self.model_id, &['.']) {
            return Err(ConfigError::InvalidModelId);
        }
        if let Some(seconds) = self.inactivity_timeout
            && !(1..=MAX_INACTIVITY_TIMEOUT).contains(&seconds)
        {
            return Err(ConfigError::InactivityTimeout(seconds));
        }
        if let Some(speed) = self.voice_settings.and_then(|settings| settings.speed)
            && !(0.7..=1.2).contains(&speed)
        {
            return Err(ConfigError::Speed(speed));
        }
        Ok(())
    }
}
