//! Adversarial cases against `compare --cascade-stt elevenlabs`, `--clips` and `--runs-out`.
//!
//! The ElevenLabs cases drive the arm `compare` builds (`elevenlabs_arm`, `measure_cascade`,
//! `elevenlabs::failure`) against a loopback fixture of the realtime speech-to-text endpoint, as
//! `tests/compare_elevenlabs.rs` does, with scripts that test does not have: a commit never
//! answered, and a connection closed at the commit. The binary cases run the `compare` example
//! built next to this test (`target/<profile>/examples/compare`), with no provider configured, so
//! nothing leaves the machine.

#![cfg(feature = "local")]

use std::num::NonZeroU32;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll};
use std::thread;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use inference_elevenlabs::{Credential, CredentialFuture, CredentialSource, Endpoint};
use inference_llm::Generator;
use inference_llm::llm_core::{
    self, BoxFuture, Cancel, Capabilities, Provenance, StreamSink, TurnOutcome, TurnRequest,
};
use inference_stt::inference_core::model::{Adapter, ModelId};
use inference_testkit::llm::{MockConfiguration as LlmConfiguration, MockGenerator};
use inference_testkit::tts::{
    MockConfiguration as TtsConfiguration, MockSynthesis, MockSynthesiser,
};
use inference_tts::inference_text::{
    AudioPlayed, OutputSegment, SegmentDiscarded, SegmentId, SegmentQueued,
};
use inference_tts::{
    ConcurrencyLimitReached as TtsLimit, DiscardRefused, OfferRefused, PlaybackReceipt,
    PlaybackRefused, SynthesisEvent, SynthesisHandle, SynthesisOutput, Synthesiser, UtteranceEnded,
};
use realtime_voice::compare::{
    Cascade, Clip, Measured, Plan, Question, elevenlabs_arm, measure_cascade,
};
use realtime_voice::{elevenlabs, pcm16};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

const STT_DELAY: Duration = Duration::from_millis(150);
const LLM_DELAY: Duration = Duration::from_millis(150);
const TTS_DELAY: Duration = Duration::from_millis(150);
/// The slack `compare_elevenlabs.rs` allows over the hold and the injected delays.
const SLACK: Duration = Duration::from_millis(250);
const DEADLINE: Duration = Duration::from_secs(40);
const KEY: &str = "fixture-key-adversary";

/// What the fixture endpoint does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Script {
    /// A partial transcript at the first chunk; each commit answered after [`STT_DELAY`].
    Answer,
    /// A partial transcript at the first chunk; commits are never answered.
    Stall,
    /// A partial transcript at the first chunk; the connection is closed at the commit.
    CloseAtCommit,
}

