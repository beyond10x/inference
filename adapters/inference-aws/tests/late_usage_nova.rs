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
use inference_aws::nova_sonic::{
    DEFAULT_MODEL_ID, NovaSonicError, ServiceError, ServiceErrorKind, Session, SessionConfig,
};
use inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_duplex::inference_tools::{DeclareTool, SideEffectClass, Tool};
use inference_duplex::{
    NativeSpeechModel, Quantity, QuantityKind, SessionConfig as SpeechConfig, SessionEvent,
    SessionFailureKind, SpeechSessionHandle, SpeechSessionState, TurnDetection, TurnTuning,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

const WAIT: Duration = Duration::from_millis(100);

/// A provider that sends `events` in order once the session opens, then does what `after` says.
#[derive(Debug, Clone)]
struct Scripted {
    events: Vec<Value>,
    after: After,
}

impl Scripted {
    fn open(events: Vec<Value>) -> Self {
        Scripted {
            events,
            after: After::StaysOpen,
        }
    }
}

/// What the scripted provider does once its events are sent.
#[derive(Debug, Clone)]
enum After {
    /// Stays open until the session ends.
    StaysOpen,
    /// Ends its output stream.
    Ends,
    /// Reports a service error on its output stream.
    Errors(ServiceError),
    /// Stops taking input, so the binding's next send fails; its output stays open.
    RefusesInput,
    /// Reports a service error on its output stream, then stops taking input as `RefusesInput`
    /// does.
    ErrorsAndRefusesInput(ServiceError),
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
        let after = self.after.clone();
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
                match after {
                    After::StaysOpen => while received.recv().await.is_some() {},
                    After::Ends => {}
                    After::Errors(error) => {
                        let _ = frames.send(Err(error)).await;
                        while received.recv().await.is_some() {}
                    }
                    After::RefusesInput => {
                        drop(received);
                        frames.closed().await;
                    }
                    After::ErrorsAndRefusesInput(error) => {
                        let _ = frames.send(Err(error)).await;
                        drop(received);
                        frames.closed().await;
                    }
                }
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
    let mut session = NovaSonicSpeech::with_connector(Scripted::open(script(late, when)))
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

/// One turn whose usage delta of 8 output text tokens follows the response's end on the stream.
fn one_turn() -> Vec<Value> {
    let mut events = vec![event("completionStart", json!({}))];
    events.extend(final_text("user-1", "USER", "hi"));
    events.push(usage([50, 0, 0, 0]));
    events.extend(final_text("assistant-1", "ASSISTANT", "Hello"));
    events.push(usage([0, 0, 0, 8]));
    events
}

/// A 20 ms frame of silence in the session's input format.
fn silence(position: u64) -> AudioFrame {
    let format = pcm();
    let samples = format.sample_rate.get() / 50;
    AudioFrame {
        format,
        position: SampleClockPosition::new(position).expect("a position"),
        sample_count: samples,
        data: vec![0; usize::try_from(format.data_len(samples)).expect("a length")],
    }
}

/// The caller reads to the response's end, waits, then closes, while the usage delta and then
/// `after` already reached the binding. Closing an open session succeeds whatever waits: the
/// session ends closed, not failed, and the usage that arrived before the failure is counted.
fn closes_over(after: After, refused_send: bool) {
    let mut session = NovaSonicSpeech::with_connector(Scripted {
        events: one_turn(),
        after,
    })
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
    if refused_send {
        session
            .append_audio(silence(0))
            .expect("the writer has not failed yet");
        std::thread::sleep(Duration::from_millis(300));
    }
    session
        .close()
        .expect("closing an open session succeeds whatever already arrived");
    events.extend(drain(&mut session));
    assert_eq!(
        session.ledger().session().state(),
        SpeechSessionState::Closed,
        "{events:#?}"
    );
    assert!(
        matches!(events.last(), Some(SessionEvent::SpeechSessionClosed(_))),
        "{events:#?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::SpeechSessionFailed(_))),
        "{events:#?}"
    );
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

#[test]
fn close_succeeds_over_a_service_error_that_already_arrived() {
    closes_over(
        After::Errors(ServiceError::new(
            ServiceErrorKind::ModelStreamError,
            "it failed",
        )),
        false,
    );
}

#[test]
fn close_succeeds_over_a_stream_end_that_already_arrived() {
    closes_over(After::Ends, false);
}

#[test]
fn close_succeeds_over_a_failed_send_that_already_arrived() {
    closes_over(After::RefusesInput, true);
}

fn weather() -> Tool {
    Tool::declare(DeclareTool {
        name: "weather".to_owned(),
        schema: json!({"type": "object", "properties": {"city": {"type": "string"}}}),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0
}

/// One turn whose response proposes a call of `weather`; its usage delta of 8 output text tokens
/// follows the response's end on the stream.
fn one_call() -> Vec<Value> {
    let mut events = vec![event("completionStart", json!({}))];
    events.extend(final_text("user-1", "USER", "how warm is it"));
    events.push(usage([50, 0, 0, 0]));
    events.extend([
        event(
            "contentStart",
            json!({
                "contentId": "tool-1",
                "type": "TOOL",
                "role": "TOOL",
                "toolUseOutputConfiguration": {"mediaType": "application/json"}
            }),
        ),
        event(
            "toolUse",
            json!({
                "contentId": "tool-1",
                "toolUseId": "tooluse-1",
                "toolName": "weather",
                "content": "{\"city\":\"Berlin\"}"
            }),
        ),
        event(
            "contentEnd",
            json!({"contentId": "tool-1", "type": "TOOL", "stopReason": "TOOL_USE"}),
        ),
    ]);
    events.push(usage([0, 0, 0, 8]));
    events
}

/// What the caller does once the provider stopped taking input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Next {
    AppendAudio,
    SubmitToolResult,
}

/// The caller reads to the response's end and sends one frame, which the writer fails to send
/// as the provider stopped taking input, while the usage delta and whatever `after` sends first
/// already reached the binding. The caller's `next` command finds the command channel closed:
/// what arrived is recorded first, so the usage delta counts once, and the session fails as the
/// first arrival that ends it says, `kind`.
fn fails_over(after: After, next: Next, kind: SessionFailureKind) {
    let mut config = config();
    let script = match next {
        Next::AppendAudio => one_turn(),
        Next::SubmitToolResult => {
            config.tools = vec![weather()];
            one_call()
        }
    };
    let mut session = NovaSonicSpeech::with_connector(Scripted {
        events: script,
        after,
    })
    .expect("the binding's runtime starts")
    .open(config)
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
        .append_audio(silence(0))
        .expect("the writer has not failed yet");
    std::thread::sleep(Duration::from_millis(300));
    let refused = match next {
        Next::AppendAudio => session.append_audio(silence(320)),
        Next::SubmitToolResult => {
            let call_id = events
                .iter()
                .find_map(|event| match event {
                    SessionEvent::SpeechToolCallProposed(proposed) => {
                        Some(proposed.call_id.clone())
                    }
                    _ => None,
                })
                .expect("a proposed call");
            session.submit_tool_result(&call_id, json!({"celsius": 21}))
        }
    };
    events.extend(drain(&mut session));
    let records = session.ledger().usage_records();
    assert_eq!(
        total(&session, QuantityKind::OutputTextTokens),
        (8, true),
        "the usage that arrived before the failure counts once: {records:#?}"
    );
    assert_eq!(
        total(&session, QuantityKind::InputAudioTokens),
        (50, true),
        "{records:#?}"
    );
    let failures: Vec<SessionFailureKind> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::SpeechSessionFailed(failed) => Some(failed.failure.kind),
            _ => None,
        })
        .collect();
    assert_eq!(failures, [kind], "{events:#?}");
    assert_eq!(
        session.ledger().session().state(),
        SpeechSessionState::Failed,
        "{events:#?}"
    );
    assert!(
        refused.is_err(),
        "the command on a failed session is refused: {refused:?}"
    );
}

#[test]
fn audio_after_a_failed_send_counts_the_usage_that_arrived_before_it() {
    fails_over(
        After::RefusesInput,
        Next::AppendAudio,
        SessionFailureKind::TransportClosed,
    );
}

#[test]
fn a_tool_result_after_a_failed_send_counts_the_usage_that_arrived_before_it() {
    fails_over(
        After::RefusesInput,
        Next::SubmitToolResult,
        SessionFailureKind::TransportClosed,
    );
}

#[test]
fn audio_after_a_failed_send_fails_as_the_service_error_that_arrived_first() {
    fails_over(
        After::ErrorsAndRefusesInput(ServiceError::new(
            ServiceErrorKind::ModelStreamError,
            "it failed",
        )),
        Next::AppendAudio,
        SessionFailureKind::ProviderError,
    );
}

#[test]
fn a_tool_result_after_a_failed_send_fails_as_the_service_error_that_arrived_first() {
    fails_over(
        After::ErrorsAndRefusesInput(ServiceError::new(
            ServiceErrorKind::ModelStreamError,
            "it failed",
        )),
        Next::SubmitToolResult,
        SessionFailureKind::ProviderError,
    );
}
