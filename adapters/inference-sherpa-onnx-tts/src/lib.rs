//! A local text-to-speech adapter of the inference framework over sherpa-onnx's offline
//! text-to-speech.
//!
//! [`SherpaOnnxSynthesiser`] implements the `inference-tts` contract as native FFI in the calling
//! process, reached through the official `sherpa-onnx` crate at 1.13.8. It speaks one of two
//! things:
//!
//! * the pinned checkpoint [`CHECKPOINT`], Kokoro-82M v0.19 for English at 24 kHz with the 11
//!   voices [`VOICES`], opened with [`SherpaOnnxSynthesiser::open`] from the unpacked archive
//!   [`ARCHIVE_URL`]. [`FILES`] names the three files it loads, each with its sha256, and
//!   [`ESPEAK_NG_DATA`] the phoneme data directory with the digest of its tree; a directory whose
//!   files are missing or differ from the pin is refused with [`OpenRefused`], naming the file.
//!   Its licences, source and revision are constants of this crate and fill the [`Model`] the
//!   synthesiser declares ([`declare_model`]).
//! * a caller's own voice, a directory holding one VITS model in the Piper format as sherpa-onnx
//!   reads it (one `*.onnx` file, `tokens.txt` and `espeak-ng-data/`), opened with
//!   [`SherpaOnnxSynthesiser::open_voice`]. Nothing of it is pinned: its licence is the
//!   caller's, and the [`Model`] declared for it says so ([`CALLERS_VOICE_LICENSE`]). The voice
//!   speaks at its own sample rate, whatever it is, and the synthesiser reports that rate in
//!   [`SherpaOnnxSynthesiser::format`] and in the `codecs` of its adapter's descriptor; nothing
//!   is resampled here.
//!
//! # The native library and espeak-ng
//!
//! Both kinds of voice turn text into phonemes with espeak-ng, which is GPL-3.0. This repository
//! admits it by a named exception of its owner, only as part of the sherpa-onnx native library
//! built with text-to-speech and of the `espeak-ng-data` directory a voice carries; neither is
//! committed to or distributed by this repository. [`CODE_LICENSES`] lists it as an entry of its
//! own beside sherpa-onnx's Apache-2.0. That is why this adapter is a crate of its own that no
//! contract crate and no other adapter depends on: a build that does not name it links none of
//! it.
//!
//! sherpa-onnx publishes its shared library in two builds with one file name and one set of
//! exported symbols, with and without text-to-speech. This crate links against either; which one
//! a process runs on is decided when the process starts. [`native_tts`] reads it, and opening a
//! synthesiser in a process that loaded the build without text-to-speech is refused with
//! [`OpenRefused::LibraryWithoutTts`] naming the library, before sherpa-onnx is called.
//!
//! # A synthesis
//!
//! The adapter claims [`Capability::IncrementalInput`] and [`Capability::OutputStreaming`]: a
//! segment is generated as soon as the synthesis is iterated after its offer, without waiting for
//! the end of the utterance, and its frames are emitted while it is still Queued, as sherpa-onnx
//! hands its samples over, one part per sentence of the segment's text. See
//! [`SherpaOnnxSynthesis`]. A text with a NUL byte is not given to sherpa-onnx: the synthesis
//! fails with [`SynthesisFailed::NulInText`], and [`speakable`] checks a text beforehand.
//! Discarding the segment being generated, or dropping the synthesis, stops the generation at
//! sherpa-onnx's next part.
//!
//! The synthesis's usage record is the contract's: the characters taken, the audio emitted and
//! one request, with `local_compute_ms` `measured`, the time sherpa-onnx spent generating the
//! segments that were not stopped.

mod engine;
mod library;
mod synthesis;

use std::error::Error;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::num::{NonZeroU16, NonZeroU32};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use sherpa_onnx::{
    OfflineTts, OfflineTtsConfig, OfflineTtsKokoroModelConfig, OfflineTtsModelConfig,
    OfflineTtsVitsModelConfig,
};

use inference_tts::inference_audio::{AudioEncoding, AudioFormat};
use inference_tts::inference_core::model::{
    Adapter, AdapterDeclared, AdapterDescriptor, Capability, DeclareAdapter, DeclareModel,
    ExecutionForm, Locality, Modality, Model, ModelDeclared, ModelId, NonPositiveConcurrency,
};
use inference_tts::{CONTRACT_REVISION, ConcurrencyLimitReached, Synthesiser};

