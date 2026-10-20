//! A batch speech recogniser over whisper.cpp.
//!
//! [`WhisperCppRecogniser`] implements the `inference-stt` contract over one pinned checkpoint,
//! [`CHECKPOINT`], OpenAI's Whisper base model for English converted to ggml: the single file
//! [`FILE_NAME`] with its sha256 [`SHA256`], which [`WhisperCppRecogniser::open`] checks before
//! whisper.cpp reads it and refuses with [`OpenRefused`], naming the file, when it is missing,
//! differs from the pin or does not load. The checkpoint's licences, source and revision are
//! constants of this module and fill the [`Model`] the recogniser declares ([`declare_model`]);
//! its [`Adapter`] ([`declare_adapter`]) runs as native FFI in the calling process, declares the
//! maximum concurrency it was opened with, 1 by default, and claims no capability.
//!
//! whisper.cpp is a chunked offline recogniser: one `full` call transcribes one complete buffer.
//! It is not native incremental recognition (`docs/design/inference.md:362`) and chunked
//! transcription is not native streaming (`docs/design/inference.md:919`), so the adapter claims
//! neither incremental input nor output streaming (`inference.model.Capability`), and the STT
//! contract then lets it take no frame and emit no span before the end of input. A recognition
//! buffers the frames offered; once input has ended it takes them all, decodes each to mono f32
//! ([`mono_samples`]), resamples them to whisper.cpp's 16 kHz ([`samples_16k`]), runs `full` on
//! the whole buffer in a whisper.cpp state of its own on the recogniser's shared context, and
//! emits the transcribed text as a Hypothesis span that it commits at once, in the same step. An
//! utterance with no text emits nothing.
//!
//! The crate's `stt_conformance` tests hold the recogniser to adapter conformance against the
//! pinned checkpoint, with the claims of the sherpa-onnx recogniser's conformance inverted.
//! Its capability record on the reference machine is `records/stt-i9-10900k-cpu.json`.
//!
//! # Known limitations
//!
//! The resampler is linear interpolation with no low-pass filter: input above 16 kHz aliases,
//! and input below it is band-limited by its own rate. A recognition holds its whole utterance in
//! memory until input has ended, and whisper.cpp decodes at most 30 s per window, segmenting
//! longer buffers by its own rules.

use std::collections::VecDeque;
use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use sha2::{Digest, Sha256};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use inference_stt::inference_audio::{AudioEncoding, AudioFrame};
use inference_stt::inference_core::envelope::EventEnvelope;
use inference_stt::inference_core::model::{
    Adapter, AdapterDeclared, DeclareAdapter, DeclareModel, ExecutionForm, Locality, Modality,
    Model, ModelDeclared, ModelId, NonPositiveConcurrency,
};
use inference_stt::inference_text::ReviseSpan;
use inference_stt::{
    CONTRACT_REVISION, ConcurrencyLimitReached, InputEnded, Recogniser, RecognitionHandle,
    RecognitionRequest, RevisionRefused, TranscriptEvent, TranscriptOutput,
};

/// The STT contract the recogniser implements, re-exported with the vocabulary crates it
/// re-exports, so that a consumer of speech recognition depends on this crate alone.
pub use inference_stt;

/// The name of the pinned checkpoint.
pub const CHECKPOINT: &str = "ggml-base.en";

/// The name of the pinned checkpoint's single file.
pub const FILE_NAME: &str = "ggml-base.en.bin";

/// The model family of the pinned checkpoint.
pub const FAMILY: &str = "whisper";

/// Where the pinned file is published, at [`SOURCE_REVISION`] of the model repository whose
/// card declares the weight licence.
pub const SOURCE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base.en.bin";

/// The revision of the model repository the pinned file and its licence were read at.
pub const SOURCE_REVISION: &str = "5359861c739e955e79d9a303bcbc70fb988958b1";

/// The sha256 of the pinned file in lowercase hex.
pub const SHA256: &str = "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002";

/// The size of the pinned file in bytes.
pub const SIZE_BYTES: u64 = 147_964_211;

