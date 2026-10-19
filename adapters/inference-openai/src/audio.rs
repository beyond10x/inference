//! Conversion of `inference-audio` frames to the Realtime PCM16 input format.

use inference_audio::{AudioEncoding, AudioFrame};

use crate::error::RealtimeError;

/// The sample rate of the `audio/pcm` format.
const RATE: u64 = 24_000;

/// The frame as 16-bit little-endian PCM, mono, 24 kHz: the `audio/pcm` input format.
///
/// A frame already in that format is returned as is. Otherwise channels are averaged to mono,
/// float samples are scaled to 16 bits and clamped, and the rate is converted by linear
/// interpolation into `floor(sample_count * 24000 / rate)` samples. Each frame is converted on its
/// own, with no state carried between frames: a frame whose length is not a whole number of
/// output samples drops the fraction, and the last output sample of a frame does not look ahead
/// into the next one.
///
/// # Errors
/// [`RealtimeError::Audio`] when the frame's data does not hold `sample_count` samples of its
/// format.
pub fn encode_pcm16_24k(frame: &AudioFrame) -> Result<Vec<u8>, RealtimeError> {
    let format = frame.format;
    let expected = format.data_len(frame.sample_count);
    if frame.data.len() as u64 != expected {
        return Err(RealtimeError::Audio(format!(
            "the frame holds {} bytes of data, its {} samples of {:?} at {} channels take {expected}",
            frame.data.len(),
            frame.sample_count,
            format.encoding,
            format.channels
        )));
    }
    let rate = u64::from(format.sample_rate.get());
    let channels = usize::from(format.channels.get());
    if format.encoding == AudioEncoding::PcmS16le && channels == 1 && rate == RATE {
        return Ok(frame.data.clone());
    }
    let width = format.encoding.width() as usize;
    let mono: Vec<f32> = frame
        .data
        .chunks_exact(width * channels)
        .map(|sample| {
            let sum: f32 = sample
                .chunks_exact(width)
                .map(|channel| match format.encoding {
                    AudioEncoding::PcmS16le => {
                        f32::from(i16::from_le_bytes([channel[0], channel[1]])) / 32_768.0
                    }
                    AudioEncoding::PcmF32le => {
                        f32::from_le_bytes([channel[0], channel[1], channel[2], channel[3]])
                    }
                })
                .sum();
            sum / channels as f32
        })
        .collect();
    let resampled = resample(&mono, rate);
    let mut pcm = Vec::with_capacity(resampled.len() * 2);
    for sample in resampled {
        let scaled = (sample * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16;
        pcm.extend_from_slice(&scaled.to_le_bytes());
    }
    Ok(pcm)
}

/// `input` at `rate` hertz, linearly interpolated to 24 kHz.
fn resample(input: &[f32], rate: u64) -> Vec<f32> {
    if rate == RATE || input.is_empty() {
        return input.to_vec();
    }
    let count = input.len() as u64 * RATE / rate;
    let last = input.len() - 1;
    (0..count)
        .map(|index| {
            let numerator = index * rate;
            let at = (numerator / RATE) as usize;
            let fraction = (numerator % RATE) as f32 / RATE as f32;
            let next = input[(at + 1).min(last)];
            input[at] + (next - input[at]) * fraction
        })
        .collect()
}
