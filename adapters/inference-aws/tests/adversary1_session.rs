//! Adversary pass 1: the session's opening events carry the caller's configuration, not the
//! defaults; a stall leaves the reader usable; text content starts carry every role and the
//! interactive flag.
//!
//! The unit's own opening test uses `SessionConfig::new`, whose voice, output rate, tools,
//! inference values and turn detection are the defaults, so a session that ignored the caller's
//! values would pass it. Shapes are those of "Handling input events with the bidirectional API",
//! <https://docs.aws.amazon.com/nova/latest/nova2-userguide/sonic-input-events.html>.

use std::time::Duration;

use inference_aws::nova_sonic::{
    AudioRate, Endpointing, InferenceConfiguration, InputContent, InputEvent, NovaSonicError,
    OutputEvent, Role, Session, SessionConfig, ToolSpec,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

fn sent(receiver: &mut mpsc::Receiver<Vec<u8>>) -> Vec<Value> {
    let mut events = Vec::new();
    while let Ok(bytes) = receiver.try_recv() {
        events.push(serde_json::from_slice(&bytes).expect("each sent frame is JSON"));
    }
    events
}

#[tokio::test]
async fn opening_events_carry_the_callers_configuration() {
    let mut config = SessionConfig::new("Be terse.");
    config.voice_id = "tiffany".into();
    config.output_rate = AudioRate::Hz8000;
    config.endpointing = Some(Endpointing::High);
    config.inference = InferenceConfiguration {
        max_tokens: 333,
        top_p: 0.5,
        temperature: 0.25,
    };
    config.tools = vec![ToolSpec {
        name: "lookup".into(),
        description: "Looks a thing up".into(),
        input_schema: json!({"type": "object"}),
    }];
    let (input, mut receiver) = mpsc::channel(64);
    let (_frames, output) = mpsc::channel(4);
    let _session = Session::open(&config, input, output).await.unwrap();
    let events = sent(&mut receiver);

    assert_eq!(
        events[0],
        json!({"event": {"sessionStart": {
            "inferenceConfiguration": {"maxTokens": 333, "topP": 0.5, "temperature": 0.25},
            "turnDetectionConfiguration": {"endpointingSensitivity": "HIGH"}
        }}})
    );
    let prompt = &events[1]["event"]["promptStart"];
    assert_eq!(prompt["audioOutputConfiguration"]["voiceId"], "tiffany");
    assert_eq!(prompt["audioOutputConfiguration"]["sampleRateHertz"], 8000);
    assert_eq!(
        prompt["toolConfiguration"]["tools"][0]["toolSpec"]["name"],
        "lookup"
    );
    assert_eq!(events[3]["event"]["textInput"]["content"], "Be terse.");
}

#[tokio::test]
async fn a_stall_leaves_the_reader_usable() {
    let mut config = SessionConfig::new("x");
    config.read_timeout = Duration::from_millis(20);
    let (input, _receiver) = mpsc::channel(64);
    let (frames, output) = mpsc::channel(4);
    let Session { mut reader, .. } = Session::open(&config, input, output).await.unwrap();
    let stalled = reader.recv().await;
    assert!(
        matches!(stalled, Err(NovaSonicError::Stall { .. })),
        "{stalled:?}"
    );
    frames
        .send(Ok(json!({"event": {"completionStart": {
            "sessionId": "s", "promptName": "p", "completionId": "c"
        }}})
        .to_string()
        .into_bytes()))
        .await
        .unwrap();
    let next = reader.recv().await;
    assert!(
        matches!(next, Ok(Some(OutputEvent::CompletionStart(_)))),
        "{next:?}"
    );
}

/// Section "3. InputContentStartEvent", "Text": the five roles and `interactive: true` for
/// cross-modal input.
#[test]
fn text_content_starts_carry_each_role_and_the_interactive_flag() {
    for (role, wire) in [
        (Role::System, "SYSTEM"),
        (Role::User, "USER"),
        (Role::Assistant, "ASSISTANT"),
        (Role::Tool, "TOOL"),
        (Role::SystemSpeech, "SYSTEM_SPEECH"),
    ] {
        let value: Value = serde_json::from_slice(
            &InputEvent::ContentStart {
                prompt_name: "p".into(),
                content_name: "c".into(),
                content: InputContent::Text {
                    role,
                    interactive: true,
                },
            }
            .encode(),
        )
        .unwrap();
        let start = &value["event"]["contentStart"];
        assert_eq!(start["role"], wire);
        assert_eq!(start["interactive"], true);
    }
}

/// Section "1. RequestStartEvent": the three sensitivities.
#[test]
fn every_endpointing_sensitivity_has_its_wire_name() {
    for (endpointing, wire) in [
        (Endpointing::High, "HIGH"),
        (Endpointing::Medium, "MEDIUM"),
        (Endpointing::Low, "LOW"),
    ] {
        let value: Value = serde_json::from_slice(
            &InputEvent::SessionStart {
                inference: InferenceConfiguration::default(),
                endpointing: Some(endpointing),
            }
            .encode(),
        )
        .unwrap();
        assert_eq!(
            value["event"]["sessionStart"]["turnDetectionConfiguration"]["endpointingSensitivity"],
            wire
        );
    }
}
