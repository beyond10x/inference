//! The cascade over the local adapters (feature `local`; A-05 of `docs/design/inference.md` at
//! 1140): the same declaration as [`declare`](crate::declare), its model nodes bound to
//!
//! | node | adapter | locality |
//! |---|---|---|
//! | `stt` | the recogniser [`STT_VARIABLE`] selects ([`SttBackend`]): `inference-sherpa-onnx`'s streaming recogniser over its pinned checkpoint, the default, or `inference-whispercpp`'s batch recogniser over its pinned checkpoint | `same_process` |
//! | `agent` | `inference-llama-server`'s generator over a `llama-server` on loopback | `local_worker` |
//! | `tts` | the testkit's mock synthesiser, until a local TTS adapter exists | `same_process` |
//!
//! under the `offline_strict` profile with egress denied admitting [`ADMITTED`], both localities.
//! The connection to the server is compute on the machine, not egress: [`LocalModels::open`]
//! refuses a server URL whose host is not loopback ([`loopback`]) before it opens anything, and
//! the egress watch of `inference-runtime` admits loopback.
//!
//! The recogniser is configuration alone: both adapter crates are dependencies of the feature,
//! and the value of [`STT_VARIABLE`], `sherpa-onnx` or `whispercpp`, decides which of them opens
//! its checkpoint, whose adapter record the catalog holds and which adapter the `stt` node is
//! bound to; the declaration, the node implementations and the tests are the same under both.
//! The `stt` node requires nothing ([`STT_REQUIRES`](crate::STT_REQUIRES)), so the batch
//! recogniser, which claims neither incremental input nor output streaming, plans there; its
//! transcript arrives once, at the turn's completion, as a span committed at once, where the
//! streaming recogniser's hypotheses precede its commits. The effective capabilities of the
//! cascade are the intersection of what its three adapters claim, the cascade having no source
//! node: with sherpa-onnx `output_streaming`, which the server's generator claims alone, and
//! with whisper.cpp none.
//!
//! [`LocalConfig::from_env`] reads the backend from [`STT_VARIABLE`], its checkpoint from
//! [`STT_DIR_VARIABLE`] or [`WHISPERCPP_MODEL_VARIABLE`], and the server from
//! [`SERVER_URL_VARIABLE`] and [`SERVER_MODEL_VARIABLE`], naming the first that is missing. The
//! mock synthesiser speaks one 10 ms frame of silence per word, so `talk` plays silence where the
//! answer will be heard once the TTS adapter replaces it at the same binding.
//!
//! The LLM node is the testkit's `AgentNode`, which answers a turn's committed spans with the
//! generator's answer to their text ([`identity`]). The server's adapter claims output streaming
//! only, so the node asks each turn's spans as the user text of the request it starts the turn's
//! generation with; each delta the server streams is a segment committed as it arrives.
//!
//! The recorded speech of a person pauses longer than the synthetic utterance of the mock tests:
//! the checkpoint's LibriSpeech clip pauses for up to 570 ms below the mock detector's threshold
//! of 0.05, and up to 240 ms below 0.02. The local cascade's detector, [`SPEECH_VAD`], ends speech
//! after 600 ms below 0.02, so the clip is one turn. Turns are the detector's under both
//! recognisers: neither recogniser's own endpoint decides one.

use std::error::Error;
use std::fmt;
use std::net::{IpAddr, Ipv6Addr};
use std::num::{NonZeroU16, NonZeroU32};
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use inference_audio::{AudioEncoding, AudioFormat, AudioFrame};
use inference_audio_device::{AudioHost, DeviceError, DeviceSelector, PlayoutConfig};
use inference_core::envelope::EventEnvelope;
use inference_core::id::RunId;
use inference_core::model::{Adapter, Locality, Model, ModelId};
use inference_llama_server::{Config, LlamaServer, declare_adapter, declare_model};
use inference_llm::Generator;
use inference_pipeline::Declaration;
use inference_pipeline::nodes::vad::VadConfig;
use inference_sherpa_onnx::stt::{SherpaOnnxRecogniser, SherpaOnnxRecognition};
use inference_testkit::cascade::{CascadeRefused, TurnStore};
use inference_testkit::graph::RunLog;
use inference_testkit::stt::inference_stt::{
    ConcurrencyLimitReached, InputEnded, Recogniser, RecognitionHandle, RecognitionRequest,
    RevisionRefused, TranscriptEvent, TranscriptOutput,
};
use inference_testkit::tts::{MockConfiguration, MockSynthesiser};
use inference_text::ReviseSpan;
use inference_tts::Synthesiser;
use inference_whispercpp::stt::{WhisperCppRecogniser, WhisperCppRecognition};

