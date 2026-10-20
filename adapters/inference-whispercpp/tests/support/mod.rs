//! What the tests recognising speech with the pinned checkpoint share: the checkpoint file named
//! by `INFERENCE_WHISPERCPP_STT_MODEL`, the fixture `tests/fixtures/question-16k.wav`, the
//! recorded speech clip named by `INFERENCE_WHISPERCPP_STT_CLIP`, the audio formats the fixture
//! is offered in, 20 ms frames in each, and a feed that
//! offers frames to a recognition and keeps what was delivered after each. Nothing here
//! downloads weights.
#![allow(dead_code)]

use std::num::{NonZeroU16, NonZeroU32};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use inference_stt::inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_stt::inference_core::envelope::EventEnvelope;
use inference_stt::inference_core::id::{NodeId, RunId, StreamId};
use inference_stt::inference_text::TurnId;
use inference_stt::{RecognitionHandle, RecognitionRequest, TranscriptEvent};
use inference_whispercpp::stt::WhisperCppRecogniser;

/// The variable naming the pinned checkpoint file, `ggml-base.en.bin`.
pub const MODEL: &str = "INFERENCE_WHISPERCPP_STT_MODEL";

/// The sample rate of the fixture.
pub const CLIP_RATE: u32 = 16_000;

/// The fixture: "What is the capital of France?", eSpeak NG, 16 kHz mono `pcm_s16le`.
pub const FIXTURE: &str = "tests/fixtures/question-16k.wav";

/// Words of the fixture as a whole, as the recogniser writes them in lowercase.
pub const FIXTURE_WORDS: [&str; 2] = ["capital", "france"];

const fn format(encoding: AudioEncoding, sample_rate: u32, channels: u16) -> AudioFormat {
    AudioFormat {
        encoding,
        sample_rate: NonZeroU32::new(sample_rate).expect("the rate is not zero"),
        channels: NonZeroU16::new(channels).expect("the channel count is not zero"),
    }
}

/// The fixture as recorded: `pcm_s16le`, 16 kHz, mono.
pub const PCM16_16K_MONO: AudioFormat = format(AudioEncoding::PcmS16le, CLIP_RATE, 1);

/// The fixture as 32-bit float.
pub const F32_16K_MONO: AudioFormat = format(AudioEncoding::PcmF32le, CLIP_RATE, 1);

/// The fixture at 8 kHz, which the recogniser resamples to 16 kHz.
pub const PCM16_8K_MONO: AudioFormat = format(AudioEncoding::PcmS16le, 8_000, 1);

/// The fixture on two channels, which the recogniser averages to mono.
pub const PCM16_16K_STEREO: AudioFormat = format(AudioEncoding::PcmS16le, CLIP_RATE, 2);

/// Every format the conformance tests offer the fixture in and the recogniser was observed to
/// recognise it from: the `audio_formats` of the capability record.
pub const FORMATS: [AudioFormat; 4] = [
    PCM16_16K_MONO,
    F32_16K_MONO,
    PCM16_8K_MONO,
    PCM16_16K_STEREO,
];

/// The file `INFERENCE_WHISPERCPP_STT_MODEL` names.
///
/// # Panics
///
/// When the variable is not set, naming it.
pub fn model_path() -> PathBuf {
    std::env::var_os(MODEL)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("set {MODEL} to the path of the pinned ggml-base.en.bin"))
}

/// Opens the pinned checkpoint of [`model_path`] at most `max_concurrency` recognitions at once.
pub fn open(max_concurrency: i64) -> WhisperCppRecogniser {
    WhisperCppRecogniser::open_with(model_path(), max_concurrency)
        .unwrap_or_else(|refused| panic!("{MODEL} names the pinned checkpoint: {refused}"))
}

/// The samples of the WAV file at `path`, 16-bit mono, from its `data` chunk.
pub fn wav_samples(path: &Path) -> Vec<i16> {
    let wav = std::fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let mut at = 12;
    while at + 8 <= wav.len() {
        let size = u32::from_le_bytes([wav[at + 4], wav[at + 5], wav[at + 6], wav[at + 7]]);
        let size = usize::try_from(size).expect("a chunk size fits usize");
        if &wav[at..at + 4] == b"data" {
            return wav[at + 8..(at + 8 + size).min(wav.len())]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&sample| i16::from_le_bytes(sample))
                .collect();
        }
        at += 8 + size + size % 2;
    }
    panic!("{} has no data chunk", path.display());
}

