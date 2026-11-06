//! A recognition's usage counts the audio its implementation took, not the audio offered
//! (`story:contract-usage-and-naming-drift`), as a synthesis counts the characters it took: a
//! recogniser that failed before taking its tail reports only the audio it took.

use std::num::{NonZeroU16, NonZeroU32};

use inference_stt::inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_stt::inference_core::id::{NodeId, RunId, StreamId};
use inference_stt::inference_core::model::{
    Adapter, AdapterDescriptor, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_stt::inference_core::usage::{Quantity, QuantityKind};
use inference_stt::inference_text::TurnId;
use inference_stt::{CONTRACT_REVISION, RecognitionRequest, TranscriptEvent, TranscriptOutput};

fn output(capabilities: Vec<Capability>) -> TranscriptOutput {
    let adapter = Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: "test".to_owned(),
        contract_revision: CONTRACT_REVISION.to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities,
        max_concurrency: 1,
        descriptor: AdapterDescriptor::default(),
    })
    .expect("one is positive")
    .0;
    TranscriptOutput::start(
        &adapter,
        RecognitionRequest::new(
            RunId::generate(),
            TurnId::generate(),
            StreamId::generate(),
            NodeId::generate(),
            "text_out",
        ),
    )
}

/// A frame of 100 ms at 16 kHz, mono, at `index` times 100 ms.
fn tenth(index: u64) -> AudioFrame {
    let format = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).expect("not zero"),
        channels: NonZeroU16::new(1).expect("not zero"),
    };
    AudioFrame {
        format,
        position: SampleClockPosition::new(index * 1_600).expect("in range"),
        sample_count: 1_600,
        data: vec![0; usize::try_from(format.data_len(1_600)).expect("a small frame")],
    }
}

fn input_audio(output: &mut TranscriptOutput) -> (Quantity, bool) {
    let envelope = output.usage().expect("input has ended");
    let TranscriptEvent::Usage(recorded) = envelope.payload else {
        panic!("a usage record")
    };
    let audio = recorded
        .quantities
        .iter()
        .copied()
        .find(|quantity| quantity.kind() == QuantityKind::InputAudioMs)
        .expect("an input_audio_ms quantity");
    (audio, recorded.r#final)
}

#[test]
fn a_recogniser_failed_before_taking_its_tail_reports_only_the_audio_it_took() {
    let mut output = output(vec![Capability::IncrementalInput]);
    for index in 0..10 {
        output.offer(tenth(index)).expect("input has not ended");
    }
    assert_eq!(output.take_input().map(|frames| frames.len()), Ok(10));
    for index in 10..15 {
        output.offer(tenth(index)).expect("input has not ended");
    }
    output.end_input().expect("input ends once");
    output.mark_failed();
    assert_eq!(output.input_audio_ms(), 1_000, "one second taken of 1.5");

    let (audio, is_final) = input_audio(&mut output);

    assert_eq!(
        audio,
        Quantity::measured(QuantityKind::InputAudioMs, 1_000),
        "the tail never taken is not counted (story:contract-usage-and-naming-drift)"
    );
    assert!(!is_final, "a failed call's counts are not final");
}

#[test]
fn audio_offered_counts_once_it_is_taken() {
    let mut output = output(Vec::new());
    for index in 0..5 {
        output.offer(tenth(index)).expect("input has not ended");
    }
    assert_eq!(output.input_audio_ms(), 0, "offered, not taken");
    output.end_input().expect("input ends once");
    assert_eq!(output.take_input().map(|frames| frames.len()), Ok(5));
    assert_eq!(output.input_audio_ms(), 500);
    let (audio, is_final) = input_audio(&mut output);
    assert_eq!(audio, Quantity::measured(QuantityKind::InputAudioMs, 500));
    assert!(is_final);
}
