//! Adversary, `story:contract-usage-and-naming-drift` (N13): a recognition's usage counts the
//! audio its implementation took. A synthesis refuses its usage record while a segment is still
//! Queued and the call is not marked failed (`SegmentNotSynthesized`), so a final synthesis record
//! never leaves input out. A final recognition record must not either: a call not marked failed
//! that records usage with offered frames never taken either is refused, or counts them.

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
        backend: "adversary".to_owned(),
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

#[test]
fn a_final_record_never_leaves_out_audio_offered_and_not_taken() {
    let mut output = output(vec![Capability::IncrementalInput]);
    for index in 0..10 {
        output.offer(tenth(index)).expect("input has not ended");
    }
    assert_eq!(output.take_input().map(|frames| frames.len()), Ok(10));
    for index in 10..15 {
        output.offer(tenth(index)).expect("input has not ended");
    }
    output.end_input().expect("input ends once");

    let Ok(envelope) = output.usage() else {
        return;
    };
    let TranscriptEvent::Usage(recorded) = envelope.payload else {
        panic!("a usage record")
    };
    let audio = recorded
        .quantities
        .iter()
        .copied()
        .find(|quantity| quantity.kind() == QuantityKind::InputAudioMs)
        .expect("an input_audio_ms quantity");
    assert!(
        !recorded.r#final || audio == Quantity::measured(QuantityKind::InputAudioMs, 1_500),
        "a call not marked failed recorded final usage of {audio:?} for 1.5 s offered: the tail \
         offered and never taken is left out of a record that says its counts are settled, where \
         a synthesis refuses its record while a segment is not taken \
         (story:contract-usage-and-naming-drift)"
    );
}
