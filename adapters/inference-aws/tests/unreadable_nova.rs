//! Frames the binding cannot read, whole or in part. One it cannot read at all is dropped and
//! reading goes on, but it is counted on the session, it is no sign of life for the liveness
//! bound, and a later failure of the session names it; a readable frame after it makes a stall
//! a stall again. One that lacks a field the binding does not need keeps its meaning. And a
//! caller operation after the session ended drops nothing. Offline: a scripted provider stream
//! over in-process channels, no network, no credential.
#![cfg(feature = "duplex")]

use std::future::Future;
use std::num::{NonZeroU16, NonZeroU32};
use std::time::{Duration, Instant};

use inference_aws::duplex::{Connector, NovaSonicSpeech};
use inference_aws::nova_sonic::{DEFAULT_MODEL_ID, NovaSonicError, Session, SessionConfig};
use inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_duplex::{
    NativeSpeechModel, QuantityKind, ResponseState, SessionConfig as SpeechConfig, SessionError,
    SessionEvent, SessionFailureKind, SpeechSessionHandle, SpeechSessionState, TurnDetection,
    TurnTuning,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

const WAIT: Duration = Duration::from_millis(100);

/// One step of the scripted provider's output stream.
#[derive(Debug, Clone)]
enum Step {
    /// An event, sent as its JSON text.
    Event(Value),
    /// Bytes sent as they are.
    Raw(&'static str),
    /// `bytes`, sent every `every` until `for_` has passed.
    Repeat {
        bytes: &'static str,
        every: Duration,
        for_: Duration,
    },
}

/// A provider that sends its steps once the session opens, then stays open.
#[derive(Debug, Clone)]
struct Scripted(Vec<Step>);

impl Connector for Scripted {
    fn model_id(&self) -> &str {
        DEFAULT_MODEL_ID
    }

    fn connect(
        &self,
        config: SessionConfig,
    ) -> impl Future<Output = Result<Session, NovaSonicError>> + Send + 'static {
        let steps = self.0.clone();
        async move {
            let (input, mut received) = mpsc::channel::<Vec<u8>>(256);
            let (frames, output) = mpsc::channel(256);
            let session = Session::open(&config, input, output).await?;
            tokio::spawn(async move {
                for step in steps {
                    let sent = match step {
                        Step::Event(event) => frames.send(Ok(event.to_string().into_bytes())).await,
                        Step::Raw(bytes) => frames.send(Ok(bytes.as_bytes().to_vec())).await,
                        Step::Repeat { bytes, every, for_ } => {
                            let until = tokio::time::Instant::now() + for_;
                            let mut sent = Ok(());
                            while sent.is_ok() && tokio::time::Instant::now() < until {
                                sent = frames.send(Ok(bytes.as_bytes().to_vec())).await;
                                tokio::time::sleep(every).await;
                            }
                            sent
                        }
                    };
                    if sent.is_err() {
                        return;
                    }
                }
                while received.recv().await.is_some() {}
            });
            Ok(session)
        }
    }
}

fn event(name: &str, mut body: Value) -> Step {
    body["sessionId"] = json!("scripted-session");
    body["promptName"] = json!("scripted-prompt");
    body["completionId"] = json!("scripted-completion");
    Step::Event(json!({"event": {name: body}}))
}

fn content_start(id: &str, kind: &str, role: &str) -> Step {
    event(
        "contentStart",
        json!({
            "contentId": id,
            "type": kind,
            "role": role,
            "additionalModelFields": "{\"generationStage\":\"FINAL\"}",
            "textOutputConfiguration": {"mediaType": "text/plain"}
        }),
    )
}

/// The user's final transcript: the user's turn is committed, and the caller awaits the answer.
fn user_turn() -> Vec<Step> {
    vec![
        event("completionStart", json!({})),
        content_start("user-1", "TEXT", "USER"),
        event(
            "textOutput",
            json!({"contentId": "user-1", "content": "hi"}),
        ),
        event(
            "contentEnd",
            json!({"contentId": "user-1", "type": "TEXT", "stopReason": "END_TURN"}),
        ),
    ]
}

/// A usage event of `output` text tokens.
fn usage(output: u64) -> Step {
    event(
        "usageEvent",
        json!({"details": {"delta": {"output": {"textTokens": output}}}}),
    )
}

fn pcm() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).expect("a positive rate"),
        channels: NonZeroU16::MIN,
    }
}

