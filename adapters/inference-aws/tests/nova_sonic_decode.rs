//! Decoding of the Nova Sonic output events, offline.
//!
//! Every input shape is written from "Handling output events with the bidirectional API" of the
//! Amazon Nova 2 user guide,
//! <https://docs.aws.amazon.com/nova/latest/nova2-userguide/sonic-output-events.html>, with the
//! placeholders of that page filled in; the section each case was read from is named on the case.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use inference_aws::nova_sonic::{
    ContentKind, Decoder, GenerationStage, NovaSonicError, OutputEvent, Role, StopReason,
    TokenCounts,
};
use serde_json::{Value, json};

fn decode(decoder: &mut Decoder, value: Value) -> OutputEvent {
    decoder
        .decode(value.to_string().as_bytes())
        .expect("a reference shape decodes")
}

fn ids() -> Value {
    json!({"sessionId": "s-1", "promptName": "p-1", "completionId": "c-1"})
}

fn with_ids(mut body: Value) -> Value {
    for (key, value) in ids().as_object().unwrap() {
        body[key] = value.clone();
    }
    body
}

fn text_start(content_id: &str, role: &str, stage: &str) -> Value {
    json!({"event": {"contentStart": with_ids(json!({
        "additionalModelFields": format!("{{\"generationStage\":\"{stage}\"}}"),
        "contentId": content_id,
        "type": "TEXT",
        "role": role,
        "textOutputConfiguration": {"mediaType": "text/plain"}
    }))}})
}

fn text_output(content_id: &str, content: &str) -> Value {
    json!({"event": {"textOutput": with_ids(json!({"contentId": content_id, "content": content}))}})
}

/// Section "2. CompleteStartEvent".
#[test]
fn completion_start_carries_the_identifiers() {
    let mut decoder = Decoder::default();
    let event = decode(&mut decoder, json!({"event": {"completionStart": ids()}}));
    let OutputEvent::CompletionStart(ids) = event else {
        panic!("expected completion start, got {event:?}");
    };
    assert_eq!(ids.session_id, "s-1");
    assert_eq!(ids.prompt_name, "p-1");
    assert_eq!(ids.completion_id, "c-1");
}

/// Section "3. TextOutputContent" with "ASR Transcription" (role USER, stage FINAL) and "Text
/// Response" (role ASSISTANT, stage SPECULATIVE): a text output carries the role and stage of its
/// content start.
#[test]
fn text_output_takes_role_and_stage_from_its_content_start() {
    let mut decoder = Decoder::default();
    let start = decode(&mut decoder, text_start("t-user", "USER", "FINAL"));
    let OutputEvent::ContentStart(start) = start else {
        panic!("expected content start, got {start:?}");
    };
    assert_eq!(start.content_id, "t-user");
    assert_eq!(start.kind, ContentKind::Text);
    assert_eq!(start.role, Role::User);
    assert_eq!(start.stage, Some(GenerationStage::Final));

    decode(
        &mut decoder,
        text_start("t-asst", "ASSISTANT", "SPECULATIVE"),
    );

    let user = decode(
        &mut decoder,
        text_output("t-user", "what is the capital of france"),
    );
    let OutputEvent::TextOutput(user) = user else {
        panic!("expected text output, got {user:?}");
    };
    assert_eq!(user.role, Some(Role::User));
    assert_eq!(user.stage, Some(GenerationStage::Final));
    assert_eq!(user.content, "what is the capital of france");
    assert_eq!(user.content_id, "t-user");
    assert_eq!(user.completion_id.as_deref(), Some("c-1"));

    let assistant = decode(&mut decoder, text_output("t-asst", "Paris."));
    let OutputEvent::TextOutput(assistant) = assistant else {
        panic!("expected text output, got {assistant:?}");
    };
    assert_eq!(assistant.role, Some(Role::Assistant));
    assert_eq!(assistant.stage, Some(GenerationStage::Speculative));
    assert_eq!(assistant.content, "Paris.");
}

