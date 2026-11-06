//! The Opal WebSocket protocol, as far as the binding speaks it.
//!
//! Taken on 2026-11-06 from Deepslate's public documentation: the WebSocket guide
//! (<https://docs.deepslate.eu/websocket.md>), the message reference
//! (<https://docs.deepslate.eu/api-reference/realtime.md>) and the protocol buffers definition it
//! links, `realtime.proto` of package `eu.deepslate.realtime.speeq`, whose URL, commit and hash
//! `protocol/README.md` beside this crate names. Every message is one binary WebSocket frame
//! holding one protocol buffers message: a `ServiceBoundMessage` from the client, a
//! `ClientBoundMessage` from the server, each a `oneof` of the messages below. The field numbers
//! here are the definition's.
//!
//! The client sends [`ServiceBound`]: `InitializeSessionRequest` first, `UpdateToolDefinitions
//! Request`, `UserInput` with audio, `ToolCallResponse` and `Ping`. The other client messages of
//! the definition (`ReconfigureSessionRequest`, `TriggerInference`, `ExportChatHistoryRequest`,
//! `PlaybackPositionReport`, `DirectSpeech`, `ConversationQuery`) are not sent, and are read back
//! only as [`ServiceBound::Other`]. The server sends [`ClientBound`]; the messages the binding
//! passes over (`ChatHistory`, `ConversationQueryResult`, `VadAnalysisFrame`, `ContextTruncated`,
//! `ModelSpeechProgress`, `TurnSnapshot`) are read as [`ClientBound::Other`] without their
//! contents.
//!
//! `google.protobuf.Struct`, the type of a tool's parameters, is converted to and from JSON as
//! the well-known type defines it: an object of `Value`s, each null, a number, a string, a bool,
//! an object or a list. A `Value` holds every number as a `double`; one read back that is whole
//! and within ±2^53 becomes a JSON integer, any other a JSON float, and one that is not finite,
//! which JSON cannot hold, is refused.
//!
//! What the documentation leaves open, and the binding therefore does not rely on, is listed in
//! `protocol/README.md`.

use std::fmt;
use std::time::Duration;

use serde_json::{Map, Number, Value as Json};

use crate::wire::{self, WireError, Writer};

pub use crate::wire::WireError as ProtocolError;

/// How deep a `google.protobuf.Struct` may nest before it is refused.
pub const STRUCT_DEPTH: usize = 32;

/// `SampleFormat.SIGNED_16_BIT`, the only sample format the binding sends: the definition says
/// the service assumes 16-bit PCM, and the guide that its samples are little-endian.
pub const SIGNED_16_BIT: u64 = 1;

/// `AudioLineConfiguration`, with the sample format fixed at [`SIGNED_16_BIT`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioLine {
    pub sample_rate: u32,
    pub channel_count: u32,
    /// The `SampleFormat` value as sent or read.
    pub sample_format: i32,
}

impl AudioLine {
    /// Signed 16-bit PCM at `sample_rate` with `channel_count` channels.
    #[must_use]
    pub const fn pcm16(sample_rate: u32, channel_count: u32) -> Self {
        Self {
            sample_rate,
            channel_count,
            sample_format: SIGNED_16_BIT as i32,
        }
    }

    fn write(&self, out: &mut Writer) {
        out.uint(1, u64::from(self.sample_rate));
        out.uint(2, u64::from(self.channel_count));
        out.uint(3, self.sample_format as u64);
    }

    fn read(bytes: &[u8]) -> Result<Self, WireError> {
        let mut line = Self {
            sample_rate: 0,
            channel_count: 0,
            sample_format: 0,
        };
        wire::fields(bytes, |field, value| {
            match field {
                1 => line.sample_rate = value.uint32(field)?,
                2 => line.channel_count = value.uint32(field)?,
                3 => line.sample_format = value.enumeration(field)?,
                _ => {}
            }
            Ok(())
        })?;
        Ok(line)
    }
}

/// `VadConfiguration`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vad {
    pub confidence_threshold: f32,
    pub min_volume: f32,
    pub start_duration: Duration,
    pub stop_duration: Duration,
    pub backbuffer_duration: Duration,
}

impl Vad {
    /// The values every example of the documentation sends: confidence 0.4, minimum volume 0,
    /// 150 ms of speech to start, 390 ms of silence to stop, and the recommended 1 s kept before
    /// speech.
    pub const DOCUMENTED: Self = Self {
        confidence_threshold: 0.4,
        min_volume: 0.0,
        start_duration: Duration::from_millis(150),
        stop_duration: Duration::from_millis(390),
        backbuffer_duration: Duration::from_secs(1),
    };

    fn write(&self, out: &mut Writer) {
        out.float(1, self.confidence_threshold);
        out.float(2, self.min_volume);
        out.message(3, |out| write_duration(out, self.start_duration));
        out.message(4, |out| write_duration(out, self.stop_duration));
        out.message(5, |out| write_duration(out, self.backbuffer_duration));
    }

