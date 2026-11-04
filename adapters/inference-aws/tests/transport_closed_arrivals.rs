//! A command that finds the provider's stream closed: usage the binding received before it, still
//! waiting in its arrivals, is in the session's usage records and counted once, and the session
//! fails as the first stream end, client failure or service error among those arrivals says, or
//! as a closed transport when none is among them. Offline: a scripted provider stream over
//! in-process channels, no network, no credential.
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
    NativeSpeechModel, QuantityKind, SessionConfig as SpeechConfig, SessionEvent, SessionFailure,
    SessionFailureKind, SpeechSessionHandle, SpeechSessionState, TurnDetection, TurnTuning,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

const WAIT: Duration = Duration::from_millis(100);

/// The pause that lets the scripted provider's tasks and the binding's writer settle.
const SETTLE: Duration = Duration::from_millis(300);

/// What the scripted provider does once its events are sent. In each case it stops taking input,
/// so the binding's next send fails and its command channel closes.
#[derive(Debug, Clone)]
enum After {
    /// Its output stays open.
    StaysOpen,
    /// Ends its output stream first.
    Ends,
    /// Reports a service error on its output stream first.
    Errors(ServiceError),
}

/// A provider that sends `events` in order once the session opens, then does what `after` says.
#[derive(Debug, Clone)]
struct Scripted {
    events: Vec<Value>,
    after: After,
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
            let (input, received) = mpsc::channel::<Vec<u8>>(256);
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
                    After::StaysOpen => {
                        drop(received);
                        frames.closed().await;
                    }
                    After::Ends => {
                        drop(frames);
                        drop(received);
                    }
                    After::Errors(error) => {
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

/// One turn whose usage delta of 8 output text tokens follows the response's end on the stream.
fn one_turn() -> Vec<Value> {
    let mut events = vec![event("completionStart", json!({}))];
    events.extend(final_text("user-1", "USER", "hi"));
    events.push(usage([50, 0, 0, 0]));
    events.extend(final_text("assistant-1", "ASSISTANT", "Hello"));
    events.push(usage([0, 0, 0, 8]));
    events
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

fn drain(session: &mut impl SpeechSessionHandle) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    while let Some(event) = session.next_event(WAIT) {
        events.push(event);
        assert!(events.len() < 500, "the session never went quiet");
    }
    events
}

/// The values of `kind` over the session's usage records, one per record that carries it.
fn values(session: &impl SpeechSessionHandle, kind: QuantityKind) -> Vec<Option<i64>> {
    session
        .ledger()
        .usage_records()
        .iter()
        .flat_map(|record| record.quantities().to_vec())
        .filter(|quantity| quantity.kind() == kind)
        .map(|quantity| quantity.value())
        .collect()
}

fn failures(events: &[SessionEvent]) -> Vec<SessionFailure> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::SpeechSessionFailed(failed) => Some(failed.failure.clone()),
            _ => None,
        })
        .collect()
}

/// What the caller sends once the provider stopped taking input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Next {
    AppendAudio,
    SubmitToolResult,
}

/// What one run observed.
struct Measured {
    events: Vec<SessionEvent>,
    session: inference_aws::duplex::NovaSonicSpeechSession,
    proposed: Option<inference_duplex::SpeechCallId>,
}

/// The caller reads to the response's end, so the delta of 8 output text tokens and what `after`
/// sends first stay in the binding's arrivals, and sends one frame, which the writer fails to
/// send. The caller's `next` command then finds the command channel closed.
fn measure(after: After, next: Next) -> (Measured, Result<(), inference_duplex::SessionError>) {
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
    assert_eq!(
        values(&session, QuantityKind::OutputTextTokens),
        Vec::<Option<i64>>::new(),
        "the delta has not been recorded before the command: {events:#?}"
    );
    std::thread::sleep(SETTLE);
    session
        .append_audio(silence(0))
        .expect("the writer has not failed yet");
    std::thread::sleep(SETTLE);
    let proposed = events.iter().find_map(|event| match event {
        SessionEvent::SpeechToolCallProposed(proposed) => Some(proposed.call_id.clone()),
        _ => None,
    });
    let refused = match next {
        Next::AppendAudio => session.append_audio(silence(320)),
        Next::SubmitToolResult => session.submit_tool_result(
            proposed.as_ref().expect("a proposed call"),
            json!({"celsius": 21}),
        ),
    };
    events.extend(drain(&mut session));
    (
        Measured {
            events,
            session,
            proposed,
        },
        refused,
    )
}

