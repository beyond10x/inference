//! `Voice`: text in, audio frames out, played or returned (`docs/design/sdk.md` §3.2).

use std::fs::OpenOptions;
use std::io::BufWriter;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::time::Duration;

use inference_audio::{AudioEncoding, AudioFrame};
use inference_facade::live::LiveSession;
use inference_pipeline::PlannedPipeline;

use crate::agent::{Handle, read_to_end};
use crate::{Agent, Description, Error, Event, Payload, Recipe, Result, Stack};

const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<Voice>();
};

/// One sentence of a text and its audio, as [`Voice::synthesise_clips`] returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    /// The sentence, trimmed, with the marks that end it.
    pub text: String,
    /// Every frame the synthesiser sent for the sentence alone, in order.
    pub frames: Vec<AudioFrame>,
    /// How long the frames play: their samples per channel at their sample rate, truncated
    /// toward zero to whole nanoseconds.
    pub duration: Duration,
}

/// A clip written as a WAV file by [`Voice::synthesise_clips_to`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipFile {
    /// The file: `clip-001.wav` onwards in the directory the call was given.
    pub path: PathBuf,
    /// The clip's [`Clip::duration`], which is the length of the file's audio.
    pub duration: Duration,
}

/// The marks that end a sentence, one or more of them followed by whitespace or the end. A run
/// of marks needs no handling of its own: only its last mark is followed by whitespace.
const SENTENCE_ENDS: [char; 3] = ['.', '!', '?'];

/// The most clips [`Voice::synthesise_clips_to`] writes: their names have three digits,
/// `clip-001.wav` to `clip-999.wav`.
const MAX_CLIP_FILES: usize = 999;

/// The sentences of `text`, in order. A sentence ends at one or more of `.`, `!` and `?`
/// followed by whitespace or the end of the text; the text between ends is trimmed, and a part
/// that is empty after trimming is no sentence. There is no list of abbreviations.
fn sentences(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((_, mark)) = chars.next() {
        if !SENTENCE_ENDS.contains(&mark) {
            continue;
        }
        if let Some(&(at, next)) = chars.peek()
            && next.is_whitespace()
        {
            parts.push(&text[start..at]);
            start = at;
        }
    }
    parts.push(&text[start..]);
    parts
        .into_iter()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect()
}

/// The time `samples` samples take at `rate` hertz, truncated toward zero to whole nanoseconds.
fn samples_at(samples: u64, rate: NonZeroU32) -> Duration {
    let nanos = u128::from(samples) * 1_000_000_000 / u128::from(rate.get());
    Duration::from_nanos_u128(nanos)
}

/// How long `frames` play: the samples of each run of frames at one sample rate, at that rate.
fn duration_of(frames: &[AudioFrame]) -> Duration {
    let mut total = Duration::ZERO;
    let mut run: Option<(NonZeroU32, u64)> = None;
    for frame in frames {
        let samples = u64::from(frame.sample_count);
        match &mut run {
            Some((rate, held)) if *rate == frame.format.sample_rate => *held += samples,
            _ => {
                if let Some((rate, held)) = run.replace((frame.format.sample_rate, samples)) {
                    total += samples_at(held, rate);
                }
            }
        }
    }
    if let Some((rate, held)) = run {
        total += samples_at(held, rate);
    }
    total
}

/// A refusal about the file or directory at `path`, saying what to do about it.
fn refused(
    path: &Path,
    message: &str,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
) -> Error {
    Error::Layer {
        message: format!("{}: {message}", path.display()),
        source,
    }
}

/// The name `ess/domains/audio.yaml` gives `encoding`.
const fn encoding_name(encoding: AudioEncoding) -> &'static str {
    match encoding {
        AudioEncoding::PcmS16le => "pcm_s16le",
        AudioEncoding::PcmF32le => "pcm_f32le",
        AudioEncoding::PcmMulaw => "pcm_mulaw",
        AudioEncoding::PcmAlaw => "pcm_alaw",
    }
}

