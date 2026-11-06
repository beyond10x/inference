//! A failed recognition counts only the audio the binding forwarded
//! (`story:contract-usage-and-naming-drift`): the frame it refuses, and every frame offered after
//! it, are never taken, so its usage record does not count them.

mod support;

use inference_deepgram::DeepgramError;
use inference_stt::inference_core::usage::QuantityKind;
use inference_stt::{Recogniser, RecognitionHandle, TranscriptEvent};
use support::{FixtureServer, Script, mulaw, request, tone};

#[test]
fn frames_after_a_refused_frame_are_not_counted_as_taken() {
    let server = FixtureServer::start(Script::fixture("conversation.jsonl"));
    let mut recogniser = server.recogniser();
    let mut handle = recogniser.start(request()).expect("a slot is free");
    let forwarded = tone(3);
    let forwarded_samples: u64 = forwarded
        .iter()
        .map(|frame| u64::from(frame.sample_count))
        .sum();
    for frame in forwarded {
        handle.offer_frame(frame).expect("input is open");
    }
    for frame in mulaw(2) {
        handle.offer_frame(frame).expect("input is open");
    }
    for frame in tone(6).into_iter().skip(3) {
        handle.offer_frame(frame).expect("input is open");
    }
    let mut events: Vec<_> = handle.by_ref().collect();
    assert!(
        matches!(handle.failure(), Some(DeepgramError::UnsupportedAudio(_))),
        "{:?}",
        handle.failure()
    );
    handle.end_input().expect("input ends once");
    events.extend(handle.by_ref().take(1_000));

    let Some(TranscriptEvent::Usage(record)) = events.last().map(|event| &event.payload) else {
        panic!("the usage record is last: {events:?}")
    };
    assert!(!record.r#final, "a failed call's usage");
    let audio = record
        .quantities
        .iter()
        .find(|quantity| quantity.kind() == QuantityKind::InputAudioMs)
        .and_then(|quantity| quantity.value());
    assert_eq!(
        audio,
        i64::try_from(forwarded_samples * 1_000 / 16_000).ok(),
        "only the three frames forwarded (story:contract-usage-and-naming-drift)"
    );
    assert_eq!(handle.output().frames_taken(), 3);
}