use engine::{Engine, Native};
use synthesis::{Running, Voice};

/// The TTS contract the synthesiser implements, re-exported with the vocabulary crates it
/// re-exports, so that a consumer of local speech synthesis depends on this crate alone.
pub use inference_tts;

pub use library::{NATIVE_LIBRARY, NativeTts, native_tts};
pub use synthesis::{FRAME_MS, NulInText, SherpaOnnxSynthesis, SynthesisFailed, speakable};

/// The name of the pinned checkpoint, and of the directory its archive unpacks to.
pub const CHECKPOINT: &str = "kokoro-en-v0_19";

/// The model family of the pinned checkpoint.
pub const FAMILY: &str = "Kokoro-82M";

/// The archive the pinned checkpoint is unpacked from, of sherpa-onnx's `tts-models` release.
pub const ARCHIVE_URL: &str =
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-en-v0_19.tar.bz2";

/// The sha256 of the archive at [`ARCHIVE_URL`], in lowercase hex.
pub const ARCHIVE_SHA256: &str = "912804855a04745fa77a30be545b3f9a5d15c4d66db00b88cbcd4921df605ac7";

/// Where the weights and voices of the pinned checkpoint are published by their author: the
/// repository whose card declares their licence at [`SOURCE_REVISION`], under `v0.19/`.
pub const SOURCE_URL: &str = "https://huggingface.co/hexgrad/kLegacy";

/// The revision of [`SOURCE_URL`] the licence of the weights and voices was read at.
pub const SOURCE_REVISION: &str = "d0acca3b0748cae52e0a14a28ad03515002ff22f";

/// One component of the code that runs a voice, with its licence and the page that carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CodeLicense {
    pub component: &'static str,
    pub license: &'static str,
    pub source: &'static str,
}

/// The licences of the code that runs a voice, one entry per component of the sherpa-onnx native
/// library built with text-to-speech: sherpa-onnx itself, onnxruntime, and the two phonemiser
/// sources sherpa-onnx 1.13.8 builds in at the revisions its build files pin. espeak-ng is
/// GPL-3.0 and is admitted by the repository owner's named exception only there and as the
/// `espeak-ng-data` a voice carries.
pub const CODE_LICENSES: [CodeLicense; 4] = [
    CodeLicense {
        component: "sherpa-onnx 1.13.8",
        license: "Apache-2.0",
        source: "https://github.com/k2-fsa/sherpa-onnx/blob/v1.13.8/LICENSE",
    },
    CodeLicense {
        component: "onnxruntime",
        license: "MIT",
        source: "https://github.com/microsoft/onnxruntime",
    },
    CodeLicense {
        component: "piper-phonemize",
        license: "MIT",
        source: "https://github.com/csukuangfj/piper-phonemize/blob/f3ff95afc03640bc1399e113e83361192a2fafb4/LICENSE.md",
    },
    CodeLicense {
        component: "espeak-ng",
        license: "GPL-3.0",
        source: "https://github.com/csukuangfj/espeak-ng/blob/ed530aa113046142eb5115cf2fc9157854d0ffe1/COPYING",
    },
];

