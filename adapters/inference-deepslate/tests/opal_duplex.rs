//! The Opal binding against the local fixture server of `support/mod.rs`: the duplex
//! conformance procedure, the opening and the configuration it sends, replies, barge-in, the
//! voice activity events, tool calls, errors, the refusals before connecting, the ping, closes,
//! frames it cannot read and the key.

mod support;

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::time::{Duration, Instant};

use inference_deepslate::inference_duplex::conformance::{
    NativeDuplexProbe, NoNativeDuplex, native_duplex_evidence,
};
use inference_deepslate::inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_deepslate::inference_duplex::inference_core::model::{
    Capability, ExecutionForm, Locality,
};
use inference_deepslate::inference_duplex::inference_tools::{
    DeclareTool, Json, SideEffectClass, Tool,
};
use inference_deepslate::inference_duplex::{
    NativeSpeechModel, OpenError, ResponseId, SessionConfig, SessionError, SessionEvent,
    SessionFailureKind, SpeechCallId, SpeechSessionHandle, TuningOption, TurnDetection,
    TurnSensitivity, TurnTuning, Unsupported,
};
use inference_deepslate::protocol::{
    AudioLine, ClientBound, ErrorCategory, HostedTts, HostedTtsMode, ServiceBound, ToolDefinition,
    TriggerMode, Vad, VadState,
};
use inference_deepslate::{
    CLOSE_BOUND, CredentialError, CredentialSource, DeepslateError, DeepslateOpal,
    DeepslateOpalSession, Endpoint, OpalConfig, capabilities, claims, native_duplex_verdict,
    tool_unavailable,
};
use serde_json::json;
use support::{
    Handshake, KEY, NoKey, OpalServer, Ready, Script, Step, audio, begin, end, reply, text,
    utterance, vad, within_deadline,
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
        voice: "de56bbd1-3ea7-4543-a17c-504664d9116b".to_owned(),
        instructions: "You are a helpful assistant.".to_owned(),
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

/// A change to the session configuration that Opal does not take.
type Change = fn(&mut SessionConfig);

/// A 20 ms frame at 16 kHz starting at sample `position`.
fn frame(position: u64) -> AudioFrame {
    AudioFrame {
        format: pcm(16_000),
        position: SampleClockPosition::new(position).unwrap(),
        sample_count: 320,
        data: vec![3; 640],
    }
}

fn weather() -> Tool {
    Tool::declare(DeclareTool {
        name: "get_weather".to_owned(),
        description: "The weather at a place.".to_owned(),
        schema: json!({
            "type": "object",
            "properties": {"location": {"type": "string"}},
            "required": ["location"]
        }),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0
}

fn endpoint(server: &OpalServer) -> Endpoint {
    Endpoint::new("vendor-7", "org-3")
        .with_url(&server.url)
        .with_open_timeout(Duration::from_secs(3))
        .with_read_timeout(Duration::from_secs(3))
        .with_ping_interval(Duration::from_secs(1))
}

fn model_with(endpoint: Endpoint, source: Arc<dyn CredentialSource>) -> DeepslateOpal {
    DeepslateOpal::new(endpoint, OpalConfig::new(), source, "deepslate-api-key")
        .expect("the fixture endpoint is valid")
}

fn model(server: &OpalServer) -> DeepslateOpal {
    model_with(endpoint(server), Arc::new(support::FixedKey(KEY)))
}

fn open(server: &OpalServer) -> DeepslateOpalSession {
    model(server).open(config()).expect("the session opens")
}

/// Every event until `stop` holds for one, that one included, or five seconds pass.
fn events_until(
    session: &mut DeepslateOpalSession,
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

fn created(events: &[SessionEvent]) -> Vec<ResponseId> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ResponseCreated(created) => Some(created.response_id),
            _ => None,
        })
        .collect()
}

/// The positions and sample counts of a response's audio frames.
fn audio_of(events: &[SessionEvent], response_id: ResponseId) -> Vec<(u64, u32)> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::OutputAudio(audio) if audio.response_id == response_id => {
                Some((audio.frame.position.get(), audio.frame.sample_count))
            }
            _ => None,
        })
        .collect()
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

fn failure_of(events: &[SessionEvent]) -> Option<(SessionFailureKind, String)> {
    events.iter().find_map(|event| match event {
        SessionEvent::SpeechSessionFailed(failed) => {
            Some((failed.failure.kind, failed.failure.detail.clone()))
        }
        _ => None,
    })
}