/// The licences of the code that runs the checkpoint: whisper.cpp, compiled into this crate, and
/// the `whisper-rs` bindings.
pub const CODE_LICENSE: &str = "MIT (whisper.cpp), Unlicense (whisper-rs 0.16.0)";

/// The licence of the weights, as the model card at [`SOURCE_REVISION`] declares it and as
/// OpenAI publishes the Whisper weights.
pub const WEIGHT_LICENSE: &str = "MIT";

/// Where the weights come from: the original model and its licence, the conversion and the
/// published file.
pub const PROVENANCE: &str = "OpenAI Whisper base.en (MIT, https://github.com/openai/whisper), \
     converted to ggml by the whisper.cpp project; the file ggml-base.en.bin of \
     https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base.en.bin \
     at revision 5359861c739e955e79d9a303bcbc70fb988958b1";

/// The version of the `whisper-rs` bindings.
pub const WHISPER_RS_VERSION: &str = "0.16.0";

/// The version of whisper.cpp that `whisper-rs-sys` ships and this crate compiles.
pub const NATIVE_VERSION: &str = "1.8.3";

/// The backend the adapter runs on.
pub const BACKEND: &str = "whisper.cpp via whisper-rs 0.16.0";

/// The maximum concurrency of [`WhisperCppRecogniser::open`].
pub const DEFAULT_MAX_CONCURRENCY: i64 = 1;

/// The sample rate whisper.cpp takes: 16 kHz mono.
pub const SAMPLE_RATE: u32 = 16_000;

/// The CPU threads one transcription runs on, whisper.cpp's own default on a machine with at
/// least four.
pub const THREADS: i32 = 4;

/// The checksum of the pinned checkpoint, the `checksum` of its [`Model`]:
/// `sha256:<hex> <file name>`.
#[must_use]
pub fn checksum() -> String {
    format!("sha256:{SHA256} {FILE_NAME}")
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
/// [`CONTRACT_REVISION`], claiming no capability, at most `max_concurrency` recognitions at once.
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
        capabilities: Vec::new(),
        max_concurrency,
    })
}

/// The version of the whisper.cpp this crate compiled.
#[must_use]
pub fn native_version() -> &'static str {
    whisper_rs::WHISPER_CPP_VERSION
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

/// `samples`, mono at `from_rate` Hz, at [`SAMPLE_RATE`] by linear interpolation: output sample
/// `i` is read at input position `i * from_rate / 16000`, between its two neighbours, the last
/// input sample standing in past the end. The output holds the whole 16 kHz samples the input's
/// duration covers; at 16 kHz the input passes through unchanged. No low-pass filter is applied.
#[must_use]
pub fn resample(samples: &[f32], from_rate: u32) -> Vec<f32> {
    if from_rate == SAMPLE_RATE || samples.is_empty() {
        return samples.to_vec();
    }
    let count = samples.len() as u64 * u64::from(SAMPLE_RATE) / u64::from(from_rate.max(1));
    let step = f64::from(from_rate) / f64::from(SAMPLE_RATE);
    let last = samples.len() - 1;
    (0..count)
        .map(|index| {
            let position = index as f64 * step;
            // The position is below the input's length, so its floor indexes the input.
            let below = (position.floor() as usize).min(last);
            let above = (below + 1).min(last);
            let fraction = (position - below as f64) as f32;
            samples[below] + (samples[above] - samples[below]) * fraction
        })
        .collect()
}

/// The samples of `frames`, in order, as whisper.cpp takes them: each frame decoded to mono f32
/// by [`mono_samples`], and each run of consecutive frames at one sample rate [`resample`]d to
/// 16 kHz as one signal.
#[must_use]
pub fn samples_16k(frames: &[AudioFrame]) -> Vec<f32> {
    let mut samples = Vec::new();
    let mut run: Vec<f32> = Vec::new();
    let mut run_rate = SAMPLE_RATE;
    for frame in frames {
        let rate = frame.format.sample_rate.get();
        if rate != run_rate {
            samples.extend(resample(&run, run_rate));
            run.clear();
            run_rate = rate;
        }
        run.extend(mono_samples(frame));
    }
    samples.extend(resample(&run, run_rate));
    samples
}

