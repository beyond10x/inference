//! Adversary pass 1 on the usage records of the session ledger: a response that ended with its
//! usage still unsettled when the session ends, the end instant of a response the caller
//! cancelled, a late report for a response already recorded, and a provider that reports
//! `requests` itself.

use std::num::{NonZeroU16, NonZeroU32};
use std::time::{Duration, Instant};

use inference_duplex::inference_audio::{AudioEncoding, AudioFormat};
use inference_duplex::inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_duplex::{
    Quantity, QuantityKind, ResponseId, ResponseState, ResponseStateConflict, ResponseUsage,
    SessionConfig, SessionError, SessionEvent, SessionFailure, SessionFailureKind, SessionLedger,
    SpeechCapabilities, SpeechSessionId, TokenCounts, TuningSupport, TurnDetection, TurnTuning,
    UsageRecorded, UsageTotal,
};

fn pcm() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).expect("a positive rate"),
        channels: NonZeroU16::MIN,
    }
}

fn adapter() -> Adapter {
    Adapter::declare_as(
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
    .0
}

fn capabilities(caller_turns: bool) -> SpeechCapabilities {
    SpeechCapabilities {
        input_formats: vec![pcm()],
        output_formats: vec![pcm()],
        turn_detection: TurnDetection::ALL.to_vec(),
        turn_tuning: TuningSupport::ALL,
        manual_commit: caller_turns,
        manual_response: caller_turns,
        manual_interrupt: caller_turns,
        client_cancel: caller_turns,
        truncation: caller_turns,
        tool_calling: false,
    }
}

fn config(server: bool) -> SessionConfig {
    SessionConfig {
        voice: "calm".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: pcm(),
        output_format: pcm(),
        turn_detection: if server {
            TurnDetection::ServerVad
        } else {
            TurnDetection::Disabled
        },
        turn_tuning: TurnTuning::default(),
        auto_response: server,
        auto_interrupt: server,
        tools: Vec::new(),
        idle_limit: Duration::from_secs(10),
    }
}

fn open(caller_turns: bool, now: Instant) -> SessionLedger {
    SessionLedger::open(
        SpeechSessionId::generate(),
        &adapter(),
        &capabilities(caller_turns),
        config(!caller_turns),
        now,
    )
    .expect("the session opens")
    .0
}

fn drain(ledger: &mut SessionLedger) -> Vec<SessionEvent> {
    std::iter::from_fn(|| ledger.next_event()).collect()
}

fn name(event: &SessionEvent) -> &'static str {
    match event {
        SessionEvent::ResponseCreated(_) => "ResponseCreated",
        SessionEvent::ResponseCompleted(_) => "ResponseCompleted",
        SessionEvent::ResponseCancelled(_) => "ResponseCancelled",
        SessionEvent::ResponseFailed(_) => "ResponseFailed",
        SessionEvent::ResponseUsageRecorded(_) => "ResponseUsageRecorded",
        SessionEvent::SpeechSessionClosed(_) => "SpeechSessionClosed",
        SessionEvent::SpeechSessionFailed(_) => "SpeechSessionFailed",
        SessionEvent::UsageRecorded(_) => "UsageRecorded",
        _ => "other",
    }
}

fn names(events: &[SessionEvent]) -> Vec<&'static str> {
    events.iter().map(name).collect()
}

fn records(events: &[SessionEvent]) -> Vec<&UsageRecorded> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::UsageRecorded(recorded) => Some(recorded),
            _ => None,
        })
        .collect()
}

fn running(ledger: &mut SessionLedger, response: ResponseId, output_audio: i64, now: Instant) {
    ledger
        .response_usage_reported(
            response,
            None,
            vec![Quantity::provider_reported(
                QuantityKind::OutputAudioTokens,
                output_audio,
            )],
            UsageTotal::Running,
            now,
        )
        .expect("a response of the session not yet recorded");
}

fn output_audio(record: &UsageRecorded) -> Option<i64> {
    record
        .quantities
        .iter()
        .find(|quantity| quantity.kind() == QuantityKind::OutputAudioTokens)
        .and_then(Quantity::value)
}

/// The ledger's doc: "the session's end ... every response not yet recorded is recorded before
/// both". A response that completed with running totals only, the last one reported after its
/// end (Nova Sonic's output usage, GPT-Live's backend usage), and no next response: closing the
/// session records it once, with the last total, before `SpeechSessionClosed`.
#[test]
fn closing_records_a_response_that_ended_unsettled_before_the_session_closes() {
    let now = Instant::now();
    let mut ledger = open(false, now);
    let response = ResponseId::generate();
    ledger.response_created(response, now).expect("created");
    running(&mut ledger, response, 5, now);
    ledger
        .response_completed(response, now)
        .expect("in progress");
    running(&mut ledger, response, 8, now);
    assert!(records(&drain(&mut ledger)).is_empty(), "not settled yet");

    ledger.close().expect("open");
    let events = drain(&mut ledger);
    assert_eq!(names(&events), ["UsageRecorded", "SpeechSessionClosed"]);
    let recorded = records(&events);
    assert_eq!(recorded[0].response_id, Some(response));
    assert_eq!(output_audio(recorded[0]), Some(8), "the last running total");
    assert_eq!(ledger.usage_records().len(), 1);
}