/// The delta counts once, in one record, with its usage event delivered once per record; the
/// session fails once, as `kind`, with `detail` when one is named, and refuses the command.
fn holds(after: After, next: Next, kind: SessionFailureKind, detail: Option<&str>) -> Measured {
    let (measured, refused) = measure(after, next);
    let Measured {
        events, session, ..
    } = &measured;
    let records = session.ledger().usage_records();
    assert_eq!(
        values(session, QuantityKind::OutputTextTokens),
        [Some(8)],
        "the usage that arrived before the closed stream is recorded once: {records:#?}"
    );
    assert_eq!(
        values(session, QuantityKind::InputAudioTokens),
        [Some(50)],
        "{records:#?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, SessionEvent::UsageRecorded(_)))
            .count(),
        records.len(),
        "{events:#?}"
    );
    let failed = failures(events);
    assert_eq!(
        failed
            .iter()
            .map(|failure| failure.kind)
            .collect::<Vec<_>>(),
        [kind],
        "{events:#?}"
    );
    if let Some(detail) = detail {
        assert_eq!(failed[0].detail, detail, "{events:#?}");
    }
    assert_eq!(
        session.ledger().session().state(),
        SpeechSessionState::Failed,
        "{events:#?}"
    );
    assert!(
        refused.is_err(),
        "the command on a failed session is refused: {refused:?}"
    );
    measured
}

fn service_error() -> ServiceError {
    ServiceError::new(ServiceErrorKind::ModelStreamError, "it failed")
}

#[test]
fn audio_on_a_closed_stream_records_the_usage_that_arrived_before_it() {
    holds(
        After::StaysOpen,
        Next::AppendAudio,
        SessionFailureKind::TransportClosed,
        None,
    );
}

#[test]
fn a_tool_result_on_a_closed_stream_records_the_usage_that_arrived_before_it() {
    holds(
        After::StaysOpen,
        Next::SubmitToolResult,
        SessionFailureKind::TransportClosed,
        None,
    );
}

#[test]
fn audio_on_a_closed_stream_fails_as_the_stream_end_that_arrived_first() {
    holds(
        After::Ends,
        Next::AppendAudio,
        SessionFailureKind::TransportClosed,
        Some("the provider's stream closed"),
    );
}

#[test]
fn a_tool_result_on_a_closed_stream_fails_as_the_stream_end_that_arrived_first() {
    holds(
        After::Ends,
        Next::SubmitToolResult,
        SessionFailureKind::TransportClosed,
        Some("the provider's stream closed"),
    );
}

#[test]
fn audio_on_a_closed_stream_fails_as_the_service_error_that_arrived_first() {
    holds(
        After::Errors(service_error()),
        Next::AppendAudio,
        SessionFailureKind::ProviderError,
        None,
    );
}

#[test]
fn a_tool_result_on_a_closed_stream_fails_as_the_service_error_that_arrived_first() {
    holds(
        After::Errors(service_error()),
        Next::SubmitToolResult,
        SessionFailureKind::ProviderError,
        None,
    );
}

#[test]
fn commands_after_the_failure_record_nothing_more() {
    let mut measured = holds(
        After::StaysOpen,
        Next::SubmitToolResult,
        SessionFailureKind::TransportClosed,
        None,
    );
    let records = measured.session.ledger().usage_records().len();
    let call_id = measured.proposed.clone().expect("a proposed call");
    let audio = measured.session.append_audio(silence(640));
    let result = measured
        .session
        .submit_tool_result(&call_id, json!({"celsius": 21}));
    let later = drain(&mut measured.session);
    assert!(audio.is_err() && result.is_err(), "{audio:?} {result:?}");
    assert_eq!(
        measured.session.ledger().usage_records().len(),
        records,
        "{later:#?}"
    );
    assert_eq!(
        values(&measured.session, QuantityKind::OutputTextTokens),
        [Some(8)],
        "{later:#?}"
    );
    assert!(failures(&later).is_empty(), "{later:#?}");
}