#[test]
fn text_output_without_a_known_content_start_has_no_role() {
    let mut decoder = Decoder::default();
    let event = decode(&mut decoder, text_output("never-started", "hello"));
    let OutputEvent::TextOutput(text) = event else {
        panic!("expected text output, got {event:?}");
    };
    assert_eq!(text.role, None);
    assert_eq!(text.stage, None);
}

/// Section "3. TextOutputContent", "ContentEnd", and "Barge-In Support": `INTERRUPTED`.
#[test]
fn text_content_end_carries_the_stop_reason() {
    let mut decoder = Decoder::default();
    for (reason, expected) in [
        ("PARTIAL_TURN", StopReason::PartialTurn),
        ("END_TURN", StopReason::EndTurn),
        ("INTERRUPTED", StopReason::Interrupted),
    ] {
        let event = decode(
            &mut decoder,
            json!({"event": {"contentEnd": with_ids(json!({
                "contentId": "t-1", "stopReason": reason, "type": "TEXT"
            }))}}),
        );
        let OutputEvent::ContentEnd(end) = event else {
            panic!("expected content end, got {event:?}");
        };
        assert_eq!(end.content_id, "t-1");
        assert_eq!(end.kind, ContentKind::Text);
        assert_eq!(end.stop_reason, Some(expected));
    }
}

/// Section "4. ToolUse": content start, tool use and content end.
#[test]
fn tool_use_is_typed() {
    let mut decoder = Decoder::default();
    let start = decode(
        &mut decoder,
        json!({"event": {"contentStart": with_ids(json!({
            "contentId": "tool-c",
            "type": "TOOL",
            "role": "TOOL",
            "toolUseOutputConfiguration": {"mediaType": "application/json"}
        }))}}),
    );
    let OutputEvent::ContentStart(start) = start else {
        panic!("expected content start, got {start:?}");
    };
    assert_eq!(start.kind, ContentKind::Tool);
    assert_eq!(start.role, Role::Tool);
    assert_eq!(start.stage, None);

    let event = decode(
        &mut decoder,
        json!({"event": {"toolUse": with_ids(json!({
            "contentId": "tool-c",
            "content": "{\"city\":\"Paris\"}",
            "toolName": "get_weather",
            "toolUseId": "use-7"
        }))}}),
    );
    let OutputEvent::ToolUse(tool) = event else {
        panic!("expected tool use, got {event:?}");
    };
    assert_eq!(tool.tool_use_id, "use-7");
    assert_eq!(tool.tool_name, "get_weather");
    assert_eq!(tool.content, "{\"city\":\"Paris\"}");
    assert_eq!(tool.content_id, "tool-c");

    let end = decode(
        &mut decoder,
        json!({"event": {"contentEnd": with_ids(json!({
            "contentId": "tool-c", "stopReason": "TOOL_USE", "type": "TOOL"
        }))}}),
    );
    let OutputEvent::ContentEnd(end) = end else {
        panic!("expected content end, got {end:?}");
    };
    assert_eq!(end.kind, ContentKind::Tool);
    assert_eq!(end.stop_reason, Some(StopReason::ToolUse));
}

/// Section "5. AudioOutputContent": the audio content start declares the rate, the audio output
/// is base64 PCM16 that decodes to bytes.
#[test]
fn audio_output_decodes_to_pcm() {
    let mut decoder = Decoder::default();
    let start = decode(
        &mut decoder,
        json!({"event": {"contentStart": with_ids(json!({
            "contentId": "a-1",
            "type": "AUDIO",
            "role": "ASSISTANT",
            "audioOutputConfiguration": {
                "mediaType": "audio/lpcm",
                "sampleRateHertz": 24000,
                "sampleSizeBits": 16,
                "encoding": "base64",
                "channelCount": 1
            }
        }))}}),
    );
    let OutputEvent::ContentStart(start) = start else {
        panic!("expected content start, got {start:?}");
    };
    assert_eq!(start.kind, ContentKind::Audio);
    assert_eq!(start.role, Role::Assistant);
    assert_eq!(start.audio_rate_hertz, Some(24000));

    let pcm: Vec<u8> = vec![0, 1, 2, 3, 254, 255];
    let event = decode(
        &mut decoder,
        json!({"event": {"audioOutput": with_ids(json!({
            "contentId": "a-1", "content": STANDARD.encode(&pcm)
        }))}}),
    );
    let OutputEvent::AudioOutput(audio) = event else {
        panic!("expected audio output, got {event:?}");
    };
    assert_eq!(audio.content_id, "a-1");
    assert_eq!(audio.pcm, pcm);
}

