//! Talk to the local voice cascade from a laptop, offline: the microphone, a local recogniser,
//! sherpa-onnx's streaming one or whisper.cpp's batch one, a `llama-server` on loopback, the mock
//! synthesiser and the speaker. Use a headset: there is no echo cancellation, and a laptop's own
//! speaker feeds back into its microphone.
//!
//! ```text
//! llama-server --host 127.0.0.1 --port 8080 -m qwen2.5-1.5b-instruct-q4_k_m.gguf \
//!     --alias qwen2.5-1.5b-instruct -c 4096 -np 1 -ngl 0
//! INFERENCE_SHERPA_ONNX_STT_DIR=<checkpoint dir> \
//! INFERENCE_LLAMA_SERVER_URL=http://127.0.0.1:8080/v1 \
//! INFERENCE_LLAMA_SERVER_MODEL=qwen2.5-1.5b-instruct \
//!     cargo run -p local-voice-cascade --features local --example talk
//! ```
//!
//! `--stt whispercpp`, or `INFERENCE_CASCADE_STT=whispercpp`, binds the whisper.cpp recogniser
//! over the checkpoint file `--whispercpp-model` or `INFERENCE_WHISPERCPP_STT_MODEL` names
//! instead; it transcribes each turn once the turn completes, so its line appears after the
//! pause that ends the question. Each location is an option as well as a variable, and only the
//! selected recogniser's checkpoint is needed; `--input` and `--output` name a device, the
//! default device otherwise. A server URL whose host is not loopback is refused before anything
//! opens, and the run fails if a socket of the process connects anywhere but loopback.
//!
//! Until a local TTS adapter exists, the mock synthesiser speaks each word of the answer as 10 ms
//! of silence: the answer is printed, the speaker plays silence for it, and speaking over it stops
//! that playback. Each turn prints the committed input to first playable audio, and each
//! interruption the interruption to playback stop. Ctrl-C ends the conversation.

use std::collections::HashMap;
use std::error::Error;
use std::ffi::OsString;
use std::future::Future;
use std::io::Write;
use std::path::PathBuf;
use std::pin::pin;
use std::process::ExitCode;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use clap::Parser;
use inference_audio_device::{CaptureConfig, CpalHost, DeviceSelector, Microphone, PlayoutConfig};
use inference_core::endpoint::InputEndpoint;
use inference_core::envelope::{EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::policy::Egress;
use inference_pipeline::{Declaration, Node};
use inference_runtime::RunController;
use inference_runtime::egress::EgressWatch;
use inference_testkit::cascade::TurnStore;
use inference_testkit::graph::RunLog;
use inference_testkit::stream::StreamEvent;
use local_voice_cascade::local::{
    LocalConfig, LocalModels, LocalRefused, MAX_OUTPUT_TOKENS, SERVER_MODEL_VARIABLE,
    SERVER_URL_VARIABLE, STT_DIR_VARIABLE, STT_VARIABLE, SttBackend, SttConfig,
    WHISPERCPP_MODEL_VARIABLE, declare_local,
};
use local_voice_cascade::{
    AUDIO_INGRESS, CascadeHandles, TEXT_EGRESS, TRANSCRIPT_EGRESS, VoicePayload,
};

/// Talk to the local voice cascade through a microphone and a speaker.
#[derive(Debug, Parser)]
struct Args {
    /// The recogniser: sherpa-onnx or whispercpp.
    #[arg(long, env = STT_VARIABLE, default_value_t = SttBackend::SherpaOnnx)]
    stt: SttBackend,
    /// The directory of the unpacked pinned checkpoint of the sherpa-onnx recogniser, for
    /// --stt sherpa-onnx. Read as given, so that an empty value of the recogniser not selected is
    /// ignored rather than refused.
    #[arg(long, env = STT_DIR_VARIABLE, value_parser = clap::value_parser!(OsString))]
    stt_dir: Option<OsString>,
    /// The pinned checkpoint file of the whisper.cpp recogniser, for --stt whispercpp.
    #[arg(long, env = WHISPERCPP_MODEL_VARIABLE, value_parser = clap::value_parser!(OsString))]
    whispercpp_model: Option<OsString>,
    /// The base URL of the llama-server's API, on loopback.
    #[arg(long, env = SERVER_URL_VARIABLE)]
    server_url: String,
    /// The model the server answers to, its --alias.
    #[arg(long, env = SERVER_MODEL_VARIABLE)]
    server_model: String,
    /// The most tokens of one answer.
    #[arg(long, default_value_t = MAX_OUTPUT_TOKENS)]
    max_output_tokens: u64,
    /// The input device by name; the default input otherwise.
    #[arg(long)]
    input: Option<String>,
    /// The output device by name; the default output otherwise.
    #[arg(long)]
    output: Option<String>,
}

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Polls `future` on this thread until it completes: the runtime commits to no executor.
fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
            return output;
        }
        thread::park();
    }
}

fn selector(name: Option<String>) -> DeviceSelector {
    name.map_or(DeviceSelector::Default, DeviceSelector::Name)
}

