//! Adversary pass 1 on Nova Sonic's usage records: input usage the provider reported before a
//! response began, when the session ends before one does, and a response whose late output
//! usage is still unsettled when the service fails the session.
#![cfg(feature = "duplex")]

#[allow(dead_code)]
mod replay;

use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use inference_aws::duplex::NovaSonicSpeech;
use inference_aws::nova_sonic::{ServiceError, ServiceErrorKind};
use inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_duplex::{
    NativeSpeechModel, Quantity, QuantityKind, SessionConfig, SessionEvent, SpeechSessionHandle,
    TurnDetection, TurnTuning, UsageRecorded,
};
use replay::{Faults, Replay, turn};

const WAIT: Duration = Duration::from_millis(100);

fn pcm() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).expect("a positive rate"),
        channels: NonZeroU16::MIN,
    }
}

fn config() -> SessionConfig {
    SessionConfig {
        voice: "tiffany".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: pcm(),
        output_format: pcm(),
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools: Vec::new(),
        idle_limit: Duration::from_secs(5),
    }
}

fn speak(session: &mut impl SpeechSessionHandle, frames: u64) {
    for index in 0..=frames {
        let byte = if index < frames { 9 } else { 0 };
        session
            .append_audio(AudioFrame {
                format: pcm(),
                position: SampleClockPosition::new(index * 320).expect("a position"),
                sample_count: 320,
                data: vec![byte; 640],
            })
            .expect("input in the session's format");
    }
}

fn drain(session: &mut impl SpeechSessionHandle) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    while let Some(event) = session.next_event(WAIT) {
        events.push(event);
        assert!(events.len() < 500, "the session never went quiet");
    }
    events
}

fn records(events: &[SessionEvent]) -> Vec<&UsageRecorded> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::UsageRecorded(recorded) => Some(recorded),
            _ => None,
        })
        .collect()
}

/// The replay answers a user turn with `completionStart`, the user's final transcript (three
/// events), then a usage event of 50 input speech tokens: five events. Stalled there, the user's
/// turn is committed and its input reported, and no response has begun when the caller closes
/// (the user said goodbye and hung up). Nova Sonic billed those 50 tokens; the session's usage
/// records must carry them somewhere.
#[test]
fn input_usage_reported_before_any_response_survives_the_sessions_end() {
    let mut replay = Replay::new(vec![turn("goodbye", "Goodbye")]);
    replay.faults = Faults {
        stall_after: Some(5),
        ..Faults::default()
    };
    let mut session = NovaSonicSpeech::with_connector(replay.clone())
        .expect("the binding's runtime starts")
        .open(config())
        .expect("opens");
    speak(&mut session, 2);
    let mut events = drain(&mut session);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::InputCommitted(_))),
        "{events:#?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::ResponseCreated(_))),
        "{events:#?}"
    );
    session.close().expect("open");
    events.extend(drain(&mut session));
    let reported: i64 = session
        .ledger()
        .usage_records()
        .iter()
        .flat_map(|record| record.quantities())
        .filter(|quantity| quantity.kind() == QuantityKind::InputAudioTokens)
        .filter_map(Quantity::value)
        .sum();
    assert_eq!(
        reported,
        50,
        "the input usage Nova Sonic reported is in no usage record: {:#?}",
        session.ledger().usage_records()
    );
}

/// The control of the case above: one event later (the speculative text's `contentStart`, which
/// begins the response) the same 50 tokens are in the response's record when the caller
/// closes. So the report arrived, and only the missing response loses it.
#[test]
fn the_same_input_usage_is_recorded_once_a_response_began() {
    let mut replay = Replay::new(vec![turn("goodbye", "Goodbye")]);
    replay.faults = Faults {
        stall_after: Some(6),
        ..Faults::default()
    };
    let mut session = NovaSonicSpeech::with_connector(replay.clone())
        .expect("the binding's runtime starts")
        .open(config())
        .expect("opens");
    speak(&mut session, 2);
    let _ = drain(&mut session);
    session.close().expect("open");
    let _ = drain(&mut session);
    let records = session.ledger().usage_records();
    assert_eq!(records.len(), 1, "{records:#?}");
    assert!(
        records[0]
            .quantities()
            .contains(&Quantity::provider_reported(
                QuantityKind::InputAudioTokens,
                50
            )),
        "{records:#?}"
    );
}

/// One turn: the response completes at its final text's `END_TURN` (event 16) and the service
/// fails the session before the usage event of its 8 output text tokens. The response is
/// recorded once, with the sums reported so far, before `SpeechSessionFailed`.
#[test]
fn a_service_failure_records_the_unsettled_response_before_the_sessions_failure() {
    let mut replay = Replay::new(vec![turn("hi", "Hello")]);
    replay.faults = Faults {
        error_after: Some((
            16,
            ServiceError::new(ServiceErrorKind::ModelStreamError, "it failed"),
        )),
        ..Faults::default()
    };
    let mut session = NovaSonicSpeech::with_connector(replay.clone())
        .expect("the binding's runtime starts")
        .open(config())
        .expect("opens");
    speak(&mut session, 2);
    let events = drain(&mut session);
    let completed = events
        .iter()
        .position(|event| matches!(event, SessionEvent::ResponseCompleted(_)))
        .expect("the response completes before the failure");
    let failed = events
        .iter()
        .position(|event| matches!(event, SessionEvent::SpeechSessionFailed(_)))
        .expect("the session fails");
    let recorded = records(&events);
    assert_eq!(recorded.len(), 1, "{events:#?}");
    assert!(
        matches!(&events[failed - 1], SessionEvent::UsageRecorded(_)),
        "{events:#?}"
    );
    assert!(completed < failed - 1);
    assert_eq!(
        recorded[0].quantities,
        vec![
            Quantity::provider_reported(QuantityKind::InputTextTokens, 0),
            Quantity::provider_reported(QuantityKind::InputAudioTokens, 50),
            Quantity::unknown(QuantityKind::CachedInputTokens),
            Quantity::provider_reported(QuantityKind::OutputTextTokens, 0),
            Quantity::provider_reported(QuantityKind::OutputAudioTokens, 30),
            Quantity::measured(QuantityKind::Requests, 1),
        ]
    );
}
