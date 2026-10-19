//! The configuration of one Nova Sonic session.

use std::time::Duration;

/// A sample rate Nova Sonic takes and produces. The input events reference lists 8000, 16000 and
/// 24000 Hz for both `audioInputConfiguration` and `audioOutputConfiguration`, always 16-bit mono
/// linear PCM: 8 kHz telephony audio is accepted without resampling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioRate {
    /// 8000 Hz, telephony.
    Hz8000,
    /// 16000 Hz, the reference's input rate.
    Hz16000,
    /// 24000 Hz, the reference's output rate.
    Hz24000,
}

impl AudioRate {
    /// The rate in hertz, the `sampleRateHertz` of the wire.
    pub fn hertz(self) -> u32 {
        match self {
            AudioRate::Hz8000 => 8000,
            AudioRate::Hz16000 => 16000,
            AudioRate::Hz24000 => 24000,
        }
    }
}

impl TryFrom<u32> for AudioRate {
    type Error = u32;

    /// The rate for `hertz`, or `hertz` back when Nova Sonic does not take it.
    fn try_from(hertz: u32) -> Result<Self, Self::Error> {
        match hertz {
            8000 => Ok(AudioRate::Hz8000),
            16000 => Ok(AudioRate::Hz16000),
            24000 => Ok(AudioRate::Hz24000),
            other => Err(other),
        }
    }
}

/// How quickly the model decides the user finished speaking, the `endpointingSensitivity` of the
/// session start's `turnDetectionConfiguration`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpointing {
    /// `HIGH`: detects pauses quickly.
    High,
    /// `MEDIUM`: the reference's recommended default.
    Medium,
    /// `LOW`: waits longer before deciding.
    Low,
}

impl Endpointing {
    /// The wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Endpointing::High => "HIGH",
            Endpointing::Medium => "MEDIUM",
            Endpointing::Low => "LOW",
        }
    }
}

/// The `inferenceConfiguration` of the session start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InferenceConfiguration {
    /// `maxTokens`: the most tokens one response generates.
    pub max_tokens: u32,
    /// `topP`: nucleus sampling, 0.0 to 1.0.
    pub top_p: f64,
    /// `temperature`: randomness, 0.0 to 1.0.
    pub temperature: f64,
}

impl Default for InferenceConfiguration {
    /// The values of the reference's getting-started example.
    fn default() -> Self {
        InferenceConfiguration {
            max_tokens: 1024,
            top_p: 0.9,
            temperature: 0.7,
        }
    }
}

/// One tool the model may call, a `toolSpec` of the prompt start's `toolConfiguration`.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    /// The tool's name, as the model's `toolUse` repeats it.
    pub name: String,
    /// What the tool does, for the model.
    pub description: String,
    /// The JSON schema of the tool's input; sent as a JSON string, as the reference asks.
    pub input_schema: serde_json::Value,
}

/// Everything one session sends before audio, and how long a read may wait.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionConfig {
    /// The session start's inference configuration.
    pub inference: InferenceConfiguration,
    /// The session start's turn detection; `None` leaves it to the service.
    pub endpointing: Option<Endpointing>,
    /// The prompt start's `voiceId`, one of the voices of the reference's language support page.
    pub voice_id: String,
    /// The rate of the audio the model speaks.
    pub output_rate: AudioRate,
    /// The rate of the audio the caller sends.
    pub input_rate: AudioRate,
    /// The system prompt, sent as the first text content.
    pub system_prompt: String,
    /// The tools the model may call.
    pub tools: Vec<ToolSpec>,
    /// How long the session may be quiet, no frame sent and none received, before a read reports
    /// a stall.
    pub read_timeout: Duration,
}

impl SessionConfig {
    /// A configuration with `system_prompt` and the reference's defaults: the getting-started
    /// inference values, voice `matthew`, 16 kHz input, 24 kHz output, no tools, turn detection
    /// left to the service, and a 30 second read timeout.
    pub fn new(system_prompt: impl Into<String>) -> Self {
        SessionConfig {
            inference: InferenceConfiguration::default(),
            endpointing: None,
            voice_id: "matthew".to_owned(),
            output_rate: AudioRate::Hz24000,
            input_rate: AudioRate::Hz16000,
            system_prompt: system_prompt.into(),
            tools: Vec::new(),
            read_timeout: Duration::from_secs(30),
        }
    }
}