/// The probe: a question, then during the answer a place; the fixture cuts the answer on the
/// speech over it, as the `IMMEDIATE` trigger interrupts the inference running, and answers in
/// a new turn.
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
    within_deadline(|| {
        let mut steps = vec![Step::AwaitAudio(2)];
        steps.extend(utterance(0, 320));
        steps.extend([
            Step::Send(ClientBound::UserTranscription {
                turn_id: 0,
                text: "what is the weather".to_owned(),
                language: "en".to_owned(),
            }),
            begin(1),
            text(1, "The weather is"),
            audio(Some(1), 640, 1),
            audio(Some(1), 640, 1),
            Step::AwaitAudio(4),
            vad(VadState::Silence, VadState::SpeechStarting, 640),
            vad(VadState::SpeechStarting, VadState::Speech, 640),
            Step::Send(ClientBound::PlaybackClearBuffer),
            audio(Some(1), 640, 1),
            vad(VadState::Speech, VadState::SpeechEnding, 960),
            vad(VadState::SpeechEnding, VadState::Silence, 960),
            Step::Send(ClientBound::UserTranscription {
                turn_id: 2,
                text: "in Lisbon".to_owned(),
                language: "en".to_owned(),
            }),
        ]);
        steps.extend(reply(3, "It is mild in Lisbon.", 1, 640));
        steps.push(Step::Pause(Duration::from_secs(10)));
        let server = OpalServer::start(Script::new(steps));
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

#[test]
fn the_binding_declares_what_opal_takes() {
    let declared = capabilities();
    assert_eq!(
        declared.input_formats,
        vec![pcm(8_000), pcm(16_000), pcm(48_000)]
    );
    assert_eq!(declared.output_formats, declared.input_formats);
    assert_eq!(declared.turn_detection, vec![TurnDetection::ServerVad]);
    assert!(declared.turn_tuning.silence && declared.turn_tuning.prefix_padding);
    assert!(!declared.turn_tuning.sensitivity);
    assert!(!declared.manual_commit && !declared.manual_response);
    assert!(!declared.manual_interrupt && !declared.client_cancel && !declared.truncation);
    assert!(declared.tool_calling);
    assert_eq!(
        claims(),
        vec![
            Capability::OutputStreaming,
            Capability::IncrementalInput,
            Capability::ConcurrentInputOutput,
            Capability::ToolCalling,
        ]
    );
    let model = DeepslateOpal::new(
        Endpoint::new("v", "o"),
        OpalConfig::new(),
        Arc::new(NoKey),
        "deepslate-api-key",
    )
    .unwrap();
    let adapter = model.adapter();
    assert_eq!(adapter.backend(), "deepslate-opal");
    assert_eq!(adapter.execution_form(), ExecutionForm::HostedService);
    assert_eq!(adapter.locality(), Locality::ExternalProvider);
    assert_eq!(adapter.capabilities(), claims().as_slice());
}

#[test]
fn the_session_configuration_reaches_the_server_first_and_once() {
    within_deadline(|| {
        let server = OpalServer::start(Script::new(vec![Step::Pause(Duration::from_secs(10))]));
        let endpoint = endpoint(&server).with_model("opal-v2.1");
        let opal = OpalConfig {
            temperature: Some(0.3),
            tts_mode: HostedTtsMode::LowLatency,
        };
        let mut model =
            DeepslateOpal::new(endpoint, opal, Arc::new(support::FixedKey(KEY)), "k").unwrap();
        let mut config = config();
        config.turn_tuning = TurnTuning {
            silence: Some(Duration::from_millis(600)),
            sensitivity: None,
            prefix_padding: Some(Duration::from_millis(300)),
        };
        config.tools = vec![weather()];
        config.input_format = pcm(8_000);
        config.output_format = pcm(48_000);
        let mut session = model.open(config).expect("the session opens");
        let seen = server.wait(0, Duration::from_secs(2), |seen| seen.messages.len() >= 2);
        assert_eq!(
            seen.path,
            "/api/v1/vendors/vendor-7/organizations/org-3/realtime/opal-v2.1"
        );
        assert_eq!(seen.authorization, Some(format!("Bearer {KEY}")));
        assert!(seen.ready_sent.is_some());
        let messages: Vec<ServiceBound> = seen.messages.iter().map(|(_, m)| m.clone()).collect();
        let ServiceBound::Initialize(init) = &messages[0] else {
            panic!("the first message is {:?}", messages[0]);
        };
        assert_eq!(init.input, AudioLine::pcm16(8_000, 1));
        assert_eq!(init.output, AudioLine::pcm16(48_000, 1));
        assert_eq!(
            init.vad,
            Vad {
                stop_duration: Duration::from_millis(600),
                backbuffer_duration: Duration::from_millis(300),
                ..Vad::DOCUMENTED
            }
        );
        assert_eq!(init.system_prompt, "You are a helpful assistant.");
        assert!((init.temperature - 0.3).abs() < f64::EPSILON);
        assert_eq!(
            init.tts,
            Some(HostedTts {
                voice_id: "de56bbd1-3ea7-4543-a17c-504664d9116b".to_owned(),
                mode: HostedTtsMode::LowLatency,
            })
        );
        assert!(!init.supports_playback_reporting);
        assert_eq!(
            messages[1],
            ServiceBound::UpdateTools(vec![ToolDefinition {
                name: "get_weather".to_owned(),
                description: Some("The weather at a place.".to_owned()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {"location": {"type": "string"}},
                    "required": ["location"]
                })),
            }])
        );
        assert!(seen.violations.is_empty(), "{:?}", seen.violations);
        session.close().unwrap();
    });
}

