//! `compare --cascade-stt elevenlabs`: the cascade arm listening with ElevenLabs speech-to-text,
//! against a loopback fixture of the realtime speech-to-text endpoint, the testkit's LLM and
//! synthesiser behind injected wall-clock delays, on the wall clock as `compare` runs it.
//!
//! The fixture endpoint answers each commit after [`STT_DELAY`]; the LLM starts streaming
//! [`LLM_DELAY`] after its turn starts; the synthesiser gives its first frame [`TTS_DELAY`] after
//! its first segment was offered. The arm's own detector ends the turn after its hold of silence,
//! so the answer's first audio comes the hold and those three delays after the end of the
//! question's speech, plus what the loopback and the threads take. A recognition that waited out
//! the endpoint's voice activity detection would add its quiet window of 500 ms
//! (`inference_elevenlabs::VAD_END_QUIET`).

#![cfg(feature = "local")]

use std::num::NonZeroU32;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
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
    Clip, Comparison, Measure, Measured, Plan, Question, Topology, elevenlabs_arm, measure_cascade,
};
use realtime_voice::{elevenlabs, pcm16};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

/// The fixture endpoint's time from a commit to its committed transcript.
const STT_DELAY: Duration = Duration::from_millis(150);
/// The LLM's time from the start of its turn to its first delta.
const LLM_DELAY: Duration = Duration::from_millis(150);
/// The synthesiser's time from its first segment's offer to its first frame.
const TTS_DELAY: Duration = Duration::from_millis(150);
/// What the loopback, the threads and the frame pace may add.
const SLACK: Duration = Duration::from_millis(250);
/// The wall-clock time a case may take.
const DEADLINE: Duration = Duration::from_secs(30);

const KEY: &str = "fixture-key-compare-elevenlabs";

/// What the fixture endpoint does at its first chunk of audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Script {
    /// Sends a partial transcript, and answers each commit with the committed transcript.
    Answer,
    /// Sends an `auth_error`.
    Refuse,
}

/// A loopback fixture of the realtime speech-to-text endpoint; it stops when dropped.
struct Fixture {
    url: String,
    queries: Arc<Mutex<Vec<String>>>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Fixture {
    fn start(script: Script) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        listener.set_nonblocking(true).expect("non-blocking");
        let url = format!("ws://{}", listener.local_addr().expect("an address"));
        let queries = Arc::new(Mutex::new(Vec::new()));
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let seen = Arc::clone(&queries);
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
                    tokio::spawn(serve(stream, script, Arc::clone(&seen)));
                }
            });
        });
        Self {
            url,
            queries,
            stop: Some(stop),
            thread: Some(thread),
        }
    }

    fn endpoint(&self) -> Endpoint {
        Endpoint::new()
            .with_url(&self.url)
            .with_read_timeout(Duration::from_secs(3))
            .with_connect_timeout(Duration::from_secs(5))
    }

    fn queries(&self) -> Vec<String> {
        self.queries.lock().expect("not poisoned").clone()
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

/// The handshake callback's error type is tungstenite's `ErrorResponse`, which its `Callback`
/// trait fixes, so the size of that error is not this function's to change.
#[allow(clippy::result_large_err)]
async fn serve(stream: tokio::net::TcpStream, script: Script, queries: Arc<Mutex<Vec<String>>>) {
    let query = Arc::new(Mutex::new(String::new()));
    let heard = Arc::clone(&query);
    let callback = move |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                         response| {
        *heard.lock().expect("not poisoned") = request.uri().query().unwrap_or_default().to_owned();
        Ok(response)
    };
    let Ok(mut socket) = tokio_tungstenite::accept_hdr_async(stream, callback).await else {
        return;
    };
    queries
        .lock()
        .expect("not poisoned")
        .push(query.lock().expect("not poisoned").clone());
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
                let first = match script {
                    Script::Answer => json!({
                        "message_type": "partial_transcript",
                        "text": "what is the capital of france"
                    }),
                    Script::Refuse => json!({
                        "message_type": "auth_error",
                        "error": "fixture: the key was refused"
                    }),
                };
                let _ = socket.send(Message::text(first.to_string())).await;
            }
        }
        if chunk["commit"].as_bool() == Some(true) && script == Script::Answer {
            tokio::time::sleep(STT_DELAY).await;
            let committed = json!({
                "message_type": "committed_transcript_with_timestamps",
                "text": "what is the capital of france",
                "words": [{"text": "what", "start": 0.0, "end": 0.3, "type": "word"}]
            });
            let _ = socket.send(Message::text(committed.to_string())).await;
        }
    }
}

/// A credential source that always gives the fixture key.
struct FixedKey;

impl CredentialSource for FixedKey {
    fn resolve<'a>(&'a self, _name: &'a str) -> CredentialFuture<'a> {
        Box::pin(std::future::ready(Ok(Credential::new(
            KEY.as_bytes().to_vec(),
        ))))
    }
}

/// Pending until `delay` of wall-clock time passed, woken by a thread of its own.
struct WallDelay {
    delay: Duration,
    done: Arc<AtomicBool>,
    started: bool,
}

