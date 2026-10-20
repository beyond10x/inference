//! Format conversion and resampling without audio hardware: down-mixing, PCM encoding and
//! decoding of frames, and the linear resampler (`story:audio-device-adapter`, Planned scope:
//! format conversion and resampling).

use std::f32::consts::TAU;
use std::num::{NonZeroU16, NonZeroU32};

use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_audio_device::convert::{LinearResampler, MalformedFrame, decode, downmix, encode};

fn hz(rate: u32) -> NonZeroU32 {
    NonZeroU32::new(rate).expect("a rate above 0")
}

fn frame(encoding: AudioEncoding, channels: u16, sample_count: u32, data: Vec<u8>) -> AudioFrame {
    AudioFrame {
        format: AudioFormat {
            encoding,
            sample_rate: hz(16_000),
            channels: NonZeroU16::new(channels).expect("a channel count above 0"),
        },
        position: SampleClockPosition::new(0).expect("0 is a position"),
        sample_count,
        data,
    }
}

fn resample(from: u32, to: u32, input: &[f32]) -> Vec<f32> {
    let mut resampler = LinearResampler::new(hz(from), hz(to));
    let mut output = Vec::new();
    resampler.process(input, &mut output);
    output
}

/// How many times `samples` crosses zero upwards.
fn rising_crossings(samples: &[f32]) -> usize {
    samples
        .windows(2)
        .filter(|pair| pair[0] < 0.0 && pair[1] >= 0.0)
        .count()
}

fn sine(frequency: f32, rate: u32, samples: usize) -> Vec<f32> {
    (0..samples)
        .map(|n| (TAU * frequency * n as f32 / rate as f32).sin() * 0.5)
        .collect()
}

#[test]
fn downmix_averages_each_whole_frame_and_ignores_a_trailing_partial_one() {
    let mut mono = vec![9.0];
    downmix(&[0.2, 0.4, -1.0, 1.0, 0.5], 2, &mut mono);
    assert_eq!(mono.len(), 3, "one kept sample and two whole stereo frames");
    assert!((mono[1] - 0.3).abs() < 1e-6, "{mono:?}");
    assert!(mono[2].abs() < 1e-6, "{mono:?}");
    let mut mono = Vec::new();
    downmix(&[0.1, -0.2, 0.3], 1, &mut mono);
    assert_eq!(mono, [0.1, -0.2, 0.3], "a mono device passes through");
}

#[test]
fn pcm16_encoding_clamps_to_full_scale_and_rounds_to_the_nearest_step() {
    let bytes = encode(
        &[0.0, 1.0, -1.0, 2.0, -2.0, 0.5, 0.375],
        AudioEncoding::PcmS16le,
    );
    let values: Vec<i16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| i16::from_le_bytes(*pair))
        .collect();
    assert_eq!(values, [0, 32767, -32767, 32767, -32767, 16384, 12288]);
}

#[test]
fn pcm_f32_encoding_keeps_each_value_bit_for_bit_little_endian() {
    let samples = [0.0_f32, -0.75, 0.123_456_79, 1.5];
    let bytes = encode(&samples, AudioEncoding::PcmF32le);
    let expected: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    assert_eq!(bytes, expected);
}

#[test]
fn decoding_round_trips_pcm16_and_pcm_f32_and_downmixes_a_stereo_frame() {
    let samples = [0.0_f32, 0.5, -0.5, 0.999, -1.0];
    let pcm16 = frame(
        AudioEncoding::PcmS16le,
        1,
        5,
        encode(&samples, AudioEncoding::PcmS16le),
    );
    let decoded = decode(&pcm16).expect("a well-formed frame");
    assert_eq!(decoded.len(), 5);
    for (got, want) in decoded.iter().zip(samples) {
        assert!((got - want).abs() <= 1.0 / 32767.0, "{got} against {want}");
    }
    let f32_frame = frame(
        AudioEncoding::PcmF32le,
        1,
        5,
        encode(&samples, AudioEncoding::PcmF32le),
    );
    assert_eq!(decode(&f32_frame).expect("a well-formed frame"), samples);
    let stereo = frame(
        AudioEncoding::PcmF32le,
        2,
        2,
        encode(&[0.5, 0.25, -1.0, 0.0], AudioEncoding::PcmF32le),
    );
    assert_eq!(decode(&stereo).expect("a well-formed frame"), [0.375, -0.5]);
}

