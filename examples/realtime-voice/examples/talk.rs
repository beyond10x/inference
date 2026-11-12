//! Talk to a realtime model from a laptop: the default microphone, a native speech model or a
//! cascade, and the default output, with barge-in. Use a headset: a laptop's own speaker feeds
//! back into its microphone, and the model interrupts itself.
//!
//! ```text
//! cargo run -p realtime-voice --example talk -- --provider openai
//! cargo run -p realtime-voice --example talk -- --provider openai --prices prices.json
//! INFERENCE_OPENAI_KEY_SOURCE=env OPENAI_API_KEY=... cargo run -p realtime-voice --example talk -- --provider openai
//! cargo run -p realtime-voice --example talk -- --provider openai-live
//! AWS_REGION=us-east-1 cargo run -p realtime-voice --example talk -- --provider nova
//! cargo run -p realtime-voice --features local --example talk -- --provider elevenlabs-cascade
//! ```
//!
//! The native providers are configuration only: each has its session configuration and binding,
//! and the conversation is the same for all, with the local time as a tool.
//!
//! OpenAI, both the Realtime API (`openai`) and GPT-Live (`openai-live`), reads its key from the
//! OS keychain, secret `openai-api-key`, or with `INFERENCE_OPENAI_KEY_SOURCE=env` from
//! `OPENAI_API_KEY`; `INFERENCE_OPENAI_REALTIME_MODEL` names another Realtime model. GPT-Live
//! decides itself when to speak and when it is interrupted, so the conversation never cancels or
//! truncates it; it bills the voice in seconds, printed with the totals. Nova Sonic takes its
//! credentials from the AWS default chain and its region from `AWS_REGION`;
//! `INFERENCE_NOVA_SONIC_MODEL` names another model. The key is never printed.
//!
//! `elevenlabs-cascade`, built with feature `local`, is a cascade: ElevenLabs realtime speech to
//! text (`pcm_16000` from the microphone, the endpoint's voice activity detection committing each
//! turn), the `llama-server` on loopback `INFERENCE_LLAMA_SERVER_URL` and
//! `INFERENCE_LLAMA_SERVER_MODEL` name, as the local voice cascade reads them, and ElevenLabs
//! text to speech in the voice `ELEVENLABS_VOICE_ID` names (`pcm_24000`). Both ElevenLabs
//! endpoints are at the host `ELEVENLABS_RESIDENCY` names, global when unset; the key is
//! `ELEVENLABS_API_KEY`, or the OS keychain's `elevenlabs-api-key` when that is unset.
//! Speaking over an answer stops its playback and drops the rest of it
//! (`realtime_voice::talk::converse`); ElevenLabs still generates the segment it was speaking.
//! It has no tool and no usage or cost. A variable it needs that is not set exits with status 2,
//! naming it.
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

use clap::Parser;
use clap::builder::{PossibleValue, PossibleValuesParser, TypedValueParser};
use inference_audio_device::CpalHost;
use inference_aws::duplex::NovaSonicSpeech;
use inference_duplex::{NativeSpeechModel, ResponseId, SessionConfig};
use inference_openai::duplex::OpenAiRealtime;
use inference_openai::live_duplex::{OpenAiLive, OpenAiLiveSession};
use inference_openai::{
    CredentialSource, Endpoint, EnvironmentSource, KeychainSource, OPENAI_KEY_NAME,
};
use realtime_voice::talk::{Provider, Refused};
use realtime_voice::{
    Conversation, Ending, Note, Options, Prices, Reported, Tally, Usage, local_time, nova_config,
    openai_config, openai_live_config,
};

/// The variable that switches the OpenAI key from the keychain to `OPENAI_API_KEY`.
const KEY_SOURCE_VARIABLE: &str = "INFERENCE_OPENAI_KEY_SOURCE";

/// Talk to a realtime speech model through the default microphone and output.
#[derive(Debug, Parser)]
struct Args {
    /// The provider to talk to.
    #[arg(long, value_parser = providers())]
    provider: Provider,
    /// A JSON price table, per million tokens, to estimate the cost of each response with.
    #[arg(long, value_name = "FILE")]
    prices: Option<PathBuf>,
}

/// The values `--provider` takes: every `realtime_voice::talk::Provider`, by name.
fn providers() -> impl TypedValueParser<Value = Provider> {
    PossibleValuesParser::new(
        Provider::ALL.map(|provider| PossibleValue::new(provider.name()).help(provider.about())),
    )
    .try_map(|name| Provider::named(&name).ok_or("no such provider"))
}

