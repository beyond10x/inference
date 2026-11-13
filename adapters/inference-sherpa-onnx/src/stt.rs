//! A streaming speech recogniser over sherpa-onnx's online recogniser, behind feature `stt`.
//!
//! [`SherpaOnnxRecogniser`] implements the `inference-stt` contract over one pinned checkpoint,
//! [`CHECKPOINT`], a streaming Zipformer transducer for English at 16 kHz: [`FILES`] names the
//! four files it loads, each with its sha256, and [`SherpaOnnxRecogniser::open`] refuses a
//! directory whose files are missing or differ from the pin with [`OpenRefused`], naming the
//! file. The checkpoint's licences, source and revision are constants of this module and fill the
//! [`Model`] the recogniser declares ([`declare_model`]); its [`Adapter`] ([`declare_adapter`])
//! runs as native FFI in the calling process, declares the maximum concurrency it was opened
//! with, 1 by default, and claims [`Capability::IncrementalInput`] and
//! [`Capability::OutputStreaming`]: the online recogniser takes waveform before the utterance
//! ends and yields partial results before it does (`docs/design/inference.md:362`).
//!
//! Each recognition is one sherpa-onnx online stream on the recogniser's shared online
//! recogniser. Iterating its handle takes the frames offered so far through
//! [`TranscriptOutput::take_input`], decodes each to mono f32 ([`mono_samples`]), passes it to the
//! stream, which resamples what is not at 16 kHz, and decodes while the stream is ready. A stream
//! is given one sample rate only, that of its first frame, because sherpa-onnx ends the process
//! when one stream is given a second: a later frame at another rate is resampled to the first
//! by linear interpolation, each run of consecutive frames at one rate as one signal, with no
//! low-pass filter. A frame at a rate [`stream_rate`] refuses, below [`MIN_STREAM_RATE`] or above
//! [`MAX_STREAM_RATE`], is not passed to the stream at all, first or later, so neither the tail
//! padding nor the resampler is sized from it. The first non-empty partial result emits a
//! Hypothesis span, a changed result revises it, and an endpoint, by sherpa-onnx's endpoint
//! rules, commits it and resets the stream for the next utterance; an utterance with no text emits nothing. At the end of input
//! the stream is flushed and the last span committed.
//!
//! The recognition's usage record, the contract's [`TranscriptOutput::usage`], is its last event,
//! delivered right after the last commit: the audio offered (on a failed call, the audio taken)
//! and one request, as the contract measures them, and `local_compute_ms` `measured`, the wall
//! time each step spent passing frames
//! to sherpa-onnx and decoding them, summed. sherpa-onnx reports no quantity of its own.
//!
//! The crate's `stt_conformance` tests hold the recogniser to adapter conformance against the
//! pinned checkpoint: the claims observed, stream order and finalization, timestamps and format
//! transformations, cancellation, concurrency and provenance. Its capability record on the
//! reference machine, `records/stt-i9-10900k-cpu.json`, names them as the evidence of its claims.
//!
//! # Known limitation
//!
//! The checkpoint is trained on LibriSpeech alone, read audiobook speech, and does not recognise
//! synthetic speech: the eSpeak NG recording of "What is the capital of France?" in this crate's
//! test fixtures comes out as the single committed span "CAPITALS". Its own LibriSpeech clip,
//! [`REFERENCE_CLIP`], is recognised word for word.

use std::collections::VecDeque;
use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Waker, ready};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use sherpa_onnx::{
    OnlineModelConfig, OnlineRecognizer, OnlineRecognizerConfig, OnlineStream,
    OnlineTransducerModelConfig,
};

use inference_stt::inference_audio::{AudioEncoding, AudioFrame};
use inference_stt::inference_core::envelope::EventEnvelope;
use inference_stt::inference_core::model::{
    Adapter, AdapterDeclared, AdapterDescriptor, Capability, DeclareAdapter, DeclareModel,
    ExecutionForm, Locality, Modality, Model, ModelDeclared, ModelId, NonPositiveConcurrency,
};
use inference_stt::inference_text::{ReviseSpan, SpanId};
use inference_stt::{
    CONTRACT_REVISION, ConcurrencyLimitReached, InputEnded, Recogniser, RecognitionHandle,
    RecognitionRequest, RevisionRefused, TranscriptEvent, TranscriptOutput,
};

/// The STT contract the recogniser implements, re-exported with the vocabulary crates it
/// re-exports, so that a consumer of speech recognition depends on this crate alone.
pub use inference_stt;

/// The name of the pinned checkpoint.
pub const CHECKPOINT: &str = "sherpa-onnx-streaming-zipformer-en-2023-06-26";

/// The model family of the pinned checkpoint.
pub const FAMILY: &str = "Zipformer streaming transducer";

/// Where the pinned files are published: the model repository whose card declares the weight
/// licence at [`SOURCE_REVISION`].
pub const SOURCE_URL: &str =
    "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-2023-06-26";

/// The revision of [`SOURCE_URL`] the pinned files and their licence were read at.
pub const SOURCE_REVISION: &str = "672fbf1b30579d6585301139bb363f42a0ad4a24";

/// The licences of the code that runs the checkpoint: sherpa-onnx and onnxruntime, the native
/// library this crate links.
pub const CODE_LICENSE: &str = "Apache-2.0 (sherpa-onnx 1.13.8), MIT (onnxruntime)";

