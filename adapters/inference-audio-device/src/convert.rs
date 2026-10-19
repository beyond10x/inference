//! Sample conversion between device audio and `inference-audio` frames: down-mixing interleaved
//! channels to mono, encoding mono samples into a frame's bytes and decoding them back, and a
//! streaming linear resampler.

use std::error::Error;
use std::fmt;
use std::num::NonZeroU32;

use inference_audio::{AudioEncoding, AudioFrame};

/// Appends to `mono` the mean of each whole frame of `channels` interleaved samples of
/// `interleaved`; a trailing partial frame is ignored.
pub fn downmix(interleaved: &[f32], channels: usize, mono: &mut Vec<f32>) {
    let channels = channels.max(1);
    if channels == 1 {
        mono.extend_from_slice(interleaved);
        return;
    }
    let scale = 1.0 / channels as f32;
    mono.extend(
        interleaved
            .chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() * scale),
    );
}

/// Full scale of `pcm_s16le`: 1.0 encodes as this step, -1.0 as its negation.
const PCM16_FULL_SCALE: f32 = 32767.0;

/// `sample` as a device can play it: finite and within full scale. NaN becomes silence, and
/// anything beyond full scale, infinities included, is clamped to it.
#[must_use]
pub fn playable(sample: f32) -> f32 {
    if sample.is_nan() {
        0.0
    } else {
        sample.clamp(-1.0, 1.0)
    }
}

/// `sample` made finite: NaN becomes silence and an infinity full scale of its sign; a finite
/// sample is kept as it is, headroom beyond full scale included, as `pcm_f32le` carries it.
#[must_use]
pub fn finite(sample: f32) -> f32 {
    if sample.is_finite() {
        sample
    } else {
        playable(sample)
    }
}

/// The `pcm_s16le` step of `sample`, clamped to full scale and rounded to the nearest step.
pub(crate) fn to_pcm16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * PCM16_FULL_SCALE).round() as i16
}

/// The bytes of `samples`, mono at full scale 1, in `encoding`: `pcm_s16le` clamps to full scale
/// and rounds to the nearest step, `pcm_f32le` keeps the value.
#[must_use]
pub fn encode(samples: &[f32], encoding: AudioEncoding) -> Vec<u8> {
    match encoding {
        AudioEncoding::PcmS16le => samples
            .iter()
            .flat_map(|&sample| to_pcm16(sample).to_le_bytes())
            .collect(),
        AudioEncoding::PcmF32le => samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect(),
    }
}

/// A frame whose data is not [`AudioFormat::data_len`](inference_audio::AudioFormat::data_len)
/// of its sample count long.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MalformedFrame {
    pub expected: u64,
    pub actual: usize,
}

impl fmt::Display for MalformedFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the frame carries {} bytes of data where its format and sample count make {}",
            self.actual, self.expected
        )
    }
}

impl Error for MalformedFrame {}

/// The samples of `frame` at full scale 1, down-mixed to mono.
///
/// # Errors
///
/// [`MalformedFrame`] when the frame's data is not as long as its format and sample count make.
pub fn decode(frame: &AudioFrame) -> Result<Vec<f32>, MalformedFrame> {
    let expected = frame.format.data_len(frame.sample_count);
    if u64::try_from(frame.data.len()) != Ok(expected) {
        return Err(MalformedFrame {
            expected,
            actual: frame.data.len(),
        });
    }
    let interleaved: Vec<f32> = match frame.format.encoding {
        AudioEncoding::PcmS16le => frame
            .data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| (f32::from(i16::from_le_bytes(*pair)) / PCM16_FULL_SCALE).max(-1.0))
            .collect(),
        AudioEncoding::PcmF32le => frame
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| f32::from_le_bytes(*word))
            .collect(),
    };
    let channels = usize::from(frame.format.channels.get());
    if channels == 1 {
        return Ok(interleaved);
    }
    let mut mono = Vec::with_capacity(interleaved.len() / channels);
    downmix(&interleaved, channels, &mut mono);
    Ok(mono)
}

/// A streaming linear resampler from `from` to `to` hertz. Output sample `k` lies at input time
/// `k * from / to` and is interpolated linearly between the two input samples around it, so it
/// is produced as soon as the input sample after it has arrived; equal rates pass samples
/// through unchanged. Fed in chunks, it produces exactly what it produces fed all at once.
///
/// It has no anti-aliasing filter: downsampling folds content above the new Nyquist frequency
/// back into the band. For speech from a headset or laptop microphone that content is small.
#[derive(Debug, Clone)]
pub struct LinearResampler {
    from: u32,
    to: u32,
    received: u64,
    produced: u64,
    previous: f32,
}

impl LinearResampler {
    /// A resampler from `from` to `to` hertz that has received nothing.
    #[must_use]
    pub const fn new(from: NonZeroU32, to: NonZeroU32) -> Self {
        Self {
            from: from.get(),
            to: to.get(),
            received: 0,
            produced: 0,
            previous: 0.0,
        }
    }

    /// Feeds `input` and appends every output sample it completes to `output`.
    pub fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        if self.from == self.to {
            output.extend_from_slice(input);
            self.received += input.len() as u64;
            self.produced += input.len() as u64;
            return;
        }
        let (from, to) = (u128::from(self.from), u128::from(self.to));
        for &sample in input {
            let arrived = u128::from(self.received);
            loop {
                let time = u128::from(self.produced) * from;
                let (index, remainder) = (time / to, time % to);
                let value = if index == arrived && remainder == 0 {
                    sample
                } else if index + 1 == arrived {
                    let fraction = remainder as f32 / to as f32;
                    self.previous + (sample - self.previous) * fraction
                } else {
                    break;
                };
                output.push(value);
                self.produced += 1;
            }
            self.previous = sample;
            self.received += 1;
        }
    }

    /// How many input samples the resampler received.
    #[must_use]
    pub const fn received(&self) -> u64 {
        self.received
    }

    /// How many output samples the resampler produced.
    #[must_use]
    pub const fn produced(&self) -> u64 {
        self.produced
    }
}
