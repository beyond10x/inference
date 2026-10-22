//! Adversary case, pass 10, story `duplex-late-usage-counted` at b873a7e: the commit says token
//! sums never wrap, becoming unknown where the record can say so. The Realtime transcription
//! usage derives its total from its input and output tokens when the server leaves the total
//! out (`event.rs`, `transcription_usage`), and that total is an `Option<u64>`, so it can say
//! unknown. The addition is unchecked: in a debug build it panics inside
//! `ServerEvent::parse`, and so inside the duplex session's `next_event`; in a release build it
//! wraps. No network and no key.

use inference_openai::{ServerEvent, TranscriptionUsage};
use serde_json::json;

/// A transcription whose input and output tokens sum past `u64::MAX`, without a total: the
/// counts are kept, and the total the server left out is unknown rather than wrapped.
#[test]
fn a_transcription_total_past_u64_max_is_unknown() {
    let text = json!({
        "type": "conversation.item.input_audio_transcription.completed",
        "event_id": "evt_t",
        "item_id": "msg_1",
        "content_index": 0,
        "transcript": "hello",
        "usage": {"type": "tokens", "input_tokens": u64::MAX, "output_tokens": 1}
    })
    .to_string();
    let event = ServerEvent::parse(&text).expect("a readable transcription");
    let ServerEvent::InputTranscript { usage, .. } = event else {
        panic!("not a transcription: {event:?}");
    };
    assert_eq!(
        usage,
        Some(TranscriptionUsage::Tokens {
            total_tokens: None,
            input_tokens: Some(u64::MAX),
            output_tokens: Some(1),
        })
    );
}

/// The companion on `response.done`, whose `Usage::total_tokens` is a `u64` and so cannot say
/// unknown: a total the server left out, its input and output summing past `u64::MAX`, stops at
/// `u64::MAX` (a guard; it passes at b873a7e).
#[test]
fn a_response_total_past_u64_max_stops_there() {
    let text = json!({
        "type": "response.done",
        "event_id": "evt_d",
        "response": {"id": "resp_1", "object": "realtime.response", "status": "completed", "output": [],
            "usage": {"input_tokens": u64::MAX, "output_tokens": 1}}
    })
    .to_string();
    let event = ServerEvent::parse(&text).expect("a readable response.done");
    let ServerEvent::ResponseDone { usage, .. } = event else {
        panic!("not a response.done: {event:?}");
    };
    let usage = usage.expect("its usage");
    assert_eq!(usage.total_tokens, u64::MAX, "{usage:?}");
    assert_eq!((usage.input_tokens, usage.output_tokens), (u64::MAX, 1));
}
