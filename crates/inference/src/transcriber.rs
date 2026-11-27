//! `Transcriber`: audio frames or a file in, committed spans out (`docs/design/sdk.md` §3.2).

use std::future::Future;
use std::num::{NonZeroU16, NonZeroU32};
use std::path::{Path, PathBuf};

use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_facade::live::LiveSession;
use inference_pipeline::PlannedPipeline;
use inference_text::Span;

use crate::agent::{Handle, read_to_end};
use crate::{Description, Error, Payload, Recipe, Result, Stack};

const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<Transcriber>();
};

/// How many frames of a file make one second: each frame holds 20 ms.
const FRAMES_PER_SECOND: u32 = 50;

/// Audio in, committed spans out, over the recipe `transcribe`:
/// [`Transcriber::transcribe_frames`] takes frames and [`Transcriber::transcribe_file`] a WAV
/// file. Each call opens one session over the stack's `stt` binding, gives it the audio as one
/// turn and closes it, so a transcriber takes any number of recordings, one after another.
pub struct Transcriber {
    handle: Handle,
}

impl std::fmt::Debug for Transcriber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transcriber")
            .field("recipe", &self.handle.describe().recipe)
            .finish_non_exhaustive()
    }
}

/// A refusal of the file at `path`, saying what to do about it.
fn refused(path: &Path, message: &str, source: Option<hound::Error>) -> Error {
    Error::Layer {
        message: format!("the audio file {}: {message}", path.display()),
        source: source.map(|error| Box::new(error) as _),
    }
}

/// The frames of the WAV file at `path`, 20 ms each, in the file's own sample rate and channels:
/// 16-bit integer samples as PCM16, 32-bit float samples as 32-bit float PCM.
fn read_wav(path: &Path) -> Result<Vec<AudioFrame>> {
    let unreadable = |error| refused(path, "it cannot be read as a WAV file", Some(error));
    let mut reader = hound::WavReader::open(path).map_err(unreadable)?;
    let spec = reader.spec();
    let (encoding, data) = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, 16) => {
            let mut data = Vec::new();
            for sample in reader.samples::<i16>() {
                data.extend_from_slice(&sample.map_err(unreadable)?.to_le_bytes());
            }
            (AudioEncoding::PcmS16le, data)
        }
        (hound::SampleFormat::Float, 32) => {
            let mut data = Vec::new();
            for sample in reader.samples::<f32>() {
                data.extend_from_slice(&sample.map_err(unreadable)?.to_le_bytes());
            }
            (AudioEncoding::PcmF32le, data)
        }
        (format, bits) => {
            return Err(refused(
                path,
                &format!(
                    "its samples are {bits}-bit {format:?}; convert it to 16-bit integer or \
                     32-bit float PCM"
                ),
                None,
            ));
        }
    };
    let (Some(sample_rate), Some(channels)) = (
        NonZeroU32::new(spec.sample_rate),
        NonZeroU16::new(spec.channels),
    ) else {
        return Err(refused(
            path,
            "its header names no sample rate or no channel",
            None,
        ));
    };
    let format = AudioFormat {
        encoding,
        sample_rate,
        channels,
    };
    let sample_bytes = match encoding {
        AudioEncoding::PcmF32le => 4,
        _ => 2,
    };
    let per_frame = (sample_rate.get() / FRAMES_PER_SECOND).max(1);
    let frame_bytes = per_frame as usize * usize::from(channels.get()) * sample_bytes;
    let mut frames = Vec::with_capacity(data.len().div_ceil(frame_bytes));
    let mut position = 0_u64;
    for chunk in data.chunks(frame_bytes) {
        let samples = chunk.len() / (usize::from(channels.get()) * sample_bytes);
        let Ok(sample_count) = u32::try_from(samples) else {
            break;
        };
        if sample_count == 0 {
            break;
        }
        let Some(at) = SampleClockPosition::new(position) else {
            return Err(refused(
                path,
                "it is longer than a sample clock counts",
                None,
            ));
        };
        frames.push(AudioFrame {
            format,
            position: at,
            sample_count,
            data: chunk[..samples * usize::from(channels.get()) * sample_bytes].to_vec(),
        });
        position += u64::from(sample_count);
    }
    Ok(frames)
}