/// The WAV format of `frames`, which share one format: 16-bit integer samples for `pcm_s16le`,
/// 32-bit float samples for `pcm_f32le`. Mu-law and A-law frames are refused, naming the
/// encoding, and so are frames of more than one format and no frame at all.
pub(crate) fn wav_spec<'a>(
    mut frames: impl Iterator<Item = &'a AudioFrame>,
) -> Result<hound::WavSpec> {
    let layer = |message: String| Error::Layer {
        message,
        source: None,
    };
    let Some(first) = frames.next() else {
        return Err(layer(
            "the synthesiser sent no audio, so no WAV file is written; check its binding".into(),
        ));
    };
    let format = first.format;
    if let Some(other) = frames.find(|frame| frame.format != format) {
        return Err(layer(format!(
            "the audio changes format from {format:?} to {:?}, and a WAV file holds one; bind a \
             synthesiser that speaks one format",
            other.format
        )));
    }
    let (bits_per_sample, sample_format) = match format.encoding {
        AudioEncoding::PcmS16le => (16, hound::SampleFormat::Int),
        AudioEncoding::PcmF32le => (32, hound::SampleFormat::Float),
        encoding @ (AudioEncoding::PcmMulaw | AudioEncoding::PcmAlaw) => {
            return Err(layer(format!(
                "the audio is {}, which is not written as a WAV file; bind a synthesiser that \
                 speaks pcm_s16le or pcm_f32le",
                encoding_name(encoding)
            )));
        }
    };
    Ok(hound::WavSpec {
        channels: format.channels.get(),
        sample_rate: format.sample_rate.get(),
        bits_per_sample,
        sample_format,
    })
}

/// Writes `frames` as a new WAV file of `spec` at `path`, which must not exist yet; the frames
/// are those [`wav_spec`] took. A file this call created is removed again when writing fails.
pub(crate) fn write_wav(path: &Path, spec: hound::WavSpec, frames: &[AudioFrame]) -> Result<()> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            refused(
                path,
                "the file cannot be created; it must not exist yet",
                Some(Box::new(error)),
            )
        })?;
    let written = (|| {
        let mut writer = hound::WavWriter::new(BufWriter::new(file), spec)?;
        for frame in frames {
            if spec.sample_format == hound::SampleFormat::Float {
                for sample in frame.data.as_chunks::<4>().0 {
                    writer.write_sample(f32::from_le_bytes(*sample))?;
                }
            } else {
                for sample in frame.data.as_chunks::<2>().0 {
                    writer.write_sample(i16::from_le_bytes(*sample))?;
                }
            }
        }
        writer.finalize()
    })();
    written.map_err(|error| {
        let _ = std::fs::remove_file(path);
        refused(
            path,
            "the WAV file cannot be written",
            Some(Box::new(error)),
        )
    })
}

/// Text in, audio out, over the recipe `speak`: [`Voice::synthesise`] returns the frames of a
/// text, [`Voice::synthesise_clips`] those of each of its sentences with their durations,
/// [`Voice::synthesise_clips_to`] writes those as WAV files, and [`Voice::play`] plays a text.
/// Each synthesis opens one session over the stack's `tts` binding, sends its text as one
/// committed segment and closes the session, so a voice says any number of texts, one after
/// another.
pub struct Voice {
    handle: Handle,
}

impl std::fmt::Debug for Voice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Voice")
            .field("recipe", &self.handle.describe().recipe)
            .finish_non_exhaustive()
    }
}

/// Gives `text` to the agent's session as one committed segment and closes the session, which
/// ends once the synthesiser has sent the last frame. The session is closed whether or not it
/// took the text.
async fn say(agent: &Agent, text: String) -> Result<()> {
    let sent = async {
        agent.send_text(text).await?;
        agent.commit().await
    }
    .await;
    let closed = agent.close().await;
    sent.and(closed)
}

impl Voice {
    /// A voice over the stack the process names: [`Voice::new_with`] over the stack resolved
    /// from the environment and the stack files.
    ///
    /// # Errors
    ///
    /// Those of [`Stack::from_env`] and of [`Voice::new_with`].
    pub fn new() -> Result<Self> {
        Self::new_with(&Stack::from_env()?)
    }

    /// A voice over `stack`, which it shares and does not take: the recipe `speak` is planned
    /// over the stack's `tts` binding, and no session is opened yet.
    ///
    /// # Errors
    ///
    /// Those of [`Stack::resolve_with`] over no environment and of [`Recipe::instantiate`]:
    /// [`Error::RoleUnbound`] when the stack binds no `tts`.
    pub fn new_with(stack: &Stack) -> Result<Self> {
        Ok(Self {
            handle: Handle::plan(Recipe::speak(), stack)?,
        })
    }

    /// The audio of `text`: every frame the synthesiser sent for it, in order. Opens one session
    /// and closes it.
    ///
    /// # Errors
    ///
    /// Those of [`AgentBuilder::start`](crate::AgentBuilder::start), [`Agent::commit`] and
    /// [`Agent::close`]; the error of the session's first [`Event::Error`], an
    /// [`Error::Provider`] when the synthesiser's provider failed.
    pub async fn synthesise(&mut self, text: impl Into<String>) -> Result<Vec<AudioFrame>> {
        let text = text.into();
        let agent = self.handle.open().await?;
        let mut frames = Vec::new();
        let read = read_to_end(agent.events(), |event| {
            if let Event::Audio(frame) = event {
                frames.push(frame);
            }
        });
        let (said, read) = tokio::join!(say(agent, text), read);
        said.and(read)?;
        Ok(frames)
    }