/// The licence of the weights, as the model card at [`SOURCE_REVISION`] declares it.
pub const WEIGHT_LICENSE: &str = "Apache-2.0";

/// Where the weights come from: the training data and its licence, the training recipe, the
/// export and the published files.
pub const PROVENANCE: &str = "Trained on LibriSpeech (CC BY 4.0) with the icefall streaming \
     Zipformer recipe (k2-fsa/icefall pull request 1058), exported to ONNX by the sherpa-onnx \
     project; the files of https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-2023-06-26 \
     at revision 672fbf1b30579d6585301139bb363f42a0ad4a24";

/// The backend the adapter runs on.
pub const BACKEND: &str = "sherpa-onnx 1.13.8";

/// The version the linked native library reports through [`native_version`].
pub const NATIVE_VERSION: &str = "1.13.8";

/// The maximum concurrency of [`SherpaOnnxRecogniser::open`].
pub const DEFAULT_MAX_CONCURRENCY: i64 = 1;

/// The silence appended before the end of input, in hundredths of a second, so that the stream's
/// last chunk is decoded rather than held back as context.
const TAIL_PADDING_HUNDREDTHS: usize = 66;

/// The lowest sample rate a frame is taken at, the narrowband telephone rate: below it a frame
/// is not speech the checkpoint can recognise, and resampling it to a stream's rate would
/// multiply its samples by the ratio of the two.
pub const MIN_STREAM_RATE: u32 = 8_000;

/// The highest sample rate a frame is taken at, the highest an audio interface commonly
/// delivers.
pub const MAX_STREAM_RATE: u32 = 384_000;

/// A sample rate outside [`MIN_STREAM_RATE`]..=[`MAX_STREAM_RATE`], refused before anything is
/// allocated or resampled from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsupportedRate {
    pub rate: u32,
}

impl fmt::Display for UnsupportedRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "a sample rate of {} Hz is outside the {MIN_STREAM_RATE} to {MAX_STREAM_RATE} Hz a \
             stream takes",
            self.rate
        )
    }
}

impl Error for UnsupportedRate {}

/// `frame_rate` when a stream takes frames at it.
///
/// # Errors
///
/// [`UnsupportedRate`] when `frame_rate` is below [`MIN_STREAM_RATE`] or above
/// [`MAX_STREAM_RATE`]; the recognition passes no frame at such a rate to its stream.
pub const fn stream_rate(frame_rate: u32) -> Result<u32, UnsupportedRate> {
    if frame_rate < MIN_STREAM_RATE || frame_rate > MAX_STREAM_RATE {
        return Err(UnsupportedRate { rate: frame_rate });
    }
    Ok(frame_rate)
}

/// The rate decision of one frame at `frame_rate` for a stream whose rate is `stream`: the
/// stream's rate, set to `frame_rate` by the first frame taken, which the frame is passed at,
/// resampled when it is at another. Nothing is set by a refused frame.
fn admit_rate(stream: &mut Option<u32>, frame_rate: u32) -> Result<u32, UnsupportedRate> {
    let frame_rate = stream_rate(frame_rate)?;
    Ok(*stream.get_or_insert(frame_rate))
}

/// The samples of silence appended at the end of input to a stream at `stream_rate`.
fn tail_padding(stream_rate: u32) -> Result<usize, UnsupportedRate> {
    let rate = self::stream_rate(stream_rate)?;
    Ok(usize::try_from(rate).unwrap_or(0) * TAIL_PADDING_HUNDREDTHS / 100)
}

/// What a pinned file is to the recogniser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileRole {
    /// The transducer's encoder.
    Encoder,
    /// The transducer's decoder, the prediction network.
    Decoder,
    /// The transducer's joiner.
    Joiner,
    /// The token table.
    Tokens,
}

/// One file of the pinned checkpoint: what it is, its name in the checkpoint directory and its
/// sha256 in lowercase hex.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CheckpointFile {
    pub role: FileRole,
    pub name: &'static str,
    pub sha256: &'static str,
}

/// The files the recogniser loads from the checkpoint directory, in the order they are checked.
pub const FILES: [CheckpointFile; 4] = [
    CheckpointFile {
        role: FileRole::Encoder,
        name: "encoder-epoch-99-avg-1-chunk-16-left-128.onnx",
        sha256: "a423883ce5754507fd941755ab0b5bc426a84ac670cbe21cf060e9e2c66dc660",
    },
    CheckpointFile {
        role: FileRole::Decoder,
        name: "decoder-epoch-99-avg-1-chunk-16-left-128.onnx",
        sha256: "7bf787f90b194b307e5a4ad6a34fadb4e748304c35f78a8d66358a05b13ee6ef",
    },
    CheckpointFile {
        role: FileRole::Joiner,
        name: "joiner-epoch-99-avg-1-chunk-16-left-128.onnx",
        sha256: "210591f72b3c56b8364f85f345dca240bc2b4c00632848f4aa923630d5639d3b",
    },
    CheckpointFile {
        role: FileRole::Tokens,
        name: "tokens.txt",
        sha256: "49e3c2646595fd907228b3c6787069658f67b17377c60aeb8619c4551b2316fb",
    },
];

/// A recording published with the checkpoint at [`SOURCE_REVISION`], for checking a checkpoint
/// directory end to end: its path in the directory, its sha256 and its transcript. The
/// recogniser does not load it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReferenceClip {
    pub name: &'static str,
    pub sha256: &'static str,
    pub transcript: &'static str,
}