#[test]
fn an_empty_voice_asks_for_no_speech_and_no_tools_sends_no_definitions() {
    within_deadline(|| {
        let server = OpalServer::start(Script::new(vec![Step::Pause(Duration::from_secs(10))]));
        let mut config = config();
        config.voice = String::new();
        let mut session = model(&server).open(config).expect("the session opens");
        session.append_audio(frame(0)).unwrap();
        let seen = server.wait(0, Duration::from_secs(2), |seen| !seen.audio().is_empty());
        let ServiceBound::Initialize(init) = &seen.messages[0].1 else {
            panic!("the first message is {:?}", seen.messages[0].1);
        };
        assert_eq!(init.tts, None);
        assert_eq!(init.vad, Vad::DOCUMENTED);
        assert!(
            !seen
                .messages
                .iter()
                .any(|(_, message)| matches!(message, ServiceBound::UpdateTools(_)))
        );
        session.close().unwrap();
    });
}

#[test]
fn audio_is_sent_as_immediate_user_input_numbered_by_its_first_sample() {
    within_deadline(|| {
        let server = OpalServer::start(Script::new(vec![Step::Pause(Duration::from_secs(10))]));
        let mut session = open(&server);
        for position in [0, 320, 640] {
            session.append_audio(frame(position)).unwrap();
        }
        let seen = server.wait(0, Duration::from_secs(2), |seen| seen.audio().len() >= 3);
        let sent: Vec<ServiceBound> = seen
            .messages
            .iter()
            .filter(|(_, message)| matches!(message, ServiceBound::Audio { .. }))
            .map(|(_, message)| message.clone())
            .collect();
        assert_eq!(
            sent,
            [0, 320, 640]
                .map(|packet_id| ServiceBound::Audio {
                    packet_id,
                    mode: TriggerMode::Immediate,
                    data: vec![3; 640],
                })
                .to_vec()
        );
        assert_eq!(
            session.append_audio(AudioFrame {
                format: pcm(8_000),
                position: SampleClockPosition::new(960).unwrap(),
                sample_count: 160,
                data: vec![0; 320],
            }),
            Err(SessionError::FormatMismatch {
                expected: pcm(16_000),
                found: pcm(8_000),
            })
        );
        assert!(seen.violations.is_empty(), "{:?}", seen.violations);
        session.close().unwrap();
    });
}

#[test]
fn a_reply_lands_on_one_response_with_its_audio_and_text() {
    within_deadline(|| {
        let server = OpalServer::start(Script::new(reply(4, "Hello, how can I help you?", 3, 640)));
        let mut session = open(&server);
        let events = events_until(&mut session, completed);
        let ids = created(&events);
        assert_eq!(ids.len(), 1, "{events:?}");
        assert_eq!(said(&events, ids[0]), "Hello, how can I help you?");
        assert_eq!(
            audio_of(&events, ids[0]),
            vec![(0, 320), (320, 320), (640, 320)]
        );
        assert!(matches!(
            events.last(),
            Some(SessionEvent::ResponseCompleted(done)) if done.response_id == ids[0]
        ));
        session.close().unwrap();
    });
}

#[test]
fn audio_without_a_turn_goes_to_the_turn_begun_last_and_half_samples_wait() {
    within_deadline(|| {
        let server = OpalServer::start(Script::new(vec![
            begin(0),
            audio(None, 641, 1),
            audio(None, 3, 1),
            audio(Some(0), 2, 1),
            end(0),
        ]));
        let mut session = open(&server);
        let events = events_until(&mut session, completed);
        let ids = created(&events);
        assert_eq!(ids.len(), 1, "{events:?}");
        assert_eq!(
            audio_of(&events, ids[0]),
            vec![(0, 320), (320, 2), (322, 1)]
        );
        session.close().unwrap();
    });
}

