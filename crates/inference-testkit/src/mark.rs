//! The marked reply audio of the mocks: a frame that says which mock produced it, for which reply
//! and as which frame of it, so a test reading what a device played can tell one reply's frames
//! from another's.
//!
//! A marked frame is `pcm_s16le` and never silent. Its first sample is [`MAGIC`], its second the
//! [`MockId`], its third the reply index and its fourth the frame index within the reply, each
//! as the 16 bits of a sample; every later sample is [`FILL`]. A frame of fewer than four
//! samples, of another encoding, or any of whose samples differ from that layout decodes to
//! nothing.

use inference_duplex::inference_audio::{AudioEncoding, AudioFrame};

/// The first sample of a marked frame.
pub const MAGIC: i16 = 0x2A5A;

/// Every sample of a marked frame after the fourth: fixed and not silence.
pub const FILL: i16 = 1000;

/// The samples a mark takes before the fill.
const HEADER: usize = 4;

/// The mock that produced a marked frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MockId {
    /// The duplex mock, `MockSpeechModel`.
    Speech,
    /// The mock synthesiser, `MockSynthesiser`.
    Synthesiser,
}

impl MockId {
    const fn sample(self) -> i16 {
        match self {
            Self::Speech => 1,
            Self::Synthesiser => 2,
        }
    }

    const fn of(sample: i16) -> Option<Self> {
        match sample {
            1 => Some(Self::Speech),
            2 => Some(Self::Synthesiser),
            _ => None,
        }
    }
}

/// What a marked frame says: the mock that produced it, the reply it belongs to and its index
/// within that reply, both from 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Mark {
    pub mock: MockId,
    pub reply: u32,
    pub frame: u32,
}

/// `shape` with its samples marked as frame `frame` of reply `reply` of `mock`: the same format,
/// position and sample count, the data replaced. `shape` is `pcm_s16le` of at least four samples,
/// as every frame the mocks emit; another is returned unmarked. The reply and frame indices are
/// kept to their low 16 bits.
#[must_use]
pub fn encode(mock: MockId, reply: u32, frame: u32, shape: AudioFrame) -> AudioFrame {
    let samples = shape.data.len() / 2;
    if shape.format.encoding != AudioEncoding::PcmS16le || samples < HEADER {
        return shape;
    }
    #[allow(clippy::cast_possible_truncation)]
    let header = [
        MAGIC,
        mock.sample(),
        reply as u16 as i16,
        frame as u16 as i16,
    ];
    let data = header
        .into_iter()
        .chain(std::iter::repeat_n(FILL, samples - HEADER))
        .flat_map(i16::to_le_bytes)
        .collect();
    AudioFrame { data, ..shape }
}

/// What `frame` says, when it is a marked frame; `None` for any other, silence among them.
#[must_use]
pub fn decode(frame: &AudioFrame) -> Option<Mark> {
    if frame.format.encoding != AudioEncoding::PcmS16le || !frame.data.len().is_multiple_of(2) {
        return None;
    }
    let mut samples = frame
        .data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| i16::from_le_bytes(*pair));
    if samples.next()? != MAGIC {
        return None;
    }
    let mock = MockId::of(samples.next()?)?;
    #[allow(clippy::cast_sign_loss)]
    let reply = u32::from(samples.next()? as u16);
    #[allow(clippy::cast_sign_loss)]
    let index = u32::from(samples.next()? as u16);
    samples.all(|sample| sample == FILL).then_some(Mark {
        mock,
        reply,
        frame: index,
    })
}

#[cfg(test)]
mod tests {
    use std::num::{NonZeroU16, NonZeroU32};

    use inference_duplex::inference_audio::{
        AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
    };

    use super::{Mark, MockId, decode, encode};

    fn silence(samples: u32) -> AudioFrame {
        let format = AudioFormat {
            encoding: AudioEncoding::PcmS16le,
            sample_rate: NonZeroU32::new(16_000).expect("16 kHz"),
            channels: NonZeroU16::MIN,
        };
        AudioFrame {
            format,
            position: SampleClockPosition::new(640).expect("a position"),
            sample_count: samples,
            data: vec![0; usize::try_from(samples).expect("small") * 2],
        }
    }

    #[test]
    fn encode_and_decode_round_trip_for_both_mocks() {
        for mock in [MockId::Speech, MockId::Synthesiser] {
            for (reply, frame) in [(0, 0), (1, 1), (170, 3)] {
                let marked = encode(mock, reply, frame, silence(320));
                assert_eq!(marked.sample_count, 320);
                assert_eq!(marked.data.len(), 640);
                assert_eq!(marked.position, silence(320).position);
                assert!(marked.data.iter().any(|byte| *byte != 0), "never silent");
                assert_eq!(
                    decode(&marked),
                    Some(Mark { mock, reply, frame }),
                    "{mock:?} reply {reply} frame {frame}"
                );
            }
        }
    }

    #[test]
    fn a_frame_of_silence_decodes_to_none() {
        assert_eq!(decode(&silence(320)), None);
        assert_eq!(decode(&silence(160)), None);
    }

    #[test]
    fn a_frame_whose_fill_differs_decodes_to_none() {
        let mut marked = encode(MockId::Speech, 0, 0, silence(320));
        marked.data[100] ^= 1;
        assert_eq!(decode(&marked), None);
    }
}
