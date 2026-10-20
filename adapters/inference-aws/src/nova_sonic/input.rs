//! The input events a client sends, from "Handling input events with the bidirectional API".

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

use super::Role;
use super::config::{AudioRate, Endpointing, InferenceConfiguration, ToolSpec};

/// What a content block carries, the `type` of its content start.
#[derive(Debug, Clone, PartialEq)]
pub enum InputContent {
    /// `TEXT`: a system prompt, history or cross-modal text.
    Text {
        /// Whose text it is.
        role: Role,
        /// `true` for cross-modal input during a voice session, `false` for prompts and history.
        interactive: bool,
    },
    /// `AUDIO`: the user's PCM16 mono audio at `rate`.
    Audio {
        /// The rate of the audio that follows.
        rate: AudioRate,
    },
    /// `TOOL`: the result of the tool use `tool_use_id`.
    ToolResult {
        /// The `toolUseId` of the model's `toolUse`.
        tool_use_id: String,
    },
}

/// One input event, encoded as `{"event": {"<name>": {…}}}`.
#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    /// `sessionStart`.
    SessionStart {
        /// `inferenceConfiguration`.
        inference: InferenceConfiguration,
        /// `turnDetectionConfiguration`, omitted when `None`.
        endpointing: Option<Endpointing>,
    },
    /// `promptStart`: output formats, voice and tools.
    PromptStart {
        /// The prompt's name, repeated by every later event.
        prompt_name: String,
        /// `voiceId`.
        voice_id: String,
        /// The `sampleRateHertz` of the audio output.
        output_rate: AudioRate,
        /// `toolConfiguration.tools`, omitted when empty.
        tools: Vec<ToolSpec>,
    },
    /// `contentStart`.
    ContentStart {
        /// The prompt's name.
        prompt_name: String,
        /// The content block's name, repeated by its content and its end.
        content_name: String,
        /// What the block carries.
        content: InputContent,
    },
    /// `textInput`.
    TextInput {
        /// The prompt's name.
        prompt_name: String,
        /// The text content block's name.
        content_name: String,
        /// The text.
        content: String,
    },
    /// `audioInput`: PCM16 bytes, sent as base64.
    AudioInput {
        /// The prompt's name.
        prompt_name: String,
        /// The audio content block's name.
        content_name: String,
        /// Little-endian 16-bit mono samples at the block's rate.
        pcm: Vec<u8>,
    },
    /// `toolResult`.
    ToolResult {
        /// The prompt's name.
        prompt_name: String,
        /// The tool result content block's name.
        content_name: String,
        /// The result, a JSON object as a string.
        content: String,
    },
    /// `contentEnd`.
    ContentEnd {
        /// The prompt's name.
        prompt_name: String,
        /// The content block it closes.
        content_name: String,
    },
    /// `promptEnd`.
    PromptEnd {
        /// The prompt it closes.
        prompt_name: String,
    },
    /// `sessionEnd`.
    SessionEnd,
}

impl InputEvent {
    /// The event as the JSON value the service reads.
    pub fn to_json(&self) -> Value {
        let (name, body) = match self {
            InputEvent::SessionStart {
                inference,
                endpointing,
            } => {
                let mut body = json!({
                    "inferenceConfiguration": {
                        "maxTokens": inference.max_tokens,
                        "topP": inference.top_p,
                        "temperature": inference.temperature
                    }
                });
                if let Some(endpointing) = endpointing {
                    body["turnDetectionConfiguration"] =
                        json!({"endpointingSensitivity": endpointing.as_str()});
                }
                ("sessionStart", body)
            }
            InputEvent::PromptStart {
                prompt_name,
                voice_id,
                output_rate,
                tools,
            } => {
                let mut body = json!({
                    "promptName": prompt_name,
                    "textOutputConfiguration": {"mediaType": "text/plain"},
                    "audioOutputConfiguration": {
                        "mediaType": "audio/lpcm",
                        "sampleRateHertz": output_rate.hertz(),
                        "sampleSizeBits": 16,
                        "channelCount": 1,
                        "voiceId": voice_id,
                        "encoding": "base64",
                        "audioType": "SPEECH"
                    },
                    "toolUseOutputConfiguration": {"mediaType": "application/json"}
                });
                if !tools.is_empty() {
                    let tools: Vec<Value> = tools
                        .iter()
                        .map(|tool| {
                            json!({
                                "toolSpec": {
                                    "name": tool.name,
                                    "description": tool.description,
                                    "inputSchema": {"json": tool.input_schema.to_string()}
                                }
                            })
                        })
                        .collect();
                    body["toolConfiguration"] = json!({ "tools": tools });
                }
                ("promptStart", body)
            }
            InputEvent::ContentStart {
                prompt_name,
                content_name,
                content,
            } => {
                let body = match content {
                    InputContent::Text { role, interactive } => json!({
                        "promptName": prompt_name,
                        "contentName": content_name,
                        "type": "TEXT",
                        "interactive": interactive,
                        "role": role.as_str(),
                        "textInputConfiguration": {"mediaType": "text/plain"}
                    }),
                    InputContent::Audio { rate } => json!({
                        "promptName": prompt_name,
                        "contentName": content_name,
                        "type": "AUDIO",
                        "interactive": true,
                        "role": "USER",
                        "audioInputConfiguration": {
                            "mediaType": "audio/lpcm",
                            "sampleRateHertz": rate.hertz(),
                            "sampleSizeBits": 16,
                            "channelCount": 1,
                            "audioType": "SPEECH",
                            "encoding": "base64"
                        }
                    }),
                    InputContent::ToolResult { tool_use_id } => json!({
                        "promptName": prompt_name,
                        "contentName": content_name,
                        "interactive": false,
                        "type": "TOOL",
                        "role": "TOOL",
                        "toolResultInputConfiguration": {
                            "toolUseId": tool_use_id,
                            "type": "TEXT",
                            "textInputConfiguration": {"mediaType": "text/plain"}
                        }
                    }),
                };
                ("contentStart", body)
            }
            InputEvent::TextInput {
                prompt_name,
                content_name,
                content,
            } => (
                "textInput",
                json!({"promptName": prompt_name, "contentName": content_name, "content": content}),
            ),
            InputEvent::AudioInput {
                prompt_name,
                content_name,
                pcm,
            } => (
                "audioInput",
                json!({
                    "promptName": prompt_name,
                    "contentName": content_name,
                    "content": STANDARD.encode(pcm)
                }),
            ),
            InputEvent::ToolResult {
                prompt_name,
                content_name,
                content,
            } => (
                "toolResult",
                json!({"promptName": prompt_name, "contentName": content_name, "content": content}),
            ),
            InputEvent::ContentEnd {
                prompt_name,
                content_name,
            } => (
                "contentEnd",
                json!({"promptName": prompt_name, "contentName": content_name}),
            ),
            InputEvent::PromptEnd { prompt_name } => {
                ("promptEnd", json!({"promptName": prompt_name}))
            }
            InputEvent::SessionEnd => ("sessionEnd", json!({})),
        };
        json!({"event": {name: body}})
    }

    /// The event as the bytes of one `chunk` of the bidirectional stream.
    pub fn encode(&self) -> Vec<u8> {
        self.to_json().to_string().into_bytes()
    }
}
