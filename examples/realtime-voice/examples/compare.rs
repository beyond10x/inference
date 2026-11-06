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
//! ```
//!
//! The question is the OpenAI adapter's speech fixture ("What is the capital of France?"). The
//! barge-in utterance is the 16-bit PCM WAV file `INFERENCE_COMPARE_BARGE_IN_WAV` names; without
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
//! A provider whose model cannot be made or whose first session does not open is
//! skipped with the reason; `--skip <provider>=<reason>` skips one without trying it.
//!
//! The cascade arm, `cascade`, speaks with ElevenLabs when its key resolves from
//! `ELEVENLABS_API_KEY` and `ELEVENLABS_VOICE_ID` names a voice, in `pcm_24000`; otherwise it is
//! skipped as "no hosted TTS configured". Its recogniser and LLM are those the local voice
//! cascade configures (`local_voice_cascade::local::LocalConfig::from_env`), bound with feature
//! `local`, with that cascade's detector; a missing one skips the arm, naming it.

use std::error::Error;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::{Parser, ValueEnum};
use inference_aws::duplex::NovaSonicSpeech;
use inference_deepgram::agent::AgentConfig;
use inference_deepgram::{AgentEndpoint as DeepgramEndpoint, DeepgramAgent};
use inference_duplex::{NativeSpeechModel, SessionConfig};
use inference_elevenlabs::agent::{AgentEndpoint, SignedUrl};
use inference_elevenlabs::{ElevenLabsAgent, ElevenLabsSynthesiser};
use inference_openai::duplex::OpenAiRealtime;
use inference_openai::live_duplex::{OpenAiLive, OpenAiLiveSession};
use inference_openai::{
    CredentialSource, Endpoint, EnvironmentSource, KeychainSource, OPENAI_KEY_NAME,
};
use inference_phonic::{PhonicConfig, PhonicEndpoint, PhonicSpeech};
use realtime_voice::compare::{
    Billing, Clip, Comparison, Measured, Plan, Topology, VOICE_VARIABLE, hosted_tts,
};
use realtime_voice::{
    Reported, deepgram_config, elevenlabs_config, nova_config, openai_config, openai_live_config,
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
        default_value = "openai,openai-live,nova,elevenlabs,deepgram,phonic,cascade"
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
    /// The local voice cascade's recogniser and LLM, speaking with ElevenLabs.
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
            | Self::Phonic => Topology::Native,
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
/// voice.
fn phonic_config() -> SessionConfig {
    SessionConfig {
        voice: String::new(),
        instructions: realtime_voice::INSTRUCTIONS.to_owned(),
        ..realtime_voice::elevenlabs_config()
    }
}

fn main() -> ExitCode {
    let args = Args::parse();
    match compare(&args) {
        Ok(comparison) => {
            print!("{comparison}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn compare(args: &Args) -> Result<Comparison, Box<dyn Error>> {
    let mut plan = Plan::new(Clip::from_wav(QUESTION)?);
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
                match hosted()
                    .and_then(|synthesiser| cascade::measure(synthesiser, &plan, args.runs))
                {
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
                        args.runs,
                        &Billing::tokens(PHONIC_REPORTED),
                    )
                }),
        };
        match outcome {
            Ok(runs) => comparison.ran_as(name, provider.topology(), runs),
            Err(error) => comparison.skipped_as(name, provider.topology(), &error.to_string()),
        }
    }
    Ok(comparison)
}

/// The cascade arm's synthesiser: ElevenLabs in `pcm_24000`, its key from `ELEVENLABS_API_KEY`
/// and its voice from [`VOICE_VARIABLE`].
fn hosted() -> Result<ElevenLabsSynthesiser, String> {
    hosted_tts(
        inference_elevenlabs::Endpoint::new(),
        Arc::new(inference_elevenlabs::EnvironmentSource::elevenlabs()),
        std::env::var(VOICE_VARIABLE).ok().as_deref(),
        inference_elevenlabs::OutputFormat::Pcm24000,
    )
}

/// The cascade arm over the local voice cascade's recogniser and LLM.
#[cfg(feature = "local")]
mod cascade {
    use inference_elevenlabs::ElevenLabsSynthesiser;
    use local_voice_cascade::local::{
        LocalConfig, LocalModels, LocalRefused, SERVER_MODEL_VARIABLE, SERVER_URL_VARIABLE,
        SPEECH_VAD,
    };
    use realtime_voice::compare::{Cascade, ClientPlayout, Measured, Plan, run_cascade_live};
    use realtime_voice::pcm16;

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

    /// `runs` runs of the plan over the local recogniser and LLM and `synthesiser`.
    pub fn measure(
        synthesiser: ElevenLabsSynthesiser,
        plan: &Plan,
        runs: usize,
    ) -> Result<Vec<Measured>, String> {
        let config = LocalConfig::from_env().map_err(|refused| missing(&refused))?;
        let models = LocalModels::open(&config).map_err(|refused| missing(&refused))?;
        let mut cascade = Cascade::new(
            models.recogniser,
            models.generator,
            synthesiser,
            pcm16(16_000),
        );
        cascade.attack = SPEECH_VAD.attack;
        cascade.hold = SPEECH_VAD.hold;
        Ok((1..=runs)
            .map(|run| {
                eprintln!("cascade: run {run} of {runs}");
                run_cascade_live(&mut cascade, plan, &mut ClientPlayout)
            })
            .collect())
    }
}

/// Without feature `local`, the cascade arm has no local recogniser or LLM.
#[cfg(not(feature = "local"))]
mod cascade {
    use inference_elevenlabs::ElevenLabsSynthesiser;
    use realtime_voice::compare::{Measured, Plan};

    /// Skips the arm, naming the missing steps.
    pub fn measure(
        _synthesiser: ElevenLabsSynthesiser,
        _plan: &Plan,
        _runs: usize,
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

/// `runs` runs of the plan over `model`; an error when the first session does not open.
fn measure<M: NativeSpeechModel>(
    name: &str,
    model: &mut M,
    config: fn() -> SessionConfig,
    plan: &Plan,
    runs: usize,
    billing: &Billing<M::Session>,
) -> Result<Vec<Measured>, Box<dyn Error>> {
    let mut measured = Vec::with_capacity(runs);
    for run in 1..=runs {
        eprintln!("{name}: run {run} of {runs}");
        match realtime_voice::compare::run(model, config(), plan, &Instant::now, billing) {
            Ok(outcome) => measured.push(outcome),
            Err(error) if measured.is_empty() => {
                return Err(format!("the session did not open: {error}").into());
            }
            Err(error) => measured.push(Measured {
                notes: vec![format!("the session did not open: {error}")],
                ..Measured::default()
            }),
        }
    }
    Ok(measured)
}
