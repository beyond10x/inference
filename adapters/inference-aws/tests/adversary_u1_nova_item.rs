//! Adversary pass 1 on `story:duplex-turn-ids`: Nova Sonic names a user turn by the `contentId`
//! of its user TEXT block (the binding's module doc). The binding's own test asserts only that
//! the commit and the transcript carry one id, which holds as well for an id the ledger assigns
//! to both; here the id is the block's `contentId` the replay issued (`text-N`), not a `turn-N`.
#![cfg(feature = "duplex")]

#[allow(dead_code)]
mod replay;

use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use inference_aws::duplex::NovaSonicSpeech;
use inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_duplex::inference_tools::{DeclareTool, SideEffectClass, Tool};
use inference_duplex::{
    NativeSpeechModel, SessionConfig, SessionEvent, SpeechSessionHandle, TurnDetection, TurnTuning,
};
use replay::{Replay, turn};
use serde_json::json;

const WAIT: Duration = Duration::from_millis(100);

fn pcm(sample_rate: u32) -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(sample_rate).expect("a positive rate"),
        channels: NonZeroU16::MIN,
    }
}

fn frame(format: AudioFormat, position: u64, byte: u8) -> AudioFrame {
    let samples = format.sample_rate.get() / 50;
    AudioFrame {
        format,
        position: SampleClockPosition::new(position).expect("a position"),
        sample_count: samples,
        data: vec![byte; usize::try_from(format.data_len(samples)).expect("a length")],
    }
}

fn weather() -> Tool {
    Tool::declare(DeclareTool {
        name: "weather".to_owned(),
        description: String::new(),
        schema: json!({"type": "object", "properties": {"city": {"type": "string"}}}),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0
}

fn config() -> SessionConfig {
    SessionConfig {
        voice: "tiffany".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: pcm(16_000),
        output_format: pcm(16_000),
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools: vec![weather()],
        idle_limit: Duration::from_secs(5),
    }
}

#[test]
fn a_user_turn_is_named_by_its_text_blocks_content_id() {
    let replay = Replay::new(vec![turn("what time is it", "It is noon")]);
    let mut session = NovaSonicSpeech::with_connector(replay.clone())
        .expect("the binding's runtime starts")
        .open(config())
        .expect("the model takes the configuration");
    for index in 0..=3u64 {
        let byte = if index < 3 { 9 } else { 0 };
        session
            .append_audio(frame(pcm(16_000), index * 320, byte))
            .expect("input in the session's format");
    }
    let mut events = Vec::new();
    while let Some(event) = session.next_event(WAIT) {
        events.push(event);
        assert!(events.len() < 500, "the session never went quiet");
    }
    let items: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::InputCommitted(committed) => Some(committed.item.to_string()),
            SessionEvent::InputTranscribed(heard) => Some(heard.item.to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(items.len(), 2, "one commit and one transcript: {events:#?}");
    assert_eq!(items[0], items[1]);
    assert!(
        items[0].starts_with("text-"),
        "the turn is named by the user TEXT block's contentId, which the replay issues as \
         `text-N`, not by an id the ledger assigns: {items:?}"
    );
}
