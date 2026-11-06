//! The agent conversation binding against the local fixture server of `support/agent.rs`: the
//! opening order, keepalive, the format check, barge-in, corrections, errors, cancels and drops,
//! the refusals before connecting, the message size bound, tools, and the duplex conformance
//! procedure.

#[path = "support/agent.rs"]
mod fixture;

use std::num::{NonZeroU16, NonZeroU32};
use std::time::{Duration, Instant};

use fixture::{AGENT, AgentServer, Handshake, Script, Step};
use inference_elevenlabs::agent::{AgentEndpoint, AgentError, SignedUrl};
use inference_elevenlabs::inference_duplex::conformance::{
    NativeDuplexProbe, NoNativeDuplex, native_duplex_evidence,
};
use inference_elevenlabs::inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_elevenlabs::inference_duplex::inference_tools::{
    DeclareTool, Json, SideEffectClass, Tool,
};
use inference_elevenlabs::inference_duplex::{
    NativeSpeechModel, OpenError, ResponseId, SessionConfig, SessionError, SessionEvent,
    SessionFailureKind, SpeechSessionHandle, TurnDetection, TurnTuning, Unsupported,
};
use inference_elevenlabs::{
    CLOSE_BOUND, ElevenLabsAgent, ElevenLabsAgentSession, agent_claims, agent_native_duplex_verdict,
};

const WAIT: Duration = Duration::from_millis(20);

fn pcm(rate: u32) -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(rate).unwrap(),
        channels: NonZeroU16::MIN,
    }
}

fn config() -> SessionConfig {
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
        idle_limit: Duration::from_secs(5),
    }
}

fn frame(position: u64) -> AudioFrame {
    AudioFrame {
        format: pcm(16_000),
        position: SampleClockPosition::new(position).unwrap(),
        sample_count: 320,
        data: vec![0; 640],
    }
}

fn endpoint(server: &AgentServer) -> AgentEndpoint {
    AgentEndpoint::new()
        .with_url(&server.url)
        .with_connect_timeout(Duration::from_secs(3))
        .with_metadata_timeout(Duration::from_secs(3))
        .with_read_timeout(Duration::from_secs(3))
}

fn agent(server: &AgentServer) -> ElevenLabsAgent {
    ElevenLabsAgent::public(AGENT, endpoint(server)).expect("a valid agent id")
}

fn open(server: &AgentServer) -> ElevenLabsAgentSession {
    agent(server).open(config()).expect("the session opens")
}