fn main() -> ExitCode {
    let args = Args::parse();
    match talk(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Holds the conversation until the run ends or the process is interrupted.
fn talk(args: Args) -> Result<(), Box<dyn Error>> {
    let checkpoint = match args.stt {
        SttBackend::SherpaOnnx => args.stt_dir,
        SttBackend::WhisperCpp => args.whispercpp_model,
    }
    .filter(|location| !location.is_empty())
    .map(PathBuf::from)
    .ok_or(LocalRefused::Missing(args.stt.variable()))?;
    let config = LocalConfig {
        stt: SttConfig::new(args.stt, checkpoint),
        server_url: args.server_url,
        server_model: args.server_model,
        max_output_tokens: args.max_output_tokens,
    };
    let models = LocalModels::open(&config)?;
    let host = CpalHost::new();
    let mut declaration = Declaration::new();
    let cascade = declare_local(&mut declaration, &models)?;
    let run_id = RunId::generate();
    let mut run = cascade.local_nodes(
        run_id,
        models,
        TurnStore::default(),
        &RunLog::new(),
        &host,
        &selector(args.output),
        PlayoutConfig::default(),
    )?;
    let handles = run.handles.clone();
    let mut controller = RunController::create_as(
        run_id,
        &cascade.planned,
        &declaration,
        HashMap::<GenerationId, i64>::new(),
        |node: &Node| {
            run.implementations
                .remove(&node.node_id())
                .expect("the cascade has an implementation for every node")
        },
    )?;
    controller.validate()?;
    block_on(controller.prepare())?;
    let mut ends = controller.exposed();
    let ingress = ends
        .ingress(AUDIO_INGRESS)
        .ok_or("the cascade exposes no audio ingress")?;
    let heard = print_heard(
        ends.egress(TRANSCRIPT_EGRESS)
            .ok_or("the cascade exposes no transcript egress")?,
    );
    let said = print_said(
        ends.egress(TEXT_EGRESS)
            .ok_or("the cascade exposes no assistant text egress")?,
    );
    block_on(controller.activate())?;
    let watch = EgressWatch::start(Egress::Denied, controller.requests())?;
    let drive = thread::spawn(move || {
        let result = block_on(controller.drive());
        (controller, result)
    });

    let mut microphone = Microphone::open(&host, &selector(args.input), CaptureConfig::default())?;
    println!(
        "listening on {:?}. Ask; speak over the answer to stop it. Ctrl-C ends.",
        microphone.device().name
    );
    let (stream_id, producer) = (StreamId::generate(), NodeId::generate());
    let mut sequence = 0;
    let mut reported = Reported::default();
    while !drive.is_finished() {
        reported.print(&handles);
        let Some(frame) = microphone.next_frame(Duration::from_millis(100)) else {
            continue;
        };
        sequence += 1;
        let envelope = EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id,
            session_id: None,
            stream_id,
            sequence,
            producer,
            port: AUDIO_INGRESS.to_owned(),
            kind: "inference.audio.AudioFrame".to_owned(),
            generation_id: None,
            timestamp: i64::try_from(frame.position.get()).unwrap_or(i64::MAX),
            timebase: Timebase::AudioFrameTime,
            causation_id: None,
            payload: VoicePayload::Audio(frame),
        };
        if block_on(ingress.send(envelope)).is_err() {
            break;
        }
    }
    drop(ingress);
    let (controller, result) = drive.join().map_err(|_| "the drive loop panicked")?;
    reported.print(&handles);
    let _ = heard.join();
    let _ = said.join();
    if let Some(watch) = watch
        && let Some(remote) = watch.tripped()
    {
        println!("[egress to {remote}: the run failed]");
    }
    println!("[the run ended {:?}]", controller.run().state());
    result?;
    Ok(())
}

/// Prints each committed span of `egress` as the user's line, on a thread of its own.
fn print_heard(mut egress: Box<dyn InputEndpoint<VoicePayload>>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        while let Some(envelope) = block_on(egress.recv()) {
            if let VoicePayload::Stream(StreamEvent::TranscriptCommitted(committed)) =
                envelope.payload
            {
                println!("you:   {}", committed.text);
            }
        }
    })
}

/// Prints each committed segment of `egress` on the answer's line, and ends the line when the
/// generation ends, on a thread of its own.
fn print_said(mut egress: Box<dyn InputEndpoint<VoicePayload>>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut speaking = None;
        while let Some(envelope) = block_on(egress.recv()) {
            let VoicePayload::Stream(event) = envelope.payload else {
                continue;
            };
            let ended = match event {
                StreamEvent::TextSegmentCommitted(committed) => {
                    if speaking != envelope.generation_id {
                        print!("model: ");
                        speaking = envelope.generation_id;
                    }
                    print!("{}", committed.text);
                    let _ = std::io::stdout().flush();
                    None
                }
                StreamEvent::GenerationCompleted(_) | StreamEvent::GenerationCancelled(_) => {
                    Some("")
                }
                StreamEvent::GenerationSuperseded(_) => Some(" [interrupted]"),
                _ => None,
            };
            if let Some(tail) = ended
                && speaking.take().is_some()
            {
                println!("{tail}");
            }
        }
    })
}

/// How many of each measure were printed.
#[derive(Debug, Default)]
struct Reported {
    turns: usize,
    interruptions: usize,
}

impl Reported {
    /// Prints the measures recorded since the last call.
    fn print(&mut self, handles: &CascadeHandles) {
        let turns = handles.committed_input_to_first_audio();
        for measure in turns.iter().skip(self.turns) {
            let Some(first_audio) = measure.first_audio else {
                break;
            };
            println!("[committed input to first playable audio: {first_audio:?}]");
            self.turns += 1;
        }
        let interruptions = handles.interruption_to_playback_stop();
        for measure in interruptions.iter().skip(self.interruptions) {
            println!("[interruption to playback stop: {:?}]", measure.stop);
            self.interruptions += 1;
        }
    }
}
