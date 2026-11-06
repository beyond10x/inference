//! The Phonic binding against the local fixture server of `support/mod.rs`, which speaks the
//! documented conversation protocol: the opening order and `config`, input chunking, a response
//! from start to finish, barge-in, a closed session, server errors and closes, the refusals
//! before connecting, liveness, tools, and the duplex conformance procedure.

mod support;

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::time::{Duration, Instant};

use inference_phonic::inference_duplex::conformance::{
    NativeDuplexProbe, NoNativeDuplex, native_duplex_evidence,
};
use inference_phonic::inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_phonic::inference_duplex::inference_core::model::Capability;
use inference_phonic::inference_duplex::inference_tools::{
    DeclareTool, Json, SideEffectClass, Tool,
};
use inference_phonic::inference_duplex::{
    NativeSpeechModel, OpenError, ResponseId, SessionConfig, SessionError, SessionEvent,
    SessionFailureKind, SpeechSessionHandle, TurnDetection, TurnTuning, Unsupported,
};
use inference_phonic::{
    CLOSE_BOUND, Credential, CredentialError, CredentialFuture, CredentialSource, PhonicConfig,
    PhonicEndpoint, PhonicError, PhonicSession, PhonicSpeech, claims, native_duplex_verdict,
};
use support::{Handshake, KEY, PhonicServer, Script, Step, bounded};

const WAIT: Duration = Duration::from_millis(20);
const KEY_NAME: &str = "phonic-api-key";

struct FixedKey(&'static str);

impl CredentialSource for FixedKey {
    fn resolve<'a>(&'a self, _name: &'a str) -> CredentialFuture<'a> {
        let key = Credential::new(self.0.as_bytes().to_vec());
        Box::pin(std::future::ready(Ok(key)))
    }
}

struct NoKey;

impl CredentialSource for NoKey {
    fn resolve<'a>(&'a self, _name: &'a str) -> CredentialFuture<'a> {
        Box::pin(std::future::ready(Err(CredentialError::Missing)))
    }
}

fn pcm(rate: u32) -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(rate).unwrap(),
        channels: NonZeroU16::MIN,
    }
}