/// The `code_license` of the [`Model`] a synthesiser declares: `<licence> (<component>)` for
/// each of [`CODE_LICENSES`], joined by `, `.
#[must_use]
pub fn code_license() -> String {
    CODE_LICENSES
        .iter()
        .map(|entry| format!("{} ({})", entry.license, entry.component))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The licence of the weights and voices of the pinned checkpoint, as the card at
/// [`SOURCE_REVISION`] and the archive's own `LICENSE` file ([`LICENSE_FILE`]) declare it.
pub const WEIGHT_LICENSE: &str = "Apache-2.0";

/// Where the pinned checkpoint comes from: its author, the licence of its weights and voices,
/// the archive and what the archive adds.
pub const PROVENANCE: &str = "Kokoro-82M v0.19 by hexgrad, trained on under 100 hours of \
     permissive or non-copyrighted audio as its card says (https://huggingface.co/hexgrad/Kokoro-82M \
     at revision f3ff3571791e39611d31c381e3a41a3af07b4987, licence apache-2.0); the v0.19 weights \
     and their 11 voicepacks are published under Apache-2.0 at https://huggingface.co/hexgrad/kLegacy \
     revision d0acca3b0748cae52e0a14a28ad03515002ff22f, v0.19/; packed by the sherpa-onnx project \
     as kokoro-en-v0_19.tar.bz2 of its tts-models release (sha256 \
     912804855a04745fa77a30be545b3f9a5d15c4d66db00b88cbcd4921df605ac7) with model.onnx, the ONNX \
     export with metadata added, voices.bin, the 11 voices, tokens.txt, a LICENSE file with the \
     text of the Apache License 2.0, and espeak-ng-data, the phoneme data of espeak-ng (GPL-3.0)";

/// The backend the adapter runs on.
pub const BACKEND: &str = "sherpa-onnx 1.13.8";

/// The version the linked native library reports through [`native_version`].
pub const NATIVE_VERSION: &str = "1.13.8";

/// The maximum concurrency of [`SherpaOnnxSynthesiser::open`] and
/// [`SherpaOnnxSynthesiser::open_voice`].
pub const DEFAULT_MAX_CONCURRENCY: i64 = 1;

/// The sample rate of the pinned checkpoint's audio.
pub const SAMPLE_RATE: u32 = 24_000;

/// The voices of the pinned checkpoint, each at its speaker id, as sherpa-onnx's documentation of
/// the archive maps them.
pub const VOICES: [&str; 11] = [
    "af",
    "af_bella",
    "af_nicole",
    "af_sarah",
    "af_sky",
    "am_adam",
    "am_michael",
    "bf_emma",
    "bf_isabella",
    "bm_george",
    "bm_lewis",
];

/// The onnxruntime threads one generation computes on.
pub const THREADS: i32 = 4;

/// The `weight_license` of the [`Model`] declared for a caller's own voice.
pub const CALLERS_VOICE_LICENSE: &str =
    "the caller's: this adapter pins and records no licence of a caller's own voice";

/// The model family declared for a caller's own voice.
pub const CALLERS_VOICE_FAMILY: &str = "VITS (Piper format)";

/// The file of tokens of a voice, pinned or the caller's.
pub const TOKENS: &str = "tokens.txt";

/// What a pinned file is to the synthesiser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileRole {
    /// The model.
    Model,
    /// The style vectors of the voices.
    Voices,
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

/// The files the synthesiser loads from the checkpoint directory, in the order they are checked.
pub const FILES: [CheckpointFile; 3] = [
    CheckpointFile {
        role: FileRole::Model,
        name: "model.onnx",
        sha256: "10ff414106a038ce7e9e0126c6461e4dc8a86efaa89dc91d2009d69fe635e339",
    },
    CheckpointFile {
        role: FileRole::Voices,
        name: "voices.bin",
        sha256: "a372c67b056ef0b695c375d39b99630d23fb07ad4c8d87aa32a19a62fca523ad",
    },
    CheckpointFile {
        role: FileRole::Tokens,
        name: TOKENS,
        sha256: "4f31c71282d14af4e926cd12462078fe9d20d00c589e63fe2750a8f56d6d7f7b",
    },
];

/// A directory of the pinned checkpoint: its name in the checkpoint directory, how many files
/// its tree holds, and the digest of the tree as [`tree_sha256`] computes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CheckpointTree {
    pub name: &'static str,
    pub files: usize,
    pub sha256: &'static str,
}

/// The phoneme data of espeak-ng the pinned checkpoint carries and sherpa-onnx reads: GPL-3.0,
/// part of the archive, never of this repository.
pub const ESPEAK_NG_DATA: CheckpointTree = CheckpointTree {
    name: "espeak-ng-data",
    files: 355,
    sha256: "71a159a95ee8e26c4110582ec69093f0e39eb453c3a33882f6e971510ff5c47a",
};

/// The licence file the archive of the pinned checkpoint carries, the text of the Apache License
/// 2.0: its name in the checkpoint directory and its sha256. The synthesiser does not load it.
pub const LICENSE_FILE: (&str, &str) = (
    "LICENSE",
    "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30",
);

/// The checksum of the pinned checkpoint, the `checksum` of its [`Model`]: `sha256:<hex> <name>`
/// for each of [`FILES`] and for the tree [`ESPEAK_NG_DATA`], joined by `, `.
#[must_use]
pub fn checksum() -> String {
    FILES
        .iter()
        .map(|file| format!("sha256:{} {}", file.sha256, file.name))
        .chain([format!(
            "sha256:{} {}/",
            ESPEAK_NG_DATA.sha256, ESPEAK_NG_DATA.name
        )])
        .collect::<Vec<_>>()
        .join(", ")
}

