//! Adversary pass 1 on `story:usage-stt`: the audio a recognition's usage record counts.
//! `input_audio_ms` is the audio offered, samples over sample rate, so a clip of a whole number of
//! milliseconds reports exactly that many, whatever frame size carried it; a frame refused after
//! the end of input is not counted; frames shorter than a millisecond add up.

use std::num::{NonZeroU16, NonZeroU32};

use inference_stt::inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_stt::inference_core::id::{NodeId, RunId, StreamId};
use inference_stt::inference_core::model::{
    Adapter, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_stt::inference_core::usage::{Quantity, QuantityKind};
use inference_stt::inference_text::TurnId;
use inference_stt::{
    CONTRACT_REVISION, InputEnded, RecognitionRequest, TranscriptEvent, TranscriptOutput,
};

fn output() -> TranscriptOutput {
    let adapter = Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: "test".to_owned(),
        contract_revision: CONTRACT_REVISION.to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities: Vec::new(),
        max_concurrency: 1,
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

fn frame(position: u64, sample_count: u32, sample_rate: u32) -> AudioFrame {
    let format = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(sample_rate).expect("not zero"),
        channels: NonZeroU16::new(1).expect("not zero"),
    };
    AudioFrame {
        format,
        position: SampleClockPosition::new(position).expect("in range"),
        sample_count,
        data: vec![0; usize::try_from(format.data_len(sample_count)).expect("a small frame")],
    }
}

/// Offers `count` frames of `sample_count` samples at `sample_rate`, back to back.
fn offer(output: &mut TranscriptOutput, count: u64, sample_count: u32, sample_rate: u32) {
    for index in 0..count {
        output
            .offer(frame(
                index * u64::from(sample_count),
                sample_count,
                sample_rate,
            ))
            .expect("input has not ended");
    }
}

fn input_audio(output: &mut TranscriptOutput) -> Quantity {
    output.end_input().expect("input ends once");
    let envelope = output.usage().expect("input has ended");
    let TranscriptEvent::Usage(recorded) = envelope.payload else {
        panic!("a usage record")
    };
    recorded
        .quantities
        .into_iter()
        .find(|quantity| quantity.kind() == QuantityKind::InputAudioMs)
        .expect("an input_audio_ms quantity")
}

#[test]
fn ten_seconds_at_44_1_khz_in_frames_that_are_no_whole_nanosecond_count_as_10000_ms() {
    let mut output = output();
    offer(&mut output, 420, 1_050, 44_100);
    assert_eq!(
        input_audio(&mut output),
        Quantity::measured(QuantityKind::InputAudioMs, 10_000),
        "420 x 1050 samples at 44.1 kHz are 441000 samples, exactly 10 s"
    );
}

#[test]
fn one_second_at_48_khz_in_single_sample_frames_counts_as_1000_ms() {
    let mut output = output();
    offer(&mut output, 48_000, 1, 48_000);
    assert_eq!(
        output.input_audio_ms(),
        1_000,
        "48000 samples at 48 kHz are exactly 1 s"
    );
}

#[test]
fn frames_shorter_than_a_millisecond_add_up() {
    let mut output = output();
    offer(&mut output, 4, 8, 16_000);
    assert_eq!(
        input_audio(&mut output),
        Quantity::measured(QuantityKind::InputAudioMs, 2),
        "four frames of 0.5 ms are 2 ms"
    );
}

#[test]
fn a_frame_refused_after_the_end_of_input_is_not_counted() {
    let mut output = output();
    offer(&mut output, 10, 160, 16_000);
    output.end_input().expect("input ends once");
    assert_eq!(output.offer(frame(1_600, 16_000, 16_000)), Err(InputEnded));
    assert_eq!(
        output.input_audio_ms(),
        100,
        "the refused second is not offered"
    );
    let envelope = output.usage().expect("input has ended");
    let TranscriptEvent::Usage(recorded) = envelope.payload else {
        panic!("a usage record")
    };
    assert!(
        recorded
            .quantities
            .contains(&Quantity::measured(QuantityKind::InputAudioMs, 100)),
        "{recorded:?}"
    );
}