    /// One clip per sentence of `text`, in order: each sentence is synthesised alone, as
    /// [`Voice::synthesise`] does it, in a session of its own. A sentence ends at one or more of
    /// `.`, `!` and `?` followed by whitespace or the end of the text, and is trimmed; a text
    /// with no such end is one sentence. An empty or whitespace-only text returns no clip and
    /// opens no session.
    ///
    /// # Errors
    ///
    /// Those of [`Voice::synthesise`], for the first sentence that fails; the clips before it
    /// are not returned.
    pub async fn synthesise_clips(&mut self, text: impl Into<String>) -> Result<Vec<Clip>> {
        let text = text.into();
        let mut clips = Vec::new();
        for sentence in sentences(&text) {
            let frames = self.synthesise(sentence).await?;
            clips.push(Clip {
                text: sentence.to_owned(),
                duration: duration_of(&frames),
                frames,
            });
        }
        Ok(clips)
    }

    /// The clips of [`Voice::synthesise_clips`], each written as a WAV file into the existing
    /// directory `dir`: `clip-001.wav`, `clip-002.wav` and onwards, with each file's path and
    /// duration in clip order. `pcm_s16le` audio is written as 16-bit integer samples and
    /// `pcm_f32le` audio as 32-bit float samples. Nothing is written before every check passed,
    /// and no file is overwritten.
    ///
    /// # Errors
    ///
    /// An [`Error::Layer`] naming the path when `dir` is no existing directory or one of the
    /// files the text needs exists already, and naming the limit when the text has more than 999
    /// sentences, all before any session opens; naming the encoding when the audio is
    /// `pcm_mulaw` or `pcm_alaw`, before any file is written; and naming the file when one
    /// cannot be written, after removing the files this call wrote. Those of
    /// [`Voice::synthesise_clips`].
    pub async fn synthesise_clips_to(
        &mut self,
        text: impl Into<String>,
        dir: impl AsRef<Path>,
    ) -> Result<Vec<ClipFile>> {
        let dir = dir.as_ref();
        let text = text.into();
        if !dir.is_dir() {
            return Err(refused(dir, "no such directory; make it first", None));
        }
        let count = sentences(&text).len();
        if count > MAX_CLIP_FILES {
            return Err(refused(
                dir,
                &format!(
                    "the text has {count} sentences and at most {MAX_CLIP_FILES} clips are \
                     written, clip-001.wav to clip-{MAX_CLIP_FILES}.wav; split the text"
                ),
                None,
            ));
        }
        let paths: Vec<PathBuf> = (1..=count)
            .map(|n| dir.join(format!("clip-{n:03}.wav")))
            .collect();
        if let Some(taken) = paths.iter().find(|path| path.symlink_metadata().is_ok()) {
            return Err(refused(
                taken,
                "the file exists and is not overwritten; move it or name another directory",
                None,
            ));
        }
        let clips = self.synthesise_clips(text).await?;
        if clips.is_empty() {
            return Ok(Vec::new());
        }
        let spec = wav_spec(clips.iter().flat_map(|clip| &clip.frames))?;
        let mut written: Vec<ClipFile> = Vec::with_capacity(clips.len());
        for (path, clip) in paths.into_iter().zip(&clips) {
            if let Err(error) = write_wav(&path, spec, &clip.frames) {
                for file in &written {
                    let _ = std::fs::remove_file(&file.path);
                }
                return Err(error);
            }
            written.push(ClipFile {
                path,
                duration: clip.duration,
            });
        }
        Ok(written)
    }

    /// Plays the audio of `text` on `host`'s default speaker and returns once the speaker played
    /// the last frame. Opens one session and closes it; no microphone is opened.
    ///
    /// # Errors
    ///
    /// [`Error::Device`] when the speaker does not open, and those of
    /// [`Voice::synthesise`].
    #[cfg(feature = "audio-host")]
    pub async fn play(
        &mut self,
        text: impl Into<String>,
        host: impl inference_audio_device::AudioHost,
    ) -> Result<()> {
        let text = text.into();
        let agent = self.handle.open().await?;
        let read = read_to_end(agent.events(), drop);
        let played = async {
            let played = agent.play_during(&host, say(agent, text)).await;
            if played.is_err() {
                let _ = agent.close().await;
            }
            played
        };
        let (played, read) = tokio::join!(played, read);
        played.and(read)
    }

    /// What the voice runs: the recipe `speak`, the stack's bindings and their sources, and the
    /// planned pipeline's description.
    #[must_use]
    pub fn describe(&self) -> Description {
        self.handle.describe()
    }

