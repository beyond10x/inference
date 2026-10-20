//! The cascade over the local adapters (feature `local`; A-05 of `docs/design/inference.md` at
//! 1141): the same declaration as [`declare`](crate::declare), its model nodes bound to
//!
//! | node | adapter | locality |
//! |---|---|---|
//! | `stt` | `inference-sherpa-onnx`'s streaming recogniser over its pinned checkpoint | `same_process` |
//! | `agent` | `inference-llama-server`'s generator over a `llama-server` on loopback | `local_worker` |
//! | `tts` | the testkit's mock synthesiser, until a local TTS adapter exists | `same_process` |
//!
//! under the `offline_strict` profile with egress denied admitting [`ADMITTED`], both localities.
//! The connection to the server is compute on the machine, not egress: [`LocalModels::open`]
//! refuses a server URL whose host is not loopback ([`loopback`]) before it opens anything, and
//! the egress watch of `inference-runtime` admits loopback.
//!
//! [`LocalConfig::from_env`] reads the checkpoint directory from [`STT_DIR_VARIABLE`] and the
//! server from [`SERVER_URL_VARIABLE`] and [`SERVER_MODEL_VARIABLE`], naming the first that is
//! missing. The mock synthesiser speaks one 10 ms frame of silence per word, so `talk` plays
//! silence where the answer will be heard once the TTS adapter replaces it at the same binding.
//!
//! The LLM node is the testkit's `AgentNode`, which answers a turn's committed spans with the
//! generator's answer to their text ([`identity`]). The server's adapter claims output streaming
//! only, so the node asks each turn's spans as the user text of the request it starts the turn's
//! generation with; each delta the server streams is a segment committed as it arrives.
//!
//! The recorded speech of a person pauses longer than the synthetic utterance of the mock tests:
//! the checkpoint's LibriSpeech clip pauses for up to 570 ms below the mock detector's threshold
//! of 0.05, and up to 240 ms below 0.02. The local cascade's detector, [`SPEECH_VAD`], ends speech
//! after 600 ms below 0.02, so the clip is one turn.

use std::error::Error;
use std::fmt;
use std::net::{IpAddr, Ipv6Addr};
use std::num::{NonZeroU16, NonZeroU32};
use std::path::PathBuf;
use std::time::Duration;

use inference_audio::{AudioEncoding, AudioFormat};
use inference_audio_device::{AudioHost, DeviceError, DeviceSelector, PlayoutConfig};
use inference_core::id::RunId;
use inference_core::model::{Adapter, Locality, Model, ModelId};
use inference_llama_server::{Config, LlamaServer, declare_adapter, declare_model};
use inference_llm::Generator;
use inference_pipeline::Declaration;
use inference_pipeline::nodes::vad::VadConfig;
use inference_sherpa_onnx::stt::{OpenRefused, SherpaOnnxRecogniser};
use inference_testkit::cascade::{CascadeRefused, TurnStore};
use inference_testkit::graph::RunLog;
use inference_testkit::stt::inference_stt::Recogniser;
use inference_testkit::tts::{MockConfiguration, MockSynthesiser};
use inference_tts::Synthesiser;

use crate::{CascadeRun, LocalCascade, Models, Output, declare_over};

/// The variable naming the directory of the unpacked pinned checkpoint of the recogniser.
pub const STT_DIR_VARIABLE: &str = "INFERENCE_SHERPA_ONNX_STT_DIR";

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

/// Where the local adapters are: the checkpoint directory of the recogniser, and the server, its
/// served model and the most tokens of one answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalConfig {
    pub stt_dir: PathBuf,
    pub server_url: String,
    pub server_model: String,
    pub max_output_tokens: u64,
}

impl LocalConfig {
    /// The configuration the process environment names, with [`MAX_OUTPUT_TOKENS`].
    ///
    /// # Errors
    ///
    /// [`LocalRefused::Missing`] naming the first of [`STT_DIR_VARIABLE`],
    /// [`SERVER_URL_VARIABLE`] and [`SERVER_MODEL_VARIABLE`] that is not set or not Unicode.
    pub fn from_env() -> Result<Self, LocalRefused> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// The configuration `lookup` answers for each variable, with [`MAX_OUTPUT_TOKENS`].
    ///
    /// # Errors
    ///
    /// [`LocalRefused::Missing`] naming the first variable `lookup` answers none for.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, LocalRefused> {
        let read = |name: &'static str| lookup(name).ok_or(LocalRefused::Missing(name));
        Ok(Self {
            stt_dir: PathBuf::from(read(STT_DIR_VARIABLE)?),
            server_url: read(SERVER_URL_VARIABLE)?,
            server_model: read(SERVER_MODEL_VARIABLE)?,
            max_output_tokens: MAX_OUTPUT_TOKENS,
        })
    }
}

/// Why the local adapters were not opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalRefused {
    /// The variable is not set.
    Missing(&'static str),
    /// The server URL is not an `http` or `https` URL whose host is loopback.
    NotLoopback { url: String },
    /// The checkpoint directory does not hold the recogniser's pinned checkpoint.
    Recogniser(OpenRefused),
    /// The server's adapter was not made: `llm` refused the binding or the client, or the adapter
    /// was not declared.
    Server { detail: String },
}

impl fmt::Display for LocalRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(name) => write!(f, "{name} is not set"),
            Self::NotLoopback { url } => write!(
                f,
                "{url} is not an http URL whose host is loopback: the local cascade's profile \
                 denies egress"
            ),
            Self::Recogniser(refused) => fmt::Display::fmt(refused, f),
            Self::Server { detail } => write!(f, "the llama-server adapter: {detail}"),
        }
    }
}

impl Error for LocalRefused {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Recogniser(refused) => Some(refused),
            Self::Missing(_) | Self::NotLoopback { .. } | Self::Server { .. } => None,
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

/// The models of the local cascade's three model nodes.
#[derive(Debug, Clone)]
pub struct LocalModels {
    pub recogniser: SherpaOnnxRecogniser,
    pub generator: Generator,
    pub synthesiser: MockSynthesiser,
}

impl LocalModels {
    /// Opens the recogniser on the checkpoint in `config.stt_dir` and the generator of the
    /// server `config` names, once its URL is found loopback, and makes the mock synthesiser.
    /// Nothing is sent to the server before the first turn.
    ///
    /// # Errors
    ///
    /// [`LocalRefused::NotLoopback`] before anything is opened, the recogniser's
    /// [`LocalRefused::Recogniser`], or [`LocalRefused::Server`].
    pub fn open(config: &LocalConfig) -> Result<Self, LocalRefused> {
        loopback(&config.server_url)?;
        let recogniser =
            SherpaOnnxRecogniser::open(&config.stt_dir).map_err(LocalRefused::Recogniser)?;
        let mut server = Config::new(config.server_url.clone(), config.server_model.clone());
        server.max_output_tokens = config.max_output_tokens;
        let server_refused = |detail: String| LocalRefused::Server { detail };
        let (model, _) = Model::declare(declare_model());
        let (adapter, _) = Adapter::declare(declare_adapter(model.model_id(), &server))
            .map_err(|refused| server_refused(refused.to_string()))?;
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
    declare_over(
        declaration,
        [
            models.recogniser.adapter().clone(),
            models.generator.adapter().clone(),
            models.synthesiser.adapter().clone(),
        ],
        ADMITTED.to_vec(),
    )
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
