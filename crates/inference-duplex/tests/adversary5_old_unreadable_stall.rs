//! Adversary case, pass 5, story `duplex-late-usage-counted` at 457724a: which stall becomes a
//! provider error.
//!
//! `SpeechSessionHandle::next_event` (contract.rs): "A provider silent beyond the liveness bound
//! yields `SpeechSessionFailed` with `SessionFailureKind::Stalled`". `SessionLedger::fail` turns a
//! stall into a provider error after unreadable frames because "a provider that kept sending
//! frames the binding could not read was not silent". `after_unreadable` checks only that one
//! such frame ever arrived: one unreadable frame early in a session, followed by a readable,
//! completed response, makes a later plain silence a provider error for the rest of the session.

use std::num::{NonZeroU16, NonZeroU32};
use std::time::{Duration, Instant};

use inference_duplex::inference_audio::{AudioEncoding, AudioFormat};
use inference_duplex::inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_duplex::{
    ResponseId, SessionConfig, SessionEvent, SessionFailureKind, SessionLedger, SpeechCapabilities,
    SpeechSessionId, TuningSupport, TurnDetection, TurnTuning,
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
        tool_calling: true,
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

#[test]
fn a_silence_after_readable_traffic_is_a_stall_whatever_came_before() {
    let start = Instant::now();
    let mut ledger = open(start);
    ledger.unreadable_frame("server event is not JSON; it begins \"<html>\"", start);
    ledger.request_response(start).expect("none in progress");
    let first = ResponseId::generate();
    ledger
        .response_created(first, start + Duration::from_millis(10))
        .expect("created");
    ledger
        .response_completed(first, start + Duration::from_millis(20))
        .expect("in progress");
    let asked = start + BOUND * 10;
    ledger.request_response(asked).expect("none in progress");
    assert!(
        ledger.check_liveness(asked + BOUND),
        "the provider sent nothing at all for the whole bound"
    );
    let events: Vec<_> = std::iter::from_fn(|| ledger.next_event()).collect();
    let failure = match events.last() {
        Some(SessionEvent::SpeechSessionFailed(failed)) => failed.failure.clone(),
        other => panic!("the session did not fail: {other:?}"),
    };
    assert_eq!(
        failure.kind,
        SessionFailureKind::Stalled,
        "the provider was silent for the whole bound after readable traffic; one unreadable \
         frame long before made it a provider error: {failure:?}"
    );
}