/// The samples of the fixture.
pub fn fixture() -> Vec<i16> {
    wav_samples(&Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE))
}

/// The variable naming the recorded speech clip, the LibriSpeech `test_wavs/0.wav` of the
/// sherpa-onnx checkpoint `sherpa-onnx-streaming-zipformer-en-2023-06-26`.
pub const SPEECH_CLIP: &str = "INFERENCE_WHISPERCPP_STT_CLIP";

/// The sha256 of the recorded speech clip.
pub const SPEECH_CLIP_SHA256: &str =
    "6bc58a4efdf20daac252b6b1502632601a71efe0308f6757dc1eda34891a7e4f";

/// The recorded speech clip: 16 kHz mono `pcm_s16le`, its transcript as the checkpoint's
/// `test_wavs/trans.txt` gives it, and words of it as the recogniser writes them in lowercase.
pub struct SpeechClip {
    pub samples: Vec<i16>,
    pub transcript: &'static str,
    pub words: [&'static str; 3],
}

/// The clip `INFERENCE_WHISPERCPP_STT_CLIP` names, after checking its sha256 against
/// [`SPEECH_CLIP_SHA256`].
///
/// # Panics
///
/// When the variable is not set, naming it, or the file is not the pinned clip.
pub fn speech_clip() -> SpeechClip {
    let path = std::env::var_os(SPEECH_CLIP)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            panic!("set {SPEECH_CLIP} to the path of the LibriSpeech test_wavs/0.wav")
        });
    let wav = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("{SPEECH_CLIP}: {}: {error}", path.display()));
    let sha256: String = Sha256::digest(&wav)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(
        sha256,
        SPEECH_CLIP_SHA256,
        "{} is the pinned clip",
        path.display()
    );
    SpeechClip {
        samples: wav_samples(&path),
        transcript: "AFTER EARLY NIGHTFALL THE YELLOW LAMPS WOULD LIGHT UP HERE AND THERE THE \
                     SQUALID QUARTER OF THE BROTHELS",
        words: ["nightfall", "lamps", "brothels"],
    }
}

/// `samples`, 16 kHz mono, in `format`: at 8 kHz each pair of samples is averaged into one, and
/// every channel carries the same samples. Only `pcm_s16le` and `pcm_f32le` at 16 or 8 kHz are
/// made.
fn convert(samples: &[i16], format: AudioFormat) -> Vec<f32> {
    let mono: Vec<f32> = match format.sample_rate.get() {
        CLIP_RATE => samples.iter().map(|&sample| f32::from(sample)).collect(),
        8_000 => samples
            .chunks(2)
            .map(|pair| pair.iter().map(|&sample| f32::from(sample)).sum::<f32>() / 2.0)
            .collect(),
        rate => panic!("the tests make no frames at {rate} Hz"),
    };
    let channels = usize::from(format.channels.get());
    mono.iter()
        .flat_map(|&sample| std::iter::repeat_n(sample, channels))
        .collect()
}

/// `samples`, 16 kHz mono, as consecutive 20 ms frames of `format` from sample position 0.
pub fn frames(samples: &[i16], format: AudioFormat) -> Vec<AudioFrame> {
    let channels = usize::from(format.channels.get());
    let per_frame = usize::try_from(format.sample_rate.get() / 50).expect("a frame fits usize");
    let interleaved = convert(samples, format);
    let mut position = 0_u64;
    interleaved
        .chunks(per_frame * channels)
        .map(|chunk| {
            let data: Vec<u8> = match format.encoding {
                AudioEncoding::PcmS16le => chunk
                    .iter()
                    .flat_map(|&sample| {
                        // The mean of two 16-bit samples is within 16 bits.
                        let sample = sample.round() as i16;
                        sample.to_le_bytes()
                    })
                    .collect(),
                AudioEncoding::PcmF32le => chunk
                    .iter()
                    .flat_map(|&sample| (sample / 32_768.0).to_le_bytes())
                    .collect(),
                encoding => panic!("the tests make no {encoding:?} frames"),
            };
            let sample_count = u32::try_from(chunk.len() / channels).expect("a frame fits u32");
            let frame = AudioFrame {
                format,
                position: SampleClockPosition::new(position).expect("the clip is short"),
                sample_count,
                data,
            };
            position += u64::from(sample_count);
            frame
        })
        .collect()
}

