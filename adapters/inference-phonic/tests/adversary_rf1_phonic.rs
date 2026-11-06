//! Adversary pass 1 on the Phonic binding: what the unit's own tests do not drive. Each case runs
//! against the local fixture server of `support/mod.rs` (or no server at all) under its
//! wall-clock deadline.

mod support;

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use inference_phonic::inference_duplex::inference_audio::{AudioEncoding, AudioFormat};
use inference_phonic::inference_duplex::inference_tools::{DeclareTool, SideEffectClass, Tool};
use inference_phonic::inference_duplex::{
    NativeSpeechModel, ResponseId, SessionConfig, SessionEvent, SpeechSessionHandle, TurnDetection,
    TurnTuning,
};
use inference_phonic::protocol::{audio_chunks, audio_formats};
use inference_phonic::{
    Credential, CredentialFuture, CredentialSource, PhonicConfig, PhonicEndpoint, PhonicSession,
    PhonicSpeech,
};
use serde_json::Value;
use support::{KEY, PhonicServer, Script, Step, bounded};

const WAIT: Duration = Duration::from_millis(20);

struct FixedKey;

impl CredentialSource for FixedKey {
    fn resolve<'a>(&'a self, _name: &'a str) -> CredentialFuture<'a> {
        Box::pin(std::future::ready(Ok(Credential::new(
            KEY.as_bytes().to_vec(),
        ))))
    }
}

fn pcm16k() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).unwrap(),
        channels: NonZeroU16::MIN,
    }
}

