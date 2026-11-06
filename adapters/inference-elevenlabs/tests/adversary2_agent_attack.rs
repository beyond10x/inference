//! Adversary cases on the agent conversation binding, against the fixture server of
//! `support/agent.rs`: audio of a message that arrives after its `agent_response`, which the
//! protocol notes allow ("usually after its audio started"), and pings while the caller reads
//! nothing for longer than the inbound queue holds.
#![cfg(feature = "duplex")]

#[path = "support/agent.rs"]
mod fixture;

use std::num::{NonZeroU16, NonZeroU32};
use std::time::{Duration, Instant};

use fixture::{AGENT, AgentServer, Script, Step};
use inference_elevenlabs::agent::AgentEndpoint;
use inference_elevenlabs::inference_duplex::inference_audio::{AudioEncoding, AudioFormat};
use inference_elevenlabs::inference_duplex::{
    NativeSpeechModel, SessionConfig, SessionEvent, SpeechSessionHandle, TurnDetection, TurnTuning,
};
use inference_elevenlabs::{ElevenLabsAgent, ElevenLabsAgentSession};

const WAIT: Duration = Duration::from_millis(20);

fn pcm16() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).unwrap(),
        channels: NonZeroU16::MIN,
    }
}

fn config(idle_limit: Duration) -> SessionConfig {
    SessionConfig {
        voice: String::new(),
        instructions: String::new(),
        input_format: pcm16(),
        output_format: pcm16(),
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools: Vec::new(),
        idle_limit,
    }
}

fn open(server: &AgentServer, idle_limit: Duration) -> ElevenLabsAgentSession {
    let endpoint = AgentEndpoint::new()
        .with_url(&server.url)
        .with_connect_timeout(Duration::from_secs(3))
        .with_metadata_timeout(Duration::from_secs(3))
        .with_read_timeout(Duration::from_secs(3));
    ElevenLabsAgent::public(AGENT, endpoint)
        .expect("a valid agent id")
        .open(config(idle_limit))
        .expect("the session opens")
}

/// Every event within `within`, or until `stop` holds for one, that one included.
fn events_for(
    session: &mut ElevenLabsAgentSession,
    within: Duration,
    mut stop: impl FnMut(&SessionEvent) -> bool,
) -> Vec<SessionEvent> {
    let deadline = Instant::now() + within;
    let mut events = Vec::new();
    while Instant::now() < deadline {
        if let Some(event) = session.next_event(WAIT) {
            let done = stop(&event);
            events.push(event);
            if done {
                break;
            }
        }
    }
    events
}

/// The tail of a message's audio, sent after its `agent_response` under the message's own
/// `event_id`, opens no response: the next response opens at the first audio after the user's
/// turn, and holds only that turn's audio.
#[test]
fn audio_after_its_finished_message_opens_no_response_before_the_users_next_turn() {
    let server = AgentServer::start(Script::new(vec![
        Step::Send(fixture::audio(1, 640)),
        Step::Send(fixture::agent_response("Hello there.", 1, Some("r1"))),
        Step::Send(fixture::audio(1, 640)),
        Step::Send(fixture::user_transcript("hi", 2)),
        Step::Send(fixture::audio(3, 640)),
        Step::Send(fixture::agent_response("How can I help?", 3, Some("r3"))),
    ]));
    let mut session = open(&server, Duration::from_secs(5));
    let mut completed = 0;
    let events = events_for(&mut session, Duration::from_secs(5), |event| {
        if matches!(event, SessionEvent::ResponseCompleted(_)) {
            completed += 1;
        }
        completed == 2
    });
    let heard = events
        .iter()
        .position(|event| matches!(event, SessionEvent::InputTranscribed(_)))
        .expect("the user's turn was transcribed");
    let answer = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::OutputTranscriptDelta(delta) if delta.text == "How can I help?" => {
                Some(delta.response_id)
            }
            _ => None,
        })
        .expect("the second message's text was delivered");
    let opened = events
        .iter()
        .position(|event| matches!(event, SessionEvent::ResponseCreated(created) if created.response_id == answer))
        .expect("the answer was created");
    assert!(
        opened > heard,
        "the response carrying the answer to the user's turn was opened by audio of the message before it, ahead of the user's turn: {events:#?}"
    );
    let frames = events
        .iter()
        .filter(|event| matches!(event, SessionEvent::OutputAudio(audio) if audio.response_id == answer))
        .count();
    assert_eq!(
        frames, 1,
        "the answer holds audio of the message before it: {events:#?}"
    );
}

/// After a finished message whose audio tail came after its `agent_response`, the user's silence
/// is no stall: no response is awaited, so the idle limit does not fail the session.
#[test]
fn the_users_silence_after_a_finished_message_with_a_late_audio_tail_is_no_stall() {
    let server = AgentServer::start(Script::new(vec![
        Step::Send(fixture::audio(1, 640)),
        Step::Send(fixture::agent_response("Hello there.", 1, Some("r1"))),
        Step::Send(fixture::audio(1, 640)),
        Step::Pause(Duration::from_secs(2)),
    ]));
    let mut session = open(&server, Duration::from_millis(300));
    let events = events_for(&mut session, Duration::from_millis(1_500), |event| {
        matches!(event, SessionEvent::SpeechSessionFailed(_))
    });
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::SpeechSessionFailed(_))),
        "the session failed while the user was silent after a finished message: {events:#?}"
    );
}

/// The module doc: every `ping` is answered at once, whether or not the caller is reading. More
/// pings than the inbound queue holds, with the caller reading nothing, are each answered.
#[test]
fn every_ping_is_answered_while_the_caller_reads_nothing_beyond_the_inbound_queue() {
    const PINGS: u64 = 300;
    let steps = (1..=PINGS)
        .map(|event_id| Step::Send(fixture::ping(event_id)))
        .collect();
    let server = AgentServer::start(Script::new(steps));
    let session = open(&server, Duration::from_secs(5));
    let seen = server.wait(0, Duration::from_secs(3), |seen| {
        seen.of_type("pong").len() as u64 == PINGS
    });
    let answered = seen.of_type("pong").len() as u64;
    drop(session);
    assert_eq!(
        answered,
        PINGS,
        "pings went unanswered once {} messages waited for the caller",
        inference_elevenlabs::INBOUND_QUEUE
    );
}
