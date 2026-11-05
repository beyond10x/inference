//! The resampler cases of story `g711-codec-and-anti-aliased-resampler`: a 1 kHz sine survives 48
//! to 8 kHz and 8 to 48 kHz within a stated amplitude and frequency error, a 6 kHz sine resampled
//! 48 to 8 kHz is attenuated by a stated figure where a linear resampler folds it to 2 kHz at full
//! level, a stream cut into frames resamples to the same samples as one frame, and a frame that is
//! not 16-bit mono PCM at the resampler's input rate, or not contiguous, is refused.

use std::f64::consts::PI;
use std::num::{NonZeroU16, NonZeroU32};

use inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, RESAMPLER_RATES, ResampleError, Resampler,
    SampleClockPosition,
};

/// The largest relative amplitude error a 1 kHz sine may show after resampling.
const AMPLITUDE_TOLERANCE: f64 = 0.001;
/// The largest frequency error, in hertz, a 1 kHz sine may show after resampling.
const FREQUENCY_TOLERANCE_HZ: f64 = 0.01;
/// The least attenuation, in decibels, of a 6 kHz sine resampled from 48 to 8 kHz.
const ALIAS_ATTENUATION_DB: f64 = 80.0;

fn sine(frequency: f64, amplitude: f64, rate: u32, len: usize) -> Vec<i16> {
    (0..len)
        .map(|n| {
            let t = n as f64 / f64::from(rate);
            (amplitude * (2.0 * PI * frequency * t).sin()).round() as i16
        })
        .collect()
}

fn resample(from: u32, to: u32, input: &[i16]) -> Vec<i16> {
    let mut resampler = Resampler::new(from, to).expect("supported rates");
    let mut output = Vec::new();
    resampler.process(input, &mut output);
    output
}

fn rms(samples: &[i16]) -> f64 {
    let sum: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    (sum / samples.len() as f64).sqrt()
}

/// The frequency of a sine from its rising zero crossings, each placed by linear interpolation
/// between the two samples around it.
fn frequency_of(samples: &[i16], rate: u32) -> f64 {
    let crossings: Vec<f64> = samples
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| pair[0] < 0 && pair[1] >= 0)
        .map(|(n, pair)| {
            let (a, b) = (f64::from(pair[0]), f64::from(pair[1]));
            n as f64 + a / (a - b)
        })
        .collect();
    let first = crossings.first().expect("a crossing");
    let last = crossings.last().expect("a crossing");
    (crossings.len() - 1) as f64 * f64::from(rate) / (last - first)
}

/// The steady part of an output: past the filter's start-up, a whole number of `period` samples.
fn steady(output: &[i16], skip: usize, period: usize) -> &[i16] {
    let usable = (output.len() - skip) / period * period;
    &output[skip..skip + usable]
}

#[test]
fn a_1_khz_sine_survives_48_to_8_khz() {
    let amplitude = 16_000.0;
    let input = sine(1_000.0, amplitude, 48_000, 48_000);
    let output = resample(48_000, 8_000, &input);
    assert_eq!(output.len(), 8_000);

    let window = steady(&output, 200, 8);
    let measured_amplitude = rms(window) * 2f64.sqrt();
    let amplitude_error = (measured_amplitude - amplitude).abs() / amplitude;
    let frequency_error = (frequency_of(window, 8_000) - 1_000.0).abs();
    println!(
        "48->8 kHz, 1 kHz: amplitude {measured_amplitude:.2} (error {:.5} %), frequency error \
         {frequency_error:.6} Hz",
        amplitude_error * 100.0
    );
    assert!(
        amplitude_error <= AMPLITUDE_TOLERANCE,
        "amplitude {measured_amplitude}"
    );
    assert!(frequency_error <= FREQUENCY_TOLERANCE_HZ);
}

#[test]
fn a_1_khz_sine_survives_8_to_48_khz() {
    let amplitude = 16_000.0;
    let input = sine(1_000.0, amplitude, 8_000, 8_000);
    let output = resample(8_000, 48_000, &input);
    assert_eq!(output.len(), 48_000);

    let window = steady(&output, 1_200, 48);
    let measured_amplitude = rms(window) * 2f64.sqrt();
    let amplitude_error = (measured_amplitude - amplitude).abs() / amplitude;
    let frequency_error = (frequency_of(window, 48_000) - 1_000.0).abs();
    println!(
        "8->48 kHz, 1 kHz: amplitude {measured_amplitude:.2} (error {:.5} %), frequency error \
         {frequency_error:.6} Hz",
        amplitude_error * 100.0
    );
    assert!(
        amplitude_error <= AMPLITUDE_TOLERANCE,
        "amplitude {measured_amplitude}"
    );
    assert!(frequency_error <= FREQUENCY_TOLERANCE_HZ);
}

