//! Adversary pass 1 of `story:example-and-testkit-findings`, finding N35: a mis-scripted
//! `Reply::Call` naming a tool the session does not offer fails the mock run. The unit fails the
//! session when the ledger refuses the call's step. A turn that ends while a response is in
//! progress, on a model that does not restart, never makes the call a step: the mock's
//! `end_turn` splices only a `Reply::Say` into the response in progress and drops any other
//! reply, so the mis-scripted call is discarded and the run passes.

#![cfg(feature = "duplex")]

use std::time::Duration;

use inference_duplex::inference_tools::{DeclareTool, SideEffectClass, Tool};
use inference_duplex::{
    NativeSpeechModel, SessionConfig, SessionEvent, SessionFailureKind, SpeechSessionHandle,
    TurnDetection, TurnTuning,
};
use inference_testkit::duplex::mock::{MockSpeechModel, Reply, ScriptedTurn, frame, pcm};
use serde_json::json;

const WAIT: Duration = Duration::from_millis(100);

fn weather() -> Tool {
    Tool::declare(DeclareTool {
        name: "weather".to_owned(),
        description: String::new(),
        schema: json!({"type": "object", "properties": {"city": {"type": "string"}}}),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0
}

fn config() -> SessionConfig {
    SessionConfig {
        voice: "calm".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: pcm(16_000),
        output_format: pcm(16_000),
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: false,
        tools: vec![weather()],
        idle_limit: Duration::from_secs(5),
    }
}

fn speak(session: &mut impl SpeechSessionHandle, from: u64, frames: u64) {
    for index in 0..=frames {
        let byte = if index < frames { 9 } else { 0 };
        session
            .append_audio(frame(pcm(16_000), from + index * 320, byte))
            .expect("input in the session's format");
    }
}

#[test]
fn a_call_of_an_unoffered_tool_scripted_for_input_during_output_fails_the_session() {
    let mut model = MockSpeechModel::native(vec![
        ScriptedTurn {
            heard: "what is the weather".to_owned(),
            reply: Reply::Say("The weather is".to_owned()),
        },
        ScriptedTurn {
            heard: "and the temperature".to_owned(),
            reply: Reply::Call {
                tool: "thermometer".to_owned(),
                arguments: json!({"city": "Berlin"}),
                then: "It is {result} degrees".to_owned(),
            },
        },
    ]);
    let mut session = model
        .open(config())
        .expect("the model takes the configuration");
    speak(&mut session, 0, 2);
    let mut events = Vec::new();
    while !events
        .iter()
        .any(|event| matches!(event, SessionEvent::OutputTranscriptDelta(_)))
    {
        events.push(session.next_event(WAIT).expect("the response streams"));
    }
    speak(&mut session, 960, 2);
    while let Some(event) = session.next_event(WAIT) {
        events.push(event);
        if events.len() > 200 {
            break;
        }
    }
    let failure = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::SpeechSessionFailed(failed) => Some(failed.failure.clone()),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!(
                "a Reply::Call of the unoffered tool `thermometer` scripted for input during \
                 output passes the run without a failure: {events:#?}"
            )
        });
    assert_eq!(failure.kind, SessionFailureKind::ProviderError);
    assert!(failure.detail.contains("thermometer"), "{failure:?}");
}
