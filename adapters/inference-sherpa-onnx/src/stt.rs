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
//! low-pass filter. The first non-empty partial result emits a Hypothesis span, a changed result
//! revises it, and an endpoint, by sherpa-onnx's endpoint rules, commits it and resets the
//! stream for the next utterance; an utterance with no text emits nothing. At the end of input
//! the stream is flushed and the last span committed.
//!
//! The recognition's usage record, the contract's [`TranscriptOutput::usage`], is its last event,
//! delivered right after the last commit: the audio offered and one request, as the contract
//! measures them, and `local_compute_ms` `measured`, the wall time each step spent passing frames
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
use std::time::Instant;

use sha2::{Digest, Sha256};
use sherpa_onnx::{
    OnlineModelConfig, OnlineRecognizer, OnlineRecognizerConfig, OnlineStream,
    OnlineTransducerModelConfig,
};

use inference_stt::inference_audio::{AudioEncoding, AudioFrame};
use inference_stt::inference_core::envelope::EventEnvelope;
use inference_stt::inference_core::model::{
    Adapter, AdapterDeclared, Capability, DeclareAdapter, DeclareModel, ExecutionForm, Locality,
    Modality, Model, ModelDeclared, ModelId, NonPositiveConcurrency,
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

/// A streaming speech recogniser over sherpa-onnx's online recogniser and the pinned checkpoint.
/// Its clones share the online recogniser and the count of running recognitions: they are the
/// same adapter.
#[derive(Clone)]
pub struct SherpaOnnxRecogniser {
    model: Model,
    adapter: Adapter,
    recognizer: Arc<OnlineRecognizer>,
    running: Running,
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
    /// Nothing is declared then.
    pub fn open_with(dir: impl AsRef<Path>, max_concurrency: i64) -> Result<Self, OpenRefused> {
        let dir = dir.as_ref();
        if max_concurrency < 1 {
            return Err(OpenRefused::NonPositiveConcurrency(
                NonPositiveConcurrency {
                    submitted: max_concurrency,
                },
            ));
        }
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
        let (model, _) = declare_model();
        let (adapter, _) = declare_adapter(model.model_id(), max_concurrency)
            .map_err(OpenRefused::NonPositiveConcurrency)?;
        Ok(Self {
            model,
            adapter,
            recognizer: Arc::new(recognizer),
            running: Running::default(),
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
        let slot = self.running.admit(self.adapter.max_concurrency())?;
        Ok(SherpaOnnxRecognition {
            stream: self.recognizer.create_stream(),
            recognizer: Arc::clone(&self.recognizer),
            output: TranscriptOutput::start(&self.adapter, request),
            utterance: Utterance::default(),
            delivered: VecDeque::new(),
            stream_rate: None,
            resampler: None,
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
pub struct SherpaOnnxRecognition {
    /// Declared before `recognizer`, so the stream is destroyed before the last reference to the
    /// online recogniser it belongs to can be.
    stream: OnlineStream,
    recognizer: Arc<OnlineRecognizer>,
    output: TranscriptOutput,
    utterance: Utterance,
    delivered: VecDeque<EventEnvelope<TranscriptEvent>>,
    /// The sample rate of the first frame the stream took, the only rate it is ever given:
    /// sherpa-onnx ends the process when one stream is given a second rate.
    stream_rate: Option<u32>,
    /// The resampler of the current run of frames at another rate than `stream_rate`.
    resampler: Option<Resampler>,
    finished: bool,
    _slot: Slot,
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
    fn advance(&mut self) {
        if self.finished {
            return;
        }
        let Ok(frames) = self.output.take_input() else {
            return;
        };
        let computing = Instant::now();
        for frame in &frames {
            let samples = mono_samples(frame);
            if samples.is_empty() {
                continue;
            }
            let frame_rate = frame.format.sample_rate.get();
            let stream_rate = *self.stream_rate.get_or_insert(frame_rate);
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
        let ended = self.output.input_ended();
        if ended {
            if let Some(stream_rate) = self.stream_rate {
                let padding =
                    usize::try_from(stream_rate).unwrap_or(0) * TAIL_PADDING_HUNDREDTHS / 100;
                self.stream
                    .accept_waveform(native_rate(stream_rate), &vec![0.0; padding]);
            }
            self.stream.input_finished();
        }
        self.decode();
        self.output.add_local_compute(computing.elapsed());
        if ended {
            self.utterance.close(&mut self.output, &mut self.delivered);
            if let Ok(usage) = self.output.usage() {
                self.delivered.push_back(usage);
            }
            self.finished = true;
        }
    }

    /// Decodes while the stream is ready, emitting from each partial result, committing and
    /// resetting at each endpoint.
    fn decode(&mut self) {
        loop {
            let ready = self.recognizer.is_ready(&self.stream);
            if ready {
                self.recognizer.decode(&self.stream);
            }
            if let Some(result) = self.recognizer.get_result(&self.stream) {
                self.utterance
                    .observe(&mut self.output, &result.text, &mut self.delivered);
            }
            if self.recognizer.is_endpoint(&self.stream) {
                self.utterance.close(&mut self.output, &mut self.delivered);
                self.recognizer.reset(&self.stream);
            }
            if !ready {
                return;
            }
        }
    }
}

impl Iterator for SherpaOnnxRecognition {
    type Item = EventEnvelope<TranscriptEvent>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.delivered.is_empty() {
            self.advance();
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
}

#[cfg(test)]
mod tests;