    fn read(bytes: &[u8]) -> Result<Self, WireError> {
        let mut vad = Self {
            confidence_threshold: 0.0,
            min_volume: 0.0,
            start_duration: Duration::ZERO,
            stop_duration: Duration::ZERO,
            backbuffer_duration: Duration::ZERO,
        };
        wire::fields(bytes, |field, value| {
            match field {
                1 => vad.confidence_threshold = value.float(field)?,
                2 => vad.min_volume = value.float(field)?,
                3 => vad.start_duration = read_duration(value.bytes(field)?)?,
                4 => vad.stop_duration = read_duration(value.bytes(field)?)?,
                5 => vad.backbuffer_duration = read_duration(value.bytes(field)?)?,
                _ => {}
            }
            Ok(())
        })?;
        Ok(vad)
    }
}

/// The protocol's own `Duration`: whole seconds and nanoseconds.
fn write_duration(out: &mut Writer, duration: Duration) {
    out.uint(1, duration.as_secs());
    out.uint(2, u64::from(duration.subsec_nanos()));
}

fn read_duration(bytes: &[u8]) -> Result<Duration, WireError> {
    let (mut seconds, mut nanos) = (0, 0);
    wire::fields(bytes, |field, value| {
        match field {
            1 => seconds = value.uint(field)?,
            2 => nanos = value.uint32(field)?,
            _ => {}
        }
        Ok(())
    })?;
    if nanos >= 1_000_000_000 {
        return Err(WireError::Range { field: 2 });
    }
    Ok(Duration::new(seconds, nanos))
}

/// `HostedTtsMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum HostedTtsMode {
    /// `HIGH_QUALITY`, the default and the documentation's recommendation.
    #[default]
    HighQuality,
    /// `LOW_LATENCY`: faster, at a quality the documentation warns may be much lower.
    LowLatency,
}

impl HostedTtsMode {
    const fn value(self) -> u64 {
        match self {
            Self::HighQuality => 0,
            Self::LowLatency => 1,
        }
    }
}

/// `TtsConfiguration` with the `hosted` provider and a `voice_ref`: speech in a voice Deepslate
/// hosts, named by its id. The ElevenLabs provider, which takes an ElevenLabs key in the message,
/// and an inline voice clone are not sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostedTts {
    pub voice_id: String,
    pub mode: HostedTtsMode,
}

/// `InitializeSessionRequest`, as the binding fills it.
#[derive(Debug, Clone, PartialEq)]
pub struct InitializeSession {
    pub input: AudioLine,
    pub output: AudioLine,
    pub vad: Vad,
    /// `InferenceConfiguration.system_prompt`.
    pub system_prompt: String,
    /// `InferenceConfiguration.temperature`; a plain `double`, so 0 and absent are one value on
    /// the wire.
    pub temperature: f64,
    /// `None` sends no `TtsConfiguration`: the server then sends text only.
    pub tts: Option<HostedTts>,
    pub supports_playback_reporting: bool,
}

impl InitializeSession {
    fn write(&self, out: &mut Writer) {
        out.message(1, |out| self.input.write(out));
        out.message(2, |out| self.output.write(out));
        out.message(3, |out| self.vad.write(out));
        out.message(4, |out| {
            out.string(1, &self.system_prompt);
            out.double(2, self.temperature);
        });
        if let Some(tts) = &self.tts {
            out.message(5, |out| {
                out.message(2, |out| {
                    out.message(1, |out| out.string(1, &tts.voice_id));
                    out.uint(2, tts.mode.value());
                });
            });
        }
        out.bool(6, self.supports_playback_reporting);
    }

    fn read(bytes: &[u8]) -> Result<Self, WireError> {
        let mut init = Self {
            input: AudioLine::read(&[])?,
            output: AudioLine::read(&[])?,
            vad: Vad::read(&[])?,
            system_prompt: String::new(),
            temperature: 0.0,
            tts: None,
            supports_playback_reporting: false,
        };
        wire::fields(bytes, |field, value| {
            match field {
                1 => init.input = AudioLine::read(value.bytes(field)?)?,
                2 => init.output = AudioLine::read(value.bytes(field)?)?,
                3 => init.vad = Vad::read(value.bytes(field)?)?,
                4 => wire::fields(value.bytes(field)?, |field, value| {
                    match field {
                        1 => init.system_prompt = value.string(field)?,
                        2 => init.temperature = value.double(field)?,
                        _ => {}
                    }
                    Ok(())
                })?,
                5 => init.tts = read_tts(value.bytes(field)?)?,
                6 => init.supports_playback_reporting = value.bool(field)?,
                _ => {}
            }
            Ok(())
        })?;
        Ok(init)
    }
}