/// The same for a failing session, and for a response that failed on its own before: each is
/// recorded once, before `SpeechSessionFailed`.
#[test]
fn failing_records_a_response_that_ended_unsettled_before_the_session_fails() {
    let now = Instant::now();
    let mut ledger = open(false, now);
    let response = ResponseId::generate();
    ledger.response_created(response, now).expect("created");
    running(&mut ledger, response, 3, now);
    ledger
        .response_failed(
            response,
            SessionFailure {
                kind: SessionFailureKind::ProviderError,
                detail: "the response failed".to_owned(),
            },
            now,
        )
        .expect("in progress");
    let ended = drain(&mut ledger);
    assert_eq!(names(&ended), ["ResponseCreated", "ResponseFailed"]);

    ledger
        .fail(SessionFailure {
            kind: SessionFailureKind::TransportClosed,
            detail: "the socket closed".to_owned(),
        })
        .expect("open");
    let events = drain(&mut ledger);
    assert_eq!(names(&events), ["UsageRecorded", "SpeechSessionFailed"]);
    assert_eq!(output_audio(records(&events)[0]), Some(3));
    assert_eq!(ledger.usage_records().len(), 1);
}

/// The ledger's doc: a response "the caller cancelled ... ends at the latest instant the ledger
/// was given". Created at +100 ms, the ledger last given +400 ms (a liveness check) when the
/// caller cancels, its final report at +900 ms: the record spans 300 ms.
#[test]
fn a_callers_cancellation_ends_the_response_at_the_latest_instant_given() {
    let start = Instant::now();
    let mut ledger = open(true, start);
    let response = ResponseId::generate();
    ledger
        .response_created(response, start + Duration::from_millis(100))
        .expect("created");
    ledger.heard(start + Duration::from_millis(250));
    ledger.check_liveness(start + Duration::from_millis(400));
    ledger.cancel(response).expect("in progress");
    ledger
        .response_usage_reported(
            response,
            None,
            Vec::new(),
            UsageTotal::Final,
            start + Duration::from_millis(900),
        )
        .expect("a cancelled response is metered");
    let events = drain(&mut ledger);
    let recorded = records(&events);
    assert_eq!(recorded.len(), 1, "{events:#?}");
    assert_eq!(
        recorded[0].ended_at_ms - recorded[0].started_at_ms,
        300,
        "{:#?}",
        recorded[0]
    );
}

/// A report for a response already recorded, in either form, makes no second record and does
/// not change the first; the next response and the session's end record nothing twice.
#[test]
fn a_late_report_for_a_recorded_response_makes_no_second_record() {
    let now = Instant::now();
    let mut ledger = open(false, now);
    let first = ResponseId::generate();
    ledger.response_created(first, now).expect("created");
    ledger.response_cancelled(first, now).expect("barge-in");
    let second = ResponseId::generate();
    ledger.response_created(second, now).expect("created");
    assert_eq!(
        ledger.response_usage_reported(
            first,
            None,
            vec![Quantity::provider_reported(
                QuantityKind::OutputAudioTokens,
                9
            )],
            UsageTotal::Running,
            now,
        ),
        Err(SessionError::ResponseNotInProgress(ResponseStateConflict {
            current: ResponseState::Cancelled
        }))
    );
    ledger
        .response_usage(
            first,
            ResponseUsage {
                input: TokenCounts::default(),
                output: TokenCounts {
                    text_tokens: 1,
                    audio_tokens: 9,
                    cached_tokens: 0,
                },
            },
            now,
        )
        .expect("the contract's usage is recorded all the same");
    running(&mut ledger, second, 4, now);
    ledger.response_completed(second, now).expect("in progress");
    ledger.close().expect("open");
    let events = drain(&mut ledger);
    let recorded = records(&events);
    assert_eq!(
        recorded
            .iter()
            .map(|record| record.response_id)
            .collect::<Vec<_>>(),
        [Some(first), Some(second)],
        "{events:#?}"
    );
    assert_eq!(
        output_audio(recorded[0]),
        None,
        "recorded unknown, unchanged"
    );
    assert_eq!(output_audio(recorded[1]), Some(4));
    assert_eq!(ledger.usage_records().len(), 2);
}

/// A provider that reports `requests` itself keeps its count, and the record carries one
/// `requests` quantity, not a measured one besides.
#[test]
fn a_reported_request_count_is_not_doubled_by_the_measured_one() {
    let now = Instant::now();
    let mut ledger = open(false, now);
    let response = ResponseId::generate();
    ledger.response_created(response, now).expect("created");
    ledger
        .response_usage_reported(
            response,
            None,
            vec![Quantity::provider_reported(QuantityKind::Requests, 2)],
            UsageTotal::Final,
            now,
        )
        .expect("a response of the session");
    ledger
        .response_completed(response, now)
        .expect("in progress");
    let events = drain(&mut ledger);
    let recorded = records(&events);
    let requests: Vec<&Quantity> = recorded[0]
        .quantities
        .iter()
        .filter(|quantity| quantity.kind() == QuantityKind::Requests)
        .collect();
    assert_eq!(
        requests,
        [&Quantity::provider_reported(QuantityKind::Requests, 2)]
    );
}