use crate::{CascadeRun, LocalCascade, Models, Output, declare_over};

/// The variable selecting the recogniser bound at `stt`: `sherpa-onnx`, the default when it is
/// not set, or `whispercpp` ([`SttBackend`]).
pub const STT_VARIABLE: &str = "INFERENCE_CASCADE_STT";

/// The variable naming the directory of the unpacked pinned checkpoint of the sherpa-onnx
/// recogniser.
pub const STT_DIR_VARIABLE: &str = "INFERENCE_SHERPA_ONNX_STT_DIR";

/// The variable naming the pinned checkpoint file of the whisper.cpp recogniser,
/// `ggml-base.en.bin`.
pub const WHISPERCPP_MODEL_VARIABLE: &str = "INFERENCE_WHISPERCPP_STT_MODEL";

/// The variable naming the base URL of the server's OpenAI-compatible API, such as
/// `http://127.0.0.1:8080/v1`.
pub const SERVER_URL_VARIABLE: &str = "INFERENCE_LLAMA_SERVER_URL";

/// The variable naming the model the server answers to, its `--alias`.
pub const SERVER_MODEL_VARIABLE: &str = "INFERENCE_LLAMA_SERVER_MODEL";

/// The localities the profile of the local cascade admits: the recogniser and the synthesiser
/// run in the process, the server is a local worker.
pub const ADMITTED: [Locality; 2] = [Locality::SameProcess, Locality::LocalWorker];

/// The detector of the local cascade: a frame is loud from a root mean square of 0.02, speech
/// starts after 30 ms of loud frames and ends after 600 ms of quiet ones; see the module
/// documentation.
pub const SPEECH_VAD: VadConfig = VadConfig {
    threshold: 0.02,
    attack: Duration::from_millis(30),
    hold: Duration::from_millis(600),
};

/// The most tokens one answer of the server may take, unless configured otherwise: a spoken
/// answer is short.
pub const MAX_OUTPUT_TOKENS: u64 = 64;

/// The reply the LLM node asks the generator for: the span's text itself.
#[must_use]
pub fn identity(text: &str) -> String {
    text.to_owned()
}

/// The recogniser bound at `stt`, by the value of [`STT_VARIABLE`] that selects it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SttBackend {
    /// `sherpa-onnx`: the streaming recogniser, claiming incremental input and output streaming.
    #[default]
    SherpaOnnx,
    /// `whispercpp`: the batch recogniser, claiming neither.
    WhisperCpp,
}

impl SttBackend {
    /// Every backend, the default first.
    pub const ALL: [Self; 2] = [Self::SherpaOnnx, Self::WhisperCpp];

    /// The value of [`STT_VARIABLE`] that selects the backend.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::SherpaOnnx => "sherpa-onnx",
            Self::WhisperCpp => "whispercpp",
        }
    }

    /// The variable naming the backend's checkpoint: [`STT_DIR_VARIABLE`] or
    /// [`WHISPERCPP_MODEL_VARIABLE`].
    #[must_use]
    pub const fn variable(self) -> &'static str {
        match self {
            Self::SherpaOnnx => STT_DIR_VARIABLE,
            Self::WhisperCpp => WHISPERCPP_MODEL_VARIABLE,
        }
    }

    /// The adapter record the backend's recogniser declares when it opens its checkpoint, at its
    /// default maximum concurrency, declared without the checkpoint: its capabilities, execution
    /// form and locality are the opened recogniser's, its model and adapter ids its own.
    ///
    /// # Panics
    ///
    /// Never: the default maximum concurrency of either adapter is positive.
    #[must_use]
    pub fn adapter(self) -> Adapter {
        let declared = match self {
            Self::SherpaOnnx => {
                let (model, _) = inference_sherpa_onnx::stt::declare_model();
                inference_sherpa_onnx::stt::declare_adapter(
                    model.model_id(),
                    inference_sherpa_onnx::stt::DEFAULT_MAX_CONCURRENCY,
                )
            }
            Self::WhisperCpp => {
                let (model, _) = inference_whispercpp::stt::declare_model();
                inference_whispercpp::stt::declare_adapter(
                    model.model_id(),
                    inference_whispercpp::stt::DEFAULT_MAX_CONCURRENCY,
                )
            }
        };
        declared.expect("a positive default concurrency").0
    }
}