/// Runs `inference.model.DeclareModel` for the pinned checkpoint: text in, audio out, with its
/// licences, checksum, provenance and source revision.
#[must_use]
pub fn declare_model() -> (Model, ModelDeclared) {
    Model::declare(DeclareModel {
        family: FAMILY.to_owned(),
        checkpoint: CHECKPOINT.to_owned(),
        code_license: code_license(),
        weight_license: WEIGHT_LICENSE.to_owned(),
        checksum: checksum(),
        input_modalities: vec![Modality::Text],
        output_modalities: vec![Modality::Audio],
        provenance: PROVENANCE.to_owned(),
        source_revision: SOURCE_REVISION.to_owned(),
    })
}

/// Runs `inference.model.DeclareAdapter` for the synthesiser of the pinned checkpoint, the model
/// `model_id`: native FFI in the same process on [`BACKEND`] with the checkpoint, implementing
/// this `inference-tts`'s [`CONTRACT_REVISION`], claiming incremental input and output streaming,
/// at most `max_concurrency` syntheses at once, its descriptor naming [`VOICES`] and the one
/// codec, [`codec`] of [`SAMPLE_RATE`].
///
/// # Errors
///
/// [`NonPositiveConcurrency`] when `max_concurrency` is below 1; nothing is declared.
pub fn declare_adapter(
    model_id: ModelId,
    max_concurrency: i64,
) -> Result<(Adapter, AdapterDeclared), NonPositiveConcurrency> {
    declare(
        model_id,
        format!("{BACKEND} ({CHECKPOINT})"),
        max_concurrency,
        VOICES.iter().map(|&voice| voice.to_owned()).collect(),
        format_at(NonZeroU32::new(SAMPLE_RATE).unwrap_or(NonZeroU32::MIN)),
    )
}

fn declare(
    model_id: ModelId,
    backend: String,
    max_concurrency: i64,
    voices: Vec<String>,
    format: AudioFormat,
) -> Result<(Adapter, AdapterDeclared), NonPositiveConcurrency> {
    Adapter::declare(DeclareAdapter {
        model_id,
        backend,
        contract_revision: CONTRACT_REVISION.to_owned(),
        execution_form: ExecutionForm::NativeFfi,
        locality: Locality::SameProcess,
        capabilities: vec![Capability::IncrementalInput, Capability::OutputStreaming],
        max_concurrency,
        descriptor: AdapterDescriptor {
            voices,
            codecs: vec![codec(format)],
            ..AdapterDescriptor::default()
        },
    })
}

/// The format of the frames of a voice at `sample_rate`: mono `pcm_s16le`.
const fn format_at(sample_rate: NonZeroU32) -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate,
        channels: NonZeroU16::MIN,
    }
}

/// `format` as an adapter descriptor names a codec: `<encoding>/<sample rate in Hz>/<channels>`,
/// such as `pcm_s16le/24000/1`.
#[must_use]
pub fn codec(format: AudioFormat) -> String {
    let encoding = match format.encoding {
        AudioEncoding::PcmS16le => "pcm_s16le",
        AudioEncoding::PcmF32le => "pcm_f32le",
        AudioEncoding::PcmMulaw => "pcm_mulaw",
        AudioEncoding::PcmAlaw => "pcm_alaw",
    };
    format!("{encoding}/{}/{}", format.sample_rate, format.channels)
}

/// The version the linked sherpa-onnx native library reports.
#[must_use]
pub fn native_version() -> &'static str {
    sherpa_onnx::version()
}

/// Why a synthesiser was not opened; no model or adapter is declared either way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenRefused {
    /// The maximum concurrency is below 1.
    NonPositiveConcurrency(NonPositiveConcurrency),
    /// The file or directory at `path`, which the voice needs, is not there, is not readable or
    /// is not of that kind.
    FileMissing { path: PathBuf, kind: io::ErrorKind },
    /// The file or tree at `path` is not the pinned one: its sha256 is `actual`, the pin
    /// `expected`.
    ChecksumMismatch {
        path: PathBuf,
        expected: &'static str,
        actual: String,
    },
    /// The directory `dir` of a caller's own voice holds `found` `*.onnx` files where a voice
    /// has exactly one.
    VoiceModelNotSingle { dir: PathBuf, found: usize },
    /// The path of a file is not Unicode, which sherpa-onnx's configuration requires.
    PathNotUnicode { path: PathBuf },
    /// The sherpa-onnx native library the process loaded, at `library`, was built without
    /// text-to-speech. sherpa-onnx was not called.
    LibraryWithoutTts { library: PathBuf },
    /// sherpa-onnx refused to create the synthesiser from the files of `dir`, or created one
    /// that reports no sample rate or speaker.
    BackendRefused { dir: PathBuf },
}

