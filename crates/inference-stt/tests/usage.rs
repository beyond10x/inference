//! The usage of one recognition (`story:usage-stt`): once input has ended, `TranscriptOutput`
//! records it as the recognition's `inference.usage.UsageRecorded`, enveloped at the next
//! sequence of the request's stream. It measures the audio offered, samples over sample rate, and
//! one request, so every recogniser reports both without code of its own; the local compute is
//! what the implementation added, `measured`, or `unknown` when it added none. The record names
//! the adapter and the model and no run, node or session, which the node placing it fills.
//! It is the last event of the recognition: refused while a span is not committed, and once it
//! is made every later move is refused. A failed backend call is recorded with its counts not
//! final.

use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use inference_stt::inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_stt::inference_core::envelope::Timebase;
use inference_stt::inference_core::id::{NodeId, RunId, StreamId};
use inference_stt::inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_stt::inference_core::usage::{Quantity, QuantityKind, UsageRecorded};
use inference_stt::inference_text::{ReviseSpan, SpanId, TurnId};
use inference_stt::{
    CONTRACT_REVISION, CommitRefused, HypothesisRefused, InputNotEnded, RecognitionEnded,
    RecognitionRequest, RevisionRefused, SequenceExhausted, SpanNotCommitted, TranscriptEvent,
    TranscriptOutput, UsageAlreadyRecorded, UsageRefused,
};

fn adapter(capabilities: Vec<Capability>) -> Adapter {
    Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: "test".to_owned(),
        contract_revision: CONTRACT_REVISION.to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities,
        max_concurrency: 1,
    })
    .expect("one is positive")
    .0
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

/// A frame of `sample_count` samples per channel at `sample_rate` on `channels` channels, at
/// `position`.
fn frame(position: u64, sample_count: u32, sample_rate: u32, channels: u16) -> AudioFrame {
    let format = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(sample_rate).expect("not zero"),
        channels: NonZeroU16::new(channels).expect("not zero"),
    };
    AudioFrame {
        format,
        position: SampleClockPosition::new(position).expect("in range"),
        sample_count,
        data: vec![0; usize::try_from(format.data_len(sample_count)).expect("a small frame")],
    }
}

/// The usage record an envelope carries.
fn recorded(event: &TranscriptEvent) -> &UsageRecorded {
    match event {
        TranscriptEvent::Usage(recorded) => recorded,
        other => panic!("a usage record, not {other:?}"),
    }
}

fn quantity(recorded: &UsageRecorded, kind: QuantityKind) -> Quantity {
    let found: Vec<&Quantity> = recorded
        .quantities
        .iter()
        .filter(|quantity| quantity.kind() == kind)
        .collect();
    let [quantity] = found.as_slice() else {
        panic!("one {kind:?} quantity in {recorded:?}")
    };
    **quantity
}

