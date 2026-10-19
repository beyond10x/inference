//! Where to connect and how to configure the session.

use std::time::Duration;

use serde_json::{Value, json};

/// The current generally available Realtime model the reference names in its WebSocket guide
/// (`https://developers.openai.com/api/docs/guides/realtime-websocket`).
pub const DEFAULT_MODEL: &str = "gpt-realtime-2.1";

/// The environment variable [`Endpoint::from_env`] reads the model id from.
pub const MODEL_VARIABLE: &str = "INFERENCE_OPENAI_REALTIME_MODEL";

/// The Realtime WebSocket URL; the model id goes in its `model` query parameter.
pub const DEFAULT_URL: &str = "wss://api.openai.com/v1/realtime";

/// How long [`RealtimeClient::next_event`](crate::RealtimeClient::next_event) waits for a server
/// event before it reports a stall.
pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Where a session connects: the WebSocket URL, the model id and the read timeout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub url: String,
    pub model: String,
    pub read_timeout: Duration,
}

impl Endpoint {
    /// The hosted endpoint for `model`, with the default read timeout.
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            url: DEFAULT_URL.to_owned(),
            model: model.into(),
            read_timeout: DEFAULT_READ_TIMEOUT,
        }
    }

    /// The hosted endpoint for the model named by [`MODEL_VARIABLE`], or [`DEFAULT_MODEL`] when
    /// it is unset or empty.
    pub fn from_env() -> Self {
        let model = std::env::var(MODEL_VARIABLE)
            .ok()
            .filter(|model| !model.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_MODEL.to_owned());
        Self::new(model)
    }

    /// This endpoint at another URL, such as a local test server.
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// This endpoint with another read timeout.
    pub fn with_read_timeout(mut self, read_timeout: Duration) -> Self {
        self.read_timeout = read_timeout;
        self
    }
}

/// An audio format on the Realtime wire, for input or output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WireFormat {
    /// `audio/pcm`: 16-bit little-endian signed PCM, mono, 24 kHz.
    Pcm16,
    /// `audio/pcmu`: G.711 mu-law, mono, 8 kHz, one byte per sample.
    Pcmu,
}

impl WireFormat {
    /// The sample rate of the format in hertz.
    pub const fn sample_rate(self) -> u32 {
        match self {
            Self::Pcm16 => 24_000,
            Self::Pcmu => 8_000,
        }
    }
}

/// What the model responds with. The API takes one of the two, not both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum OutputModality {
    /// Audio and its transcript.
    #[default]
    Audio,
    /// Text only.
    Text,
}

/// How eager semantic turn detection is to end the user's turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Eagerness {
    Low,
    Medium,
    High,
    #[default]
    Auto,
}

/// Turn detection on the input audio.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TurnDetection {
    /// None: the client commits the input audio and creates each response itself.
    Off,
    /// `server_vad`: turns end on silence. `None` fields keep the server's defaults.
    /// `create_response` creates a response when a turn ends; `interrupt_response` cancels the
    /// running response when speech starts (auto-interrupt).
    Server {
        threshold: Option<f64>,
        prefix_padding_ms: Option<u32>,
        silence_duration_ms: Option<u32>,
        create_response: bool,
        interrupt_response: bool,
    },
    /// `semantic_vad`: a turn detection model estimates whether the user has finished.
    Semantic {
        eagerness: Eagerness,
        create_response: bool,
        interrupt_response: bool,
    },
}

/// A function the model may call. `parameters` is its JSON Schema.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionTool {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// The session configuration sent in `session.update`. `None` fields keep the server's value.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionConfig {
    pub instructions: Option<String>,
    pub voice: Option<String>,
    pub output_modality: OutputModality,
    pub input_format: WireFormat,
    pub output_format: WireFormat,
    pub turn_detection: TurnDetection,
    /// The transcription model for input audio; `None` turns input transcription off.
    pub input_transcription: Option<String>,
    pub tools: Vec<FunctionTool>,
}

impl Default for SessionConfig {
    /// PCM16 at 24 kHz in and out, audio responses, turn detection off, no transcription and no
    /// tools.
    fn default() -> Self {
        Self {
            instructions: None,
            voice: None,
            output_modality: OutputModality::Audio,
            input_format: WireFormat::Pcm16,
            output_format: WireFormat::Pcm16,
            turn_detection: TurnDetection::Off,
            input_transcription: None,
            tools: Vec::new(),
        }
    }
}

impl SessionConfig {
    /// The `session` object of a `session.update` client event, in the reference's session
    /// configuration shape: `type` `realtime`, `output_modalities`, `instructions`, `audio.input`
    /// (`format`, `transcription`, `turn_detection`), `audio.output` (`format`, `voice`) and
    /// `tools`. Turn detection off is an explicit `null`; other unset fields are left out.
    pub fn to_session_json(&self) -> Value {
        let modality = match self.output_modality {
            OutputModality::Audio => "audio",
            OutputModality::Text => "text",
        };
        let mut session = json!({"type": "realtime", "output_modalities": [modality]});
        if let Some(instructions) = &self.instructions {
            session["instructions"] = json!(instructions);
        }
        let mut input = json!({"format": self.input_format.to_json()});
        if let Some(model) = &self.input_transcription {
            input["transcription"] = json!({"model": model});
        }
        input["turn_detection"] = self.turn_detection.to_json();
        let mut output = json!({"format": self.output_format.to_json()});
        if let Some(voice) = &self.voice {
            output["voice"] = json!(voice);
        }
        session["audio"] = json!({"input": input, "output": output});
        if !self.tools.is_empty() {
            session["tools"] = self.tools.iter().map(FunctionTool::to_json).collect();
        }
        session
    }
}

impl WireFormat {
    fn to_json(self) -> Value {
        match self {
            Self::Pcm16 => json!({"type": "audio/pcm", "rate": 24_000}),
            Self::Pcmu => json!({"type": "audio/pcmu"}),
        }
    }
}

impl Eagerness {
    fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Auto => "auto",
        }
    }
}

impl TurnDetection {
    fn to_json(self) -> Value {
        match self {
            Self::Off => Value::Null,
            Self::Server {
                threshold,
                prefix_padding_ms,
                silence_duration_ms,
                create_response,
                interrupt_response,
            } => {
                let mut detection = json!({"type": "server_vad"});
                if let Some(threshold) = threshold {
                    detection["threshold"] = json!(threshold);
                }
                if let Some(padding) = prefix_padding_ms {
                    detection["prefix_padding_ms"] = json!(padding);
                }
                if let Some(silence) = silence_duration_ms {
                    detection["silence_duration_ms"] = json!(silence);
                }
                detection["create_response"] = json!(create_response);
                detection["interrupt_response"] = json!(interrupt_response);
                detection
            }
            Self::Semantic {
                eagerness,
                create_response,
                interrupt_response,
            } => json!({
                "type": "semantic_vad",
                "eagerness": eagerness.as_str(),
                "create_response": create_response,
                "interrupt_response": interrupt_response,
            }),
        }
    }
}

impl FunctionTool {
    fn to_json(&self) -> Value {
        json!({
            "type": "function",
            "name": self.name,
            "description": self.description,
            "parameters": self.parameters,
        })
    }
}
