//! Compare native speech providers on the same recorded utterances, without a microphone or a
//! speaker: latency from the end of the question's speech to the answer's first audio, first
//! transcript and end, barge-in to output stop, and the usage of each run. Every run is billed.
//!
//! ```text
//! cargo run -p realtime-voice --example compare -- --providers openai,openai-live --runs 2
//! INFERENCE_COMPARE_BARGE_IN_WAV=<a WAV file of speech> cargo run -p realtime-voice --example compare -- --runs 2
//! cargo run -p realtime-voice --example compare -- --skip nova="no model access"
//! ```
//!
//! The question is the OpenAI adapter's speech fixture ("What is the capital of France?"). The
//! barge-in utterance is the 16-bit PCM WAV file `INFERENCE_COMPARE_BARGE_IN_WAV` names; without
//! it no barge-in is sent and that column stays empty. `realtime_voice::compare` says how each
//! figure is taken: on the client, at the receipt of each duplex event.
//!
//! Each provider runs with the session configuration `talk` uses and its key or credentials as
//! `talk` reads them. A provider whose model cannot be made or whose first session does not open
//! is skipped with the reason; `--skip <provider>=<reason>` skips one without trying it. The
//! local cascade is not a native speech model and is listed as skipped.

use std::error::Error;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::{Parser, ValueEnum};
use inference_aws::duplex::NovaSonicSpeech;
use inference_duplex::{NativeSpeechModel, SessionConfig};
use inference_openai::duplex::OpenAiRealtime;
use inference_openai::live_duplex::{OpenAiLive, OpenAiLiveSession};
use inference_openai::{
    CredentialSource, Endpoint, EnvironmentSource, KeychainSource, OPENAI_KEY_NAME,
};
use realtime_voice::compare::{Billing, Clip, Comparison, Measured, Plan};
use realtime_voice::{Reported, nova_config, openai_config, openai_live_config};

const QUESTION: &[u8] =
    include_bytes!("../../../adapters/inference-openai/tests/fixtures/speech.wav");

/// The variable naming the barge-in utterance.
const BARGE_IN_VARIABLE: &str = "INFERENCE_COMPARE_BARGE_IN_WAV";

/// The variable that switches the OpenAI key from the keychain to `OPENAI_API_KEY`.
const KEY_SOURCE_VARIABLE: &str = "INFERENCE_OPENAI_KEY_SOURCE";

/// Why the local cascade is not in the comparison.
const LOCAL_CASCADE: &str = "a pipeline of STT, LLM and TTS, not a native speech model; \
     the local-voice-cascade example measures it";

/// Compare native speech providers on recorded utterances.
#[derive(Debug, Parser)]
struct Args {
    /// The providers to run, in order.
    #[arg(
        long,
        value_enum,
        value_delimiter = ',',
        default_value = "openai,openai-live,nova"
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
}

impl Provider {
    const fn name(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::OpenaiLive => "openai-live",
            Self::Nova => "nova",
        }
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
            comparison.skipped(name, reason);
            continue;
        }
        let outcome = match provider {
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
        };
        match outcome {
            Ok(runs) => comparison.ran(name, runs),
            Err(error) => comparison.skipped(name, &error.to_string()),
        }
    }
    comparison.skipped("local cascade", LOCAL_CASCADE);
    Ok(comparison)
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
