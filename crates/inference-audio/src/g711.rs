//! G.711 μ-law and A-law companding (ITU-T G.711) between 16-bit linear PCM and one byte per
//! sample, and the conversion of an [`AudioFrame`] between `pcm_s16le`, `pcm_mulaw` and
//! `pcm_alaw`.

use std::fmt;

use crate::{AudioEncoding, AudioFrame};

/// The bias μ-law adds to a magnitude, 33 in the standard's 14-bit units, here in 16-bit units.
const MULAW_BIAS: i32 = 0x84;
/// The largest 16-bit magnitude μ-law encodes without clipping, 8159 in 14-bit units.
const MULAW_CLIP: i32 = 32_635;

/// The G.711 μ-law code word of a 16-bit linear sample. The sample is read on the standard's
/// 14-bit scale (the two low bits only round), magnitudes above 32635 clip to the largest code
/// word, and the code word is the one whose decision interval holds the sample, inverted for the
/// line as the standard requires. Zero encodes to `0xFF`.
pub const fn encode_mulaw(sample: i16) -> u8 {
    let value = sample as i32;
    let (magnitude, mask) = if value < 0 {
        (-value, 0x7F)
    } else {
        (value, 0xFF)
    };
    let biased = (if magnitude > MULAW_CLIP {
        MULAW_CLIP
    } else {
        magnitude
    }) + MULAW_BIAS;
    let segment = segment_of(biased >> 7);
    let step = (biased >> (segment + 3)) & 0x0F;
    ((segment << 4) as u8 | step as u8) ^ mask
}

/// The 16-bit linear sample G.711 μ-law decodes a code word to: the decoder output value of the
/// standard's table, on its 14-bit scale times four. The range is -32124 to 32124; `0xFF` and
/// `0x7F` both decode to zero.
pub const fn decode_mulaw(code: u8) -> i16 {
    let word = !code;
    let segment = ((word >> 4) & 0x07) as i32;
    let step = (word & 0x0F) as i32;
    let magnitude = (((step << 3) + MULAW_BIAS) << segment) - MULAW_BIAS;
    (if word & 0x80 != 0 {
        -magnitude
    } else {
        magnitude
    }) as i16
}

/// The G.711 A-law code word of a 16-bit linear sample. The sample is read on the standard's
/// 13-bit scale (the three low bits only round), and the code word is the one whose decision
/// interval holds the sample, its even bits inverted for the line as the standard requires. Zero
/// encodes to `0xD5`.
pub const fn encode_alaw(sample: i16) -> u8 {
    let value = (sample as i32) >> 3;
    let (magnitude, mask) = if value >= 0 {
        (value, 0xD5)
    } else {
        (-value - 1, 0x55)
    };
    let segment = segment_of(magnitude >> 4);
    let step = if segment < 2 {
        (magnitude >> 1) & 0x0F
    } else {
        (magnitude >> segment) & 0x0F
    };
    ((segment << 4) as u8 | step as u8) ^ mask
}

/// The 16-bit linear sample G.711 A-law decodes a code word to: the decoder output value of the
/// standard's table, on its 13-bit scale times eight. The range is -32256 to 32256; A-law has no
/// zero, and `0xD5` and `0x55` decode to 8 and -8.
pub const fn decode_alaw(code: u8) -> i16 {
    let word = code ^ 0x55;
    let segment = ((word >> 4) & 0x07) as i32;
    let step = (word & 0x0F) as i32;
    let magnitude = if segment == 0 {
        (step << 4) + 8
    } else {
        ((step << 4) + 0x108) << (segment - 1)
    };
    (if word & 0x80 != 0 {
        magnitude
    } else {
        -magnitude
    }) as i16
}

/// The G.711 segment of a magnitude already shifted so that segment 0 is below 2: the number of
/// significant bits above the first, at most 7.
const fn segment_of(shifted: i32) -> i32 {
    let mut segment = 0;
    while segment < 7 && shifted >> (segment + 1) != 0 {
        segment += 1;
    }
    segment
}

/// Why [`AudioFrame::to_encoding`] refused a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionError {
    /// The frame's data is not [`crate::AudioFormat::data_len`] of its sample count long.
    DataLength {
        /// The length the format and sample count require.
        expected: u64,
        /// The length of the data.
        actual: u64,
    },
    /// No conversion between these encodings: only `pcm_s16le`, `pcm_mulaw` and `pcm_alaw`
    /// convert, into one another.
    Unsupported {
        /// The frame's encoding.
        from: AudioEncoding,
        /// The encoding asked for.
        to: AudioEncoding,
    },
}

impl fmt::Display for ConversionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DataLength { expected, actual } => write!(
                f,
                "the frame carries {actual} bytes of data where its format and sample count \
                 require {expected}"
            ),
            Self::Unsupported { from, to } => {
                write!(f, "no conversion from {from:?} to {to:?}")
            }
        }
    }
}

impl std::error::Error for ConversionError {}

/// The data length a frame's format and sample count require and the length it carries, when the
/// two differ.
pub(crate) fn data_len_mismatch(frame: &AudioFrame) -> Option<(u64, u64)> {
    let expected = frame.format.data_len(frame.sample_count);
    let actual = frame.data.len() as u64;
    (expected != actual).then_some((expected, actual))
}

impl AudioFrame {
    /// This frame in `encoding`: the same sample rate, channels, position and sample count, each
    /// sample converted on its own. `pcm_s16le` converts to and from `pcm_mulaw` and `pcm_alaw`
    /// by G.711, and μ-law and A-law convert into one another through the 16-bit sample; the
    /// same encoding returns a copy. `pcm_f32le` does not convert. A frame whose data is not the
    /// length its format and sample count require is refused, and nothing panics.
    pub fn to_encoding(&self, encoding: AudioEncoding) -> Result<AudioFrame, ConversionError> {
        if let Some((expected, actual)) = data_len_mismatch(self) {
            return Err(ConversionError::DataLength { expected, actual });
        }
        let from = self.format.encoding;
        let unsupported = ConversionError::Unsupported { from, to: encoding };
        let data = match (from, encoding) {
            (AudioEncoding::PcmF32le, _) | (_, AudioEncoding::PcmF32le) => return Err(unsupported),
            (from, to) if from == to => self.data.clone(),
            (AudioEncoding::PcmS16le, to) => {
                let encode = if to == AudioEncoding::PcmMulaw {
                    encode_mulaw
                } else {
                    encode_alaw
                };
                self.data
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| encode(i16::from_le_bytes(*pair)))
                    .collect()
            }
            (from, to) => {
                let decode = if from == AudioEncoding::PcmMulaw {
                    decode_mulaw
                } else {
                    decode_alaw
                };
                match to {
                    AudioEncoding::PcmS16le => self
                        .data
                        .iter()
                        .flat_map(|&code| decode(code).to_le_bytes())
                        .collect(),
                    AudioEncoding::PcmMulaw => self
                        .data
                        .iter()
                        .map(|&code| encode_mulaw(decode(code)))
                        .collect(),
                    AudioEncoding::PcmAlaw | AudioEncoding::PcmF32le => self
                        .data
                        .iter()
                        .map(|&code| encode_alaw(decode(code)))
                        .collect(),
                }
            }
        };
        let mut format = self.format;
        format.encoding = encoding;
        Ok(AudioFrame {
            format,
            position: self.position,
            sample_count: self.sample_count,
            data,
        })
    }
}