impl fmt::Display for OpenRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonPositiveConcurrency(error) => fmt::Display::fmt(error, f),
            Self::FileMissing { path, kind } => {
                write!(f, "the voice's {} is missing: {kind}", path.display())
            }
            Self::ChecksumMismatch {
                path,
                expected,
                actual,
            } => write!(
                f,
                "{} is not the pinned one: its sha256 is {actual}, the pin {expected}",
                path.display()
            ),
            Self::VoiceModelNotSingle { dir, found } => write!(
                f,
                "{} holds {found} *.onnx files where a voice has exactly one",
                dir.display()
            ),
            Self::PathNotUnicode { path } => write!(
                f,
                "the path {} is not Unicode, so sherpa-onnx cannot be given it",
                path.display()
            ),
            Self::LibraryWithoutTts { library } => write!(
                f,
                "the sherpa-onnx native library this process loaded, {}, was built without \
                 text-to-speech: start the process with the build that has it first on the \
                 loader's path",
                library.display()
            ),
            Self::BackendRefused { dir } => write!(
                f,
                "sherpa-onnx refused to create the synthesiser from {}",
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

/// A speaker id outside the voices of a synthesiser, `0..speakers`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeakerOutOfRange {
    pub speaker: i32,
    pub speakers: i32,
}

impl fmt::Display for SpeakerOutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the voice has the speakers 0 to {}, not {}",
            self.speakers - 1,
            self.speaker
        )
    }
}

impl Error for SpeakerOutOfRange {}

fn hex(digest: impl AsRef<[u8]>) -> String {
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
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
    Ok(hex(hasher.finalize()))
}

/// The files of the tree under `dir` and the digest of the tree, in lowercase hex: the sha256 of
/// one line per file, `<sha256 of the file>  <its path below dir>` and a line feed, the lines in
/// the byte order of the paths, whose parts are joined by `/`. It is what
/// `find * -type f -print0 | LC_ALL=C sort -z | xargs -0 sha256sum | sha256sum` prints in `dir`.
///
/// # Errors
///
/// The error of reading a directory or a file of the tree.
pub fn tree_sha256(dir: &Path) -> io::Result<(usize, String)> {
    let mut files = Vec::new();
    let mut directories = vec![(dir.to_path_buf(), String::new())];
    while let Some((directory, prefix)) = directories.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
            if entry.path().metadata()?.is_dir() {
                directories.push((entry.path(), format!("{name}/")));
            } else {
                files.push((name, entry.path()));
            }
        }
    }
    files.sort();
    let mut hasher = Sha256::new();
    for (name, path) in &files {
        hasher.update(format!("{}  {name}\n", sha256_of(path)?));
    }
    Ok((files.len(), hex(hasher.finalize())))
}

/// Refuses `path` unless it is a file, or a directory when `directory`.
fn present(path: PathBuf, directory: bool) -> Result<PathBuf, OpenRefused> {
    let kind = match path.metadata() {
        Ok(metadata) if metadata.is_dir() == directory => return Ok(path),
        Ok(_) if directory => io::ErrorKind::NotADirectory,
        Ok(_) => io::ErrorKind::IsADirectory,
        Err(error) => error.kind(),
    };
    Err(OpenRefused::FileMissing { path, kind })
}

/// `path` as sherpa-onnx's configuration takes it.
fn unicode(path: &Path) -> Result<String, OpenRefused> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| OpenRefused::PathNotUnicode {
            path: path.to_path_buf(),
        })
}

/// What sherpa-onnx created from the files of a voice: the engine, the rate it speaks at and
/// how many speakers it has.
struct Created {
    engine: Arc<dyn Engine>,
    sample_rate: NonZeroU32,
    speakers: i32,
}