/// A `TtsConfiguration` read back: the hosted provider's `voice_ref`, or `None` for any other.
fn read_tts(bytes: &[u8]) -> Result<Option<HostedTts>, WireError> {
    let mut tts = None;
    wire::fields(bytes, |field, value| {
        if field == 2 {
            let mut hosted = HostedTts {
                voice_id: String::new(),
                mode: HostedTtsMode::HighQuality,
            };
            wire::fields(value.bytes(field)?, |field, value| {
                match field {
                    1 => wire::fields(value.bytes(field)?, |field, value| {
                        if field == 1 {
                            hosted.voice_id = value.string(field)?;
                        }
                        Ok(())
                    })?,
                    2 if value.enumeration(field)? == 1 => hosted.mode = HostedTtsMode::LowLatency,
                    _ => {}
                }
                Ok(())
            })?;
            tts = Some(hosted);
        }
        Ok(())
    })?;
    Ok(tts)
}

/// `ToolDefinition`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDefinition {
    pub name: String,
    /// `None` leaves the optional description out.
    pub description: Option<String>,
    /// The JSON Schema of the parameters, a JSON object sent as a `google.protobuf.Struct`.
    /// `None`, or anything but an object, sends no parameters.
    pub parameters: Option<Json>,
}

/// `InferenceTriggerMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TriggerMode {
    /// `NO_TRIGGER`.
    NoTrigger,
    /// `QUEUE`.
    Queue,
    /// `IMMEDIATE`: interrupts any inference running and starts a new one; the documentation
    /// recommends it for streaming audio.
    Immediate,
    /// A value the definition does not name.
    Other(i32),
}

impl TriggerMode {
    const fn value(self) -> i32 {
        match self {
            Self::NoTrigger => 0,
            Self::Queue => 1,
            Self::Immediate => 2,
            Self::Other(value) => value,
        }
    }

    const fn of(value: i32) -> Self {
        match value {
            0 => Self::NoTrigger,
            1 => Self::Queue,
            2 => Self::Immediate,
            other => Self::Other(other),
        }
    }
}

/// A `ServiceBoundMessage`: what the client sends.
#[derive(Debug, Clone, PartialEq)]
pub enum ServiceBound {
    /// `initialize_session_request` (1).
    Initialize(InitializeSession),
    /// `user_input` (3) with `audio_data`.
    Audio {
        packet_id: u64,
        mode: TriggerMode,
        data: Vec<u8>,
    },
    /// `user_input` (3) with `text_data`; read back only.
    Text {
        packet_id: u64,
        mode: TriggerMode,
        text: String,
    },
    /// `update_tool_definitions_request` (4).
    UpdateTools(Vec<ToolDefinition>),
    /// `tool_call_response` (5).
    ToolCallResponse { id: String, result: String },
    /// `ping` (11).
    Ping,
    /// Another member of the `oneof`, by field number, read back only.
    Other(u32),
    /// No member at all.
    Empty,
}

impl ServiceBound {
    /// The message as one binary frame. [`ServiceBound::Other`] encodes as that field, empty,
    /// and [`ServiceBound::Empty`] as no field.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Writer::new();
        match self {
            Self::Initialize(init) => out.message(1, |out| init.write(out)),
            Self::Audio {
                packet_id,
                mode,
                data,
            } => out.message(3, |out| {
                out.uint(1, *packet_id);
                out.uint(2, mode.value() as u64);
                out.message(3, |out| out.bytes(1, data));
            }),
            Self::Text {
                packet_id,
                mode,
                text,
            } => out.message(3, |out| {
                out.uint(1, *packet_id);
                out.uint(2, mode.value() as u64);
                out.message(4, |out| out.string(1, text));
            }),
            Self::UpdateTools(tools) => out.message(4, |out| {
                for tool in tools {
                    out.message(1, |out| {
                        out.string(1, &tool.name);
                        out.optional_string(2, tool.description.as_deref());
                        if let Some(Json::Object(schema)) = &tool.parameters {
                            out.message(3, |out| write_struct(out, schema));
                        }
                    });
                }
            }),
            Self::ToolCallResponse { id, result } => out.message(5, |out| {
                out.string(1, id);
                out.string(2, result);
            }),
            Self::Ping => out.message(11, |_| {}),
            Self::Other(field) => out.message(*field, |_| {}),
            Self::Empty => {}
        }
        out.finish()
    }

    /// Reads one frame the client sent.
    ///
    /// # Errors
    /// [`ProtocolError`] when the bytes are not such a message.
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let mut message = Self::Empty;
        wire::fields(bytes, |field, value| {
            message = match field {
                1 => Self::Initialize(InitializeSession::read(value.bytes(field)?)?),
                3 => read_user_input(value.bytes(field)?)?,
                4 => {
                    let mut tools = Vec::new();
                    wire::fields(value.bytes(field)?, |field, value| {
                        if field == 1 {
                            tools.push(read_tool(value.bytes(field)?)?);
                        }
                        Ok(())
                    })?;
                    Self::UpdateTools(tools)
                }
                5 => {
                    let (mut id, mut result) = (String::new(), String::new());
                    wire::fields(value.bytes(field)?, |field, value| {
                        match field {
                            1 => id = value.string(field)?,
                            2 => result = value.string(field)?,
                            _ => {}
                        }
                        Ok(())
                    })?;
                    Self::ToolCallResponse { id, result }
                }
                11 => {
                    value.bytes(field)?;
                    Self::Ping
                }
                2 | 6..=10 => {
                    value.bytes(field)?;
                    Self::Other(field)
                }
                _ => return Ok(()),
            };
            Ok(())
        })?;
        Ok(message)
    }
}