/// The checkpoint's LibriSpeech clip `test_wavs/0.wav`, 16 kHz mono `pcm_s16le`, and its
/// transcript as the checkpoint's `test_wavs/trans.txt` gives it.
pub const REFERENCE_CLIP: ReferenceClip = ReferenceClip {
    name: "test_wavs/0.wav",
    sha256: "6bc58a4efdf20daac252b6b1502632601a71efe0308f6757dc1eda34891a7e4f",
    transcript: "AFTER EARLY NIGHTFALL THE YELLOW LAMPS WOULD LIGHT UP HERE AND THERE THE SQUALID \
                 QUARTER OF THE BROTHELS",
};

/// The checksum of the pinned checkpoint, the `checksum` of its [`Model`]: `sha256:<hex> <name>`
/// for each of [`FILES`], joined by `, `.
#[must_use]
pub fn checksum() -> String {
    FILES
        .iter()
        .map(|file| format!("sha256:{} {}", file.sha256, file.name))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Runs `inference.model.DeclareModel` for the pinned checkpoint: audio in, text out, with its
/// licences, checksum, provenance and source revision.
#[must_use]
pub fn declare_model() -> (Model, ModelDeclared) {
    Model::declare(DeclareModel {
        family: FAMILY.to_owned(),
        checkpoint: CHECKPOINT.to_owned(),
        code_license: CODE_LICENSE.to_owned(),
        weight_license: WEIGHT_LICENSE.to_owned(),
        checksum: checksum(),
        input_modalities: vec![Modality::Audio],
        output_modalities: vec![Modality::Text],
        provenance: PROVENANCE.to_owned(),
        source_revision: SOURCE_REVISION.to_owned(),
    })
}

/// Runs `inference.model.DeclareAdapter` for the recogniser of the model `model_id`: native FFI
/// in the same process on [`BACKEND`] with the checkpoint, implementing this `inference-stt`'s
/// [`CONTRACT_REVISION`], claiming incremental input and output streaming, at most
/// `max_concurrency` recognitions at once.
///
/// # Errors
///
/// [`NonPositiveConcurrency`] when `max_concurrency` is below 1; nothing is declared.
pub fn declare_adapter(
    model_id: ModelId,
    max_concurrency: i64,
) -> Result<(Adapter, AdapterDeclared), NonPositiveConcurrency> {
    Adapter::declare(DeclareAdapter {
        model_id,
        backend: format!("{BACKEND} ({CHECKPOINT})"),
        contract_revision: CONTRACT_REVISION.to_owned(),
        execution_form: ExecutionForm::NativeFfi,
        locality: Locality::SameProcess,
        capabilities: vec![Capability::IncrementalInput, Capability::OutputStreaming],
        max_concurrency,
        descriptor: AdapterDescriptor::default(),
    })
}

/// The version the linked sherpa-onnx native library reports.
#[must_use]
pub fn native_version() -> &'static str {
    sherpa_onnx::version()
}

/// The samples of `frame` as f32 in `-1.0..=1.0`, its channels averaged to mono. `pcm_s16le` and
/// the G.711 encodings are scaled by `1 / 32768`; `pcm_f32le` passes through. Only whole sample
/// groups (one sample of every channel) are decoded, at most `sample_count` of them: a frame
/// whose data is shorter or longer than its count declares yields what it holds of the count.
#[must_use]
pub fn mono_samples(frame: &AudioFrame) -> Vec<f32> {
    let encoding = frame.format.encoding;
    let width = usize::try_from(encoding.width()).expect("a sample is at most 4 bytes");
    let channels = frame.format.channels.get();
    let group = width * usize::from(channels);
    let groups =
        (frame.data.len() / group).min(usize::try_from(frame.sample_count).unwrap_or(usize::MAX));
    let scale = 1.0 / f32::from(channels);
    frame.data[..groups * group]
        .chunks_exact(group)
        .map(|samples| {
            samples
                .chunks_exact(width)
                .map(|sample| decode_sample(encoding, sample))
                .sum::<f32>()
                * scale
        })
        .collect()
}

/// One sample of `encoding`, `width` bytes long, as f32.
fn decode_sample(encoding: AudioEncoding, sample: &[u8]) -> f32 {
    const PCM_SCALE: f32 = 1.0 / 32_768.0;
    match encoding {
        AudioEncoding::PcmS16le => {
            f32::from(i16::from_le_bytes([sample[0], sample[1]])) * PCM_SCALE
        }
        AudioEncoding::PcmF32le => f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]),
        AudioEncoding::PcmMulaw => f32::from(mulaw(sample[0])) * PCM_SCALE,
        AudioEncoding::PcmAlaw => f32::from(alaw(sample[0])) * PCM_SCALE,
    }
}

/// G.711 mu-law to linear 16-bit PCM.
fn mulaw(byte: u8) -> i16 {
    let byte = !byte;
    let exponent = (byte >> 4) & 0x07;
    let mantissa = i16::from(byte & 0x0F);
    let magnitude = (((mantissa << 3) + 0x84) << exponent) - 0x84;
    if byte & 0x80 == 0 {
        magnitude
    } else {
        -magnitude
    }
}

/// G.711 A-law to linear 16-bit PCM.
fn alaw(byte: u8) -> i16 {
    let byte = byte ^ 0x55;
    let exponent = (byte >> 4) & 0x07;
    let mantissa = i16::from(byte & 0x0F);
    let magnitude = if exponent == 0 {
        (mantissa << 4) + 8
    } else {
        ((mantissa << 4) + 0x108) << (exponent - 1)
    };
    if byte & 0x80 == 0 {
        -magnitude
    } else {
        magnitude
    }
}

