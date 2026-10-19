//! The output events the service sends, from "Handling output events with the bidirectional API".

use std::collections::HashMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;

use super::Role;
use super::error::{NovaSonicError, ServiceError};

/// The identifiers every output event of one response repeats.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompletionIds {
    /// `sessionId`.
    pub session_id: String,
    /// `promptName`, the prompt the response belongs to.
    pub prompt_name: String,
    /// `completionId`, one response.
    pub completion_id: String,
}

/// What a content block carries, the `type` of its content start or end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentKind {
    /// `TEXT`: a user transcript or an assistant text.
    Text,
    /// `AUDIO`: assistant speech.
    Audio,
    /// `TOOL`: a tool use.
    Tool,
    /// A type the reference does not list, kept as sent.
    Other(String),
}

/// The `generationStage` of a text content start's `additionalModelFields`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenerationStage {
    /// `SPECULATIVE`: a preview of what the model plans to say.
    Speculative,
    /// `FINAL`: the user transcript, or what the model actually said.
    Final,
    /// A stage the reference does not list, kept as sent.
    Other(String),
}

/// Why a content block or a completion ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// `PARTIAL_TURN`: more content of the same turn follows.
    PartialTurn,
    /// `END_TURN`: the turn is complete.
    EndTurn,
    /// `INTERRUPTED`: the user barged in.
    Interrupted,
    /// `TOOL_USE`: the model waits for a tool result.
    ToolUse,
    /// A reason the reference does not list, kept as sent.
    Other(String),
}

/// A `contentStart` the service sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputContentStart {
    /// `completionId`, when present.
    pub completion_id: Option<String>,
    /// `contentId`, repeated by the block's content and end.
    pub content_id: String,
    /// `type`.
    pub kind: ContentKind,
    /// `role`.
    pub role: Role,
    /// The `generationStage` of `additionalModelFields`, on text blocks.
    pub stage: Option<GenerationStage>,
    /// The `sampleRateHertz` of `audioOutputConfiguration`, on audio blocks.
    pub audio_rate_hertz: Option<u32>,
}

/// A `textOutput`, with the role and stage of its content start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextOutput {
    /// `completionId`, when present.
    pub completion_id: Option<String>,
    /// `contentId`.
    pub content_id: String,
    /// The role of the content start with this id, if one was seen.
    pub role: Option<Role>,
    /// The generation stage of the content start with this id, if one was seen.
    pub stage: Option<GenerationStage>,
    /// The text.
    pub content: String,
}

/// An `audioOutput`, decoded from base64.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioOutput {
    /// `completionId`, when present.
    pub completion_id: Option<String>,
    /// `contentId`.
    pub content_id: String,
    /// Little-endian 16-bit mono samples at the audio content start's rate.
    pub pcm: Vec<u8>,
}

/// A `toolUse`: the model asks for a tool to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolUse {
    /// `completionId`, when present.
    pub completion_id: Option<String>,
    /// `contentId`.
    pub content_id: String,
    /// `toolUseId`, which the tool result names.
    pub tool_use_id: String,
    /// `toolName`.
    pub tool_name: String,
    /// The tool input, a JSON string.
    pub content: String,
}

/// A `contentEnd` the service sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputContentEnd {
    /// `completionId`, when present.
    pub completion_id: Option<String>,
    /// `contentId`.
    pub content_id: String,
    /// `type`.
    pub kind: ContentKind,
    /// `stopReason`, when present.
    pub stop_reason: Option<StopReason>,
}

/// Speech and text token counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TokenCounts {
    /// `speechTokens`.
    pub speech: u64,
    /// `textTokens`.
    pub text: u64,
}

/// Input and output token counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TokenUsage {
    /// `input`.
    pub input: TokenCounts,
    /// `output`.
    pub output: TokenCounts,
}

/// A `usageEvent`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Usage {
    /// `completionId`, when present.
    pub completion_id: Option<String>,
    /// `details.delta`: the change since the previous usage event.
    pub delta: TokenUsage,
    /// `details.total`: the running totals.
    pub total: TokenUsage,
    /// `totalInputTokens`.
    pub total_input_tokens: u64,
    /// `totalOutputTokens`.
    pub total_output_tokens: u64,
    /// `totalTokens`.
    pub total_tokens: u64,
}