fn read_user_input(bytes: &[u8]) -> Result<ServiceBound, WireError> {
    let (mut packet_id, mut mode) = (0, TriggerMode::NoTrigger);
    let mut audio = None;
    let mut text = None;
    wire::fields(bytes, |field, value| {
        match field {
            1 => packet_id = value.uint(field)?,
            2 => mode = TriggerMode::of(value.enumeration(field)?),
            3 => {
                let mut data = Vec::new();
                wire::fields(value.bytes(field)?, |field, value| {
                    if field == 1 {
                        data = value.bytes(field)?.to_vec();
                    }
                    Ok(())
                })?;
                audio = Some(data);
                text = None;
            }
            4 => {
                let mut data = String::new();
                wire::fields(value.bytes(field)?, |field, value| {
                    if field == 1 {
                        data = value.string(field)?;
                    }
                    Ok(())
                })?;
                text = Some(data);
                audio = None;
            }
            _ => {}
        }
        Ok(())
    })?;
    Ok(match (audio, text) {
        (_, Some(text)) => ServiceBound::Text {
            packet_id,
            mode,
            text,
        },
        (audio, None) => ServiceBound::Audio {
            packet_id,
            mode,
            data: audio.unwrap_or_default(),
        },
    })
}

fn read_tool(bytes: &[u8]) -> Result<ToolDefinition, WireError> {
    let mut tool = ToolDefinition {
        name: String::new(),
        description: None,
        parameters: None,
    };
    wire::fields(bytes, |field, value| {
        match field {
            1 => tool.name = value.string(field)?,
            2 => tool.description = Some(value.string(field)?),
            3 => tool.parameters = Some(Json::Object(read_struct(value.bytes(field)?, 0)?)),
            _ => {}
        }
        Ok(())
    })?;
    Ok(tool)
}

/// `SessionErrorCategory`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCategory {
    Unknown,
    Session,
    Configuration,
    Protocol,
    Inference,
    Audio,
    Tts,
    Internal,
    /// A value the definition does not name.
    Other(i32),
}

impl ErrorCategory {
    const fn of(value: i32) -> Self {
        match value {
            0 => Self::Unknown,
            1 => Self::Session,
            2 => Self::Configuration,
            3 => Self::Protocol,
            4 => Self::Inference,
            5 => Self::Audio,
            6 => Self::Tts,
            7 => Self::Internal,
            other => Self::Other(other),
        }
    }

    const fn value(self) -> i32 {
        match self {
            Self::Unknown => 0,
            Self::Session => 1,
            Self::Configuration => 2,
            Self::Protocol => 3,
            Self::Inference => 4,
            Self::Audio => 5,
            Self::Tts => 6,
            Self::Internal => 7,
            Self::Other(value) => value,
        }
    }

    /// The name the definition gives it, `ERROR_CONFIGURATION` for one.
    #[must_use]
    pub fn name(self) -> String {
        match self {
            Self::Unknown => "ERROR_UNKNOWN".to_owned(),
            Self::Session => "ERROR_SESSION".to_owned(),
            Self::Configuration => "ERROR_CONFIGURATION".to_owned(),
            Self::Protocol => "ERROR_PROTOCOL".to_owned(),
            Self::Inference => "ERROR_INFERENCE".to_owned(),
            Self::Audio => "ERROR_AUDIO".to_owned(),
            Self::Tts => "ERROR_TTS".to_owned(),
            Self::Internal => "ERROR_INTERNAL".to_owned(),
            Self::Other(value) => format!("category {value}"),
        }
    }
}

impl fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

/// `VadState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VadState {
    Silence,
    SpeechStarting,
    Speech,
    SpeechEnding,
    /// A value the definition does not name.
    Other(i32),
}

impl VadState {
    const fn of(value: i32) -> Self {
        match value {
            0 => Self::Silence,
            1 => Self::SpeechStarting,
            2 => Self::Speech,
            3 => Self::SpeechEnding,
            other => Self::Other(other),
        }
    }

