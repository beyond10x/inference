//! Adversary probe: `SpeechSessionHandle::close` says every event the binding received before the
//! close reaches the caller before `SpeechSessionClosed`, "a stream end or provider failure among
//! them is not". An undecodable frame is neither a stream end nor the last thing on the stream:
//! the reader keeps forwarding after it, so a usage delta that follows it was received before the
//! close too. Offline: a scripted provider stream over in-process channels, no network, no
//! credential.
#![cfg(feature = "duplex")]

use std::future::Future;
use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use inference_aws::duplex::{Connector, NovaSonicSpeech};
use inference_aws::nova_sonic::{DEFAULT_MODEL_ID, NovaSonicError, Session, SessionConfig};
use inference_duplex::inference_audio::{AudioEncoding, AudioFormat};
use inference_duplex::{
    NativeSpeechModel, Quantity, QuantityKind, SessionConfig as SpeechConfig, SessionEvent,
    SpeechSessionHandle, SpeechSessionState, TurnDetection, TurnTuning,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

const WAIT: Duration = Duration::from_millis(100);

/// A provider that sends `frames` in order once the session opens, then stays open.
#[derive(Debug, Clone)]
struct Scripted {
    frames: Vec<Vec<u8>>,
}

impl Connector for Scripted {
    fn model_id(&self) -> &str {
        DEFAULT_MODEL_ID
    }

    fn connect(
        &self,
        config: SessionConfig,
    ) -> impl Future<Output = Result<Session, NovaSonicError>> + Send + 'static {
        let script = self.frames.clone();
        async move {
            let (input, mut received) = mpsc::channel::<Vec<u8>>(256);
            let (frames, output) = mpsc::channel(256);
            let session = Session::open(&config, input, output).await?;
            tokio::spawn(async move {
                for frame in script {
                    if frames.send(Ok(frame)).await.is_err() {
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

/// One turn with 8 output text tokens after the response, then a frame that is not JSON, then a
/// further delta of 5 output text tokens. The caller reads to the response's end, waits until
/// all of it reached the binding, then closes: 13 output text tokens were received before the
/// close.
#[test]
fn close_counts_usage_received_after_an_undecodable_frame() {
    let mut script: Vec<Vec<u8>> = Vec::new();
    let mut push = |value: Value| script.push(value.to_string().into_bytes());
    push(event("completionStart", json!({})));
    for value in final_text("user-1", "USER", "hi") {
        push(value);
    }
    push(usage([50, 0, 0, 0]));
    for value in final_text("assistant-1", "ASSISTANT", "Hello") {
        push(value);
    }
    push(usage([0, 0, 0, 8]));
    script.push(b"not json".to_vec());
    script.push(usage([0, 0, 0, 5]).to_string().into_bytes());

    let mut session = NovaSonicSpeech::with_connector(Scripted { frames: script })
        .expect("the binding's runtime starts")
        .open(config())
        .expect("opens");
    let mut events = Vec::new();
    loop {
        let event = session.next_event(WAIT).expect("the response completes");
        let completed = matches!(event, SessionEvent::ResponseCompleted(_));
        events.push(event);
        if completed {
            break;
        }
        assert!(events.len() < 500, "the response never completed");
    }
    std::thread::sleep(Duration::from_millis(300));
    session
        .close()
        .expect("closing an open session succeeds whatever already arrived");
    while let Some(event) = session.next_event(WAIT) {
        events.push(event);
        assert!(events.len() < 500, "the session never went quiet");
    }
    assert_eq!(
        session.ledger().session().state(),
        SpeechSessionState::Closed,
        "{events:#?}"
    );
    let records = session.ledger().usage_records();
    assert_eq!(
        total(&session, QuantityKind::OutputTextTokens),
        (13, true),
        "the delta received after the undecodable frame, before the close, is counted: \
         {records:#?}"
    );
}
