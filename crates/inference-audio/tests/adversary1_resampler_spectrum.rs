//! Adversary cases, pass 1, story `g711-codec-and-anti-aliased-resampler`: the `Resampler` over
//! every rate pair, against what its documentation states. Frame splits at seeded random points
//! (lengths 0 and 1 and frames shorter than the filter included) give the one-shot output
//! exactly and the stated `ceil(n * L / M)` count after every call; `delay` is the measured lag
//! and the neighbours of it are not; content just above and at the lower Nyquist frequency is
//! attenuated by the stated figure where it would fold or image; the pass band at 0.8 of the lower
//! Nyquist frequency keeps its level; full-scale input saturates without wrapping; and
//! `resample_frame` keeps its positions on the output clock over a long stream, refuses a gap and
//! a restart, and accepts an empty frame.

use std::f64::consts::PI;
use std::num::{NonZeroU16, NonZeroU32};

use inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, RESAMPLER_RATES, ResampleError, Resampler,
    SampleClockPosition,
};

/// The least attenuation, in dB, of content that would fold or image: the resampler's docs say
/// "about 80 dB of stopband attenuation".
const STOPBAND_DB: f64 = 75.0;
/// The largest level change, in dB, at 0.8 of the lower Nyquist frequency.
const PASSBAND_DB: f64 = 0.05;

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
}

fn pairs() -> impl Iterator<Item = (u32, u32)> {
    RESAMPLER_RATES
        .into_iter()
        .flat_map(|from| RESAMPLER_RATES.into_iter().map(move |to| (from, to)))
}

fn sine(frequency: f64, amplitude: f64, phase: f64, rate: u32, len: usize) -> Vec<i16> {
    (0..len)
        .map(|n| {
            let t = n as f64 / f64::from(rate);
            (amplitude * (2.0 * PI * frequency * t + phase).sin()).round() as i16
        })
        .collect()
}

fn whole(from: u32, to: u32, input: &[i16]) -> Vec<i16> {
    let mut resampler = Resampler::new(from, to).expect("supported rates");
    let mut output = Vec::new();
    resampler.process(input, &mut output);
    output
}

/// The amplitude of the `frequency` component of `samples` at `rate`, by a single DFT bin; the
/// window holds a whole second, so an integer frequency sits on a bin.
fn amplitude_at(samples: &[i16], rate: u32, frequency: f64) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for (n, &s) in samples.iter().enumerate() {
        let angle = 2.0 * PI * frequency * n as f64 / f64::from(rate);
        re += f64::from(s) * angle.cos();
        im += f64::from(s) * angle.sin();
    }
    2.0 * (re * re + im * im).sqrt() / samples.len() as f64
}

fn rms(samples: &[i16]) -> f64 {
    let sum: f64 = samples.iter().map(|&s| f64::from(s).powi(2)).sum();
    (sum / samples.len() as f64).sqrt()
}

#[test]
fn random_frame_splits_give_the_one_shot_output_exactly_and_the_stated_count_after_each_call() {
    let mut rng = Lcg(0x5eed_0001);
    for (from, to) in pairs() {
        let input: Vec<i16> = (0..from as usize / 5)
            .map(|_| (rng.next() as i64 - (1 << 30)) as i16)
            .collect();
        let reference = whole(from, to, &input);
        let resampler = Resampler::new(from, to).expect("supported rates");
        let (up, down) = {
            let mut a = u64::from(from);
            let mut b = u64::from(to);
            while b != 0 {
                (a, b) = (b, a % b);
            }
            (u64::from(to) / a, u64::from(from) / a)
        };
        for trial in 0..4 {
            let mut split = resampler.clone();
            let mut pieces = Vec::new();
            let mut fed = 0usize;
            while fed < input.len() {
                let len = match rng.next() % 6 {
                    0 => 0,
                    1 => 1,
                    2 => 2,
                    3 => (rng.next() % 40) as usize,
                    _ => (rng.next() % 700) as usize,
                }
                .min(input.len() - fed);
                split.process(&input[fed..fed + len], &mut pieces);
                fed += len;
                let stated = (fed as u64 * up).div_ceil(down);
                assert_eq!(
                    pieces.len() as u64,
                    stated,
                    "{from} -> {to}, trial {trial}: after {fed} input samples"
                );
            }
            assert_eq!(pieces, reference, "{from} -> {to}, trial {trial}");
        }
    }
}