    const fn value(self) -> i32 {
        match self {
            Self::Silence => 0,
            Self::SpeechStarting => 1,
            Self::Speech => 2,
            Self::SpeechEnding => 3,
            Self::Other(value) => value,
        }
    }
}

/// A `ClientBoundMessage`: what the server sends.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientBound {
    /// `tool_call_request` (1); `parameters` is the JSON object of its `Struct`.
    ToolCallRequest {
        id: String,
        name: String,
        parameters: Json,
        turn_id: Option<u32>,
    },
    /// `model_text_fragment` (2).
    ModelTextFragment { text: String, turn_id: u32 },
    /// `model_audio_chunk` (3).
    ModelAudioChunk {
        audio: Vec<u8>,
        turn_id: Option<u32>,
    },
    /// `playback_clear_buffer` (4).
    PlaybackClearBuffer,
    /// `response_begin` (5).
    ResponseBegin { turn_id: u32 },
    /// `response_end` (6).
    ResponseEnd { turn_id: u32 },
    /// `error` (8): a `SessionErrorNotification`.
    Error {
        category: ErrorCategory,
        message: String,
        trace_id: Option<String>,
    },
    /// `user_transcription_result` (9).
    UserTranscription {
        turn_id: u32,
        text: String,
        language: String,
    },
    /// `session_ready` (11).
    SessionReady,
    /// `vad_state_event` (13); `session_time` is not read.
    VadStateEvent {
        from: VadState,
        to: VadState,
        packet_id: u64,
    },
    /// `pong` (15).
    Pong,
    /// `inference_complete` (18).
    InferenceComplete { turn_id: u32 },
    /// A member the binding passes over, by field number: 7, 10, 12, 14, 16 or 17, or one the
    /// definition does not name.
    Other(u32),
    /// No member at all.
    Empty,
}

impl ClientBound {
    /// The message as one binary frame.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Writer::new();
        match self {
            Self::ToolCallRequest {
                id,
                name,
                parameters,
                turn_id,
            } => out.message(1, |out| {
                out.string(1, id);
                out.string(2, name);
                if let Json::Object(parameters) = parameters {
                    out.message(3, |out| write_struct(out, parameters));
                }
                out.optional_uint(4, turn_id.map(u64::from));
            }),
            Self::ModelTextFragment { text, turn_id } => out.message(2, |out| {
                out.string(1, text);
                out.uint(2, u64::from(*turn_id));
            }),
            Self::ModelAudioChunk { audio, turn_id } => out.message(3, |out| {
                out.message(1, |out| out.bytes(1, audio));
                out.optional_uint(3, turn_id.map(u64::from));
            }),
            Self::PlaybackClearBuffer => out.message(4, |_| {}),
            Self::ResponseBegin { turn_id } => {
                out.message(5, |out| out.uint(1, u64::from(*turn_id)));
            }
            Self::ResponseEnd { turn_id } => out.message(6, |out| out.uint(1, u64::from(*turn_id))),
            Self::Error {
                category,
                message,
                trace_id,
            } => out.message(8, |out| {
                out.uint(1, category.value() as u64);
                out.string(2, message);
                out.optional_string(3, trace_id.as_deref());
            }),
            Self::UserTranscription {
                turn_id,
                text,
                language,
            } => out.message(9, |out| {
                out.uint(1, u64::from(*turn_id));
                out.string(2, text);
                out.string(3, language);
            }),
            Self::SessionReady => out.message(11, |_| {}),
            Self::VadStateEvent {
                from,
                to,
                packet_id,
            } => out.message(13, |out| {
                out.uint(2, from.value() as u64);
                out.uint(3, to.value() as u64);
                out.uint(4, *packet_id);
            }),
            Self::Pong => out.message(15, |_| {}),
            Self::InferenceComplete { turn_id } => {
                out.message(18, |out| out.uint(1, u64::from(*turn_id)));
            }
            Self::Other(field) => out.message(*field, |_| {}),
            Self::Empty => {}
        }
        out.finish()
    }

    /// Reads one frame the server sent. The last member of the `oneof` in the frame wins, as the
    /// encoding defines; a field number the definition does not name is [`ClientBound::Other`]
    /// when no member came.
    ///
    /// # Errors
    /// [`ProtocolError`] when the bytes are not such a message, or a member the binding reads is
    /// not one.
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let mut message = Self::Empty;
        let mut unknown = None;
        wire::fields(bytes, |field, value| {
            message = match field {
                1 => read_tool_call(value.bytes(field)?)?,
                2 => {
                    let (mut text, mut turn_id) = (String::new(), 0);
                    wire::fields(value.bytes(field)?, |field, value| {
                        match field {
                            1 => text = value.string(field)?,
                            2 => turn_id = value.uint32(field)?,
                            _ => {}
                        }
                        Ok(())
                    })?;
                    Self::ModelTextFragment { text, turn_id }
                }
                3 => {
                    let (mut audio, mut turn_id) = (Vec::new(), None);
                    wire::fields(value.bytes(field)?, |field, value| {
                        match field {
                            1 => wire::fields(value.bytes(field)?, |field, value| {
                                if field == 1 {
                                    audio = value.bytes(field)?.to_vec();
                                }
                                Ok(())
                            })?,
                            3 => turn_id = Some(value.uint32(field)?),
                            _ => {}
                        }
                        Ok(())
                    })?;
                    Self::ModelAudioChunk { audio, turn_id }
                }
                4 => {
                    value.bytes(field)?;
                    Self::PlaybackClearBuffer
                }
                5 => Self::ResponseBegin {
                    turn_id: read_turn(value.bytes(field)?)?,
                },
                6 => Self::ResponseEnd {
                    turn_id: read_turn(value.bytes(field)?)?,
                },
                8 => {
                    let (mut category, mut text, mut trace_id) =
                        (ErrorCategory::Unknown, String::new(), None);
                    wire::fields(value.bytes(field)?, |field, value| {
                        match field {
                            1 => category = ErrorCategory::of(value.enumeration(field)?),
                            2 => text = value.string(field)?,
                            3 => trace_id = Some(value.string(field)?),
                            _ => {}
                        }
                        Ok(())
                    })?;
                    Self::Error {
                        category,
                        message: text,
                        trace_id,
                    }
                }
                9 => {
                    let (mut turn_id, mut text, mut language) = (0, String::new(), String::new());
                    wire::fields(value.bytes(field)?, |field, value| {
                        match field {
                            1 => turn_id = value.uint32(field)?,
                            2 => text = value.string(field)?,
                            3 => language = value.string(field)?,
                            _ => {}
                        }
                        Ok(())
                    })?;
                    Self::UserTranscription {
                        turn_id,
                        text,
                        language,
                    }
                }
                11 => {
                    value.bytes(field)?;
                    Self::SessionReady
                }
                13 => {
                    let (mut from, mut to, mut packet_id) =
                        (VadState::Silence, VadState::Silence, 0);
                    wire::fields(value.bytes(field)?, |field, value| {
                        match field {
                            2 => from = VadState::of(value.enumeration(field)?),
                            3 => to = VadState::of(value.enumeration(field)?),
                            4 => packet_id = value.uint(field)?,
                            _ => {}
                        }
                        Ok(())
                    })?;
                    Self::VadStateEvent {
                        from,
                        to,
                        packet_id,
                    }
                }
                15 => {
                    value.bytes(field)?;
                    Self::Pong
                }
                18 => Self::InferenceComplete {
                    turn_id: read_turn(value.bytes(field)?)?,
                },
                7 | 10 | 12 | 14 | 16 | 17 => {
                    value.bytes(field)?;
                    Self::Other(field)
                }
                _ => {
                    unknown = Some(field);
                    return Ok(());
                }
            };
            Ok(())
        })?;
        Ok(match (message, unknown) {
            (Self::Empty, Some(field)) => Self::Other(field),
            (message, _) => message,
        })
    }
}

