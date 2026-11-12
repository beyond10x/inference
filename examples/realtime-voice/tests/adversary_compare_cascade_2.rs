//! Adversarial cases, pass 2, against the fixes of F1 to F4 of pass 1: the cascade arm's
//! recogniser worker that a cut turn leaves to end on its own, the table printed before
//! `--runs-out` is written, and the ElevenLabs key resolved once for the cascade arm.
//!
//! The ElevenLabs cases drive the arm `compare` builds against a loopback fixture of the realtime
//! speech-to-text endpoint, as `adversary_compare_cascade_1.rs` does. The binary cases run the
//! `compare` example built next to this test with the environment cleared and
//! `DBUS_SESSION_BUS_ADDRESS` pointed at a socket that does not exist, so neither a provider nor
//! the OS keychain is reached.

#![cfg(feature = "local")]
#![allow(dead_code)]

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

/// `runs` runs of `arm` over `plan` on a thread of its own, as `compare` runs the cascade arm.
fn run_arm<R>(
    arm: Cascade<R, LateSynthesiser>,
    plan: Plan,
    runs: usize,
    failure: fn(&R::Handle) -> Option<String>,
) -> Vec<Measured>
where
    R: inference_stt::Recogniser + Send + 'static,
    R::Handle: Send + 'static,
{
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut arm = arm;
        let questions = [Question {
            name: "question".to_owned(),
            clip: plan.question.clone(),
        }];
        let measured = measure_cascade("cascade", &mut arm, &plan, &questions, runs, failure);
        let _ = sender.send(measured);
    });
    receiver
        .recv_timeout(DEADLINE)
        .expect("the arm's runs end within the deadline")
}

/// The first run's commit is never answered and its turn is cut at the run's limit; the
/// recognition outlives the plan's drain and is left to end on its own, as F1's fix intends. The
/// second run, and with `--clips` the next clip, then starts its recognition while the first one
/// still holds the recogniser's only place (`ElevenLabsRecogniser`'s maximum concurrency is one),
/// and it is refused. `compare` reaches this with its fixed values (read timeout 10 s, limit
/// 20 s, drain 3 s) for a `--clips` question longer than about 12.4 s; here the read timeout is
/// 3 s, the limit 2 s and the drain 300 ms after a 300 ms question, the same order of events.
#[test]
fn adv2_a_recognition_left_to_end_on_its_own_does_not_refuse_the_next_runs_turn() {
    let fixture = Fixture::start(Script::Stall);
    let arm = elevenlabs_arm(
        fixture.endpoint(Duration::from_secs(3)),
        Arc::new(FixedKey),
        generator(),
        synthesiser(),
    )
    .expect("the arm is made");
    let mut plan = Plan::new(utterance(Duration::from_millis(300)));
    plan.limit = Duration::from_secs(2);
    plan.drain = Duration::from_millis(300);
    let runs = run_arm(arm, plan, 2, elevenlabs::failure);
    drop(fixture);
    for run in &runs {
        println!("{:?}", run.notes);
    }
    assert!(
        runs[0]
            .notes
            .iter()
            .any(|note| note.contains("left to end on its own")),
        "the first run leaves its recognition: {:?}",
        runs[0].notes
    );
    assert!(
        !runs[1]
            .notes
            .iter()
            .any(|note| note.contains("the recogniser refused the turn")),
        "the second run is refused by the first run's recognition, still running: {:?}",
        runs[1].notes
    );
}

/// Control for F1's fix: a stall whose read timeout passes within the drain after the limit is
/// one note of the run, not two, and the next run is not touched by it.
#[test]
fn adv2_a_stall_caught_in_the_drain_is_noted_once_and_stays_in_its_run() {
    let fixture = Fixture::start(Script::Stall);
    let arm = elevenlabs_arm(
        fixture.endpoint(Duration::from_secs(2)),
        Arc::new(FixedKey),
        generator(),
        synthesiser(),
    )
    .expect("the arm is made");
    let mut plan = Plan::new(utterance(Duration::from_millis(300)));
    plan.limit = Duration::from_secs(2);
    plan.drain = Duration::from_secs(3);
    let runs = run_arm(arm, plan, 2, elevenlabs::failure);
    drop(fixture);
    for run in &runs {
        println!("{:?}", run.notes);
    }
    for run in &runs {
        let failures = run
            .notes
            .iter()
            .filter(|note| note.contains("the recognition failed"))
            .count();
        assert_eq!(failures, 1, "{:?}", run.notes);
        assert!(
            !run.notes
                .iter()
                .any(|note| note.contains("the recogniser refused the turn")),
            "{:?}",
            run.notes
        );
    }
}