impl fmt::Display for SttBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for SttBackend {
    type Err = LocalRefused;

    /// The backend whose [`SttBackend::name`] `value` is.
    ///
    /// # Errors
    ///
    /// [`LocalRefused::UnknownBackend`] naming `value` for any other.
    fn from_str(value: &str) -> Result<Self, LocalRefused> {
        Self::ALL
            .into_iter()
            .find(|backend| backend.name() == value)
            .ok_or_else(|| LocalRefused::UnknownBackend {
                value: value.to_owned(),
            })
    }
}

/// The recogniser bound at `stt` and where its checkpoint is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SttConfig {
    /// The sherpa-onnx recogniser over the unpacked checkpoint in `dir`.
    SherpaOnnx { dir: PathBuf },
    /// The whisper.cpp recogniser over the checkpoint file `model`.
    WhisperCpp { model: PathBuf },
}

impl SttConfig {
    /// The backend the configuration selects.
    #[must_use]
    pub const fn backend(&self) -> SttBackend {
        match self {
            Self::SherpaOnnx { .. } => SttBackend::SherpaOnnx,
            Self::WhisperCpp { .. } => SttBackend::WhisperCpp,
        }
    }

    /// The configuration of `backend`, its checkpoint at `path`.
    #[must_use]
    pub fn new(backend: SttBackend, path: PathBuf) -> Self {
        match backend {
            SttBackend::SherpaOnnx => Self::SherpaOnnx { dir: path },
            SttBackend::WhisperCpp => Self::WhisperCpp { model: path },
        }
    }
}

/// Where the local adapters are: the recogniser and its checkpoint, and the server, its served
/// model and the most tokens of one answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalConfig {
    pub stt: SttConfig,
    pub server_url: String,
    pub server_model: String,
    pub max_output_tokens: u64,
}

impl LocalConfig {
    /// The configuration the process environment names, with [`MAX_OUTPUT_TOKENS`].
    ///
    /// # Errors
    ///
    /// As [`LocalConfig::from_lookup`].
    pub fn from_env() -> Result<Self, LocalRefused> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// The configuration `lookup` answers for each variable, with [`MAX_OUTPUT_TOKENS`]: the
    /// backend [`STT_VARIABLE`] selects, [`SttBackend::SherpaOnnx`] when it answers none, the
    /// backend's checkpoint, [`SttBackend::variable`], and the server.
    ///
    /// # Errors
    ///
    /// [`LocalRefused::UnknownBackend`] when [`STT_VARIABLE`] names no backend, and
    /// [`LocalRefused::Missing`] naming the first of the backend's variable,
    /// [`SERVER_URL_VARIABLE`] and [`SERVER_MODEL_VARIABLE`] that `lookup` answers none for.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, LocalRefused> {
        let read = |name: &'static str| lookup(name).ok_or(LocalRefused::Missing(name));
        let backend = lookup(STT_VARIABLE).map_or(Ok(SttBackend::SherpaOnnx), |value| {
            value.parse::<SttBackend>()
        })?;
        Ok(Self {
            stt: SttConfig::new(backend, PathBuf::from(read(backend.variable())?)),
            server_url: read(SERVER_URL_VARIABLE)?,
            server_model: read(SERVER_MODEL_VARIABLE)?,
            max_output_tokens: MAX_OUTPUT_TOKENS,
        })
    }

    /// The configuration of the server's adapter: its URL, served model and
    /// [`LocalConfig::max_output_tokens`].
    #[must_use]
    pub fn server(&self) -> Config {
        let mut server = Config::new(self.server_url.clone(), self.server_model.clone());
        server.max_output_tokens = self.max_output_tokens;
        server
    }
}

