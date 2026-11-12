//! Compare native speech providers and a cascade of STT, LLM and TTS on the same recorded
//! utterances, without a microphone or a speaker: latency from the end of the question's speech
//! to the answer's first audio, first transcript and end, barge-in to output stop, the
//! cascade's own steps, and the usage of each run. Every run of a hosted provider is billed.
//!
//! ```text
//! cargo run -p realtime-voice --example compare -- --providers openai,openai-live --runs 2
//! INFERENCE_COMPARE_BARGE_IN_WAV=<a WAV file of speech> cargo run -p realtime-voice --example compare -- --runs 2
//! cargo run -p realtime-voice --example compare -- --skip nova="no model access"
//! cargo run -p realtime-voice --features local --example compare -- --providers openai,cascade
//! cargo run -p realtime-voice --features local --example compare -- --providers elevenlabs,cascade --cascade-stt elevenlabs
//! cargo run -p realtime-voice --example compare -- --clips questions/ --runs 10 --runs-out runs.jsonl
//! ```
//!
//! The question is the OpenAI adapter's speech fixture ("What is the capital of France?"), or with
//! `--clips <dir>` each WAV file of the directory in turn (`realtime_voice::compare::Question`),
//! every provider running `--runs` runs of each. `--runs-out <file>` writes every run's times as
//! JSON lines (`Comparison::runs_json`). The barge-in utterance is the 16-bit PCM WAV file `INFERENCE_COMPARE_BARGE_IN_WAV` names; without
//! it no barge-in is sent and that column stays empty. `realtime_voice::compare` says how each
//! figure is taken: on the client, at the receipt of each event.
//!
//! Each native provider runs with the session configuration `talk` uses and its key or
//! credentials as `talk` reads them. The ElevenLabs agent is `ELEVENLABS_AGENT_ID` for a public
//! agent, or `ELEVENLABS_AGENT_SIGNED_URL` for a private one, and must take and give `pcm_16000`
//! (`realtime_voice::elevenlabs_config`); it reports no usage. The Deepgram Voice Agent reads its
//! key from `DEEPGRAM_API_KEY`, runs with the default providers of
//! `inference_deepgram::agent::AgentConfig` in G.711 μ-law at 8 kHz
//! (`realtime_voice::deepgram_config`), and reports no usage: it bills per minute, which no
//! message carries. Phonic reads its key from `PHONIC_API_KEY`, runs `phonic-1` in 16-bit PCM at
//! 16 kHz both ways with the demo instructions and tool (`phonic_config`), and reports no usage.
//! Deepslate Opal reads its key from `DEEPSLATE_API_KEY`, its vendor and
//! organization from `DEEPSLATE_VENDOR_ID` and `DEEPSLATE_ORGANIZATION_ID`, and the hosted voice
//! it speaks with from `DEEPSLATE_VOICE_ID`, without which it is skipped; it runs in 16-bit PCM
//! at 16 kHz and reports no usage, which no message carries. A provider whose model cannot be
//! made or whose first session does not open is skipped with the reason;
//! `--skip <provider>=<reason>` skips one without trying it.
//!
//! The cascade arm, `cascade`, speaks with ElevenLabs when its key resolves, from
//! `ELEVENLABS_API_KEY` or else the OS keychain's `elevenlabs-api-key`
//! (`realtime_voice::elevenlabs::key`, which says on standard error when it asks the keychain),
//! and `ELEVENLABS_VOICE_ID` names a voice, in `pcm_24000`, at the host
//! `ELEVENLABS_RESIDENCY` names (`realtime_voice::elevenlabs::endpoint`); otherwise it is
//! skipped as "no hosted TTS configured". With `--cascade-voice cartesia` it speaks with Cartesia
//! Sonic instead, its key from `CARTESIA_API_KEY` and its voice from `CARTESIA_VOICE_ID`, in
//! 16-bit PCM at 24 kHz. Its recogniser and LLM are those the local voice
//! cascade configures (`local_voice_cascade::local::LocalConfig::from_env`), bound with feature
//! `local`, with that cascade's detector; a missing one skips the arm, naming it. With
//! `--cascade-stt elevenlabs` it listens with ElevenLabs realtime speech-to-text instead
//! (`realtime_voice::compare::elevenlabs_arm`), its key `ELEVENLABS_API_KEY` or the OS keychain's
//! `elevenlabs-api-key` (`realtime_voice::elevenlabs::key`), committed manually when the arm's
//! detector ends the turn, and its LLM is the server `INFERENCE_LLAMA_SERVER_URL` and
//! `INFERENCE_LLAMA_SERVER_MODEL` name (`realtime_voice::elevenlabs::generator`), without the
//! local recogniser's variables. A failed recognition is a note of the run.

