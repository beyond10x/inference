//! A band-limited resampler for 16-bit mono PCM between 8, 16, 24 and 48 kHz: a polyphase FIR
//! whose prototype is a Kaiser-windowed sinc low-pass below the lower rate's Nyquist frequency.

use std::fmt;
use std::num::{NonZeroU16, NonZeroU32};

use crate::g711::data_len_mismatch;
use crate::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};

/// The sample rates, in hertz, [`Resampler`] converts between.
pub const RESAMPLER_RATES: [u32; 4] = [8_000, 16_000, 24_000, 48_000];

/// Zero crossings of the prototype sinc on each side of its centre, counted at the lower rate.
const ZERO_CROSSINGS: u64 = 32;
/// The prototype's cutoff as a fraction of the lower rate's Nyquist frequency.
const CUTOFF: f64 = 0.9;
/// The Kaiser window's shape parameter, about 80 dB of stopband attenuation.
const KAISER_BETA: f64 = 8.0;

/// Converts a stream of 16-bit mono PCM from one rate in [`RESAMPLER_RATES`] to another.
///
/// The output at rate `to` is the input upsampled by `L`, low-pass filtered and downsampled by
/// `M`, where `L / M` is `to / from` in lowest terms; only the taps that meet an input sample are
/// computed. The low-pass is a Kaiser-windowed sinc with its cutoff at 0.9 of the lower rate's
/// Nyquist frequency and about 80 dB of stopband attenuation, so content the lower rate cannot
/// carry is removed rather than folded back into the band. Between equal rates the resampler
/// copies its input.
///
/// The resampler keeps the input its filter still needs between calls, so a stream cut into any
/// frames gives the same output samples as the whole stream at once. The filter is linear-phase
/// and causal: the output lags the input by [`Resampler::delay`] output samples, and nothing is
/// held back, so after `n` input samples it has produced `ceil(n * L / M)` output samples.
/// Construction allocates the filter; processing allocates only to grow its buffers to the
/// largest frame it has seen.
#[derive(Debug, Clone)]
pub struct Resampler {
    from: u32,
    to: u32,
    up: u64,
    down: u64,
    taps: usize,
    /// `phases[p * taps + j]` multiplies input sample `base - (taps - 1 - j)` for output phase `p`.
    phases: Vec<f32>,
    /// Input samples from global index `origin` on; before the first sample, zeros.
    history: Vec<f32>,
    origin: i64,
    consumed: u64,
    produced: u64,
    scratch_in: Vec<i16>,
    scratch_out: Vec<i16>,
    next_position: Option<(SampleClockPosition, u64)>,
}

impl Resampler {
    /// A resampler from `from` hertz to `to` hertz, both in [`RESAMPLER_RATES`].
    pub fn new(from: u32, to: u32) -> Result<Self, ResampleError> {
        for rate in [from, to] {
            if !RESAMPLER_RATES.contains(&rate) {
                return Err(ResampleError::UnsupportedRate(rate));
            }
        }
        let common = gcd(u64::from(from), u64::from(to));
        let up = u64::from(to) / common;
        let down = u64::from(from) / common;
        let (taps, phases) = if from == to {
            (1, vec![1.0])
        } else {
            design(up, u64::from(from.min(to)), u64::from(from) * up)
        };
        Ok(Self {
            from,
            to,
            up,
            down,
            taps,
            phases,
            history: vec![0.0; taps - 1],
            origin: 1 - taps as i64,
            consumed: 0,
            produced: 0,
            scratch_in: Vec::new(),
            scratch_out: Vec::new(),
            next_position: None,
        })
    }

    /// The input rate in hertz.
    pub fn input_rate(&self) -> u32 {
        self.from
    }

    /// The output rate in hertz.
    pub fn output_rate(&self) -> u32 {
        self.to
    }

    /// How many output samples the output lags the input: output sample `n + delay()` lies at the
    /// time of output-clock sample `n` of the input. It is 32 samples of the lower rate, and zero
    /// between equal rates.
    pub fn delay(&self) -> u32 {
        if self.from == self.to {
            0
        } else {
            (ZERO_CROSSINGS * u64::from(self.to) / u64::from(self.from.min(self.to))) as u32
        }
    }

