//! What a recognition asks the realtime speech-to-text endpoint for: the input format, the model,
//! the language, the key terms and the commit strategy.

use std::fmt;
use std::num::{NonZeroU16, NonZeroU32};
use std::str::FromStr;

use inference_stt::inference_audio::{AudioEncoding, AudioFormat};

use crate::error::ConfigError;

/// The model the binding asks for unless told otherwise, the endpoint's own default.
pub const DEFAULT_STT_MODEL: &str = "scribe_v2_realtime";

/// An `audio_format` the binding sends, each one the realtime endpoint documents: G.711 μ-law at
/// 8 kHz, or PCM at one of six rates. A-law, other rates and compressed formats are not among
/// them: [`InputFormat::from_str`] refuses them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputFormat {
    /// `ulaw_8000`: G.711 μ-law, 8 kHz, one byte per sample.
    Ulaw8000,
    /// `pcm_8000`: PCM, 8 kHz, taken as signed 16-bit little-endian mono.
    Pcm8000,
    /// `pcm_16000`: PCM, 16 kHz, taken as signed 16-bit little-endian mono.
    Pcm16000,
    /// `pcm_22050`: PCM, 22.05 kHz, taken as signed 16-bit little-endian mono.
    Pcm22050,
    /// `pcm_24000`: PCM, 24 kHz, taken as signed 16-bit little-endian mono.
    Pcm24000,
    /// `pcm_44100`: PCM, 44.1 kHz, taken as signed 16-bit little-endian mono.
    Pcm44100,
    /// `pcm_48000`: PCM, 48 kHz, taken as signed 16-bit little-endian mono.
    Pcm48000,
}

impl InputFormat {
    /// Every format the binding sends.
    pub const ALL: [Self; 7] = [
        Self::Ulaw8000,
        Self::Pcm8000,
        Self::Pcm16000,
        Self::Pcm22050,
        Self::Pcm24000,
        Self::Pcm44100,
        Self::Pcm48000,
    ];

    /// The name the endpoint takes in its `audio_format` query parameter.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ulaw8000 => "ulaw_8000",
            Self::Pcm8000 => "pcm_8000",
            Self::Pcm16000 => "pcm_16000",
            Self::Pcm22050 => "pcm_22050",
            Self::Pcm24000 => "pcm_24000",
            Self::Pcm44100 => "pcm_44100",
            Self::Pcm48000 => "pcm_48000",
        }
    }

    /// The sample rate in hertz.
    pub const fn sample_rate(self) -> u32 {
        match self {
            Self::Ulaw8000 | Self::Pcm8000 => 8_000,
            Self::Pcm16000 => 16_000,
            Self::Pcm22050 => 22_050,
            Self::Pcm24000 => 24_000,
            Self::Pcm44100 => 44_100,
            Self::Pcm48000 => 48_000,
        }
    }

    /// The contract format every frame of a recognition in this input format must be in: mono,
    /// at the format's rate, μ-law or signed 16-bit little-endian PCM.
    pub const fn audio_format(self) -> AudioFormat {
        let encoding = match self {
            Self::Ulaw8000 => AudioEncoding::PcmMulaw,
            _ => AudioEncoding::PcmS16le,
        };
        AudioFormat {
            encoding,
            sample_rate: match NonZeroU32::new(self.sample_rate()) {
                Some(rate) => rate,
                None => NonZeroU32::MIN,
            },
            channels: NonZeroU16::MIN,
        }
    }
}

impl fmt::Display for InputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for InputFormat {
    type Err = ConfigError;

    /// The format of an `audio_format` name.
    ///
    /// # Errors
    /// [`ConfigError::UnsupportedInputFormat`] for any other name: A-law, PCM at a rate the
    /// endpoint does not document, and compressed or container formats.
    fn from_str(name: &str) -> Result<Self, ConfigError> {
        if let Some(format) = Self::ALL.into_iter().find(|format| format.as_str() == name) {
            return Ok(format);
        }
        let reason = if name.starts_with("alaw_") {
            "the realtime endpoint documents μ-law and PCM input, not A-law"
        } else if name.starts_with("pcm_") {
            "a PCM rate the realtime endpoint does not document"
        } else {
            "not an input format the realtime endpoint documents"
        };
        Err(ConfigError::UnsupportedInputFormat {
            name: name.to_owned(),
            reason,
        })
    }
}