impl WallDelay {
    fn new(delay: Duration) -> Self {
        Self {
            delay,
            done: Arc::new(AtomicBool::new(false)),
            started: false,
        }
    }
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

/// The testkit's model, starting each turn [`LLM_DELAY`] late.
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
            WallDelay::new(LLM_DELAY).await;
            self.inner.model().turn(request, sink, cancel).await
        })
    }
}

fn generator() -> Generator {
    let mock: Generator = MockGenerator::new(
        LlmConfiguration::StreamingOnly,
        ModelId::generate(),
        [
            "Paris is the capital of France, and it has been for a very long time, \
             one two three four five six seven eight nine ten eleven twelve.",
        ],
    )
    .into();
    Generator::new(mock.adapter().clone(), Arc::new(LateModel { inner: mock }))
        .expect("the wrapper declares what the mock declares")
}

/// The testkit's synthesiser, giving its first frame [`TTS_DELAY`] after its first offer.
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

/// 300 ms of speech, then 100 ms of silence, at 16 kHz.
fn utterance() -> Clip {
    let mut samples = vec![0.25_f32; 4_800];
    samples.extend([0.0; 1_600]);
    Clip::new(samples, NonZeroU32::new(16_000).expect("non-zero"))
}

/// Runs `runs` runs of the ElevenLabs arm against `fixture` on a thread of its own, failing once
/// [`DEADLINE`] passed; returns the runs and the arm's hold.
fn measure(fixture: &Fixture, plan: Plan, runs: usize) -> (Vec<Measured>, Duration) {
    let endpoint = fixture.endpoint();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut arm = elevenlabs_arm(endpoint, Arc::new(FixedKey), generator(), synthesiser())
            .expect("the arm is made");
        let hold = arm.hold;
        let questions = [Question {
            name: "question".to_owned(),
            clip: plan.question.clone(),
        }];
        let measured = measure_cascade(
            "cascade",
            &mut arm,
            &plan,
            &questions,
            runs,
            elevenlabs::failure,
        );
        let _ = sender.send((measured, hold));
    });
    receiver
        .recv_timeout(DEADLINE)
        .expect("the arm's runs end within the deadline")
}

#[test]
fn the_elevenlabs_arm_commits_at_its_own_end_of_turn_and_gives_every_column() {
    let fixture = Fixture::start(Script::Answer);
    let mut plan = Plan::new(utterance());
    plan.barge_in = Some(utterance());
    plan.barge_after = Duration::from_millis(60);
    plan.pause = Duration::from_millis(200);
    let (runs, hold) = measure(&fixture, plan, 1);
    println!("{runs:?}");
    let run = &runs[0];
    assert_eq!(run.notes, Vec::<String>::new(), "{run:?}");
    assert_eq!(run.topology, Topology::Cascade);

    let first_audio = run.first_audio.expect("first audio measured");
    let delays = hold + STT_DELAY + LLM_DELAY + TTS_DELAY;
    assert!(
        first_audio >= delays && first_audio <= delays + SLACK,
        "first audio {first_audio:?}: the hold {hold:?} and the injected delays come to \
         {delays:?}, with at most {SLACK:?} more"
    );
    for query in fixture.queries() {
        assert!(
            query
                .split('&')
                .any(|pair| pair == "commit_strategy=manual"),
            "the arm commits itself, not by the endpoint's VAD: {query}"
        );
    }

    let mut comparison = Comparison::new();
    comparison.ran_as("cascade", Topology::Cascade, runs);
    let table = comparison.to_string();
    println!("{table}");
    let row = table
        .lines()
        .find(|line| line.starts_with("cascade "))
        .unwrap_or_else(|| panic!("a cascade row in\n{table}"));
    for measure in Measure::ALL {
        if measure.applies(Topology::Cascade) {
            assert!(
                comparison.spread("cascade", measure).is_some(),
                "{} measured: {row}",
                measure.name()
            );
        }
    }
    assert!(!row.contains("- 0/1"), "every column measured: {row}");
}

#[test]
fn a_failed_elevenlabs_recognition_is_a_note_of_the_arms_run() {
    let fixture = Fixture::start(Script::Refuse);
    let (runs, _) = measure(&fixture, Plan::new(utterance()), 1);
    let run = &runs[0];
    assert!(
        run.notes
            .iter()
            .any(|note| note.contains("the recognition failed")
                && note.contains("fixture: the key was refused")),
        "{:?}",
        run.notes
    );
    let mut comparison = Comparison::new();
    comparison.ran_as("cascade", Topology::Cascade, runs);
    let table = comparison.to_string();
    let lines: Vec<&str> = table.lines().collect();
    let at = lines
        .iter()
        .position(|line| line.starts_with("cascade ") && line.contains(" run 1: "))
        .unwrap_or_else(|| panic!("the arm's run in\n{table}"));
    assert!(
        lines[at + 1..]
            .iter()
            .take_while(|line| line.starts_with(' '))
            .any(|line| line.contains("fixture: the key was refused")),
        "the failure under the arm's run in\n{table}"
    );
}
