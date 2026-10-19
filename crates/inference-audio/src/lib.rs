//! The audio vocabulary of the inference framework.
//!
//! `inference-audio` holds the `inference.audio` vocabulary of `ess/domains/audio.yaml`: an
//! [`AudioFrame`] is a bounded binary buffer of samples in one [`AudioFormat`], placed on the
//! stream's sample clock by the [`SampleClockPosition`] of its first sample. A sample clock
//! position is audio frame time, distinct from inference execution time, wall time and playback
//! time. The crate carries no codec, resampler or format conversion. Serialized names are the ESS
//! names, and serialized values stay within the ESS primitives: every `Integer` fits an `i64`, and
//! `Bytes` is a padded standard base64 string. A field the specification does not declare is
//! refused.

use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize};

/// `inference.audio.AudioEncoding`: how the samples of a frame are encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioEncoding {
    /// Little-endian signed 16-bit integer PCM.
    PcmS16le,
    /// Little-endian 32-bit float PCM.
    PcmF32le,
}

impl AudioEncoding {
    /// Every encoding, in the order `ess/domains/audio.yaml` lists them.
    pub const ALL: [Self; 2] = [Self::PcmS16le, Self::PcmF32le];

    /// The bytes one sample of one channel takes: 2 for `pcm_s16le`, 4 for `pcm_f32le`.
    pub const fn width(self) -> u64 {
        match self {
            Self::PcmS16le => 2,
            Self::PcmF32le => 4,
        }
    }
}

/// `inference.audio.AudioFormat`: the encoding, sample rate in hertz and channel count of a frame.
/// The sample rate is at least one and fits a `u32`; the channel count is at least one and fits a
/// `u16`. Both narrowings are declared in `ess/domains/audio.yaml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioFormat {
    pub encoding: AudioEncoding,
    pub sample_rate: NonZeroU32,
    pub channels: NonZeroU16,
}

impl AudioFormat {
    /// The length in bytes of the data of a frame of `sample_count` samples per channel in this
    /// format: `sample_count * channels * width(encoding)`. It is below `2^50` and never
    /// overflows.
    pub const fn data_len(&self, sample_count: u32) -> u64 {
        sample_count as u64 * self.channels.get() as u64 * self.encoding.width()
    }
}

/// `inference.audio.SampleClockPosition`: a non-negative sample index on a stream's sample clock at
/// the format's sample rate. It is audio frame time. The index lies in `0..=i64::MAX`, the range
/// of the ESS `Integer`; [`SampleClockPosition::new`] and deserialization refuse anything above.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SampleClockPosition(u64);

impl SampleClockPosition {
    /// The largest position, `i64::MAX`.
    pub const MAX: Self = Self(i64::MAX as u64);

    /// The position at sample `index`, or `None` when `index` is above `i64::MAX`.
    pub const fn new(index: u64) -> Option<Self> {
        if index <= Self::MAX.0 {
            Some(Self(index))
        } else {
            None
        }
    }

    /// The sample index.
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The audio frame time of this position at the sample rate of `format`, truncated toward zero
    /// to whole nanoseconds.
    pub fn frame_time(self, format: &AudioFormat) -> Duration {
        samples_at(self.0, format.sample_rate)
    }
}

impl<'de> Deserialize<'de> for SampleClockPosition {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let index = u64::deserialize(deserializer)?;
        Self::new(index).ok_or_else(|| {
            serde::de::Error::custom(format_args!(
                "sample clock position {index} is above i64::MAX, the ESS Integer range"
            ))
        })
    }
}

/// The time `samples` samples take at `sample_rate` hertz, truncated toward zero to whole
/// nanoseconds. The remainder is below the sample rate, so the nanosecond product stays below
/// `u32::MAX * 10^9` and never overflows `u64`.
fn samples_at(samples: u64, sample_rate: NonZeroU32) -> Duration {
    const NANOS_PER_SECOND: u64 = 1_000_000_000;
    let rate = u64::from(sample_rate.get());
    let seconds = samples / rate;
    let nanos = (samples % rate) * NANOS_PER_SECOND / rate;
    Duration::new(seconds, nanos as u32)
}

/// `inference.audio.AudioFrame`: a bounded binary buffer of audio that owns its bytes. `position`
/// is the sample clock position of its first sample and `sample_count` the number of samples per
/// channel, which fits a `u32` as `ess/domains/audio.yaml` declares.
///
/// `data` is ESS `Bytes` on the serde wire, a padded standard base64 string in both directions.
/// The design's rule against base64 JSON is about runtime transport, not this conformance wire.
///
/// A well-formed frame carries [`AudioFormat::data_len`] of its `sample_count` bytes of data. The
/// frame does not refuse one that does not: `ess/domains/audio.yaml` cannot state the invariant,
/// so the wire admits what the specification admits, and a contract that produces frames, such as
/// the TTS contract, refuses a mismatched one instead.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioFrame {
    pub format: AudioFormat,
    pub position: SampleClockPosition,
    pub sample_count: u32,
    #[serde(with = "ess_bytes")]
    pub data: Vec<u8>,
}

impl AudioFrame {
    /// The duration of the frame: `sample_count` samples at the format's sample rate, truncated
    /// toward zero to whole nanoseconds.
    pub fn duration(&self) -> Duration {
        samples_at(u64::from(self.sample_count), self.format.sample_rate)
    }
}

/// The ESS `Bytes` wire form: a padded standard base64 string. Decoding refuses a missing or
/// surplus pad, any character outside the standard alphabet and non-zero trailing bits: only the
/// canonical encoding of a buffer is admitted, as ESS itself requires of retained bytes.
mod ess_bytes {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer>(data: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(data))
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        STANDARD.decode(text).map_err(serde::de::Error::custom)
    }
}