#[test]
fn the_stated_delay_is_the_measured_lag_for_every_pair() {
    for (from, to) in pairs() {
        let lower = from.min(to);
        let frequency = f64::from(lower) / 8.0;
        let amplitude = 10_000.0;
        let input = sine(frequency, amplitude, 0.3, from, 2 * from as usize);
        let output = whole(from, to, &input);
        let delay = Resampler::new(from, to).expect("supported rates").delay() as i64;
        let error_at = |lag: i64| -> f64 {
            output
                .iter()
                .enumerate()
                .skip(to as usize / 2)
                .map(|(n, &s)| {
                    let t = (n as i64 - lag) as f64 / f64::from(to);
                    (f64::from(s) - amplitude * (2.0 * PI * frequency * t + 0.3).sin()).abs()
                })
                .fold(0.0, f64::max)
        };
        let at = error_at(delay);
        println!("{from} -> {to}: delay {delay}, largest error {at:.2}");
        assert!(at <= 4.0, "{from} -> {to}: off the delayed input by {at}");
        if delay > 0 {
            assert!(error_at(delay - 1) > 100.0 && error_at(delay + 1) > 100.0);
        }
    }
}

#[test]
fn content_just_above_and_at_the_lower_nyquist_frequency_does_not_fold_or_image() {
    for (from, to) in pairs().filter(|(f, t)| f != t) {
        let lower = from.min(to);
        let nyquist = f64::from(lower) / 2.0;
        let amplitude = 30_000.0;
        let cases: Vec<(f64, f64)> = if to < from {
            vec![
                (nyquist * 1.02, nyquist * 0.98),
                (nyquist * 1.1, nyquist * 0.9),
            ]
        } else {
            vec![
                (nyquist * 0.98, nyquist * 1.02),
                (nyquist * 0.9, nyquist * 1.1),
            ]
        };
        for (tone, fold) in cases {
            let input = sine(tone, amplitude, 0.0, from, 2 * from as usize);
            let output = whole(from, to, &input);
            let last = &output[output.len() - to as usize..];
            let level = amplitude_at(last, to, fold);
            let attenuation = 20.0 * (amplitude / level.max(1e-9)).log10();
            println!("{from} -> {to}: {tone} Hz at {fold} Hz is {attenuation:.1} dB down");
            assert!(
                attenuation >= STOPBAND_DB,
                "{from} -> {to}: {tone} Hz lands at {fold} Hz only {attenuation:.1} dB down"
            );
        }
        if to < from {
            let input = sine(nyquist, amplitude, PI / 4.0, from, 2 * from as usize);
            let output = whole(from, to, &input);
            let last = &output[output.len() - to as usize..];
            let attenuation = 20.0 * (amplitude / 2f64.sqrt() / rms(last).max(0.5)).log10();
            println!("{from} -> {to}: the lower Nyquist tone is {attenuation:.1} dB down");
            assert!(
                attenuation >= STOPBAND_DB,
                "{from} -> {to}: the lower Nyquist tone is only {attenuation:.1} dB down"
            );
        }
    }
}

#[test]
fn the_pass_band_at_0_8_of_the_lower_nyquist_frequency_keeps_its_level() {
    for (from, to) in pairs().filter(|(f, t)| f != t) {
        let tone = 0.8 * f64::from(from.min(to)) / 2.0;
        let amplitude = 16_000.0;
        let input = sine(tone, amplitude, 0.0, from, 2 * from as usize);
        let output = whole(from, to, &input);
        let last = &output[output.len() - to as usize..];
        let gain = 20.0 * (amplitude_at(last, to, tone) / amplitude).log10();
        println!("{from} -> {to}: {tone} Hz passes at {gain:.4} dB");
        assert!(
            gain.abs() <= PASSBAND_DB,
            "{from} -> {to}: {tone} Hz passes at {gain} dB"
        );
    }
}

#[test]
fn full_scale_input_saturates_and_never_wraps() {
    for (from, to) in pairs() {
        let delay = Resampler::new(from, to).expect("supported rates").delay() as usize;
        let alternating: Vec<i16> = (0..from as usize)
            .map(|n| if n % 2 == 0 { i16::MAX } else { i16::MIN })
            .collect();
        let output = whole(from, to, &alternating);
        let settled = &output[(2 * delay + 4).min(output.len())..];
        if from != to {
            let peak = settled
                .iter()
                .map(|&s| i32::from(s).abs())
                .max()
                .unwrap_or(0);
            assert!(
                peak <= 8,
                "{from} -> {to}: the input's Nyquist tone at full scale leaves {peak}"
            );
        }

        let period = from as usize / 50;
        let square: Vec<i16> = (0..from as usize)
            .map(|n| {
                if (n / (period / 2)).is_multiple_of(2) {
                    i16::MAX
                } else {
                    i16::MIN
                }
            })
            .collect();
        let output = whole(from, to, &square);
        let out_period = to as usize / 50;
        let guard = out_period / 8;
        for (n, &s) in output.iter().enumerate().skip(2 * delay + out_period) {
            let at = (n - delay) % out_period;
            let middle_high = at > guard && at < out_period / 2 - guard;
            let middle_low = at > out_period / 2 + guard && at < out_period - guard;
            if middle_high {
                assert!(
                    s > 30_000,
                    "{from} -> {to}: sample {n} is {s} on a high plateau"
                );
            }
            if middle_low {
                assert!(
                    s < -30_000,
                    "{from} -> {to}: sample {n} is {s} on a low plateau"
                );
            }
        }
        if from != to {
            assert!(
                output.contains(&i16::MAX) && output.contains(&i16::MIN),
                "{from} -> {to}: the overshoot of a full-scale square wave reaches the clamp"
            );
        }
    }
}

