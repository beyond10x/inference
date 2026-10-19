//! Synthetic audio: tone and silence, generated, never recorded.
//!
//! [`synthesize`] renders a list of [`Stretch`]es into consecutive frames of 16 kHz mono
//! `pcm_s16le`, each [`FRAME_SAMPLES`] long, as the mock recogniser's [`frames`](super::frames)
//! are, the first at sample 0. A tone is a sine of its frequency and peak amplitude, its phase
//! counted from its own first sample; silence is all zeros. [`tone_silence_tone_silence`] is the
//! fixture of the endpoint controller: two utterances, each followed by a pause.
//!
//! ```
//! # use inference_testkit::stt::FRAME_SAMPLES;
//! # use inference_testkit::stt::synthetic_audio::{FIXTURE, tone_silence_tone_silence};
//! let frames = tone_silence_tone_silence();
//! let lengths: usize = FIXTURE.iter().map(|stretch| stretch.frames()).sum();
//! assert_eq!(frames.len(), lengths);
//! assert_eq!(frames[1].position.get(), u64::from(FRAME_SAMPLES));
//! ```

use std::f64::consts::TAU;
use std::num::{NonZeroU16, NonZeroU32};

use inference_stt::inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};

use super::FRAME_SAMPLES;

/// The sample rate of the synthetic frames, in hertz.
pub const SAMPLE_RATE: u32 = 16_000;

/// A stretch of synthetic audio, `frames` frames of [`FRAME_SAMPLES`] samples long.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Stretch {
    /// A sine of `hertz` at peak `amplitude`, full scale 1.
    Tone {
        frames: usize,
        hertz: u32,
        amplitude: f64,
    },
    /// All zeros.
    Silence { frames: usize },
}

impl Stretch {
    /// How many frames the stretch lasts.
    #[must_use]
    pub const fn frames(&self) -> usize {
        match self {
            Self::Tone { frames, .. } | Self::Silence { frames } => *frames,
        }
    }

    /// The sample `index` of the stretch, counted from its first.
    fn sample(&self, index: u64) -> i16 {
        match self {
            Self::Silence { .. } => 0,
            Self::Tone {
                hertz, amplitude, ..
            } => {
                let at = (index % u64::from(SAMPLE_RATE)) as f64 / f64::from(SAMPLE_RATE);
                let value = amplitude.clamp(0.0, 1.0) * (TAU * f64::from(*hertz) * at).sin();
                (value * f64::from(i16::MAX)).round() as i16
            }
        }
    }
}

/// The frames of 300 ms of a 440 Hz tone at half scale.
const UTTERANCE: Stretch = Stretch::Tone {
    frames: 30,
    hertz: 440,
    amplitude: 0.5,
};

/// The frames of 500 ms of silence.
const PAUSE: Stretch = Stretch::Silence { frames: 50 };

/// The fixture of the endpoint controller: 300 ms of tone, 500 ms of silence, 300 ms of tone,
/// 500 ms of silence.
pub const FIXTURE: [Stretch; 4] = [UTTERANCE, PAUSE, UTTERANCE, PAUSE];

/// The frames of `stretches`, one after the other; see the module documentation.
///
/// # Panics
///
/// When the position of a frame would lie above `i64::MAX`.
#[must_use]
pub fn synthesize(stretches: &[Stretch]) -> Vec<AudioFrame> {
    let format = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(SAMPLE_RATE).expect("16000 is not zero"),
        channels: NonZeroU16::new(1).expect("1 is not zero"),
    };
    let mut frames = Vec::new();
    let mut position = 0_u64;
    for stretch in stretches {
        for frame in 0..stretch.frames() as u64 {
            let first = frame * u64::from(FRAME_SAMPLES);
            let data = (first..first + u64::from(FRAME_SAMPLES))
                .flat_map(|index| stretch.sample(index).to_le_bytes())
                .collect();
            frames.push(AudioFrame {
                format,
                position: SampleClockPosition::new(position)
                    .expect("the fixture stays within i64::MAX samples"),
                sample_count: FRAME_SAMPLES,
                data,
            });
            position += u64::from(FRAME_SAMPLES);
        }
    }
    frames
}

/// The frames of [`FIXTURE`].
#[must_use]
pub fn tone_silence_tone_silence() -> Vec<AudioFrame> {
    synthesize(&FIXTURE)
}

#[cfg(test)]
mod tests {
    use super::{FIXTURE, Stretch, synthesize, tone_silence_tone_silence};
    use crate::stt::FRAME_SAMPLES;

    fn peak(frame: &inference_stt::inference_audio::AudioFrame) -> i16 {
        frame
            .data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|bytes| i16::from_le_bytes(*bytes).saturating_abs())
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn the_fixture_is_tone_then_silence_twice_on_one_sample_clock() {
        let frames = tone_silence_tone_silence();
        assert_eq!(frames.len(), 160);
        for (index, frame) in frames.iter().enumerate() {
            assert_eq!(
                frame.position.get(),
                index as u64 * u64::from(FRAME_SAMPLES)
            );
            assert_eq!(frame.sample_count, FRAME_SAMPLES);
            assert_eq!(
                frame.data.len() as u64,
                frame.format.data_len(FRAME_SAMPLES)
            );
            let tone = (0..30).contains(&index) || (80..110).contains(&index);
            if tone {
                assert!(peak(frame) > i16::MAX / 3, "frame {index} carries the tone");
            } else {
                assert_eq!(peak(frame), 0, "frame {index} is silent");
            }
        }
        assert_eq!(FIXTURE.iter().map(Stretch::frames).sum::<usize>(), 160);
    }

    #[test]
    fn synthesis_is_deterministic() {
        assert_eq!(synthesize(&FIXTURE), synthesize(&FIXTURE));
    }
}
