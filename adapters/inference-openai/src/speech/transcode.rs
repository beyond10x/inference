//! The endpoint's 24 kHz PCM as samples of the configured format, one segment at a time.

use inference_tts::inference_audio::{
    AudioEncoding, AudioFormat, ResampleError, Resampler, encode_alaw, encode_mulaw,
};

use super::protocol::RESPONSE_RATE;

/// Turns the body of one response into the data of frames of the configured format.
///
/// The bytes are read as 16-bit little-endian samples, a byte of a sample split across chunks
/// kept for the next. Between rates the samples go through `inference-audio`'s resampler, whose
/// output lags its input by [`Resampler::delay`] samples: the first that many output samples are
/// dropped, and [`Transcoder::finish`] feeds the filter the silence that brings the last input
/// samples out, so the segment's audio starts and ends where the response's does. A μ-law or
/// A-law frame is encoded from the samples by the G.711 codec.
pub struct Transcoder {
    encoding: AudioEncoding,
    resampler: Option<Resampler>,
    skip: usize,
    carry: Option<u8>,
    input: Vec<i16>,
    output: Vec<i16>,
}

impl Transcoder {
    /// A transcoder of a fresh response into frames of `format`, which the configuration has
    /// already been validated to allow.
    pub fn new(format: AudioFormat) -> Result<Self, ResampleError> {
        let rate = format.sample_rate.get();
        let resampler = if rate == RESPONSE_RATE {
            None
        } else {
            Some(Resampler::new(RESPONSE_RATE, rate)?)
        };
        let skip = resampler
            .as_ref()
            .map_or(0, |resampler| resampler.delay() as usize);
        Ok(Self {
            encoding: format.encoding,
            resampler,
            skip,
            carry: None,
            input: Vec::new(),
            output: Vec::new(),
        })
    }

    /// The data of the samples `bytes` completes, after the bytes kept from the chunk before:
    /// the samples' count and their bytes in the configured encoding.
    pub fn push(&mut self, bytes: &[u8]) -> (u32, Vec<u8>) {
        self.input.clear();
        let mut rest = bytes;
        if let Some(low) = self.carry.take() {
            match rest.split_first() {
                Some((&high, after)) => {
                    self.input.push(i16::from_le_bytes([low, high]));
                    rest = after;
                }
                None => {
                    self.carry = Some(low);
                    return (0, Vec::new());
                }
            }
        }
        let (pairs, odd) = rest.as_chunks::<2>();
        self.input
            .extend(pairs.iter().map(|pair| i16::from_le_bytes(*pair)));
        self.carry = odd.first().copied();
        let input = std::mem::take(&mut self.input);
        let data = self.convert(&input);
        self.input = input;
        data
    }

    /// The data of the samples still held in the filter, once the body has ended.
    ///
    /// # Errors
    /// The count of bytes left of a sample, when the body ended on half of one.
    pub fn finish(&mut self) -> Result<(u32, Vec<u8>), usize> {
        if self.carry.is_some() {
            return Err(1);
        }
        let Some(resampler) = self.resampler.as_ref() else {
            return Ok((0, Vec::new()));
        };
        let delay = u64::from(resampler.delay());
        let from = u64::from(resampler.input_rate());
        let to = u64::from(resampler.output_rate());
        let padding = usize::try_from((delay * from).div_ceil(to)).unwrap_or(0);
        Ok(self.convert(&vec![0; padding]))
    }

    /// Resamples `input`, drops what the filter's delay still owes, and encodes the rest.
    fn convert(&mut self, input: &[i16]) -> (u32, Vec<u8>) {
        self.output.clear();
        let samples: &[i16] = match self.resampler.as_mut() {
            None => input,
            Some(resampler) => {
                resampler.process(input, &mut self.output);
                let dropped = self.skip.min(self.output.len());
                self.skip -= dropped;
                &self.output[dropped..]
            }
        };
        let count = u32::try_from(samples.len()).unwrap_or(u32::MAX);
        let samples = &samples[..count as usize];
        let data = match self.encoding {
            AudioEncoding::PcmMulaw => samples.iter().map(|&s| encode_mulaw(s)).collect(),
            AudioEncoding::PcmAlaw => samples.iter().map(|&s| encode_alaw(s)).collect(),
            AudioEncoding::PcmS16le | AudioEncoding::PcmF32le => {
                samples.iter().flat_map(|s| s.to_le_bytes()).collect()
            }
        };
        (count, data)
    }
}
