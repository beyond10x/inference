//! Talk to a realtime model from a laptop: the default microphone, a native speech model and the
//! default output, with barge-in and the local time as a tool. Use a headset: a laptop's own
//! speaker feeds back into its microphone, and the model interrupts itself.
//!
//! ```text
//! cargo run -p realtime-voice --example talk -- --provider openai
//! cargo run -p realtime-voice --example talk -- --provider openai --prices prices.json
//! INFERENCE_OPENAI_KEY_SOURCE=env OPENAI_API_KEY=... cargo run -p realtime-voice --example talk -- --provider openai
//! AWS_REGION=us-east-1 cargo run -p realtime-voice --example talk -- --provider nova
//! ```
//!
//! OpenAI reads its key from the OS keychain, secret `openai-api-key`, or with
//! `INFERENCE_OPENAI_KEY_SOURCE=env` from `OPENAI_API_KEY`; `INFERENCE_OPENAI_REALTIME_MODEL`
//! names another model. Nova Sonic takes its credentials from the AWS default chain and its region
//! from `AWS_REGION`; `INFERENCE_NOVA_SONIC_MODEL` names another model. The key is never printed.
//!
//! Speak, interrupt the model mid-answer, ask for the time. Enter ends the conversation and prints
//! the session's usage totals and its duration; Ctrl-C ends it without them.
//!
//! Each response's usage is printed when it ends, `?` for a count the provider did not report.
//! With `--prices <file>`, a price table in the JSON form `realtime_voice::Prices` reads, the
//! estimated cost is printed with each response and in total, with the table's name and date.
//! No prices are built in.

use std::error::Error;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use clap::{Parser, ValueEnum};
use inference_audio_device::CpalHost;
use inference_aws::duplex::NovaSonicSpeech;
use inference_duplex::{NativeSpeechModel, ResponseId, SessionConfig};
use inference_openai::duplex::OpenAiRealtime;
use inference_openai::{
    CredentialSource, Endpoint, EnvironmentSource, KeychainSource, OPENAI_KEY_NAME,
};
use realtime_voice::{
    Conversation, Ending, Note, Options, Prices, Reported, Tally, Usage, local_time, nova_config,
    openai_config,
};

/// The variable that switches the OpenAI key from the keychain to `OPENAI_API_KEY`.
const KEY_SOURCE_VARIABLE: &str = "INFERENCE_OPENAI_KEY_SOURCE";

/// Talk to a realtime speech model through the default microphone and output.
#[derive(Debug, Parser)]
struct Args {
    /// The provider to talk to.
    #[arg(long, value_enum)]
    provider: Provider,
    /// A JSON price table, per million tokens, to estimate the cost of each response with.
    #[arg(long, value_name = "FILE")]
    prices: Option<PathBuf>,
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
    let outcome = prices(args.prices.as_ref()).and_then(|prices| {
        let host = CpalHost::new();
        match args.provider {
            Provider::Openai => openai().and_then(|mut model| {
                talk(
                    &mut model,
                    openai_config(),
                    &host,
                    Report::new(Reported::OPENAI, prices),
                )
            }),
            Provider::Nova => NovaSonicSpeech::from_env()
                .map_err(Box::<dyn Error>::from)
                .and_then(|mut model| {
                    talk(
                        &mut model,
                        nova_config(),
                        &host,
                        Report::new(Reported::NOVA, prices),
                    )
                }),
        }
    });
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The price table at `path`, when one is named.
fn prices(path: Option<&PathBuf>) -> Result<Option<Prices>, Box<dyn Error>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("the price table {} did not read: {error}", path.display()))?;
    let prices = Prices::parse(&text)
        .map_err(|error| format!("the price table {}: {error}", path.display()))?;
    Ok(Some(prices))
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

