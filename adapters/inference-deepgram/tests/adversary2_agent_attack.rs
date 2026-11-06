//! Adversary pass 1 on the Voice Agent binding: the open canceller, a flood the caller does not
//! read, reply boundaries, barge-in, the size bound and a silent agent after a warning, over the
//! local fixture server of `support/voice_agent.rs` and one raw server of its own.
#![cfg(feature = "duplex")]

#[path = "support/voice_agent.rs"]
mod fixture;

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fixture::{AgentServer, FixedKey, KEY, Script, Step};
use futures_util::{SinkExt, StreamExt};
use inference_deepgram::agent::{AgentConfig, MAX_MESSAGE_SIZE};
use inference_deepgram::inference_duplex::inference_audio::{AudioEncoding, AudioFormat};
use inference_deepgram::inference_duplex::inference_tools::Json;
use inference_deepgram::inference_duplex::{
    NativeSpeechModel, ResponseId, SessionConfig, SessionError, SessionEvent, SpeechCallId,
    SpeechSessionHandle, TurnDetection, TurnTuning,
};
use inference_deepgram::{AgentEndpoint, DeepgramAgent, DeepgramAgentSession, DeepgramError};
use serde_json::json;
use tokio_tungstenite::tungstenite::{Bytes, Message};

const WAIT: Duration = Duration::from_millis(20);

fn pcm(rate: u32) -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(rate).unwrap(),
        channels: NonZeroU16::MIN,
    }
}

fn config(idle_limit: Duration) -> SessionConfig {
    SessionConfig {
        voice: String::new(),
        instructions: String::new(),
        input_format: pcm(16_000),
        output_format: pcm(16_000),
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools: Vec::new(),
        idle_limit,
    }
}

fn endpoint(url: &str) -> AgentEndpoint {
    AgentEndpoint::new()
        .with_url(url)
        .with_open_timeout(Duration::from_secs(3))
        .with_read_timeout(Duration::from_secs(3))
}

fn agent(url: &str) -> DeepgramAgent {
    DeepgramAgent::new(
        endpoint(url),
        AgentConfig::new(),
        Arc::new(FixedKey(KEY)),
        "deepgram-api-key",
    )
    .expect("the fixture configuration is valid")
}

fn open(server: &AgentServer, idle_limit: Duration) -> DeepgramAgentSession {
    agent(&server.url)
        .open(config(idle_limit))
        .expect("the session opens")
}

/// Every event for `within`, or until the session ends.
fn events_for(session: &mut DeepgramAgentSession, within: Duration) -> Vec<SessionEvent> {
    let deadline = Instant::now() + within;
    let mut events = Vec::new();
    while Instant::now() < deadline {
        if let Some(event) = session.next_event(WAIT) {
            let end = ended(&event);
            events.push(event);
            if end {
                break;
            }
        }
    }
    events
}

fn ended(event: &SessionEvent) -> bool {
    matches!(
        event,
        SessionEvent::SpeechSessionClosed(_) | SessionEvent::SpeechSessionFailed(_)
    )
}

fn created(events: &[SessionEvent]) -> Vec<ResponseId> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ResponseCreated(created) => Some(created.response_id),
            _ => None,
        })
        .collect()
}

fn audio_of(events: &[SessionEvent], response_id: ResponseId) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, SessionEvent::OutputAudio(audio) if audio.response_id == response_id))
        .count()
}

fn said(events: &[SessionEvent], response_id: ResponseId) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::OutputTranscriptDelta(delta) if delta.response_id == response_id => {
                Some(delta.text.as_str())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("|")
}

/// A cancel issued while no open is in progress must not cancel a later, unrelated open.
#[test]
fn a_cancel_with_no_open_in_progress_does_not_cancel_the_next_open() {
    let server = AgentServer::start(Script::new(Vec::new()));
    let mut model = agent(&server.url);
    model.canceller().cancel();
    let outcome = model.open(config(Duration::from_secs(5)));
    assert!(
        outcome.is_ok(),
        "a cancel issued before any open cancelled the next one: {outcome:?}, {:?}",
        model.open_error()
    );
}