/// Why [`SherpaOnnxRecogniser::open_with`] refused; no model or adapter is declared either way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenRefused {
    /// The maximum concurrency is below 1.
    NonPositiveConcurrency(NonPositiveConcurrency),
    /// The pinned file at `path` is not there or is not a readable file.
    FileMissing { path: PathBuf, kind: io::ErrorKind },
    /// The file at `path` is not the pinned one: its sha256 is `actual`, the pin `expected`.
    ChecksumMismatch {
        path: PathBuf,
        expected: &'static str,
        actual: String,
    },
    /// The path of a pinned file is not Unicode, which sherpa-onnx's configuration requires.
    PathNotUnicode { path: PathBuf },
    /// sherpa-onnx refused to create the online recogniser from the files of `dir`.
    BackendRefused { dir: PathBuf },
}

impl fmt::Display for OpenRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonPositiveConcurrency(error) => fmt::Display::fmt(error, f),
            Self::FileMissing { path, kind } => write!(
                f,
                "the pinned checkpoint file {} is missing: {kind}",
                path.display()
            ),
            Self::ChecksumMismatch {
                path,
                expected,
                actual,
            } => write!(
                f,
                "{} is not the pinned file: its sha256 is {actual}, the pin {expected}",
                path.display()
            ),
            Self::PathNotUnicode { path } => write!(
                f,
                "the path {} is not Unicode, so sherpa-onnx cannot be given it",
                path.display()
            ),
            Self::BackendRefused { dir } => write!(
                f,
                "sherpa-onnx refused to create the online recogniser from {}",
                dir.display()
            ),
        }
    }
}

impl Error for OpenRefused {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::NonPositiveConcurrency(error) => Some(error),
            _ => None,
        }
    }
}

/// The sha256 of the file at `path` in lowercase hex.
fn sha256_of(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 16];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// The path of the pinned file `role` in `dir`, as sherpa-onnx's configuration takes it.
fn config_path(dir: &Path, role: FileRole) -> Result<String, OpenRefused> {
    let file = FILES
        .iter()
        .find(|file| file.role == role)
        .expect("every role is pinned");
    let path = dir.join(file.name);
    path.to_str()
        .map(str::to_owned)
        .ok_or(OpenRefused::PathNotUnicode { path })
}

/// The recognitions of one recogniser that run, shared by its clones, which are the same adapter.
#[derive(Debug, Clone, Default)]
struct Running(Arc<AtomicUsize>);

impl Running {
    /// Counts one more running recognition, if fewer than `max_concurrency` run.
    fn admit(&self, max_concurrency: i64) -> Result<Slot, ConcurrencyLimitReached> {
        let limit = usize::try_from(max_concurrency).unwrap_or(usize::MAX);
        let slot = Slot(Arc::clone(&self.0));
        if self.0.fetch_add(1, Ordering::AcqRel) < limit {
            Ok(slot)
        } else {
            drop(slot);
            Err(ConcurrencyLimitReached { max_concurrency })
        }
    }
}

/// The place of one running recognition among [`Running`], given back when it is dropped.
#[derive(Debug)]
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The batches of one recogniser that decode at once, shared by its clones and their
/// recognitions. A batch holds a [`Place`] from before it starts until it ends, also when the
/// async form decodes it on a thread of its own whose recognition was dropped meanwhile, so a
/// recogniser never decodes more batches at once than its maximum concurrency.
#[derive(Debug, Default)]
struct Gate {
    state: std::sync::Mutex<GateState>,
    freed: std::sync::Condvar,
}

#[derive(Debug, Default)]
struct GateState {
    held: usize,
    waiting: Vec<Waker>,
}

impl Gate {
    fn lock(&self) -> std::sync::MutexGuard<'_, GateState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A place among `limit`, waiting on the calling thread while every place is held.
    fn take(self: &Arc<Self>, limit: usize) -> Place {
        let mut state = self.lock();
        while state.held >= limit {
            state = self
                .freed
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        state.held += 1;
        Place(Arc::clone(self))
    }

    /// A place among `limit`, or `Pending` while every place is held; the place given back
    /// wakes `cx`.
    fn poll_take(self: &Arc<Self>, limit: usize, cx: &Context<'_>) -> Poll<Place> {
        let mut state = self.lock();
        if state.held < limit {
            state.held += 1;
            return Poll::Ready(Place(Arc::clone(self)));
        }
        if !state.waiting.iter().any(|kept| kept.will_wake(cx.waker())) {
            state.waiting.push(cx.waker().clone());
        }
        Poll::Pending
    }
}

/// One decode's place in a [`Gate`], given back when it is dropped.
#[derive(Debug)]
struct Place(Arc<Gate>);

impl Drop for Place {
    fn drop(&mut self) {
        let waiting = {
            let mut state = self.0.lock();
            state.held = state.held.saturating_sub(1);
            std::mem::take(&mut state.waiting)
        };
        self.0.freed.notify_all();
        for waker in waiting {
            waker.wake();
        }
    }
}

/// A streaming speech recogniser over sherpa-onnx's online recogniser and the pinned checkpoint.
/// Its clones share the online recogniser and the count of running recognitions: they are the
/// same adapter.
#[derive(Clone)]
pub struct SherpaOnnxRecogniser {
    model: Model,
    adapter: Adapter,
    recognizer: Arc<OnlineRecognizer>,
    running: Running,
    /// The batches decoding at once, at most the maximum concurrency.
    gate: Arc<Gate>,
}

impl fmt::Debug for SherpaOnnxRecogniser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SherpaOnnxRecogniser")
            .field("model", &self.model)
            .field("adapter", &self.adapter)
            .field("running", &self.running)
            .finish_non_exhaustive()
    }
}