/// A request on a fresh stream of a fresh run and turn, port `text_out`.
pub fn request() -> RecognitionRequest {
    RecognitionRequest::new(
        RunId::generate(),
        TurnId::generate(),
        StreamId::generate(),
        NodeId::generate(),
        "text_out",
    )
}

/// One event a recognition delivered, with what had been offered to it when it was.
#[derive(Debug)]
pub struct Delivery {
    pub envelope: EventEnvelope<TranscriptEvent>,
    /// The end of the frames offered so far, in samples per channel of their format.
    pub offered_through: i64,
    /// Whether input had not ended yet.
    pub before_end: bool,
}

/// What a recognition delivered while frames were offered to it one at a time, every event it
/// had ready drained after each frame and after the end of input.
#[derive(Debug, Default)]
pub struct Feed {
    pub deliveries: Vec<Delivery>,
    offered_through: i64,
    /// The frames offered and the frames the recognition had taken, after each drain before the
    /// end of input.
    pub taken_before_end: Vec<(usize, usize)>,
    frames_offered: usize,
}

impl Feed {
    /// Offers `frame` to `handle`, then drains it.
    pub fn offer<H: RecognitionHandle>(&mut self, handle: &mut H, frame: AudioFrame) {
        let end = frame.position.get() + u64::from(frame.sample_count);
        handle.offer_frame(frame).expect("input has not ended");
        self.frames_offered += 1;
        self.offered_through = i64::try_from(end).expect("the clip is short");
        self.drain(handle, true);
        self.taken_before_end
            .push((self.frames_offered, handle.output().frames_taken()));
    }

    /// Ends the input of `handle`, then drains it.
    pub fn end<H: RecognitionHandle>(&mut self, handle: &mut H) {
        handle.end_input().expect("input ends once");
        self.drain(handle, false);
    }

    fn drain<H: RecognitionHandle>(&mut self, handle: &mut H, before_end: bool) {
        for envelope in handle.by_ref() {
            self.deliveries.push(Delivery {
                envelope,
                offered_through: self.offered_through,
                before_end,
            });
        }
    }

    /// The texts of the committed spans, in commit order, in lowercase and joined by spaces.
    pub fn committed(&self) -> String {
        self.deliveries
            .iter()
            .filter_map(|delivery| match &delivery.envelope.payload {
                TranscriptEvent::Committed(committed) => Some(committed.text.to_lowercase()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The text every delivered event carries, in lowercase and joined by spaces.
    pub fn every_text(&self) -> String {
        self.deliveries
            .iter()
            .map(|delivery| match &delivery.envelope.payload {
                TranscriptEvent::Hypothesis(event) => event.text.to_lowercase(),
                TranscriptEvent::Revision(event) => event.text.to_lowercase(),
                TranscriptEvent::Committed(event) => event.text.to_lowercase(),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Prints every delivery to stderr, for the test's output.
    pub fn print(&self, label: &str) {
        for delivery in &self.deliveries {
            let envelope = &delivery.envelope;
            eprintln!(
                "{label}: seq {} ts {} offered {} {} {:?}",
                envelope.sequence,
                envelope.timestamp,
                delivery.offered_through,
                if delivery.before_end {
                    "before end"
                } else {
                    "after end"
                },
                envelope.payload
            );
        }
        eprintln!("{label}: committed {:?}", self.committed());
    }
}

/// Offers `frames` to `handle` one at a time, then ends its input.
pub fn stream<H: RecognitionHandle>(handle: &mut H, frames: Vec<AudioFrame>) -> Feed {
    let mut feed = Feed::default();
    for frame in frames {
        feed.offer(handle, frame);
    }
    feed.end(handle);
    feed
}
