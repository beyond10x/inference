//! Speech recognition alone, on the local machine: reads a WAV file, streams it in 20 ms frames
//! through the sherpa-onnx recogniser of `inference-sherpa-onnx` over the STT contract, and
//! returns each span it commits. The package depends on `inference-stt` and the adapter's `stt`
//! feature only (A-01, `docs/design/inference.md:1137`); `examples/transcribe.rs` prints what
//! [`run`] returns.
//!
//! [`read_wav`] takes a RIFF WAV file of 16-bit PCM, 32-bit float, G.711 mu-law or A-law at any
//! sample rate and channel count; the recogniser averages the channels and resamples to the
//! checkpoint's 16 kHz.

use std::error::Error;
use std::fmt;
use std::num::{NonZeroU16, NonZeroU32};
use std::path::{Path, PathBuf};

use inference_sherpa_onnx::stt::{OpenRefused, SherpaOnnxRecogniser};
use inference_stt::inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_stt::inference_core::envelope::EventEnvelope;
use inference_stt::inference_core::id::{NodeId, RunId, StreamId};
use inference_stt::inference_text::TurnId;
use inference_stt::{Recogniser, RecognitionHandle, RecognitionRequest, TranscriptEvent};

/// The audio of a WAV file: its format and its sample data, channels interleaved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wav {
    pub format: AudioFormat,
    pub data: Vec<u8>,
}

/// Why a file was not read as a WAV file of a format the recogniser takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WavRefused {
    /// The file does not start with a RIFF `WAVE` header.
    NotWav,
    /// The file has no `fmt ` chunk before its `data` chunk, or no `data` chunk.
    MissingChunk(&'static str),
    /// The `fmt ` chunk is shorter than the 16 bytes of a PCM format.
    ShortFormat,
    /// The format tag and sample width are not 16-bit PCM, 32-bit float, mu-law or A-law.
    UnsupportedEncoding { tag: u16, bits: u16 },
    /// The sample rate or the channel count is zero.
    ZeroRateOrChannels,
}

impl fmt::Display for WavRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotWav => f.write_str("the file is not a RIFF WAVE file"),
            Self::MissingChunk(chunk) => write!(f, "the file has no `{chunk}` chunk"),
            Self::ShortFormat => f.write_str("the `fmt ` chunk is shorter than 16 bytes"),
            Self::UnsupportedEncoding { tag, bits } => write!(
                f,
                "format tag {tag} at {bits} bits is not 16-bit PCM, 32-bit float, mu-law or A-law"
            ),
            Self::ZeroRateOrChannels => f.write_str("the sample rate or channel count is zero"),
        }
    }
}

impl Error for WavRefused {}

/// Why [`run`] did not recognise the file.
#[derive(Debug)]
pub enum RunFailed {
    /// The WAV file at `path` could not be read.
    Unreadable {
        path: PathBuf,
        error: std::io::Error,
    },
    /// The file at `path` is not a WAV file the recogniser takes.
    Wav { path: PathBuf, refused: WavRefused },
    /// The checkpoint directory does not hold the pinned checkpoint.
    Open(OpenRefused),
}

impl fmt::Display for RunFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable { path, error } => write!(f, "{}: {error}", path.display()),
            Self::Wav { path, refused } => write!(f, "{}: {refused}", path.display()),
            Self::Open(refused) => fmt::Display::fmt(refused, f),
        }
    }
}

impl Error for RunFailed {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Unreadable { error, .. } => Some(error),
            Self::Wav { refused, .. } => Some(refused),
            Self::Open(refused) => Some(refused),
        }
    }
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// The encoding of format tag `tag` at `bits` per sample; `WAVE_FORMAT_EXTENSIBLE` (0xFFFE) has
/// been resolved to its subformat's tag before.
const fn encoding(tag: u16, bits: u16) -> Option<AudioEncoding> {
    match (tag, bits) {
        (1, 16) => Some(AudioEncoding::PcmS16le),
        (3, 32) => Some(AudioEncoding::PcmF32le),
        (6, 8) => Some(AudioEncoding::PcmAlaw),
        (7, 8) => Some(AudioEncoding::PcmMulaw),
        _ => None,
    }
}

