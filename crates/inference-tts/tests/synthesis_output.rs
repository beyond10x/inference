//! The bookkeeping every synthesiser shares, read without a mock: input is taken before the
//! utterance is complete only where the adapter claims `incremental_input`, audio of a Queued
//! segment is emitted only where it claims `output_streaming`, and only for a segment taken, in a
//! frame whose data length matches its sample count, channels and encoding width.

use std::num::{NonZeroU16, NonZeroU32};

use inference_tts::inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_tts::inference_core::generation::{Generation, StartGeneration};
use inference_tts::inference_core::id::{NodeId, RunId};
use inference_tts::inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_tts::inference_text::{OutputSegment, SegmentState, SegmentStateConflict};
use inference_tts::{
    AudioRefused, CONTRACT_REVISION, CapabilityNotClaimed, CompletionRefused, FrameLengthMismatch,
    OfferRefused, SegmentNotTaken, SynthesisEvent, SynthesisOutput, UtteranceEnded,
};

fn adapter(capabilities: Vec<Capability>) -> Adapter {
    Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: "contract test".to_owned(),
        contract_revision: CONTRACT_REVISION.to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities,
        max_concurrency: 1,
    })
    .expect("a maximum concurrency of one is positive")
    .0
}

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

fn frame(sample_count: u32, bytes: usize) -> AudioFrame {
    AudioFrame {
        format: AudioFormat {
            encoding: AudioEncoding::PcmS16le,
            sample_rate: NonZeroU32::new(16_000).expect("not zero"),
            channels: NonZeroU16::new(1).expect("not zero"),
        },
        position: SampleClockPosition::new(0).expect("in range"),
        sample_count,
        data: vec![0; bytes],
    }
}

#[test]
fn input_is_taken_before_the_utterance_ends_only_where_incremental_input_is_claimed() {
    let segment = committed("hello");

    let mut batch = SynthesisOutput::start(&adapter(Vec::new()));
    batch.offer(segment.clone()).expect("queued");
    assert_eq!(
        batch.take_input(),
        Err(CapabilityNotClaimed {
            capability: Capability::IncrementalInput,
        })
    );
    assert_eq!(batch.segments_taken(), 0);
    batch.end_utterance().expect("ends once");
    assert_eq!(batch.end_utterance(), Err(UtteranceEnded));
    let taken = batch.take_input().expect("after the end, input is taken");
    assert_eq!(taken.len(), 1);
    assert_eq!(taken[0].state(), SegmentState::Queued);

    let mut incremental = SynthesisOutput::start(&adapter(vec![Capability::IncrementalInput]));
    incremental.offer(segment.clone()).expect("queued");
    assert_eq!(incremental.take_input().map(|taken| taken.len()), Ok(1));
    assert_eq!(incremental.take_input().map(|taken| taken.len()), Ok(0));
    assert_eq!(incremental.segments_taken(), 1);
}