/// Creates sherpa-onnx's offline text-to-speech from `config`, the files of `dir`, unless the
/// native library the process loaded has none.
fn create(dir: &Path, config: &OfflineTtsConfig) -> Result<Created, OpenRefused> {
    if let NativeTts::Absent { library } = native_tts() {
        return Err(OpenRefused::LibraryWithoutTts { library });
    }
    let refused = || OpenRefused::BackendRefused {
        dir: dir.to_path_buf(),
    };
    let tts = OfflineTts::create(config).ok_or_else(refused)?;
    let sample_rate = u32::try_from(tts.sample_rate())
        .ok()
        .and_then(NonZeroU32::new)
        .ok_or_else(refused)?;
    let speakers = tts.num_speakers().max(1);
    Ok(Created {
        engine: Arc::new(Native(tts)),
        sample_rate,
        speakers,
    })
}

fn config(model: OfflineTtsModelConfig) -> OfflineTtsConfig {
    OfflineTtsConfig {
        model: OfflineTtsModelConfig {
            num_threads: THREADS,
            ..model
        },
        max_num_sentences: 1,
        silence_scale: 0.2,
        ..OfflineTtsConfig::default()
    }
}

/// A speech synthesiser over sherpa-onnx's offline text-to-speech and one voice, the pinned
/// checkpoint or a caller's own. Its clones share the engine and the count of running syntheses:
/// they are the same adapter.
#[derive(Clone)]
pub struct SherpaOnnxSynthesiser {
    model: Model,
    adapter: Adapter,
    voice: Voice,
    speakers: i32,
    running: Running,
}

impl fmt::Debug for SherpaOnnxSynthesiser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SherpaOnnxSynthesiser")
            .field("model", &self.model)
            .field("adapter", &self.adapter)
            .field("format", &self.voice.format)
            .field("speaker", &self.voice.speaker)
            .field("speakers", &self.speakers)
            .field("running", &self.running)
            .finish_non_exhaustive()
    }
}

impl SherpaOnnxSynthesiser {
    /// Opens the pinned checkpoint in `dir` with a maximum concurrency of
    /// [`DEFAULT_MAX_CONCURRENCY`], as [`SherpaOnnxSynthesiser::open_with`].
    ///
    /// # Errors
    ///
    /// The [`OpenRefused`] of [`SherpaOnnxSynthesiser::open_with`].
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, OpenRefused> {
        Self::open_with(dir, DEFAULT_MAX_CONCURRENCY)
    }

    /// Opens the pinned checkpoint in `dir`, the directory holding the three [`FILES`] and
    /// [`ESPEAK_NG_DATA`], and declares its model and an adapter running at most
    /// `max_concurrency` syntheses at once. Every file is checked against its pin before
    /// sherpa-onnx reads any. The synthesiser speaks in the voice at speaker id 0.
    ///
    /// # Errors
    ///
    /// [`OpenRefused::NonPositiveConcurrency`] when `max_concurrency` is below 1, before any file
    /// is read; [`OpenRefused::FileMissing`] naming the first of [`FILES`] that is not a file in
    /// `dir`, or [`ESPEAK_NG_DATA`] when it is not a directory there;
    /// [`OpenRefused::ChecksumMismatch`] naming the first whose sha256 differs from its pin;
    /// [`OpenRefused::PathNotUnicode`] when a path is not Unicode;
    /// [`OpenRefused::LibraryWithoutTts`] when the native library the process loaded was built
    /// without text-to-speech; and [`OpenRefused::BackendRefused`] when sherpa-onnx does not
    /// create the synthesiser. No synthesiser is returned then.
    pub fn open_with(dir: impl AsRef<Path>, max_concurrency: i64) -> Result<Self, OpenRefused> {
        positive(max_concurrency)?;
        let (model, _) = declare_model();
        let (adapter, _) = declare_adapter(model.model_id(), max_concurrency)
            .map_err(OpenRefused::NonPositiveConcurrency)?;
        Self::open_declared(dir, model, adapter)
    }

