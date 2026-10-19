//! `TranscriptOutput` holds one recognition under the transitions of `inference-text`: frames are
//! given up before the end of input only when the adapter claims `incremental_input`, a span is
//! emitted before it only when the adapter claims `output_streaming`, a committed span refuses a
//! revision and keeps its text, and every event is enveloped at the next sequence of the
//! request's stream, a refused move using none.

use std::num::{NonZeroU16, NonZeroU32};

use inference_stt::inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_stt::inference_core::envelope::Timebase;
use inference_stt::inference_core::id::{NodeId, RunId, StreamId};
use inference_stt::inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_stt::inference_text::{
    CommittedSpanImmutable, ReviseSpan, SpanId, SpanNotFound, SpanState, SpanStateConflict, TurnId,
};
use inference_stt::{
    CONTRACT_REVISION, CapabilityNotClaimed, CommitRefused, InputEnded, RecognitionRequest,
    RevisionRefused, TranscriptEvent, TranscriptOutput,
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

fn frame(position: u64, sample_count: u32) -> AudioFrame {
    AudioFrame {
        format: AudioFormat {
            encoding: AudioEncoding::PcmS16le,
            sample_rate: NonZeroU32::new(8_000).expect("not zero"),
            channels: NonZeroU16::new(1).expect("not zero"),
        },
        position: SampleClockPosition::new(position).expect("in range"),
        sample_count,
        data: vec![0; sample_count as usize * 2],
    }
}

fn output(capabilities: Vec<Capability>) -> TranscriptOutput {
    TranscriptOutput::start(&adapter(capabilities), request())
}

#[test]
fn input_is_given_up_before_the_end_only_when_incremental_input_is_claimed() {
    let mut batch = output(Vec::new());
    batch.offer(frame(0, 80)).expect("input has not ended");
    assert_eq!(
        batch.take_input(),
        Err(CapabilityNotClaimed {
            capability: Capability::IncrementalInput,
        })
    );
    assert_eq!(batch.frames_taken(), 0);
    batch.end_input().expect("input ends once");
    assert_eq!(batch.take_input(), Ok(vec![frame(0, 80)]));
    assert_eq!(batch.frames_taken(), 1);

    let mut incremental = output(vec![Capability::IncrementalInput]);
    incremental
        .offer(frame(0, 80))
        .expect("input has not ended");
    assert_eq!(incremental.take_input(), Ok(vec![frame(0, 80)]));
    assert_eq!(incremental.frames_taken(), 1);
}

#[test]
fn a_span_is_emitted_before_the_end_only_when_output_streaming_is_claimed() {
    let mut batch = output(vec![Capability::IncrementalInput]);
    assert_eq!(
        batch.hypothesis("early".to_owned()),
        Err(CapabilityNotClaimed {
            capability: Capability::OutputStreaming,
        })
    );
    assert!(batch.spans().is_empty());
    batch.end_input().expect("input ends once");
    let envelope = batch
        .hypothesis("late".to_owned())
        .expect("input has ended");
    assert_eq!(
        envelope.sequence, 1,
        "the refused hypothesis used no sequence"
    );

    let mut streaming = output(vec![Capability::OutputStreaming]);
    let envelope = streaming
        .hypothesis("early".to_owned())
        .expect("output_streaming is claimed");
    assert_eq!(envelope.sequence, 1);
}

#[test]
fn a_committed_span_refuses_a_revision_and_keeps_its_text() {
    let mut output = output(vec![Capability::OutputStreaming]);
    let span_id = output
        .hypothesis("hello".to_owned())
        .expect("streaming")
        .payload
        .span_id();
    output
        .revise(ReviseSpan {
            span_id,
            text: "hello there".to_owned(),
        })
        .expect("a Hypothesis is revised");
    output.commit(span_id).expect("a Hypothesis commits");

    assert_eq!(
        output.revise(ReviseSpan {
            span_id,
            text: "goodbye".to_owned(),
        }),
        Err(RevisionRefused::CommittedSpanImmutable(
            CommittedSpanImmutable { span_id }
        ))
    );
    let span = output.span(span_id).expect("held");
    assert_eq!(span.text(), "hello there");
    assert_eq!(span.state(), SpanState::Committed);

    assert_eq!(
        output.commit(span_id),
        Err(CommitRefused::SpanStateConflict(SpanStateConflict {
            current: SpanState::Committed,
        }))
    );

    let unknown = SpanId::generate();
    assert_eq!(
        output.revise(ReviseSpan {
            span_id: unknown,
            text: "x".to_owned(),
        }),
        Err(RevisionRefused::SpanNotFound(SpanNotFound {
            span_id: unknown
        }))
    );
    assert_eq!(
        output.commit(unknown),
        Err(CommitRefused::SpanNotFound(SpanNotFound {
            span_id: unknown
        }))
    );

    let next = output.hypothesis("next".to_owned()).expect("streaming");
    assert_eq!(next.sequence, 4, "the four refusals used no sequence");
}

#[test]
fn no_frame_is_offered_and_the_end_is_not_marked_after_input_ended() {
    let mut output = output(Vec::new());
    output.end_input().expect("input ends once");
    assert_eq!(output.end_input(), Err(InputEnded));
    assert_eq!(output.offer(frame(0, 80)), Err(InputEnded));
    assert_eq!(output.take_input(), Ok(Vec::new()));
}

#[test]
fn every_event_is_enveloped_on_the_request_stream_at_the_next_sequence() {
    let request = request();
    let mut output = TranscriptOutput::start(
        &adapter(vec![
            Capability::IncrementalInput,
            Capability::OutputStreaming,
        ]),
        request.clone(),
    );
    output.offer(frame(0, 80)).expect("open");
    output.offer(frame(80, 80)).expect("open");
    output.take_input().expect("incremental");

    let hypothesis = output.hypothesis("hi".to_owned()).expect("streaming");
    let span_id = hypothesis.payload.span_id();
    let revision = output
        .revise(ReviseSpan {
            span_id,
            text: "hi there".to_owned(),
        })
        .expect("a Hypothesis is revised");
    let committed = output.commit(span_id).expect("a Hypothesis commits");

    for (envelope, sequence, kind) in [
        (&hypothesis, 1, "inference.stream.TranscriptHypothesis"),
        (&revision, 2, "inference.stream.TranscriptRevision"),
        (&committed, 3, "inference.stream.TranscriptCommitted"),
    ] {
        assert_eq!(envelope.sequence, sequence);
        assert_eq!(envelope.kind, kind);
        assert_eq!(envelope.kind, envelope.payload.name());
        assert_eq!(envelope.run_id, request.run_id);
        assert_eq!(envelope.stream_id, request.stream_id);
        assert_eq!(envelope.producer, request.node_id);
        assert_eq!(envelope.port, request.port);
        assert_eq!(envelope.session_id, None);
        assert_eq!(envelope.generation_id, None);
        assert_eq!(envelope.causation_id, None);
        assert_eq!(envelope.timebase, Timebase::AudioFrameTime);
        assert_eq!(envelope.timestamp, 160, "just past the last sample taken");
    }
    assert_ne!(hypothesis.event_id, revision.event_id);
    assert_ne!(revision.event_id, committed.event_id);

    let span = output.span(span_id).expect("held");
    assert_eq!(span.sequence(), hypothesis.sequence);
    assert_eq!(span.turn_id(), request.turn_id);
    assert_eq!(span.run_id(), request.run_id);
    assert_eq!(span.stream_id(), request.stream_id);
    match committed.payload {
        TranscriptEvent::Committed(event) => assert_eq!(event.text, "hi there"),
        other => panic!("a commit envelopes TranscriptCommitted, got {other:?}"),
    }
}

#[test]
fn a_recognition_continuing_a_stream_numbers_its_envelopes_and_spans_from_the_given_sequence() {
    let fresh = request();
    assert_eq!(fresh.first_sequence, 1);
    let fresh_output = TranscriptOutput::start(&adapter(Vec::new()), fresh.clone());
    assert_eq!(fresh_output.next_sequence(), 1);

    let request = RecognitionRequest::continuing(
        fresh.run_id,
        TurnId::generate(),
        fresh.stream_id,
        7,
        fresh.node_id,
        "text_out",
    );
    assert_eq!(request.first_sequence, 7);
    let mut output = TranscriptOutput::start(&adapter(Vec::new()), request);
    assert_eq!(output.next_sequence(), 7);
    output.end_input().expect("input ends once");
    let hypothesis = output.hypothesis("hi".to_owned()).expect("input has ended");
    let span_id = hypothesis.payload.span_id();
    let committed = output.commit(span_id).expect("a Hypothesis commits");

    assert_eq!(hypothesis.sequence, 7);
    assert_eq!(committed.sequence, 8);
    assert_eq!(output.span(span_id).expect("held").sequence(), 7);
    assert_eq!(output.next_sequence(), 9);
}