fn config(idle_limit: Duration) -> SpeechConfig {
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
        idle_limit,
    }
}

fn open(steps: Vec<Step>, idle_limit: Duration) -> impl SpeechSessionHandle {
    NovaSonicSpeech::with_connector(Scripted(steps))
        .expect("the binding's runtime starts")
        .open(config(idle_limit))
        .expect("opens")
}

fn drain(session: &mut impl SpeechSessionHandle) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    while let Some(event) = session.next_event(WAIT) {
        events.push(event);
        assert!(events.len() < 500, "the session never went quiet");
    }
    events
}

fn output_text_tokens(session: &impl SpeechSessionHandle) -> i64 {
    session
        .ledger()
        .usage_records()
        .iter()
        .flat_map(|record| record.quantities().to_vec())
        .filter(|quantity| quantity.kind() == QuantityKind::OutputTextTokens)
        .filter_map(|quantity| quantity.value())
        .sum()
}

/// A frame that is not JSON is dropped and the usage after it is counted, but the session counts
/// it and names it by the start of its text.
#[test]
fn an_undecodable_frame_is_counted_on_the_session() {
    let mut steps = user_turn();
    steps.push(content_start("assistant-1", "TEXT", "ASSISTANT"));
    steps.push(Step::Raw("not json at all"));
    steps.push(usage(5));
    let mut session = open(steps, Duration::from_secs(5));
    let events = drain(&mut session);
    assert_eq!(session.ledger().unreadable_frames(), 1, "{events:#?}");
    let last = session.ledger().last_unreadable().unwrap_or_default();
    assert!(last.contains("not json at all"), "{last}");
    session.close().expect("open");
    drain(&mut session);
    assert_eq!(output_text_tokens(&session), 5);
}

/// Frames the binding cannot read are no sign of life: a provider that answers the user's turn
/// only with them fails the session at the liveness bound, as a provider error that names how
/// many there were and the last, not as a stall.
#[test]
fn undecodable_frames_do_not_keep_the_session_alive() {
    let mut steps = user_turn();
    steps.push(Step::Repeat {
        bytes: "<html>bad gateway",
        every: Duration::from_millis(50),
        for_: Duration::from_secs(10),
    });
    let mut session = open(steps, Duration::from_millis(600));
    let started = Instant::now();
    let mut events = Vec::new();
    while started.elapsed() < Duration::from_secs(6) {
        if let Some(event) = session.next_event(WAIT) {
            let failed = matches!(event, SessionEvent::SpeechSessionFailed(_));
            events.push(event);
            if failed {
                break;
            }
        }
    }
    let Some(SessionEvent::SpeechSessionFailed(failed)) = events.last() else {
        panic!("the session did not fail: {events:#?}");
    };
    assert_eq!(
        failed.failure.kind,
        SessionFailureKind::ProviderError,
        "{failed:?}"
    );
    assert!(
        failed.failure.detail.contains("frames") && failed.failure.detail.contains("<html>bad"),
        "{failed:?}"
    );
    assert!(session.ledger().unreadable_frames() > 1);
}

/// A frame the binding cannot read, then a readable one, then silence while the caller awaits
/// the answer: the readable frame was the last, so the stall stays a stall, and its detail
/// still names the frame the binding could not read.
#[test]
fn a_stall_after_a_readable_frame_stays_a_stall_naming_the_unreadable_frame() {
    let mut steps = user_turn();
    steps.push(Step::Raw("not json at all"));
    steps.push(usage(5));
    let mut session = open(steps, Duration::from_millis(600));
    let started = Instant::now();
    let mut events = Vec::new();
    while started.elapsed() < Duration::from_secs(6) {
        if let Some(event) = session.next_event(WAIT) {
            let failed = matches!(event, SessionEvent::SpeechSessionFailed(_));
            events.push(event);
            if failed {
                break;
            }
        }
    }
    let Some(SessionEvent::SpeechSessionFailed(failed)) = events.last() else {
        panic!("the session did not fail: {events:#?}");
    };
    assert_eq!(session.ledger().unreadable_frames(), 1, "{events:#?}");
    assert_eq!(
        failed.failure.kind,
        SessionFailureKind::Stalled,
        "{failed:?}"
    );
    assert!(
        failed.failure.detail.contains("1 frame") && failed.failure.detail.contains("not json"),
        "{failed:?}"
    );
}