/// One output event.
#[derive(Debug, Clone, PartialEq)]
pub enum OutputEvent {
    /// `completionStart`: a response begins.
    CompletionStart(CompletionIds),
    /// `contentStart`.
    ContentStart(OutputContentStart),
    /// `textOutput`.
    TextOutput(TextOutput),
    /// `audioOutput`.
    AudioOutput(AudioOutput),
    /// `toolUse`.
    ToolUse(ToolUse),
    /// `contentEnd`.
    ContentEnd(OutputContentEnd),
    /// `completionEnd`: a response ends.
    CompletionEnd {
        /// The response's identifiers.
        ids: CompletionIds,
        /// `stopReason`.
        stop_reason: StopReason,
    },
    /// The barge-in notice: the user spoke over the assistant, which stopped. The service sends it
    /// as a `textOutput` whose content is `{ "interrupted" : true }` (the interruption check of the
    /// bidirectional streaming sample, <https://docs.aws.amazon.com/nova/latest/userguide/speech-bidirection.html>);
    /// it is never assistant text. A client drops the audio it has queued but not yet played.
    Interrupted {
        /// `completionId`, when present.
        completion_id: Option<String>,
        /// The `contentId` of the text block that carried the notice.
        content_id: String,
    },
    /// `usageEvent`.
    Usage(Usage),
    /// An exception the service reported on the stream.
    Error(ServiceError),
    /// An event the reference does not describe, kept by name with its body.
    Unknown {
        /// The event's name, the key under `event`.
        name: String,
        /// The event's body.
        body: Value,
    },
}

/// Decodes output frames and remembers each content start, so that a text output carries the role
/// and stage of the block it belongs to.
#[derive(Debug, Default)]
pub struct Decoder {
    starts: HashMap<String, (Role, Option<GenerationStage>)>,
}

impl Decoder {
    /// The event one `chunk` of the bidirectional stream carries.
    pub fn decode(&mut self, frame: &[u8]) -> Result<OutputEvent, NovaSonicError> {
        let value: Value = serde_json::from_slice(frame)
            .map_err(|error| NovaSonicError::Decode(format!("not JSON: {error}")))?;
        let event = value
            .get("event")
            .and_then(Value::as_object)
            .ok_or_else(|| NovaSonicError::Decode("no `event` object".into()))?;
        let (name, body) = event
            .iter()
            .next()
            .ok_or_else(|| NovaSonicError::Decode("an empty `event` object".into()))?;
        let event = match name.as_str() {
            "completionStart" => OutputEvent::CompletionStart(ids(body)),
            "contentStart" => {
                let start = OutputContentStart {
                    completion_id: optional(body, "completionId"),
                    content_id: required(name, body, "contentId")?,
                    kind: ContentKind::parse(&required(name, body, "type")?),
                    role: Role::parse(&required(name, body, "role")?),
                    stage: stage(body),
                    audio_rate_hertz: body
                        .get("audioOutputConfiguration")
                        .and_then(|audio| audio.get("sampleRateHertz"))
                        .and_then(Value::as_u64)
                        .and_then(|hertz| u32::try_from(hertz).ok()),
                };
                self.starts.insert(
                    start.content_id.clone(),
                    (start.role.clone(), start.stage.clone()),
                );
                OutputEvent::ContentStart(start)
            }
            "textOutput" => {
                let content_id = required(name, body, "contentId")?;
                let content = required(name, body, "content")?;
                if is_interruption_notice(&content) {
                    return Ok(OutputEvent::Interrupted {
                        completion_id: optional(body, "completionId"),
                        content_id,
                    });
                }
                let (role, stage) = match self.starts.get(&content_id) {
                    Some((role, stage)) => (Some(role.clone()), stage.clone()),
                    None => (None, None),
                };
                OutputEvent::TextOutput(TextOutput {
                    completion_id: optional(body, "completionId"),
                    content,
                    content_id,
                    role,
                    stage,
                })
            }
            "audioOutput" => {
                let content = required(name, body, "content")?;
                let pcm = STANDARD.decode(content.as_bytes()).map_err(|error| {
                    NovaSonicError::Decode(format!("`audioOutput.content` is not base64: {error}"))
                })?;
                OutputEvent::AudioOutput(AudioOutput {
                    completion_id: optional(body, "completionId"),
                    content_id: required(name, body, "contentId")?,
                    pcm,
                })
            }
            "toolUse" => OutputEvent::ToolUse(ToolUse {
                completion_id: optional(body, "completionId"),
                content_id: required(name, body, "contentId")?,
                tool_use_id: required(name, body, "toolUseId")?,
                tool_name: required(name, body, "toolName")?,
                content: match body.get("content") {
                    Some(Value::String(content)) => content.clone(),
                    Some(other) => other.to_string(),
                    None => return Err(missing(name, "content")),
                },
            }),
            "contentEnd" => {
                let content_id = required(name, body, "contentId")?;
                self.starts.remove(&content_id);
                OutputEvent::ContentEnd(OutputContentEnd {
                    completion_id: optional(body, "completionId"),
                    content_id,
                    kind: ContentKind::parse(&required(name, body, "type")?),
                    stop_reason: optional(body, "stopReason")
                        .map(|reason| StopReason::parse(&reason)),
                })
            }
            "completionEnd" => OutputEvent::CompletionEnd {
                ids: ids(body),
                stop_reason: StopReason::parse(&required(name, body, "stopReason")?),
            },
            "usageEvent" => OutputEvent::Usage(Usage {
                completion_id: optional(body, "completionId"),
                delta: token_usage(body.get("details").and_then(|details| details.get("delta"))),
                total: token_usage(body.get("details").and_then(|details| details.get("total"))),
                total_input_tokens: count(body.get("totalInputTokens")),
                total_output_tokens: count(body.get("totalOutputTokens")),
                total_tokens: count(body.get("totalTokens")),
            }),
            _ => OutputEvent::Unknown {
                name: name.clone(),
                body: body.clone(),
            },
        };
        Ok(event)
    }
}

