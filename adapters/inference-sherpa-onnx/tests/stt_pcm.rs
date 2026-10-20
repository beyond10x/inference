//! Decoding of every `AudioEncoding` to f32 and down-mixing to mono, as the recogniser hands
//! frames to sherpa-onnx. Needs no weights.
#![cfg(feature = "stt")]

use std::num::{NonZeroU16, NonZeroU32};

use inference_sherpa_onnx::stt::mono_samples;
use inference_stt::inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};

fn frame(encoding: AudioEncoding, channels: u16, sample_count: u32, data: Vec<u8>) -> AudioFrame {
    AudioFrame {
        format: AudioFormat {
            encoding,
            sample_rate: NonZeroU32::new(16_000).expect("16000 is not zero"),
            channels: NonZeroU16::new(channels).expect("channels are not zero"),
        },
        position: SampleClockPosition::new(0).expect("0 is a position"),
        sample_count,
        data,
    }
}

fn s16(samples: &[i16]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect()
}

fn f32le(samples: &[f32]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect()
}

#[test]
fn pcm_s16le_mono_is_scaled_to_unit_range() {
    let decoded = mono_samples(&frame(
        AudioEncoding::PcmS16le,
        1,
        4,
        s16(&[0, 16_384, -32_768, 32_767]),
    ));
    assert_eq!(decoded, vec![0.0, 0.5, -1.0, 32_767.0 / 32_768.0]);
}

#[test]
fn pcm_s16le_stereo_averages_the_channels() {
    let decoded = mono_samples(&frame(
        AudioEncoding::PcmS16le,
        2,
        2,
        s16(&[16_384, 0, -16_384, -16_384]),
    ));
    assert_eq!(decoded, vec![0.25, -0.5]);
}

#[test]
fn pcm_f32le_passes_samples_through_and_averages_three_channels() {
    let decoded = mono_samples(&frame(
        AudioEncoding::PcmF32le,
        3,
        2,
        f32le(&[0.25, 0.5, 0.75, -1.0, 0.0, 1.0]),
    ));
    assert_eq!(decoded, vec![0.5, 0.0]);
}

#[test]
fn pcm_mulaw_decodes_g711() {
    let decoded = mono_samples(&frame(
        AudioEncoding::PcmMulaw,
        1,
        4,
        vec![0xFF, 0x7F, 0x80, 0x00],
    ));
    assert_eq!(
        decoded,
        vec![0.0, 0.0, 32_124.0 / 32_768.0, -32_124.0 / 32_768.0]
    );
}

#[test]
fn pcm_alaw_decodes_g711() {
    let decoded = mono_samples(&frame(
        AudioEncoding::PcmAlaw,
        1,
        4,
        vec![0xD5, 0x55, 0xAA, 0x2A],
    ));
    assert_eq!(
        decoded,
        vec![
            8.0 / 32_768.0,
            -8.0 / 32_768.0,
            32_256.0 / 32_768.0,
            -32_256.0 / 32_768.0
        ]
    );
}

#[test]
fn silence_of_every_encoding_decodes_to_near_zero() {
    for encoding in AudioEncoding::ALL {
        let width = usize::try_from(encoding.width()).expect("a width fits usize");
        let decoded = mono_samples(&frame(
            encoding,
            2,
            8,
            vec![encoding.silence(); 8 * 2 * width],
        ));
        assert_eq!(
            decoded.len(),
            8,
            "{encoding:?} decodes one sample per group"
        );
        assert!(
            decoded.iter().all(|sample| sample.abs() <= 8.0 / 32_768.0),
            "{encoding:?} silence decodes to {decoded:?}"
        );
    }
}

#[test]
fn a_trailing_partial_sample_group_and_samples_past_the_count_are_not_decoded() {
    let mut data = s16(&[100, 200, 300, 400, 500, 600]);
    data.push(7);
    let decoded = mono_samples(&frame(AudioEncoding::PcmS16le, 2, 2, data));
    assert_eq!(decoded, vec![150.0 / 32_768.0, 350.0 / 32_768.0]);

    let short = mono_samples(&frame(AudioEncoding::PcmS16le, 1, 4, s16(&[1_024])));
    assert_eq!(short, vec![1_024.0 / 32_768.0]);
}