/// A watchdog that cancels just after an open succeeded (the race every timeout-driven caller
/// has) must not cancel the caller's next open.
#[test]
fn a_cancel_racing_a_finished_open_does_not_cancel_the_next_open() {
    let server = AgentServer::start(Script::new(Vec::new()));
    let mut model = agent(&server.url);
    let canceller = model.canceller();
    let first = model
        .open(config(Duration::from_secs(5)))
        .expect("the first session opens");
    canceller.cancel();
    drop(first);
    let second = model.open(config(Duration::from_secs(5)));
    assert!(
        second.is_ok(),
        "a cancel that came after the first open finished cancelled the second: {second:?}, {:?}",
        model.open_error()
    );
}

/// A raw Voice Agent server: `Welcome`, `SettingsApplied` after `Settings`, then `flood`
/// `AgentThinking` messages and a WebSocket ping. It reports whether the pong came back within
/// `within`.
fn ping_after_flood(flood: usize, within: Duration) -> (String, std::sync::mpsc::Receiver<bool>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    listener.set_nonblocking(true).expect("non-blocking");
    let url = format!("ws://{}", listener.local_addr().expect("an address"));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).expect("a listener");
            let Ok(Ok((stream, _))) =
                tokio::time::timeout(Duration::from_secs(5), listener.accept()).await
            else {
                let _ = tx.send(false);
                return;
            };
            let Ok(mut socket) = tokio_tungstenite::accept_async(stream).await else {
                let _ = tx.send(false);
                return;
            };
            let welcome = json!({"type": "Welcome", "request_id": "adv"}).to_string();
            if socket.send(Message::text(welcome)).await.is_err() {
                let _ = tx.send(false);
                return;
            }
            loop {
                match tokio::time::timeout(Duration::from_secs(3), socket.next()).await {
                    Ok(Some(Ok(Message::Text(text)))) if text.as_str().contains("Settings") => {
                        break;
                    }
                    Ok(Some(Ok(_))) => {}
                    _ => {
                        let _ = tx.send(false);
                        return;
                    }
                }
            }
            let applied = json!({"type": "SettingsApplied"}).to_string();
            let _ = socket.send(Message::text(applied)).await;
            for n in 0..flood {
                let thinking = json!({"type": "AgentThinking", "content": format!("t{n}")});
                let _ = socket.send(Message::text(thinking.to_string())).await;
            }
            let _ = socket
                .send(Message::Ping(Bytes::from_static(b"adversary-ping")))
                .await;
            let deadline = tokio::time::Instant::now() + within;
            let mut ponged = false;
            while let Ok(Some(Ok(message))) =
                tokio::time::timeout_at(deadline, socket.next()).await
            {
                if matches!(&message, Message::Pong(payload) if payload.as_ref() == b"adversary-ping")
                {
                    ponged = true;
                    break;
                }
            }
            let _ = tx.send(ponged);
            tokio::time::sleep(Duration::from_millis(200)).await;
        });
    });
    (url, rx)
}

/// RFC 6455 asks for a pong to every ping. A caller that reads no events for a moment, while
/// the agent sends more than the inbound queue holds, must not leave the server's ping
/// unanswered: the connection task answers control frames without the caller.
#[test]
fn a_server_ping_is_answered_while_the_caller_reads_nothing() {
    let (url, ponged) = ping_after_flood(100, Duration::from_secs(2));
    let session = agent(&url)
        .open(config(Duration::from_secs(30)))
        .expect("the session opens");
    let answered = ponged
        .recv_timeout(Duration::from_secs(6))
        .expect("the raw server reported");
    drop(session);
    assert!(
        answered,
        "the server's ping, sent after 100 messages the caller did not read, got no pong within 2 s"
    );
}