impl ContentKind {
    fn parse(name: &str) -> ContentKind {
        match name {
            "TEXT" => ContentKind::Text,
            "AUDIO" => ContentKind::Audio,
            "TOOL" => ContentKind::Tool,
            other => ContentKind::Other(other.to_owned()),
        }
    }
}

impl GenerationStage {
    fn parse(name: &str) -> GenerationStage {
        match name {
            "SPECULATIVE" => GenerationStage::Speculative,
            "FINAL" => GenerationStage::Final,
            other => GenerationStage::Other(other.to_owned()),
        }
    }
}

impl StopReason {
    fn parse(name: &str) -> StopReason {
        match name {
            "PARTIAL_TURN" => StopReason::PartialTurn,
            "END_TURN" => StopReason::EndTurn,
            "INTERRUPTED" => StopReason::Interrupted,
            "TOOL_USE" => StopReason::ToolUse,
            other => StopReason::Other(other.to_owned()),
        }
    }
}

/// Whether a text output's content is the barge-in notice: a JSON object whose `interrupted` is
/// `true`, whatever its whitespace.
fn is_interruption_notice(content: &str) -> bool {
    let content = content.trim();
    content.starts_with('{')
        && serde_json::from_str::<Value>(content)
            .is_ok_and(|notice| notice.get("interrupted") == Some(&Value::Bool(true)))
}

fn missing(event: &str, field: &str) -> NovaSonicError {
    NovaSonicError::Decode(format!("`{event}` has no string `{field}`"))
}

fn required(event: &str, body: &Value, field: &str) -> Result<String, NovaSonicError> {
    optional(body, field).ok_or_else(|| missing(event, field))
}

fn optional(body: &Value, field: &str) -> Option<String> {
    body.get(field).and_then(Value::as_str).map(str::to_owned)
}

fn ids(body: &Value) -> CompletionIds {
    CompletionIds {
        session_id: optional(body, "sessionId").unwrap_or_default(),
        prompt_name: optional(body, "promptName").unwrap_or_default(),
        completion_id: optional(body, "completionId").unwrap_or_default(),
    }
}

/// The `generationStage` inside `additionalModelFields`, which the service sends as a JSON string.
fn stage(body: &Value) -> Option<GenerationStage> {
    let fields = body.get("additionalModelFields")?;
    let fields = match fields {
        Value::String(text) => serde_json::from_str::<Value>(text).ok()?,
        other => other.clone(),
    };
    fields
        .get("generationStage")
        .and_then(Value::as_str)
        .map(GenerationStage::parse)
}

fn count(value: Option<&Value>) -> u64 {
    value.and_then(Value::as_u64).unwrap_or(0)
}

fn token_counts(value: Option<&Value>) -> TokenCounts {
    TokenCounts {
        speech: count(value.and_then(|counts| counts.get("speechTokens"))),
        text: count(value.and_then(|counts| counts.get("textTokens"))),
    }
}

fn token_usage(value: Option<&Value>) -> TokenUsage {
    TokenUsage {
        input: token_counts(value.and_then(|usage| usage.get("input"))),
        output: token_counts(value.and_then(|usage| usage.get("output"))),
    }
}
