//! The Voice Agent binding against the local fixture server of `support/voice_agent.rs`: the
//! opening order, the settings it sends, replies, barge-in, warnings and errors, keepalive,
//! cancels, closes and drops, the key, the refusals before connecting, the message size bound,
//! functions, and the duplex conformance procedure.

#[path = "support/voice_agent.rs"]
mod fixture;

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fixture::{AgentServer, Applied, FixedKey, Handshake, KEY, NoKey, Script, Step};
use inference_deepgram::agent::AgentConfig;
use inference_deepgram::inference_duplex::conformance::{
    NativeDuplexProbe, NoNativeDuplex, native_duplex_evidence,
};
use inference_deepgram::inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_deepgram::inference_duplex::inference_core::model::Capability;
use inference_deepgram::inference_duplex::inference_tools::{
    DeclareTool, Json, SideEffectClass, Tool,
};
use inference_deepgram::inference_duplex::{
    NativeSpeechModel, OpenError, ResponseId, SessionConfig, SessionError, SessionEvent,
    SessionFailureKind, SpeechSessionHandle, TurnDetection, TurnTuning,
};
use inference_deepgram::{
    AgentEndpoint, CLOSE_BOUND, ConfigError, CredentialError, CredentialSource, DeepgramAgent,
    DeepgramAgentSession, DeepgramError, agent_claims, agent_native_duplex_verdict,
};

const WAIT: Duration = Duration::from_millis(20);

fn format(encoding: AudioEncoding, rate: u32) -> AudioFormat {
    AudioFormat {
        encoding,
        sample_rate: NonZeroU32::new(rate).unwrap(),
        channels: NonZeroU16::MIN,
    }
}

fn pcm(rate: u32) -> AudioFormat {
    format(AudioEncoding::PcmS16le, rate)
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
        .with_open_timeout(Duration::from_secs(3))
        .with_read_timeout(Duration::from_secs(3))
}

fn agent_with(endpoint: AgentEndpoint, source: Arc<dyn CredentialSource>) -> DeepgramAgent {
    DeepgramAgent::new(endpoint, AgentConfig::new(), source, "deepgram-api-key")
        .expect("the fixture configuration is valid")
}

fn agent(server: &AgentServer) -> DeepgramAgent {
    agent_with(endpoint(server), Arc::new(FixedKey(KEY)))
}

fn open(server: &AgentServer) -> DeepgramAgentSession {
    agent(server).open(config()).expect("the session opens")
}