/// The same flood with the fixture server: the `KeepAlive` still goes out while the caller
/// reads nothing.
#[test]
fn a_keepalive_goes_out_while_the_caller_reads_nothing() {
    let steps = (0..100)
        .map(|n| Step::Send(json!({"type": "AgentThinking", "content": format!("t{n}")})))
        .collect();
    let server = AgentServer::start(Script::new(steps));
    let session = DeepgramAgent::new(
        endpoint(&server.url).with_keepalive_interval(Duration::from_millis(300)),
        AgentConfig::new(),
        Arc::new(FixedKey(KEY)),
        "deepgram-api-key",
    )
    .unwrap()
    .open(config(Duration::from_secs(30)))
    .expect("the session opens");
    let seen = server.wait(0, Duration::from_secs(3), |seen| {
        !seen.of_type("KeepAlive").is_empty()
    });
    drop(session);
    assert!(
        !seen.of_type("KeepAlive").is_empty(),
        "no KeepAlive while the inbound queue was full"
    );
}

/// Reply boundaries without a reply id: `AgentStartedSpeaking` twice with no audio between is
/// one reply; `AgentAudioDone` with no reply opens nothing; two assistant texts before one reply
/// both land on it; a reply's frames stay on it.
#[test]
fn reply_boundaries_hold_without_a_reply_id() {
    let server = AgentServer::start(Script::new(vec![
        Step::Send(fixture::agent_audio_done()),
        Step::Send(fixture::conversation_text("assistant", "One.")),
        Step::Send(fixture::conversation_text("assistant", "Two.")),
        Step::Send(fixture::agent_started_speaking()),
        Step::Send(json!({"type": "AgentStartedSpeaking", "total_latency": 2.5})),
        Step::Binary(vec![3; 640]),
        Step::Binary(vec![3; 640]),
        Step::Send(fixture::agent_audio_done()),
        Step::Send(fixture::agent_audio_done()),
    ]));
    let mut session = open(&server, Duration::from_secs(5));
    let events = events_for(&mut session, Duration::from_millis(800));
    let ids = created(&events);
    assert_eq!(ids.len(), 1, "{events:#?}");
    assert_eq!(audio_of(&events, ids[0]), 2);
    assert_eq!(said(&events, ids[0]), "One. Two.");
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, SessionEvent::ResponseCompleted(_)))
            .count(),
        1
    );
    assert!(!events.iter().any(ended), "{events:#?}");
}

/// Barge-in twice during one reply, with no `AgentAudioDone` after the cut: one cancel, the
/// late frames dropped and counted, and the next reply complete and whole.
#[test]
fn a_double_barge_in_cancels_once_and_loses_nothing_of_the_next_reply() {
    let mut steps = vec![
        Step::Send(fixture::agent_started_speaking()),
        Step::Binary(vec![7; 640]),
        Step::Send(fixture::user_started_speaking()),
        Step::Binary(vec![7; 640]),
        Step::Send(fixture::user_started_speaking()),
        Step::Binary(vec![7; 640]),
    ];
    steps.extend(fixture::turn("again", "Here.", 3, 640));
    let server = AgentServer::start(Script::new(steps));
    let mut session = open(&server, Duration::from_secs(5));
    let events = events_for(&mut session, Duration::from_millis(800));
    let ids = created(&events);
    assert_eq!(ids.len(), 2, "{events:#?}");
    assert_eq!(audio_of(&events, ids[0]), 1);
    assert_eq!(audio_of(&events, ids[1]), 3);
    assert_eq!(said(&events, ids[1]), "Here.");
    assert_eq!(session.late_audio_dropped(), 2);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, SessionEvent::ResponseCancelled(_)))
            .count(),
        1
    );
    assert!(!events.iter().any(ended), "{events:#?}");
}