#[test]
fn decoding_refuses_a_frame_whose_data_does_not_match_its_format_and_sample_count() {
    let short = frame(AudioEncoding::PcmS16le, 2, 4, vec![0; 15]);
    assert_eq!(
        decode(&short),
        Err(MalformedFrame {
            expected: 16,
            actual: 15
        })
    );
}

#[test]
fn equal_rates_pass_every_sample_through_unchanged() {
    let input = sine(440.0, 16_000, 999);
    assert_eq!(resample(16_000, 16_000, &input), input);
}

#[test]
fn downsampling_a_ramp_lands_on_every_third_input_sample() {
    let input: Vec<f32> = (0..4800).map(|j| j as f32 * 1e-4).collect();
    let output = resample(48_000, 16_000, &input);
    assert_eq!(output.len(), 4799 / 3 + 1);
    for (k, value) in output.iter().enumerate() {
        assert!((value - input[3 * k]).abs() < 1e-6, "output {k} is {value}");
    }
}

#[test]
fn upsampling_a_ramp_interpolates_between_input_samples() {
    let input: Vec<f32> = (0..320).map(|j| j as f32 * 1e-3).collect();
    let output = resample(16_000, 48_000, &input);
    assert_eq!(
        output.len(),
        319 * 3 + 1,
        "every output up to the last input"
    );
    for (k, value) in output.iter().enumerate() {
        let want = k as f32 / 3.0 * 1e-3;
        assert!(
            (value - want).abs() < 1e-5,
            "output {k} is {value}, not {want}"
        );
    }
}

#[test]
fn a_constant_level_stays_the_same_level_at_any_ratio() {
    for (from, to) in [
        (48_000, 16_000),
        (44_100, 16_000),
        (16_000, 48_000),
        (8_000, 44_100),
    ] {
        let output = resample(from, to, &[0.5; 2000]);
        assert!(!output.is_empty());
        assert!(
            output.iter().all(|value| (value - 0.5).abs() < 1e-6),
            "{from} to {to} Hz changed the level"
        );
    }
}

#[test]
fn fed_in_uneven_chunks_the_resampler_produces_exactly_what_it_does_fed_at_once() {
    let input = sine(1000.0, 44_100, 44_100);
    let whole = resample(44_100, 16_000, &input);
    let mut resampler = LinearResampler::new(hz(44_100), hz(16_000));
    let mut chunked = Vec::new();
    let mut rest = input.as_slice();
    for size in [1, 2, 3, 441, 7, 1024, 13].into_iter().cycle() {
        if rest.is_empty() {
            break;
        }
        let (chunk, after) = rest.split_at(size.min(rest.len()));
        resampler.process(chunk, &mut chunked);
        rest = after;
    }
    assert_eq!(chunked, whole);
    assert_eq!(resampler.received(), 44_100);
    assert_eq!(resampler.produced(), whole.len() as u64);
}

#[test]
fn a_tone_keeps_its_frequency_through_downsampling_and_upsampling() {
    let tone = sine(1000.0, 48_000, 48_000);
    let down = resample(48_000, 16_000, &tone);
    assert_eq!(down.len(), 16_000);
    assert!(
        (rising_crossings(&down) as i64 - 1000).abs() <= 1,
        "{} rising crossings in one second at 16 kHz",
        rising_crossings(&down)
    );
    let up = resample(16_000, 24_000, &down);
    assert!(
        (rising_crossings(&up) as i64 - 1000).abs() <= 1,
        "{} rising crossings in one second at 24 kHz",
        rising_crossings(&up)
    );
}