fn config() -> SessionConfig {
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

fn speech(server: &PhonicServer, phonic: PhonicConfig) -> PhonicSpeech {
    PhonicSpeech::new(
        PhonicEndpoint::new()
            .with_url(&server.url)
            .with_connect_timeout(Duration::from_secs(3))
            .with_ready_timeout(Duration::from_secs(3))
            .with_read_timeout(Duration::from_secs(3)),
        phonic,
        Arc::new(FixedKey),
        "phonic-api-key",
    )
    .expect("a valid endpoint")
}

fn events_until(
    session: &mut PhonicSession,
    within: Duration,
    stop: impl Fn(&SessionEvent) -> bool,
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

fn failed(events: &[SessionEvent]) -> Option<&SessionEvent> {
    events
        .iter()
        .find(|event| matches!(event, SessionEvent::SpeechSessionFailed(_)))
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
        .filter(|event| {
            matches!(event, SessionEvent::OutputAudio(audio) if audio.response_id == response_id)
        })
        .count()
}

/// Phonic's WebSocket tools guide documents pre-defined WebSocket tools: created once, listed on
/// an agent, and called over the conversation's socket when the `config` names that agent. The
/// binding takes an agent (`PhonicConfig::with_agent`), so the agent's first tool call is a call
/// of a tool the session itself never declared. A documented configuration must not end the
/// conversation at its first tool call.
#[test]
fn adversary_rf1_a_websocket_tool_of_the_named_agent_does_not_fail_the_session() {
    bounded(|| {
        let server = PhonicServer::start(Script::new(vec![
            Step::Send(support::tool_call("get_temperature", "call_agent")),
            Step::Pause(Duration::from_millis(200)),
            Step::Send(support::bare("assistant_started_speaking")),
            Step::Send(support::audio(640, "It is mild.")),
            Step::Send(support::bare("assistant_finished_speaking")),
            Step::Pause(Duration::from_secs(5)),
        ]));
        let mut model = speech(&server, PhonicConfig::new().with_agent("weather-assistant"));
        let mut session = model.open(config()).expect("the session opens");
        let events = events_until(&mut session, Duration::from_secs(3), |event| {
            matches!(
                event,
                SessionEvent::ResponseCompleted(_) | SessionEvent::SpeechSessionFailed(_)
            )
        });
        assert!(
            failed(&events).is_none(),
            "a tool call of the named agent's pre-defined WebSocket tool ended the session: {:?}",
            session.failure_cause()
        );
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

/// A sync WebSocket tool holds Phonic silent until `tool_call_output` arrives, up to the tool's
/// output timeout (5 s by default). The contract's liveness bound runs "from the answer to a
/// call", so a caller whose tool runs longer than the session's idle limit, well inside
/// Phonic's output timeout, while it keeps draining events, has not met a silent provider.
#[test]
fn adversary_rf1_a_caller_running_its_tool_past_the_idle_limit_is_not_a_stalled_provider() {
    bounded(|| {
        let server = PhonicServer::start(Script::new(vec![
            Step::Send(support::tool_call("local_time", "call_slow")),
            Step::AwaitType("tool_call_output"),
            Step::Send(support::bare("assistant_started_speaking")),
            Step::Send(support::audio(640, "It is noon.")),
            Step::Send(support::bare("assistant_finished_speaking")),
            Step::Pause(Duration::from_secs(5)),
        ]));
        let mut model = speech(&server, PhonicConfig::new());
        let mut session_config = config();
        session_config.tools = vec![time_tool()];
        session_config.idle_limit = Duration::from_secs(1);
        let mut session = model.open(session_config).expect("the session opens");
        let events = events_until(&mut session, Duration::from_secs(3), |event| {
            matches!(event, SessionEvent::SpeechToolCallProposed(_))
        });
        let Some(SessionEvent::SpeechToolCallProposed(proposed)) = events.last() else {
            panic!("{events:#?}");
        };
        let call_id = proposed.call_id.clone();
        let tool_runs = events_until(&mut session, Duration::from_millis(1_800), |_| false);
        assert!(
            failed(&tool_runs).is_none(),
            "the session failed while the caller's tool ran for 1.8 s, inside Phonic's 5 s \
             output timeout: {tool_runs:#?}"
        );
        session
            .submit_tool_result(&call_id, serde_json::json!({"time": "12:00"}))
            .expect("the answer goes out");
        let after = events_until(&mut session, Duration::from_secs(3), |event| {
            matches!(event, SessionEvent::ResponseCompleted(_))
        });
        assert!(failed(&after).is_none(), "{after:#?}");
    });
}

/// The binding decided that audio while no response is in progress opens one (a response need
/// not begin with `assistant_started_speaking`), and that audio after an interruption belongs to
/// the cut response until the next `assistant_started_speaking`. Together they drop a whole new
/// turn: once the user's next turn has ended (`input_text`), audio is that turn's answer, not
/// late audio of the cut one. The inferred rule is to drop the cut response's audio only.
#[test]
fn adversary_rf1_the_answer_to_the_next_turn_after_a_cut_is_not_dropped_as_late_audio() {
    bounded(|| {
        let server = PhonicServer::start(Script::new(vec![
            Step::Send(support::bare("assistant_started_speaking")),
            Step::Send(support::audio(640, "It is ")),
            Step::Send(support::bare("user_started_speaking")),
            Step::Send(support::interrupted("It is")),
            Step::Send(support::audio(640, "mild")),
            Step::Send(support::bare("user_finished_speaking")),
            Step::Send(support::input_text("in Lisbon")),
            Step::Send(support::audio(640, "Lisbon is warm.")),
            Step::Send(support::bare("assistant_finished_speaking")),
            Step::Pause(Duration::from_secs(5)),
        ]));
        let mut model = speech(&server, PhonicConfig::new());
        let mut session = model.open(config()).expect("the session opens");
        let mut events = events_until(&mut session, Duration::from_secs(3), |event| {
            matches!(event, SessionEvent::InputTranscribed(_))
        });
        events.extend(events_until(
            &mut session,
            Duration::from_millis(800),
            |event| matches!(event, SessionEvent::ResponseCompleted(_)),
        ));
        let ids = created(&events);
        assert_eq!(
            ids.len(),
            2,
            "the answer to the turn after the cut opened no response; late audio dropped: {}",
            session.late_audio_dropped()
        );
        assert_eq!(audio_of(&events, ids[0]), 1);
        assert_eq!(audio_of(&events, ids[1]), 1);
        assert_eq!(session.late_audio_dropped(), 1);
    });
}

/// Every `audio_chunk` the binding builds, at every documented format and for frame lengths
/// around every chunk boundary, holds whole samples, at most 40 ms (the documented bound) and at
/// most 20 ms (the binding's own), is never empty (`minLength: 1`), and together the chunks are
/// the frame unchanged.
#[test]
fn adversary_rf1_every_chunk_is_whole_samples_within_forty_milliseconds_at_every_format() {
    bounded(|| {
        for format in audio_formats() {
            let width = match format.encoding {
                AudioEncoding::PcmS16le => 2,
                _ => 1,
            };
            let rate = u64::from(format.sample_rate.get());
            let per_chunk = (rate * 20 / 1_000) as usize;
            let mut lengths: Vec<usize> = (1..=3).collect();
            for k in 1..=5 {
                for delta in [-1_i64, 0, 1] {
                    let samples = (per_chunk * k) as i64 + delta;
                    lengths.push(samples as usize);
                }
            }
            lengths.push(rate as usize);
            for samples in lengths {
                let data: Vec<u8> = (0..samples * width).map(|i| (i % 251) as u8).collect();
                let chunks = audio_chunks(&data, format);
                let mut joined = Vec::new();
                for chunk in &chunks {
                    let value: Value = serde_json::from_str(chunk).expect("JSON");
                    assert_eq!(value["type"], "audio_chunk");
                    let bytes = STANDARD
                        .decode(value["audio"].as_str().expect("audio"))
                        .expect("base64");
                    assert!(!bytes.is_empty(), "{format:?} {samples}: an empty chunk");
                    assert_eq!(
                        bytes.len() % width,
                        0,
                        "{format:?} {samples}: a split sample"
                    );
                    let chunk_samples = (bytes.len() / width) as u64;
                    assert!(
                        chunk_samples * 1_000 <= rate * 40,
                        "{format:?} {samples}: {chunk_samples} samples exceed 40 ms"
                    );
                    assert!(
                        chunk_samples * 1_000 <= rate * 20,
                        "{format:?} {samples}: {chunk_samples} samples exceed 20 ms"
                    );
                    joined.extend_from_slice(&bytes);
                }
                assert_eq!(joined, data, "{format:?} {samples}");
            }
        }
    });
}