/// Every event until `stop` holds for one, that one included, or five seconds pass.
fn events_until(
    session: &mut ElevenLabsAgentSession,
    stop: impl Fn(&SessionEvent) -> bool,
) -> Vec<SessionEvent> {
    let deadline = Instant::now() + Duration::from_secs(5);
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

fn ended(event: &SessionEvent) -> bool {
    matches!(
        event,
        SessionEvent::SpeechSessionClosed(_) | SessionEvent::SpeechSessionFailed(_)
    )
}

/// The response ids of the `ResponseCreated` events, in order.
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

#[test]
fn the_initiation_is_sent_first_and_no_audio_precedes_the_metadata() {
    let mut script = Script::new(vec![Step::AwaitAudio(2)]);
    script.metadata_delay = Duration::from_millis(300);
    let server = AgentServer::start(script);
    let started = Instant::now();
    let mut session = open(&server);
    assert!(
        started.elapsed() >= Duration::from_millis(250),
        "the open returned before the metadata"
    );
    session.append_audio(frame(0)).unwrap();
    session.append_audio(frame(320)).unwrap();
    let seen = server.wait(0, Duration::from_secs(3), |seen| seen.audio_chunks() == 2);
    assert_eq!(seen.audio_chunks(), 2);
    assert!(seen.violations.is_empty(), "{:?}", seen.violations);
    assert_eq!(
        seen.messages[0].1["type"],
        "conversation_initiation_client_data"
    );
    assert_eq!(seen.path, "/v1/convai/conversation");
    assert_eq!(seen.query, format!("agent_id={AGENT}"));
}

#[test]
fn every_ping_is_answered_with_a_pong_carrying_its_event_id_while_the_caller_reads_nothing() {
    let server = AgentServer::start(Script::new(vec![
        Step::Send(fixture::ping(41)),
        Step::Pause(Duration::from_millis(50)),
        Step::Send(fixture::ping(42)),
    ]));
    let session = open(&server);
    let seen = server.wait(0, Duration::from_secs(3), |seen| {
        seen.of_type("pong").len() == 2
    });
    let ids: Vec<_> = seen
        .of_type("pong")
        .iter()
        .map(|pong| pong["event_id"].as_u64())
        .collect();
    assert_eq!(ids, vec![Some(41), Some(42)]);
    drop(session);
}

#[test]
fn a_format_the_agent_reports_other_than_the_sessions_is_refused_at_open() {
    for (metadata, refused, expected_input, expected_output) in [
        (
            ("pcm_16000", "pcm_24000"),
            Unsupported::OutputFormat(pcm(16_000)),
            "pcm_16000",
            "pcm_24000",
        ),
        (
            ("ulaw_8000", "pcm_16000"),
            Unsupported::InputFormat(pcm(16_000)),
            "ulaw_8000",
            "pcm_16000",
        ),
    ] {
        let mut script = Script::new(Vec::new());
        script.metadata = Some(metadata);
        let server = AgentServer::start(script);
        let mut model = agent(&server);
        let outcome = model.open(config());
        assert_eq!(outcome.err(), Some(OpenError::Unsupported(refused)));
        match model.open_error() {
            Some(AgentError::FormatMismatch { input, output }) => {
                assert_eq!(
                    (input.as_str(), output.as_str()),
                    (expected_input, expected_output)
                );
            }
            other => panic!("{other:?}"),
        }
        let seen = server.wait(0, CLOSE_BOUND * 2, |seen| seen.closed.is_some());
        assert!(
            seen.closed.is_some(),
            "the refused session stayed connected"
        );
    }
}

#[test]
fn barge_in_cuts_the_response_by_event_id_and_later_audio_opens_a_new_one() {
    let server = AgentServer::start(Script::new(vec![
        Step::Send(fixture::user_transcript("what is the weather", 4)),
        Step::Send(fixture::audio(5, 640)),
        Step::Send(fixture::audio(5, 640)),
        Step::Send(fixture::interruption(5)),
        Step::Send(fixture::audio(5, 640)),
        Step::Send(fixture::audio(6, 640)),
        Step::Send(fixture::agent_response("It is mild.", 6, Some("r2"))),
    ]));
    let mut session = open(&server);
    let events = events_until(&mut session, |event| {
        matches!(event, SessionEvent::ResponseCompleted(_))
    });
    let ids = created(&events);
    assert_eq!(ids.len(), 2, "{events:#?}");
    let (cut, next) = (ids[0], ids[1]);
    assert_ne!(cut, next);
    assert_eq!(
        audio_of(&events, cut),
        2,
        "late audio of the cut response was delivered"
    );
    assert_eq!(audio_of(&events, next), 1);
    let cancelled = events
        .iter()
        .position(|event| matches!(event, SessionEvent::ResponseCancelled(gone) if gone.response_id == cut))
        .expect("the cut response was cancelled");
    let speech = events
        .iter()
        .position(|event| matches!(event, SessionEvent::SpeechStarted(_)))
        .expect("the interruption reported speech");
    assert!(speech < cancelled);
    assert!(matches!(
        events.last(),
        Some(SessionEvent::ResponseCompleted(done)) if done.response_id == next
    ));
    assert_eq!(session.provider_response_id(next), Some("r2"));
}

#[test]
fn the_correction_lands_on_the_cut_response() {
    let server = AgentServer::start(Script::new(vec![
        Step::Send(fixture::audio(5, 640)),
        Step::Send(fixture::interruption(5)),
        Step::Send(fixture::correction(
            "The weather in Paris is mild and",
            "The weather in Paris",
            7,
            None,
        )),
        Step::Send(fixture::audio(8, 640)),
        Step::Send(fixture::agent_response("Sure.", 8, Some("r9"))),
        Step::Send(fixture::correction("Sure.", "Su", 9, Some("r9"))),
        Step::Send(fixture::ping(10)),
    ]));
    let mut session = open(&server);
    let events = events_until(&mut session, |event| {
        matches!(event, SessionEvent::ResponseCompleted(_))
    });
    let ids = created(&events);
    assert_eq!(ids.len(), 2, "{events:#?}");
    server.wait(0, Duration::from_secs(3), |seen| {
        !seen.of_type("pong").is_empty()
    });
    let _ = session.next_event(Duration::from_millis(200));
    assert_eq!(
        session.final_transcript(ids[0]),
        Some("The weather in Paris")
    );
    assert_eq!(session.final_transcript(ids[1]), Some("Su"));
}

#[test]
fn a_client_error_fails_the_session_with_its_code_and_name() {
    let server = AgentServer::start(Script::new(vec![Step::Send(fixture::client_error(
        4000,
        "invalid_message",
        "the message was not understood",
    ))]));
    let mut session = open(&server);
    let events = events_until(&mut session, ended);
    match events.last() {
        Some(SessionEvent::SpeechSessionFailed(failed)) => {
            assert_eq!(failed.failure.kind, SessionFailureKind::ProviderError);
            assert!(failed.failure.detail.contains("invalid_message"));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        session.failure_cause(),
        Some(&AgentError::Client {
            code: Some(4000),
            name: "invalid_message".to_owned(),
            message: "the message was not understood".to_owned(),
        })
    );
}

#[test]
fn a_close_mid_response_fails_the_response_and_the_session_typed() {
    let server = AgentServer::start(Script::new(vec![
        Step::Send(fixture::audio(3, 640)),
        Step::Close(1011),
    ]));
    let mut session = open(&server);
    let events = events_until(&mut session, ended);
    let ids = created(&events);
    assert_eq!(ids.len(), 1);
    assert!(
        events.iter().any(
            |event| matches!(event, SessionEvent::ResponseFailed(failed) if failed.response_id == ids[0])
        ),
        "{events:#?}"
    );
    match events.last() {
        Some(SessionEvent::SpeechSessionFailed(failed)) => {
            assert_eq!(failed.failure.kind, SessionFailureKind::TransportClosed);
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        session.failure_cause(),
        Some(AgentError::Closed {
            code: Some(1011),
            ..
        })
    ));
}

#[test]
fn a_cancel_during_the_handshake_closes_the_connection_within_the_close_bound() {
    let mut script = Script::new(Vec::new());
    script.handshake = Handshake::Stall;
    let server = AgentServer::start(script);
    let mut model = agent(&server);
    let canceller = model.canceller();
    let cancelling = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        canceller.cancel();
        Instant::now()
    });
    let outcome = model.open(config());
    let cancelled_at = cancelling.join().unwrap();
    assert!(matches!(outcome, Err(OpenError::Failed(_))), "{outcome:?}");
    assert!(
        cancelled_at.elapsed() < CLOSE_BOUND,
        "the open outlived its cancel"
    );
    assert_eq!(model.open_error(), Some(&AgentError::Cancelled));
    let seen = server.wait(0, CLOSE_BOUND, |seen| seen.closed.is_some());
    let closed = seen.closed.expect("the connection was closed");
    assert!(closed.saturating_duration_since(cancelled_at) < CLOSE_BOUND);
}