/// Audio the caller appends after the close is refused with `NotOpen`, the writer stopped or
/// not, and the session drops nothing for it: no refusal is counted or kept.
#[test]
fn audio_appended_after_the_close_is_refused_and_drops_nothing() {
    let mut session = open(user_turn(), Duration::from_secs(5));
    drain(&mut session);
    session.close().expect("open");
    drain(&mut session);
    let dropped = session.ledger().dropped_count();
    let kept = session.ledger().dropped_events().len();
    let started = Instant::now();
    let mut appended = 0_u64;
    while started.elapsed() < Duration::from_secs(1) {
        let frame = AudioFrame {
            format: pcm(),
            position: SampleClockPosition::new(appended * 160).expect("a position"),
            sample_count: 160,
            data: vec![0; 320],
        };
        assert!(
            matches!(session.append_audio(frame), Err(SessionError::NotOpen(_))),
            "an append after the close was not refused as NotOpen"
        );
        appended += 1;
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(appended > 10, "{appended}");
    assert_eq!(session.ledger().dropped_count(), dropped);
    assert_eq!(session.ledger().dropped_events().len(), kept);
}

/// A `completionEnd` without its `stopReason` still ends the response in progress: the binding
/// never reads the reason.
#[test]
fn a_completion_end_without_its_stop_reason_ends_the_response() {
    let mut steps = user_turn();
    steps.push(content_start("assistant-1", "TEXT", "ASSISTANT"));
    steps.push(event(
        "textOutput",
        json!({"contentId": "assistant-1", "content": "Hello"}),
    ));
    steps.push(event("completionEnd", json!({})));
    let mut session = open(steps, Duration::from_secs(5));
    let events = drain(&mut session);
    let states: Vec<_> = session
        .ledger()
        .responses()
        .iter()
        .map(|response| response.state())
        .collect();
    assert_eq!(states, [ResponseState::Completed], "{events:#?}");
    assert_eq!(session.ledger().session().state(), SpeechSessionState::Open);
}

/// A `contentEnd` that says the user interrupted, without its `contentId` or `type`, still
/// cancels the response in progress: Nova Sonic streams one response at a time.
#[test]
fn a_content_end_without_its_id_still_cancels_the_response_in_progress() {
    let mut steps = user_turn();
    steps.push(content_start("assistant-1", "TEXT", "ASSISTANT"));
    steps.push(event("contentEnd", json!({"stopReason": "INTERRUPTED"})));
    let mut session = open(steps, Duration::from_secs(5));
    let events = drain(&mut session);
    let states: Vec<_> = session
        .ledger()
        .responses()
        .iter()
        .map(|response| response.state())
        .collect();
    assert_eq!(states, [ResponseState::Cancelled], "{events:#?}");
}

/// A `contentStart` without its `contentId`, `type` or `role` opens no block the binding could
/// place; it is not dropped silently but counted as a frame the binding could not read, each
/// named by the field it lacked.
#[test]
fn a_content_start_without_its_id_type_or_role_is_counted_as_unreadable() {
    let mut steps = user_turn();
    for lacking in ["contentId", "type", "role"] {
        let mut body = json!({
            "contentId": "assistant-1",
            "type": "TEXT",
            "role": "ASSISTANT",
            "textOutputConfiguration": {"mediaType": "text/plain"}
        });
        body.as_object_mut().expect("an object").remove(lacking);
        steps.push(event("contentStart", body));
    }
    steps.push(usage(5));
    let mut session = open(steps, Duration::from_secs(5));
    let events = drain(&mut session);
    assert_eq!(session.ledger().unreadable_frames(), 3, "{events:#?}");
    let last = session.ledger().last_unreadable().unwrap_or_default();
    assert!(
        last.contains("contentStart") && last.contains("role"),
        "{last}"
    );
    session.close().expect("open");
    drain(&mut session);
    assert_eq!(output_text_tokens(&session), 5);
}