/// Why the local adapters were not opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalRefused {
    /// The variable is not set.
    Missing(&'static str),
    /// [`STT_VARIABLE`] is set to `value`, which names no [`SttBackend`].
    UnknownBackend { value: String },
    /// The server URL is not an `http` or `https` URL whose host is loopback.
    NotLoopback { url: String },
    /// The checkpoint directory does not hold the sherpa-onnx recogniser's pinned checkpoint.
    SherpaOnnx(inference_sherpa_onnx::stt::OpenRefused),
    /// The checkpoint file is not the whisper.cpp recogniser's pinned checkpoint.
    WhisperCpp(inference_whispercpp::stt::OpenRefused),
    /// The server's adapter was not made: `llm` refused the binding or the client, or the adapter
    /// was not declared.
    Server { detail: String },
}

impl fmt::Display for LocalRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(name) => write!(f, "{name} is not set"),
            Self::UnknownBackend { value } => write!(
                f,
                "{STT_VARIABLE} is {value:?}, which names no recogniser: one of {:?}",
                SttBackend::ALL.map(SttBackend::name)
            ),
            Self::NotLoopback { url } => write!(
                f,
                "{url} is not an http URL whose host is loopback: the local cascade's profile \
                 denies egress"
            ),
            Self::SherpaOnnx(refused) => fmt::Display::fmt(refused, f),
            Self::WhisperCpp(refused) => fmt::Display::fmt(refused, f),
            Self::Server { detail } => write!(f, "the llama-server adapter: {detail}"),
        }
    }
}

impl Error for LocalRefused {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::SherpaOnnx(refused) => Some(refused),
            Self::WhisperCpp(refused) => Some(refused),
            Self::Missing(_)
            | Self::UnknownBackend { .. }
            | Self::NotLoopback { .. }
            | Self::Server { .. } => None,
        }
    }
}

/// Whether `url` is an `http` or `https` URL whose host is `localhost` or a loopback address,
/// IPv4 or IPv6, an IPv4 address mapped into IPv6 read as the IPv4 address.
///
/// # Errors
///
/// [`LocalRefused::NotLoopback`] naming `url` otherwise.
pub fn loopback(url: &str) -> Result<(), LocalRefused> {
    let refused = || LocalRefused::NotLoopback {
        url: url.to_owned(),
    };
    let (scheme, rest) = url.split_once("://").ok_or_else(refused)?;
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")) {
        return Err(refused());
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = authority.rsplit('@').next().unwrap_or_default();
    let host = if let Some(bracketed) = host_port.strip_prefix('[') {
        let (address, _) = bracketed.split_once(']').ok_or_else(refused)?;
        let address: Ipv6Addr = address.parse().map_err(|_| refused())?;
        return if IpAddr::V6(address).to_canonical().is_loopback() {
            Ok(())
        } else {
            Err(refused())
        };
    } else {
        host_port.split(':').next().unwrap_or_default()
    };
    let admitted = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.to_canonical().is_loopback());
    if admitted { Ok(()) } else { Err(refused()) }
}

/// The mock synthesiser bound at `tts`: streaming each segment as it takes it, in 16 kHz mono
/// `pcm_s16le`.
#[must_use]
pub fn synthesiser() -> MockSynthesiser {
    MockSynthesiser::new(
        MockConfiguration::IncrementalStreaming,
        ModelId::generate(),
        AudioFormat {
            encoding: AudioEncoding::PcmS16le,
            sample_rate: NonZeroU32::new(16_000).unwrap_or(NonZeroU32::MIN),
            channels: NonZeroU16::MIN,
        },
    )
}