/// The `turn_id` (field 1) of a message that carries only that.
fn read_turn(bytes: &[u8]) -> Result<u32, WireError> {
    let mut turn_id = 0;
    wire::fields(bytes, |field, value| {
        if field == 1 {
            turn_id = value.uint32(field)?;
        }
        Ok(())
    })?;
    Ok(turn_id)
}

fn read_tool_call(bytes: &[u8]) -> Result<ClientBound, WireError> {
    let (mut id, mut name, mut parameters, mut turn_id) =
        (String::new(), String::new(), Map::new(), None);
    wire::fields(bytes, |field, value| {
        match field {
            1 => id = value.string(field)?,
            2 => name = value.string(field)?,
            3 => parameters = read_struct(value.bytes(field)?, 0)?,
            4 => turn_id = Some(value.uint32(field)?),
            _ => {}
        }
        Ok(())
    })?;
    Ok(ClientBound::ToolCallRequest {
        id,
        name,
        parameters: Json::Object(parameters),
        turn_id,
    })
}

/// A JSON object as a `google.protobuf.Struct`: one map entry (field 1) per member, its key
/// field 1 and its `Value` field 2.
fn write_struct(out: &mut Writer, object: &Map<String, Json>) {
    for (key, value) in object {
        out.message(1, |out| {
            out.bytes_always(1, key.as_bytes());
            out.message(2, |out| write_value(out, value));
        });
    }
}

