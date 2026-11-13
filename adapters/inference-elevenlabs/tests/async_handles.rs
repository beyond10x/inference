//! The async form of the three ElevenLabs handles against their fixture servers: 20 syntheses,
//! 20 recognitions and 20 agent conversations at once on one current-thread runtime, without
//! `spawn_blocking`, each delivering every event while a ticker on the same runtime keeps its
//! largest gap under 50 ms. Each server holds its answer back for longer than that bound, so a
//! handle that blocks the runtime thread while it waits fails the bound.

mod stt_support;
mod support;

#[path = "support/agent.rs"]
mod fixture;

use std::cell::Cell;
use std::future::Future;
use std::num::{NonZeroU16, NonZeroU32};
use std::rc::Rc;
use std::time::{Duration, Instant};

use fixture::{AGENT, AgentServer, Step as AgentStep};
use inference_elevenlabs::agent::AgentEndpoint;
use inference_elevenlabs::inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_elevenlabs::inference_duplex::{
    NativeSpeechModel, SessionConfig, SessionEvent, SpeechSessionHandle, TurnDetection, TurnTuning,
};
use inference_elevenlabs::{ElevenLabsAgent, ElevenLabsAgentSession, InputFormat, OutputFormat};
use inference_stt::{Recogniser, RecognitionHandle};
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use stt_support::{Action, Script as SttScript, Step as SttStep, Trigger, describe, manual};

const SESSIONS: usize = 20;
const TICK: Duration = Duration::from_millis(10);
const GAP_BOUND: Duration = Duration::from_millis(50);
/// How long each server holds its answer back: longer than the gap bound.
const HELD: Duration = Duration::from_millis(150);
/// How long one session may take before the test fails it.
const SESSION_BOUND: Duration = Duration::from_secs(20);

/// Runs `sessions` on a current-thread runtime beside a ticker that ticks every [`TICK`] until
/// they are done; returns what they returned and the ticker's largest gap.
fn with_ticker<F, T>(sessions: Vec<F>) -> (Vec<T>, Duration)
where
    F: Future<Output = T> + 'static,
    T: 'static,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime");
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async move {
        let done = Rc::new(Cell::new(false));
        let ticker = {
            let done = Rc::clone(&done);
            tokio::task::spawn_local(async move {
                let mut largest = Duration::ZERO;
                let mut last = Instant::now();
                while !done.get() {
                    tokio::time::sleep(TICK).await;
                    let now = Instant::now();
                    largest = largest.max(now - last);
                    last = now;
                }
                largest
            })
        };
        let running: Vec<_> = sessions
            .into_iter()
            .map(|session| {
                tokio::task::spawn_local(async move {
                    tokio::time::timeout(SESSION_BOUND, session)
                        .await
                        .expect("the session ends within its bound")
                })
            })
            .collect();
        let mut results = Vec::new();
        for session in running {
            results.push(session.await.expect("a session task ends without a panic"));
        }
        done.set(true);
        let largest = ticker.await.expect("the ticker ends without a panic");
        (results, largest)
    })
}

/// One synthesis of two segments against `server` through the async form: every event until
/// the async form ends, and the failure, if any.
async fn synthesise(server: Rc<support::FixtureServer>) -> (Vec<SynthesisEvent>, Option<String>) {
    let mut synthesiser = server.synthesiser(OutputFormat::Ulaw8000);
    let mut handle = synthesiser
        .start()
        .expect("a fresh synthesiser runs nothing");
    for segment in support::committed(&["First.", "Second."]) {
        handle
            .offer_segment(segment)
            .expect("the utterance is open");
    }
    handle.end_utterance().expect("the utterance ends once");
    let mut events = Vec::new();
    while let Some(event) = handle.event().await {
        events.push(event);
    }
    (events, handle.failure().map(ToString::to_string))
}

#[test]
fn twenty_syntheses_wait_for_the_server_without_blocking_the_runtime() {
    let server = Rc::new(support::FixtureServer::paced(
        OutputFormat::Ulaw8000,
        vec![support::Script::Serve],
        HELD,
        Duration::ZERO,
    ));
    let (results, largest) = with_ticker(
        (0..SESSIONS)
            .map(|_| synthesise(Rc::clone(&server)))
            .collect(),
    );
    for (events, failure) in &results {
        assert_eq!(failure, &None);
        assert!(
            matches!(events.last(), Some(SynthesisEvent::Usage(_))),
            "the usage record is the last event"
        );
        let completed = events
            .iter()
            .filter(|event| matches!(event, SynthesisEvent::Completed(_)))
            .count();
        assert_eq!(completed, 2, "both segments complete");
    }
    assert_eq!(server.violations(), Vec::<String>::new());
    assert!(largest < GAP_BOUND, "the ticker's largest gap {largest:?}");
}