impl SherpaOnnxRecogniser {
    /// Opens the pinned checkpoint in `dir` with a maximum concurrency of
    /// [`DEFAULT_MAX_CONCURRENCY`], as [`SherpaOnnxRecogniser::open_with`].
    ///
    /// # Errors
    ///
    /// The [`OpenRefused`] of [`SherpaOnnxRecogniser::open_with`].
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, OpenRefused> {
        Self::open_with(dir, DEFAULT_MAX_CONCURRENCY)
    }

    /// Opens the pinned checkpoint in `dir`, the directory holding the four [`FILES`], and
    /// declares its model and an adapter running at most `max_concurrency` recognitions at once.
    /// Every file is checked against its pinned sha256 before sherpa-onnx reads any.
    ///
    /// # Errors
    ///
    /// [`OpenRefused::NonPositiveConcurrency`] when `max_concurrency` is below 1, before any file
    /// is read; [`OpenRefused::FileMissing`] naming the first of [`FILES`] that is not a file in
    /// `dir`; [`OpenRefused::ChecksumMismatch`] naming the first whose sha256 differs from its
    /// pin; [`OpenRefused::PathNotUnicode`] when a file's path is not Unicode; and
    /// [`OpenRefused::BackendRefused`] when sherpa-onnx does not create the online recogniser.
    /// No recogniser is returned then.
    pub fn open_with(dir: impl AsRef<Path>, max_concurrency: i64) -> Result<Self, OpenRefused> {
        if max_concurrency < 1 {
            return Err(OpenRefused::NonPositiveConcurrency(
                NonPositiveConcurrency {
                    submitted: max_concurrency,
                },
            ));
        }
        let (model, _) = declare_model();
        let (adapter, _) = declare_adapter(model.model_id(), max_concurrency)
            .map_err(OpenRefused::NonPositiveConcurrency)?;
        Self::open_declared(dir, model, adapter)
    }

    /// Opens the pinned checkpoint in `dir` as the `model` and `adapter` already declared by
    /// [`declare_model`] and [`declare_adapter`], so a caller that declared them before any file
    /// was read keeps their ids. Every file is checked against its pinned sha256 before
    /// sherpa-onnx reads any.
    ///
    /// # Errors
    ///
    /// The [`OpenRefused`] of [`SherpaOnnxRecogniser::open_with`] but
    /// [`OpenRefused::NonPositiveConcurrency`].
    pub fn open_declared(
        dir: impl AsRef<Path>,
        model: Model,
        adapter: Adapter,
    ) -> Result<Self, OpenRefused> {
        let dir = dir.as_ref();
        for file in FILES {
            let path = dir.join(file.name);
            match path.metadata() {
                Ok(metadata) if metadata.is_file() => {}
                Ok(_) => {
                    return Err(OpenRefused::FileMissing {
                        path,
                        kind: io::ErrorKind::IsADirectory,
                    });
                }
                Err(error) => {
                    return Err(OpenRefused::FileMissing {
                        path,
                        kind: error.kind(),
                    });
                }
            }
        }
        for file in FILES {
            let path = dir.join(file.name);
            let actual = match sha256_of(&path) {
                Ok(actual) => actual,
                Err(error) => {
                    return Err(OpenRefused::FileMissing {
                        path,
                        kind: error.kind(),
                    });
                }
            };
            if actual != file.sha256 {
                return Err(OpenRefused::ChecksumMismatch {
                    path,
                    expected: file.sha256,
                    actual,
                });
            }
        }
        let config = OnlineRecognizerConfig {
            model_config: OnlineModelConfig {
                transducer: OnlineTransducerModelConfig {
                    encoder: Some(config_path(dir, FileRole::Encoder)?),
                    decoder: Some(config_path(dir, FileRole::Decoder)?),
                    joiner: Some(config_path(dir, FileRole::Joiner)?),
                },
                tokens: Some(config_path(dir, FileRole::Tokens)?),
                ..OnlineModelConfig::default()
            },
            decoding_method: Some("greedy_search".to_owned()),
            enable_endpoint: true,
            rule1_min_trailing_silence: 2.4,
            rule2_min_trailing_silence: 1.2,
            rule3_min_utterance_length: 20.0,
            ..OnlineRecognizerConfig::default()
        };
        let recognizer =
            OnlineRecognizer::create(&config).ok_or_else(|| OpenRefused::BackendRefused {
                dir: dir.to_path_buf(),
            })?;
        Ok(Self {
            model,
            adapter,
            recognizer: Arc::new(recognizer),
            running: Running::default(),
            gate: Arc::default(),
        })
    }

    /// The model the recogniser declared for the pinned checkpoint.
    #[must_use]
    pub const fn model(&self) -> &Model {
        &self.model
    }
}

