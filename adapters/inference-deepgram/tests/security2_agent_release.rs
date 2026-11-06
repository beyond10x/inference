//! Security review of the Voice Agent binding, resource release: every session, closed,
//! dropped mid-reply, failed by the server, or whose opening failed or stalled, ends the thread
//! that drove its connection and its connection. The count is read from the process's own task
//! list, so this file holds this one test and nothing runs beside it.
#![cfg(feature = "duplex")]

#[path = "support/voice_agent.rs"]
mod fixture;

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fixture::{AgentServer, Applied, FixedKey, KEY, Script, Step};
use inference_deepgram::agent::AgentConfig;
use inference_deepgram::inference_duplex::inference_audio::{AudioEncoding, AudioFormat};
use inference_deepgram::inference_duplex::{
    NativeSpeechModel, SessionConfig, SessionEvent, SpeechSessionHandle, TurnDetection, TurnTuning,
};
use inference_deepgram::{AgentEndpoint, DeepgramAgent, DeepgramAgentSession};

fn driving_threads() -> usize {
    std::fs::read_dir("/proc/self/task")
        .expect("the task list")
        .filter_map(Result::ok)
        .filter(|task| {
            std::fs::read_to_string(task.path().join("comm"))
                .is_ok_and(|name| name.trim_end() == "deepgram-agent")
        })
        .count()
}

fn settled(bound: Duration) -> usize {
    let deadline = Instant::now() + bound;
    loop {
        let count = driving_threads();
        if count == 0 || Instant::now() >= deadline {
            return count;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn config() -> SessionConfig {
    let pcm = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).unwrap(),
        channels: NonZeroU16::MIN,
    };
    SessionConfig {
        voice: String::new(),
        instructions: String::new(),
        input_format: pcm,
        output_format: pcm,
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools: Vec::new(),
        idle_limit: Duration::from_secs(5),
    }
}

fn wait_for(session: &mut DeepgramAgentSession, stop: impl Fn(&SessionEvent) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(event) = session.next_event(Duration::from_millis(20))
            && stop(&event)
        {
            return true;
        }
    }
    false
}

#[test]
#[cfg(target_os = "linux")]
fn no_agent_session_leaves_its_thread_or_connection_behind() {
    assert_eq!(driving_threads(), 0);
    let mid_reply = Script::new(vec![
        Step::Send(fixture::agent_started_speaking()),
        Step::Binary(vec![1; 640]),
        Step::Pause(Duration::from_secs(10)),
    ]);
    let failing = Script::new(vec![Step::Send(fixture::error(
        "INTERNAL_SERVER_ERROR",
        "boom",
    ))]);
    let closing = {
        let mut script = Script::new(Vec::new());
        script.applied = Applied::Close(1008);
        script
    };
    let stalling = {
        let mut script = Script::new(Vec::new());
        script.applied = Applied::Never;
        script
    };
    let mut scripts = Vec::new();
    for _ in 0..4 {
        scripts.extend([
            mid_reply.clone(),
            mid_reply.clone(),
            failing.clone(),
            closing.clone(),
            stalling.clone(),
        ]);
    }
    let rounds = scripts.len();
    let server = AgentServer::scripted(scripts);
    let mut model = DeepgramAgent::new(
        AgentEndpoint::new()
            .with_url(&server.url)
            .with_open_timeout(Duration::from_secs(2))
            .with_read_timeout(Duration::from_secs(3)),
        AgentConfig::new(),
        Arc::new(FixedKey(KEY)),
        "deepgram-api-key",
    )
    .expect("the fixture configuration is valid");
    let mut peak = 0;
    for round in 0..rounds {
        match round % 5 {
            0 | 1 => {
                let mut session = model.open(config()).expect("the session opens");
                assert!(
                    wait_for(&mut session, |event| matches!(
                        event,
                        SessionEvent::OutputAudio(_)
                    )),
                    "round {round}"
                );
                peak = peak.max(driving_threads());
                if round % 5 == 0 {
                    session.close().expect("the session closes");
                }
                drop(session);
            }
            2 => {
                let mut session = model.open(config()).expect("the session opens");
                assert!(
                    wait_for(&mut session, |event| matches!(
                        event,
                        SessionEvent::SpeechSessionFailed(_)
                    )),
                    "round {round}"
                );
                drop(session);
            }
            _ => {
                assert!(model.open(config()).is_err(), "round {round}");
            }
        }
    }
    assert!(
        peak >= 1,
        "the measure sees a driving thread while one runs"
    );
    assert_eq!(
        settled(Duration::from_secs(3)),
        0,
        "threads left behind after {rounds} sessions"
    );
    let unclosed: Vec<usize> = (0..rounds)
        .filter(|index| {
            server
                .wait(*index, Duration::from_secs(2), |seen| seen.closed.is_some())
                .closed
                .is_none()
        })
        .collect();
    assert!(unclosed.is_empty(), "connections left open: {unclosed:?}");
}