#[test]
fn audio_output_that_is_not_base64_is_a_decode_error() {
    let mut decoder = Decoder::default();
    let result = decoder.decode(
        json!({"event": {"audioOutput": with_ids(json!({"contentId": "a-1", "content": "%%%"}))}})
            .to_string()
            .as_bytes(),
    );
    assert!(
        matches!(result, Err(NovaSonicError::Decode(_))),
        "{result:?}"
    );
}

/// Section "1. UsageEvent".
#[test]
fn usage_is_typed() {
    let mut decoder = Decoder::default();
    let event = decode(
        &mut decoder,
        json!({"event": {"usageEvent": with_ids(json!({
            "details": {
                "delta": {
                    "input": {"speechTokens": 10, "textTokens": 2},
                    "output": {"speechTokens": 30, "textTokens": 4}
                },
                "total": {
                    "input": {"speechTokens": 110, "textTokens": 52},
                    "output": {"speechTokens": 130, "textTokens": 14}
                }
            },
            "totalInputTokens": 162,
            "totalOutputTokens": 144,
            "totalTokens": 306
        }))}}),
    );
    let OutputEvent::Usage(usage) = event else {
        panic!("expected usage, got {event:?}");
    };
    assert_eq!(usage.completion_id.as_deref(), Some("c-1"));
    assert_eq!(
        usage.delta.input,
        TokenCounts {
            speech: Some(10),
            text: Some(2)
        }
    );
    assert_eq!(
        usage.delta.output,
        TokenCounts {
            speech: Some(30),
            text: Some(4)
        }
    );
    assert_eq!(
        usage.total.input,
        TokenCounts {
            speech: Some(110),
            text: Some(52)
        }
    );
    assert_eq!(
        usage.total.output,
        TokenCounts {
            speech: Some(130),
            text: Some(14)
        }
    );
    assert_eq!(usage.total_input_tokens, Some(162));
    assert_eq!(usage.total_output_tokens, Some(144));
    assert_eq!(usage.total_tokens, Some(306));
}

/// Section "1. UsageEvent", with counts left out: a count the event does not carry is absent,
/// never 0, so a meter records it as unknown.
#[test]
fn a_count_the_usage_event_leaves_out_is_absent_not_zero() {
    let mut decoder = Decoder::default();
    let event = decode(
        &mut decoder,
        json!({"event": {"usageEvent": with_ids(json!({
            "details": {
                "delta": {
                    "input": {"speechTokens": 10},
                    "output": {"textTokens": 4}
                },
                "total": {
                    "input": {"speechTokens": 110, "textTokens": 52}
                }
            },
            "totalInputTokens": 162
        }))}}),
    );
    let OutputEvent::Usage(usage) = event else {
        panic!("expected usage, got {event:?}");
    };
    assert_eq!(
        usage.delta.input,
        TokenCounts {
            speech: Some(10),
            text: None
        }
    );
    assert_eq!(
        usage.delta.output,
        TokenCounts {
            speech: None,
            text: Some(4)
        }
    );
    assert_eq!(usage.total.output, TokenCounts::default());
    assert_eq!(usage.total.output.speech, None);
    assert_eq!(usage.total_input_tokens, Some(162));
    assert_eq!(usage.total_output_tokens, None);
    assert_eq!(usage.total_tokens, None);
}