/// The adapter of the server `server` configures, declared without a request to it.
///
/// # Errors
///
/// [`LocalRefused::Server`] when the adapter is not declared.
pub fn server_adapter(server: &Config) -> Result<Adapter, LocalRefused> {
    let (model, _) = Model::declare(declare_model());
    let (adapter, _) =
        Adapter::declare(declare_adapter(model.model_id(), server)).map_err(|refused| {
            LocalRefused::Server {
                detail: refused.to_string(),
            }
        })?;
    Ok(adapter)
}

/// The recogniser bound at `stt`, of the backend its configuration selected. It is one
/// `inference-stt` [`Recogniser`] whichever backend it runs, so the node implementation and the
/// cascade over it are the same under both.
#[derive(Debug, Clone)]
pub enum LocalRecogniser {
    /// The sherpa-onnx streaming recogniser.
    SherpaOnnx(SherpaOnnxRecogniser),
    /// The whisper.cpp batch recogniser.
    WhisperCpp(WhisperCppRecogniser),
}

impl LocalRecogniser {
    /// Opens the recogniser `config` selects on its checkpoint.
    ///
    /// # Errors
    ///
    /// [`LocalRefused::SherpaOnnx`] or [`LocalRefused::WhisperCpp`], the adapter's refusal to
    /// open the checkpoint.
    pub fn open(config: &SttConfig) -> Result<Self, LocalRefused> {
        match config {
            SttConfig::SherpaOnnx { dir } => SherpaOnnxRecogniser::open(dir)
                .map(Self::SherpaOnnx)
                .map_err(LocalRefused::SherpaOnnx),
            SttConfig::WhisperCpp { model } => WhisperCppRecogniser::open(model)
                .map(Self::WhisperCpp)
                .map_err(LocalRefused::WhisperCpp),
        }
    }

    /// The backend the recogniser runs.
    #[must_use]
    pub const fn backend(&self) -> SttBackend {
        match self {
            Self::SherpaOnnx(_) => SttBackend::SherpaOnnx,
            Self::WhisperCpp(_) => SttBackend::WhisperCpp,
        }
    }
}

impl Recogniser for LocalRecogniser {
    type Handle = LocalRecognition;

    fn adapter(&self) -> &Adapter {
        match self {
            Self::SherpaOnnx(recogniser) => recogniser.adapter(),
            Self::WhisperCpp(recogniser) => recogniser.adapter(),
        }
    }

    fn start(
        &mut self,
        request: RecognitionRequest,
    ) -> Result<LocalRecognition, ConcurrencyLimitReached> {
        match self {
            Self::SherpaOnnx(recogniser) => {
                recogniser.start(request).map(LocalRecognition::SherpaOnnx)
            }
            Self::WhisperCpp(recogniser) => {
                recogniser.start(request).map(LocalRecognition::WhisperCpp)
            }
        }
    }
}

/// One recognition of a [`LocalRecogniser`], of the backend that started it.
#[derive(Debug)]
pub enum LocalRecognition {
    /// A recognition of the sherpa-onnx recogniser.
    SherpaOnnx(SherpaOnnxRecognition),
    /// A recognition of the whisper.cpp recogniser.
    WhisperCpp(WhisperCppRecognition),
}

impl Iterator for LocalRecognition {
    type Item = EventEnvelope<TranscriptEvent>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::SherpaOnnx(recognition) => recognition.next(),
            Self::WhisperCpp(recognition) => recognition.next(),
        }
    }
}

impl RecognitionHandle for LocalRecognition {
    fn output(&self) -> &TranscriptOutput {
        match self {
            Self::SherpaOnnx(recognition) => recognition.output(),
            Self::WhisperCpp(recognition) => recognition.output(),
        }
    }

    fn offer_frame(&mut self, frame: AudioFrame) -> Result<(), InputEnded> {
        match self {
            Self::SherpaOnnx(recognition) => recognition.offer_frame(frame),
            Self::WhisperCpp(recognition) => recognition.offer_frame(frame),
        }
    }

    fn end_input(&mut self) -> Result<(), InputEnded> {
        match self {
            Self::SherpaOnnx(recognition) => recognition.end_input(),
            Self::WhisperCpp(recognition) => recognition.end_input(),
        }
    }