/// A message and a frame of exactly the size bound are read; the bound refuses only beyond it.
#[test]
fn a_message_and_a_frame_at_the_size_bound_are_read() {
    let prefix = "{\"type\":\"History\",\"pad\":\"";
    let suffix = "\"}";
    let pad = MAX_MESSAGE_SIZE - prefix.len() - suffix.len();
    let text = format!("{prefix}{}{suffix}", "x".repeat(pad));
    assert_eq!(text.len(), MAX_MESSAGE_SIZE);
    let server = AgentServer::start(Script::new(vec![
        Step::Text(text),
        Step::Send(fixture::agent_started_speaking()),
        Step::Binary(vec![0; MAX_MESSAGE_SIZE]),
        Step::Send(fixture::agent_audio_done()),
    ]));
    let mut session = open(&server, Duration::from_secs(5));
    let events = events_for(&mut session, Duration::from_secs(2));
    assert!(!events.iter().any(ended), "{:?}", session.failure_cause());
    let samples: u64 = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::OutputAudio(audio) => Some(u64::from(audio.frame.sample_count)),
            _ => None,
        })
        .sum();
    assert_eq!(samples, (MAX_MESSAGE_SIZE / 2) as u64);
}

/// A text that is not JSON is counted and not fatal.
#[test]
fn a_non_json_text_is_counted_and_not_fatal() {
    let mut steps = vec![Step::Text("not json {".to_owned())];
    steps.extend(fixture::turn("hi", "Hello.", 1, 640));
    let server = AgentServer::start(Script::new(steps));
    let mut session = open(&server, Duration::from_secs(5));
    let events = events_for(&mut session, Duration::from_millis(800));
    assert!(!events.iter().any(ended), "{events:#?}");
    assert_eq!(created(&events).len(), 1);
    assert_eq!(session.ledger().unreadable_frames(), 1);
}

/// A result for a call the server never made is refused and sends nothing.
#[test]
fn a_result_for_an_unknown_call_is_refused_and_not_sent() {
    let server = AgentServer::start(Script::new(Vec::new()));
    let mut session = open(&server, Duration::from_secs(5));
    let refused = session.submit_tool_result(
        &SpeechCallId::new("never-made".to_owned()),
        Json::String("x".to_owned()),
    );
    assert!(
        matches!(refused, Err(SessionError::UnknownCall { .. })),
        "{refused:?}"
    );
    std::thread::sleep(Duration::from_millis(200));
    assert!(server.seen(0).of_type("FunctionCallResponse").is_empty());
}

/// Audio of a reply that arrives after its `AgentAudioDone` (the ElevenLabs sibling's defect,
/// put to this binding): it must not open a response that stays in progress while the agent
/// waits for the user, so a healthy, silent session is not failed as stalled.
#[test]
fn audio_after_agent_audio_done_does_not_leave_a_response_in_progress() {
    let server = AgentServer::start(Script::new(vec![
        Step::Send(fixture::agent_started_speaking()),
        Step::Binary(vec![5; 640]),
        Step::Send(fixture::agent_audio_done()),
        Step::Binary(vec![5; 640]),
    ]));
    let mut session = open(&server, Duration::from_secs(1));
    let events = events_for(&mut session, Duration::from_millis(2500));
    assert!(
        !events.iter().any(ended),
        "a stray response kept the session awaiting a silent agent: {:?}, {:?}",
        events.last(),
        session.failure_cause()
    );
    assert_eq!(session.ledger().in_progress(), None);
}

/// A `THINK_REQUEST_FAILED` warning is documented as not fatal, and no reply follows the user's
/// turn it failed. The session must not then fail as stalled while the agent waits for the user.
#[test]
fn a_think_request_failed_warning_does_not_end_in_a_stall() {
    let server = AgentServer::start(Script::new(vec![
        Step::Send(fixture::user_started_speaking()),
        Step::Send(fixture::conversation_text("user", "hello")),
        Step::Send(fixture::warning(
            "THINK_REQUEST_FAILED",
            "the think provider failed",
        )),
    ]));
    let mut session = open(&server, Duration::from_secs(1));
    let events = events_for(&mut session, Duration::from_millis(2500));
    assert_eq!(session.warnings_seen(), 1);
    assert!(
        !events.iter().any(ended),
        "a non-fatal warning ended in a failed session: {:?}, {:?}",
        events.last(),
        session.failure_cause()
    );
    assert!(!matches!(
        session.failure_cause(),
        Some(DeepgramError::Stalled(_))
    ));
}
