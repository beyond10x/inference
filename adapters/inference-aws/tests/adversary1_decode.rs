//! Adversary pass 1: the decoder on interleaved content blocks, a block that ended, a barge-in
//! notification and a large audio chunk.
//!
//! Shapes are those of "Handling output events with the bidirectional API",
//! <https://docs.aws.amazon.com/nova/latest/nova2-userguide/sonic-output-events.html>.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use inference_aws::nova_sonic::{Decoder, GenerationStage, OutputEvent, Role};
use serde_json::{Value, json};

fn decode(decoder: &mut Decoder, value: Value) -> OutputEvent {
    decoder
        .decode(value.to_string().as_bytes())
        .expect("a reference shape decodes")
}

fn text_start(content_id: &str, role: &str, stage: &str) -> Value {
    json!({"event": {"contentStart": {
        "sessionId": "s", "promptName": "p", "completionId": "c",
        "additionalModelFields": format!("{{\"generationStage\":\"{stage}\"}}"),
        "contentId": content_id, "type": "TEXT", "role": role,
        "textOutputConfiguration": {"mediaType": "text/plain"}
    }}})
}

fn text_output(content_id: &str, content: &str) -> Value {
    json!({"event": {"textOutput": {
        "sessionId": "s", "promptName": "p", "completionId": "c",
        "contentId": content_id, "content": content
    }}})
}

fn text_of(event: OutputEvent) -> (Option<Role>, Option<GenerationStage>) {
    let OutputEvent::TextOutput(text) = event else {
        panic!("expected text output, got {event:?}");
    };
    (text.role, text.stage)
}

/// Ending one block leaves an interleaved block's role and stage in place.
#[test]
fn ending_one_block_keeps_the_other() {
    let mut decoder = Decoder::default();
    decode(&mut decoder, text_start("a", "ASSISTANT", "SPECULATIVE"));
    decode(&mut decoder, text_start("u", "USER", "FINAL"));
    decode(
        &mut decoder,
        json!({"event": {"contentEnd": {
            "sessionId": "s", "promptName": "p", "completionId": "c",
            "contentId": "a", "stopReason": "PARTIAL_TURN", "type": "TEXT"
        }}}),
    );
    assert_eq!(
        text_of(decode(&mut decoder, text_output("u", "hi"))),
        (Some(Role::User), Some(GenerationStage::Final))
    );
}

/// `additionalModelFields` given as an object rather than a JSON string still yields the stage.
#[test]
fn a_stage_given_as_an_object_is_read() {
    let mut decoder = Decoder::default();
    let event = decode(
        &mut decoder,
        json!({"event": {"contentStart": {
            "contentId": "a", "type": "TEXT", "role": "ASSISTANT",
            "additionalModelFields": {"generationStage": "FINAL"}
        }}}),
    );
    let OutputEvent::ContentStart(start) = event else {
        panic!("expected content start, got {event:?}");
    };
    assert_eq!(start.stage, Some(GenerationStage::Final));
}

/// A tool use whose `content` arrives as a JSON object is kept as its JSON text.
#[test]
fn a_tool_use_with_object_content_is_kept_as_json_text() {
    let mut decoder = Decoder::default();
    let event = decode(
        &mut decoder,
        json!({"event": {"toolUse": {
            "contentId": "t", "toolName": "lookup", "toolUseId": "u-1",
            "content": {"city": "Paris"}
        }}}),
    );
    let OutputEvent::ToolUse(tool) = event else {
        panic!("expected tool use, got {event:?}");
    };
    assert_eq!(
        serde_json::from_str::<Value>(&tool.content).unwrap(),
        json!({"city": "Paris"})
    );
}

/// A one-megabyte audio chunk decodes whole.
#[test]
fn a_large_audio_chunk_decodes_whole() {
    let pcm: Vec<u8> = (0..1_048_576u32).map(|i| (i % 251) as u8).collect();
    let mut decoder = Decoder::default();
    let event = decode(
        &mut decoder,
        json!({"event": {"audioOutput": {"contentId": "a", "content": STANDARD.encode(&pcm)}}}),
    );
    let OutputEvent::AudioOutput(audio) = event else {
        panic!("expected audio output, got {event:?}");
    };
    assert_eq!(audio.pcm, pcm);
}

/// An event object with no member is a decode error, not a panic.
#[test]
fn an_empty_event_object_is_a_decode_error() {
    let mut decoder = Decoder::default();
    assert!(decoder.decode(b"{\"event\": {}}").is_err());
}
