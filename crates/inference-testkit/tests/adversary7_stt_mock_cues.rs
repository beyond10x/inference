//! Adversary, pass 1: what the mock recogniser's module documentation promises about its cues and
//! its handle, asserted on the events themselves. Claiming `incremental_input` and
//! `output_streaming`, it "takes the frames offered so far at its next step, when the handle is
//! next iterated, and runs every cue whose frame count is reached"; "at the end of input every
//! cue left runs, whatever its frame count"; and a
//! revision issued through the handle "is delivered in sequence with the other events".

#![cfg(feature = "stt")]

use inference_core::id::{NodeId, RunId, StreamId};
use inference_core::model::ModelId;
use inference_stt::{Recogniser, RecognitionHandle, RecognitionRequest, TranscriptEvent};
use inference_testkit::stream::EventLog;
use inference_testkit::stt::{
    Cue, FRAME_SAMPLES, MockConfiguration, MockRecogniser, ScriptStep, frames, script,
    stream_envelope,
};
use inference_text::{ReviseSpan, SpanState, TurnId};

fn request() -> RecognitionRequest {
    RecognitionRequest::new(
        RunId::generate(),
        TurnId::generate(),
        StreamId::generate(),
        NodeId::generate(),
        "text_out",
    )
}

fn kind_of(event: &TranscriptEvent) -> &'static str {
    match event {
        TranscriptEvent::Hypothesis(_) => "hypothesis",
        TranscriptEvent::Revision(_) => "revision",
        TranscriptEvent::Committed(_) => "committed",
    }
}

#[test]
fn incremental_runs_each_cue_on_the_frame_that_reaches_its_count() {
    let mut handle = MockRecogniser::new(
        MockConfiguration::Incremental,
        ModelId::generate(),
        script(),
    )
    .start(request())
    .expect("a fresh recogniser runs nothing else");
    let expected = [
        "hypothesis",
        "revision",
        "committed",
        "hypothesis",
        "revision",
        "committed",
    ];
    for (index, frame) in frames(6).into_iter().enumerate() {
        handle.offer_frame(frame).expect("input has not ended");
        let delivered: Vec<_> = handle.by_ref().collect();
        let kinds: Vec<&str> = delivered.iter().map(|e| kind_of(&e.payload)).collect();
        assert_eq!(
            kinds,
            [expected[index]],
            "after frame {} exactly the cue counting {} frames runs",
            index + 1,
            index + 1
        );
        let past = i64::try_from(index + 1).expect("small") * i64::from(FRAME_SAMPLES);
        assert_eq!(
            delivered[0].timestamp,
            past,
            "stamped just past frame {}",
            index + 1
        );
    }
    handle.end_input().expect("input ends once");
    assert_eq!(handle.next(), None, "nothing is left for the end of input");
}

#[test]
fn a_cue_past_the_offered_frames_runs_at_the_end_of_input() {
    for configuration in MockConfiguration::ALL {
        let cues = vec![
            Cue {
                after_frames: 1,
                step: ScriptStep::Hypothesis("one".to_owned()),
            },
            Cue {
                after_frames: 10,
                step: ScriptStep::Commit { span: 0 },
            },
        ];
        let mut handle = MockRecogniser::new(configuration, ModelId::generate(), cues)
            .start(request())
            .expect("a fresh recogniser runs nothing else");
        let mut events = Vec::new();
        for frame in frames(2) {
            handle.offer_frame(frame).expect("input has not ended");
            events.extend(handle.by_ref());
        }
        handle.end_input().expect("input ends once");
        events.extend(handle.by_ref());
        let kinds: Vec<&str> = events.iter().map(|e| kind_of(&e.payload)).collect();
        assert_eq!(
            kinds,
            ["hypothesis", "committed"],
            "{configuration:?}: every cue left runs at the end of input, whatever its frame count"
        );
        assert_eq!(handle.output().spans()[0].state(), SpanState::Committed);
    }
}

#[test]
fn a_revision_through_the_handle_is_delivered_in_sequence() {
    let request = request();
    let mut handle = MockRecogniser::new(
        MockConfiguration::Incremental,
        ModelId::generate(),
        script(),
    )
    .start(request.clone())
    .expect("a fresh recogniser runs nothing else");
    let mut frames = frames(6).into_iter();
    handle
        .offer_frame(frames.next().expect("six"))
        .expect("input has not ended");
    let mut events: Vec<_> = handle.by_ref().collect();
    let span_id = events[0].payload.span_id();

    handle
        .revise(ReviseSpan {
            span_id,
            text: "hullo".to_owned(),
        })
        .expect("a Hypothesis is revised");
    let revision = handle.next().expect("the revision is delivered");
    assert_eq!(revision.sequence, 2);
    match &revision.payload {
        TranscriptEvent::Revision(event) => {
            assert_eq!(event.span_id, span_id);
            assert_eq!(event.text, "hullo");
        }
        other => panic!("a TranscriptRevision is delivered, got {other:?}"),
    }
    events.push(revision);

    for frame in frames {
        handle.offer_frame(frame).expect("input has not ended");
        events.extend(handle.by_ref());
    }
    handle.end_input().expect("input ends once");
    events.extend(handle.by_ref());

    let sequences: Vec<i64> = events.iter().map(|e| e.sequence).collect();
    assert_eq!(sequences, (1..=7).collect::<Vec<i64>>());
    let mut log = EventLog::new();
    for envelope in events {
        log.record(stream_envelope(envelope))
            .unwrap_or_else(|refusal| panic!("{refusal}"));
    }
}
