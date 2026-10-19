//! Adversary pass 2: the liveness bound after a tool result, which Nova Sonic answers in a
//! response of its own (`capabilities().manual_response` is false), so the caller awaits it.
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
    NativeSpeechModel, SessionConfig, SessionEvent, SessionFailureKind, SpeechSessionHandle,
    TurnDetection, TurnTuning,
};
use replay::{Faults, Replay, Reply, Turn};
use serde_json::json;

fn pcm16k() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).expect("a positive rate"),
        channels: NonZeroU16::MIN,
    }
}

fn frame(position: u64, byte: u8) -> AudioFrame {
    let format = pcm16k();
    AudioFrame {
        format,
        position: SampleClockPosition::new(position).expect("a position"),
        sample_count: 320,
        data: vec![byte; usize::try_from(format.data_len(320)).expect("a length")],
    }
}

fn weather() -> Tool {
    Tool::declare(DeclareTool {
        name: "weather".to_owned(),
        schema: json!({"type": "object"}),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0
}

fn config(idle_limit: Duration) -> SessionConfig {
    SessionConfig {
        voice: "tiffany".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: pcm16k(),
        output_format: pcm16k(),
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools: vec![weather()],
        idle_limit,
    }
}

/// The replay sends the user transcript, a usage event and the tool block (eight events with the
/// completion start), takes the tool result, and then sends nothing more: a provider that never
/// answers the result. The caller awaits that answer, as it awaits the response a commit owes, so
/// the session fails as stalled at the bound, not stays open in silence for ever.
#[test]
fn a_provider_silent_after_a_tool_result_fails_the_session_as_stalled() {
    let mut replay = Replay::new(vec![Turn {
        heard: "how warm is it".to_owned(),
        reply: Reply::Call {
            tool: "weather".to_owned(),
            arguments: json!({}),
            then: "It is {result}".to_owned(),
        },
    }]);
    replay.faults = Faults {
        stall_after: Some(8),
        ..Faults::default()
    };
    let mut session = NovaSonicSpeech::with_connector(replay.clone())
        .expect("the binding's runtime starts")
        .open(config(Duration::from_millis(300)))
        .expect("opens");
    for (index, byte) in [9, 9, 0].into_iter().enumerate() {
        session
            .append_audio(frame(index as u64 * 320, byte))
            .expect("accepted");
    }
    let mut call = None;
    for _ in 0..40 {
        if let Some(SessionEvent::SpeechToolCallProposed(proposed)) =
            session.next_event(Duration::from_millis(50))
        {
            call = Some(proposed.call_id);
            break;
        }
    }
    let call = call.expect("the replay proposed the call");
    session
        .submit_tool_result(&call, json!({"celsius": 21}))
        .expect("the call awaits its result");

    let mut seen = Vec::new();
    let mut failure = None;
    for _ in 0..60 {
        match session.next_event(Duration::from_millis(50)) {
            Some(SessionEvent::SpeechSessionFailed(failed)) => {
                failure = Some(failed.failure);
                break;
            }
            Some(event) => seen.push(event),
            None => {}
        }
    }
    assert!(
        matches!(&failure, Some(f) if f.kind == SessionFailureKind::Stalled),
        "three seconds after the tool result, ten bounds of 300 ms, the session is {:?} and \
         nothing failed it; events after the result: {seen:#?}",
        session.ledger().session().state()
    );
}
