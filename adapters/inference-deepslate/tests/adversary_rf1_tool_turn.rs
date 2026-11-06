//! Adversary cases: a tool call's answer and the liveness bound of the session.
//!
//! The binding's own test (`a_tool_call_is_proposed_and_its_result_goes_back_by_id`) has the
//! answer to a call continue on the call's turn: text and audio of the same `turn_id`, then its
//! `ResponseEnd`. The ledger treats an answer as a request for a new response ("a model that
//! creates every response owes a new one that answers the result"), so if no new response
//! begins, the session is awaiting one when the turn ends. A server that then stays silent
//! (it answers pings, which the ledger never hears) is failed as stalled one idle limit later,
//! although nothing is owed. The control case answers in a new turn, as the protocol may.

mod support;

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::time::{Duration, Instant};

use inference_deepslate::inference_duplex::inference_audio::{AudioEncoding, AudioFormat};
use inference_deepslate::inference_duplex::inference_tools::{DeclareTool, SideEffectClass, Tool};
use inference_deepslate::inference_duplex::{
    NativeSpeechModel, SessionConfig, SessionEvent, SpeechCallId, SpeechSessionHandle,
    TurnDetection, TurnTuning,
};
use inference_deepslate::protocol::ClientBound;
use inference_deepslate::{DeepslateOpal, DeepslateOpalSession, Endpoint, OpalConfig};
use serde_json::json;
use support::{KEY, OpalServer, Script, Step, audio, begin, end, reply, text, within_deadline};

const IDLE: Duration = Duration::from_secs(1);

fn pcm(rate: u32) -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(rate).unwrap(),
        channels: NonZeroU16::MIN,
    }
}

fn weather() -> Tool {
    Tool::declare(DeclareTool {
        name: "get_weather".to_owned(),
        description: "The weather at a place.".to_owned(),
        schema: json!({"type": "object", "properties": {"location": {"type": "string"}}}),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0
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
        tools: vec![weather()],
        idle_limit: IDLE,
    }
}

fn open(server: &OpalServer) -> DeepslateOpalSession {
    let endpoint = Endpoint::new("vendor-7", "org-3")
        .with_url(&server.url)
        .with_open_timeout(Duration::from_secs(3))
        .with_read_timeout(Duration::from_secs(3))
        .with_ping_interval(Duration::from_millis(300));
    DeepslateOpal::new(
        endpoint,
        OpalConfig::new(),
        Arc::new(support::FixedKey(KEY)),
        "deepslate-api-key",
    )
    .expect("the fixture endpoint is valid")
    .open(config())
    .expect("the session opens")
}

/// Every event for `within`, or until one satisfies `stop`.
fn events_for(
    session: &mut DeepslateOpalSession,
    within: Duration,
    stop: impl Fn(&SessionEvent) -> bool,
) -> Vec<SessionEvent> {
    let deadline = Instant::now() + within;
    let mut events = Vec::new();
    while Instant::now() < deadline {
        if let Some(event) = session.next_event(Duration::from_millis(20)) {
            let done = stop(&event);
            events.push(event);
            if done {
                break;
            }
        }
    }
    events
}

fn failed(events: &[SessionEvent]) -> Option<String> {
    events.iter().find_map(|event| match event {
        SessionEvent::SpeechSessionFailed(failed) => Some(format!(
            "{:?}: {}",
            failed.failure.kind, failed.failure.detail
        )),
        _ => None,
    })
}

fn call(id: &str, turn: u32) -> Step {
    Step::Send(ClientBound::ToolCallRequest {
        id: id.to_owned(),
        name: "get_weather".to_owned(),
        parameters: json!({"location": "Amsterdam"}),
        turn_id: Some(turn),
    })
}

/// Proposes, answers, waits for the turn's completion, then idles two idle limits and a half.
fn answer_then_idle(server: &OpalServer, id: &'static str) -> Vec<SessionEvent> {
    let mut session = open(server);
    let proposed = events_for(&mut session, Duration::from_secs(5), |event| {
        matches!(event, SessionEvent::SpeechToolCallProposed(_))
    });
    assert!(
        proposed
            .iter()
            .any(|event| matches!(event, SessionEvent::SpeechToolCallProposed(_))),
        "no call was proposed: {proposed:?}"
    );
    session
        .submit_tool_result(
            &SpeechCallId::new(id),
            json!({"temperature": 22, "condition": "sunny"}),
        )
        .unwrap();
    let mut events = events_for(&mut session, Duration::from_secs(5), |event| {
        matches!(event, SessionEvent::ResponseCompleted(_))
    });
    assert!(
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::ResponseCompleted(_))),
        "the answered turn never completed: {events:?}"
    );
    events.extend(events_for(&mut session, IDLE * 5 / 2, |event| {
        matches!(event, SessionEvent::SpeechSessionFailed(_))
    }));
    let _ = session.close();
    events
}

/// The binding's own fixture shape: the answer continues on the call's turn and the turn ends.
/// Nothing is owed afterwards, so a silent, healthy connection must not fail the session.
#[test]
fn an_answer_continued_on_its_own_turn_leaves_nothing_owed() {
    within_deadline(|| {
        let server = OpalServer::start(Script::new(vec![
            begin(2),
            call("call_rf1", 2),
            Step::AwaitToolResponse("call_rf1"),
            text(2, "It is sunny."),
            audio(Some(2), 640, 1),
            Step::Send(ClientBound::InferenceComplete { turn_id: 2 }),
            end(2),
            Step::Pause(Duration::from_secs(10)),
        ]));
        let events = answer_then_idle(&server, "call_rf1");
        assert_eq!(
            failed(&events),
            None,
            "the turn that carried the answer ended and the server stayed healthy, yet the session failed"
        );
    });
}

/// Control: the call's turn ends, the answer comes in a new turn. Expected green.
#[test]
fn an_answer_in_a_new_turn_leaves_nothing_owed() {
    within_deadline(|| {
        let mut steps = vec![begin(2), call("call_rf2", 2), end(2)];
        steps.push(Step::AwaitToolResponse("call_rf2"));
        steps.extend(reply(3, "It is sunny.", 1, 640));
        steps.push(Step::Pause(Duration::from_secs(10)));
        let server = OpalServer::start(Script::new(steps));
        let mut session = open(&server);
        let proposed = events_for(&mut session, Duration::from_secs(5), |event| {
            matches!(event, SessionEvent::SpeechToolCallProposed(_))
        });
        assert!(
            proposed
                .iter()
                .any(|event| matches!(event, SessionEvent::SpeechToolCallProposed(_)))
        );
        session
            .submit_tool_result(&SpeechCallId::new("call_rf2"), json!("sunny"))
            .unwrap();
        let mut events = events_for(&mut session, Duration::from_secs(5), |event| {
            matches!(event, SessionEvent::ResponseCreated(_))
        });
        events.extend(events_for(&mut session, Duration::from_secs(5), |event| {
            matches!(event, SessionEvent::ResponseCompleted(_))
        }));
        events.extend(events_for(&mut session, IDLE * 5 / 2, |event| {
            matches!(event, SessionEvent::SpeechSessionFailed(_))
        }));
        assert_eq!(failed(&events), None);
        let _ = session.close();
    });
}
