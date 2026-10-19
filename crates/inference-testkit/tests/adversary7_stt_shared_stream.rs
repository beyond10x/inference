//! Adversary, pass 1: `RecognitionRequest` takes its `stream_id` from the caller, and
//! `TranscriptOutput` numbers every recognition's envelopes from 1. Two recognitions on one
//! stream, such as two turns recognised on one output port, then write sequence 1 twice on that
//! stream, which the per-stream rule of §23.1 test 6 (`docs/design/inference.md:1110`), as the
//! testkit's `EventLog` enforces it, refuses; and two spans on one stream carry one sequence,
//! the duplicate `ess/domains/stream.yaml` says is to be refused (`EmitHypothesis`, UNMAPPED).
//!
//! Correction round 1: the contract does not remember streams; the stream's owner tracks the next
//! sequence and starts the second recognition with `RecognitionRequest::continuing` at the first's
//! last sequence + 1. A second recognition started fresh on the same stream is still refused by
//! the `EventLog`.

#![cfg(feature = "stt")]

use inference_core::envelope::EventEnvelope;
use inference_core::id::{NodeId, RunId, StreamId};
use inference_core::model::ModelId;
use inference_stt::{Recogniser, RecognitionHandle, RecognitionRequest, TranscriptEvent};
use inference_testkit::stream::{EnvelopeRefusal, EventLog};
use inference_testkit::stt::{MockConfiguration, MockRecogniser, frames, script, stream_envelope};
use inference_text::TurnId;

/// Runs one recognition of the fixture and returns its events and its spans' sequences.
fn recognise(
    recogniser: &mut MockRecogniser,
    request: RecognitionRequest,
) -> (Vec<EventEnvelope<TranscriptEvent>>, Vec<i64>) {
    let mut handle = recogniser
        .start(request)
        .expect("the previous recognition was dropped");
    let mut events = Vec::new();
    for frame in frames(6) {
        handle.offer_frame(frame).expect("input has not ended");
        events.extend(handle.by_ref());
    }
    handle.end_input().expect("input ends once");
    events.extend(handle.by_ref());
    let sequences = handle
        .output()
        .spans()
        .iter()
        .map(|span| span.sequence())
        .collect();
    (events, sequences)
}

#[test]
fn two_recognitions_on_one_stream_obey_the_per_stream_sequence() {
    let run_id = RunId::generate();
    let stream_id = StreamId::generate();
    let node_id = NodeId::generate();
    let mut recogniser = MockRecogniser::new(
        MockConfiguration::Incremental,
        ModelId::generate(),
        script(),
    );
    let mut log = EventLog::new();
    let mut span_sequences = Vec::new();
    for turn in 0..2 {
        let request = match log.envelopes().last() {
            None => {
                RecognitionRequest::new(run_id, TurnId::generate(), stream_id, node_id, "text_out")
            }
            Some(last) => RecognitionRequest::continuing(
                run_id,
                TurnId::generate(),
                stream_id,
                last.sequence + 1,
                node_id,
                "text_out",
            )
            .expect("one past a recorded sequence is a sequence"),
        };
        let (events, sequences) = recognise(&mut recogniser, request);
        span_sequences.extend(sequences);
        for envelope in events {
            log.record(stream_envelope(envelope))
                .unwrap_or_else(|refusal| panic!("turn {turn} on the shared stream: {refusal}"));
        }
    }
    let mut unique = span_sequences.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        span_sequences.len(),
        "no two spans on one stream carry one sequence: {span_sequences:?}"
    );
}

#[test]
fn a_second_recognition_started_fresh_on_one_stream_is_refused_by_the_log() {
    let run_id = RunId::generate();
    let stream_id = StreamId::generate();
    let node_id = NodeId::generate();
    let mut recogniser = MockRecogniser::new(
        MockConfiguration::Incremental,
        ModelId::generate(),
        script(),
    );
    let mut log = EventLog::new();
    let fresh =
        || RecognitionRequest::new(run_id, TurnId::generate(), stream_id, node_id, "text_out");

    let (first, _) = recognise(&mut recogniser, fresh());
    for envelope in first {
        log.record(stream_envelope(envelope))
            .expect("the first recognition on the stream is recorded");
    }
    let last = log.envelopes().last().expect("recorded").sequence;

    let (second, _) = recognise(&mut recogniser, fresh());
    let opening = second
        .into_iter()
        .next()
        .expect("the second recognition emits");
    assert_eq!(
        log.record(stream_envelope(opening)),
        Err(EnvelopeRefusal::SequenceNotIncreasing {
            stream_id,
            last,
            sequence: 1,
        }),
        "a recognition started fresh on a stream that already carries events writes sequence 1 again"
    );
}