/// The `compare` example built next to this test binary.
fn compare_binary() -> std::path::PathBuf {
    let exe = std::env::current_exe().expect("the test's path");
    let profile = exe
        .parent()
        .and_then(std::path::Path::parent)
        .expect("target/<profile>/deps");
    let binary = profile.join("examples").join("compare");
    assert!(
        binary.is_file(),
        "{} is not built: run `cargo build -p realtime-voice --features local --example compare` \
         first (cargo test of the workspace builds it)",
        binary.display()
    );
    binary
}

/// `compare` with the environment cleared but for `PATH`, the loader's path and `extra`, its
/// session bus a socket that does not exist, so the OS keychain cannot be reached.
fn compare(args: &[&str], extra: &[(&str, &str)]) -> std::process::Output {
    let bus = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("adv2-no-session-bus");
    let mut command = std::process::Command::new(compare_binary());
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env(
            "LD_LIBRARY_PATH",
            std::env::var_os("LD_LIBRARY_PATH").unwrap_or_default(),
        )
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path={}", bus.display()),
        );
    for (name, value) in extra {
        command.env(name, value);
    }
    command.args(args).output().expect("compare runs")
}

fn show(output: &std::process::Output) -> (String, String) {
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    println!(
        "status {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status
    );
    (stdout, stderr)
}

/// F2's fix: an unwritable `--runs-out` file prints the table, then the error on standard error,
/// and exits 1.
#[test]
fn adv2_an_unwritable_runs_out_prints_the_table_then_fails_naming_the_file() {
    let missing = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("adv2-no-such-dir-{}", std::process::id()))
        .join("runs.jsonl");
    let output = compare(
        &[
            "--providers",
            "cascade",
            "--runs",
            "1",
            "--runs-out",
            missing.to_str().expect("UTF-8"),
        ],
        &[("ELEVENLABS_API_KEY", "fixture-key-not-a-real-key")],
    );
    let (stdout, stderr) = show(&output);
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout.contains("cascade   cascade   skipped:"), "{stdout}");
    assert!(
        stderr.contains("--runs-out") && stderr.contains("runs.jsonl"),
        "{stderr}"
    );
    assert!(!stdout.contains("--runs-out"), "{stdout}");
}

/// F3's fix: with `ELEVENLABS_API_KEY` set the keychain is never asked, for the synthesiser or
/// the recogniser.
#[test]
fn adv2_with_the_key_variable_set_the_keychain_is_not_asked() {
    let output = compare(
        &[
            "--providers",
            "cascade",
            "--cascade-stt",
            "elevenlabs",
            "--runs",
            "1",
        ],
        &[("ELEVENLABS_API_KEY", "fixture-key-not-a-real-key")],
    );
    let (stdout, stderr) = show(&output);
    assert!(output.status.success());
    assert!(stdout.contains("cascade   cascade   skipped:"), "{stdout}");
    assert!(
        !stdout.contains("keychain") && !stderr.contains("keychain"),
        "{stdout}{stderr}"
    );
}

/// F3's fix resolves the key before anything else of the arm is checked. An arm that cannot run
/// for want of its LLM (`INFERENCE_LLAMA_SERVER_URL` unset) still asks the OS keychain first,
/// which may raise an unlock prompt and waits up to `elevenlabs::KEY_WAIT`; the same holds for
/// the default invocation of a `compare` built without feature `local`, whose cascade arm never
/// runs. Here the keychain cannot be reached, so the ask shows in the arm's reason.
#[test]
fn adv2_an_arm_skipped_for_its_llm_does_not_ask_the_keychain() {
    let output = compare(
        &[
            "--providers",
            "cascade",
            "--cascade-stt",
            "elevenlabs",
            "--runs",
            "1",
        ],
        &[],
    );
    let (stdout, stderr) = show(&output);
    assert!(output.status.success());
    assert!(stdout.contains("cascade   cascade   skipped:"), "{stdout}");
    assert!(
        !stdout.contains("keychain") && !stderr.contains("keychain"),
        "the keychain was asked for an arm without an LLM:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}