struct Fixture {
    url: String,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Fixture {
    fn start(script: Script) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        listener.set_nonblocking(true).expect("non-blocking");
        let url = format!("ws://{}", listener.local_addr().expect("an address"));
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let thread = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime");
            runtime.block_on(async move {
                let listener = TcpListener::from_std(listener).expect("a listener");
                let mut stopped = stopped;
                loop {
                    let accepted = tokio::select! {
                        _ = &mut stopped => break,
                        accepted = listener.accept() => accepted,
                    };
                    let Ok((stream, _)) = accepted else { break };
                    tokio::spawn(serve(stream, script));
                }
            });
        });
        Self {
            url,
            stop: Some(stop),
            thread: Some(thread),
        }
    }

    fn endpoint(&self, read_timeout: Duration) -> Endpoint {
        Endpoint::new()
            .with_url(&self.url)
            .with_read_timeout(read_timeout)
            .with_connect_timeout(Duration::from_secs(5))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

async fn serve(stream: tokio::net::TcpStream, script: Script) {
    let Ok(mut socket) = tokio_tungstenite::accept_async(stream).await else {
        return;
    };
    let started = json!({
        "message_type": "session_started",
        "session_id": "fixture-session",
        "config": {"sample_rate": 16000, "audio_format": "pcm_16000", "model_id": "scribe_v2_realtime"}
    });
    let _ = socket.send(Message::text(started.to_string())).await;
    let mut audio = 0;
    while let Some(Ok(message)) = socket.next().await {
        let Message::Text(text) = message else {
            if matches!(message, Message::Close(_)) {
                break;
            }
            continue;
        };
        let chunk: Value = serde_json::from_str(text.as_str()).expect("a JSON chunk");
        if chunk["audio_base_64"]
            .as_str()
            .is_some_and(|audio| !audio.is_empty())
        {
            audio += 1;
            if audio == 1 {
                let partial = json!({
                    "message_type": "partial_transcript",
                    "text": "what is the capital of france"
                });
                let _ = socket.send(Message::text(partial.to_string())).await;
            }
        }
        if chunk["commit"].as_bool() == Some(true) {
            match script {
                Script::Answer => {
                    tokio::time::sleep(STT_DELAY).await;
                    let committed = json!({
                        "message_type": "committed_transcript_with_timestamps",
                        "text": "what is the capital of france",
                        "words": [{"text": "what", "start": 0.0, "end": 0.3, "type": "word"}]
                    });
                    let _ = socket.send(Message::text(committed.to_string())).await;
                }
                Script::Stall => {}
                Script::CloseAtCommit => {
                    let _ = socket.close(None).await;
                    return;
                }
            }
        }
    }
}

struct FixedKey;

impl CredentialSource for FixedKey {
    fn resolve<'a>(&'a self, _name: &'a str) -> CredentialFuture<'a> {
        Box::pin(std::future::ready(Ok(Credential::new(
            KEY.as_bytes().to_vec(),
        ))))
    }
}

struct WallDelay {
    delay: Duration,
    done: Arc<AtomicBool>,
    started: bool,
}

impl std::future::Future for WallDelay {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.done.load(Ordering::SeqCst) {
            return Poll::Ready(());
        }
        if !self.started {
            self.started = true;
            let done = Arc::clone(&self.done);
            let delay = self.delay;
            let waker = cx.waker().clone();
            thread::spawn(move || {
                thread::sleep(delay);
                done.store(true, Ordering::SeqCst);
                waker.wake();
            });
        }
        Poll::Pending
    }
}

#[derive(Debug)]
struct LateModel {
    inner: Generator,
}

impl llm_core::Model for LateModel {
    fn provenance(&self) -> &Provenance {
        self.inner.model().provenance()
    }

    fn capabilities(&self) -> &Capabilities {
        self.inner.model().capabilities()
    }

    fn turn<'a>(
        &'a self,
        request: &'a TurnRequest,
        sink: &'a mut dyn StreamSink,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<TurnOutcome, llm_core::Error>> {
        Box::pin(async move {
            WallDelay {
                delay: LLM_DELAY,
                done: Arc::new(AtomicBool::new(false)),
                started: false,
            }
            .await;
            self.inner.model().turn(request, sink, cancel).await
        })
    }
}

fn generator() -> Generator {
    let mock: Generator = MockGenerator::new(
        LlmConfiguration::StreamingOnly,
        ModelId::generate(),
        ["Paris is the capital of France, one two three four five six seven eight."],
    )
    .into();
    Generator::new(mock.adapter().clone(), Arc::new(LateModel { inner: mock }))
        .expect("the wrapper declares what the mock declares")
}

struct LateSynthesiser(MockSynthesiser);

impl Synthesiser for LateSynthesiser {
    type Handle = LateSynthesis;

    fn adapter(&self) -> &Adapter {
        self.0.adapter()
    }

    fn start(&mut self) -> Result<LateSynthesis, TtsLimit> {
        Ok(LateSynthesis {
            inner: self.0.start()?,
            offered: None,
            audio: false,
        })
    }
}

struct LateSynthesis {
    inner: MockSynthesis,
    offered: Option<Instant>,
    audio: bool,
}

impl Iterator for LateSynthesis {
    type Item = SynthesisEvent;

    fn next(&mut self) -> Option<SynthesisEvent> {
        let event = self.inner.next()?;
        if matches!(event, SynthesisEvent::Audio(_)) && !self.audio {
            self.audio = true;
            if let Some(offered) = self.offered {
                thread::sleep((offered + TTS_DELAY).saturating_duration_since(Instant::now()));
            }
        }
        Some(event)
    }
}

impl SynthesisHandle for LateSynthesis {
    fn output(&self) -> &SynthesisOutput {
        self.inner.output()
    }