impl Recogniser for SherpaOnnxRecogniser {
    type Handle = SherpaOnnxRecognition;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn start(
        &mut self,
        request: RecognitionRequest,
    ) -> Result<SherpaOnnxRecognition, ConcurrencyLimitReached> {
        let max_concurrency = self.adapter.max_concurrency();
        let slot = self.running.admit(max_concurrency)?;
        Ok(SherpaOnnxRecognition {
            gate: Arc::clone(&self.gate),
            limit: usize::try_from(max_concurrency).unwrap_or(usize::MAX),
            batch: None,
            engine: Some(Engine {
                stream: self.recognizer.create_stream(),
                recognizer: Arc::clone(&self.recognizer),
                stream_rate: None,
                resampler: None,
            }),
            decoding: None,
            output: TranscriptOutput::start(&self.adapter, request),
            utterance: Utterance::default(),
            delivered: VecDeque::new(),
            finished: false,
            _slot: slot,
        })
    }
}

/// The span of the utterance being recognised: none until the recogniser shows text, then the
/// Hypothesis span carrying the last text it showed, until an endpoint or the end of input
/// commits it.
#[derive(Debug, Default)]
struct Utterance {
    span: Option<(SpanId, String)>,
    stopped: bool,
}

impl Utterance {
    /// The recogniser shows `text` for the utterance: the first non-empty text emits a
    /// hypothesis, a changed one revises it.
    fn observe(
        &mut self,
        output: &mut TranscriptOutput,
        text: &str,
        delivered: &mut VecDeque<EventEnvelope<TranscriptEvent>>,
    ) {
        let text = text.trim();
        if self.stopped || text.is_empty() {
            return;
        }
        match &mut self.span {
            None => match output.hypothesis(text.to_owned()) {
                Ok(envelope) => {
                    let span_id = envelope
                        .payload
                        .span_id()
                        .expect("a hypothesis is about its span");
                    self.span = Some((span_id, text.to_owned()));
                    delivered.push_back(envelope);
                }
                Err(_) => self.stopped = true,
            },
            Some((_, shown)) if shown == text => {}
            Some((span_id, shown)) => match output.revise(ReviseSpan {
                span_id: *span_id,
                text: text.to_owned(),
            }) {
                Ok(envelope) => {
                    text.clone_into(shown);
                    delivered.push_back(envelope);
                }
                Err(_) => self.stopped = true,
            },
        }
    }

    /// The utterance ended: its span, if it has one, is committed.
    fn close(
        &mut self,
        output: &mut TranscriptOutput,
        delivered: &mut VecDeque<EventEnvelope<TranscriptEvent>>,
    ) {
        if self.stopped {
            return;
        }
        if let Some((span_id, _)) = self.span.take() {
            match output.commit(span_id) {
                Ok(envelope) => delivered.push_back(envelope),
                Err(_) => self.stopped = true,
            }
        }
    }
}

/// One recognition of a [`SherpaOnnxRecogniser`]: one sherpa-onnx online stream. It runs until
/// it is dropped.
///
/// Iterating it decodes on the calling thread. Its async form,
/// [`RecognitionHandle::poll_event`], decodes each batch of frames on a thread it starts for the
/// batch and returns `Pending` until that thread is done, which wakes the caller, so the caller's
/// runtime thread never decodes. Either way a batch waits while the recogniser already decodes
/// its maximum concurrency of them. Dropped while a batch decodes, the recognition does not wait
/// for it; the batch's thread decodes it to its end and holds its place among the batches
/// decoding until then.
pub struct SherpaOnnxRecognition {
    gate: Arc<Gate>,
    limit: usize,
    /// The frames taken and whether input ended with them, while the async form waits for a
    /// place to decode them.
    batch: Option<(Vec<AudioFrame>, bool)>,
    /// The stream and what feeds it; `None` while a batch decodes on a thread of its own, which
    /// holds it.
    engine: Option<Engine>,
    /// The batch the async form is decoding on a thread of its own.
    decoding: Option<Decoding>,
    output: TranscriptOutput,
    utterance: Utterance,
    delivered: VecDeque<EventEnvelope<TranscriptEvent>>,
    finished: bool,
    _slot: Slot,
}

/// The decoding side of a recognition: its online stream and what feeds the stream.
struct Engine {
    /// Declared before `recognizer`, so the stream is destroyed before the last reference to the
    /// online recogniser it belongs to can be.
    stream: OnlineStream,
    recognizer: Arc<OnlineRecognizer>,
    /// The sample rate of the first frame the stream took at a rate [`stream_rate`] admits, the
    /// only rate it is ever given: sherpa-onnx ends the process when one stream is given a
    /// second rate.
    stream_rate: Option<u32>,
    /// The resampler of the current run of frames at another rate than `stream_rate`.
    resampler: Option<Resampler>,
}

/// What decoding showed, in order: the text the recogniser shows for the utterance, or an
/// endpoint, which ends it.
enum Seen {
    Text(String),
    Endpoint,
}

/// What one batch came to: the engine back, what it showed, and the time it took.
type Decoded = (Engine, Vec<Seen>, Duration);

/// A batch decoding on a thread of its own: it sends what it came to and then wakes the waker
/// kept.
struct Decoding {
    result: std::sync::mpsc::Receiver<Decoded>,
    waker: Arc<std::sync::Mutex<Option<Waker>>>,
    ended: bool,
}

