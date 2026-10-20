//! The events an utterance emits from the recogniser's successive results, without weights.

use std::collections::VecDeque;

use inference_stt::inference_core::envelope::EventEnvelope;
use inference_stt::inference_core::id::{NodeId, RunId, StreamId};
use inference_stt::inference_text::{SpanState, TurnId};
use inference_stt::{RecognitionRequest, TranscriptEvent, TranscriptOutput};

use super::{Utterance, declare_adapter, declare_model};

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
    TranscriptOutput::start(&adapter, request)
}

fn kinds(delivered: &VecDeque<EventEnvelope<TranscriptEvent>>) -> Vec<(&'static str, String)> {
    delivered
        .iter()
        .map(|envelope| match &envelope.payload {
            TranscriptEvent::Hypothesis(event) => ("hypothesis", event.text.clone()),
            TranscriptEvent::Revision(event) => ("revision", event.text.clone()),
            TranscriptEvent::Committed(_) => ("committed", String::new()),
        })
        .collect()
}

#[test]
fn an_utterance_with_no_text_emits_nothing() {
    let mut output = output(1);
    let mut delivered = VecDeque::new();
    let mut utterance = Utterance::default();
    utterance.observe(&mut output, "", &mut delivered);
    utterance.observe(&mut output, "  ", &mut delivered);
    utterance.close(&mut output, &mut delivered);
    assert!(delivered.is_empty());
    assert!(output.spans().is_empty());
}

#[test]
fn the_first_text_emits_a_hypothesis_a_change_revises_it_and_the_close_commits_it() {
    let mut output = output(1);
    let mut delivered = VecDeque::new();
    let mut utterance = Utterance::default();
    utterance.observe(&mut output, "", &mut delivered);
    utterance.observe(&mut output, " WHAT", &mut delivered);
    utterance.observe(&mut output, "WHAT", &mut delivered);
    utterance.observe(&mut output, "WHAT IS THE", &mut delivered);
    utterance.close(&mut output, &mut delivered);
    assert_eq!(
        kinds(&delivered),
        vec![
            ("hypothesis", "WHAT".to_owned()),
            ("revision", "WHAT IS THE".to_owned()),
            ("committed", String::new()),
        ]
    );
    let sequences: Vec<i64> = delivered.iter().map(|envelope| envelope.sequence).collect();
    assert_eq!(sequences, vec![1, 2, 3]);
    assert_eq!(output.spans().len(), 1);
    assert_eq!(output.spans()[0].text(), "WHAT IS THE");
    assert_eq!(output.spans()[0].state(), SpanState::Committed);
}

#[test]
fn text_after_a_close_opens_the_next_span() {
    let mut output = output(1);
    let mut delivered = VecDeque::new();
    let mut utterance = Utterance::default();
    utterance.observe(&mut output, "HELLO", &mut delivered);
    utterance.close(&mut output, &mut delivered);
    utterance.close(&mut output, &mut delivered);
    utterance.observe(&mut output, "THERE", &mut delivered);
    utterance.close(&mut output, &mut delivered);
    assert_eq!(
        kinds(&delivered),
        vec![
            ("hypothesis", "HELLO".to_owned()),
            ("committed", String::new()),
            ("hypothesis", "THERE".to_owned()),
            ("committed", String::new()),
        ]
    );
    let texts: Vec<&str> = output.spans().iter().map(|span| span.text()).collect();
    assert_eq!(texts, vec!["HELLO", "THERE"]);
    assert!(
        output
            .spans()
            .iter()
            .all(|span| span.state() == SpanState::Committed)
    );
}

#[test]
fn an_exhausted_stream_stops_the_utterance_without_panicking() {
    let mut output = output(i64::MAX);
    let mut delivered = VecDeque::new();
    let mut utterance = Utterance::default();
    utterance.observe(&mut output, "WHAT", &mut delivered);
    utterance.observe(&mut output, "WHAT IS", &mut delivered);
    utterance.close(&mut output, &mut delivered);
    utterance.observe(&mut output, "MORE", &mut delivered);
    assert_eq!(kinds(&delivered), vec![("hypothesis", "WHAT".to_owned())]);
    assert_eq!(output.next_sequence(), None);
    assert_eq!(output.spans().len(), 1);
}