#[test]
fn text_ahead_of_its_turn_is_held_for_it_and_audio_of_a_new_turn_begins_one() {
    within_deadline(|| {
        let server = OpalServer::start(Script::new(vec![
            text(5, "Good "),
            text(5, "morning."),
            audio(Some(5), 640, 1),
            begin(5),
            end(5),
        ]));
        let mut session = open(&server);
        let events = events_until(&mut session, completed);
        let ids = created(&events);
        assert_eq!(ids.len(), 1, "{events:?}");
        assert_eq!(said(&events, ids[0]), "Good morning.");
        assert_eq!(audio_of(&events, ids[0]), vec![(0, 320)]);
        session.close().unwrap();
    });
}

#[test]
fn speech_over_a_reply_cancels_it_and_its_late_audio_is_dropped() {
    within_deadline(|| {
        let mut steps = vec![begin(0), text(0, "Let me tell you"), audio(Some(0), 640, 1)];
        steps.extend([
            Step::AwaitAudio(2),
            vad(VadState::Silence, VadState::SpeechStarting, 640),
            vad(VadState::SpeechStarting, VadState::Speech, 640),
            Step::Send(ClientBound::PlaybackClearBuffer),
            audio(Some(0), 640, 1),
            audio(None, 640, 1),
            text(0, " about"),
            end(0),
        ]);
        steps.extend(reply(1, "Yes?", 1, 640));
        let server = OpalServer::start(Script::new(steps));
        let mut session = open(&server);
        session.append_audio(frame(0)).unwrap();
        session.append_audio(frame(320)).unwrap();
        let events = events_until(&mut session, completed);
        let ids = created(&events);
        assert_eq!(ids.len(), 2, "{events:?}");
        let started = events
            .iter()
            .position(|event| matches!(event, SessionEvent::SpeechStarted(started) if started.position.get() == 640))
            .expect("speech started at the packet that crossed the threshold");
        let cancelled = events
            .iter()
            .position(|event| matches!(event, SessionEvent::ResponseCancelled(gone) if gone.response_id == ids[0]))
            .expect("the cut reply is cancelled");
        assert!(started < cancelled);
        assert_eq!(audio_of(&events, ids[0]), vec![(0, 320)]);
        assert_eq!(said(&events, ids[0]), "Let me tell you");
        assert_eq!(said(&events, ids[1]), "Yes?");
        assert_eq!(audio_of(&events, ids[1]), vec![(0, 320)]);
        assert_eq!(session.late_audio_dropped(), 2);
        session.close().unwrap();
    });
}

#[test]
fn the_end_of_speech_commits_the_turn_and_its_transcription_follows() {
    within_deadline(|| {
        let mut steps = vec![Step::AwaitAudio(4)];
        steps.extend(utterance(320, 1_280));
        steps.push(Step::Send(ClientBound::UserTranscription {
            turn_id: 0,
            text: "What time is it?".to_owned(),
            language: "en".to_owned(),
        }));
        steps.push(vad(VadState::Silence, VadState::SpeechStarting, 2_000));
        steps.push(vad(VadState::SpeechStarting, VadState::Silence, 2_320));
        steps.extend(reply(1, "Noon.", 1, 640));
        let server = OpalServer::start(Script::new(steps));
        let mut session = open(&server);
        for position in [0, 320, 640, 960] {
            session.append_audio(frame(position)).unwrap();
        }
        let events = events_until(&mut session, completed);
        let kinds: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::SpeechStarted(started) => {
                    assert_eq!(started.position.get(), 320);
                    Some("started")
                }
                SessionEvent::SpeechStopped(stopped) => {
                    assert_eq!(stopped.position.get(), 1_280);
                    Some("stopped")
                }
                SessionEvent::InputCommitted(_) => Some("committed"),
                SessionEvent::InputTranscribed(heard) => {
                    assert_eq!(heard.text, "What time is it?");
                    Some("transcribed")
                }
                SessionEvent::ResponseCreated(_) => Some("created"),
                _ => None,
            })
            .collect();
        assert_eq!(
            kinds,
            ["started", "stopped", "committed", "transcribed", "created"]
        );
        let items: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::InputCommitted(committed) => Some(committed.item.as_str()),
                SessionEvent::InputTranscribed(heard) => Some(heard.item.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            items,
            ["turn-1", "turn-1"],
            "the transcription of turn_id 0 names the commit it belongs to"
        );
        session.close().unwrap();
    });
}