/// What a linear resampler does from 48 to 8 kHz: output sample `n` lies on input sample `6n`, so
/// it is that sample, and nothing removes what lies above 4 kHz.
fn linear_48_to_8(input: &[i16]) -> Vec<i16> {
    input.iter().step_by(6).copied().collect()
}

#[test]
fn a_6_khz_sine_is_attenuated_from_48_to_8_khz_where_a_linear_resampler_folds_it() {
    let input = sine(6_000.0, 16_000.0, 48_000, 48_000);
    let input_rms = rms(&input);

    let output = resample(48_000, 8_000, &input);
    let output_rms = rms(steady(&output, 200, 8));
    // An output that rounds to silence carries a residue under half a step in every sample, so
    // half a step is the floor of what a 16-bit output can show: the figure is then a lower bound.
    let attenuation = 20.0 * (input_rms / output_rms.max(0.5)).log10();
    println!("48->8 kHz, 6 kHz: output RMS {output_rms:.3}");

    let folded = linear_48_to_8(&input);
    let linear_attenuation = 20.0 * (input_rms / rms(&folded)).log10();
    let folded_frequency = frequency_of(&folded, 8_000);

    println!(
        "48->8 kHz, 6 kHz: attenuated {attenuation:.1} dB; linear: {linear_attenuation:.2} dB at \
         {folded_frequency:.1} Hz"
    );
    assert!(
        linear_attenuation.abs() < 1.0,
        "the linear resampler keeps the level"
    );
    assert!(
        (folded_frequency - 2_000.0).abs() < 1.0,
        "and folds it to 2 kHz"
    );
    assert!(
        attenuation >= ALIAS_ATTENUATION_DB,
        "attenuated by only {attenuation} dB"
    );
}

/// A deterministic broadband test signal: a linear congruential generator's top bits.
fn noise(len: usize) -> Vec<i16> {
    let mut state: u32 = 0x1234_5678;
    (0..len)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((state >> 16) as i16) / 2
        })
        .collect()
}

#[test]
fn a_stream_cut_into_frames_resamples_to_the_same_samples_as_one_frame() {
    let cuts = [1, 7, 480, 13, 0, 160, 2, 999, 31];
    for from in RESAMPLER_RATES {
        for to in RESAMPLER_RATES {
            let input = noise(from as usize / 10);
            let whole = resample(from, to, &input);

            let mut resampler = Resampler::new(from, to).expect("supported rates");
            let mut pieces = Vec::new();
            let mut rest = input.as_slice();
            for cut in cuts.iter().cycle() {
                if rest.is_empty() {
                    break;
                }
                let (head, tail) = rest.split_at((*cut).min(rest.len()));
                resampler.process(head, &mut pieces);
                rest = tail;
            }

            let largest = whole
                .iter()
                .zip(&pieces)
                .map(|(a, b)| (i32::from(*a) - i32::from(*b)).abs())
                .max()
                .unwrap_or(0);
            assert_eq!(whole.len(), pieces.len(), "{from} -> {to}: sample count");
            assert!(largest <= 1, "{from} -> {to}: differs by {largest}");
        }
    }
}

#[test]
fn every_pair_of_supported_rates_is_accepted_and_any_other_rate_refused() {
    assert_eq!(RESAMPLER_RATES, [8_000, 16_000, 24_000, 48_000]);
    for from in RESAMPLER_RATES {
        for to in RESAMPLER_RATES {
            let resampler = Resampler::new(from, to).expect("supported rates");
            assert_eq!(resampler.input_rate(), from);
            assert_eq!(resampler.output_rate(), to);
        }
    }
    assert_eq!(
        Resampler::new(44_100, 8_000).err(),
        Some(ResampleError::UnsupportedRate(44_100))
    );
    assert_eq!(
        Resampler::new(8_000, 11_025).err(),
        Some(ResampleError::UnsupportedRate(11_025))
    );
}