fn frame(rate: u32, position: u64, samples: &[i16]) -> AudioFrame {
    AudioFrame {
        format: AudioFormat {
            encoding: AudioEncoding::PcmS16le,
            sample_rate: NonZeroU32::new(rate).expect("non-zero"),
            channels: NonZeroU16::MIN,
        },
        position: SampleClockPosition::new(position).expect("in range"),
        sample_count: samples.len() as u32,
        data: samples.iter().flat_map(|s| s.to_le_bytes()).collect(),
    }
}

#[test]
fn resample_frame_positions_stay_on_the_output_clock_over_a_long_stream() {
    let mut rng = Lcg(0x5eed_0002);
    for (from, to) in pairs() {
        let start = 1 + rng.next() % 10_000 + if from > to { 1 << 40 } else { 0 };
        let mut resampler = Resampler::new(from, to).expect("supported rates");
        let first_out = start * u64::from(to) / u64::from(from);
        let mut position = start;
        let mut out_end = first_out;
        let mut consumed = 0u64;
        for _ in 0..400 {
            let len = (rng.next() % 600) as usize;
            let samples = vec![0i16; len];
            let out = resampler
                .resample_frame(&frame(from, position, &samples))
                .expect("a contiguous frame");
            assert_eq!(out.position.get(), out_end, "{from} -> {to}");
            assert_eq!(out.data.len(), 2 * out.sample_count as usize);
            out_end += u64::from(out.sample_count);
            position += len as u64;
            consumed += len as u64;
            let carried = (u128::from(start + consumed) * u128::from(to)) as f64 / f64::from(from);
            assert!(
                (out_end as f64 - carried).abs() <= 1.0,
                "{from} -> {to}: output end {out_end} drifted from {carried}"
            );
            assert_eq!(
                out_end - first_out,
                (consumed * u64::from(to)).div_ceil(u64::from(from)),
                "{from} -> {to}: the output count is the stated ceil(n * L / M)"
            );
        }

        let restart = frame(from, 0, &[0; 16]);
        assert_eq!(
            resampler.resample_frame(&restart),
            Err(ResampleError::Discontinuous {
                expected: SampleClockPosition::new(position).expect("in range"),
                actual: restart.position
            }),
            "{from} -> {to}: a restart is a gap"
        );
        let overlap = frame(from, position - 1, &[0; 16]);
        assert!(matches!(
            resampler.resample_frame(&overlap),
            Err(ResampleError::Discontinuous { .. })
        ));
        let empty = resampler
            .resample_frame(&frame(from, position, &[]))
            .expect("an empty frame at the stream's end");
        assert_eq!(empty.sample_count, 0);
        assert_eq!(empty.position.get(), out_end);
        let mut fresh = Resampler::new(from, to).expect("supported rates");
        let restarted = fresh.resample_frame(&restart).expect("a new stream");
        assert_eq!(restarted.position.get(), 0);
    }
}

#[test]
fn only_the_four_rates_are_taken() {
    for rate in [
        0,
        1,
        7_999,
        11_025,
        22_050,
        32_000,
        44_100,
        96_000,
        u32::MAX,
    ] {
        for other in RESAMPLER_RATES {
            assert_eq!(
                Resampler::new(rate, other).err(),
                Some(ResampleError::UnsupportedRate(rate))
            );
            assert_eq!(
                Resampler::new(other, rate).err(),
                Some(ResampleError::UnsupportedRate(rate))
            );
        }
    }
    for rate in RESAMPLER_RATES {
        let input: Vec<i16> = (0..999).map(|n| (n * 37 - 16_000) as i16).collect();
        assert_eq!(
            whole(rate, rate, &input),
            input,
            "{rate}: the identity copies"
        );
        assert_eq!(Resampler::new(rate, rate).expect("ok").delay(), 0);
    }
}
