//! A usage delta the provider sends for a response after the next response began: Nova Sonic's
//! usage events name no response, so the binding counts such a delta toward the response in
//! progress (or, between responses, toward the last one) and never toward one whose usage the
//! ledger already recorded. Offline: a scripted provider stream over in-process channels, no
//! network, no credential.
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

/// A provider that sends `events` in order once the session opens, then stays open until the
/// session ends.
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

/// A final text block of `role` saying `text`, ending `END_TURN`.
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

/// A usage event whose delta is `[input speech, input text, output speech, output text]`.
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

/// Where the late deltas for response 1 arrive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Late {
    /// After response 2 began, while it is in progress.
    NextInProgress,
    /// After response 1 completed and before response 2 began.
    BetweenResponses,
}

/// Two turns: the user's, response 1 with its 8 output text tokens, the user's next turn, then
/// response 2. `late` usage deltas of output text tokens for response 1 arrive `when` says,
/// before response 2 completes.
fn script(late: &[u64], when: Late) -> Vec<Value> {
    let late_events: Vec<Value> = late
        .iter()
        .map(|tokens| usage([0, 0, 0, *tokens]))
        .collect();
    let mut events = vec![event("completionStart", json!({}))];
    events.extend(final_text("user-1", "USER", "hi"));
    events.push(usage([50, 0, 0, 0]));
    events.extend(final_text("assistant-1", "ASSISTANT", "Hello"));
    events.push(usage([0, 0, 0, 8]));
    events.extend(final_text("user-2", "USER", "again"));
    events.push(usage([40, 0, 0, 0]));
    if when == Late::BetweenResponses {
        events.extend(late_events.iter().cloned());
    }
    let start = final_text("assistant-2", "ASSISTANT", "Hello again");
    events.push(start[0].clone());
    if when == Late::NextInProgress {
        events.extend(late_events);
    }
    events.extend(start[1..].iter().cloned());
    events.push(usage([0, 0, 0, 6]));
    events
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

fn drain(session: &mut impl SpeechSessionHandle) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    while let Some(event) = session.next_event(WAIT) {
        events.push(event);
        assert!(events.len() < 500, "the session never went quiet");
    }
    events
}

/// The sum of `kind` over every usage record of the session, and whether every one is known.
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

/// Runs the script with `late` deltas to the session's close and checks the session total
/// counts every output text token the provider reported once: 8 of response 1, the late ones,
/// 6 of response 2. Nothing in the delta names response 1: while response 2 is in progress it
/// carries the late ones; between the two, response 1 does, because the ledger records a
/// running total only when the next response begins.
fn counted_once(late: &[u64], when: Late) {
    let mut session = NovaSonicSpeech::with_connector(Scripted {
        events: script(late, when),
    })
    .expect("the binding's runtime starts")
    .open(config())
    .expect("opens");
    let mut events = drain(&mut session);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, SessionEvent::ResponseCreated(_)))
            .count(),
        2,
        "{events:#?}"
    );
    session.close().expect("open");
    events.extend(drain(&mut session));
    let records = session.ledger().usage_records();
    let recorded = events
        .iter()
        .filter(|event| matches!(event, SessionEvent::UsageRecorded(_)))
        .count();
    assert_eq!(recorded, records.len(), "{events:#?}");
    let expected = 8 + late.iter().sum::<u64>() + 6;
    assert_eq!(
        total(&session, QuantityKind::OutputTextTokens),
        (i64::try_from(expected).expect("small"), true),
        "{records:#?}"
    );
    assert_eq!(
        total(&session, QuantityKind::InputAudioTokens),
        (90, true),
        "{records:#?}"
    );
    let per_response: Vec<Option<i64>> = records
        .iter()
        .filter(|record| record.response_id().is_some())
        .map(|record| {
            record
                .quantities()
                .iter()
                .find(|quantity| quantity.kind() == QuantityKind::OutputTextTokens)
                .and_then(Quantity::value)
        })
        .collect();
    let late = i64::try_from(late.iter().sum::<u64>()).expect("small");
    let expected = match when {
        Late::NextInProgress => vec![Some(8), Some(6 + late)],
        Late::BetweenResponses => vec![Some(8 + late), Some(6)],
    };
    assert_eq!(
        per_response, expected,
        "a delta counts toward the response in progress, or between responses toward the last \
         one: {records:#?}"
    );
}

#[test]
fn one_late_delta_for_a_recorded_response_is_counted_once() {
    counted_once(&[5], Late::NextInProgress);
}

#[test]
fn two_late_deltas_for_a_recorded_response_are_counted_once() {
    counted_once(&[5, 3], Late::NextInProgress);
}

#[test]
fn one_late_delta_between_responses_is_counted_once() {
    counted_once(&[5], Late::BetweenResponses);
}

#[test]
fn two_late_deltas_between_responses_are_counted_once() {
    counted_once(&[5, 3], Late::BetweenResponses);
}