/// A JSON value as a `google.protobuf.Value`: `null_value` (1), `number_value` (2),
/// `string_value` (3), `bool_value` (4), `struct_value` (5) or `list_value` (6). A number beyond
/// a `double`'s precision is sent rounded to one, as the type holds nothing else.
fn write_value(out: &mut Writer, value: &Json) {
    match value {
        Json::Null => out.optional_uint(1, Some(0)),
        Json::Bool(flag) => out.optional_uint(4, Some(u64::from(*flag))),
        Json::Number(number) => out.double_always(2, number.as_f64().unwrap_or(0.0)),
        Json::String(text) => out.bytes_always(3, text.as_bytes()),
        Json::Object(object) => out.message(5, |out| write_struct(out, object)),
        Json::Array(items) => out.message(6, |out| {
            for item in items {
                out.message(1, |out| write_value(out, item));
            }
        }),
    }
}

fn read_struct(bytes: &[u8], depth: usize) -> Result<Map<String, Json>, WireError> {
    if depth > STRUCT_DEPTH {
        return Err(WireError::Depth);
    }
    let mut object = Map::new();
    wire::fields(bytes, |field, value| {
        if field == 1 {
            let (mut key, mut member) = (String::new(), Json::Null);
            wire::fields(value.bytes(field)?, |field, value| {
                match field {
                    1 => key = value.string(field)?,
                    2 => member = read_value(value.bytes(field)?, depth + 1)?,
                    _ => {}
                }
                Ok(())
            })?;
            object.insert(key, member);
        }
        Ok(())
    })?;
    Ok(object)
}

fn read_value(bytes: &[u8], depth: usize) -> Result<Json, WireError> {
    if depth > STRUCT_DEPTH {
        return Err(WireError::Depth);
    }
    let mut json = Json::Null;
    wire::fields(bytes, |field, value| {
        json = match field {
            1 => {
                value.enumeration(field)?;
                Json::Null
            }
            2 => number(value.double(field)?).ok_or(WireError::Range { field })?,
            3 => Json::String(value.string(field)?),
            4 => Json::Bool(value.bool(field)?),
            5 => Json::Object(read_struct(value.bytes(field)?, depth + 1)?),
            6 => {
                let mut items = Vec::new();
                wire::fields(value.bytes(field)?, |field, value| {
                    if field == 1 {
                        items.push(read_value(value.bytes(field)?, depth + 1)?);
                    }
                    Ok(())
                })?;
                Json::Array(items)
            }
            _ => return Ok(()),
        };
        Ok(())
    })?;
    Ok(json)
}