/// Why [`WhisperCppRecogniser::open_with`] refused; no model or adapter is declared either way.
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
    /// whisper.cpp did not load the file at `path`, for the reason `detail` gives.
    BackendRefused { path: PathBuf, detail: String },
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
            Self::BackendRefused { path, detail } => {
                write!(f, "whisper.cpp did not load {}: {detail}", path.display())
            }
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

/// whisper.cpp did not transcribe a recognition's input, for the reason `detail` gives; the
/// recognition emitted nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecognitionFailed {
    pub detail: String,
}

impl fmt::Display for RecognitionFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "whisper.cpp did not transcribe the input: {}",
            self.detail
        )
    }
}

impl Error for RecognitionFailed {}

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

/// A batch speech recogniser over whisper.cpp and the pinned checkpoint. Its clones share the
/// whisper.cpp context and the count of running recognitions: they are the same adapter.
#[derive(Clone)]
pub struct WhisperCppRecogniser {
    model: Model,
    adapter: Adapter,
    context: Arc<WhisperContext>,
    running: Running,
}

impl fmt::Debug for WhisperCppRecogniser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WhisperCppRecogniser")
            .field("model", &self.model)
            .field("adapter", &self.adapter)
            .field("running", &self.running)
            .finish_non_exhaustive()
    }
}

impl WhisperCppRecogniser {
    /// Opens the pinned checkpoint file at `path` with a maximum concurrency of
    /// [`DEFAULT_MAX_CONCURRENCY`], as [`WhisperCppRecogniser::open_with`].
    ///
    /// # Errors
    ///
    /// The [`OpenRefused`] of [`WhisperCppRecogniser::open_with`].
    pub fn open(path: impl AsRef<Path>) -> Result<Self, OpenRefused> {
        Self::open_with(path, DEFAULT_MAX_CONCURRENCY)
    }

    /// Opens the pinned checkpoint file at `path`, [`FILE_NAME`] under whatever name, and
    /// declares its model and an adapter running at most `max_concurrency` recognitions at once.
    /// The file is checked against [`SHA256`] before whisper.cpp reads it, and loaded on the CPU.
    /// whisper.cpp's own logging is routed away from stderr.
    ///
    /// # Errors
    ///
    /// [`OpenRefused::NonPositiveConcurrency`] when `max_concurrency` is below 1, before the file
    /// is read; [`OpenRefused::FileMissing`] when `path` is not a readable file;
    /// [`OpenRefused::ChecksumMismatch`] when its sha256 differs from the pin; and
    /// [`OpenRefused::BackendRefused`] when whisper.cpp does not load it. Nothing is declared
    /// then.
    pub fn open_with(path: impl AsRef<Path>, max_concurrency: i64) -> Result<Self, OpenRefused> {
        let path = path.as_ref();
        if max_concurrency < 1 {
            return Err(OpenRefused::NonPositiveConcurrency(
                NonPositiveConcurrency {
                    submitted: max_concurrency,
                },
            ));
        }
        let missing = |kind| OpenRefused::FileMissing {
            path: path.to_path_buf(),
            kind,
        };
        match path.metadata() {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => return Err(missing(io::ErrorKind::IsADirectory)),
            Err(error) => return Err(missing(error.kind())),
        }
        let actual = sha256_of(path).map_err(|error| missing(error.kind()))?;
        if actual != SHA256 {
            return Err(OpenRefused::ChecksumMismatch {
                path: path.to_path_buf(),
                expected: SHA256,
                actual,
            });
        }
        whisper_rs::install_logging_hooks();
        let mut parameters = WhisperContextParameters::default();
        parameters.use_gpu(false);
        let context = WhisperContext::new_with_params(path, parameters).map_err(|error| {
            OpenRefused::BackendRefused {
                path: path.to_path_buf(),
                detail: error.to_string(),
            }
        })?;
        let (model, _) = declare_model();
        let (adapter, _) = declare_adapter(model.model_id(), max_concurrency)
            .map_err(OpenRefused::NonPositiveConcurrency)?;
        Ok(Self {
            model,
            adapter,
            context: Arc::new(context),
            running: Running::default(),
        })
    }

    /// The model the recogniser declared for the pinned checkpoint.
    #[must_use]
    pub const fn model(&self) -> &Model {
        &self.model
    }
}

