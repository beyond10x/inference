//! Frames the binding could not read: each is counted on the session, none refreshes the liveness
//! bound, and a later failure of the session names them, a stall becoming a provider error. A
//! response the provider ended with an outcome the binding could not read fails, and the caller
//! still awaits the provider.

use std::num::{NonZeroU16, NonZeroU32};
use std::time::{Duration, Instant};

use inference_duplex::inference_audio::{AudioEncoding, AudioFormat};
use inference_duplex::inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_duplex::{
    ResponseId, ResponseState, SessionConfig, SessionEvent, SessionFailure, SessionFailureKind,
    SessionLedger, SpeechCapabilities, SpeechSessionId, TuningSupport, TurnDetection, TurnTuning,
};

const BOUND: Duration = Duration::from_millis(400);

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

fn capabilities() -> SpeechCapabilities {
    SpeechCapabilities {
        input_formats: vec![pcm()],
        output_formats: vec![pcm()],
        turn_detection: TurnDetection::ALL.to_vec(),
        turn_tuning: TuningSupport::ALL,
        manual_commit: true,
        manual_response: true,
        manual_interrupt: true,
        client_cancel: true,
        truncation: true,
        tool_calling: true,
    }
}

fn open(now: Instant) -> SessionLedger {
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
        &adapter(),
        &capabilities(),
        config,
        now,
    )
    .expect("the session opens")
    .0
}

fn failure(events: &[SessionEvent]) -> SessionFailure {
    match events.last() {
        Some(SessionEvent::SpeechSessionFailed(failed)) => failed.failure.clone(),
        other => panic!("the session did not fail: {other:?} in {events:#?}"),
    }
}

#[test]
fn unreadable_frames_are_counted_and_do_not_refresh_the_bound() {
    let start = Instant::now();
    let mut ledger = open(start);
    ledger.request_response(start).expect("none in progress");
    ledger.unreadable_frame(
        "server event is not JSON; it begins \"not json\"",
        start + BOUND / 2,
    );
    ledger.unreadable_frame(
        "server event is not JSON; it begins \"still not\"",
        start + BOUND - Duration::from_millis(1),
    );
    assert_eq!(ledger.unreadable_frames(), 2);
    assert_eq!(
        ledger.last_unreadable(),
        Some("server event is not JSON; it begins \"still not\"")
    );
    assert!(
        ledger.check_liveness(start + BOUND),
        "the provider sent nothing readable for the whole bound"
    );
    let events: Vec<_> = std::iter::from_fn(|| ledger.next_event()).collect();
    let failure = failure(&events);
    assert_eq!(
        failure.kind,
        SessionFailureKind::ProviderError,
        "{failure:?}"
    );
    assert!(failure.detail.contains("2 frames"), "{failure:?}");
    assert!(failure.detail.contains("still not"), "{failure:?}");
}

/// A failure whose kind was stated, by the transport or by the provider, keeps it after frames
/// the binding could not read; its detail names them.
#[test]
fn a_stated_failure_after_unreadable_frames_keeps_its_kind_and_names_them() {
    for kind in [
        SessionFailureKind::TransportClosed,
        SessionFailureKind::Rejected,
        SessionFailureKind::ProviderError,
    ] {
        let start = Instant::now();
        let mut ledger = open(start);
        ledger.unreadable_frame("`response.done` lacks the object field `response`", start);
        ledger
            .fail(SessionFailure {
                kind,
                detail: "the provider closed the connection".to_owned(),
            })
            .expect("open");
        let events: Vec<_> = std::iter::from_fn(|| ledger.next_event()).collect();
        let failure = failure(&events);
        assert_eq!(failure.kind, kind, "{failure:?}");
        assert!(
            failure
                .detail
                .contains("the provider closed the connection")
                && failure.detail.contains("1 frame ")
                && failure.detail.contains("`response.done`"),
            "{failure:?}"
        );
    }
}

#[test]
fn a_failure_without_unreadable_frames_keeps_its_kind() {
    let start = Instant::now();
    let mut ledger = open(start);
    ledger.request_response(start).expect("none in progress");
    assert!(ledger.check_liveness(start + BOUND));
    let events: Vec<_> = std::iter::from_fn(|| ledger.next_event()).collect();
    assert_eq!(failure(&events).kind, SessionFailureKind::Stalled);
}

#[test]
fn a_response_ended_with_an_unread_outcome_fails_and_the_bound_runs_on() {
    let start = Instant::now();
    let mut ledger = open(start);
    ledger.request_response(start).expect("none in progress");
    let response = ResponseId::generate();
    ledger.response_created(response, start).expect("created");
    let ended = start + Duration::from_millis(100);
    ledger
        .response_ended_unread(
            response,
            "`response.done` lacks the string field `status`",
            ended,
        )
        .expect("in progress");
    assert_eq!(
        ledger.response(response).map(|response| response.state()),
        Some(ResponseState::Failed)
    );
    assert_eq!(ledger.unreadable_frames(), 1);
    assert!(
        !ledger.check_liveness(ended + BOUND - Duration::from_millis(1)),
        "the end was heard: the bound runs from it"
    );
    assert!(
        ledger.check_liveness(ended + BOUND),
        "the caller still awaits the provider after an end it could not read"
    );
    let events: Vec<_> = std::iter::from_fn(|| ledger.next_event()).collect();
    assert!(
        events.iter().any(|event| matches!(
            event,
            SessionEvent::ResponseFailed(failed)
                if failed.response_id == response
                    && failed.failure.kind == SessionFailureKind::ProviderError
                    && failed.failure.detail.contains("`status`")
        )),
        "{events:#?}"
    );
    let failure = failure(&events);
    assert_eq!(
        failure.kind,
        SessionFailureKind::ProviderError,
        "{failure:?}"
    );
    assert!(failure.detail.contains("response.done"), "{failure:?}");
}

#[test]
fn a_response_created_after_an_unread_end_satisfies_the_wait() {
    let start = Instant::now();
    let mut ledger = open(start);
    ledger.request_response(start).expect("none in progress");
    let first = ResponseId::generate();
    ledger.response_created(first, start).expect("created");
    ledger
        .response_ended_unread(
            first,
            "`response.done` lacks the string field `status`",
            start,
        )
        .expect("in progress");
    let second = ResponseId::generate();
    ledger
        .response_created(second, start + Duration::from_millis(10))
        .expect("the unread end left no response in progress");
    ledger
        .response_completed(second, start + Duration::from_millis(20))
        .expect("in progress");
    assert!(
        !ledger.check_liveness(start + BOUND * 3),
        "nothing is awaited"
    );
}