use std::error::Error;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::{Parser, ValueEnum};
use inference_aws::duplex::NovaSonicSpeech;
use inference_deepgram::agent::AgentConfig;
use inference_deepgram::{AgentEndpoint as DeepgramEndpoint, DeepgramAgent};
use inference_deepslate::{DeepslateOpal, OpalConfig};
use inference_duplex::inference_tools::{DeclareTool, Tool};
use inference_duplex::{NativeSpeechModel, SessionConfig};
use inference_elevenlabs::ElevenLabsAgent;
use inference_elevenlabs::agent::{AgentEndpoint, SignedUrl};
use inference_openai::duplex::OpenAiRealtime;
use inference_openai::live_duplex::{OpenAiLive, OpenAiLiveSession};
use inference_openai::{
    CredentialSource, Endpoint, EnvironmentSource, KeychainSource, OPENAI_KEY_NAME,
};
use inference_phonic::{PhonicConfig, PhonicEndpoint, PhonicSpeech};
use realtime_voice::compare::{
    Billing, Clip, Comparison, Measured, Plan, Question, Topology, measure_arm,
};
use realtime_voice::{
    Reported, deepgram_config, elevenlabs_config, nova_config, openai_config, openai_live_config,
    pcm16,
};

const QUESTION: &[u8] =
    include_bytes!("../../../adapters/inference-openai/tests/fixtures/speech.wav");

/// The variable naming the barge-in utterance.
const BARGE_IN_VARIABLE: &str = "INFERENCE_COMPARE_BARGE_IN_WAV";

/// The variable that switches the OpenAI key from the keychain to `OPENAI_API_KEY`.
const KEY_SOURCE_VARIABLE: &str = "INFERENCE_OPENAI_KEY_SOURCE";

/// Compare native speech providers and a cascade on recorded utterances.
#[derive(Debug, Parser)]
struct Args {
    /// The providers to run, in order.
    #[arg(
        long,
        value_enum,
        value_delimiter = ',',
        default_value = "openai,openai-live,nova,elevenlabs,deepgram,phonic,deepslate,cascade"
    )]
    providers: Vec<Provider>,
    /// The runs of each provider.
    #[arg(long, default_value_t = 3)]
    runs: usize,
    /// A provider not to try, and why: `nova="no model access"`.
    #[arg(long, value_name = "PROVIDER=REASON")]
    skip: Vec<String>,
    /// The length of one input frame, in milliseconds.
    #[arg(long, default_value_t = 20)]
    frame_ms: u64,
    /// The hosted voice the cascade arm speaks with.
    #[arg(long, value_enum, default_value = "elevenlabs")]
    cascade_voice: CascadeVoice,
    /// The recogniser the cascade arm listens with.
    #[arg(long, value_enum, default_value = "local")]
    cascade_stt: CascadeStt,
    /// A directory of 16-bit PCM WAV question clips, each asked in turn in place of the built-in
    /// question.
    #[arg(long, value_name = "DIR")]
    clips: Option<PathBuf>,
    /// A file to write every run's times to, one JSON line per run.
    #[arg(long, value_name = "FILE")]
    runs_out: Option<PathBuf>,
}

/// A recogniser the cascade arm can listen with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum CascadeStt {
    /// The local voice cascade's recogniser, sherpa-onnx or whisper.cpp.
    Local,
    /// ElevenLabs realtime speech-to-text, committed when the arm's detector ends the turn.
    Elevenlabs,
}