    fn offer_segment(&mut self, segment: OutputSegment) -> Result<SegmentQueued, OfferRefused> {
        self.offered.get_or_insert_with(Instant::now);
        self.inner.offer_segment(segment)
    }

    fn end_utterance(&mut self) -> Result<(), UtteranceEnded> {
        self.inner.end_utterance()
    }

    fn acknowledge_playback(
        &mut self,
        segment_id: SegmentId,
        receipt: PlaybackReceipt,
    ) -> Result<AudioPlayed, PlaybackRefused> {
        self.inner.acknowledge_playback(segment_id, receipt)
    }

    fn discard(&mut self, segment_id: SegmentId) -> Result<SegmentDiscarded, DiscardRefused> {
        self.inner.discard(segment_id)
    }
}

fn synthesiser() -> LateSynthesiser {
    LateSynthesiser(MockSynthesiser::new(
        TtsConfiguration::IncrementalStreaming,
        ModelId::generate(),
        pcm16(16_000),
    ))
}

/// `speech` of voiced audio, then 100 ms of silence, at 16 kHz.
fn utterance(speech: Duration) -> Clip {
    let voiced = (speech.as_millis() as usize) * 16;
    let mut samples = vec![0.25_f32; voiced];
    samples.extend([0.0; 1_600]);
    Clip::new(samples, NonZeroU32::new(16_000).expect("non-zero"))
}

/// One run of `arm` over `plan` on a thread of its own, as `compare` runs the cascade arm.
fn run_arm<R>(
    arm: Cascade<R, LateSynthesiser>,
    plan: Plan,
    failure: fn(&R::Handle) -> Option<String>,
) -> Measured
where
    R: inference_stt::Recogniser + Send + 'static,
    R::Handle: Send,
{
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut arm = arm;
        let questions = [Question {
            name: "question".to_owned(),
            clip: plan.question.clone(),
        }];
        let mut measured = measure_cascade("cascade", &mut arm, &plan, &questions, 1, failure);
        let _ = sender.send(measured.remove(0));
    });
    receiver
        .recv_timeout(DEADLINE)
        .expect("the arm's run ends within the deadline")
}

fn elevenlabs_run(script: Script, read_timeout: Duration, plan: Plan) -> Measured {
    let fixture = Fixture::start(script);
    let arm = elevenlabs_arm(
        fixture.endpoint(read_timeout),
        Arc::new(FixedKey),
        generator(),
        synthesiser(),
    )
    .expect("the arm is made");
    let measured = run_arm(arm, plan, elevenlabs::failure);
    drop(fixture);
    measured
}

fn failed(run: &Measured) -> bool {
    run.notes
        .iter()
        .any(|note| note.contains("the recognition failed"))
}

/// Control: a commit the endpoint never answers, its read timeout well inside the run's limit,
/// is a note of the run naming the recognition's failure.
#[test]
fn adv_a_stall_within_the_limit_is_a_note_of_the_arms_run() {
    let run = elevenlabs_run(
        Script::Stall,
        Duration::from_secs(2),
        Plan::new(utterance(Duration::from_millis(300))),
    );
    println!("{run:?}");
    assert!(failed(&run), "{:?}", run.notes);
}

/// A commit the endpoint never answers, where the recogniser's read timeout passes after the
/// run's limit. `compare` cannot set either: the endpoint's read timeout is
/// `DEFAULT_READ_TIMEOUT` (10 s), the plan's limit 20 s from the start of the question, so a
/// `--clips` question longer than about 9.4 s reaches this. Here the read timeout is 3 s and the
/// limit 3 s after a 300 ms question, the same order of events. The stall is the reason the run
/// measured nothing, and the row has to say so.
#[test]
fn adv_a_stall_past_the_runs_limit_is_still_a_note_of_the_arms_run() {
    let mut plan = Plan::new(utterance(Duration::from_millis(300)));
    plan.limit = Duration::from_secs(3);
    let run = elevenlabs_run(Script::Stall, Duration::from_secs(3), plan);
    println!("{run:?}");
    assert!(
        failed(&run),
        "the recogniser's stall is dropped from the arm's row; its notes: {:?}",
        run.notes
    );
}

