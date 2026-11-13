//! The events an utterance emits from the recogniser's successive results, and the resampling of
//! frames at a second sample rate, and the end of a decode thread that panics, without weights.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use inference_stt::inference_core::envelope::EventEnvelope;
use inference_stt::inference_core::id::{NodeId, RunId, StreamId};
use inference_stt::inference_text::{SpanState, TurnId};
use inference_stt::{RecognitionRequest, TranscriptEvent, TranscriptOutput};

use super::{
    Decoded, Decoding, MAX_STREAM_RATE, MIN_STREAM_RATE, Outcome, Resampler, UnsupportedRate,
    Utterance, admit_rate, declare_adapter, declare_model, stream_rate, tail_padding,
};

/// Counts its wakes.
#[derive(Default)]
struct Wakes(AtomicUsize);

impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn a_decode_thread_that_panics_wakes_the_async_form_which_then_sees_no_result() {
    let (outcome, result, waker) = Outcome::<Decoded>::channel();
    let decoding = Decoding {
        result,
        waker,
        ended: true,
    };
    let wakes = Arc::new(Wakes::default());
    let waker = Waker::from(Arc::clone(&wakes));
    let mut cx = Context::from_waker(&waker);
    assert!(decoding.poll(Some(&mut cx)).is_pending());
    let thread = std::thread::spawn(move || {
        let _outcome = outcome;
        panic!("the decode thread panics, as this case wants");
    });
    assert!(thread.join().is_err(), "the thread panicked");
    assert_eq!(
        wakes.0.load(Ordering::SeqCst),
        1,
        "its end woke the async form"
    );
    assert!(
        matches!(decoding.poll(Some(&mut cx)), Poll::Ready(None)),
        "the async form sees the thread ended without a result, which loses the engine and fails \
         the call"
    );
}

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
            TranscriptEvent::Usage(_) => ("usage", String::new()),
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

/// A ramp of `count` samples, `0.0, 1.0, 2.0, …`, so that linear interpolation is exact.
fn ramp(count: u16) -> Vec<f32> {
    (0..count).map(f32::from).collect()
}

#[test]
fn a_run_resampled_in_frames_equals_the_run_resampled_whole() {
    let input = ramp(1_000);
    let whole = Resampler::new(16_000, 8_000).push(&input);
    let mut split = Resampler::new(16_000, 8_000);
    let pieces: Vec<f32> = input
        .chunks(37)
        .flat_map(|chunk| split.push(chunk))
        .collect();
    assert_eq!(pieces, whole);
    assert_eq!(whole.len(), 500);
    assert!(
        whole
            .iter()
            .enumerate()
            .all(|(index, sample)| (sample - 2.0 * index as f32).abs() < 1e-3)
    );
}

#[test]
fn upsampling_interpolates_between_neighbours_across_frames() {
    let mut resampler = Resampler::new(8_000, 16_000);
    let mut output = resampler.push(&ramp(3));
    output.extend(resampler.push(&[3.0, 4.0]));
    assert_eq!(output, vec![0.0, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5]);
}

#[test]
fn an_absurd_rate_is_refused_before_the_tail_padding_is_allocated() {
    assert_eq!(
        tail_padding(u32::MAX),
        Err(UnsupportedRate { rate: u32::MAX })
    );
    assert_eq!(
        tail_padding(MAX_STREAM_RATE + 1),
        Err(UnsupportedRate {
            rate: MAX_STREAM_RATE + 1
        })
    );
    assert_eq!(tail_padding(16_000), Ok(10_560));
    assert_eq!(tail_padding(MAX_STREAM_RATE), Ok(253_440));
    assert_eq!(tail_padding(1), Err(UnsupportedRate { rate: 1 }));
}

#[test]
fn a_rate_outside_the_bounds_is_refused_and_the_bounds_are_taken() {
    assert_eq!(
        stream_rate(u32::MAX),
        Err(UnsupportedRate { rate: u32::MAX })
    );
    assert_eq!(stream_rate(1), Err(UnsupportedRate { rate: 1 }));
    assert_eq!(
        stream_rate(MIN_STREAM_RATE - 1),
        Err(UnsupportedRate {
            rate: MIN_STREAM_RATE - 1
        })
    );
    assert_eq!(stream_rate(MIN_STREAM_RATE), Ok(MIN_STREAM_RATE));
    assert_eq!(stream_rate(MAX_STREAM_RATE), Ok(MAX_STREAM_RATE));
    assert_eq!(stream_rate(48_000), Ok(48_000));
}

#[test]
fn a_first_frame_at_a_refused_rate_does_not_set_the_streams_rate() {
    for rate in [u32::MAX, 1] {
        let mut stream = None;
        assert_eq!(admit_rate(&mut stream, rate), Err(UnsupportedRate { rate }));
        assert_eq!(
            stream, None,
            "a refused {rate} Hz frame sets no stream rate"
        );
    }
    let mut stream = None;
    assert_eq!(admit_rate(&mut stream, 8_000), Ok(8_000));
    assert_eq!(stream, Some(8_000));
}

#[test]
fn a_later_frame_at_a_refused_rate_is_refused_and_the_stream_keeps_its_rate() {
    let mut stream = Some(MAX_STREAM_RATE);
    assert_eq!(admit_rate(&mut stream, 1), Err(UnsupportedRate { rate: 1 }));
    assert_eq!(
        admit_rate(&mut stream, u32::MAX),
        Err(UnsupportedRate { rate: u32::MAX })
    );
    assert_eq!(stream, Some(MAX_STREAM_RATE));
    assert_eq!(admit_rate(&mut stream, 16_000), Ok(MAX_STREAM_RATE));
}
