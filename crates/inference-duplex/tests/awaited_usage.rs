//! A response whose final usage the provider sends in a frame of its own
//! (`SessionLedger::usage_awaited`): the next response does not record it, its final report
//! records it once, the binding's word that the report was lost records it unknown, and the
//! session's end records it unknown before the end event. A report for a response already
//! recorded changes nothing. And which stall a frame the binding could not read turns into a
//! provider error.

use std::num::{NonZeroU16, NonZeroU32};
use std::time::{Duration, Instant};

use inference_duplex::inference_audio::{AudioEncoding, AudioFormat};
use inference_duplex::inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_duplex::{
    Quantity, QuantityKind, ResponseId, ResponseUsage, SessionConfig, SessionError, SessionEvent,
    SessionFailure, SessionFailureKind, SessionLedger, SpeechCapabilities, SpeechSessionId,
    TokenCounts, TuningSupport, TurnDetection, TurnTuning, UsageRecorded, UsageTotal,
};

const BOUND: Duration = Duration::from_millis(400);

fn pcm() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).expect("a positive rate"),
        channels: NonZeroU16::MIN,
    }
}

fn open(now: Instant) -> SessionLedger {
    let adapter = Adapter::declare_as(
        AdapterId::generate(),
        DeclareAdapter {
            model_id: ModelId::generate(),
            backend: "scripted".to_owned(),
            contract_revision: "1".to_owned(),
            execution_form: ExecutionForm::HostedService,
            locality: Locality::ExternalProvider,
            capabilities: vec![Capability::ConcurrentInputOutput],
            max_concurrency: 1,
        },
    )
    .expect("a positive concurrency")
    .0;
    let capabilities = SpeechCapabilities {
        input_formats: vec![pcm()],
        output_formats: vec![pcm()],
        turn_detection: TurnDetection::ALL.to_vec(),
        turn_tuning: TuningSupport::ALL,
        manual_commit: true,
        manual_response: true,
        manual_interrupt: true,
        client_cancel: true,
        truncation: true,
        tool_calling: false,
    };
    let config = SessionConfig {
        voice: "calm".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: pcm(),
        output_format: pcm(),
        turn_detection: TurnDetection::Disabled,
        turn_tuning: TurnTuning::default(),
        auto_response: false,
        auto_interrupt: false,
        tools: Vec::new(),
        idle_limit: BOUND,
    };
    SessionLedger::open(
        SpeechSessionId::generate(),
        &adapter,
        &capabilities,
        config,
        now,
    )
    .expect("the session opens")
    .0
}

fn drain(ledger: &mut SessionLedger) -> Vec<SessionEvent> {
    std::iter::from_fn(|| ledger.next_event()).collect()
}

fn records(events: &[SessionEvent], response: ResponseId) -> Vec<&UsageRecorded> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::UsageRecorded(recorded) if recorded.response_id == Some(response) => {
                Some(recorded)
            }
            _ => None,
        })
        .collect()
}

fn output_audio(record: &UsageRecorded) -> Option<i64> {
    record
        .quantities
        .iter()
        .find(|quantity| quantity.kind() == QuantityKind::OutputAudioTokens)
        .and_then(Quantity::value)
}

fn final_report(ledger: &mut SessionLedger, response: ResponseId, audio: u64, now: Instant) {
    let usage = ResponseUsage {
        input: TokenCounts::default(),
        output: TokenCounts {
            text_tokens: 0,
            audio_tokens: audio,
            cached_tokens: 0,
        },
    };
    ledger
        .response_usage_reported(
            response,
            Some(usage),
            usage.quantities(),
            UsageTotal::Final,
            now,
        )
        .expect("a response not yet recorded");
}

/// A cancelled response whose final usage is awaited: the next response does not record it, and
/// its final report after that records it once, with its counts.
#[test]
fn the_next_response_does_not_record_a_response_whose_usage_is_awaited() {
    let now = Instant::now();
    let mut ledger = open(now);
    ledger.request_response(now).expect("none in progress");
    let first = ResponseId::generate();
    ledger.response_created(first, now).expect("created");
    ledger.usage_awaited(first).expect("not recorded");
    ledger.cancel(first).expect("in progress");
    ledger.request_response(now).expect("none in progress");
    let second = ResponseId::generate();
    ledger.response_created(second, now).expect("created");
    let before = drain(&mut ledger);
    assert!(records(&before, first).is_empty(), "{before:#?}");
    final_report(&mut ledger, first, 12, now);
    let after = drain(&mut ledger);
    let recorded = records(&after, first);
    assert_eq!(recorded.len(), 1, "{after:#?}");
    assert_eq!(output_audio(recorded[0]), Some(12));
}