impl Decoding {
    /// Feeds `frames`, and the end of input when `ended`, to `engine` on a new thread, which
    /// holds `place` until it ends; the engine and the frames back when no thread starts.
    fn start(
        engine: Engine,
        frames: Vec<AudioFrame>,
        ended: bool,
        place: Place,
    ) -> Result<Self, (Engine, Vec<AudioFrame>)> {
        let (sender, result) = std::sync::mpsc::sync_channel(1);
        let waker: Arc<std::sync::Mutex<Option<Waker>>> = Arc::default();
        let wake = Arc::clone(&waker);
        let (handing, handed) = std::sync::mpsc::sync_channel::<(Engine, Vec<AudioFrame>)>(1);
        let started = std::thread::Builder::new()
            .name("inference-sherpa-onnx".to_owned())
            .spawn(move || {
                let _place = place;
                let Ok((mut engine, frames)) = handed.recv() else {
                    return;
                };
                let (seen, spent) = Engine::feed(&mut engine, &frames, ended);
                let _ = sender.send((engine, seen, spent));
                drop(sender);
                let kept = wake
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                if let Some(waker) = kept {
                    waker.wake();
                }
            });
        if started.is_err() {
            return Err((engine, frames));
        }
        if let Err(std::sync::mpsc::SendError(back)) = handing.send((engine, frames)) {
            return Err(back);
        }
        Ok(Self {
            result,
            waker,
            ended,
        })
    }

    /// What the thread came to, `None` when it ended without a result: blocking without `cx`;
    /// with it, `Pending` until the thread is done, which wakes `cx`.
    fn poll(&self, cx: Option<&mut Context<'_>>) -> Poll<Option<Decoded>> {
        let Some(cx) = cx else {
            return Poll::Ready(self.result.recv().ok());
        };
        let mut kept = self
            .waker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *kept = Some(cx.waker().clone());
        drop(kept);
        match self.result.try_recv() {
            Ok(decoded) => Poll::Ready(Some(decoded)),
            Err(std::sync::mpsc::TryRecvError::Empty) => Poll::Pending,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Poll::Ready(None),
        }
    }
}

/// A sample rate as sherpa-onnx takes it.
fn native_rate(rate: u32) -> i32 {
    i32::try_from(rate).unwrap_or(i32::MAX)
}

/// Linear interpolation of one run of consecutive frames at the rate `from` to the rate `to`,
/// carried across the frames so that the run is resampled as one signal: output sample `i` of
/// the run is read at input position `i * from / to`, between its two neighbours, once both have
/// arrived. No low-pass filter is applied.
#[derive(Debug)]
struct Resampler {
    from: u64,
    to: u64,
    /// The last input sample taken, the left neighbour of the next output sample.
    previous: Option<f32>,
    /// How many input samples the run has taken.
    taken: u64,
    /// How many output samples the run has emitted.
    emitted: u64,
}

impl Resampler {
    fn new(from: u32, to: u32) -> Self {
        Self {
            from: u64::from(from.max(1)),
            to: u64::from(to.max(1)),
            previous: None,
            taken: 0,
            emitted: 0,
        }
    }

    /// The output samples `samples`, the run's next input, completes.
    fn push(&mut self, samples: &[f32]) -> Vec<f32> {
        let end = self.taken + samples.len() as u64;
        let sample = |index: u64| {
            if index < self.taken {
                self.previous.unwrap_or(0.0)
            } else {
                samples[usize::try_from(index - self.taken).expect("within the frame")]
            }
        };
        let mut output = Vec::new();
        loop {
            let numerator = self.emitted * self.from;
            let below = numerator / self.to;
            if below + 1 >= end {
                break;
            }
            #[allow(clippy::cast_precision_loss)]
            let fraction = (numerator % self.to) as f32 / self.to as f32;
            let (left, right) = (sample(below), sample(below + 1));
            output.push(left + (right - left) * fraction);
            self.emitted += 1;
        }
        if let Some(last) = samples.last() {
            self.previous = Some(*last);
        }
        self.taken = end;
        output
    }
}

impl fmt::Debug for SherpaOnnxRecognition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SherpaOnnxRecognition")
            .field("output", &self.output)
            .field("utterance", &self.utterance)
            .field("delivered", &self.delivered)
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

impl SherpaOnnxRecognition {
    /// Takes the frames offered so far into the stream and decodes what is ready, adding the time
    /// that took to the recognition's local compute; at the end of input flushes the stream,
    /// commits the last span and records the recognition's usage. Once that has run, nothing more
    /// is decoded.
    ///
    /// With `cx`, the async form's, a batch of frames, or the end of input, decodes on a thread
    /// of its own and this is `Pending` until it is done; without, a batch the async form started
    /// is waited for.
    fn advance(&mut self, mut cx: Option<&mut Context<'_>>) -> Poll<()> {
        loop {
            if let Some(decoding) = self.decoding.as_ref() {
                let decoded = ready!(decoding.poll(cx.as_deref_mut()));
                let ended = decoding.ended;
                self.decoding = None;
                match decoded {
                    Some((engine, seen, spent)) => {
                        self.engine = Some(engine);
                        self.settle(seen, spent, ended);
                    }
                    None => self.lose_engine(),
                }
                if !self.delivered.is_empty() || self.finished {
                    return Poll::Ready(());
                }
                continue;
            }
            if self.batch.is_none() {
                if self.finished {
                    return Poll::Ready(());
                }
                let Ok(frames) = self.output.take_input() else {
                    return Poll::Ready(());
                };
                let ended = self.output.input_ended();
                if cx.is_some() && frames.is_empty() && !ended {
                    return Poll::Ready(());
                }
                self.batch = Some((frames, ended));
            }
            let place = match cx.as_deref_mut() {
                None => self.gate.take(self.limit),
                Some(cx) => ready!(self.gate.poll_take(self.limit, cx)),
            };
            let Some((frames, ended)) = self.batch.take() else {
                return Poll::Ready(());
            };
            let Some(mut engine) = self.engine.take() else {
                self.lose_engine();
                return Poll::Ready(());
            };
            let inline = if cx.is_some() {
                match Decoding::start(engine, frames, ended, place) {
                    Ok(decoding) => {
                        self.decoding = Some(decoding);
                        continue;
                    }
                    Err((back, frames)) => {
                        engine = back;
                        (frames, None)
                    }
                }
            } else {
                (frames, Some(place))
            };
            let (frames, place) = inline;
            let (seen, spent) = engine.feed(&frames, ended);
            drop(place);
            self.engine = Some(engine);
            self.settle(seen, spent, ended);
            return Poll::Ready(());
        }
    }