/// The voice activity detection settings of the `vad` commit strategy. `None` fields are left
/// out, so the endpoint's own defaults apply.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct VadSettings {
    /// `vad_threshold`.
    pub threshold: Option<f64>,
    /// `vad_silence_threshold_secs`.
    pub silence_threshold_secs: Option<f64>,
    /// `min_speech_duration_ms`.
    pub min_speech_duration_ms: Option<u32>,
    /// `min_silence_duration_ms`.
    pub min_silence_duration_ms: Option<u32>,
}

/// How the endpoint cuts the audio into committed transcripts.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum CommitStrategy {
    /// `manual`, the endpoint's default: only a commit the binding sends commits, at the end of
    /// input and on [`ElevenLabsRecognition::commit`](crate::ElevenLabsRecognition::commit).
    #[default]
    Manual,
    /// `vad`: the endpoint commits on its own after silence, with these settings; the end of
    /// input still commits what is left.
    Vad(VadSettings),
}

impl CommitStrategy {
    /// The name the endpoint takes in its `commit_strategy` query parameter.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Vad(_) => "vad",
        }
    }
}

/// What a recognition asks the realtime speech-to-text endpoint for.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptionConfig {
    /// The model, [`DEFAULT_STT_MODEL`] from [`TranscriptionConfig::new`]: ASCII letters,
    /// digits, `-`, `_` and `.` only.
    pub model_id: String,
    /// The format every frame must be in.
    pub input_format: InputFormat,
    /// The `language_code`, ISO 639-1 or ISO 639-3: two or three ASCII letters. `None` leaves it
    /// out, and the endpoint detects the language.
    pub language_code: Option<String>,
    /// The `keyterms` the model is biased towards, each sent as a query parameter of its own;
    /// none may be blank.
    pub keyterms: Vec<String>,
    pub commit_strategy: CommitStrategy,
}

impl TranscriptionConfig {
    /// `input_format` with [`DEFAULT_STT_MODEL`], manual commit and nothing else set.
    pub fn new(input_format: InputFormat) -> Self {
        Self {
            model_id: DEFAULT_STT_MODEL.to_owned(),
            input_format,
            language_code: None,
            keyterms: Vec::new(),
            commit_strategy: CommitStrategy::Manual,
        }
    }

    /// This configuration with another model.
    pub fn with_model(mut self, model_id: impl Into<String>) -> Self {
        self.model_id = model_id.into();
        self
    }

    /// This configuration with a language.
    pub fn with_language(mut self, language_code: impl Into<String>) -> Self {
        self.language_code = Some(language_code.into());
        self
    }

    /// This configuration with another commit strategy.
    pub fn with_commit_strategy(mut self, commit_strategy: CommitStrategy) -> Self {
        self.commit_strategy = commit_strategy;
        self
    }

    /// Checks what the URL needs and what the endpoint's documented types admit.
    ///
    /// # Errors
    /// [`ConfigError::InvalidModelId`], [`ConfigError::InvalidLanguageCode`],
    /// [`ConfigError::InvalidKeyterm`] or [`ConfigError::VadSetting`] for a VAD threshold that is
    /// not a finite number of at least zero.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let model = &self.model_id;
        if model.is_empty()
            || model.len() > 128
            || !model
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(ConfigError::InvalidModelId);
        }
        if let Some(language) = &self.language_code
            && !((2..=3).contains(&language.len())
                && language.chars().all(|c| c.is_ascii_alphabetic()))
        {
            return Err(ConfigError::InvalidLanguageCode);
        }
        if self.keyterms.iter().any(|term| term.trim().is_empty()) {
            return Err(ConfigError::InvalidKeyterm);
        }
        if let CommitStrategy::Vad(settings) = &self.commit_strategy {
            for (name, value) in [
                ("vad_threshold", settings.threshold),
                (
                    "vad_silence_threshold_secs",
                    settings.silence_threshold_secs,
                ),
            ] {
                if let Some(value) = value
                    && !(value.is_finite() && value >= 0.0)
                {
                    return Err(ConfigError::VadSetting { name, value });
                }
            }
        }
        Ok(())
    }
}