fn config() -> SessionConfig {
    SessionConfig {
        voice: "sabrina".to_owned(),
        instructions: "Answer in one sentence.".to_owned(),
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

/// 20 ms of silence at 16 kHz.
fn frame(position: u64) -> AudioFrame {
    AudioFrame {
        format: pcm(16_000),
        position: SampleClockPosition::new(position).unwrap(),
        sample_count: 320,
        data: vec![0; 640],
    }
}

fn endpoint(server: &PhonicServer) -> PhonicEndpoint {
    PhonicEndpoint::new()
        .with_url(&server.url)
        .with_connect_timeout(Duration::from_secs(3))
        .with_ready_timeout(Duration::from_secs(3))
        .with_read_timeout(Duration::from_secs(3))
}

fn model(server: &PhonicServer) -> PhonicSpeech {
    PhonicSpeech::new(
        endpoint(server),
        PhonicConfig::new(),
        Arc::new(FixedKey(KEY)),
        KEY_NAME,
    )
    .expect("a valid endpoint")
}

fn open(server: &PhonicServer) -> PhonicSession {
    model(server).open(config()).expect("the session opens")
}

/// Every event until `stop` holds for one, that one included, or five seconds pass.
fn events_until(
    session: &mut PhonicSession,
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

fn position_of(events: &[SessionEvent], wanted: impl Fn(&SessionEvent) -> bool) -> usize {
    events
        .iter()
        .position(wanted)
        .unwrap_or_else(|| panic!("{events:#?}"))
}

#[test]
fn config_goes_first_with_the_bearer_key_and_no_audio_precedes_ready() {
    bounded(|| {
        let mut script = Script::new(vec![Step::AwaitAudio(2)]);
        script.ready_delay = Duration::from_millis(300);
        let server = PhonicServer::start(script);
        let started = Instant::now();
        let mut session = open(&server);
        assert!(
            started.elapsed() >= Duration::from_millis(250),
            "the open returned before ready_to_start_conversation"
        );
        assert_eq!(session.conversation_id(), Some("conv_fixture"));
        session.append_audio(frame(0)).unwrap();
        session.append_audio(frame(320)).unwrap();
        let seen = server.wait(0, Duration::from_secs(3), |seen| {
            seen.of_type("audio_chunk").len() == 2
        });
        assert!(seen.violations.is_empty(), "{:?}", seen.violations);
        assert_eq!(seen.path, "/v1/sts/ws");
        assert_eq!(seen.query, "");
        assert_eq!(
            seen.authorization.as_deref(),
            Some(&*format!("Bearer {KEY}"))
        );
        let config = &seen.messages[0].1;
        assert_eq!(config["type"], "config");
        assert_eq!(config["model"], "phonic-1");
        assert_eq!(config["input_format"], "pcm_16000");
        assert_eq!(config["output_format"], "pcm_16000");
        assert_eq!(config["voice_id"], "sabrina");
        assert_eq!(config["system_prompt"], "Answer in one sentence.");
        assert!(config.get("tools").is_none(), "{config}");
        assert_eq!(seen.audio(), vec![vec![0_u8; 640], vec![0_u8; 640]]);
    });
}

#[test]
fn a_frame_longer_than_twenty_milliseconds_goes_out_in_chunks_of_at_most_twenty() {
    bounded(|| {
        let server = PhonicServer::start(Script::new(vec![Step::AwaitAudio(6)]));
        let mut session = open(&server);
        let long = AudioFrame {
            format: pcm(16_000),
            position: SampleClockPosition::new(0).unwrap(),
            sample_count: 1_650,
            data: vec![1; 3_300],
        };
        session.append_audio(long).unwrap();
        let seen = server.wait(0, Duration::from_secs(3), |seen| {
            seen.of_type("audio_chunk").len() == 6
        });
        let sizes: Vec<_> = seen.audio().iter().map(Vec::len).collect();
        assert_eq!(sizes, [640, 640, 640, 640, 640, 100]);
    });
}

#[test]
fn a_response_runs_from_assistant_started_to_finished_with_its_audio_and_text() {
    bounded(|| {
        let server = PhonicServer::start(Script::new(vec![
            Step::Send(support::bare("user_started_speaking")),
            Step::Send(support::bare("user_finished_speaking")),
            Step::Send(support::input_text("what is the capital of France")),
            Step::Send(support::bare("assistant_started_speaking")),
            Step::Send(support::audio(640, "Paris ")),
            Step::Send(support::audio(641, "is the capital.")),
            Step::Send(support::audio(641, "")),
            Step::Send(support::bare("assistant_finished_speaking")),
        ]));
        let mut session = open(&server);
        let events = events_until(&mut session, |event| {
            matches!(event, SessionEvent::ResponseCompleted(_))
        });
        let ids = created(&events);
        assert_eq!(ids.len(), 1, "{events:#?}");
        let id = ids[0];
        let heard = position_of(
            &events,
            |event| matches!(event, SessionEvent::InputTranscribed(heard) if heard.text == "what is the capital of France"),
        );
        let committed = position_of(&events, |event| {
            matches!(event, SessionEvent::InputCommitted(_))
        });
        let started = position_of(&events, |event| {
            matches!(event, SessionEvent::SpeechStarted(_))
        });
        let stopped = position_of(&events, |event| {
            matches!(event, SessionEvent::SpeechStopped(_))
        });
        assert!(started < stopped && committed < heard, "{events:#?}");
        let text: String = events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::OutputTranscriptDelta(delta) if delta.response_id == id => {
                    Some(delta.text.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(text, "Paris is the capital.");
        let samples: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::OutputAudio(audio) if audio.response_id == id => {
                    Some((audio.frame.position.get(), audio.frame.sample_count))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            samples,
            vec![(0, 320), (320, 320), (640, 321)],
            "a half sample was not carried over"
        );
        assert!(matches!(
            events.last(),
            Some(SessionEvent::ResponseCompleted(done)) if done.response_id == id
        ));
        assert_eq!(session.final_transcript(id), Some("Paris is the capital."));
    });
}

#[test]
fn barge_in_cancels_the_response_in_progress_and_drops_its_late_audio() {
    bounded(|| {
        let server = PhonicServer::start(Script::new(vec![
            Step::Send(support::bare("assistant_started_speaking")),
            Step::Send(support::audio(640, "It is ")),
            Step::Send(support::audio(640, "mild ")),
            Step::Send(support::bare("user_started_speaking")),
            Step::Send(support::interrupted("It is mild")),
            Step::Send(support::audio(640, "and sunny")),
            Step::Send(support::input_text("in Lisbon")),
            Step::Send(support::bare("assistant_started_speaking")),
            Step::Send(support::audio(640, "Lisbon is warm.")),
            Step::Send(support::bare("assistant_finished_speaking")),
        ]));
        let mut session = open(&server);
        let mut events = events_until(&mut session, |event| {
            matches!(event, SessionEvent::ResponseCancelled(_))
        });
        events.extend(events_until(&mut session, |event| {
            matches!(event, SessionEvent::ResponseCompleted(_))
        }));
        let ids = created(&events);
        assert_eq!(ids.len(), 2, "{events:#?}");
        let (cut, next) = (ids[0], ids[1]);
        assert_eq!(
            audio_of(&events, cut),
            2,
            "late audio of the cut response was delivered"
        );
        assert_eq!(audio_of(&events, next), 1);
        let speech = position_of(&events, |event| {
            matches!(event, SessionEvent::SpeechStarted(_))
        });
        let cancelled = position_of(
            &events,
            |event| matches!(event, SessionEvent::ResponseCancelled(gone) if gone.response_id == cut),
        );
        assert!(speech < cancelled);
        assert!(matches!(
            events.last(),
            Some(SessionEvent::ResponseCompleted(done)) if done.response_id == next
        ));
        assert_eq!(session.late_audio_dropped(), 1);
        assert_eq!(session.final_transcript(cut), Some("It is mild"));
        assert_eq!(session.final_transcript(next), Some("Lisbon is warm."));
    });
}

#[test]
fn a_closed_session_takes_nothing_more_and_closes_its_connection() {
    bounded(|| {
        let server = PhonicServer::start(Script::new(vec![
            Step::Send(support::bare("assistant_started_speaking")),
            Step::Send(support::audio(640, "Hello")),
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
        assert!(
            seen.closed.is_some(),
            "the closed session left its connection open"
        );
        assert!(seen.audio().is_empty());
    });
}

#[test]
fn dropping_a_session_mid_stream_closes_the_connection_within_the_close_bound() {
    bounded(|| {
        let server = PhonicServer::start(Script::new(vec![
            Step::Send(support::audio(640, "Hello")),
            Step::Pause(Duration::from_secs(10)),
        ]));
        let mut session = open(&server);
        let events = events_until(&mut session, |event| {
            matches!(event, SessionEvent::OutputAudio(_))
        });
        assert!(
            !created(&events).is_empty(),
            "audio with no response in progress opened none"
        );
        let dropped_at = Instant::now();
        drop(session);
        assert!(dropped_at.elapsed() < CLOSE_BOUND);
        let seen = server.wait(0, CLOSE_BOUND, |seen| seen.closed.is_some());
        let closed = seen.closed.expect("the connection was closed");
        assert!(closed.saturating_duration_since(dropped_at) < CLOSE_BOUND);
    });
}

#[test]
fn a_server_close_mid_response_fails_the_response_and_the_session_with_its_code() {
    bounded(|| {
        let server = PhonicServer::start(Script::new(vec![
            Step::Send(support::bare("assistant_started_speaking")),
            Step::Send(support::audio(640, "Hel")),
            Step::Close(4505),
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
                assert!(failed.failure.detail.contains("4505"), "{failed:?}");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            session.failure_cause(),
            Some(PhonicError::Closed {
                code: Some(4505),
                ..
            })
        ));
    });
}

#[test]
fn an_error_fails_the_session_as_a_provider_error_and_a_warning_does_not() {
    bounded(|| {
        let server = PhonicServer::start(Script::new(vec![
            Step::Send(support::warning("the voice is deprecated")),
            Step::Send(support::error("audio chunk too long", "chunk_too_long")),
        ]));
        let mut session = open(&server);
        let events = events_until(&mut session, ended);
        match events.last() {
            Some(SessionEvent::SpeechSessionFailed(failed)) => {
                assert_eq!(failed.failure.kind, SessionFailureKind::ProviderError);
                assert!(failed.failure.detail.contains("chunk_too_long"));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(session.warnings(), 1);
        assert_eq!(
            session.failure_cause(),
            Some(&PhonicError::Server {
                code: Some("chunk_too_long".to_owned()),
                message: "audio chunk too long".to_owned(),
            })
        );
    });
}

#[test]
fn no_ready_within_the_deadline_fails_the_open_and_closes_the_connection() {
    bounded(|| {
        let mut script = Script::new(Vec::new());
        script.ready = false;
        let server = PhonicServer::start(script);
        let mut model = PhonicSpeech::new(
            endpoint(&server).with_ready_timeout(Duration::from_millis(200)),
            PhonicConfig::new(),
            Arc::new(FixedKey(KEY)),
            KEY_NAME,
        )
        .unwrap();
        let started = Instant::now();
        let outcome = model.open(config());
        match &outcome {
            Err(OpenError::Failed(failure)) => {
                assert_eq!(failure.kind, SessionFailureKind::Stalled)
            }
            other => panic!("{other:?}"),
        }
        assert!(started.elapsed() < Duration::from_millis(200) + CLOSE_BOUND);
        assert!(matches!(model.open_error(), Some(PhonicError::Stalled(_))));
        let seen = server.wait(0, CLOSE_BOUND, |seen| seen.closed.is_some());
        assert!(
            seen.closed.is_some(),
            "the failed open left its connection open"
        );
    });
}

#[test]
fn a_refused_handshake_or_a_missing_key_fails_the_open_and_the_key_is_in_no_error() {
    bounded(|| {
        let mut script = Script::new(Vec::new());
        script.handshake = Handshake::Refuse(401);
        let server = PhonicServer::start(script);
        let mut model = model(&server);
        let outcome = model.open(config());
        match &outcome {
            Err(OpenError::Failed(failure)) => {
                assert_eq!(failure.kind, SessionFailureKind::Rejected)
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            model.open_error(),
            Some(PhonicError::Connect {
                status: Some(401),
                ..
            })
        ));
        for rendered in [
            format!("{outcome:?}"),
            format!("{:?}", model.open_error()),
            format!("{model:?}"),
        ] {
            assert!(!rendered.contains(KEY), "{rendered}");
        }

        let mut keyless = PhonicSpeech::new(
            endpoint(&server),
            PhonicConfig::new(),
            Arc::new(NoKey),
            KEY_NAME,
        )
        .unwrap();
        assert!(matches!(keyless.open(config()), Err(OpenError::Failed(_))));
        assert_eq!(
            keyless.open_error(),
            Some(&PhonicError::Credential(CredentialError::Missing))
        );
        assert_eq!(server.connections(), 1, "a session without a key connected");
    });
}

#[test]
fn a_format_phonic_does_not_take_and_a_plaintext_remote_host_are_refused_before_connecting() {
    bounded(|| {
        let server = PhonicServer::start(Script::new(Vec::new()));
        let mut model = model(&server);
        let mut wide = config();
        wide.output_format = pcm(48_000);
        assert_eq!(
            model.open(wide).err(),
            Some(OpenError::Unsupported(Unsupported::OutputFormat(pcm(
                48_000
            ))))
        );
        let mut manual = config();
        manual.turn_detection = TurnDetection::Disabled;
        assert!(matches!(model.open(manual), Err(OpenError::Unsupported(_))));
        for url in [
            "ws://api.phonic.ai",
            "ws://10.0.0.1:9",
            "http://127.0.0.1:9",
        ] {
            assert!(
                matches!(
                    PhonicSpeech::new(
                        PhonicEndpoint::new().with_url(url),
                        PhonicConfig::new(),
                        Arc::new(FixedKey(KEY)),
                        KEY_NAME,
                    ),
                    Err(PhonicError::InvalidUrl(_))
                ),
                "{url}"
            );
        }
        assert!(matches!(
            PhonicSpeech::new(
                endpoint(&server),
                PhonicConfig::new().with_agent("  "),
                Arc::new(FixedKey(KEY)),
                KEY_NAME,
            ),
            Err(PhonicError::InvalidConfig("agent"))
        ));
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(server.connections(), 0);
    });
}

#[test]
fn a_quiet_server_that_answers_pings_stays_open_and_a_deaf_one_stalls() {
    bounded(|| {
        let server = PhonicServer::start(Script::new(vec![Step::Pause(Duration::from_secs(10))]));
        let quiet = endpoint(&server)
            .with_read_timeout(Duration::from_millis(400))
            .with_ping_interval(Duration::from_millis(100));
        let mut model = PhonicSpeech::new(
            quiet.clone(),
            PhonicConfig::new(),
            Arc::new(FixedKey(KEY)),
            KEY_NAME,
        )
        .unwrap();
        let mut session = model.open(config()).unwrap();
        let mut quiet_session = Vec::new();
        let until = Instant::now() + Duration::from_millis(1_200);
        while Instant::now() < until {
            quiet_session.extend(session.next_event(WAIT));
        }
        assert!(!quiet_session.iter().any(ended), "{quiet_session:#?}");
        assert!(server.seen(0).pings >= 2, "{:?}", server.seen(0).pings);
        drop(session);

        let mut script = Script::new(Vec::new());
        script.deaf = true;
        let deaf = PhonicServer::start(script);
        let mut model = PhonicSpeech::new(
            quiet.with_url(&deaf.url),
            PhonicConfig::new(),
            Arc::new(FixedKey(KEY)),
            KEY_NAME,
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
    });
}

fn time_tool() -> Tool {
    Tool::declare(DeclareTool {
        name: "local_time".to_owned(),
        description: "The local time.".to_owned(),
        schema: serde_json::json!({"type": "object", "properties": {}, "additionalProperties": false}),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0
}

#[test]
fn a_tool_the_session_offers_is_declared_inline_proposed_and_answered() {
    bounded(|| {
        let server = PhonicServer::start(Script::new(vec![
            Step::Send(support::tool_call("local_time", "call_1")),
            Step::AwaitType("tool_call_output"),
            Step::Send(support::bare("assistant_started_speaking")),
            Step::Send(support::audio(640, "It is noon.")),
            Step::Send(support::bare("assistant_finished_speaking")),
        ]));
        let mut model = model(&server);
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
            .submit_tool_result(&call_id, serde_json::json!({"time": "12:00"}))
            .unwrap();
        let events = events_until(&mut session, |event| {
            matches!(event, SessionEvent::ResponseCompleted(_))
        });
        assert!(matches!(
            events.last(),
            Some(SessionEvent::ResponseCompleted(_))
        ));
        let seen = server.seen(0);
        let tools = &seen.messages[0].1["tools"];
        assert_eq!(tools[0]["type"], "custom_websocket");
        assert_eq!(tools[0]["tool_schema"]["function"]["name"], "local_time");
        assert_eq!(tools[0]["tool_schema"]["function"]["strict"], true);
        let outputs = seen.of_type("tool_call_output");
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0]["tool_call_id"], "call_1");
        assert_eq!(outputs[0]["output"], serde_json::json!({"time": "12:00"}));
        let _: Json = outputs[0]["output"].clone();
    });
}

#[test]
fn a_tool_the_session_does_not_offer_fails_the_session_naming_it() {
    bounded(|| {
        let server = PhonicServer::start(Script::new(vec![Step::Send(support::tool_call(
            "open_door",
            "call_2",
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
            Some(&PhonicError::ToolNotOffered {
                name: "open_door".to_owned()
            })
        );
    });
}

/// The probe: a question, then during the answer a place; the fixture interrupts the answer as
/// the documentation describes, and answers in a new response.
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
fn the_duplex_conformance_procedure_runs_over_the_fixture_with_barge_in_and_agrees_with_the_claims()
{
    bounded(|| {
        let server = PhonicServer::start(Script::new(vec![
            Step::AwaitAudio(2),
            Step::Send(support::input_text("what is the weather")),
            Step::Send(support::bare("assistant_started_speaking")),
            Step::Send(support::audio(640, "The weather ")),
            Step::Send(support::audio(640, "is ")),
            Step::AwaitAudio(4),
            Step::Send(support::bare("user_started_speaking")),
            Step::Send(support::interrupted("The weather is")),
            Step::Send(support::input_text("in Lisbon")),
            Step::Send(support::bare("assistant_started_speaking")),
            Step::Send(support::audio(640, "It is mild in Lisbon.")),
            Step::Send(support::bare("assistant_finished_speaking")),
            Step::Pause(Duration::from_secs(10)),
        ]));
        let mut model = model(&server);
        let outcome = native_duplex_evidence(&mut model, &probe());
        assert!(
            matches!(outcome, Err(NoNativeDuplex::Restarted { first, second }) if first != second),
            "{outcome:?}"
        );
        assert_eq!(native_duplex_verdict(&outcome), Ok(true));
        assert!(!claims().contains(&Capability::NativeDuplex));
        let seen = server.wait(0, CLOSE_BOUND * 2, |seen| seen.closed.is_some());
        assert!(
            seen.closed.is_some(),
            "the procedure's close left the connection open"
        );
        assert!(seen.violations.is_empty(), "{:?}", seen.violations);
    });
}
