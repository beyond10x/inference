//! Adversary pass 1 against format conversion: PCM16 extremes and the linear resampler at the
//! ratios a laptop meets, against a reference computed independently (`story:audio-device-adapter`,
//! Planned scope: format conversion and resampling).

use std::num::{NonZeroU16, NonZeroU32};

use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_audio_device::convert::{LinearResampler, decode, encode};

fn hz(rate: u32) -> NonZeroU32 {
    NonZeroU32::new(rate).expect("a rate above 0")
}

fn pcm16(values: &[i16]) -> AudioFrame {
    AudioFrame {
        format: AudioFormat {
            encoding: AudioEncoding::PcmS16le,
            sample_rate: hz(16_000),
            channels: NonZeroU16::MIN,
        },
        position: SampleClockPosition::new(0).expect("0 is a position"),
        sample_count: u32::try_from(values.len()).expect("a short frame"),
        data: values.iter().flat_map(|v| v.to_le_bytes()).collect(),
    }
}

/// Linear interpolation of `input` at output `k`, in f64: input time `k * from / to`.
fn reference(input: &[f32], from: u32, to: u32, k: usize) -> f64 {
    let time = k as f64 * f64::from(from) / f64::from(to);
    let index = time.floor() as usize;
    let fraction = time - time.floor();
    let first = f64::from(input[index]);
    if fraction == 0.0 {
        first
    } else {
        first + (f64::from(input[index + 1]) - first) * fraction
    }
}

#[test]
fn pcm16_extremes_decode_within_full_scale_one() {
    let decoded = decode(&pcm16(&[i16::MIN, i16::MAX, -32767, 0])).expect("a well-formed frame");
    assert_eq!(decoded, [-1.0, 1.0, -1.0, 0.0]);
}

#[test]
fn non_finite_samples_encode_as_silence_or_full_scale_in_pcm16() {
    let bytes = encode(
        &[f32::NAN, f32::INFINITY, f32::NEG_INFINITY],
        AudioEncoding::PcmS16le,
    );
    let values: Vec<i16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| i16::from_le_bytes(*pair))
        .collect();
    assert_eq!(values, [0, 32767, -32767]);
}

#[test]
fn the_resampler_matches_linear_interpolation_at_44_1k_to_16k_and_48k_to_24k() {
    let input: Vec<f32> = (0..44_100)
        .map(|n| ((n * 7919) % 10_007) as f32 / 10_007.0 - 0.5)
        .collect();
    for (from, to) in [
        (44_100, 16_000),
        (48_000, 24_000),
        (16_000, 44_100),
        (8_000, 48_000),
    ] {
        let mut resampler = LinearResampler::new(hz(from), hz(to));
        let mut output = Vec::new();
        for chunk in input.chunks(333) {
            resampler.process(chunk, &mut output);
        }
        let last = input.len() - 1;
        let expected = (0_u64..)
            .take_while(|&k| {
                let numerator = k as u128 * u128::from(from);
                let to = u128::from(to);
                numerator / to < last as u128 || numerator == last as u128 * to
            })
            .count();
        assert_eq!(output.len(), expected, "{from} to {to} Hz: output count");
        for (k, value) in output.iter().enumerate() {
            let want = reference(&input, from, to, k);
            assert!(
                (f64::from(*value) - want).abs() < 1e-5,
                "{from} to {to} Hz: output {k} is {value}, not {want}"
            );
        }
    }
}
