//! What the STT contract lets a recognition of the declared adapter do before the end of input:
//! nothing. With neither capability claimed, taking frames is refused with
//! `CapabilityNotClaimed` naming incremental input and emitting a span with one naming output
//! streaming; once input has ended, both pass. Needs no weights.

use std::num::{NonZeroU16, NonZeroU32};

use inference_stt::inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_stt::inference_core::id::{NodeId, RunId, StreamId};
use inference_stt::inference_core::model::{Adapter, Capability};
use inference_stt::inference_text::TurnId;
use inference_stt::{
    CapabilityNotClaimed, HypothesisRefused, RecognitionRequest, TranscriptOutput,
};
use inference_whispercpp::stt::{DEFAULT_MAX_CONCURRENCY, declare_adapter, declare_model};

fn adapter() -> Adapter {
    let (model, _) = declare_model();
    declare_adapter(model.model_id(), DEFAULT_MAX_CONCURRENCY)
        .expect("the default maximum concurrency is positive")
        .0
}

fn output() -> TranscriptOutput {
    TranscriptOutput::start(
        &adapter(),
        RecognitionRequest::new(
            RunId::generate(),
            TurnId::generate(),
            StreamId::generate(),
            NodeId::generate(),
            "text_out",
        ),
    )
}

fn frame() -> AudioFrame {
    AudioFrame {
        format: AudioFormat {
            encoding: AudioEncoding::PcmS16le,
            sample_rate: NonZeroU32::new(16_000).expect("16000 is not zero"),
            channels: NonZeroU16::new(1).expect("1 is not zero"),
        },
        position: SampleClockPosition::new(0).expect("0 is a position"),
        sample_count: 320,
        data: vec![0; 640],
    }
}

#[test]
fn take_input_before_the_end_of_input_is_refused_naming_incremental_input() {
    let mut output = output();
    output.offer(frame()).expect("input has not ended");
    assert_eq!(
        output.take_input(),
        Err(CapabilityNotClaimed {
            capability: Capability::IncrementalInput
        })
    );
    assert_eq!(output.frames_taken(), 0, "no frame is taken");

    output.end_input().expect("input ends once");
    assert_eq!(output.take_input().map(|frames| frames.len()), Ok(1));
    assert_eq!(output.frames_taken(), 1);
}

#[test]
fn a_hypothesis_before_the_end_of_input_is_refused_naming_output_streaming() {
    let mut output = output();
    assert_eq!(
        output.hypothesis("early".to_owned()).map(|_| ()),
        Err(HypothesisRefused::CapabilityNotClaimed(
            CapabilityNotClaimed {
                capability: Capability::OutputStreaming
            }
        ))
    );
    assert!(output.spans().is_empty(), "no span is added");
    assert_eq!(output.next_sequence(), Some(1), "no sequence is used");

    output.end_input().expect("input ends once");
    let envelope = output
        .hypothesis("late".to_owned())
        .expect("a span is emitted once input has ended");
    assert_eq!(envelope.sequence, 1);
}