#[test]
fn a_tool_call_is_proposed_and_its_result_goes_back_by_id() {
    within_deadline(|| {
        let server = OpalServer::start(Script::new(vec![
            begin(2),
            Step::Send(ClientBound::ToolCallRequest {
                id: "call_abc123".to_owned(),
                name: "get_weather".to_owned(),
                parameters: json!({"location": "Amsterdam"}),
                turn_id: Some(2),
            }),
            Step::AwaitToolResponse("call_abc123"),
            text(2, "It is sunny."),
            audio(Some(2), 640, 1),
            end(2),
        ]));
        let tool = weather();
        let mut config = config();
        config.tools = vec![tool.clone()];
        let mut session = model(&server).open(config).expect("the session opens");
        let events = events_until(&mut session, |event| {
            matches!(event, SessionEvent::SpeechToolCallProposed(_))
        });
        let Some(SessionEvent::SpeechToolCallProposed(proposed)) = events.last() else {
            panic!("no call was proposed: {events:?}");
        };
        assert_eq!(proposed.call_id, SpeechCallId::new("call_abc123"));
        assert_eq!(proposed.tool_id, tool.tool_id());
        assert_eq!(proposed.arguments, json!({"location": "Amsterdam"}));
        assert_eq!(proposed.response_id, created(&events)[0]);
        session
            .submit_tool_result(
                &SpeechCallId::new("call_abc123"),
                json!({"temperature": 22, "condition": "sunny"}),
            )
            .unwrap();
        let rest = events_until(
            &mut session,
            |event| matches!(event, SessionEvent::ResponseCompleted(done) if done.response_id != proposed.response_id),
        );
        let answers = created(&rest);
        assert_eq!(answers.len(), 1, "{rest:?}");
        let first_done = rest.iter().position(|event| {
            matches!(event, SessionEvent::ResponseCompleted(done) if done.response_id == proposed.response_id)
        });
        let answer_made = rest
            .iter()
            .position(|event| matches!(event, SessionEvent::ResponseCreated(_)));
        assert!(first_done.is_some() && first_done < answer_made, "{rest:?}");
        assert_eq!(said(&rest, proposed.response_id), "");
        assert_eq!(said(&rest, answers[0]), "It is sunny.");
        assert_eq!(audio_of(&rest, answers[0]), vec![(0, 320)]);
        let seen = server.seen(0);
        assert_eq!(
            seen.tool_responses(),
            vec![(
                "call_abc123".to_owned(),
                "{\"condition\":\"sunny\",\"temperature\":22}".to_owned()
            )]
        );
        assert_eq!(
            session.submit_tool_result(&SpeechCallId::new("call_abc123"), Json::Null),
            Err(SessionError::CallConflict(
                inference_deepslate::inference_duplex::SpeechToolCallStateConflict {
                    current: inference_deepslate::inference_duplex::SpeechToolCallState::Answered,
                }
            ))
        );
        session.close().unwrap();
    });
}

#[test]
fn a_string_result_is_sent_as_it_is() {
    within_deadline(|| {
        let server = OpalServer::start(Script::new(vec![
            Step::Send(ClientBound::ToolCallRequest {
                id: "c1".to_owned(),
                name: "get_weather".to_owned(),
                parameters: json!({}),
                turn_id: None,
            }),
            Step::AwaitToolResponse("c1"),
        ]));
        let mut config = config();
        config.tools = vec![weather()];
        let mut session = model(&server).open(config).expect("the session opens");
        events_until(&mut session, |event| {
            matches!(event, SessionEvent::SpeechToolCallProposed(_))
        });
        session
            .submit_tool_result(&SpeechCallId::new("c1"), json!("sunny, 22 degrees"))
            .unwrap();
        let seen = server.wait(0, Duration::from_secs(2), |seen| {
            !seen.tool_responses().is_empty()
        });
        assert_eq!(
            seen.tool_responses(),
            vec![("c1".to_owned(), "sunny, 22 degrees".to_owned())]
        );
        session.close().unwrap();
    });
}

