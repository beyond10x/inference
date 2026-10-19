//! Talk to a realtime model from a laptop: the default microphone, a native speech model and the
//! default output, with barge-in and the local time as a tool. Use a headset: a laptop's own
//! speaker feeds back into its microphone, and the model interrupts itself.
//!
//! ```text
//! cargo run -p realtime-voice --example talk -- --provider openai
//! INFERENCE_OPENAI_KEY_SOURCE=env OPENAI_API_KEY=... cargo run -p realtime-voice --example talk -- --provider openai
//! AWS_REGION=us-east-1 cargo run -p realtime-voice --example talk -- --provider nova
//! ```
//!
//! OpenAI reads its key from the OS keychain, secret `openai-api-key`, or with
//! `INFERENCE_OPENAI_KEY_SOURCE=env` from `OPENAI_API_KEY`; `INFERENCE_OPENAI_REALTIME_MODEL`
//! names another model. Nova Sonic takes its credentials from the AWS default chain and its region
//! from `AWS_REGION`; `INFERENCE_NOVA_SONIC_MODEL` names another model. The key is never printed.
//!
//! Speak, interrupt the model mid-answer, ask for the time. Ctrl-C ends the conversation.

use std::error::Error;
use std::io::Write;
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use inference_audio_device::CpalHost;
use inference_aws::duplex::NovaSonicSpeech;
use inference_duplex::{NativeSpeechModel, ResponseId, SessionConfig};
use inference_openai::duplex::OpenAiRealtime;
use inference_openai::{
    CredentialSource, Endpoint, EnvironmentSource, KeychainSource, OPENAI_KEY_NAME,
};
use realtime_voice::{Conversation, Ending, Note, Options, local_time, nova_config, openai_config};

/// The variable that switches the OpenAI key from the keychain to `OPENAI_API_KEY`.
const KEY_SOURCE_VARIABLE: &str = "INFERENCE_OPENAI_KEY_SOURCE";

/// Talk to a realtime speech model through the default microphone and output.
#[derive(Debug, Parser)]
struct Args {
    /// The provider to talk to.
    #[arg(long, value_enum)]
    provider: Provider,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Provider {
    /// The OpenAI Realtime API.
    Openai,
    /// Amazon Nova Sonic on Bedrock.
    Nova,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let host = CpalHost::new();
    let outcome = match args.provider {
        Provider::Openai => openai().and_then(|mut model| talk(&mut model, openai_config(), &host)),
        Provider::Nova => NovaSonicSpeech::from_env()
            .map_err(Box::<dyn Error>::from)
            .and_then(|mut model| talk(&mut model, nova_config(), &host)),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The OpenAI Realtime model, its key from the keychain or the environment.
fn openai() -> Result<OpenAiRealtime, Box<dyn Error>> {
    let source: Box<dyn CredentialSource> =
        if std::env::var(KEY_SOURCE_VARIABLE).as_deref() == Ok("env") {
            Box::new(EnvironmentSource::openai())
        } else {
            Box::new(KeychainSource::native()?)
        };
    Ok(OpenAiRealtime::new(
        Endpoint::from_env(),
        source,
        OPENAI_KEY_NAME,
    ))
}

/// Holds the conversation until the session ends.
fn talk<M: NativeSpeechModel>(
    model: &mut M,
    config: SessionConfig,
    host: &CpalHost,
) -> Result<(), Box<dyn Error>> {
    let mut conversation = Conversation::open(
        model,
        config,
        host,
        Options::default(),
        Box::new(local_time),
    )?;
    println!(
        "listening on {:?}, playing on {:?}. Speak; interrupt; ask for the time. Ctrl-C ends.",
        conversation.microphone().device().name,
        conversation.speaker().device().name
    );
    let mut speaking = None;
    while conversation.is_open() {
        for note in conversation.step() {
            report(&note, &mut speaking);
        }
    }
    finish_line(&mut speaking);
    println!("capture: {:?}", conversation.microphone().stats());
    println!("playout: {:?}", conversation.speaker().stats());
    Ok(())
}

/// Prints one note; a response's transcript continues on one line.
fn report(note: &Note, speaking: &mut Option<ResponseId>) {
    match note {
        Note::Said { response_id, text } => {
            if *speaking != Some(*response_id) {
                finish_line(speaking);
                print!("model: ");
                *speaking = Some(*response_id);
            }
            print!("{text}");
            let _ = std::io::stdout().flush();
        }
        Note::Heard(text) => {
            finish_line(speaking);
            println!("you:   {text}");
        }
        Note::Ended { how, .. } => {
            finish_line(speaking);
            if let Ending::Failed(failure) = how {
                println!("[the response failed: {failure}]");
            }
        }
        Note::BargeIn { heard, .. } => {
            finish_line(speaking);
            match heard {
                Some(heard) => println!("[barge-in: playout stopped at sample {}]", heard.get()),
                None => println!("[barge-in: playout stopped]"),
            }
        }
        Note::Truncated { heard, .. } => {
            println!("[the response is cut at sample {}]", heard.get());
        }
        Note::Answered { result, .. } => {
            finish_line(speaking);
            println!("[local_time: {result}]");
        }
        Note::Refused(detail) => {
            finish_line(speaking);
            println!("[refused: {detail}]");
        }
        Note::Closed => {
            finish_line(speaking);
            println!("[the session closed]");
        }
        Note::Failed(failure) => {
            finish_line(speaking);
            println!("[{failure}]");
        }
    }
}

fn finish_line(speaking: &mut Option<ResponseId>) {
    if speaking.take().is_some() {
        println!();
    }
}