/// A connection the endpoint closes at the commit, without answering it.
#[test]
fn adv_a_connection_closed_at_the_commit_is_a_note_of_the_arms_run() {
    let run = elevenlabs_run(
        Script::CloseAtCommit,
        Duration::from_secs(3),
        Plan::new(utterance(Duration::from_millis(300))),
    );
    println!("{run:?}");
    assert!(failed(&run), "{:?}", run.notes);
}

/// The mutant `compare_elevenlabs.rs`'s bound has to catch: the same arm committing by the
/// endpoint's VAD (`elevenlabs::recogniser`, as `talk` binds it) instead of manually. Its first
/// audio must fall outside the hold, the injected delays and the slack.
#[test]
fn adv_the_first_audio_bound_catches_an_arm_that_waits_out_the_vad_quiet_window() {
    let fixture = Fixture::start(Script::Answer);
    let recogniser =
        elevenlabs::recogniser(fixture.endpoint(Duration::from_secs(3)), Arc::new(FixedKey))
            .expect("the recogniser is made");
    let mut arm = Cascade::new(recogniser, generator(), synthesiser(), pcm16(16_000));
    arm.attack = local_voice_cascade::local::SPEECH_VAD.attack;
    arm.hold = local_voice_cascade::local::SPEECH_VAD.hold;
    let hold = arm.hold;
    let run = run_arm(
        arm,
        Plan::new(utterance(Duration::from_millis(300))),
        elevenlabs::failure,
    );
    drop(fixture);
    println!("{run:?}");
    let first_audio = run.first_audio.expect("first audio measured");
    let bound = hold + STT_DELAY + LLM_DELAY + TTS_DELAY + SLACK;
    assert!(
        first_audio > bound,
        "a VAD-committed arm's first audio {first_audio:?} passes the bound {bound:?}"
    );
}

/// The `compare` example in the profile directory above this test binary: `target/<profile>/deps`
/// before Cargo 1.100, `target/<profile>/build/<package>/<hash>/deps` since.
fn compare_binary() -> std::path::PathBuf {
    let exe = std::env::current_exe().expect("the test's path");
    let binary = exe
        .ancestors()
        .map(|dir| dir.join("examples").join("compare"))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| exe.with_file_name("examples").join("compare"));
    assert!(
        binary.is_file(),
        "{} is not built: run `cargo build -p realtime-voice --features local --example compare` \
         first (cargo test of the workspace builds it)",
        binary.display()
    );
    binary
}

/// `compare` with no provider configured: the cascade arm skips, naming why, and nothing leaves
/// the machine.
fn compare(args: &[&str]) -> std::process::Output {
    std::process::Command::new(compare_binary())
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env(
            "LD_LIBRARY_PATH",
            std::env::var_os("LD_LIBRARY_PATH").unwrap_or_default(),
        )
        .env("ELEVENLABS_API_KEY", "fixture-key-not-a-real-key")
        .args(args)
        .output()
        .expect("compare runs")
}

/// Control: the same invocation with a `--runs-out` file that can be written prints the table.
#[test]
fn adv_compare_prints_its_table_with_a_writable_runs_out() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("adv-runs-out-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("the directory");
    let file = dir.join("runs.jsonl");
    let output = compare(&[
        "--providers",
        "cascade",
        "--cascade-stt",
        "elevenlabs",
        "--runs",
        "1",
        "--runs-out",
        file.to_str().expect("UTF-8"),
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!(
        "status {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status
    );
    assert!(output.status.success());
    assert!(stdout.contains("cascade   cascade   skipped:"), "{stdout}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `--runs-out` names a file that cannot be written, as a mistyped directory does. The file is
/// written after every provider ran, billed runs included; the table of those runs is what the
/// operator paid for, and it must still be printed.
#[test]
fn adv_a_runs_out_file_that_does_not_write_does_not_swallow_the_table() {
    let missing = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("adv-no-such-dir-{}", std::process::id()))
        .join("runs.jsonl");
    let output = compare(&[
        "--providers",
        "cascade",
        "--cascade-stt",
        "elevenlabs",
        "--runs",
        "1",
        "--runs-out",
        missing.to_str().expect("UTF-8"),
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!(
        "status {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status
    );
    assert!(
        stdout.contains("provider  topology") && stdout.contains("cascade   cascade   skipped:"),
        "the comparison ran and its table was not printed because --runs-out did not write; \
         stdout:\n{stdout}\nstderr:\n{stderr}"
    );
}