#[test]
fn a_cancel_while_awaiting_the_metadata_closes_the_connection_within_the_close_bound() {
    let mut script = Script::new(Vec::new());
    script.metadata = None;
    let server = AgentServer::start(script);
    let mut model = agent(&server);
    let canceller = model.canceller();
    let cancelling = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        canceller.cancel();
        Instant::now()
    });
    let outcome = model.open(config());
    let cancelled_at = cancelling.join().unwrap();
    assert!(matches!(outcome, Err(OpenError::Failed(_))), "{outcome:?}");
    assert_eq!(model.open_error(), Some(&AgentError::Cancelled));
    let seen = server.wait(0, CLOSE_BOUND, |seen| seen.closed.is_some());
    let closed = seen.closed.expect("the connection was closed");
    assert!(closed.saturating_duration_since(cancelled_at) < CLOSE_BOUND);
}

/// A cancel issued while no open is in progress does nothing: the opens after it, on the same
/// model, go through.
#[test]
fn a_cancel_with_no_open_in_progress_does_not_reach_a_later_open() {
    let server = AgentServer::start(Script::new(Vec::new()));
    let mut model = agent(&server);
    let canceller = model.canceller();
    canceller.cancel();
    let first = model.open(config());
    assert!(first.is_ok(), "{:?}", model.open_error());
    drop(first);
    canceller.cancel();
    canceller.cancel();
    let second = model.open(config());
    assert!(second.is_ok(), "{:?}", model.open_error());
    assert_eq!(model.open_error(), None);
}