/// `session_started`, a partial transcript after the fifth chunk, and the committed transcript
/// [`HELD`] after the commit.
fn held_commit() -> SttScript {
    let step = |on, action| SttStep { on, action };
    let committed = serde_json::json!({
        "message_type": "committed_transcript_with_timestamps",
        "text": "eins",
        "words": [{"text": "eins", "start": 0.1, "end": 0.4, "type": "word", "logprob": -0.1}],
    });
    SttScript::Steps(vec![
        step(Trigger::Open, Action::Send(stt_support::session_started())),
        step(
            Trigger::Audio(5),
            Action::Send(stt_support::partial("eins")),
        ),
        step(Trigger::Commit(1), Action::Wait(HELD)),
        step(Trigger::Commit(1), Action::Send(committed.to_string())),
    ])
}

/// One recognition against `server` through the async form: ten frames, each followed by what
/// has arrived, then the end of input and every event until the async form ends.
async fn recognise(server: Rc<stt_support::FixtureServer>) -> (Vec<String>, Option<String>) {
    let mut recogniser = server.recogniser(manual(InputFormat::Ulaw8000));
    let mut handle = recogniser
        .start(stt_support::request())
        .expect("a fresh recogniser runs nothing");
    let mut events = Vec::new();
    for frame in stt_support::mulaw(10) {
        handle.offer_frame(frame).expect("input is open");
        while let Some(event) = handle.event().await {
            events.push(describe(&event));
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    handle.end_input().expect("input ends once");
    while let Some(event) = handle.event().await {
        events.push(describe(&event));
    }
    (events, handle.failure().map(ToString::to_string))
}

#[test]
fn twenty_recognitions_wait_for_the_server_without_blocking_the_runtime() {
    let server = Rc::new(stt_support::FixtureServer::start(held_commit()));
    let (results, largest) = with_ticker(
        (0..SESSIONS)
            .map(|_| recognise(Rc::clone(&server)))
            .collect(),
    );
    for (events, failure) in &results {
        assert_eq!(failure, &None);
        assert_eq!(events, &["hypothesis eins", "commit eins", "usage"]);
    }
    assert_eq!(server.violations(), Vec::<String>::new());
    assert!(largest < GAP_BOUND, "the ticker's largest gap {largest:?}");
}

fn pcm16k() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).expect("not zero"),
        channels: NonZeroU16::MIN,
    }
}

fn agent_config() -> SessionConfig {
    SessionConfig {
        voice: String::new(),
        instructions: String::new(),
        input_format: pcm16k(),
        output_format: pcm16k(),
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools: Vec::new(),
        idle_limit: Duration::from_secs(5),
    }
}

/// A session of a fresh agent at `server`. Opening is `NativeSpeechModel::open`, the
/// handshake and the metadata, which the contract makes synchronous (3.7 to 8.4 ms each against
/// the fixture, measured); the async form is the session's, so the test opens every session
/// before the ticker starts and the bound holds for the conversations.
fn open(server: &AgentServer) -> ElevenLabsAgentSession {
    let endpoint = AgentEndpoint::new()
        .with_url(&server.url)
        .with_connect_timeout(Duration::from_secs(5))
        .with_metadata_timeout(Duration::from_secs(5))
        .with_read_timeout(Duration::from_secs(5));
    let mut agent = ElevenLabsAgent::public(AGENT, endpoint).expect("a valid agent id");
    agent.open(agent_config()).expect("the session opens")
}

/// One conversation of `session` through the async form: a frame of audio, every event until
/// the agent's response completes, then the close and every event until the async form ends.
async fn converse(mut session: ElevenLabsAgentSession) -> Vec<SessionEvent> {
    session
        .append_audio(AudioFrame {
            format: pcm16k(),
            position: SampleClockPosition::new(0).expect("zero"),
            sample_count: 320,
            data: vec![0; 640],
        })
        .expect("audio in the session's format");
    let mut events = Vec::new();
    while let Some(event) = session.event().await {
        let completed = matches!(event, SessionEvent::ResponseCompleted(_));
        events.push(event);
        if completed {
            break;
        }
    }
    session.close().expect("the session is open");
    while let Some(event) = session.event().await {
        events.push(event);
    }
    events
}

#[test]
fn twenty_agent_conversations_wait_for_the_server_without_blocking_the_runtime() {
    let server = Rc::new(AgentServer::start(fixture::Script::new(vec![
        AgentStep::AwaitAudio(1),
        AgentStep::Pause(HELD),
        AgentStep::Send(fixture::user_transcript("what is the weather", 4)),
        AgentStep::Send(fixture::audio(5, 640)),
        AgentStep::Send(fixture::audio(5, 640)),
        AgentStep::Send(fixture::agent_response("It is mild.", 5, Some("r1"))),
    ])));
    let sessions: Vec<_> = (0..SESSIONS).map(|_| open(&server)).collect();
    let (results, largest) = with_ticker(sessions.into_iter().map(converse).collect());
    for events in &results {
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, SessionEvent::OutputAudio(_)))
                .count(),
            2,
            "both frames of the agent's audio: {events:#?}"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, SessionEvent::ResponseCompleted(_))),
            "the response completed: {events:#?}"
        );
        assert!(
            matches!(events.last(), Some(SessionEvent::SpeechSessionClosed(_))),
            "the async form ends after the close: {events:#?}"
        );
    }
    assert_eq!(server.connections(), SESSIONS);
    assert!(largest < GAP_BOUND, "the ticker's largest gap {largest:?}");
}
