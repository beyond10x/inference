//! Adversary: a usage delta the provider sent before the caller closed the session, still waiting
//! in the binding's arrivals when `close` runs. Offline: a scripted provider stream over
//! in-process channels, no network, no credential.
#![cfg(feature = "duplex")]

use std::future::Future;
use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use inference_aws::duplex::{Connector, NovaSonicSpeech};
use inference_aws::nova_sonic::{DEFAULT_MODEL_ID, NovaSonicError, Session, SessionConfig};
use inference_duplex::inference_audio::{AudioEncoding, AudioFormat};
use inference_duplex::{
    NativeSpeechModel, Quantity, QuantityKind, SessionConfig as SpeechConfig, SessionEvent,
    SpeechSessionHandle, TurnDetection, TurnTuning,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

const WAIT: Duration = Duration::from_millis(100);

#[derive(Debug, Clone)]
struct Scripted {
    events: Vec<Value>,
}

impl Connector for Scripted {
    fn model_id(&self) -> &str {
        DEFAULT_MODEL_ID
    }

    fn connect(
        &self,
        config: SessionConfig,
    ) -> impl Future<Output = Result<Session, NovaSonicError>> + Send + 'static {
        let events = self.events.clone();
        async move {
            let (input, mut received) = mpsc::channel::<Vec<u8>>(256);
            let (frames, output) = mpsc::channel(256);
            let session = Session::open(&config, input, output).await?;
            tokio::spawn(async move {
                for event in events {
                    if frames
                        .send(Ok(event.to_string().into_bytes()))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                while received.recv().await.is_some() {}
            });
            Ok(session)
        }
    }
}

fn event(name: &str, mut body: Value) -> Value {
    body["sessionId"] = json!("scripted-session");
    body["promptName"] = json!("scripted-prompt");
    body["completionId"] = json!("scripted-completion");
    json!({"event": {name: body}})
}

fn final_text(id: &str, role: &str, text: &str) -> Vec<Value> {
    vec![
        event(
            "contentStart",
            json!({
                "contentId": id,
                "type": "TEXT",
                "role": role,
                "additionalModelFields": "{\"generationStage\":\"FINAL\"}",
                "textOutputConfiguration": {"mediaType": "text/plain"}
            }),
        ),
        event("textOutput", json!({"contentId": id, "content": text})),
        event(
            "contentEnd",
            json!({"contentId": id, "type": "TEXT", "stopReason": "END_TURN"}),
        ),
    ]
}

fn usage(delta: [u64; 4]) -> Value {
    event(
        "usageEvent",
        json!({
            "details": {
                "delta": {
                    "input": {"speechTokens": delta[0], "textTokens": delta[1]},
                    "output": {"speechTokens": delta[2], "textTokens": delta[3]}
                },
                "total": {
                    "input": {"speechTokens": 0, "textTokens": 0},
                    "output": {"speechTokens": 0, "textTokens": 0}
                }
            }
        }),
    )
}

fn pcm() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).expect("a positive rate"),
        channels: NonZeroU16::MIN,
    }
}

fn config() -> SpeechConfig {
    SpeechConfig {
        voice: "tiffany".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: pcm(),
        output_format: pcm(),
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools: Vec::new(),
        idle_limit: Duration::from_secs(5),
    }
}

fn total(session: &impl SpeechSessionHandle, kind: QuantityKind) -> (i64, bool) {
    let quantities: Vec<Quantity> = session
        .ledger()
        .usage_records()
        .iter()
        .flat_map(|record| record.quantities().to_vec())
        .filter(|quantity| quantity.kind() == kind)
        .collect();
    (
        quantities.iter().filter_map(Quantity::value).sum(),
        quantities.iter().all(|quantity| quantity.value().is_some()),
    )
}

/// The caller reads events until its response completes, then closes, the common shape of a
/// one-turn session. The provider's usage delta for that response followed the response's end
/// on the stream and reached the binding before the close (the pause makes sure of it), so the
/// session total counts its 8 output text tokens.
#[test]
fn usage_that_arrived_before_close_is_counted() {
    let mut events = vec![event("completionStart", json!({}))];
    events.extend(final_text("user-1", "USER", "hi"));
    events.push(usage([50, 0, 0, 0]));
    events.extend(final_text("assistant-1", "ASSISTANT", "Hello"));
    events.push(usage([0, 0, 0, 8]));
    let mut session = NovaSonicSpeech::with_connector(Scripted { events })
        .expect("the binding's runtime starts")
        .open(config())
        .expect("opens");
    let mut seen = Vec::new();
    loop {
        let event = session.next_event(WAIT).expect("the response completes");
        let completed = matches!(event, SessionEvent::ResponseCompleted(_));
        seen.push(event);
        if completed {
            break;
        }
        assert!(seen.len() < 500, "the response never completed");
    }
    std::thread::sleep(Duration::from_millis(300));
    session.close().expect("open");
    while let Some(event) = session.next_event(WAIT) {
        seen.push(event);
    }
    let records = session.ledger().usage_records();
    assert_eq!(
        total(&session, QuantityKind::OutputTextTokens),
        (8, true),
        "{records:#?}"
    );
    assert_eq!(
        total(&session, QuantityKind::InputAudioTokens),
        (50, true),
        "{records:#?}"
    );
}
