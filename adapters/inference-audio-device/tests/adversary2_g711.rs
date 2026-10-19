//! Adversary pass 2: G.711 encoding at the values the standard's reference coder gives.
//!
//! The crate's own G.711 cases round-trip bytes through the decoder, whose largest magnitude is
//! 32_124 (mu-law) and 32_256 (A-law), so no case reaches the mu-law clip at 32_635 and no case
//! names an expected byte for a sample a device gives. Expected bytes here are worked by hand from
//! ITU-T G.711 as the classic reference coder (`linear2ulaw`, `linear2alaw`) implements it.

use std::num::{NonZeroU16, NonZeroU32};

use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_audio_device::convert::{decode, encode};

fn steps(values: &[i16]) -> Vec<f32> {
    values
        .iter()
        .map(|value| f32::from(*value) / 32_767.0)
        .collect()
}

fn frame(encoding: AudioEncoding, bytes: &[u8]) -> AudioFrame {
    AudioFrame {
        format: AudioFormat {
            encoding,
            sample_rate: NonZeroU32::new(8_000).expect("a rate"),
            channels: NonZeroU16::MIN,
        },
        position: SampleClockPosition::new(0).expect("a position"),
        sample_count: u32::try_from(bytes.len()).expect("short"),
        data: bytes.to_vec(),
    }
}

#[test]
fn mu_law_encodes_full_scale_and_mid_level_samples_as_g711_does() {
    assert_eq!(
        encode(
            &steps(&[0, 32_767, -32_767, 1_000, -1_000]),
            AudioEncoding::PcmMulaw
        ),
        vec![0xFF, 0x80, 0x00, 0xCE, 0x4E],
        "silence, the loudest positive and negative bytes (full scale clips at 32_635), and 1000"
    );
}

#[test]
fn a_law_encodes_full_scale_and_mid_level_samples_as_g711_does() {
    assert_eq!(
        encode(
            &steps(&[0, 32_767, -32_767, 1_000, -1_000]),
            AudioEncoding::PcmAlaw
        ),
        vec![0xD5, 0xAA, 0x2A, 0xFA, 0x7A],
    );
}

#[test]
fn g711_bytes_decode_to_the_reference_coders_linear_values() {
    let mulaw =
        decode(&frame(AudioEncoding::PcmMulaw, &[0x80, 0x00, 0xCE, 0xFF])).expect("a frame");
    let alaw = decode(&frame(AudioEncoding::PcmAlaw, &[0xAA, 0x2A, 0xFA, 0xD5])).expect("a frame");
    let back = |samples: Vec<f32>| -> Vec<i32> {
        samples
            .into_iter()
            .map(|sample| (sample * 32_767.0).round() as i32)
            .collect()
    };
    assert_eq!(back(mulaw), vec![32_124, -32_124, 988, 0]);
    assert_eq!(back(alaw), vec![32_256, -32_256, 1_008, 8]);
}