/// A `double` as JSON: an integer when it is whole and within ±2^53, a float otherwise, `None`
/// when it is not finite.
fn number(value: f64) -> Option<Json> {
    const EXACT: f64 = 9_007_199_254_740_992.0;
    if value.fract() == 0.0 && value.abs() <= EXACT {
        return Some(Json::Number(Number::from(value as i64)));
    }
    Number::from_f64(value).map(Json::Number)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// `ResponseBegin { turn_id: 4 }` as the definition lays it out: `ClientBoundMessage` field
    /// 5, length 2, holding field 1 = 4.
    #[test]
    fn a_response_begin_is_field_five_holding_its_turn() {
        let bytes = ClientBound::ResponseBegin { turn_id: 4 }.encode();
        assert_eq!(bytes, [0x2a, 0x02, 0x08, 0x04]);
        assert_eq!(
            ClientBound::decode(&bytes),
            Ok(ClientBound::ResponseBegin { turn_id: 4 })
        );
    }

    /// Turn 0 is a real turn: a `ResponseBegin` of turn 0 carries no field, and reads as turn 0.
    #[test]
    fn turn_zero_is_an_empty_member_and_reads_as_zero() {
        let bytes = ClientBound::ResponseBegin { turn_id: 0 }.encode();
        assert_eq!(bytes, [0x2a, 0x00]);
        assert_eq!(
            ClientBound::decode(&bytes),
            Ok(ClientBound::ResponseBegin { turn_id: 0 })
        );
    }

    /// `Ping {}` is `ServiceBoundMessage` field 11, empty: `5a 00`.
    #[test]
    fn a_ping_is_an_empty_field_eleven() {
        assert_eq!(ServiceBound::Ping.encode(), [0x5a, 0x00]);
        assert_eq!(ServiceBound::decode(&[0x5a, 0x00]), Ok(ServiceBound::Ping));
        assert_eq!(ClientBound::decode(&[0x7a, 0x00]), Ok(ClientBound::Pong));
    }

    /// `UserInput { packet_id: 1, mode: IMMEDIATE, audio_data { data: 01 02 } }`.
    #[test]
    fn user_audio_is_field_three_with_its_packet_mode_and_data() {
        let message = ServiceBound::Audio {
            packet_id: 1,
            mode: TriggerMode::Immediate,
            data: vec![1, 2],
        };
        let bytes = message.encode();
        assert_eq!(
            bytes,
            [
                0x1a, 0x0a, 0x08, 0x01, 0x10, 0x02, 0x1a, 0x04, 0x0a, 0x02, 0x01, 0x02
            ]
        );
        assert_eq!(ServiceBound::decode(&bytes), Ok(message));
    }

    #[test]
    fn the_initialize_request_reads_back_as_written() {
        let init = InitializeSession {
            input: AudioLine::pcm16(16_000, 1),
            output: AudioLine::pcm16(48_000, 1),
            vad: Vad::DOCUMENTED,
            system_prompt: "You are a helpful assistant.".to_owned(),
            temperature: 0.7,
            tts: Some(HostedTts {
                voice_id: "de56bbd1-3ea7-4543-a17c-504664d9116b".to_owned(),
                mode: HostedTtsMode::LowLatency,
            }),
            supports_playback_reporting: false,
        };
        let bytes = ServiceBound::Initialize(init.clone()).encode();
        assert_eq!(bytes[0], 0x0a);
        assert_eq!(
            ServiceBound::decode(&bytes),
            Ok(ServiceBound::Initialize(init))
        );
    }

    #[test]
    fn tool_parameters_cross_as_a_struct_and_back() {
        let schema = json!({
            "type": "object",
            "properties": {"location": {"type": "string"}, "days": {"type": "integer", "maximum": 7}},
            "required": ["location"],
            "additionalProperties": false,
            "default": null,
            "scale": 0.5
        });
        let tools = vec![ToolDefinition {
            name: "get_weather".to_owned(),
            description: Some(String::new()),
            parameters: Some(schema.clone()),
        }];
        let bytes = ServiceBound::UpdateTools(tools.clone()).encode();
        assert_eq!(
            ServiceBound::decode(&bytes),
            Ok(ServiceBound::UpdateTools(tools))
        );

        let call = ClientBound::ToolCallRequest {
            id: "call_abc123".to_owned(),
            name: "get_weather".to_owned(),
            parameters: json!({"location": "Amsterdam", "days": 3}),
            turn_id: Some(0),
        };
        assert_eq!(ClientBound::decode(&call.encode()), Ok(call));
    }

    #[test]
    fn a_struct_nested_past_the_bound_is_refused() {
        let mut value = json!("leaf");
        for _ in 0..=STRUCT_DEPTH {
            value = json!({ "a": value });
        }
        let call = ClientBound::ToolCallRequest {
            id: "c".to_owned(),
            name: "t".to_owned(),
            parameters: value,
            turn_id: None,
        };
        assert_eq!(ClientBound::decode(&call.encode()), Err(WireError::Depth));
    }

    #[test]
    fn a_number_that_is_not_finite_is_refused() {
        let mut out = Writer::new();
        out.message(1, |out| {
            out.string(1, "c");
            out.message(3, |out| {
                out.message(1, |out| {
                    out.bytes_always(1, b"x");
                    out.message(2, |out| out.double_always(2, f64::NAN));
                });
            });
        });
        assert_eq!(
            ClientBound::decode(&out.finish()),
            Err(WireError::Range { field: 2 })
        );
    }

    #[test]
    fn every_server_message_reads_back_as_written() {
        for message in [
            ClientBound::ModelTextFragment {
                text: "Hello, how can I help you?".to_owned(),
                turn_id: 4,
            },
            ClientBound::ModelAudioChunk {
                audio: vec![1, 0, 2, 0],
                turn_id: Some(4),
            },
            ClientBound::ModelAudioChunk {
                audio: vec![1, 0],
                turn_id: None,
            },
            ClientBound::PlaybackClearBuffer,
            ClientBound::ResponseEnd { turn_id: 4 },
            ClientBound::Error {
                category: ErrorCategory::Configuration,
                message: "Invalid sample rate".to_owned(),
                trace_id: Some("abc-123-xyz".to_owned()),
            },
            ClientBound::UserTranscription {
                turn_id: 3,
                text: "What time is it?".to_owned(),
                language: "en".to_owned(),
            },
            ClientBound::SessionReady,
            ClientBound::VadStateEvent {
                from: VadState::SpeechStarting,
                to: VadState::Speech,
                packet_id: 42,
            },
            ClientBound::InferenceComplete { turn_id: 4 },
            ClientBound::Other(16),
        ] {
            assert_eq!(ClientBound::decode(&message.encode()), Ok(message.clone()));
        }
    }

    #[test]
    fn a_member_the_definition_does_not_name_is_other_and_a_known_one_wins() {
        assert_eq!(
            ClientBound::decode(&[0xa2, 0x01, 0x00]),
            Ok(ClientBound::Other(20))
        );
        assert_eq!(
            ClientBound::decode(&[0xa2, 0x01, 0x00, 0x22, 0x00]),
            Ok(ClientBound::PlaybackClearBuffer)
        );
        assert_eq!(ClientBound::decode(&[]), Ok(ClientBound::Empty));
    }

    #[test]
    fn a_known_member_in_the_wrong_wire_type_is_refused() {
        assert_eq!(
            ClientBound::decode(&[0x28, 0x04]),
            Err(WireError::Mismatch { field: 5 })
        );
    }
}