    /// Resamples `input`, the next samples of the stream, and appends every output sample they
    /// complete to `output`.
    pub fn process(&mut self, input: &[i16], output: &mut Vec<i16>) {
        self.history.extend(input.iter().map(|&s| f32::from(s)));
        self.consumed += input.len() as u64;
        let total = self.consumed * self.up;
        let first = self.produced;
        let ready = total.div_ceil(self.down);
        output.reserve((ready - first) as usize);
        for n in first..ready {
            let at = n * self.down;
            let base = (at / self.up) as i64;
            let phase = (at % self.up) as usize;
            let end = (base - self.origin) as usize + 1;
            let window = &self.history[end - self.taps..end];
            let coefficients = &self.phases[phase * self.taps..(phase + 1) * self.taps];
            let value: f32 = window
                .iter()
                .zip(coefficients)
                .map(|(sample, coefficient)| sample * coefficient)
                .sum();
            output.push(
                value
                    .round()
                    .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16,
            );
        }
        self.produced = ready;
        let next_base = (ready * self.down / self.up) as i64;
        let keep_from = next_base + 1 - self.taps as i64;
        if keep_from > self.origin {
            let drop = ((keep_from - self.origin) as usize).min(self.history.len());
            self.history.drain(..drop);
            self.origin += drop as i64;
        }
    }

    /// Resamples the next frame of the stream: `pcm_s16le`, one channel, at the input rate, its
    /// data the length its sample count requires, and placed where the previous frame ended. The
    /// output frame is `pcm_s16le` mono at the output rate; the first one sits at the first
    /// frame's position carried onto the output clock (rounded down), and each next one where the
    /// previous ended. A refused frame leaves the stream as it was.
    pub fn resample_frame(&mut self, frame: &AudioFrame) -> Result<AudioFrame, ResampleError> {
        let expected = AudioFormat {
            encoding: AudioEncoding::PcmS16le,
            sample_rate: NonZeroU32::new(self.from).unwrap_or(NonZeroU32::MIN),
            channels: NonZeroU16::MIN,
        };
        if frame.format != expected {
            return Err(ResampleError::UnexpectedFormat {
                expected,
                actual: frame.format,
            });
        }
        if let Some((expected, actual)) = data_len_mismatch(frame) {
            return Err(ResampleError::DataLength { expected, actual });
        }
        let output_start = match self.next_position {
            Some((input, output)) => {
                if frame.position != input {
                    return Err(ResampleError::Discontinuous {
                        expected: input,
                        actual: frame.position,
                    });
                }
                output
            }
            None => {
                let carried =
                    u128::from(frame.position.get()) * u128::from(self.to) / u128::from(self.from);
                u64::try_from(carried).map_err(|_| ResampleError::OutOfRange)?
            }
        };
        let total = (self.consumed + u64::from(frame.sample_count)) * self.up;
        let count = u32::try_from(total.div_ceil(self.down) - self.produced)
            .map_err(|_| ResampleError::OutOfRange)?;
        let position = SampleClockPosition::new(output_start).ok_or(ResampleError::OutOfRange)?;
        let next_input = frame
            .position
            .get()
            .checked_add(u64::from(frame.sample_count))
            .and_then(SampleClockPosition::new)
            .ok_or(ResampleError::OutOfRange)?;
        let next_output = output_start
            .checked_add(u64::from(count))
            .ok_or(ResampleError::OutOfRange)?;

        let mut input = std::mem::take(&mut self.scratch_in);
        let mut output = std::mem::take(&mut self.scratch_out);
        input.clear();
        output.clear();
        input.extend(
            frame
                .data
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| i16::from_le_bytes(*pair)),
        );
        self.process(&input, &mut output);
        let data = output.iter().flat_map(|s| s.to_le_bytes()).collect();
        self.scratch_in = input;
        self.scratch_out = output;
        self.next_position = Some((next_input, next_output));