#[test]
fn a_tool_the_session_does_not_offer_is_answered_unavailable_and_the_session_goes_on() {
    within_deadline(|| {
        let mut steps = vec![
            begin(0),
            Step::Send(ClientBound::ToolCallRequest {
                id: "c9".to_owned(),
                name: "open_door".to_owned(),
                parameters: json!({}),
                turn_id: Some(0),
            }),
            Step::AwaitToolResponse("c9"),
            text(0, "I cannot open doors."),
            end(0),
        ];
        steps.extend(reply(1, "Anything else?", 1, 640));
        let server = OpalServer::start(Script::new(steps));
        let mut session = open(&server);
        let events = events_until(&mut session, |event| completed(event) || ended(event));
        let mut events = events;
        events.extend(events_until(&mut session, |event| {
            completed(event) || ended(event)
        }));
        assert_eq!(failure_of(&events), None, "{events:?}");
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, SessionEvent::SpeechToolCallProposed(_)))
        );
        let ids = created(&events);
        assert_eq!(ids.len(), 2, "{events:?}");
        assert_eq!(said(&events, ids[0]), "I cannot open doors.");
        assert_eq!(said(&events, ids[1]), "Anything else?");
        assert_eq!(
            session.ledger().dropped_events(),
            &[SessionError::ToolNotOffered {
                name: "open_door".to_owned()
            }]
        );
        assert_eq!(
            server.seen(0).tool_responses(),
            vec![("c9".to_owned(), tool_unavailable("open_door"))]
        );
        assert!(tool_unavailable("open_door").contains("not available in this session"));
        session.close().unwrap();
    });
}

#[test]
fn a_session_error_fails_the_session_with_its_category() {
    within_deadline(|| {
        let mut steps = reply(0, "Hi.", 1, 640);
        steps.push(begin(1));
        steps.push(Step::Send(ClientBound::Error {
            category: ErrorCategory::Inference,
            message: "model unavailable".to_owned(),
            trace_id: Some("abc-123-xyz".to_owned()),
        }));
        steps.push(Step::Close(1011));
        let server = OpalServer::start(Script::new(steps));
        let mut session = open(&server);
        let events = events_until(&mut session, ended);
        let (kind, detail) = failure_of(&events).expect("the session failed");
        assert_eq!(kind, SessionFailureKind::ProviderError);
        assert!(detail.contains("ERROR_INFERENCE"), "{detail}");
        assert!(detail.contains("model unavailable"), "{detail}");
        let ids = created(&events);
        assert!(events.iter().any(
            |event| matches!(event, SessionEvent::ResponseFailed(failed) if failed.response_id == ids[1])
        ));
        assert_eq!(
            session.failure_cause(),
            Some(&DeepslateError::Server {
                category: ErrorCategory::Inference,
                message: "model unavailable".to_owned(),
                trace_id: Some("abc-123-xyz".to_owned()),
            })
        );
    });
}

#[test]
fn options_opal_does_not_take_are_refused_before_connecting() {
    within_deadline(|| {
        let server = OpalServer::start(Script::new(Vec::new()));
        let mut model = model(&server);
        let refused = |change: Change| {
            let mut config = config();
            change(&mut config);
            config
        };
        let cases: [(Change, Unsupported); 7] = [
            (
                |config| config.input_format = pcm(24_000),
                Unsupported::InputFormat(pcm(24_000)),
            ),
            (
                |config| {
                    config.output_format = AudioFormat {
                        encoding: AudioEncoding::PcmMulaw,
                        ..pcm(8_000)
                    }
                },
                Unsupported::OutputFormat(AudioFormat {
                    encoding: AudioEncoding::PcmMulaw,
                    ..pcm(8_000)
                }),
            ),
            (
                |config| {
                    config.input_format = AudioFormat {
                        channels: NonZeroU16::new(2).unwrap(),
                        ..pcm(16_000)
                    }
                },
                Unsupported::InputFormat(AudioFormat {
                    channels: NonZeroU16::new(2).unwrap(),
                    ..pcm(16_000)
                }),
            ),
            (
                |config| config.turn_detection = TurnDetection::Disabled,
                Unsupported::TurnDetection(TurnDetection::Disabled),
            ),
            (
                |config| config.turn_tuning.sensitivity = Some(TurnSensitivity::High),
                Unsupported::TurnTuning(TuningOption::Sensitivity),
            ),
            (
                |config| config.auto_response = false,
                Unsupported::ManualResponse,
            ),
            (
                |config| config.auto_interrupt = false,
                Unsupported::ManualInterrupt,
            ),
        ];
        for (change, unsupported) in cases {
            assert_eq!(
                model.open(refused(change)).err(),
                Some(OpenError::Unsupported(unsupported))
            );
        }
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(server.connections(), 0);
    });
}