/// Section "6. CompletionEndEvent".
#[test]
fn completion_end_carries_the_stop_reason() {
    let mut decoder = Decoder::default();
    let event = decode(
        &mut decoder,
        json!({"event": {"completionEnd": with_ids(json!({"stopReason": "END_TURN"}))}}),
    );
    let OutputEvent::CompletionEnd { ids, stop_reason } = event else {
        panic!("expected completion end, got {event:?}");
    };
    assert_eq!(ids.completion_id, "c-1");
    assert_eq!(stop_reason, StopReason::EndTurn);
}

#[test]
fn an_unlisted_stop_reason_is_kept() {
    let mut decoder = Decoder::default();
    let event = decode(
        &mut decoder,
        json!({"event": {"completionEnd": with_ids(json!({"stopReason": "SOMETHING_NEW"}))}}),
    );
    let OutputEvent::CompletionEnd { stop_reason, .. } = event else {
        panic!("expected completion end, got {event:?}");
    };
    assert_eq!(stop_reason, StopReason::Other("SOMETHING_NEW".into()));
}

#[test]
fn an_unknown_event_is_kept_as_unknown() {
    let mut decoder = Decoder::default();
    let event = decode(&mut decoder, json!({"event": {"somethingNew": {"a": 1}}}));
    assert_eq!(
        event,
        OutputEvent::Unknown {
            name: "somethingNew".into(),
            body: json!({"a": 1})
        }
    );
}

#[test]
fn malformed_frames_are_decode_errors() {
    let mut decoder = Decoder::default();
    for frame in [
        b"not json".as_slice(),
        b"{}".as_slice(),
        b"{\"event\": 3}".as_slice(),
        b"{\"event\": {\"textOutput\": {\"content\": \"no content id\"}}}".as_slice(),
    ] {
        let result = decoder.decode(frame);
        assert!(
            matches!(result, Err(NovaSonicError::Decode(_))),
            "{} -> {result:?}",
            String::from_utf8_lossy(frame)
        );
    }
}

/// "Barge-In Support": the service tells the client it was interrupted. The notice is a
/// `textOutput` whose content is `{ "interrupted" : true }`, the string the interruption check of
/// the bidirectional streaming sample looks for
/// (<https://docs.aws.amazon.com/nova/latest/userguide/speech-bidirection.html>). It is an
/// interruption, never assistant text, however its whitespace is laid out.
#[test]
fn a_barge_in_notice_is_an_interruption_not_assistant_text() {
    for notice in [
        "{ \"interrupted\" : true }",
        "{\"interrupted\":true}",
        " {\n  \"interrupted\": true\n} ",
    ] {
        let mut decoder = Decoder::default();
        decode(
            &mut decoder,
            text_start("t-asst", "ASSISTANT", "SPECULATIVE"),
        );
        let event = decode(&mut decoder, text_output("t-asst", notice));
        assert_eq!(
            event,
            OutputEvent::Interrupted {
                completion_id: Some("c-1".into()),
                content_id: "t-asst".into(),
            },
            "{notice:?}"
        );
    }
}

/// Text that merely mentions an interruption, or a JSON object that does not say it, stays text.
#[test]
fn text_that_is_not_the_notice_stays_text() {
    for content in [
        "I was interrupted, sorry.",
        "{\"interrupted\": false}",
        "{\"interrupted\": \"true\"}",
        "{\"interrupted\": true, \"also\": 1} and more",
    ] {
        let mut decoder = Decoder::default();
        decode(
            &mut decoder,
            text_start("t-asst", "ASSISTANT", "SPECULATIVE"),
        );
        let event = decode(&mut decoder, text_output("t-asst", content));
        let OutputEvent::TextOutput(text) = event else {
            panic!("expected text for {content:?}, got {event:?}");
        };
        assert_eq!(text.content, content);
        assert_eq!(text.role, Some(Role::Assistant));
    }
}
