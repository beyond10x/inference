//! Amazon Nova Sonic over Bedrock's bidirectional stream.
//!
//! The event shapes are those of the Amazon Nova 2 user guide: "Handling input events with the
//! bidirectional API" (<https://docs.aws.amazon.com/nova/latest/nova2-userguide/sonic-input-events.html>)
//! for [`InputEvent`] and "Handling output events with the bidirectional API"
//! (<https://docs.aws.amazon.com/nova/latest/nova2-userguide/sonic-output-events.html>) for
//! [`OutputEvent`]. The service errors are those of the `InvokeModelWithBidirectionalStream` API
//! reference.
//!
//! A [`Session`] is transport-neutral: it writes encoded input events into one channel and reads
//! output frames from another, so its sequence is testable offline. [`NovaSonic::start`] binds a
//! session to the service.

mod client;
mod config;
mod error;
mod input;
mod output;
mod session;

pub use client::NovaSonic;
pub use config::{AudioRate, Endpointing, InferenceConfiguration, SessionConfig, ToolSpec};
pub use error::{NovaSonicError, ServiceError, ServiceErrorKind, redact};
pub use input::{InputContent, InputEvent};
pub use output::{
    AudioOutput, CompletionIds, ContentKind, Decoder, GenerationStage, OutputContentEnd,
    OutputContentStart, OutputEvent, StopReason, TextOutput, TokenCounts, TokenUsage, ToolUse,
    Usage,
};
pub use session::{OutputFrame, Session, SessionReader, SessionWriter};

/// The model id used when [`MODEL_ID_ENV`] is unset or empty: Amazon Nova 2 Sonic, the id the
/// Bedrock model card names (<https://docs.aws.amazon.com/bedrock/latest/userguide/model-card-amazon-nova-2-sonic.html>).
pub const DEFAULT_MODEL_ID: &str = "amazon.nova-2-sonic-v1:0";

/// The environment variable that overrides [`DEFAULT_MODEL_ID`].
pub const MODEL_ID_ENV: &str = "INFERENCE_NOVA_SONIC_MODEL";

/// The model id for an override value: the value itself, or [`DEFAULT_MODEL_ID`] when it is absent
/// or empty.
pub fn resolve_model_id(value: Option<&str>) -> String {
    match value {
        Some(id) if !id.trim().is_empty() => id.trim().to_owned(),
        _ => DEFAULT_MODEL_ID.to_owned(),
    }
}

/// The model id [`MODEL_ID_ENV`] names, or [`DEFAULT_MODEL_ID`].
pub fn model_id_from_env() -> String {
    resolve_model_id(std::env::var(MODEL_ID_ENV).ok().as_deref())
}

/// Who a content block is from, as the `role` of a content start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    /// `SYSTEM`: the system prompt.
    System,
    /// `USER`: the user's audio, text or transcript.
    User,
    /// `ASSISTANT`: the model's text or audio, or history.
    Assistant,
    /// `TOOL`: a tool use or a tool result.
    Tool,
    /// `SYSTEM_SPEECH`: transcription formatting instructions.
    SystemSpeech,
    /// A role the reference does not list, kept as sent.
    Other(String),
}

impl Role {
    /// The wire name.
    pub fn as_str(&self) -> &str {
        match self {
            Role::System => "SYSTEM",
            Role::User => "USER",
            Role::Assistant => "ASSISTANT",
            Role::Tool => "TOOL",
            Role::SystemSpeech => "SYSTEM_SPEECH",
            Role::Other(name) => name,
        }
    }

    pub(crate) fn parse(name: &str) -> Role {
        match name {
            "SYSTEM" => Role::System,
            "USER" => Role::User,
            "ASSISTANT" => Role::Assistant,
            "TOOL" => Role::Tool,
            "SYSTEM_SPEECH" => Role::SystemSpeech,
            other => Role::Other(other.to_owned()),
        }
    }
}