    /// Opens the pinned checkpoint in `dir` as the `model` and `adapter` already declared by
    /// [`declare_model`] and [`declare_adapter`], so a caller that declared them before any file
    /// was read keeps their ids.
    ///
    /// # Errors
    ///
    /// The [`OpenRefused`] of [`SherpaOnnxSynthesiser::open_with`] but
    /// [`OpenRefused::NonPositiveConcurrency`].
    pub fn open_declared(
        dir: impl AsRef<Path>,
        model: Model,
        adapter: Adapter,
    ) -> Result<Self, OpenRefused> {
        let dir = dir.as_ref();
        for file in FILES {
            present(dir.join(file.name), false)?;
        }
        let data = present(dir.join(ESPEAK_NG_DATA.name), true)?;
        for file in FILES {
            let path = dir.join(file.name);
            let actual = sha256_of(&path).map_err(|error| OpenRefused::FileMissing {
                path: path.clone(),
                kind: error.kind(),
            })?;
            if actual != file.sha256 {
                return Err(OpenRefused::ChecksumMismatch {
                    path,
                    expected: file.sha256,
                    actual,
                });
            }
        }
        let (_, actual) = tree_sha256(&data).map_err(|error| OpenRefused::FileMissing {
            path: data.clone(),
            kind: error.kind(),
        })?;
        if actual != ESPEAK_NG_DATA.sha256 {
            return Err(OpenRefused::ChecksumMismatch {
                path: data,
                expected: ESPEAK_NG_DATA.sha256,
                actual,
            });
        }
        let path = |role: FileRole| {
            let file = FILES.iter().find(|file| file.role == role);
            unicode(&dir.join(file.map_or("", |file| file.name))).map(Some)
        };
        let created = create(
            dir,
            &config(OfflineTtsModelConfig {
                kokoro: OfflineTtsKokoroModelConfig {
                    model: path(FileRole::Model)?,
                    voices: path(FileRole::Voices)?,
                    tokens: path(FileRole::Tokens)?,
                    data_dir: Some(unicode(&data)?),
                    ..OfflineTtsKokoroModelConfig::default()
                },
                ..OfflineTtsModelConfig::default()
            }),
        )?;
        if created.sample_rate.get() != SAMPLE_RATE {
            return Err(OpenRefused::BackendRefused {
                dir: dir.to_path_buf(),
            });
        }
        Ok(Self::over(model, adapter, created))
    }

    /// Opens a caller's own voice in `dir` with a maximum concurrency of
    /// [`DEFAULT_MAX_CONCURRENCY`], as [`SherpaOnnxSynthesiser::open_voice_with`].
    ///
    /// # Errors
    ///
    /// The [`OpenRefused`] of [`SherpaOnnxSynthesiser::open_voice_with`].
    pub fn open_voice(dir: impl AsRef<Path>) -> Result<Self, OpenRefused> {
        Self::open_voice_with(dir, DEFAULT_MAX_CONCURRENCY)
    }