/// Reads `bytes`, the content of a RIFF WAV file: the format of its `fmt ` chunk and the
/// samples of its `data` chunk, cut to whole sample groups.
///
/// # Errors
///
/// [`WavRefused`] naming what is missing or not supported.
pub fn read_wav(bytes: &[u8]) -> Result<Wav, WavRefused> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(WavRefused::NotWav);
    }
    let mut format = None;
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let size = usize::try_from(u32_at(bytes, at + 4)).unwrap_or(usize::MAX);
        let body = &bytes[at + 8..at.saturating_add(8).saturating_add(size).min(bytes.len())];
        match &bytes[at..at + 4] {
            b"fmt " => {
                if body.len() < 16 {
                    return Err(WavRefused::ShortFormat);
                }
                let mut tag = u16_at(body, 0);
                if tag == 0xFFFE && body.len() >= 26 {
                    tag = u16_at(body, 24);
                }
                let channels = u16_at(body, 2);
                let sample_rate = u32_at(body, 4);
                let bits = u16_at(body, 14);
                let encoding =
                    encoding(tag, bits).ok_or(WavRefused::UnsupportedEncoding { tag, bits })?;
                format = Some(AudioFormat {
                    encoding,
                    sample_rate: NonZeroU32::new(sample_rate)
                        .ok_or(WavRefused::ZeroRateOrChannels)?,
                    channels: NonZeroU16::new(channels).ok_or(WavRefused::ZeroRateOrChannels)?,
                });
            }
            b"data" => {
                let format = format.ok_or(WavRefused::MissingChunk("fmt "))?;
                let group = group_bytes(format);
                let whole = body.len() / group * group;
                return Ok(Wav {
                    format,
                    data: body[..whole].to_vec(),
                });
            }
            _ => {}
        }
        at = at
            .saturating_add(8)
            .saturating_add(size)
            .saturating_add(size % 2);
    }
    Err(WavRefused::MissingChunk(if format.is_some() {
        "data"
    } else {
        "fmt "
    }))
}

/// The bytes one sample of every channel of `format` takes.
fn group_bytes(format: AudioFormat) -> usize {
    usize::try_from(format.encoding.width()).expect("a sample is at most 4 bytes")
        * usize::from(format.channels.get())
}

/// The audio of `wav` as consecutive frames of 20 ms (the last one shorter), from sample
/// position 0.
#[must_use]
pub fn frames(wav: &Wav) -> Vec<AudioFrame> {
    let group = group_bytes(wav.format);
    let per_frame = usize::try_from(wav.format.sample_rate.get() / 50)
        .unwrap_or(1)
        .max(1);
    let mut position = 0_u64;
    wav.data
        .chunks(per_frame * group)
        .map(|chunk| {
            let sample_count = u32::try_from(chunk.len() / group).expect("a frame fits u32");
            let frame = AudioFrame {
                format: wav.format,
                position: SampleClockPosition::new(position).expect("a file fits the sample clock"),
                sample_count,
                data: chunk.to_vec(),
            };
            position += u64::from(sample_count);
            frame
        })
        .collect()
}

/// Opens the pinned checkpoint in `checkpoint_dir`, streams the WAV file at `wav` through one
/// recognition in 20 ms frames, and returns one line per committed span, in commit order.
///
/// # Errors
///
/// [`RunFailed`] when the file cannot be read or is not a WAV file the recogniser takes, or when
/// the directory does not hold the pinned checkpoint.
///
/// # Panics
///
/// When the recogniser refuses to start its only recognition, or refuses input before its end
/// was marked, which it does not.
pub fn run(checkpoint_dir: &Path, wav: &Path) -> Result<Vec<String>, RunFailed> {
    let bytes = std::fs::read(wav).map_err(|error| RunFailed::Unreadable {
        path: wav.to_path_buf(),
        error,
    })?;
    let audio = read_wav(&bytes).map_err(|refused| RunFailed::Wav {
        path: wav.to_path_buf(),
        refused,
    })?;
    let mut recogniser = SherpaOnnxRecogniser::open(checkpoint_dir).map_err(RunFailed::Open)?;
    let mut recognition = recogniser
        .start(RecognitionRequest::new(
            RunId::generate(),
            TurnId::generate(),
            StreamId::generate(),
            NodeId::generate(),
            "text_out",
        ))
        .expect("a fresh recogniser runs no other recognition");
    let mut lines = Vec::new();
    for frame in frames(&audio) {
        recognition.offer_frame(frame).expect("input has not ended");
        committed_lines(recognition.by_ref(), &mut lines);
    }
    recognition.end_input().expect("input ends once");
    committed_lines(recognition.by_ref(), &mut lines);
    Ok(lines)
}

/// Appends a line for each committed span among `events` to `lines`.
fn committed_lines(
    events: impl Iterator<Item = EventEnvelope<TranscriptEvent>>,
    lines: &mut Vec<String>,
) {
    for envelope in events {
        if let TranscriptEvent::Committed(committed) = envelope.payload {
            lines.push(format!("committed: {}", committed.text));
        }
    }
}