#[test]
fn audio_of_a_queued_segment_is_emitted_only_where_output_streaming_is_claimed() {
    let segment = committed("hello");
    let id = segment.segment_id();

    let mut batch = SynthesisOutput::start(&adapter(Vec::new()));
    batch.offer(segment.clone()).expect("queued");
    batch.end_utterance().expect("ends once");
    assert_eq!(
        batch.audio(id, frame(2, 4)),
        Err(AudioRefused::SegmentNotTaken(SegmentNotTaken {
            segment_id: id
        })),
        "no audio for a segment the implementation has not taken"
    );
    batch.take_input().expect("after the end");
    assert_eq!(
        batch.audio(id, frame(2, 4)),
        Err(AudioRefused::CapabilityNotClaimed(CapabilityNotClaimed {
            capability: Capability::OutputStreaming,
        }))
    );
    assert_eq!(
        batch.complete(id, vec![frame(2, 4), frame(2, 3)]),
        Err(CompletionRefused::FrameLengthMismatch(
            FrameLengthMismatch {
                expected: 4,
                actual: 3,
            }
        ))
    );
    assert_eq!(
        batch.segment(id).map(OutputSegment::state),
        Some(SegmentState::Queued)
    );
    let completed = batch
        .complete(id, vec![frame(2, 4), frame(2, 4)])
        .expect("Queued to Synthesized");
    assert!(matches!(&completed[0], SynthesisEvent::Completed(event) if event.segment_id == id));
    assert_eq!(
        completed.len(),
        3,
        "SynthesisCompleted, then the whole audio"
    );
    assert!(
        completed[1..]
            .iter()
            .all(|event| matches!(event, SynthesisEvent::Audio(audio) if audio.segment_id == id))
    );
    assert_eq!(batch.frames_emitted(id), Some(2));
    assert_eq!(
        batch.complete(id, Vec::new()),
        Err(CompletionRefused::SegmentStateConflict(
            SegmentStateConflict {
                current: SegmentState::Synthesized,
            }
        ))
    );
    assert_eq!(
        batch.audio(id, frame(2, 4)),
        Err(AudioRefused::SegmentStateConflict(SegmentStateConflict {
            current: SegmentState::Synthesized,
        })),
        "the audio of a Synthesized segment is final"
    );
    batch
        .acknowledge_playback(id)
        .expect("Synthesized to Played");
    assert_eq!(
        batch.audio(id, frame(2, 4)),
        Err(AudioRefused::SegmentStateConflict(SegmentStateConflict {
            current: SegmentState::Played,
        })),
        "no frame of a Played segment is emitted"
    );
    assert_eq!(batch.frames_emitted(id), Some(2));

    let mut streaming = SynthesisOutput::start(&adapter(vec![Capability::OutputStreaming]));
    streaming.offer(segment.clone()).expect("queued");
    streaming.end_utterance().expect("ends once");
    streaming.take_input().expect("after the end");
    assert!(streaming.audio(id, frame(2, 4)).is_ok());
    assert_eq!(
        streaming.segment(id).map(OutputSegment::state),
        Some(SegmentState::Queued),
        "streamed audio does not complete the synthesis"
    );
}

#[test]
fn a_frame_whose_data_length_disagrees_with_its_samples_is_refused() {
    let segment = committed("hello");
    let id = segment.segment_id();
    let mut output = SynthesisOutput::start(&adapter(vec![Capability::OutputStreaming]));
    output.offer(segment.clone()).expect("queued");
    output.end_utterance().expect("ends once");
    output.take_input().expect("after the end");

    for bytes in [0, 3, 5, 8] {
        assert_eq!(
            output.audio(id, frame(2, bytes)),
            Err(AudioRefused::FrameLengthMismatch(FrameLengthMismatch {
                expected: 4,
                actual: bytes as u64,
            })),
            "2 samples of mono pcm_s16le are 4 bytes"
        );
    }
}

#[test]
fn a_segment_is_queued_once_and_not_after_the_utterance_ends() {
    let segment = committed("hello");
    let mut output = SynthesisOutput::start(&adapter(Vec::new()));
    let queued = output.offer(segment.clone()).expect("queued");
    assert_eq!(queued.segment_id, segment.segment_id());
    assert_eq!(
        output.offer(segment.clone()),
        Err(OfferRefused::SegmentStateConflict(SegmentStateConflict {
            current: SegmentState::Queued,
        })),
        "the synthesis already holds the segment Queued"
    );
    assert_eq!(output.segments().len(), 1);

    output.end_utterance().expect("ends once");
    assert_eq!(
        output.offer(committed("late")),
        Err(OfferRefused::UtteranceEnded(UtteranceEnded))
    );
    let stranger = committed("stranger").segment_id();
    assert_eq!(
        output.complete(stranger, Vec::new()),
        Err(CompletionRefused::SegmentNotTaken(SegmentNotTaken {
            segment_id: stranger,
        }))
    );
}