/// A hosted TTS the cascade arm can speak with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum CascadeVoice {
    /// ElevenLabs streaming TTS.
    Elevenlabs,
    /// Cartesia Sonic streaming TTS.
    Cartesia,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Provider {
    /// The OpenAI Realtime API.
    Openai,
    /// OpenAI GPT-Live.
    OpenaiLive,
    /// Amazon Nova Sonic on Bedrock.
    Nova,
    /// An ElevenLabs agent conversation.
    Elevenlabs,
    /// A Deepgram Voice Agent session.
    Deepgram,
    /// A Phonic speech-to-speech conversation.
    Phonic,
    /// A Deepslate Opal session.
    Deepslate,
    /// A recogniser (--cascade-stt), the local LLM and a hosted voice (--cascade-voice).
    Cascade,
}

impl Provider {
    const fn name(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::OpenaiLive => "openai-live",
            Self::Nova => "nova",
            Self::Elevenlabs => "elevenlabs",
            Self::Deepgram => "deepgram",
            Self::Phonic => "phonic",
            Self::Deepslate => "deepslate",
            Self::Cascade => "cascade",
        }
    }

    const fn topology(self) -> Topology {
        match self {
            Self::Openai
            | Self::OpenaiLive
            | Self::Nova
            | Self::Elevenlabs
            | Self::Deepgram
            | Self::Phonic
            | Self::Deepslate => Topology::Native,
            Self::Cascade => Topology::Cascade,
        }
    }
}

/// What no ElevenLabs agent message reports: every count.
const ELEVENLABS_REPORTED: Reported = Reported {
    input_text: false,
    input_audio: false,
    input_cached: false,
    output_text: false,
    output_audio: false,
};

/// What no Deepgram Voice Agent message reports: every count.
const DEEPGRAM_REPORTED: Reported = Reported {
    input_text: false,
    input_audio: false,
    input_cached: false,
    output_text: false,
    output_audio: false,
};

/// What no Phonic message reports: every count.
const PHONIC_REPORTED: Reported = Reported {
    input_text: false,
    input_audio: false,
    input_cached: false,
    output_text: false,
    output_audio: false,
};

/// The session compare opens with Phonic: 16-bit PCM at 16 kHz both ways, Phonic's own
/// turn-taking, responses and barge-in, the demo instructions and tool, and Phonic's default
/// voice. Phonic declares inline tools with `strict: true`, so the tool's schema takes the
/// strict form: no additional properties, every property required.
fn phonic_config() -> SessionConfig {
    SessionConfig {
        voice: String::new(),
        instructions: realtime_voice::INSTRUCTIONS.to_owned(),
        tools: vec![strict(&realtime_voice::time_tool())],
        ..realtime_voice::elevenlabs_config()
    }
}

/// `tool` with its schema in OpenAI's strict form.
fn strict(tool: &Tool) -> Tool {
    let mut schema = tool.schema().clone();
    let required: Vec<serde_json::Value> = schema["properties"]
        .as_object()
        .map(|properties| properties.keys().cloned().map(Into::into).collect())
        .unwrap_or_default();
    schema["additionalProperties"] = false.into();
    schema["required"] = required.into();
    Tool::declare(DeclareTool {
        name: tool.name().to_owned(),
        description: tool.description().to_owned(),
        schema,
        schema_revision: tool.schema_revision().to_owned(),
        side_effect: tool.side_effect(),
        requires_confirmation: tool.requires_confirmation(),
    })
    .0
}