impl Transcriber {
    /// A transcriber over the stack the process names: [`Transcriber::new_with`] over the stack
    /// resolved from the environment and the stack files.
    ///
    /// # Errors
    ///
    /// Those of [`Stack::from_env`] and of [`Transcriber::new_with`].
    pub fn new() -> Result<Self> {
        Self::new_with(&Stack::from_env()?)
    }

    /// A transcriber over `stack`, which it shares and does not take: the recipe `transcribe` is
    /// planned over the stack's `stt` binding, and no session is opened yet.
    ///
    /// # Errors
    ///
    /// Those of [`Stack::resolve_with`] over no environment and of [`Recipe::instantiate`]:
    /// [`Error::RoleUnbound`] when the stack binds no `stt`.
    pub fn new_with(stack: &Stack) -> Result<Self> {
        Ok(Self {
            handle: Handle::plan(Recipe::transcribe(), stack)?,
        })
    }

    /// The committed spans of `frames`, in the order the recogniser committed them: the frames
    /// are one turn, and what the recogniser still holds at their end is committed there. Opens
    /// one session and closes it. The frames are taken before the future is first polled.
    ///
    /// # Errors
    ///
    /// Those of [`AgentBuilder::start`](crate::AgentBuilder::start),
    /// [`Agent::send_audio`](crate::Agent::send_audio) and [`Agent::close`](crate::Agent::close);
    /// the error of the session's first [`Event::Error`](crate::Event::Error), such as a
    /// recognition the recogniser refused.
    pub fn transcribe_frames(
        &mut self,
        frames: impl IntoIterator<Item = AudioFrame>,
    ) -> impl Future<Output = Result<Vec<Span>>> + Send {
        let frames: Vec<AudioFrame> = frames.into_iter().collect();
        self.heard(frames)
    }

    /// The committed spans of the WAV file at `path`, as [`Transcriber::transcribe_frames`] of
    /// its samples in 20 ms frames, in the file's own sample rate and channels. The file holds
    /// 16-bit integer or 32-bit float PCM.
    ///
    /// # Errors
    ///
    /// [`Error::Layer`] when the file cannot be read as WAV or holds another sample format, its
    /// message naming the file and what to do; those of [`Transcriber::transcribe_frames`].
    pub async fn transcribe_file(&mut self, path: impl AsRef<Path>) -> Result<Vec<Span>> {
        let path: PathBuf = path.as_ref().to_path_buf();
        let read = tokio::task::spawn_blocking(move || read_wav(&path));
        let frames = read.await.map_err(|error| Error::Layer {
            message: "reading the audio file did not finish".to_owned(),
            source: Some(Box::new(error)),
        })??;
        self.heard(frames).await
    }

    /// One session over `frames`: sent, closed, and its committed spans read.
    async fn heard(&mut self, frames: Vec<AudioFrame>) -> Result<Vec<Span>> {
        let agent = self.handle.open().await?;
        let read = read_to_end(agent.events(), drop);
        let sent = async {
            let mut sent = Ok(());
            for frame in frames {
                sent = agent.send_audio(frame).await;
                if sent.is_err() {
                    break;
                }
            }
            let closed = agent.close().await;
            sent.and(closed)
        };
        let (sent, read) = tokio::join!(sent, read);
        sent.and(read)?;
        Ok(agent.spans())
    }

    /// What the transcriber runs: the recipe `transcribe`, the stack's bindings and their
    /// sources, and the planned pipeline's description.
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
    /// first call, the one planned when the transcriber was built.
    #[must_use]
    pub fn pipeline(&self) -> &PlannedPipeline {
        self.handle.pipeline()
    }
}