        Ok(AudioFrame {
            format: AudioFormat {
                sample_rate: NonZeroU32::new(self.to).unwrap_or(NonZeroU32::MIN),
                ..expected
            },
            position,
            sample_count: count,
            data,
        })
    }
}

/// The polyphase filter for upsampling by `up` to `upsampled` hertz, below the Nyquist frequency of
/// `lower` hertz: the taps per phase and the phases, each reversed and normalised to unit gain at
/// zero frequency.
fn design(up: u64, lower: u64, upsampled: u64) -> (usize, Vec<f32>) {
    let per_lower = upsampled / lower;
    let centre = ZERO_CROSSINGS * per_lower;
    let len = 2 * centre + 1;
    let taps = len.div_ceil(up) as usize;
    let cutoff = CUTOFF * 0.5 * lower as f64 / upsampled as f64;
    let i0_beta = bessel_i0(KAISER_BETA);
    let prototype = |k: u64| -> f64 {
        if k >= len {
            return 0.0;
        }
        let x = k as f64 - centre as f64;
        let sinc = if x == 0.0 {
            2.0 * cutoff
        } else {
            (2.0 * std::f64::consts::PI * cutoff * x).sin() / (std::f64::consts::PI * x)
        };
        let ratio = x / centre as f64;
        sinc * bessel_i0(KAISER_BETA * (1.0 - ratio * ratio).sqrt()) / i0_beta
    };
    let up_len = up as usize;
    let mut phases = vec![0.0f32; up_len * taps];
    for phase in 0..up_len {
        let row: Vec<f64> = (0..taps)
            .map(|t| prototype(phase as u64 + t as u64 * up))
            .collect();
        let gain: f64 = row.iter().sum();
        for (t, value) in row.iter().enumerate() {
            phases[phase * taps + taps - 1 - t] = (value / gain) as f32;
        }
    }
    (taps, phases)
}

/// The modified Bessel function of the first kind and order zero, by its power series.
fn bessel_i0(x: f64) -> f64 {
    let quarter = x * x / 4.0;
    let mut term = 1.0;
    let mut sum = 1.0;
    let mut k = 1.0;
    while term > sum * 1e-17 {
        term *= quarter / (k * k);
        sum += term;
        k += 1.0;
    }
    sum
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// Why a [`Resampler`] refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResampleError {
    /// A rate outside [`RESAMPLER_RATES`].
    UnsupportedRate(u32),
    /// The frame is not `pcm_s16le`, one channel, at the resampler's input rate.
    UnexpectedFormat {
        /// The format the resampler takes.
        expected: AudioFormat,
        /// The frame's format.
        actual: AudioFormat,
    },
    /// The frame's data is not [`AudioFormat::data_len`] of its sample count long.
    DataLength {
        /// The length the format and sample count require.
        expected: u64,
        /// The length of the data.
        actual: u64,
    },
    /// The frame does not start where the previous frame ended.
    Discontinuous {
        /// Where the previous frame ended.
        expected: SampleClockPosition,
        /// Where the frame starts.
        actual: SampleClockPosition,
    },
    /// The output frame's sample count would not fit a `u32`, or a position would pass
    /// [`SampleClockPosition::MAX`].
    OutOfRange,
}

impl fmt::Display for ResampleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedRate(rate) => write!(
                f,
                "{rate} Hz is not a resampler rate; the rates are 8000, 16000, 24000 and 48000 Hz"
            ),
            Self::UnexpectedFormat { expected, actual } => {
                write!(
                    f,
                    "the resampler takes {expected:?}, the frame is {actual:?}"
                )
            }
            Self::DataLength { expected, actual } => write!(
                f,
                "the frame carries {actual} bytes of data where its format and sample count \
                 require {expected}"
            ),
            Self::Discontinuous { expected, actual } => write!(
                f,
                "the frame starts at sample {} where the stream ended at {}",
                actual.get(),
                expected.get()
            ),
            Self::OutOfRange => write!(
                f,
                "the output frame's sample count or position is out of range"
            ),
        }
    }
}

impl std::error::Error for ResampleError {}
