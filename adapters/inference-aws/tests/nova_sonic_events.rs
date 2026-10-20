//! Encoding of the Nova Sonic input events, offline.
//!
//! Every expected shape is written from "Handling input events with the bidirectional API" of the
//! Amazon Nova 2 user guide,
//! <https://docs.aws.amazon.com/nova/latest/nova2-userguide/sonic-input-events.html>; the section of
//! that page each case was read from is named on the case.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use inference_aws::nova_sonic::{
    AudioRate, Endpointing, InferenceConfiguration, InputContent, InputEvent, Role, ToolSpec,
};
use serde_json::{Value, json};

fn encoded(event: &InputEvent) -> Value {
    serde_json::from_slice(&event.encode()).expect("an input event encodes to JSON")
}

/// Section "1. RequestStartEvent (Session Start)", its example.
#[test]
fn session_start_carries_inference_and_turn_detection() {
    let event = InputEvent::SessionStart {
        inference: InferenceConfiguration {
            max_tokens: 2048,
            top_p: 0.9,
            temperature: 0.7,
        },
        endpointing: Some(Endpointing::Medium),
    };
    assert_eq!(
        encoded(&event),
        json!({
            "event": {
                "sessionStart": {
                    "inferenceConfiguration": {
                        "maxTokens": 2048,
                        "topP": 0.9,
                        "temperature": 0.7
                    },
                    "turnDetectionConfiguration": {
                        "endpointingSensitivity": "MEDIUM"
                    }
                }
            }
        })
    );
}

#[test]
fn session_start_without_turn_detection_omits_it() {
    let event = InputEvent::SessionStart {
        inference: InferenceConfiguration {
            max_tokens: 1024,
            top_p: 0.9,
            temperature: 0.7,
        },
        endpointing: None,
    };
    let value = encoded(&event);
    assert!(
        value["event"]["sessionStart"]
            .get("turnDetectionConfiguration")
            .is_none(),
        "{value}"
    );
}

/// Section "2. PromptStartEvent".
#[test]
fn prompt_start_carries_voice_audio_format_and_tools() {
    let event = InputEvent::PromptStart {
        prompt_name: "prompt-1".into(),
        voice_id: "matthew".into(),
        output_rate: AudioRate::Hz24000,
        tools: vec![ToolSpec {
            name: "get_weather".into(),
            description: "Current weather of a city".into(),
            input_schema: json!({"type": "object", "properties": {"city": {"type": "string"}}}),
        }],
    };
    let value = encoded(&event);
    let schema = value["event"]["promptStart"]["toolConfiguration"]["tools"][0]["toolSpec"]
        ["inputSchema"]["json"]
        .as_str()
        .expect("the tool input schema is a JSON string");
    assert_eq!(
        serde_json::from_str::<Value>(schema).unwrap(),
        json!({"type": "object", "properties": {"city": {"type": "string"}}})
    );
    assert_eq!(
        value,
        json!({
            "event": {
                "promptStart": {
                    "promptName": "prompt-1",
                    "textOutputConfiguration": {"mediaType": "text/plain"},
                    "audioOutputConfiguration": {
                        "mediaType": "audio/lpcm",
                        "sampleRateHertz": 24000,
                        "sampleSizeBits": 16,
                        "channelCount": 1,
                        "voiceId": "matthew",
                        "encoding": "base64",
                        "audioType": "SPEECH"
                    },
                    "toolUseOutputConfiguration": {"mediaType": "application/json"},
                    "toolConfiguration": {
                        "tools": [{
                            "toolSpec": {
                                "name": "get_weather",
                                "description": "Current weather of a city",
                                "inputSchema": {"json": schema}
                            }
                        }]
                    }
                }
            }
        })
    );
}

#[test]
fn prompt_start_without_tools_omits_tool_configuration() {
    let event = InputEvent::PromptStart {
        prompt_name: "prompt-1".into(),
        voice_id: "tiffany".into(),
        output_rate: AudioRate::Hz8000,
        tools: Vec::new(),
    };
    let value = encoded(&event);
    let prompt = &value["event"]["promptStart"];
    assert!(prompt.get("toolConfiguration").is_none(), "{value}");
    assert_eq!(prompt["audioOutputConfiguration"]["sampleRateHertz"], 8000);
    assert_eq!(prompt["audioOutputConfiguration"]["voiceId"], "tiffany");
}

/// Section "3. InputContentStartEvent", "Text", the system prompt example.
#[test]
fn text_content_start_for_the_system_prompt() {
    let event = InputEvent::ContentStart {
        prompt_name: "conv-12345".into(),
        content_name: "system-prompt-1".into(),
        content: InputContent::Text {
            role: Role::System,
            interactive: false,
        },
    };
    assert_eq!(
        encoded(&event),
        json!({
            "event": {
                "contentStart": {
                    "promptName": "conv-12345",
                    "contentName": "system-prompt-1",
                    "type": "TEXT",
                    "interactive": false,
                    "role": "SYSTEM",
                    "textInputConfiguration": {"mediaType": "text/plain"}
                }
            }
        })
    );
}