    /// Opens a caller's own voice in `dir`: a directory holding one VITS model in the Piper
    /// format as sherpa-onnx reads it, one `*.onnx` file, [`TOKENS`] and an `espeak-ng-data`
    /// directory. It declares a model for it, under [`CALLERS_VOICE_FAMILY`] with the sha256 of
    /// the model and the tokens as read and [`CALLERS_VOICE_LICENSE`], and an adapter running at
    /// most `max_concurrency` syntheses at once whose descriptor names the voice's speaker ids
    /// and its codec at the voice's own sample rate. Nothing is checked against a pin. The
    /// synthesiser speaks in the voice at speaker id 0.
    ///
    /// # Errors
    ///
    /// [`OpenRefused::NonPositiveConcurrency`] when `max_concurrency` is below 1, before any file
    /// is read; [`OpenRefused::FileMissing`] naming `dir` when it cannot be listed, or
    /// [`TOKENS`] or `espeak-ng-data` when it is not there; [`OpenRefused::VoiceModelNotSingle`]
    /// when `dir` holds no `*.onnx` file or more than one; [`OpenRefused::PathNotUnicode`];
    /// [`OpenRefused::LibraryWithoutTts`] when the native library the process loaded was built
    /// without text-to-speech; and [`OpenRefused::BackendRefused`] when sherpa-onnx does not
    /// create the synthesiser. No synthesiser is returned then.
    pub fn open_voice_with(
        dir: impl AsRef<Path>,
        max_concurrency: i64,
    ) -> Result<Self, OpenRefused> {
        positive(max_concurrency)?;
        let dir = dir.as_ref();
        let missing = |error: io::Error| OpenRefused::FileMissing {
            path: dir.to_path_buf(),
            kind: error.kind(),
        };
        let mut models = Vec::new();
        for entry in fs::read_dir(dir).map_err(missing)? {
            let path = entry.map_err(missing)?.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "onnx")
                && path.is_file()
            {
                models.push(path);
            }
        }
        let [model_path] = models.as_slice() else {
            return Err(OpenRefused::VoiceModelNotSingle {
                dir: dir.to_path_buf(),
                found: models.len(),
            });
        };
        let tokens = present(dir.join(TOKENS), false)?;
        let data = present(dir.join("espeak-ng-data"), true)?;
        let created = create(
            dir,
            &config(OfflineTtsModelConfig {
                vits: OfflineTtsVitsModelConfig {
                    model: Some(unicode(model_path)?),
                    tokens: Some(unicode(&tokens)?),
                    data_dir: Some(unicode(&data)?),
                    ..OfflineTtsVitsModelConfig::default()
                },
                ..OfflineTtsModelConfig::default()
            }),
        )?;
        let name = |path: &Path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let sum = |path: &Path| {
            sha256_of(path)
                .map(|sha256| format!("sha256:{sha256} {}", name(path)))
                .map_err(|error| OpenRefused::FileMissing {
                    path: path.to_path_buf(),
                    kind: error.kind(),
                })
        };
        let (model, _) = Model::declare(DeclareModel {
            family: CALLERS_VOICE_FAMILY.to_owned(),
            checkpoint: name(model_path),
            code_license: code_license(),
            weight_license: CALLERS_VOICE_LICENSE.to_owned(),
            checksum: format!("{}, {}", sum(model_path)?, sum(&tokens)?),
            input_modalities: vec![Modality::Text],
            output_modalities: vec![Modality::Audio],
            provenance: "a caller's own voice, opened from a directory given as configuration; \
                         not pinned by this adapter"
                .to_owned(),
            source_revision: "not recorded".to_owned(),
        });
        let (adapter, _) = declare(
            model.model_id(),
            format!("{BACKEND} ({})", name(model_path)),
            max_concurrency,
            (0..created.speakers).map(|id| id.to_string()).collect(),
            format_at(created.sample_rate),
        )
        .map_err(OpenRefused::NonPositiveConcurrency)?;
        Ok(Self::over(model, adapter, created))
    }

    fn over(model: Model, adapter: Adapter, created: Created) -> Self {
        Self {
            model,
            adapter,
            voice: Voice {
                engine: created.engine,
                format: format_at(created.sample_rate),
                speaker: 0,
                gate: Arc::default(),
                workers: Arc::default(),
            },
            speakers: created.speakers,
            running: Running::default(),
        }
    }

    /// This synthesiser speaking in the voice at speaker id `speaker`, for the pinned checkpoint
    /// its index in [`VOICES`]. Syntheses already started keep the voice they started with.
    ///
    /// # Errors
    ///
    /// [`SpeakerOutOfRange`] when `speaker` is not one of [`SherpaOnnxSynthesiser::speakers`].
    pub fn with_speaker(mut self, speaker: i32) -> Result<Self, SpeakerOutOfRange> {
        if !(0..self.speakers).contains(&speaker) {
            return Err(SpeakerOutOfRange {
                speaker,
                speakers: self.speakers,
            });
        }
        self.voice.speaker = speaker;
        Ok(self)
    }

    /// The model the synthesiser declared for its voice.
    #[must_use]
    pub const fn model(&self) -> &Model {
        &self.model
    }

    /// The format of every frame it emits: mono `pcm_s16le` at the voice's own sample rate.
    #[must_use]
    pub const fn format(&self) -> AudioFormat {
        self.voice.format
    }

    /// The speaker id it speaks in.
    #[must_use]
    pub const fn speaker(&self) -> i32 {
        self.voice.speaker
    }

    /// How many speakers the voice has; their ids are `0..speakers`.
    #[must_use]
    pub const fn speakers(&self) -> i32 {
        self.speakers
    }
}

fn positive(max_concurrency: i64) -> Result<(), OpenRefused> {
    if max_concurrency < 1 {
        return Err(OpenRefused::NonPositiveConcurrency(
            NonPositiveConcurrency {
                submitted: max_concurrency,
            },
        ));
    }
    Ok(())
}

impl Synthesiser for SherpaOnnxSynthesiser {
    type Handle = SherpaOnnxSynthesis;

    fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    fn start(&mut self) -> Result<SherpaOnnxSynthesis, ConcurrencyLimitReached> {
        let slot = self.running.admit(self.adapter.max_concurrency())?;
        Ok(SherpaOnnxSynthesis::start(
            &self.adapter,
            self.voice.clone(),
            slot,
        ))
    }
}

#[cfg(test)]
mod tests;
