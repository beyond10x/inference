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
/// and rounds to the nearest step, `pcm_f32le` keeps the value, `pcm_mulaw` and `pcm_alaw`
/// compand the `pcm_s16le` step as G.711 does.
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
        AudioEncoding::PcmMulaw => samples
            .iter()
            .map(|&sample| mulaw_of(to_pcm16(sample)))
            .collect(),
        AudioEncoding::PcmAlaw => samples
            .iter()
            .map(|&sample| alaw_of(to_pcm16(sample)))
            .collect(),
    }
}

/// The G.711 mu-law byte of `sample`: bias 0x84, clip 32_635, the bits inverted.
fn mulaw_of(sample: i16) -> u8 {
    let magnitude = i32::from(sample).abs().min(32_635) + 0x84;
    let exponent = (0..8)
        .rev()
        .find(|exponent| magnitude & (0x80 << exponent) != 0)
        .unwrap_or(0);
    let mantissa = (magnitude >> (exponent + 3)) & 0x0F;
    let sign = if sample < 0 { 0x80 } else { 0 };
    !u8::try_from(sign | (exponent << 4) | mantissa).unwrap_or(0x7F)
}

/// The G.711 A-law byte of `sample`: 13 bits, eight segments, the even bits inverted.
fn alaw_of(sample: i16) -> u8 {
    const SEGMENT_ENDS: [i32; 8] = [0x1F, 0x3F, 0x7F, 0xFF, 0x1FF, 0x3FF, 0x7FF, 0xFFF];
    let pcm = i32::from(sample) >> 3;
    let (mask, pcm) = if pcm >= 0 {
        (0xD5, pcm)
    } else {
        (0x55, -pcm - 1)
    };
    let value = match SEGMENT_ENDS.iter().position(|end| pcm <= *end) {
        None => 0x7F,
        Some(segment) => {
            let segment = i32::try_from(segment).unwrap_or(7);
            let shift = if segment < 2 { 1 } else { segment };
            (segment << 4) | ((pcm >> shift) & 0x0F)
        }
    };
    u8::try_from(value ^ mask).unwrap_or(0xD5)
}

/// The linear sample a G.711 mu-law byte encodes, within `-32_124..=32_124`.
fn mulaw(byte: u8) -> i16 {
    let byte = !byte;
    let exponent = (byte >> 4) & 0x07;
    let magnitude = ((i32::from(byte & 0x0F) << 3) + 0x84) << exponent;
    let sample = i16::try_from(magnitude - 0x84).unwrap_or(i16::MAX);
    if byte & 0x80 == 0 { sample } else { -sample }
}

/// The linear sample a G.711 A-law byte encodes, within `-32_256..=32_256`.
fn alaw(byte: u8) -> i16 {
    let byte = byte ^ 0x55;
    let segment = (byte >> 4) & 0x07;
    let step = i32::from(byte & 0x0F) << 4;
    let magnitude = match segment {
        0 => step + 8,
        _ => (step + 0x108) << (segment - 1),
    };
    let sample = i16::try_from(magnitude).unwrap_or(i16::MAX);
    if byte & 0x80 == 0 { -sample } else { sample }
}

/// A linear 16-bit step at full scale 1.
fn from_pcm16(step: i16) -> f32 {
    (f32::from(step) / PCM16_FULL_SCALE).clamp(-1.0, 1.0)
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
        AudioEncoding::PcmMulaw => frame
            .data
            .iter()
            .map(|byte| from_pcm16(mulaw(*byte)))
            .collect(),
        AudioEncoding::PcmAlaw => frame
            .data
            .iter()
            .map(|byte| from_pcm16(alaw(*byte)))
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

#[cfg(test)]
mod g711 {
    use super::{alaw, alaw_of, from_pcm16, mulaw, mulaw_of};

    #[test]
    fn silence_encodes_as_the_silent_byte_of_each_law() {
        assert_eq!(mulaw_of(0), 0xFF);
        assert_eq!(alaw_of(0), 0xD5);
        assert_eq!(from_pcm16(mulaw(0xFF)), 0.0);
    }

    #[test]
    fn every_byte_decodes_and_encodes_back_to_itself() {
        for byte in 0..=u8::MAX {
            let decoded = alaw(byte);
            assert_eq!(alaw_of(decoded), byte, "A-law {byte:#04x}");
            let decoded = mulaw(byte);
            let back = mulaw_of(decoded);
            assert_eq!(mulaw(back), decoded, "mu-law {byte:#04x}");
        }
    }
}