/// Holds the conversation until the session ends or Enter is pressed, then prints the totals.
fn talk<M: NativeSpeechModel>(
    model: &mut M,
    config: SessionConfig,
    host: &CpalHost,
    mut report: Report,
) -> Result<(), Box<dyn Error>> {
    let mut conversation = Conversation::open(
        model,
        config,
        host,
        Options::default(),
        Box::new(local_time),
    )?;
    let started = Instant::now();
    let enter = enter_pressed();
    println!(
        "listening on {:?}, playing on {:?}. Speak; interrupt; ask for the time. Enter ends.",
        conversation.microphone().device().name,
        conversation.speaker().device().name
    );
    if let Some(prices) = &report.prices {
        println!("cost estimated by {prices}, in {}", prices.currency);
    }
    while conversation.is_open() {
        if enter.load(Ordering::Relaxed) {
            for note in conversation.close() {
                report.note(&note);
            }
            break;
        }
        for note in conversation.step() {
            report.note(&note);
        }
    }
    report.finish_line();
    report.totals(started.elapsed());
    println!("capture: {:?}", conversation.microphone().stats());
    println!("playout: {:?}", conversation.speaker().stats());
    Ok(())
}

/// A flag that turns on when a line is read from the terminal.
fn enter_pressed() -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    let set = Arc::clone(&flag);
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::stdin().lock().read_line(&mut line);
        set.store(true, Ordering::Relaxed);
    });
    flag
}

/// What the terminal shows: the transcripts, the events, and the usage.
struct Report {
    tally: Tally,
    prices: Option<Prices>,
    speaking: Option<ResponseId>,
}

impl Report {
    const fn new(reported: Reported, prices: Option<Prices>) -> Self {
        Self {
            tally: Tally::new(reported),
            prices,
            speaking: None,
        }
    }

    /// Prints one note; a response's transcript continues on one line.
    fn note(&mut self, note: &Note) {
        self.tally.record(note);
        match note {
            Note::Said { response_id, text } => {
                if self.speaking != Some(*response_id) {
                    self.finish_line();
                    print!("model: ");
                    self.speaking = Some(*response_id);
                }
                print!("{text}");
                let _ = std::io::stdout().flush();
            }
            Note::Heard(text) => {
                self.finish_line();
                println!("you:   {text}");
            }
            Note::Ended { response_id, how } => {
                self.finish_line();
                if let Ending::Failed(failure) = how {
                    println!("[the response failed: {failure}]");
                }
                self.usage_line(*response_id, "usage");
            }
            Note::Usage(Usage::Response { response_id, .. }) => {
                if self.tally.ended(*response_id) {
                    self.finish_line();
                    self.usage_line(*response_id, "usage, updated");
                }
            }
            Note::Usage(Usage::Transcription(_)) => {}
            Note::BargeIn { heard, .. } => {
                self.finish_line();
                match heard {
                    Some(heard) => {
                        println!("[barge-in: playout stopped at sample {}]", heard.get())
                    }
                    None => println!("[barge-in: playout stopped]"),
                }
            }
            Note::Truncated { heard, .. } => {
                println!("[the response is cut at sample {}]", heard.get());
            }
            Note::Answered { result, .. } => {
                self.finish_line();
                println!("[local_time: {result}]");
            }
            Note::Refused(detail) => {
                self.finish_line();
                println!("[refused: {detail}]");
            }
            Note::Closed => {
                self.finish_line();
                println!("[the session closed]");
            }
            Note::Failed(failure) => {
                self.finish_line();
                println!("[{failure}]");
            }
        }
    }

    /// One compact line of a response's usage, and its cost with a price table.
    fn usage_line(&self, response_id: ResponseId, label: &str) {
        let counts = self.tally.response(response_id).unwrap_or_default();
        match &self.prices {
            Some(prices) => {
                let cost = prices
                    .cost(&counts)
                    .map_or_else(|| "?".to_owned(), |cost| cost.to_string());
                println!("[{label}: {counts}; cost {cost} {}]", prices.currency);
            }
            None => println!("[{label}: {counts}]"),
        }
    }

    /// The session's totals, its wall duration, and its cost with a price table.
    fn totals(&self, wall: Duration) {
        println!(
            "session: {:.1} s; {}",
            wall.as_secs_f64(),
            self.tally.totals()
        );
        if let Some(prices) = &self.prices {
            let cost = self
                .tally
                .cost(prices)
                .map_or_else(|| "?".to_owned(), |cost| cost.to_string());
            println!(
                "session cost: {cost} {} by {prices}, responses that reported usage only, \
                 transcription not priced",
                prices.currency
            );
        }
    }

    fn finish_line(&mut self) {
        if self.speaking.take().is_some() {
            println!();
        }
    }
}