impl Recogniser for WhisperCppRecogniser {
    type Handle = WhisperCppRecognition;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn start(
        &mut self,
        request: RecognitionRequest,
    ) -> Result<WhisperCppRecognition, ConcurrencyLimitReached> {
        let slot = self.running.admit(self.adapter.max_concurrency())?;
        Ok(WhisperCppRecognition {
            context: Arc::clone(&self.context),
            output: TranscriptOutput::start(&self.adapter, request),
            delivered: VecDeque::new(),
            finished: false,
            failure: None,
            _slot: slot,
        })
    }
}

/// One recognition of a [`WhisperCppRecogniser`]: the frames offered, buffered until input has
/// ended, then one transcription in a whisper.cpp state of its own. It runs until it is dropped.
pub struct WhisperCppRecognition {
    context: Arc<WhisperContext>,
    output: TranscriptOutput,
    delivered: VecDeque<EventEnvelope<TranscriptEvent>>,
    finished: bool,
    failure: Option<RecognitionFailed>,
    _slot: Slot,
}

impl fmt::Debug for WhisperCppRecognition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WhisperCppRecognition")
            .field("output", &self.output)
            .field("delivered", &self.delivered)
            .field("finished", &self.finished)
            .field("failure", &self.failure)
            .finish_non_exhaustive()
    }
}

impl WhisperCppRecognition {
    /// Why whisper.cpp did not transcribe the input, if it did not; the recognition then emitted
    /// nothing.
    #[must_use]
    pub const fn failure(&self) -> Option<&RecognitionFailed> {
        self.failure.as_ref()
    }

    /// Once input has ended, takes every frame offered, transcribes them and emits the text. Before
    /// that the contract refuses to give up any frame, as the adapter does not claim incremental
    /// input, and nothing happens; after it, nothing more is transcribed.
    fn advance(&mut self) {
        if self.finished {
            return;
        }
        let Ok(frames) = self.output.take_input() else {
            return;
        };
        self.finished = true;
        let samples = samples_16k(&frames);
        if samples.is_empty() {
            return;
        }
        match transcribe(&self.context, &samples) {
            Ok(text) => transcribed(&mut self.output, &text, &mut self.delivered),
            Err(failed) => self.failure = Some(failed),
        }
    }
}

/// The text whisper.cpp transcribes from `samples`, 16 kHz mono, in a new state on `context`:
/// greedy decoding in English on [`THREADS`] threads, its segments joined.
fn transcribe(context: &WhisperContext, samples: &[f32]) -> Result<String, RecognitionFailed> {
    let failed = |what: &str, error: whisper_rs::WhisperError| RecognitionFailed {
        detail: format!("{what}: {error}"),
    };
    let mut state = context
        .create_state()
        .map_err(|error| failed("no state was created", error))?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(THREADS);
    params.set_language(Some("en"));
    params.set_translate(false);
    params.set_no_context(true);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    state
        .full(params, samples)
        .map_err(|error| failed("the transcription failed", error))?;
    let mut text = String::new();
    for segment in state.as_iter() {
        let piece = segment
            .to_str_lossy()
            .map_err(|error| failed("a segment has no text", error))?;
        text.push_str(&piece);
    }
    Ok(text)
}

/// Emits `text`, trimmed, as a Hypothesis span and commits it at once; text that is empty once
/// trimmed emits nothing. A stream that runs out of sequences stops the emission where it ran
/// out.
fn transcribed(
    output: &mut TranscriptOutput,
    text: &str,
    delivered: &mut VecDeque<EventEnvelope<TranscriptEvent>>,
) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    let Ok(hypothesis) = output.hypothesis(text.to_owned()) else {
        return;
    };
    let span_id = hypothesis.payload.span_id();
    delivered.push_back(hypothesis);
    if let Ok(committed) = output.commit(span_id) {
        delivered.push_back(committed);
    }
}

impl Iterator for WhisperCppRecognition {
    type Item = EventEnvelope<TranscriptEvent>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.delivered.is_empty() {
            self.advance();
        }
        self.delivered.pop_front()
    }
}

impl RecognitionHandle for WhisperCppRecognition {
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