#[test]
fn the_callers_commit_create_cancel_and_truncate_are_unsupported() {
    within_deadline(|| {
        let server = OpalServer::start(Script::new(vec![
            begin(0),
            audio(Some(0), 640, 1),
            Step::Pause(Duration::from_secs(10)),
        ]));
        let mut session = open(&server);
        let events = events_until(&mut session, |event| {
            matches!(event, SessionEvent::OutputAudio(_))
        });
        let id = created(&events)[0];
        assert_eq!(
            session.commit_input(),
            Err(SessionError::Unsupported(Unsupported::ManualCommit))
        );
        assert_eq!(
            session.create_response(),
            Err(SessionError::Unsupported(Unsupported::ManualResponse))
        );
        assert_eq!(
            session.cancel_response(id),
            Err(SessionError::Unsupported(Unsupported::ClientCancel))
        );
        assert_eq!(
            session.truncate_output(id, SampleClockPosition::new(10).unwrap()),
            Err(SessionError::Unsupported(Unsupported::Truncation))
        );
        session.close().unwrap();
    });
}

#[test]
fn a_refused_or_failed_opening_opens_nothing() {
    within_deadline(|| {
        let cases = [
            (
                Script {
                    handshake: Handshake::Refuse(401),
                    ..Script::new(Vec::new())
                },
                SessionFailureKind::Rejected,
            ),
            (
                Script {
                    handshake: Handshake::Refuse(404),
                    ..Script::new(Vec::new())
                },
                SessionFailureKind::Rejected,
            ),
            (
                Script {
                    ready: Ready::ErrorThenClose(ClientBound::Error {
                        category: ErrorCategory::Configuration,
                        message: "Invalid sample rate".to_owned(),
                        trace_id: None,
                    }),
                    ..Script::new(Vec::new())
                },
                SessionFailureKind::Rejected,
            ),
            (
                Script {
                    ready: Ready::Close(1011),
                    ..Script::new(Vec::new())
                },
                SessionFailureKind::TransportClosed,
            ),
        ];
        for (script, kind) in cases {
            let server = OpalServer::start(script);
            let mut model = model(&server);
            let failed = model.open(config()).err();
            assert!(
                matches!(&failed, Some(OpenError::Failed(failure)) if failure.kind == kind),
                "{failed:?} {:?}",
                model.open_error()
            );
            assert!(model.open_error().is_some());
        }

        let server = OpalServer::start(Script {
            ready: Ready::Never,
            ..Script::new(Vec::new())
        });
        let mut model = model_with(
            endpoint(&server).with_open_timeout(Duration::from_millis(300)),
            Arc::new(support::FixedKey(KEY)),
        );
        let started = Instant::now();
        let failed = model.open(config()).err();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(
            matches!(&failed, Some(OpenError::Failed(failure)) if failure.kind == SessionFailureKind::Stalled),
            "{failed:?}"
        );
        let seen = server.wait(0, CLOSE_BOUND * 2, |seen| seen.closed.is_some());
        assert!(
            seen.closed.is_some(),
            "a stalled opening left its connection open"
        );

        let server = OpalServer::start(Script {
            handshake: Handshake::Stall,
            ..Script::new(Vec::new())
        });
        let mut model = model_with(
            endpoint(&server).with_open_timeout(Duration::from_millis(300)),
            Arc::new(support::FixedKey(KEY)),
        );
        let started = Instant::now();
        assert!(model.open(config()).is_err());
        assert!(started.elapsed() < Duration::from_secs(2));

        let server = OpalServer::start(Script::new(Vec::new()));
        let mut model = model_with(endpoint(&server), Arc::new(NoKey));
        let failed = model.open(config()).err();
        assert!(
            matches!(&failed, Some(OpenError::Failed(failure)) if failure.kind == SessionFailureKind::Rejected),
            "{failed:?}"
        );
        assert_eq!(
            model.open_error(),
            Some(&DeepslateError::Credential(CredentialError::Missing))
        );
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(server.connections(), 0);
    });
}

#[test]
fn a_ping_goes_out_at_the_interval_and_a_pong_is_no_answer_from_the_model() {
    within_deadline(|| {
        let mut steps = utterance(0, 320);
        steps.push(Step::Pause(Duration::from_secs(10)));
        let server = OpalServer::start(Script::new(steps));
        let mut model = model_with(
            endpoint(&server).with_ping_interval(Duration::from_millis(100)),
            Arc::new(support::FixedKey(KEY)),
        );
        let mut config = config();
        config.idle_limit = Duration::from_millis(800);
        let mut session = model.open(config).expect("the session opens");
        let started = Instant::now();
        let events = events_until(&mut session, ended);
        let (kind, _) = failure_of(&events).expect("the session stalled");
        assert_eq!(kind, SessionFailureKind::Stalled);
        assert!(
            started.elapsed() >= Duration::from_millis(700),
            "{events:?}"
        );
        let seen = server.seen(0);
        assert!(seen.pings().len() >= 4, "{} pings", seen.pings().len());
        assert!(seen.violations.is_empty(), "{:?}", seen.violations);
    });
}