    /// Acts on what one batch showed, in order, adds the time it took to the local compute, and,
    /// at the end of input, commits the last span and records the recognition's usage.
    fn settle(&mut self, seen: Vec<Seen>, spent: Duration, ended: bool) {
        for seen in seen {
            match seen {
                Seen::Text(text) => {
                    self.utterance
                        .observe(&mut self.output, &text, &mut self.delivered);
                }
                Seen::Endpoint => self.utterance.close(&mut self.output, &mut self.delivered),
            }
        }
        self.output.add_local_compute(spent);
        if ended {
            self.utterance.close(&mut self.output, &mut self.delivered);
            if let Ok(usage) = self.output.usage() {
                self.delivered.push_back(usage);
            }
            self.finished = true;
        }
    }

    /// The thread decoding a batch ended without giving the stream back: the call is failed and
    /// its usage recorded, and nothing more is decoded.
    fn lose_engine(&mut self) {
        self.output.mark_failed();
        if let Ok(usage) = self.output.usage() {
            self.delivered.push_back(usage);
        }
        self.finished = true;
    }
}

impl Engine {
    /// Feeds `frames` to the stream, and the end of input when `ended`, and decodes while the
    /// stream is ready; returns what decoding showed, in order, and the time it all took.
    fn feed(&mut self, frames: &[AudioFrame], ended: bool) -> (Vec<Seen>, Duration) {
        let computing = Instant::now();
        for frame in frames {
            let samples = mono_samples(frame);
            if samples.is_empty() {
                continue;
            }
            let frame_rate = frame.format.sample_rate.get();
            let Ok(stream_rate) = admit_rate(&mut self.stream_rate, frame_rate) else {
                continue;
            };
            let samples = if frame_rate == stream_rate {
                self.resampler = None;
                samples
            } else {
                let resampler = self
                    .resampler
                    .take()
                    .filter(|resampler| resampler.from == u64::from(frame_rate))
                    .unwrap_or_else(|| Resampler::new(frame_rate, stream_rate));
                self.resampler.insert(resampler).push(&samples)
            };
            if samples.is_empty() {
                continue;
            }
            self.stream
                .accept_waveform(native_rate(stream_rate), &samples);
        }
        if ended {
            if let Some(stream_rate) = self.stream_rate
                && let Ok(padding) = tail_padding(stream_rate)
            {
                self.stream
                    .accept_waveform(native_rate(stream_rate), &vec![0.0; padding]);
            }
            self.stream.input_finished();
        }
        let seen = self.decode();
        (seen, computing.elapsed())
    }

    /// Decodes while the stream is ready, noting each partial result's text, and each endpoint,
    /// at which the stream is reset.
    fn decode(&mut self) -> Vec<Seen> {
        let mut seen = Vec::new();
        loop {
            let ready = self.recognizer.is_ready(&self.stream);
            if ready {
                self.recognizer.decode(&self.stream);
            }
            if let Some(result) = self.recognizer.get_result(&self.stream) {
                seen.push(Seen::Text(result.text));
            }
            if self.recognizer.is_endpoint(&self.stream) {
                seen.push(Seen::Endpoint);
                self.recognizer.reset(&self.stream);
            }
            if !ready {
                return seen;
            }
        }
    }
}

impl Iterator for SherpaOnnxRecognition {
    type Item = EventEnvelope<TranscriptEvent>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.delivered.is_empty() {
            let _ = self.advance(None);
        }
        self.delivered.pop_front()
    }
}

impl RecognitionHandle for SherpaOnnxRecognition {
    fn output(&self) -> &TranscriptOutput {
        &self.output
    }

    fn offer_frame(&mut self, frame: AudioFrame) -> Result<(), InputEnded> {
        self.output.offer(frame)
    }

    fn end_input(&mut self) -> Result<(), InputEnded> {
        self.output.end_input()
    }

    fn revise(&mut self, input: ReviseSpan) -> Result<(), RevisionRefused> {
        let envelope = self.output.revise(input)?;
        self.delivered.push_back(envelope);
        Ok(())
    }

    /// Takes what iterating takes, but decodes on a thread of its own: `Pending` while that
    /// thread decodes, which wakes `cx` when it is done.
    fn poll_event(&mut self, cx: &mut Context<'_>) -> Poll<Option<EventEnvelope<TranscriptEvent>>> {
        if self.delivered.is_empty() {
            ready!(self.advance(Some(cx)));
        }
        Poll::Ready(self.delivered.pop_front())
    }
}

#[cfg(test)]
mod tests;