/// A response whose final usage is awaited and never reported: the close records it once,
/// every count unknown, before `SpeechSessionClosed`.
#[test]
fn the_close_records_an_awaited_usage_never_reported_as_unknown() {
    let now = Instant::now();
    let mut ledger = open(now);
    ledger.request_response(now).expect("none in progress");
    let first = ResponseId::generate();
    ledger.response_created(first, now).expect("created");
    ledger.usage_awaited(first).expect("not recorded");
    ledger.cancel(first).expect("in progress");
    ledger.request_response(now).expect("none in progress");
    let second = ResponseId::generate();
    ledger.response_created(second, now).expect("created");
    ledger.close().expect("open");
    let events = drain(&mut ledger);
    let recorded = records(&events, first);
    assert_eq!(recorded.len(), 1, "{events:#?}");
    assert_eq!(output_audio(recorded[0]), None);
    let recorded_at = events
        .iter()
        .position(|event| {
            matches!(event, SessionEvent::UsageRecorded(record) if record.response_id == Some(first))
        })
        .expect("recorded");
    let closed_at = events
        .iter()
        .position(|event| matches!(event, SessionEvent::SpeechSessionClosed(_)))
        .expect("closed");
    assert!(recorded_at < closed_at, "{events:#?}");
}

/// The binding's word that an awaited report was lost records the response at once, unknown;
/// a report after that is refused and records nothing, not even `ResponseUsageRecorded`.
#[test]
fn a_report_after_the_usage_was_recorded_unknown_records_nothing() {
    let now = Instant::now();
    let mut ledger = open(now);
    ledger.request_response(now).expect("none in progress");
    let first = ResponseId::generate();
    ledger.response_created(first, now).expect("created");
    ledger.usage_awaited(first).expect("not recorded");
    ledger.cancel(first).expect("in progress");
    ledger
        .response_usage_unknown(first, now)
        .expect("not recorded");
    let usage = ResponseUsage {
        input: TokenCounts::default(),
        output: TokenCounts {
            text_tokens: 0,
            audio_tokens: 12,
            cached_tokens: 0,
        },
    };
    assert!(matches!(
        ledger.response_usage_reported(
            first,
            Some(usage),
            usage.quantities(),
            UsageTotal::Final,
            now
        ),
        Err(SessionError::ResponseNotInProgress(_))
    ));
    let events = drain(&mut ledger);
    let recorded = records(&events, first);
    assert_eq!(recorded.len(), 1, "{events:#?}");
    assert_eq!(output_audio(recorded[0]), None);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::ResponseUsageRecorded(_))),
        "{events:#?}"
    );
}

fn failure_of(ledger: &mut SessionLedger) -> SessionFailure {
    match drain(ledger).last() {
        Some(SessionEvent::SpeechSessionFailed(failed)) => failed.failure.clone(),
        other => panic!("the session did not fail: {other:?}"),
    }
}

/// An unreadable frame after the last readable one: the stall is a provider error naming it.
#[test]
fn a_stall_after_an_unreadable_frame_is_a_provider_error() {
    let start = Instant::now();
    let mut ledger = open(start);
    ledger.request_response(start).expect("none in progress");
    ledger.heard(start + Duration::from_millis(10));
    ledger.unreadable_frame(
        "server event is not JSON",
        start + Duration::from_millis(20),
    );
    assert!(ledger.check_liveness(start + BOUND * 2));
    let failure = failure_of(&mut ledger);
    assert_eq!(
        failure.kind,
        SessionFailureKind::ProviderError,
        "{failure:?}"
    );
    assert!(failure.detail.contains("1 frame"), "{failure:?}");
}

/// A readable frame after the unreadable ones: the stall stays a stall, and its detail still
/// names the total count.
#[test]
fn a_stall_after_a_readable_frame_stays_a_stall_naming_the_unreadable_count() {
    let start = Instant::now();
    let mut ledger = open(start);
    ledger.request_response(start).expect("none in progress");
    ledger.unreadable_frame(
        "server event is not JSON",
        start + Duration::from_millis(10),
    );
    ledger.unreadable_frame(
        "server event is not JSON",
        start + Duration::from_millis(20),
    );
    ledger.heard(start + Duration::from_millis(30));
    assert!(ledger.check_liveness(start + BOUND * 2));
    let failure = failure_of(&mut ledger);
    assert_eq!(failure.kind, SessionFailureKind::Stalled, "{failure:?}");
    assert!(failure.detail.contains("2 frames"), "{failure:?}");
}