/// Section "3. InputContentStartEvent", "Audio".
#[test]
fn audio_content_start_declares_the_input_format() {
    let event = InputEvent::ContentStart {
        prompt_name: "p".into(),
        content_name: "audio-1".into(),
        content: InputContent::Audio {
            rate: AudioRate::Hz8000,
        },
    };
    assert_eq!(
        encoded(&event),
        json!({
            "event": {
                "contentStart": {
                    "promptName": "p",
                    "contentName": "audio-1",
                    "type": "AUDIO",
                    "interactive": true,
                    "role": "USER",
                    "audioInputConfiguration": {
                        "mediaType": "audio/lpcm",
                        "sampleRateHertz": 8000,
                        "sampleSizeBits": 16,
                        "channelCount": 1,
                        "audioType": "SPEECH",
                        "encoding": "base64"
                    }
                }
            }
        })
    );
}

/// Section "3. InputContentStartEvent", "Tool".
#[test]
fn tool_content_start_names_the_tool_use() {
    let event = InputEvent::ContentStart {
        prompt_name: "p".into(),
        content_name: "tool-result-1".into(),
        content: InputContent::ToolResult {
            tool_use_id: "use-7".into(),
        },
    };
    assert_eq!(
        encoded(&event),
        json!({
            "event": {
                "contentStart": {
                    "promptName": "p",
                    "contentName": "tool-result-1",
                    "interactive": false,
                    "type": "TOOL",
                    "role": "TOOL",
                    "toolResultInputConfiguration": {
                        "toolUseId": "use-7",
                        "type": "TEXT",
                        "textInputConfiguration": {"mediaType": "text/plain"}
                    }
                }
            }
        })
    );
}

/// Sections "4. TextInputContent", "6. ToolResultContentEvent", "7. InputContentEndEvent",
/// "8. PromptEndEvent" and "9. RequestEndEvent".
#[test]
fn text_tool_result_and_end_events() {
    assert_eq!(
        encoded(&InputEvent::TextInput {
            prompt_name: "p".into(),
            content_name: "c".into(),
            content: "You are brief.".into(),
        }),
        json!({"event": {"textInput": {"promptName": "p", "contentName": "c", "content": "You are brief."}}})
    );
    assert_eq!(
        encoded(&InputEvent::ToolResult {
            prompt_name: "p".into(),
            content_name: "c".into(),
            content: "{\"key\": \"value\"}".into(),
        }),
        json!({"event": {"toolResult": {"promptName": "p", "contentName": "c", "content": "{\"key\": \"value\"}"}}})
    );
    assert_eq!(
        encoded(&InputEvent::ContentEnd {
            prompt_name: "p".into(),
            content_name: "c".into(),
        }),
        json!({"event": {"contentEnd": {"promptName": "p", "contentName": "c"}}})
    );
    assert_eq!(
        encoded(&InputEvent::PromptEnd {
            prompt_name: "p".into()
        }),
        json!({"event": {"promptEnd": {"promptName": "p"}}})
    );
    assert_eq!(
        encoded(&InputEvent::SessionEnd),
        json!({"event": {"sessionEnd": {}}})
    );
}

/// Section "5. AudioInputContent": the content is base64 of the PCM16 bytes.
#[test]
fn audio_input_is_base64_pcm() {
    let pcm: Vec<u8> = (0u8..=255).collect();
    let value = encoded(&InputEvent::AudioInput {
        prompt_name: "p".into(),
        content_name: "a".into(),
        pcm: pcm.clone(),
    });
    assert_eq!(value["event"]["audioInput"]["promptName"], "p");
    assert_eq!(value["event"]["audioInput"]["contentName"], "a");
    let content = value["event"]["audioInput"]["content"]
        .as_str()
        .expect("audio content is a string");
    assert_eq!(STANDARD.decode(content).unwrap(), pcm);
}

/// The reference lists 8000, 16000 and 24000 Hz for both `audioInputConfiguration` and
/// `audioOutputConfiguration`: 8 kHz telephony audio is accepted as it is.
#[test]
fn audio_rates_are_the_three_the_reference_lists() {
    assert_eq!(AudioRate::try_from(8000), Ok(AudioRate::Hz8000));
    assert_eq!(AudioRate::try_from(16000), Ok(AudioRate::Hz16000));
    assert_eq!(AudioRate::try_from(24000), Ok(AudioRate::Hz24000));
    assert!(AudioRate::try_from(44100).is_err());
    assert!(AudioRate::try_from(22050).is_err());
    assert_eq!(AudioRate::Hz8000.hertz(), 8000);
    assert_eq!(AudioRate::Hz24000.hertz(), 24000);
}