/// Every event until `stop` holds for one, that one included, or five seconds pass.
fn events_until(
    session: &mut DeepgramAgentSession,
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

fn completed(event: &SessionEvent) -> bool {
    matches!(event, SessionEvent::ResponseCompleted(_))
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

fn said(events: &[SessionEvent], response_id: ResponseId) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::OutputTranscriptDelta(delta) if delta.response_id == response_id => {
                Some(delta.text.as_str())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn settings_go_first_and_once_and_no_audio_precedes_settings_applied() {
    let mut script = Script::new(vec![Step::AwaitAudio(2)]);
    script.welcome_delay = Duration::from_millis(150);
    script.applied = Applied::After(Duration::from_millis(300));
    let server = AgentServer::start(script);
    let started = Instant::now();
    let mut session = open(&server);
    assert!(
        started.elapsed() >= Duration::from_millis(400),
        "the open returned before SettingsApplied"
    );
    session.append_audio(frame(0)).unwrap();
    session.append_audio(frame(320)).unwrap();
    let seen = server.wait(0, Duration::from_secs(3), |seen| seen.audio.len() == 2);
    assert_eq!(seen.audio.len(), 2);
    assert!(seen.audio.iter().all(|(_, bytes)| *bytes == 640));
    assert!(seen.violations.is_empty(), "{:?}", seen.violations);
    assert_eq!(seen.messages[0].1["type"], "Settings");
    assert_eq!(seen.of_type("Settings").len(), 1);
    assert_eq!(seen.path, "/v1/agent/converse");
    assert_eq!(
        seen.authorization.as_deref(),
        Some(&*format!("Token {KEY}"))
    );
    assert_eq!(session.request_id(), Some("fixture-request-0001"));
}

#[test]
fn the_settings_encode_the_session_formats_and_the_agent_configuration() {
    let cases = [
        (
            format(AudioEncoding::PcmMulaw, 8_000),
            format(AudioEncoding::PcmMulaw, 8_000),
            ("mulaw", 8_000),
            ("mulaw", 8_000),
        ),
        (
            pcm(24_000),
            pcm(24_000),
            ("linear16", 24_000),
            ("linear16", 24_000),
        ),
        (
            format(AudioEncoding::PcmAlaw, 8_000),
            pcm(16_000),
            ("alaw", 8_000),
            ("linear16", 16_000),
        ),
    ];
    for (input, output, (input_encoding, input_rate), (output_encoding, output_rate)) in cases {
        let server = AgentServer::start(Script::new(Vec::new()));
        let mut session_config = config();
        session_config.input_format = input;
        session_config.output_format = output;
        session_config.instructions = "Answer briefly.".to_owned();
        let mut model = DeepgramAgent::new(
            endpoint(&server),
            AgentConfig::new().with_greeting("Hello."),
            Arc::new(FixedKey(KEY)),
            "deepgram-api-key",
        )
        .unwrap();
        let session = model.open(session_config).expect("the session opens");
        let seen = server.wait(0, Duration::from_secs(3), |seen| !seen.messages.is_empty());
        let settings = &seen.of_type("Settings")[0];
        assert_eq!(settings["audio"]["input"]["encoding"], input_encoding);
        assert_eq!(settings["audio"]["input"]["sample_rate"], input_rate);
        assert_eq!(settings["audio"]["output"]["encoding"], output_encoding);
        assert_eq!(settings["audio"]["output"]["sample_rate"], output_rate);
        assert_eq!(settings["audio"]["output"]["container"], "none");
        assert_eq!(settings["agent"]["listen"]["provider"]["type"], "deepgram");
        assert!(settings["agent"]["listen"]["provider"]["model"].is_string());
        assert!(settings["agent"]["think"]["provider"]["type"].is_string());
        assert!(settings["agent"]["think"]["provider"]["model"].is_string());
        assert_eq!(settings["agent"]["think"]["prompt"], "Answer briefly.");
        assert!(settings["agent"]["speak"]["provider"]["model"].is_string());
        assert_eq!(settings["agent"]["greeting"], "Hello.");
        assert!(seen.violations.is_empty(), "{:?}", seen.violations);
        drop(session);
    }
    let server = AgentServer::start(Script::new(Vec::new()));
    let session = open(&server);
    let seen = server.wait(0, Duration::from_secs(3), |seen| !seen.messages.is_empty());
    assert!(
        seen.of_type("Settings")[0]["agent"]
            .get("greeting")
            .is_none(),
        "a greeting that was not configured was sent"
    );
    drop(session);
}

#[test]
fn a_formats_the_voice_agent_does_not_take_is_refused_at_open() {
    let server = AgentServer::start(Script::new(Vec::new()));
    let mut model = agent(&server);
    let mut session_config = config();
    session_config.input_format = format(AudioEncoding::PcmMulaw, 16_000);
    assert!(matches!(
        model.open(session_config),
        Err(OpenError::Unsupported(_))
    ));
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(server.connections(), 0);
}

#[test]
fn a_replys_frames_land_on_one_response_with_its_transcripts() {
    let mut steps = fixture::turn("what is the weather", "It is mild.", 2, 641);
    steps.push(Step::Send(fixture::conversation_text(
        "assistant",
        "Anything else?",
    )));
    steps.push(Step::Binary(vec![1; 640]));
    steps.push(Step::Send(fixture::agent_audio_done()));
    let server = AgentServer::start(Script::new(steps));
    let mut session = open(&server);
    let mut events = events_until(&mut session, completed);
    events.extend(events_until(&mut session, completed));
    let ids = created(&events);
    assert_eq!(ids.len(), 2, "{events:#?}");
    assert_eq!(said(&events, ids[0]), "It is mild.");
    assert_eq!(said(&events, ids[1]), "Anything else?");
    let heard: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::InputTranscribed(heard) => Some(heard.text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(heard, vec!["what is the weather".to_owned()]);
    let samples: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::OutputAudio(audio) if audio.response_id == ids[0] => {
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
    assert_eq!(audio_of(&events, ids[1]), 1);
    let latency = session.latency(ids[0]).expect("the latencies were read");
    assert_eq!(latency.total, Some(1.2));
    assert_eq!(latency.tts, Some(0.3));
    assert_eq!(latency.ttt, Some(0.6));
    assert_eq!(session.final_transcript(ids[0]), Some("It is mild."));
}

fn barge_in(audio_done_after_cut: bool) {
    let mut steps = vec![
        Step::Send(fixture::conversation_text(
            "assistant",
            "The weather in Paris",
        )),
        Step::Send(fixture::agent_started_speaking()),
        Step::Binary(vec![7; 640]),
        Step::Pause(Duration::from_millis(50)),
        Step::Send(fixture::user_started_speaking()),
        Step::Binary(vec![7; 640]),
        Step::Binary(vec![7; 640]),
    ];
    if audio_done_after_cut {
        steps.push(Step::Send(fixture::agent_audio_done()));
    }
    steps.extend([
        Step::Send(fixture::conversation_text("user", "and Lisbon")),
        Step::Send(fixture::conversation_text("assistant", "Lisbon is warm.")),
        Step::Send(fixture::agent_started_speaking()),
        Step::Binary(vec![9; 640]),
        Step::Send(fixture::agent_audio_done()),
    ]);
    let server = AgentServer::start(Script::new(steps));
    let mut session = open(&server);
    let events = events_until(&mut session, completed);
    let ids = created(&events);
    assert_eq!(ids.len(), 2, "{audio_done_after_cut}: {events:#?}");
    let (cut, next) = (ids[0], ids[1]);
    assert_eq!(
        audio_of(&events, cut),
        1,
        "late audio of the cut reply was delivered"
    );
    assert_eq!(audio_of(&events, next), 1);
    assert_eq!(session.late_audio_dropped(), 2);
    let cancelled = events
        .iter()
        .position(|event| matches!(event, SessionEvent::ResponseCancelled(gone) if gone.response_id == cut))
        .expect("the cut reply was cancelled");
    let speech = events
        .iter()
        .position(|event| matches!(event, SessionEvent::SpeechStarted(_)))
        .expect("the barge-in reported speech");
    assert!(speech < cancelled);
    assert_eq!(said(&events, cut), "The weather in Paris");
    assert_eq!(said(&events, next), "Lisbon is warm.");
    assert!(matches!(
        events.last(),
        Some(SessionEvent::ResponseCompleted(done)) if done.response_id == next
    ));
}

#[test]
fn barge_in_drops_the_rest_of_the_cut_reply_and_the_next_reply_is_a_new_response() {
    barge_in(false);
}

#[test]
fn barge_in_holds_when_agent_audio_done_follows_the_cut_reply() {
    barge_in(true);
}

#[test]
fn a_greeting_without_agent_started_speaking_is_a_response() {
    let server = AgentServer::start(Script::new(vec![
        Step::Send(fixture::conversation_text("assistant", "Hello.")),
        Step::Binary(vec![1; 640]),
        Step::Binary(vec![1; 640]),
        Step::Send(fixture::agent_audio_done()),
    ]));
    let mut session = open(&server);
    let events = events_until(&mut session, completed);
    let ids = created(&events);
    assert_eq!(ids.len(), 1, "{events:#?}");
    assert_eq!(audio_of(&events, ids[0]), 2);
    assert_eq!(said(&events, ids[0]), "Hello.");
}

#[test]
fn a_warning_is_recorded_and_not_fatal() {
    let mut steps = vec![Step::Send(fixture::warning(
        "SLOW_THINK_REQUEST",
        "the model is slow",
    ))];
    steps.extend(fixture::turn("hi", "Hello.", 1, 640));
    let server = AgentServer::start(Script::new(steps));
    let mut session = open(&server);
    let events = events_until(&mut session, completed);
    assert!(events.last().is_some_and(completed), "{events:#?}");
    assert!(!events.iter().any(ended));
    let warnings = session.warnings();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].code.as_deref(), Some("SLOW_THINK_REQUEST"));
    assert_eq!(warnings[0].description, "the model is slow");
    assert!(session.failure_cause().is_none());
}

#[test]
fn an_error_fails_the_session_typed_with_its_code() {
    let server = AgentServer::start(Script::new(vec![
        Step::Send(fixture::agent_started_speaking()),
        Step::Binary(vec![1; 640]),
        Step::Send(fixture::error("FAILED_TO_THINK", "the model failed")),
    ]));
    let mut session = open(&server);
    let events = events_until(&mut session, ended);
    match events.last() {
        Some(SessionEvent::SpeechSessionFailed(failed)) => {
            assert_eq!(failed.failure.kind, SessionFailureKind::ProviderError);
            assert!(failed.failure.detail.contains("FAILED_TO_THINK"));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        session.failure_cause(),
        Some(&DeepgramError::Server {
            code: Some("FAILED_TO_THINK".to_owned()),
            description: "the model failed".to_owned(),
        })
    );
}

#[test]
fn a_close_before_settings_applied_is_typed() {
    let mut script = Script::new(Vec::new());
    script.applied = Applied::Close(1011);
    let server = AgentServer::start(script);
    let mut model = agent(&server);
    let outcome = model.open(config());
    assert!(matches!(outcome, Err(OpenError::Failed(_))), "{outcome:?}");
    assert!(
        matches!(
            model.open_error(),
            Some(DeepgramError::ClosedBeforeSettings {
                code: Some(1011),
                ..
            })
        ),
        "{:?}",
        model.open_error()
    );

    let mut script = Script::new(Vec::new());
    script.applied = Applied::ErrorThenClose(fixture::error("INVALID_SETTINGS", "bad settings"));
    let server = AgentServer::start(script);
    let mut model = agent(&server);
    assert!(model.open(config()).is_err());
    assert_eq!(
        model.open_error(),
        Some(&DeepgramError::Server {
            code: Some("INVALID_SETTINGS".to_owned()),
            description: "bad settings".to_owned(),
        })
    );
}

#[test]
fn settings_applied_not_arriving_within_the_deadline_fails_the_open_and_closes() {
    let mut script = Script::new(Vec::new());
    script.applied = Applied::Never;
    let server = AgentServer::start(script);
    let mut model = agent_with(
        endpoint(&server).with_open_timeout(Duration::from_millis(300)),
        Arc::new(FixedKey(KEY)),
    );
    let started = Instant::now();
    let outcome = model.open(config());
    assert!(matches!(outcome, Err(OpenError::Failed(_))), "{outcome:?}");
    assert!(started.elapsed() < Duration::from_millis(300) + CLOSE_BOUND);
    assert!(matches!(
        model.open_error(),
        Some(DeepgramError::Stalled(_))
    ));
    let seen = server.wait(0, CLOSE_BOUND, |seen| seen.closed.is_some());
    assert!(seen.closed.is_some());
}

#[test]
fn a_keepalive_goes_out_after_silence_and_not_while_audio_flows() {
    let server = AgentServer::start(Script::new(vec![Step::Pause(Duration::from_secs(10))]));
    let mut model = agent_with(
        endpoint(&server).with_keepalive_interval(Duration::from_millis(300)),
        Arc::new(FixedKey(KEY)),
    );
    let mut session = model.open(config()).unwrap();
    let opened = Instant::now();
    for index in 0..30 {
        session.append_audio(frame(index * 320)).unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
    let talking = server.seen(0);
    assert!(
        talking.of_type("KeepAlive").is_empty(),
        "a KeepAlive went out while audio flowed"
    );
    let seen = server.wait(0, Duration::from_secs(3), |seen| {
        !seen.of_type("KeepAlive").is_empty()
    });
    let sent = seen.times_of("KeepAlive");
    assert!(!sent.is_empty(), "no KeepAlive after silence");
    let last_audio = seen.audio.last().map(|(at, _)| *at).unwrap_or(opened);
    assert!(sent[0].saturating_duration_since(last_audio) >= Duration::from_millis(250));
    assert!(seen.violations.is_empty(), "{:?}", seen.violations);
    drop(session);
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
    assert_eq!(model.open_error(), Some(&DeepgramError::Cancelled));
    let seen = server.wait(0, CLOSE_BOUND, |seen| seen.closed.is_some());
    let closed = seen.closed.expect("the connection was closed");
    assert!(closed.saturating_duration_since(cancelled_at) < CLOSE_BOUND);
}

#[test]
fn a_cancel_while_awaiting_settings_applied_closes_the_connection_within_the_close_bound() {
    let mut script = Script::new(Vec::new());
    script.applied = Applied::Never;
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
    assert_eq!(model.open_error(), Some(&DeepgramError::Cancelled));
    let seen = server.wait(0, CLOSE_BOUND, |seen| seen.closed.is_some());
    let closed = seen.closed.expect("the connection was closed");
    assert!(closed.saturating_duration_since(cancelled_at) < CLOSE_BOUND);
}

#[test]
fn dropping_a_session_mid_reply_closes_the_connection_within_the_close_bound() {
    let server = AgentServer::start(Script::new(vec![
        Step::Send(fixture::agent_started_speaking()),
        Step::Binary(vec![1; 640]),
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
fn a_session_closed_mid_reply_takes_nothing_more_and_closes_its_connection() {
    let server = AgentServer::start(Script::new(vec![
        Step::Send(fixture::agent_started_speaking()),
        Step::Binary(vec![1; 640]),
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
    let seen = server.wait(0, CLOSE_BOUND, |seen| seen.closed.is_some());
    assert!(seen.closed.is_some());
}

#[test]
fn an_empty_blank_or_missing_key_is_refused_before_connecting() {
    let server = AgentServer::start(Script::new(Vec::new()));
    let sources: Vec<(Arc<dyn CredentialSource>, CredentialError)> = vec![
        (Arc::new(FixedKey("")), CredentialError::Invalid),
        (Arc::new(FixedKey("  \t")), CredentialError::Invalid),
        (Arc::new(NoKey), CredentialError::Missing),
    ];
    for (source, expected) in sources {
        let mut model = agent_with(endpoint(&server), source);
        assert!(matches!(model.open(config()), Err(OpenError::Failed(_))));
        assert_eq!(
            model.open_error(),
            Some(&DeepgramError::Credential(expected))
        );
    }
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(server.connections(), 0);
}

#[test]
fn the_key_is_in_no_error_and_no_debug() {
    let refusing = {
        let mut script = Script::new(Vec::new());
        script.handshake = Handshake::Refuse(401);
        AgentServer::start(script)
    };
    let closing = {
        let mut script = Script::new(Vec::new());
        script.applied = Applied::Close(1008);
        AgentServer::start(script)
    };
    let failing = AgentServer::start(Script::new(vec![Step::Send(fixture::error(
        "INTERNAL_SERVER_ERROR",
        "boom",
    ))]));
    let closed_port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        format!("ws://{address}")
    };
    let mut rendered = Vec::new();
    for url in [refusing.url.clone(), closing.url.clone(), closed_port] {
        let mut model = agent_with(
            AgentEndpoint::new()
                .with_url(url)
                .with_open_timeout(Duration::from_secs(2)),
            Arc::new(FixedKey(KEY)),
        );
        rendered.push(format!("{model:?}"));
        let error = model.open(config()).expect_err("the open fails");
        rendered.push(format!("{error} {error:?}"));
        let cause = model.open_error().expect("a cause");
        rendered.push(format!("{cause} {cause:?} {model:?}"));
    }
    let mut session = open(&failing);
    let events = events_until(&mut session, ended);
    rendered.push(format!("{events:?} {session:?}"));
    let cause = session.failure_cause().expect("a cause");
    rendered.push(format!("{cause} {cause:?}"));
    for text in rendered {
        assert!(!text.contains(KEY), "{text}");
    }
    let seen = refusing.wait(0, Duration::from_secs(1), |seen| {
        seen.authorization.is_some()
    });
    assert_eq!(
        seen.authorization.as_deref(),
        Some(&*format!("Token {KEY}")),
        "the key did not reach the server"
    );
}

#[test]
fn the_authorization_scheme_is_configurable() {
    let server = AgentServer::start(Script::new(Vec::new()));
    let mut model = agent_with(
        endpoint(&server).with_auth_scheme("Bearer"),
        Arc::new(FixedKey(KEY)),
    );
    let session = model.open(config()).unwrap();
    let seen = server.wait(0, Duration::from_secs(3), |seen| {
        seen.authorization.is_some()
    });
    assert_eq!(
        seen.authorization.as_deref(),
        Some(&*format!("Bearer {KEY}"))
    );
    drop(session);
    for scheme in ["", "Token x", "Tok\r\nen"] {
        assert!(
            matches!(
                DeepgramAgent::new(
                    AgentEndpoint::new().with_auth_scheme(scheme),
                    AgentConfig::new(),
                    Arc::new(FixedKey(KEY)),
                    "deepgram-api-key",
                ),
                Err(ConfigError::AuthScheme)
            ),
            "{scheme:?}"
        );
    }
}

#[test]
fn plaintext_is_refused_to_a_non_loopback_host() {
    let build = |url: &str| {
        DeepgramAgent::new(
            AgentEndpoint::new().with_url(url),
            AgentConfig::new(),
            Arc::new(FixedKey(KEY)),
            "deepgram-api-key",
        )
    };
    for url in [
        "ws://agent.deepgram.com",
        "ws://10.0.0.1:9",
        "ws://localhost.example.com",
        "ws://127.0.0.1@evil.example",
        "ws://0.0.0.0:9",
        "WS://127.0.0.1:9",
        "http://127.0.0.1:9",
        "wss://agent.deepgram.com/v1/agent/converse?x=1",
    ] {
        assert!(
            matches!(build(url), Err(ConfigError::InvalidUrl(_))),
            "{url}"
        );
    }
    for url in [
        "wss://agent.deepgram.com",
        "ws://127.0.0.1:9",
        "ws://[::1]:9",
        "ws://localhost:9",
    ] {
        assert!(build(url).is_ok(), "{url}");
    }
    assert_eq!(AgentEndpoint::new().url, "wss://agent.deepgram.com");
}

#[test]
fn a_message_beyond_the_size_bound_fails_the_session() {
    for step in [
        Step::Text(format!(
            "{{\"type\":\"History\",\"pad\":\"{}\"}}",
            "x".repeat(5 << 20)
        )),
        Step::Binary(vec![0; 5 << 20]),
    ] {
        let server = AgentServer::start(Script::new(vec![
            Step::Send(fixture::agent_started_speaking()),
            step,
        ]));
        let mut session = open(&server);
        let events = events_until(&mut session, ended);
        assert!(
            matches!(events.last(), Some(SessionEvent::SpeechSessionFailed(_))),
            "{events:#?}"
        );
        assert!(
            matches!(session.failure_cause(), Some(DeepgramError::Transport(detail)) if detail.contains("too long") || detail.contains("Space limit")),
            "{:?}",
            session.failure_cause()
        );
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
fn a_client_function_the_session_offers_is_proposed_and_its_result_sent() {
    let server = AgentServer::start(Script::new(vec![
        Step::Send(fixture::user_started_speaking()),
        Step::Send(fixture::conversation_text("user", "what time is it")),
        Step::Send(fixture::function_call("server_side", "call_0", false)),
        Step::Send(fixture::function_call("local_time", "call_1", true)),
        Step::AwaitType("FunctionCallResponse"),
        Step::Send(fixture::conversation_text("assistant", "It is noon.")),
        Step::Send(fixture::agent_started_speaking()),
        Step::Binary(vec![1; 640]),
        Step::Send(fixture::agent_audio_done()),
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
    let events = events_until(&mut session, completed);
    assert!(events.last().is_some_and(completed), "{events:#?}");
    let seen = server.seen(0);
    let functions = &seen.of_type("Settings")[0]["agent"]["think"]["functions"];
    assert_eq!(functions[0]["name"], "local_time");
    assert_eq!(functions[0]["description"], "The local time.");
    assert_eq!(
        functions[0]["parameters"],
        serde_json::json!({"type": "object"})
    );
    let responses = seen.of_type("FunctionCallResponse");
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0]["id"], "call_1");
    assert_eq!(responses[0]["name"], "local_time");
    assert_eq!(responses[0]["content"], "12:00");
}

#[test]
fn a_client_function_the_session_does_not_offer_fails_the_session_naming_it() {
    let server = AgentServer::start(Script::new(vec![Step::Send(fixture::function_call(
        "open_door",
        "call_2",
        true,
    ))]));
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
        Some(&DeepgramError::FunctionNotOffered {
            name: "open_door".to_owned()
        })
    );
}

/// The probe: a question, then during the answer a place; the fixture cuts the answer as the
/// Voice Agent does on speech over it, and answers in a new reply.
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
        Step::Send(fixture::user_started_speaking()),
        Step::Send(fixture::conversation_text("user", "what is the weather")),
        Step::Send(fixture::agent_thinking("")),
        Step::Send(fixture::conversation_text("assistant", "The weather is")),
        Step::Send(fixture::agent_started_speaking()),
        Step::Binary(vec![1; 640]),
        Step::Binary(vec![1; 640]),
        Step::AwaitAudio(4),
        Step::Send(fixture::user_started_speaking()),
        Step::Binary(vec![1; 640]),
        Step::Send(fixture::conversation_text("user", "in Lisbon")),
        Step::Send(fixture::conversation_text(
            "assistant",
            "It is mild in Lisbon.",
        )),
        Step::Send(fixture::agent_started_speaking()),
        Step::Binary(vec![2; 640]),
        Step::Send(fixture::agent_audio_done()),
        Step::Pause(Duration::from_secs(10)),
    ]));
    let mut model = agent(&server);
    let outcome = native_duplex_evidence(&mut model, &probe());
    assert!(
        matches!(outcome, Err(NoNativeDuplex::Restarted { first, second }) if first != second),
        "{outcome:?}"
    );
    assert_eq!(agent_native_duplex_verdict(&outcome), Ok(true));
    assert!(!agent_claims().contains(&Capability::NativeDuplex));
    let seen = server.wait(0, CLOSE_BOUND * 2, |seen| seen.closed.is_some());
    assert!(
        seen.closed.is_some(),
        "the procedure's close left the connection open"
    );
    assert!(seen.violations.is_empty(), "{:?}", seen.violations);
}