fn main() -> ExitCode {
    let args = Args::parse();
    let comparison = match compare(&args) {
        Ok(comparison) => comparison,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
    print!("{comparison}");
    if let Some(path) = &args.runs_out
        && let Err(error) = std::fs::write(path, comparison.runs_json())
    {
        eprintln!(
            "error: --runs-out {} was not written: {error}",
            path.display()
        );
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// What no Deepslate Opal message reports: every count.
const DEEPSLATE_REPORTED: Reported = DEEPGRAM_REPORTED;

/// The variable naming the hosted voice Deepslate Opal speaks with.
const DEEPSLATE_VOICE_VARIABLE: &str = "DEEPSLATE_VOICE_ID";

/// The session `compare` opens on Deepslate Opal: 16-bit PCM at 16 kHz both ways, its own
/// turn-taking, responses and barge-in, the hosted voice `DEEPSLATE_VOICE_ID` names, the demo
/// instructions and tool.
fn deepslate_config() -> SessionConfig {
    SessionConfig {
        voice: std::env::var(DEEPSLATE_VOICE_VARIABLE).unwrap_or_default(),
        input_format: pcm16(16_000),
        output_format: pcm16(16_000),
        ..deepgram_config()
    }
}

/// The questions to ask: the WAV clips of `--clips`, or the built-in question.
fn questions(args: &Args) -> Result<Vec<Question>, Box<dyn Error>> {
    Ok(match &args.clips {
        Some(dir) => Question::in_dir(dir)?,
        None => vec![Question {
            name: "question".to_owned(),
            clip: Clip::from_wav(QUESTION)?,
        }],
    })
}

fn compare(args: &Args) -> Result<Comparison, Box<dyn Error>> {
    let questions = questions(args)?;
    let mut plan = Plan::new(questions[0].clip.clone());
    plan.frame = Duration::from_millis(args.frame_ms.max(1));
    match std::env::var_os(BARGE_IN_VARIABLE) {
        Some(path) => {
            let path = PathBuf::from(path);
            let bytes = std::fs::read(&path)
                .map_err(|error| format!("{} did not read: {error}", path.display()))?;
            plan.barge_in = Some(
                Clip::from_wav(&bytes).map_err(|error| format!("{}: {error}", path.display()))?,
            );
        }
        None => eprintln!("{BARGE_IN_VARIABLE} is not set: no barge-in is sent"),
    }
    let mut comparison = Comparison::new();
    for provider in &args.providers {
        let name = provider.name();
        if let Some(reason) = skipped(&args.skip, name) {
            comparison.skipped_as(name, provider.topology(), reason);
            continue;
        }
        let outcome = match provider {
            Provider::Cascade => {
                match cascade::measure(&plan, &questions, args) {
                    Ok(runs) => comparison.ran_as(name, provider.topology(), runs),
                    Err(reason) => comparison.skipped_as(name, provider.topology(), &reason),
                }
                continue;
            }
            Provider::Openai => openai_key().and_then(|source| {
                let mut model = OpenAiRealtime::new(Endpoint::from_env(), source, OPENAI_KEY_NAME);
                measure(
                    name,
                    &mut model,
                    openai_config,
                    &plan,
                    &questions,
                    args.runs,
                    &Billing::tokens(Reported::OPENAI),
                )
            }),
            Provider::OpenaiLive => openai_key().and_then(|source| {
                let mut model = OpenAiLive::hosted(source, OPENAI_KEY_NAME);
                let billing = Billing {
                    reported: Reported::OPENAI_LIVE,
                    seconds: OpenAiLiveSession::voice_seconds,
                };
                measure(
                    name,
                    &mut model,
                    openai_live_config,
                    &plan,
                    &questions,
                    args.runs,
                    &billing,
                )
            }),
            Provider::Nova => NovaSonicSpeech::from_env()
                .map_err(Box::<dyn Error>::from)
                .and_then(|mut model| {
                    measure(
                        name,
                        &mut model,
                        nova_config,
                        &plan,
                        &questions,
                        args.runs,
                        &Billing::tokens(Reported::NOVA),
                    )
                }),
            Provider::Elevenlabs => elevenlabs_agent().and_then(|mut model| {
                measure(
                    name,
                    &mut model,
                    elevenlabs_config,
                    &plan,
                    &questions,
                    args.runs,
                    &Billing::tokens(ELEVENLABS_REPORTED),
                )
            }),
            Provider::Deepgram => {
                DeepgramAgent::from_env(DeepgramEndpoint::new(), AgentConfig::new())
                    .map_err(Box::<dyn Error>::from)
                    .and_then(|mut model| {
                        measure(
                            name,
                            &mut model,
                            deepgram_config,
                            &plan,
                            &questions,
                            args.runs,
                            &Billing::tokens(DEEPGRAM_REPORTED),
                        )
                    })
            }
            Provider::Phonic => PhonicSpeech::from_env(PhonicEndpoint::new(), PhonicConfig::new())
                .map_err(Box::<dyn Error>::from)
                .and_then(|mut model| {
                    measure(
                        name,
                        &mut model,
                        phonic_config,
                        &plan,
                        &questions,
                        args.runs,
                        &Billing::tokens(PHONIC_REPORTED),
                    )
                }),
            Provider::Deepslate => match std::env::var_os(DEEPSLATE_VOICE_VARIABLE) {
                None => Err(format!("{DEEPSLATE_VOICE_VARIABLE} names no hosted voice").into()),
                Some(_) => DeepslateOpal::from_env(OpalConfig::new())
                    .map_err(Box::<dyn Error>::from)
                    .and_then(|mut model| {
                        measure(
                            name,
                            &mut model,
                            deepslate_config,
                            &plan,
                            &questions,
                            args.runs,
                            &Billing::tokens(DEEPSLATE_REPORTED),
                        )
                    }),
            },
        };
        match outcome {
            Ok(runs) => comparison.ran_as(name, provider.topology(), runs),
            Err(error) => comparison.skipped_as(name, provider.topology(), &error.to_string()),
        }
    }
    Ok(comparison)
}

/// The cascade arm over the local voice cascade's recogniser and LLM, or over ElevenLabs
/// speech-to-text and that LLM, speaking with ElevenLabs or Cartesia. Everything that needs no
/// key is checked first, the ElevenLabs key last, so an arm skipped for another reason never asks
/// the OS keychain.
#[cfg(feature = "local")]
mod cascade {
    use std::sync::Arc;

    use inference_elevenlabs::{CredentialSource, ElevenLabsSynthesiser};
    use inference_llm::Generator;
    use inference_tts::Synthesiser;
    use local_voice_cascade::local::{
        LocalConfig, LocalModels, LocalRefused, SERVER_MODEL_VARIABLE, SERVER_URL_VARIABLE,
        SPEECH_VAD,
    };
    use realtime_voice::compare::{
        Cascade, Measured, NO_HOSTED_TTS, Plan, Question, VOICE_VARIABLE, elevenlabs_arm,
        measure_cascade,
    };
    use realtime_voice::talk::Refused;
    use realtime_voice::{elevenlabs, pcm16};

    use super::{Args, CascadeStt, CascadeVoice};

    /// What the arm listens with, and its LLM, opened before any key is asked for.
    enum Listener {
        /// The local voice cascade's recogniser and LLM.
        Local(Box<LocalModels>),
        /// ElevenLabs speech-to-text, its key still to be resolved, and the LLM.
        Elevenlabs(Box<Generator>),
    }

    /// The reason a local step is missing for, naming it.
    fn missing(refused: &LocalRefused) -> String {
        let llm = match refused {
            LocalRefused::Missing(name) => {
                [SERVER_URL_VARIABLE, SERVER_MODEL_VARIABLE].contains(name)
            }
            LocalRefused::NotLoopback { .. } | LocalRefused::Server { .. } => true,
            LocalRefused::UnknownBackend { .. }
            | LocalRefused::SherpaOnnx(_)
            | LocalRefused::WhisperCpp(_) => false,
        };
        let step = if llm { "LLM" } else { "STT" };
        format!("no local {step} configured: {refused}")
    }

    /// The reason the ElevenLabs arm's LLM or recogniser is missing for, naming it.
    fn refused(step: &str, refused: &Refused) -> String {
        format!("no {step} configured: {refused}")
    }

    /// `runs` runs of each question over the recogniser `--cascade-stt` names, the LLM and the
    /// voice `--cascade-voice` names. The recogniser and the LLM are checked first, then the
    /// voice's configuration, and the ElevenLabs key last, once, for the synthesiser and the
    /// recogniser that need it.
    pub fn measure(
        plan: &Plan,
        questions: &[Question],
        args: &Args,
    ) -> Result<Vec<Measured>, String> {
        let listener = match args.cascade_stt {
            CascadeStt::Local => {
                let config = LocalConfig::from_env().map_err(|refused| missing(&refused))?;
                Listener::Local(Box::new(
                    LocalModels::open(&config).map_err(|refused| missing(&refused))?,
                ))
            }
            CascadeStt::Elevenlabs => Listener::Elevenlabs(Box::new(
                elevenlabs::generator().map_err(|error| refused("local LLM", &error))?,
            )),
        };
        match args.cascade_voice {
            CascadeVoice::Cartesia => {
                let synthesiser = cartesia()?;
                run(listener, synthesiser, None, plan, questions, args.runs)
            }
            CascadeVoice::Elevenlabs => {
                let voice = std::env::var(VOICE_VARIABLE)
                    .ok()
                    .filter(|voice| !voice.trim().is_empty())
                    .ok_or_else(|| format!("{NO_HOSTED_TTS}: {VOICE_VARIABLE} names no voice"))?;
                elevenlabs::endpoint().map_err(|refused| format!("{NO_HOSTED_TTS}: {refused}"))?;
                let key =
                    elevenlabs::key().map_err(|refused| format!("{NO_HOSTED_TTS}: {refused}"))?;
                let synthesiser = hosted(Arc::clone(&key), &voice)?;
                run(listener, synthesiser, Some(key), plan, questions, args.runs)
            }
        }
    }

    /// The runs over `listener` and `synthesiser`; `key` is the ElevenLabs key when it was
    /// resolved for the synthesiser, and resolved here when the recogniser needs it and it was
    /// not.
    fn run<S>(
        listener: Listener,
        synthesiser: S,
        key: Option<Arc<dyn CredentialSource>>,
        plan: &Plan,
        questions: &[Question],
        runs: usize,
    ) -> Result<Vec<Measured>, String>
    where
        S: Synthesiser,
        S::Handle: Send,
    {
        match listener {
            Listener::Local(models) => {
                let mut cascade = Cascade::new(
                    models.recogniser,
                    models.generator,
                    synthesiser,
                    pcm16(16_000),
                );
                cascade.attack = SPEECH_VAD.attack;
                cascade.hold = SPEECH_VAD.hold;
                Ok(measure_cascade(
                    "cascade",
                    &mut cascade,
                    plan,
                    questions,
                    runs,
                    |_| None,
                ))
            }
            Listener::Elevenlabs(generator) => {
                let endpoint =
                    elevenlabs::endpoint().map_err(|error| refused("ElevenLabs STT", &error))?;
                let source = match key {
                    Some(key) => key,
                    None => elevenlabs::key().map_err(|error| refused("ElevenLabs STT", &error))?,
                };
                let mut cascade = elevenlabs_arm(endpoint, source, *generator, synthesiser)
                    .map_err(|error| format!("no ElevenLabs STT configured: {error}"))?;
                Ok(measure_cascade(
                    "cascade",
                    &mut cascade,
                    plan,
                    questions,
                    runs,
                    elevenlabs::failure,
                ))
            }
        }
    }

    /// The cascade arm's synthesiser: ElevenLabs in `pcm_24000` at the host
    /// `ELEVENLABS_RESIDENCY` names, in the voice `voice`, with the key `elevenlabs::key`
    /// resolved, `ELEVENLABS_API_KEY` or the OS keychain's `elevenlabs-api-key`.
    fn hosted(
        source: Arc<dyn CredentialSource>,
        voice: &str,
    ) -> Result<ElevenLabsSynthesiser, String> {
        let endpoint =
            elevenlabs::endpoint().map_err(|refused| format!("{NO_HOSTED_TTS}: {refused}"))?;
        elevenlabs::synthesiser(endpoint, source, Some(voice))
    }

    /// The cascade arm's synthesiser with `--cascade-voice cartesia`: Cartesia Sonic in 16-bit
    /// PCM at 24 kHz, its key from `CARTESIA_API_KEY` and its voice from `CARTESIA_VOICE_ID`.
    fn cartesia() -> Result<inference_cartesia::CartesiaSynthesiser, String> {
        if std::env::var_os(inference_cartesia::CARTESIA_KEY_VARIABLE).is_none() {
            return Err(format!("{NO_HOSTED_TTS}: CARTESIA_API_KEY is not set"));
        }
        let voice = std::env::var("CARTESIA_VOICE_ID")
            .ok()
            .filter(|voice| !voice.trim().is_empty())
            .ok_or_else(|| format!("{NO_HOSTED_TTS}: CARTESIA_VOICE_ID names no voice"))?;
        inference_cartesia::CartesiaSynthesiser::from_env(
            inference_cartesia::Endpoint::new(),
            inference_cartesia::VoiceConfig::new(
                voice.trim(),
                inference_cartesia::OutputFormat::PCM_S16LE_24000,
            ),
        )
        .map_err(|error| format!("{NO_HOSTED_TTS}: {error}"))
    }
}

/// Without feature `local`, the cascade arm has no local LLM, nor a local recogniser, and is
/// skipped before anything else of it is checked.
#[cfg(not(feature = "local"))]
mod cascade {
    use realtime_voice::compare::{Measured, Plan, Question};

    use super::Args;

    /// Skips the arm, naming the missing steps.
    pub fn measure(
        _plan: &Plan,
        _questions: &[Question],
        _args: &Args,
    ) -> Result<Vec<Measured>, String> {
        Err(
            "no local STT or LLM configured: compare was built without feature local, \
             which binds the local voice cascade's recogniser and LLM"
                .to_owned(),
        )
    }
}

/// The reason `--skip` gives for `provider`.
fn skipped<'a>(skips: &'a [String], provider: &str) -> Option<&'a str> {
    skips.iter().find_map(|skip| {
        let (name, reason) = skip.split_once('=').unwrap_or((skip, "skipped by --skip"));
        (name == provider).then_some(reason)
    })
}

/// Where the OpenAI key is read: the keychain, or the environment.
fn openai_key() -> Result<Box<dyn CredentialSource>, Box<dyn Error>> {
    Ok(
        if std::env::var(KEY_SOURCE_VARIABLE).as_deref() == Ok("env") {
            Box::new(EnvironmentSource::openai())
        } else {
            Box::new(KeychainSource::native()?)
        },
    )
}

/// The ElevenLabs agent the environment names: a signed URL, else a public agent id.
fn elevenlabs_agent() -> Result<ElevenLabsAgent, Box<dyn Error>> {
    if let Ok(url) = std::env::var("ELEVENLABS_AGENT_SIGNED_URL") {
        return Ok(ElevenLabsAgent::signed(
            SignedUrl::new(url)?,
            AgentEndpoint::new(),
        ));
    }
    let id = std::env::var("ELEVENLABS_AGENT_ID")
        .map_err(|_| "neither ELEVENLABS_AGENT_ID nor ELEVENLABS_AGENT_SIGNED_URL is set")?;
    Ok(ElevenLabsAgent::public(id, AgentEndpoint::new())?)
}

/// `runs` runs of each question over `model`; an error when the first session does not open.
fn measure<M: NativeSpeechModel>(
    name: &str,
    model: &mut M,
    config: fn() -> SessionConfig,
    plan: &Plan,
    questions: &[Question],
    runs: usize,
    billing: &Billing<M::Session>,
) -> Result<Vec<Measured>, Box<dyn Error>> {
    measure_arm(name, plan, questions, runs, |plan| {
        realtime_voice::compare::run(model, config(), plan, &Instant::now, billing)
            .map_err(|error| format!("the session did not open: {error}"))
    })
    .map_err(Into::into)
}