    /// The live session of the last call, one layer down; none before the first call. After the
    /// call it is closed, and still answers for its ids and its usage.
    #[must_use]
    pub fn session(&self) -> Option<&LiveSession<Payload>> {
        self.handle.session()
    }

    /// The planned pipeline, one layer down: that of the last call's session, or, before the
    /// first call, the one planned when the voice was built.
    #[must_use]
    pub fn pipeline(&self) -> &PlannedPipeline {
        self.handle.pipeline()
    }
}

#[cfg(test)]
mod tests {
    use std::num::{NonZeroU16, NonZeroU32};

    use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};

    use super::{sentences, wav_spec, write_wav};

    #[test]
    fn a_sentence_ends_at_its_marks_before_whitespace_or_the_end() {
        assert_eq!(
            sentences(" One. Two!? Three...\tFour?! "),
            ["One.", "Two!?", "Three...", "Four?!"]
        );
        assert_eq!(
            sentences("Version 3.5 is out.Really? Yes"),
            ["Version 3.5 is out.Really?", "Yes"]
        );
        assert_eq!(sentences("Dr. Who"), ["Dr.", "Who"], "no abbreviations");
    }

    #[test]
    fn a_part_that_is_empty_after_trimming_is_no_sentence() {
        assert!(sentences("").is_empty());
        assert!(sentences(" \n\t ").is_empty());
        assert_eq!(sentences("no end at all"), ["no end at all"]);
        assert_eq!(sentences("?! . Hi."), ["?!", ".", "Hi."]);
    }

    /// Two stereo frames of three samples per channel in `encoding`, the samples' bytes given.
    fn stereo(encoding: AudioEncoding, bytes: &[Vec<u8>; 2]) -> Vec<AudioFrame> {
        let format = AudioFormat {
            encoding,
            sample_rate: NonZeroU32::new(22_050).expect("a rate"),
            channels: NonZeroU16::new(2).expect("two channels"),
        };
        bytes
            .iter()
            .enumerate()
            .map(|(index, data)| AudioFrame {
                format,
                position: SampleClockPosition::new(3 * index as u64).expect("a position"),
                sample_count: 3,
                data: data.clone(),
            })
            .collect()
    }

    /// Writes `frames` with [`wav_spec`] and [`write_wav`] into a new file and reads it back.
    fn written(frames: &[AudioFrame]) -> hound::WavReader<std::io::BufReader<std::fs::File>> {
        let dir = tempfile::tempdir().expect("a directory");
        let path = dir.path().join("clip.wav");
        let spec = wav_spec(frames.iter()).expect("a WAV format");
        write_wav(&path, spec, frames).expect("written");
        let reader = hound::WavReader::open(&path).expect("a WAV file");
        let spec = reader.spec();
        assert_eq!((spec.channels, spec.sample_rate), (2, 22_050));
        reader
    }

    #[test]
    fn integer_samples_in_stereo_are_written_sample_for_sample() {
        let samples: [[i16; 6]; 2] = [
            [1, -1, 12_345, -12_345, i16::MAX, i16::MIN],
            [258, -258, 7, -7, 1_000, -32_000],
        ];
        let bytes = samples.map(|frame| frame.iter().flat_map(|s| s.to_le_bytes()).collect());
        let mut reader = written(&stereo(AudioEncoding::PcmS16le, &bytes));
        let spec = reader.spec();
        assert_eq!(
            (spec.bits_per_sample, spec.sample_format),
            (16, hound::SampleFormat::Int)
        );
        let read: Vec<i16> = reader
            .samples::<i16>()
            .map(|s| s.expect("a sample"))
            .collect();
        assert_eq!(read, samples.concat());
    }

    #[test]
    fn float_samples_in_stereo_are_written_sample_for_sample() {
        let samples: [[f32; 6]; 2] = [
            [0.5, -0.5, 0.25, -0.75, 1.0, -1.0],
            [0.125, -0.0625, 0.333, -0.999, 1.0e-3, -2.5e-4],
        ];
        let bytes = samples.map(|frame| frame.iter().flat_map(|s| s.to_le_bytes()).collect());
        let mut reader = written(&stereo(AudioEncoding::PcmF32le, &bytes));
        let spec = reader.spec();
        assert_eq!(
            (spec.bits_per_sample, spec.sample_format),
            (32, hound::SampleFormat::Float)
        );
        let read: Vec<u32> = reader
            .samples::<f32>()
            .map(|s| s.expect("a sample").to_bits())
            .collect();
        let expected: Vec<u32> = samples.concat().iter().map(|s| s.to_bits()).collect();
        assert_eq!(read, expected, "bit for bit");
    }
}
