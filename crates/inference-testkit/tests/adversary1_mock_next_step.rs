//! Adversary pass 1 of `story:contracts-review-fixes`: the mock docs said the recogniser and the
//! synthesiser take what was offered "at its next step, when the handle is next iterated". Both
//! take input only when nothing is left to deliver: an iteration that hands out an event already
//! produced takes nothing. Correction round 1 corrected the docs to "its next step that has
//! nothing left to deliver", and the two cases below assert that. And a recognition continuing a
//! stream near `i64::MAX` runs its script to the last sequence without a panic.

#![cfg(all(feature = "stt", feature = "tts"))]

use std::num::{NonZeroU16, NonZeroU32};

use inference_core::generation::{Generation, StartGeneration};
use inference_core::id::{NodeId, RunId, StreamId};
use inference_core::model::ModelId;
use inference_stt::inference_text::ReviseSpan;
use inference_stt::{
    Recogniser, RecognitionHandle, RecognitionRequest, RevisionRefused, SequenceExhausted,
};
use inference_testkit::stt::{self, MockRecogniser, frames, script};
use inference_testkit::tts::{self, MockSynthesiser};
use inference_text::{OutputSegment, TurnId};
use inference_tts::inference_audio::{AudioEncoding, AudioFormat};
use inference_tts::{SynthesisHandle, Synthesiser};

fn committed(text: &str) -> OutputSegment {
    let (generation, _) = Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    });
    let (tentative, _) =
        OutputSegment::emit(&generation, 1, text.to_owned()).expect("the generation is Active");
    tentative.commit().expect("a Tentative segment commits").0
}

fn request() -> RecognitionRequest {
    RecognitionRequest::new(
        RunId::generate(),
        TurnId::generate(),
        StreamId::generate(),
        NodeId::generate(),
        "text_out",
    )
}

/// Two frames reach two cues (hypothesis, revision); the first iteration delivers one and keeps
/// the other. A third frame is then offered. Correction round 1: by the corrected doc, the
/// iteration that hands out the kept revision takes nothing; the next one, with nothing left to
/// deliver, takes the frame.
#[test]
fn the_mock_recogniser_takes_the_offered_frames_at_its_next_step() {
    let mut handle = MockRecogniser::new(
        stt::MockConfiguration::Incremental,
        ModelId::generate(),
        script(),
    )
    .start(request())
    .expect("a fresh recogniser runs nothing else");
    let mut fixture = frames(3).into_iter();
    handle
        .offer_frame(fixture.next().expect("three"))
        .expect("open");
    handle
        .offer_frame(fixture.next().expect("three"))
        .expect("open");
    assert!(handle.next().is_some(), "the hypothesis of the first cue");
    assert_eq!(handle.output().frames_taken(), 2);

    handle
        .offer_frame(fixture.next().expect("three"))
        .expect("open");
    assert!(handle.next().is_some(), "the revision already produced");
    assert_eq!(
        handle.output().frames_taken(),
        2,
        "an iteration that hands out an event already produced takes nothing"
    );
    assert!(handle.next().is_some(), "the commit of the third cue");
    assert_eq!(
        handle.output().frames_taken(),
        3,
        "the next step with nothing left to deliver takes the frame offered before it"
    );
}

/// Without output streaming the mock completes a segment in one step, queueing its
/// `SynthesisCompleted` and its frames; the first iteration delivers the completion. A second
/// segment is then offered. Correction round 1: by the corrected doc, the iterations that hand
/// out the first segment's two frames take nothing; the next one, with nothing left to deliver,
/// takes the segment.
#[test]
fn the_mock_synthesiser_takes_the_offered_segments_at_its_next_step() {
    let format = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).expect("not zero"),
        channels: NonZeroU16::new(1).expect("not zero"),
    };
    let mut handle = MockSynthesiser::new(
        tts::MockConfiguration::IncrementalInputOnly,
        ModelId::generate(),
        format,
    )
    .start()
    .expect("a fresh synthesiser runs nothing else");
    handle.offer_segment(committed("one two")).expect("queued");
    assert!(
        handle.next().is_some(),
        "the completion of the first segment"
    );
    assert_eq!(handle.output().segments_taken(), 1);

    handle.offer_segment(committed("three")).expect("queued");
    for frame in 1..=2 {
        assert!(
            handle.next().is_some(),
            "frame {frame} of the first segment"
        );
        assert_eq!(
            handle.output().segments_taken(),
            1,
            "an iteration that hands out an event already produced takes nothing"
        );
    }
    assert!(
        handle.next().is_some(),
        "the completion of the second segment"
    );
    assert_eq!(
        handle.output().segments_taken(),
        2,
        "the next step with nothing left to deliver takes the segment offered before it"
    );
}

/// Guard, green: a recognition continuing its stream one before `i64::MAX` delivers the two
/// events that fit, then nothing, through the end of input, without a panic; a revision through
/// the handle is refused with the exhausted stream.
#[test]
fn the_mock_recogniser_stops_at_the_last_sequence_of_its_stream_without_a_panic() {
    for configuration in stt::MockConfiguration::ALL {
        let fresh = request();
        let continuing = RecognitionRequest::continuing(
            fresh.run_id,
            fresh.turn_id,
            fresh.stream_id,
            i64::MAX - 1,
            fresh.node_id,
            "text_out",
        )
        .expect("i64::MAX - 1 is a sequence");
        let mut handle = MockRecogniser::new(configuration, ModelId::generate(), script())
            .start(continuing)
            .expect("a fresh recogniser runs nothing else");
        let mut events = Vec::new();
        for frame in frames(6) {
            handle.offer_frame(frame).expect("input has not ended");
            events.extend(handle.by_ref());
        }
        handle.end_input().expect("input ends once");
        events.extend(handle.by_ref());
        events.extend(handle.by_ref());

        let sequences: Vec<i64> = events.iter().map(|event| event.sequence).collect();
        assert_eq!(
            sequences,
            vec![i64::MAX - 1, i64::MAX],
            "{configuration:?}: the two sequences left, then nothing"
        );
        assert_eq!(handle.output().next_sequence(), None);
        let span_id = handle.output().spans()[0].span_id();
        assert_eq!(
            handle.revise(ReviseSpan {
                span_id,
                text: "late".to_owned(),
            }),
            Err(RevisionRefused::SequenceExhausted(SequenceExhausted {
                stream_id: fresh.stream_id,
            })),
            "{configuration:?}"
        );
        assert!(handle.next().is_none(), "{configuration:?}");
    }
}
