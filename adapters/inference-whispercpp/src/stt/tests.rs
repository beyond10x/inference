//! The events a recognition emits from the text whisper.cpp transcribed, and which segments are
//! annotations rather than text, without weights.

use std::collections::VecDeque;
use std::time::Duration;

use inference_stt::inference_core::envelope::EventEnvelope;
use inference_stt::inference_core::id::{NodeId, RunId, StreamId};
use inference_stt::inference_core::usage::{Quantity, QuantityKind, UsageRecorded};
use inference_stt::inference_text::{SpanState, TurnId};
use inference_stt::{RecognitionRequest, TranscriptEvent, TranscriptOutput};

use super::{
    RecognitionFailed, Transcription, declare_adapter, declare_model, is_annotation, settle,
    transcribed,
};

fn output(first_sequence: i64) -> TranscriptOutput {
    let (model, _) = declare_model();
    let (adapter, _) = declare_adapter(model.model_id(), 1).expect("one is positive");
    let request = RecognitionRequest::continuing(
        RunId::generate(),
        TurnId::generate(),
        StreamId::generate(),
        first_sequence,
        NodeId::generate(),
        "text_out",
    )
    .expect("the sequence is positive");
    let mut output = TranscriptOutput::start(&adapter, request);
    output.end_input().expect("input ends once");
    output
}

fn kinds(delivered: &VecDeque<EventEnvelope<TranscriptEvent>>) -> Vec<(&'static str, String)> {
    delivered
        .iter()
        .map(|envelope| match &envelope.payload {
            TranscriptEvent::Hypothesis(event) => ("hypothesis", event.text.clone()),
            TranscriptEvent::Revision(event) => ("revision", event.text.clone()),
            TranscriptEvent::Committed(event) => ("committed", event.text.clone()),
            TranscriptEvent::Usage(_) => ("usage", String::new()),
        })
        .collect()
}

#[test]
fn no_text_emits_nothing() {
    let mut output = output(1);
    let mut delivered = VecDeque::new();
    transcribed(&mut output, "", &mut delivered);
    transcribed(&mut output, "  \n", &mut delivered);
    assert!(delivered.is_empty());
    assert!(output.spans().is_empty());
    assert_eq!(output.next_sequence(), Some(1));
}

#[test]
fn text_is_emitted_as_a_hypothesis_and_committed_at_once_trimmed() {
    let mut output = output(7);
    let mut delivered = VecDeque::new();
    transcribed(
        &mut output,
        " What is the capital of France? ",
        &mut delivered,
    );
    assert_eq!(
        kinds(&delivered),
        vec![
            ("hypothesis", "What is the capital of France?".to_owned()),
            ("committed", "What is the capital of France?".to_owned()),
        ]
    );
    let sequences: Vec<i64> = delivered.iter().map(|envelope| envelope.sequence).collect();
    assert_eq!(sequences, vec![7, 8]);
    assert_eq!(output.spans().len(), 1);
    assert_eq!(output.spans()[0].state(), SpanState::Committed);
}

/// The usage record `delivered` ends with.
fn last_usage(delivered: &VecDeque<EventEnvelope<TranscriptEvent>>) -> &UsageRecorded {
    match delivered.back().map(|envelope| &envelope.payload) {
        Some(TranscriptEvent::Usage(recorded)) => recorded,
        other => panic!("the usage record is the last event, not {other:?}"),
    }
}

fn local_compute(recorded: &UsageRecorded) -> Quantity {
    *recorded
        .quantities
        .iter()
        .find(|quantity| quantity.kind() == QuantityKind::LocalComputeMs)
        .expect("a local_compute_ms quantity")
}

#[test]
fn a_failed_transcription_records_its_usage_with_counts_not_final() {
    let mut output = output(1);
    let mut delivered = VecDeque::new();
    let failure = settle(
        &mut output,
        Some(Transcription {
            result: Err(RecognitionFailed {
                detail: "the transcription failed: test".to_owned(),
            }),
            spent: Duration::from_millis(30),
        }),
        &mut delivered,
    );
    assert_eq!(
        failure.map(|failed| failed.detail),
        Some("the transcription failed: test".to_owned())
    );
    assert_eq!(kinds(&delivered), vec![("usage", String::new())]);
    let recorded = last_usage(&delivered);
    assert!(!recorded.r#final, "a failed call's counts are not final");
    assert_eq!(
        local_compute(recorded),
        Quantity::measured(QuantityKind::LocalComputeMs, 30),
        "the compute spent on the failed call is kept"
    );
}

#[test]
fn a_transcription_records_its_usage_after_the_commit_with_final_counts() {
    let mut output = output(1);
    let mut delivered = VecDeque::new();
    let failure = settle(
        &mut output,
        Some(Transcription {
            result: Ok("hello".to_owned()),
            spent: Duration::from_millis(5),
        }),
        &mut delivered,
    );
    assert!(failure.is_none());
    assert_eq!(
        kinds(&delivered),
        vec![
            ("hypothesis", "hello".to_owned()),
            ("committed", "hello".to_owned()),
            ("usage", String::new()),
        ]
    );
    let recorded = last_usage(&delivered);
    assert!(recorded.r#final);
    assert_eq!(
        local_compute(recorded),
        Quantity::measured(QuantityKind::LocalComputeMs, 5)
    );
}

#[test]
fn no_audio_records_its_usage_with_the_compute_unknown() {
    let mut output = output(1);
    let mut delivered = VecDeque::new();
    assert!(settle(&mut output, None, &mut delivered).is_none());
    let recorded = last_usage(&delivered);
    assert!(recorded.r#final);
    assert_eq!(
        local_compute(recorded),
        Quantity::unknown(QuantityKind::LocalComputeMs),
        "whisper.cpp was not called"
    );
}

#[test]
fn an_exhausted_stream_emits_what_it_can_without_panicking() {
    let mut output = output(i64::MAX);
    let mut delivered = VecDeque::new();
    transcribed(&mut output, "WHAT", &mut delivered);
    assert_eq!(kinds(&delivered), vec![("hypothesis", "WHAT".to_owned())]);
    assert_eq!(output.next_sequence(), None);
    assert_eq!(output.spans().len(), 1);
    assert_eq!(output.spans()[0].state(), SpanState::Hypothesis);
}

#[test]
fn a_segment_of_bracketed_or_parenthesised_annotations_alone_is_not_text() {
    for annotation in [
        "[BLANK_AUDIO]",
        " [BLANK_AUDIO]",
        "(silence)",
        " [Music] (wind blowing) ",
        "[ Inaudible ]",
    ] {
        assert!(is_annotation(annotation), "{annotation:?}");
    }
    for text in [
        "",
        "  ",
        " you",
        " After early nightfall",
        " [Music] hello",
        " hello (laughs)",
        "[unclosed",
        "closed]",
        "(mismatched]",
    ] {
        assert!(!is_annotation(text), "{text:?}");
    }
}