fn main() -> ExitCode {
    let args = Args::parse();
    let outcome = args
        .provider
        .required(|name| std::env::var(name).ok())
        .map_err(Box::<dyn Error>::from)
        .and_then(|()| prices(args.prices.as_ref()))
        .and_then(|prices| {
            let host = CpalHost::new();
            match args.provider {
                Provider::Openai => openai_key().and_then(|source| {
                    let mut model =
                        OpenAiRealtime::new(Endpoint::from_env(), source, OPENAI_KEY_NAME);
                    talk(
                        &mut model,
                        openai_config(),
                        &host,
                        Report::new(Reported::OPENAI, prices),
                        |_| None,
                    )
                }),
                Provider::OpenaiLive => openai_key().and_then(|source| {
                    let mut model = OpenAiLive::hosted(source, OPENAI_KEY_NAME);
                    talk(
                        &mut model,
                        openai_live_config(),
                        &host,
                        Report::new(Reported::OPENAI_LIVE, prices),
                        OpenAiLiveSession::voice_seconds,
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
                            |_| None,
                        )
                    }),
                Provider::ElevenlabsCascade => cascade::talk(&host),
            }
        });
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            error
                .downcast_ref::<Refused>()
                .map_or(ExitCode::FAILURE, |refused| {
                    ExitCode::from(refused.exit_code())
                })
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

/// Holds the conversation until the session ends or Enter is pressed, then prints the totals
/// and, for a provider that bills by the second, the `seconds` it billed.
fn talk<M: NativeSpeechModel>(
    model: &mut M,
    config: SessionConfig,
    host: &CpalHost,
    mut report: Report,
    seconds: fn(&M::Session) -> Option<f64>,
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
    if let Some(seconds) = seconds(conversation.session()) {
        println!("billed: {seconds:.1} s of voice");
    }
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

/// `elevenlabs-cascade`: ElevenLabs speech to text, the local LLM, ElevenLabs text to speech.
#[cfg(feature = "local")]
mod cascade {
    use std::error::Error;
    use std::io::Write;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    use inference_audio_device::{
        CaptureConfig, CpalHost, DeviceSelector, Microphone, PlayoutConfig, Speaker,
    };
    use inference_elevenlabs::{
        CredentialSource, ELEVENLABS_KEY_NAME, ELEVENLABS_KEY_VARIABLE, ElevenLabsRecognition,
        EnvironmentSource, KeychainSource,
    };
    use inference_llama_server::{Config, LlamaServer};
    use inference_llm::Generator;
    use local_voice_cascade::local::{
        MAX_OUTPUT_TOKENS, SERVER_MODEL_VARIABLE, SERVER_URL_VARIABLE, loopback, server_adapter,
    };
    use realtime_voice::compare::{Cascade, Playout, VOICE_VARIABLE};
    use realtime_voice::talk::{CascadeNote, Refused, converse};
    use realtime_voice::{elevenlabs, pcm16};

    /// How much answer audio the speaker queues: an answer arrives faster than it plays.
    const PLAYOUT_BUFFER: Duration = Duration::from_secs(120);

    /// How long one read of the microphone waits for a frame.
    const PATIENCE: Duration = Duration::from_millis(10);

    /// Holds the conversation until Enter is pressed or the recognition ends.
    pub fn talk(host: &CpalHost) -> Result<(), Box<dyn Error>> {
        let endpoint = elevenlabs::endpoint()?;
        let source = key()?;
        let recogniser = elevenlabs::recogniser(endpoint.clone(), Arc::clone(&source))?;
        let generator = generator()?;
        let voice = std::env::var(VOICE_VARIABLE).ok();
        let synthesiser = elevenlabs::synthesiser(endpoint, source, voice.as_deref())?;
        let mut cascade = Cascade::new(recogniser, generator, synthesiser, pcm16(16_000));
        let format = cascade.input_format;
        let mut microphone = Microphone::open(
            host,
            &DeviceSelector::Default,
            CaptureConfig {
                sample_rate: format.sample_rate,
                encoding: format.encoding,
                ..CaptureConfig::default()
            },
        )?;
        let speaker = Speaker::open(
            host,
            &DeviceSelector::Default,
            PlayoutConfig {
                buffer: PLAYOUT_BUFFER,
            },
        )?;
        let started = Instant::now();
        let enter = super::enter_pressed();
        println!(
            "listening on {:?}, playing on {:?}. Speak; speak over the answer to stop it. Enter \
             ends.",
            microphone.device().name,
            speaker.device().name
        );
        let mut playout = SpeakerPlayout(&speaker);
        let mut speaking = false;
        converse(
            &mut cascade,
            &mut || microphone.next_frame(PATIENCE),
            &mut playout,
            &|| !enter.load(Ordering::Relaxed),
            |recognition: &ElevenLabsRecognition| recognition.failure().map(ToString::to_string),
            &mut |note| print(&note, &mut speaking),
        );
        if speaking {
            println!();
        }
        println!("session: {:.1} s", started.elapsed().as_secs_f64());
        println!("capture: {:?}", microphone.stats());
        println!("playout: {:?}", speaker.stats());
        Ok(())
    }

    /// The ElevenLabs key: `ELEVENLABS_API_KEY` when it is set, the OS keychain's
    /// `elevenlabs-api-key` otherwise, resolved once here so a missing key stops before anything
    /// opens. The key is never printed.
    fn key() -> Result<Arc<dyn CredentialSource>, Box<dyn Error>> {
        let source: Arc<dyn CredentialSource> =
            if std::env::var_os(ELEVENLABS_KEY_VARIABLE).is_some() {
                Arc::new(EnvironmentSource::elevenlabs())
            } else {
                Arc::new(KeychainSource::native()?)
            };
        elevenlabs::resolves(&*source).map_err(|why| {
            Refused::Invalid(format!(
                "{ELEVENLABS_KEY_VARIABLE} is not set and the keychain gave no \
                 {ELEVENLABS_KEY_NAME}: {why}"
            ))
        })?;
        Ok(source)
    }

    /// The generator of the `llama-server` the local voice cascade's variables name, refused
    /// before anything is sent when its URL is not loopback.
    fn generator() -> Result<Generator, Box<dyn Error>> {
        let read = |name: &'static str| std::env::var(name).map_err(|_| Refused::Missing(name));
        let url = read(SERVER_URL_VARIABLE)?;
        loopback(&url)?;
        let mut server = Config::new(url, read(SERVER_MODEL_VARIABLE)?);
        server.max_output_tokens = MAX_OUTPUT_TOKENS;
        let adapter = server_adapter(&server)?;
        Ok(LlamaServer::new(&server)?.generator(adapter)?)
    }

    /// Prints one note; an answer continues on one line.
    fn print(note: &CascadeNote, speaking: &mut bool) {
        let end_line = |speaking: &mut bool| {
            if std::mem::take(speaking) {
                println!();
            }
        };
        match note {
            CascadeNote::Heard(text) => {
                end_line(speaking);
                println!("you:   {text}");
            }
            CascadeNote::Said(text) => {
                if !*speaking {
                    print!("model: ");
                    *speaking = true;
                }
                print!("{text}");
                let _ = std::io::stdout().flush();
            }
            CascadeNote::Answered => end_line(speaking),
            CascadeNote::BargeIn => {
                end_line(speaking);
                println!("[barge-in: playout stopped]");
            }
            CascadeNote::Refused(detail) => {
                end_line(speaking);
                println!("[{detail}]");
            }
            CascadeNote::Ended => {
                end_line(speaking);
                println!("[the recognition ended]");
            }
        }
    }

    /// The speaker as the cascade's playout.
    struct SpeakerPlayout<'a>(&'a Speaker);

    impl Playout for SpeakerPlayout<'_> {
        fn play(&mut self, frame: &inference_duplex::inference_audio::AudioFrame) {
            if let Err(error) = self.0.try_push(frame) {
                println!("[output audio: {error}]");
            }
        }

        fn stop(&mut self) {
            let _ = self.0.stop();
        }
    }
}

/// Without feature `local` the cascade has no LLM: `Provider::required` refuses it first.
#[cfg(not(feature = "local"))]
mod cascade {
    use std::error::Error;

    use inference_audio_device::CpalHost;
    use realtime_voice::talk::Refused;

    pub fn talk(_host: &CpalHost) -> Result<(), Box<dyn Error>> {
        Err(Refused::Unbuilt { feature: "local" }.into())
    }
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