#[test]
fn the_stated_delay_aligns_the_output_with_the_input() {
    let input = sine(440.0, 10_000.0, 24_000, 24_000);
    let mut resampler = Resampler::new(24_000, 8_000).expect("supported rates");
    let delay = resampler.delay() as usize;
    let mut output = Vec::new();
    resampler.process(&input, &mut output);

    let largest = output
        .iter()
        .enumerate()
        .skip(2 * delay)
        .map(|(n, &s)| {
            let t = (n - delay) as f64 / 8_000.0;
            let expected = 10_000.0 * (2.0 * PI * 440.0 * t).sin();
            (f64::from(s) - expected).abs()
        })
        .fold(0.0, f64::max);
    assert!(largest <= 3.0, "off the delayed input by {largest}");
}

fn mono(encoding: AudioEncoding, rate: u32, channels: u16) -> AudioFormat {
    AudioFormat {
        encoding,
        sample_rate: NonZeroU32::new(rate).expect("non-zero"),
        channels: NonZeroU16::new(channels).expect("non-zero"),
    }
}

fn frame(rate: u32, position: u64, samples: &[i16]) -> AudioFrame {
    AudioFrame {
        format: mono(AudioEncoding::PcmS16le, rate, 1),
        position: SampleClockPosition::new(position).expect("in range"),
        sample_count: samples.len() as u32,
        data: samples.iter().flat_map(|s| s.to_le_bytes()).collect(),
    }
}

fn samples_of(frame: &AudioFrame) -> Vec<i16> {
    frame
        .data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| i16::from_le_bytes(*pair))
        .collect()
}

#[test]
fn frames_resample_onto_the_output_clock_with_the_samples_of_the_stream() {
    let input = noise(4_800);
    let whole = resample(48_000, 8_000, &input);

    let mut resampler = Resampler::new(48_000, 8_000).expect("supported rates");
    let mut collected = Vec::new();
    let mut position = 4_800;
    let mut out_position = 800;
    for chunk in input.chunks(960) {
        let out = resampler
            .resample_frame(&frame(48_000, position, chunk))
            .expect("a contiguous mono s16 frame at the input rate");
        assert_eq!(out.format, mono(AudioEncoding::PcmS16le, 8_000, 1));
        assert_eq!(out.position.get(), out_position);
        assert_eq!(u64::from(out.sample_count) * 2, out.data.len() as u64);
        out_position += u64::from(out.sample_count);
        position += chunk.len() as u64;
        collected.extend(samples_of(&out));
    }
    assert_eq!(collected, whole);
}

#[test]
fn a_frame_the_resampler_cannot_take_is_refused_with_a_typed_error() {
    let mut resampler = Resampler::new(16_000, 8_000).expect("supported rates");
    let expected = mono(AudioEncoding::PcmS16le, 16_000, 1);

    let wrong_rate = frame(8_000, 0, &[0; 4]);
    assert_eq!(
        resampler.resample_frame(&wrong_rate),
        Err(ResampleError::UnexpectedFormat {
            expected,
            actual: wrong_rate.format
        })
    );

    let mut stereo = frame(16_000, 0, &[0; 4]);
    stereo.format.channels = NonZeroU16::new(2).expect("non-zero");
    stereo.sample_count = 2;
    assert_eq!(
        resampler.resample_frame(&stereo),
        Err(ResampleError::UnexpectedFormat {
            expected,
            actual: stereo.format
        })
    );

    let mut mulaw = frame(16_000, 0, &[0; 2]);
    mulaw.format.encoding = AudioEncoding::PcmMulaw;
    assert!(matches!(
        resampler.resample_frame(&mulaw),
        Err(ResampleError::UnexpectedFormat { .. })
    ));

    let mut short = frame(16_000, 0, &[0; 4]);
    short.data.pop();
    assert_eq!(
        resampler.resample_frame(&short),
        Err(ResampleError::DataLength {
            expected: 8,
            actual: 7
        })
    );

    resampler
        .resample_frame(&frame(16_000, 0, &[0; 4]))
        .expect("the first frame");
    let gap = frame(16_000, 10, &[0; 4]);
    let refused = resampler
        .resample_frame(&gap)
        .expect_err("a gap is refused");
    assert_eq!(
        refused,
        ResampleError::Discontinuous {
            expected: SampleClockPosition::new(4).expect("in range"),
            actual: gap.position
        }
    );
    assert!(!refused.to_string().is_empty());
    resampler
        .resample_frame(&frame(16_000, 4, &[0; 4]))
        .expect("the refused frame left the stream where it was");
}
