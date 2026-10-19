//! §23.1 test 1 of `docs/design/inference.md` (line 1105): STT revisions cannot rewrite committed
//! spans, against the mock recogniser of both configurations. Beside it, incremental input and
//! output streaming are observed on the same mock, and each configuration claims exactly what it
//! was observed to do: input is ingested incrementally when frames are taken before the end of
//! input, and output streams when an event reaches the caller before it (line 358). Every event
//! of a recognition is recorded in the `EventLog` of the stream domain, so the envelopes it
//! carries obey §23.1 test 6.

#![cfg(feature = "stt")]

use std::collections::HashSet;

use inference_core::envelope::{EventEnvelope, Timebase};
use inference_core::id::{NodeId, RunId, StreamId};
use inference_core::model::{Capability, ModelId};
use inference_stt::{
    Recogniser, RecognitionHandle, RecognitionRequest, RevisionRefused, TranscriptEvent,
};
use inference_testkit::stream::EventLog;
use inference_testkit::stt::{
    FRAME_SAMPLES, MockConfiguration, MockRecogniser, MockRecognition, frames, script,
    stream_envelope,
};
use inference_text::{CommittedSpanImmutable, ReviseSpan, SpanState, TurnId};

const FRAMES: usize = 6;

fn request() -> RecognitionRequest {
    RecognitionRequest::new(
        RunId::generate(),
        TurnId::generate(),
        StreamId::generate(),
        NodeId::generate(),
        "text_out",
    )
}

fn recogniser(configuration: MockConfiguration) -> MockRecogniser {
    MockRecogniser::new(configuration, ModelId::generate(), script())
}

/// What one recognition delivered before and after the end of input, with the frames the
/// recogniser had taken before it.
struct Recognition {
    handle: MockRecognition,
    before_end: Vec<EventEnvelope<TranscriptEvent>>,
    taken_before_end: usize,
    after_end: Vec<EventEnvelope<TranscriptEvent>>,
}

/// Offers the fixture frames one at a time, draining what is delivered after each, then ends
/// input and drains the rest.
fn recognise(configuration: MockConfiguration) -> Recognition {
    let mut handle = recogniser(configuration).start(request());
    let mut before_end = Vec::new();
    for frame in frames(FRAMES) {
        handle.offer_frame(frame).expect("input has not ended");
        before_end.extend(handle.by_ref());
    }
    let taken_before_end = handle.output().frames_taken();
    handle.end_input().expect("input ends once");
    let after_end = handle.by_ref().collect();
    Recognition {
        handle,
        before_end,
        taken_before_end,
        after_end,
    }
}

#[test]
fn a_revision_of_a_committed_span_is_refused_and_its_text_is_unchanged() {
    for configuration in MockConfiguration::ALL {
        let Recognition {
            mut handle,
            before_end,
            after_end,
            ..
        } = recognise(configuration);
        let committed = before_end
            .iter()
            .chain(&after_end)
            .find_map(|envelope| match &envelope.payload {
                TranscriptEvent::Committed(event) => Some(event.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{configuration:?} commits a span"));

        let answer = handle.revise(ReviseSpan {
            span_id: committed.span_id,
            text: "rewritten".to_owned(),
        });

        assert_eq!(
            answer,
            Err(RevisionRefused::CommittedSpanImmutable(
                CommittedSpanImmutable {
                    span_id: committed.span_id,
                }
            )),
            "{configuration:?}: a revision of a committed span is refused as CommittedSpanImmutable"
        );
        let span = handle
            .output()
            .span(committed.span_id)
            .expect("the committed span is held");
        assert_eq!(
            span.text(),
            committed.text,
            "{configuration:?}: the committed text is unchanged"
        );
        assert_eq!(span.state(), SpanState::Committed);
        assert_eq!(
            handle.next(),
            None,
            "{configuration:?}: a refused revision delivers no event"
        );
    }
}

#[test]
fn incremental_emits_a_hypothesis_before_end_of_input_and_batch_nothing() {
    let incremental = recognise(MockConfiguration::Incremental);
    assert!(
        incremental
            .before_end
            .iter()
            .any(|envelope| matches!(envelope.payload, TranscriptEvent::Hypothesis(_))),
        "claiming incremental_input and output_streaming, a hypothesis arrives before the end of input: {:?}",
        incremental.before_end
    );

    let batch = recognise(MockConfiguration::Batch);
    assert_eq!(
        batch.before_end,
        [],
        "claiming neither, nothing arrives before the end of input"
    );
    assert_eq!(
        batch.taken_before_end, 0,
        "claiming neither, no frame is taken before the end of input"
    );
    assert!(
        batch
            .after_end
            .iter()
            .any(|envelope| matches!(envelope.payload, TranscriptEvent::Hypothesis(_))),
        "claiming neither, the hypotheses arrive after the end of input: {:?}",
        batch.after_end
    );
}

#[test]
fn each_configuration_claims_exactly_what_it_was_observed_to_do() {
    for configuration in MockConfiguration::ALL {
        let recognition = recognise(configuration);
        let mut observed = HashSet::new();
        if recognition.taken_before_end > 0 {
            observed.insert(Capability::IncrementalInput);
        }
        if !recognition.before_end.is_empty() {
            observed.insert(Capability::OutputStreaming);
        }
        let claimed: HashSet<Capability> = recogniser(configuration)
            .adapter()
            .capabilities()
            .iter()
            .copied()
            .collect();
        assert_eq!(
            claimed, observed,
            "{configuration:?} claims what it was observed to do"
        );
    }
}

#[test]
fn every_event_of_a_recognition_is_enveloped_in_sequence_on_audio_frame_time() {
    for configuration in MockConfiguration::ALL {
        let recognition = recognise(configuration);
        let request = recognition.handle.output().request().clone();
        let envelopes: Vec<_> = recognition
            .before_end
            .into_iter()
            .chain(recognition.after_end)
            .collect();
        let sequences: Vec<i64> = envelopes.iter().map(|envelope| envelope.sequence).collect();
        let expected: Vec<i64> = (1..=6).collect();
        assert_eq!(sequences, expected, "{configuration:?}: sequences from 1");

        let mut last_timestamp = 0;
        for envelope in &envelopes {
            assert_eq!(envelope.stream_id, request.stream_id);
            assert_eq!(envelope.run_id, request.run_id);
            assert_eq!(envelope.producer, request.node_id);
            assert_eq!(envelope.port, request.port);
            assert_eq!(envelope.timebase, Timebase::AudioFrameTime);
            assert!(envelope.timestamp >= last_timestamp);
            last_timestamp = envelope.timestamp;
        }
        assert_eq!(
            last_timestamp,
            i64::try_from(FRAMES).expect("six fits") * i64::from(FRAME_SAMPLES),
            "{configuration:?}: the last event is stamped past the last sample taken"
        );

        let mut log = EventLog::new();
        for envelope in envelopes {
            log.record(stream_envelope(envelope))
                .unwrap_or_else(|refusal| panic!("{configuration:?}: {refusal}"));
        }

        let spans = recognition.handle.output().spans();
        let texts: Vec<&str> = spans.iter().map(|span| span.text()).collect();
        assert_eq!(texts, ["hello there", "how are you"]);
        assert!(
            spans
                .iter()
                .all(|span| span.state() == SpanState::Committed)
        );
    }
}