    fn revise(&mut self, input: ReviseSpan) -> Result<(), RevisionRefused> {
        match self {
            Self::SherpaOnnx(recognition) => recognition.revise(input),
            Self::WhisperCpp(recognition) => recognition.revise(input),
        }
    }
}

/// The models of the local cascade's three model nodes.
#[derive(Debug, Clone)]
pub struct LocalModels {
    pub recogniser: LocalRecogniser,
    pub generator: Generator,
    pub synthesiser: MockSynthesiser,
}

impl LocalModels {
    /// Opens the recogniser `config.stt` selects on its checkpoint and the generator of the
    /// server `config` names, once its URL is found loopback, and makes the mock synthesiser.
    /// Nothing is sent to the server before the first turn.
    ///
    /// # Errors
    ///
    /// [`LocalRefused::NotLoopback`] before anything is opened, the recogniser's
    /// [`LocalRefused::SherpaOnnx`] or [`LocalRefused::WhisperCpp`], or
    /// [`LocalRefused::Server`].
    pub fn open(config: &LocalConfig) -> Result<Self, LocalRefused> {
        loopback(&config.server_url)?;
        let recogniser = LocalRecogniser::open(&config.stt)?;
        let server = config.server();
        let server_refused = |detail: String| LocalRefused::Server { detail };
        let adapter = server_adapter(&server)?;
        let generator = LlamaServer::new(&server)
            .map_err(|refused| server_refused(refused.to_string()))?
            .generator(adapter)
            .map_err(|refused| server_refused(refused.to_string()))?;
        Ok(Self {
            recogniser,
            generator,
            synthesiser: synthesiser(),
        })
    }
}

/// Declares the cascade in `declaration`, its model nodes bound to the adapters of `models`,
/// under the profile admitting [`ADMITTED`], and validates and plans it.
///
/// # Errors
///
/// [`CascadeRefused`] when the declaration does not validate or plan, which it does unless an
/// adapter claims less than its node requires.
pub fn declare_local(
    declaration: &mut Declaration,
    models: &LocalModels,
) -> Result<LocalCascade, CascadeRefused> {
    declare_local_adapters(
        declaration,
        [
            models.recogniser.adapter().clone(),
            models.generator.adapter().clone(),
            models.synthesiser.adapter().clone(),
        ],
    )
}

/// Declares the cascade in `declaration`, its nodes `stt`, `agent` and `tts` bound to `adapters`
/// in that order, under the profile admitting [`ADMITTED`], and validates and plans it: the
/// declaration of [`declare_local`] over adapter records alone, such as
/// [`SttBackend::adapter`] and [`server_adapter`], with no checkpoint opened.
///
/// # Errors
///
/// [`CascadeRefused`] when the declaration does not validate or plan, as when an adapter claims
/// less than its node requires or its locality is not admitted.
pub fn declare_local_adapters(
    declaration: &mut Declaration,
    adapters: [Adapter; 3],
) -> Result<LocalCascade, CascadeRefused> {
    declare_over(declaration, adapters, ADMITTED.to_vec())
}

impl LocalCascade {
    /// The implementations of the nodes of the run `run_id` over the local `models`, the
    /// detector [`SPEECH_VAD`], the LLM node asking [`identity`], the turn policy's commands
    /// answered by `turns`, what the nodes emit recorded in `log`, and the speaker the output
    /// device `device` of `host` plays, queuing `playout`.
    ///
    /// # Errors
    ///
    /// The [`DeviceError`] of opening the speaker; no node is made.
    #[allow(clippy::too_many_arguments)]
    pub fn local_nodes(
        &self,
        run_id: RunId,
        models: LocalModels,
        turns: TurnStore,
        log: &RunLog,
        host: &dyn AudioHost,
        device: &DeviceSelector,
        playout: PlayoutConfig,
    ) -> Result<CascadeRun, DeviceError> {
        let models = Models {
            recogniser: models.recogniser,
            generator: models.generator,
            reply: identity,
            synthesiser: models.synthesiser,
        };
        let output = Output {
            host,
            device,
            playout,
        };
        self.nodes_over(run_id, models, SPEECH_VAD, turns, log, &output)
    }
}