#[test]
fn no_metadata_within_the_deadline_fails_the_open_and_closes() {
    let mut script = Script::new(Vec::new());
    script.metadata = None;
    let server = AgentServer::start(script);
    let mut model = ElevenLabsAgent::public(
        AGENT,
        endpoint(&server).with_metadata_timeout(Duration::from_millis(200)),
    )
    .unwrap();
    let started = Instant::now();
    let outcome = model.open(config());
    assert!(matches!(outcome, Err(OpenError::Failed(_))), "{outcome:?}");
    assert!(started.elapsed() < Duration::from_millis(200) + CLOSE_BOUND);
    assert!(matches!(model.open_error(), Some(AgentError::Stalled(_))));
    let seen = server.wait(0, CLOSE_BOUND, |seen| seen.closed.is_some());
    assert!(seen.closed.is_some());
}

#[test]
fn dropping_a_session_mid_stream_closes_the_connection_within_the_close_bound() {
    let server = AgentServer::start(Script::new(vec![
        Step::Send(fixture::audio(3, 640)),
        Step::Pause(Duration::from_secs(10)),
    ]));
    let mut session = open(&server);
    let events = events_until(&mut session, |event| {
        matches!(event, SessionEvent::OutputAudio(_))
    });
    assert!(!created(&events).is_empty());
    let dropped_at = Instant::now();
    drop(session);
    assert!(
        dropped_at.elapsed() < CLOSE_BOUND,
        "the drop blocked beyond the close bound"
    );
    let seen = server.wait(0, CLOSE_BOUND, |seen| seen.closed.is_some());
    let closed = seen.closed.expect("the connection was closed");
    assert!(closed.saturating_duration_since(dropped_at) < CLOSE_BOUND);
}

#[test]
fn a_closed_session_takes_nothing_more_and_closes_its_connection() {
    let server = AgentServer::start(Script::new(vec![
        Step::Send(fixture::audio(3, 640)),
        Step::Pause(Duration::from_secs(10)),
    ]));
    let mut session = open(&server);
    events_until(&mut session, |event| {
        matches!(event, SessionEvent::OutputAudio(_))
    });
    let closing = Instant::now();
    session.close().unwrap();
    assert!(
        closing.elapsed() < CLOSE_BOUND * 2,
        "the close outlived its deadline"
    );
    let events = events_until(&mut session, ended);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::ResponseCancelled(_))),
        "{events:#?}"
    );
    assert!(matches!(
        events.last(),
        Some(SessionEvent::SpeechSessionClosed(_))
    ));
    assert!(matches!(
        session.append_audio(frame(0)),
        Err(SessionError::NotOpen(_))
    ));
    assert!(session.close().is_err());
    let seen = server.wait(0, CLOSE_BOUND, |seen| seen.closed.is_some());
    assert!(seen.closed.is_some());
}

#[test]
fn a_blank_agent_id_or_signed_url_is_refused_before_connecting() {
    let server = AgentServer::start(Script::new(Vec::new()));
    for id in ["", "   ", "agent id", "agent/../x", "a?b=c"] {
        assert!(
            matches!(
                ElevenLabsAgent::public(id, endpoint(&server)),
                Err(AgentError::InvalidAgentId)
            ),
            "{id:?}"
        );
    }
    for url in ["", "  ", "\n"] {
        assert!(
            matches!(SignedUrl::new(url), Err(AgentError::InvalidSignedUrl(_))),
            "{url:?}"
        );
    }
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(server.connections(), 0);
}

#[test]
fn the_signed_url_token_is_in_no_error_and_no_debug() {
    const TOKEN: &str = "sEcReTtOkEn0123456789";
    let refusing = {
        let mut script = Script::new(Vec::new());
        script.handshake = Handshake::Refuse(403);
        AgentServer::start(script)
    };
    let closed_port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        format!("ws://{address}")
    };
    for base in [refusing.url.clone(), closed_port] {
        let url = format!("{base}/v1/convai/conversation?agent_id={AGENT}&token={TOKEN}");
        let signed = SignedUrl::new(url).unwrap();
        assert!(!format!("{signed:?}").contains(TOKEN));
        let mut model = ElevenLabsAgent::signed(
            signed,
            AgentEndpoint::new().with_connect_timeout(Duration::from_secs(2)),
        );
        assert!(!format!("{model:?}").contains(TOKEN));
        let outcome = model.open(config());
        let error = outcome.expect_err("the open fails");
        for rendered in [
            format!("{error}"),
            format!("{error:?}"),
            format!("{:?}", model.open_error()),
            model
                .open_error()
                .map(ToString::to_string)
                .unwrap_or_default(),
            format!("{model:?}"),
        ] {
            assert!(!rendered.contains(TOKEN), "{rendered}");
        }
    }
    let seen = refusing.wait(0, Duration::from_secs(1), |seen| !seen.query.is_empty());
    assert!(
        seen.query.contains(TOKEN),
        "the token did not reach the server"
    );
    for refused in [
        format!("ws://api.elevenlabs.io/v1/convai/conversation?agent_id=a&token={TOKEN}"),
        format!("http://127.0.0.1:9/v1/convai/conversation?token={TOKEN}"),
    ] {
        let error = SignedUrl::new(refused).expect_err("refused");
        assert!(!format!("{error} {error:?}").contains(TOKEN));
    }
}