#[test]
fn a_recognition_of_one_and_a_half_seconds_records_its_audio_and_one_request() {
    let adapter = adapter(Vec::new());
    let request = request();
    let mut output = TranscriptOutput::start(&adapter, request.clone());
    for index in 0..150 {
        output
            .offer(frame(index * 160, 160, 16_000, 1))
            .expect("input has not ended");
    }
    output.end_input().expect("input ends once");
    output.take_input().expect("input has ended");
    let hypothesis = output.hypothesis("hello".to_owned()).expect("a span");
    let span_id = hypothesis.payload.span_id().expect("a span event");
    output.commit(span_id).expect("the hypothesis commits");

    let envelope = output.usage().expect("input has ended");

    assert_eq!(envelope.kind, UsageRecorded::KIND);
    assert_eq!(envelope.payload.name(), UsageRecorded::KIND);
    assert_eq!(
        envelope.payload.span_id(),
        None,
        "a usage record has no span"
    );
    assert_eq!(envelope.sequence, 3, "the next sequence after the commit");
    assert_eq!(envelope.stream_id, request.stream_id);
    assert_eq!(envelope.run_id, request.run_id);
    assert_eq!(envelope.producer, request.node_id);
    assert_eq!(envelope.port, request.port);
    assert_eq!(envelope.timebase, Timebase::AudioFrameTime);
    assert_eq!(envelope.timestamp, 24_000, "past the last sample taken");
    assert_eq!(output.next_sequence(), Some(4));

    let record = recorded(&envelope.payload);
    assert_eq!(
        quantity(record, QuantityKind::InputAudioMs),
        Quantity::measured(QuantityKind::InputAudioMs, 1_500)
    );
    assert_eq!(
        quantity(record, QuantityKind::Requests),
        Quantity::measured(QuantityKind::Requests, 1)
    );
    assert_eq!(
        quantity(record, QuantityKind::LocalComputeMs),
        Quantity::unknown(QuantityKind::LocalComputeMs),
        "nothing added, so nobody counted it"
    );
    assert_eq!(record.quantities.len(), 3, "{record:?}");
    assert_eq!(record.adapter_id, adapter.adapter_id());
    assert_eq!(record.model_id, adapter.model_id());
    assert_eq!(record.run_id, None, "the node placing it fills the run");
    assert_eq!(record.node_id, None);
    assert_eq!(record.session_id, None);
    assert_eq!(record.speech_session_id, None);
    assert_eq!(record.response_id, None);
    assert_eq!(record.upstream_model, None);
    assert!(record.r#final);
    assert!(record.started_at_ms > 0, "on the wall clock: {record:?}");
    assert!(record.ended_at_ms >= record.started_at_ms, "{record:?}");
    assert!(output.usage_recorded());
}

#[test]
fn the_audio_counted_is_the_audio_offered_at_each_frame_rate() {
    let mut output = TranscriptOutput::start(&adapter(Vec::new()), request());
    output
        .offer(frame(0, 8_000, 8_000, 2))
        .expect("input has not ended");
    output
        .offer(frame(8_000, 441, 44_100, 1))
        .expect("input has not ended");
    output
        .offer(frame(8_441, 160, 16_000, 1))
        .expect("input has not ended");
    assert_eq!(
        output.input_audio_ms(),
        1_020,
        "1 s on two channels, 10 ms and 10 ms"
    );
    output.end_input().expect("input ends once");

    let envelope = output
        .usage()
        .expect("input has ended, taken or not: the audio offered is counted");

    assert_eq!(
        quantity(recorded(&envelope.payload), QuantityKind::InputAudioMs),
        Quantity::measured(QuantityKind::InputAudioMs, 1_020)
    );
    assert_eq!(envelope.sequence, 1, "the first event of the stream");
    assert_eq!(envelope.timestamp, 0, "no sample was taken");
}

#[test]
fn the_local_compute_added_is_measured_and_summed() {
    let mut output = TranscriptOutput::start(&adapter(Vec::new()), request());
    assert_eq!(output.local_compute(), None);
    output.add_local_compute(Duration::from_micros(700));
    output.add_local_compute(Duration::from_micros(800));
    output.add_local_compute(Duration::from_millis(40));
    assert_eq!(output.local_compute(), Some(Duration::from_micros(41_500)));
    output.end_input().expect("input ends once");

    let envelope = output.usage().expect("input has ended");

    assert_eq!(
        quantity(recorded(&envelope.payload), QuantityKind::LocalComputeMs),
        Quantity::measured(QuantityKind::LocalComputeMs, 41),
        "summed, then rounded down to the millisecond"
    );
}

#[test]
fn no_usage_is_recorded_before_the_end_of_input_nor_twice() {
    let mut output =
        TranscriptOutput::start(&adapter(vec![Capability::OutputStreaming]), request());
    output
        .offer(frame(0, 160, 16_000, 1))
        .expect("input has not ended");
    assert_eq!(
        output.usage().map(|_| ()),
        Err(UsageRefused::InputNotEnded(InputNotEnded)),
        "a recognition still taking input has completed no call"
    );
    assert!(!output.usage_recorded());
    assert_eq!(output.next_sequence(), Some(1), "no sequence used");

    output.end_input().expect("input ends once");
    let first = output.usage().expect("input has ended");
    assert_eq!(first.sequence, 1);
    assert_eq!(
        output.usage().map(|_| ()),
        Err(UsageRefused::UsageAlreadyRecorded(UsageAlreadyRecorded)),
        "one record per recognition"
    );
    assert_eq!(output.next_sequence(), Some(2), "no sequence used");
}

#[test]
fn past_the_last_sequence_no_usage_is_recorded() {
    let request = RecognitionRequest::continuing(
        RunId::generate(),
        TurnId::generate(),
        StreamId::generate(),
        i64::MAX - 1,
        NodeId::generate(),
        "text_out",
    )
    .expect("i64::MAX - 1 is a sequence");
    let mut output = TranscriptOutput::start(&adapter(Vec::new()), request.clone());
    output.end_input().expect("input ends once");
    let hypothesis = output.hypothesis("last".to_owned()).expect("two are left");
    let span_id = hypothesis.payload.span_id().expect("a span event");
    let committed = output.commit(span_id).expect("one is left");
    assert_eq!(committed.sequence, i64::MAX);

    assert_eq!(
        output.usage().map(|_| ()),
        Err(UsageRefused::SequenceExhausted(SequenceExhausted {
            stream_id: request.stream_id
        }))
    );
    assert!(!output.usage_recorded());
}

#[test]
fn no_usage_is_recorded_while_a_span_is_not_committed() {
    let mut output = TranscriptOutput::start(&adapter(Vec::new()), request());
    output.end_input().expect("input ends once");
    let first = output.hypothesis("one".to_owned()).expect("a span");
    let first = first.payload.span_id().expect("a span event");
    output.commit(first).expect("the hypothesis commits");
    let last = output.hypothesis("two".to_owned()).expect("a span");
    let last = last.payload.span_id().expect("a span event");

    assert_eq!(
        output.usage().map(|_| ()),
        Err(UsageRefused::SpanNotCommitted(SpanNotCommitted {
            span_id: last
        })),
        "the last span is not committed"
    );
    assert!(!output.usage_recorded());
    assert_eq!(output.next_sequence(), Some(4), "no sequence used");

    output.commit(last).expect("the last hypothesis commits");
    let envelope = output.usage().expect("every span is committed");
    assert_eq!(envelope.sequence, 5, "right after the last commit");
}

#[test]
fn after_the_usage_record_the_recognition_emits_nothing_more() {
    let mut output = TranscriptOutput::start(&adapter(Vec::new()), request());
    output.end_input().expect("input ends once");
    let hypothesis = output.hypothesis("hello".to_owned()).expect("a span");
    let span_id = hypothesis.payload.span_id().expect("a span event");
    output.commit(span_id).expect("the hypothesis commits");
    let usage = output.usage().expect("every span is committed");
    assert_eq!(usage.sequence, 3);

    assert_eq!(
        output.hypothesis("more".to_owned()).map(|_| ()),
        Err(HypothesisRefused::RecognitionEnded(RecognitionEnded))
    );
    assert_eq!(
        output
            .revise(ReviseSpan {
                span_id,
                text: "rewritten".to_owned(),
            })
            .map(|_| ()),
        Err(RevisionRefused::RecognitionEnded(RecognitionEnded))
    );
    assert_eq!(
        output
            .revise(ReviseSpan {
                span_id: SpanId::generate(),
                text: "unknown".to_owned(),
            })
            .map(|_| ()),
        Err(RevisionRefused::RecognitionEnded(RecognitionEnded)),
        "the end is refused first, whatever the span"
    );
    assert_eq!(
        output.commit(span_id).map(|_| ()),
        Err(CommitRefused::RecognitionEnded(RecognitionEnded))
    );
    assert_eq!(
        output.usage().map(|_| ()),
        Err(UsageRefused::UsageAlreadyRecorded(UsageAlreadyRecorded))
    );
    assert_eq!(output.next_sequence(), Some(4), "no sequence used");
    assert_eq!(output.spans().len(), 1, "no span added");
    assert_eq!(output.spans()[0].text(), "hello", "its text unchanged");
}

#[test]
fn a_failed_backend_call_records_what_was_measured_with_counts_not_final() {
    let mut output = TranscriptOutput::start(&adapter(Vec::new()), request());
    output
        .offer(frame(0, 16_000, 16_000, 1))
        .expect("input has not ended");
    output.end_input().expect("input ends once");
    output.add_local_compute(Duration::from_millis(12));
    assert!(!output.failed());
    output.mark_failed();
    assert!(output.failed());

    let envelope = output.usage().expect("a failed call is still counted");

    let record = recorded(&envelope.payload);
    assert!(!record.r#final, "the counts of a failed call are not final");
    assert_eq!(
        quantity(record, QuantityKind::InputAudioMs),
        Quantity::measured(QuantityKind::InputAudioMs, 1_000)
    );
    assert_eq!(
        quantity(record, QuantityKind::LocalComputeMs),
        Quantity::measured(QuantityKind::LocalComputeMs, 12)
    );
    assert_eq!(
        quantity(record, QuantityKind::Requests),
        Quantity::measured(QuantityKind::Requests, 1)
    );
}