#[test]
fn a_closed_transport_fails_the_session_at_once_after_what_came_before() {
    within_deadline(|| {
        let mut steps = vec![begin(0), audio(Some(0), 640, 1)];
        steps.push(Step::Close(1001));
        let server = OpalServer::start(Script::new(steps));
        let mut session = open(&server);
        let events = events_until(&mut session, ended);
        let ids = created(&events);
        assert_eq!(audio_of(&events, ids[0]), vec![(0, 320)]);
        let (kind, _) = failure_of(&events).expect("the session failed");
        assert_eq!(kind, SessionFailureKind::TransportClosed);
        assert!(matches!(
            session.failure_cause(),
            Some(DeepslateError::Closed {
                code: Some(1001),
                ..
            })
        ));
        assert!(matches!(
            session.append_audio(frame(0)),
            Err(SessionError::NotOpen(_))
        ));
    });
}

#[test]
fn a_frame_that_cannot_be_read_is_counted_and_not_fatal() {
    within_deadline(|| {
        let mut steps = vec![
            Step::Text("{\"type\":\"hello\"}".to_owned()),
            Step::Raw(vec![0x2a, 0x05, 0x08]),
            Step::Send(ClientBound::Other(16)),
            Step::Raw(vec![0xa2, 0x01, 0x00]),
        ];
        steps.extend(reply(0, "Fine.", 1, 640));
        let server = OpalServer::start(Script::new(steps));
        let mut session = open(&server);
        let events = events_until(&mut session, completed);
        assert_eq!(created(&events).len(), 1, "{events:?}");
        assert_eq!(session.ledger().unreadable_frames(), 2);
        session.close().unwrap();
    });
}

#[test]
fn closing_cancels_the_response_in_progress_and_closes_the_connection() {
    within_deadline(|| {
        let server = OpalServer::start(Script::new(vec![
            begin(0),
            audio(Some(0), 640, 1),
            Step::Pause(Duration::from_millis(300)),
            audio(Some(0), 640, 1),
            end(0),
            Step::Pause(Duration::from_secs(10)),
        ]));
        let mut session = open(&server);
        events_until(&mut session, |event| {
            matches!(event, SessionEvent::OutputAudio(_))
        });
        let started = Instant::now();
        session.close().unwrap();
        assert!(started.elapsed() <= CLOSE_BOUND * 2);
        let rest = events_until(&mut session, ended);
        assert!(
            rest.iter()
                .any(|event| matches!(event, SessionEvent::ResponseCancelled(_)))
        );
        assert!(matches!(
            rest.last(),
            Some(SessionEvent::SpeechSessionClosed(_))
        ));
        assert!(
            !rest
                .iter()
                .any(|event| matches!(event, SessionEvent::OutputAudio(_)))
        );
        let seen = server.wait(0, CLOSE_BOUND * 2, |seen| seen.closed.is_some());
        assert!(seen.closed.is_some());
        assert!(matches!(session.close(), Err(SessionError::NotOpen(_))));
    });
}

#[test]
fn the_key_is_in_no_error_and_no_debug() {
    within_deadline(|| {
        let server = OpalServer::start(Script {
            handshake: Handshake::Refuse(401),
            ..Script::new(Vec::new())
        });
        let mut model = model(&server);
        let failed = model.open(config()).err();
        assert!(!format!("{failed:?}").contains(KEY));
        assert!(!format!("{:?}", model.open_error()).contains(KEY));
        assert!(!format!("{model:?}").contains(KEY));
        let server = OpalServer::start(Script::new(vec![Step::Pause(Duration::from_secs(10))]));
        let session = open(&server);
        assert!(!format!("{session:?}").contains(KEY));
    });
}

#[test]
fn an_endpoint_it_cannot_use_is_refused_before_anything_connects() {
    assert!(
        DeepslateOpal::new(
            Endpoint::new("v", "o").with_url("ws://example.com"),
            OpalConfig::new(),
            Arc::new(NoKey),
            "k",
        )
        .is_err()
    );
    assert!(
        DeepslateOpal::new(
            Endpoint::new("v/../x", "o"),
            OpalConfig::new(),
            Arc::new(NoKey),
            "k",
        )
        .is_err()
    );
}