#[test]
fn plaintext_is_refused_to_a_non_loopback_host() {
    for url in [
        "ws://api.elevenlabs.io",
        "ws://10.0.0.1:9",
        "ws://localhost.example.com",
        "ws://127.0.0.1@evil.example",
        "ws://0.0.0.0:9",
        "WS://127.0.0.1:9",
        "http://127.0.0.1:9",
    ] {
        assert!(
            matches!(
                ElevenLabsAgent::public(AGENT, AgentEndpoint::new().with_url(url)),
                Err(AgentError::InvalidUrl(_))
            ),
            "{url}"
        );
    }
    for url in [
        "wss://api.elevenlabs.io",
        "wss://api.eu.residency.elevenlabs.io",
        "ws://127.0.0.1:9",
        "ws://[::1]:9",
        "ws://localhost:9",
    ] {
        assert!(
            ElevenLabsAgent::public(AGENT, AgentEndpoint::new().with_url(url)).is_ok(),
            "{url}"
        );
    }
}

#[test]
fn a_message_beyond_the_size_bound_fails_the_session() {
    let server = AgentServer::start(Script::new(vec![Step::Text(format!(
        "{{\"type\":\"vad_score\",\"pad\":\"{}\"}}",
        "x".repeat(2 << 20)
    ))]));
    let mut session = open(&server);
    let events = events_until(&mut session, ended);
    match events.last() {
        Some(SessionEvent::SpeechSessionFailed(failed)) => {
            assert_eq!(failed.failure.kind, SessionFailureKind::ProviderError);
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        session.failure_cause(),
        Some(AgentError::Oversize(_))
    ));
}

#[test]
fn a_silent_connection_fails_the_session_at_its_read_deadline() {
    let server = AgentServer::start(Script::new(vec![Step::Pause(Duration::from_secs(10))]));
    let mut model = ElevenLabsAgent::public(
        AGENT,
        endpoint(&server).with_read_timeout(Duration::from_millis(300)),
    )
    .unwrap();
    let mut session = model.open(config()).unwrap();
    let started = Instant::now();
    let events = events_until(&mut session, ended);
    assert!(started.elapsed() < Duration::from_secs(2));
    match events.last() {
        Some(SessionEvent::SpeechSessionFailed(failed)) => {
            assert_eq!(failed.failure.kind, SessionFailureKind::Stalled);
        }
        other => panic!("{other:?}"),
    }
}

