//! The events a recognition emits from the text whisper.cpp transcribed, without weights.

use std::collections::VecDeque;

use inference_stt::inference_core::envelope::EventEnvelope;
use inference_stt::inference_core::id::{NodeId, RunId, StreamId};
use inference_stt::inference_text::{SpanState, TurnId};
use inference_stt::{RecognitionRequest, TranscriptEvent, TranscriptOutput};

use super::{declare_adapter, declare_model, transcribed};

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