fn time_tool() -> Tool {
    Tool::declare(DeclareTool {
        name: "local_time".to_owned(),
        description: "The local time.".to_owned(),
        schema: serde_json::json!({"type": "object"}),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0
}

#[test]
fn a_tool_call_the_session_offers_is_proposed_and_its_result_sent() {
    let server = AgentServer::start(Script::new(vec![
        Step::Send(fixture::tool_call("local_time", "call_1", true)),
        Step::AwaitType("client_tool_result"),
        Step::Send(fixture::audio(91, 640)),
        Step::Send(fixture::agent_response("It is noon.", 91, None)),
    ]));
    let mut model = agent(&server);
    let mut session_config = config();
    session_config.tools = vec![time_tool()];
    let mut session = model.open(session_config).unwrap();
    let events = events_until(&mut session, |event| {
        matches!(event, SessionEvent::SpeechToolCallProposed(_))
    });
    let Some(SessionEvent::SpeechToolCallProposed(proposed)) = events.last() else {
        panic!("{events:#?}");
    };
    let call_id = proposed.call_id.clone();
    assert_eq!(call_id.as_str(), "call_1");
    session
        .submit_tool_result(&call_id, Json::String("12:00".to_owned()))
        .unwrap();
    let events = events_until(&mut session, |event| {
        matches!(event, SessionEvent::ResponseCompleted(_))
    });
    assert!(matches!(
        events.last(),
        Some(SessionEvent::ResponseCompleted(_))
    ));
    let seen = server.seen(0);
    let results = seen.of_type("client_tool_result");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["tool_call_id"], "call_1");
    assert_eq!(results[0]["result"], "12:00");
    assert_eq!(results[0]["is_error"], false);
}

#[test]
fn a_tool_call_the_session_does_not_offer_fails_the_session_naming_it() {
    let server = AgentServer::start(Script::new(vec![
        Step::Send(fixture::tool_call("open_door", "call_2", false)),
        Step::Send(fixture::tool_call("open_door", "call_3", true)),
    ]));
    let mut session = open(&server);
    let events = events_until(&mut session, ended);
    match events.last() {
        Some(SessionEvent::SpeechSessionFailed(failed)) => {
            assert!(failed.failure.detail.contains("open_door"));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        session.failure_cause(),
        Some(&AgentError::ToolNotOffered {
            name: "open_door".to_owned()
        })
    );
}

#[test]
fn a_finished_message_completes_its_response_with_transcripts_in_order() {
    let server = AgentServer::start(Script::new(vec![
        Step::Send(fixture::audio(1, 640)),
        Step::Send(fixture::agent_response("Hello there.", 1, Some("r1"))),
        Step::Send(fixture::user_transcript("hi", 2)),
        Step::Send(fixture::audio(3, 641)),
        Step::Send(fixture::audio(3, 641)),
        Step::Send(fixture::agent_response("How can I help?", 3, None)),
    ]));
    let mut session = open(&server);
    let mut events = events_until(&mut session, |event| {
        matches!(event, SessionEvent::ResponseCompleted(_))
    });
    events.extend(events_until(&mut session, |event| {
        matches!(event, SessionEvent::ResponseCompleted(_))
    }));
    let ids = created(&events);
    assert_eq!(ids.len(), 2, "{events:#?}");
    let transcripts: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::OutputTranscriptDelta(delta) => {
                Some((delta.response_id, delta.text.clone()))
            }
            SessionEvent::InputTranscribed(heard) => {
                Some((ids[0], format!("user: {}", heard.text)))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        transcripts,
        vec![
            (ids[0], "Hello there.".to_owned()),
            (ids[0], "user: hi".to_owned()),
            (ids[1], "How can I help?".to_owned()),
        ]
    );
    let samples: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::OutputAudio(audio) if audio.response_id == ids[1] => {
                Some((audio.frame.position.get(), audio.frame.sample_count))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        samples,
        vec![(0, 320), (320, 321)],
        "a half sample was not carried over"
    );
    assert_eq!(session.provider_response_id(ids[0]), Some("r1"));
    assert_eq!(session.provider_response_id(ids[1]), None);
}

/// The probe: a question, then during the answer a place; the fixture interrupts the answer as
/// the hosted agent does on speech over it, and answers in a new response.
fn probe() -> NativeDuplexProbe {
    NativeDuplexProbe {
        config: config(),
        opening: vec![frame(0), frame(320)],
        during: vec![frame(640), frame(960)],
        marker: "lisbon".to_owned(),
        wait: WAIT,
        patience: 100,
    }
}

#[test]
fn the_native_duplex_procedure_runs_over_the_fixture_and_agrees_with_the_claims() {
    let server = AgentServer::start(Script::new(vec![
        Step::AwaitAudio(2),
        Step::Send(fixture::user_transcript("what is the weather", 1)),
        Step::Send(fixture::audio(2, 640)),
        Step::Send(fixture::audio(2, 640)),
        Step::AwaitAudio(4),
        Step::Send(fixture::interruption(2)),
        Step::Send(fixture::user_transcript("in Lisbon", 3)),
        Step::Send(fixture::audio(4, 640)),
        Step::Send(fixture::agent_response("It is mild in Lisbon.", 4, None)),
        Step::Pause(Duration::from_secs(10)),
    ]));
    let mut model = agent(&server);
    let outcome = native_duplex_evidence(&mut model, &probe());
    assert!(
        matches!(outcome, Err(NoNativeDuplex::Restarted { first, second }) if first != second),
        "{outcome:?}"
    );
    assert_eq!(agent_native_duplex_verdict(&outcome), Ok(true));
    assert!(!agent_claims().contains(
        &inference_elevenlabs::inference_duplex::inference_core::model::Capability::NativeDuplex
    ));
    let seen = server.wait(0, CLOSE_BOUND * 2, |seen| seen.closed.is_some());
    assert!(
        seen.closed.is_some(),
        "the procedure's close left the connection open"
    );
    assert!(seen.violations.is_empty(), "{:?}", seen.violations);
}
