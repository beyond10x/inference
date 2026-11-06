//! `SessionLedger::response_usage` after the response's usage record is refused, as
//! `SessionLedger::response_usage_reported` is (`story:contract-usage-and-naming-drift`): no
//! `ResponseUsageRecorded` gives counts its usage record does not carry, and the binding counts
//! the refusal as a dropped event.

use std::num::{NonZeroU16, NonZeroU32};
use std::time::{Duration, Instant};

use inference_duplex::inference_audio::{AudioEncoding, AudioFormat};
use inference_duplex::inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_duplex::{
    ResponseId, ResponseState, ResponseStateConflict, ResponseUsage, SessionConfig, SessionError,
    SessionEvent, SessionLedger, SpeechCapabilities, SpeechSessionId, TokenCounts, TuningSupport,
    TurnDetection, TurnTuning,
};

fn pcm() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).expect("a positive rate"),
        channels: NonZeroU16::MIN,
    }
}

fn open(now: Instant) -> SessionLedger {
    let adapter = Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: "scripted".to_owned(),
        contract_revision: "1".to_owned(),
        execution_form: ExecutionForm::HostedService,
        locality: Locality::ExternalProvider,
        capabilities: vec![Capability::ConcurrentInputOutput],
        max_concurrency: 1,
    })
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
        idle_limit: Duration::from_secs(10),
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

fn usage(output_audio_tokens: u64) -> ResponseUsage {
    ResponseUsage {
        input: TokenCounts::default(),
        output: TokenCounts {
            text_tokens: 1,
            audio_tokens: output_audio_tokens,
            cached_tokens: 0,
        },
    }
}

#[test]
fn response_usage_after_the_usage_record_is_refused_and_recorded_as_a_drop() {
    let now = Instant::now();
    let mut ledger = open(now);
    let response = ResponseId::generate();
    ledger.response_created(response, now).expect("created");
    ledger
        .response_usage(response, usage(9), now)
        .expect("the response is in progress");
    ledger
        .response_completed(response, now)
        .expect("in progress");
    assert_eq!(ledger.usage_records().len(), 1, "recorded when it ended");
    let before: Vec<SessionEvent> = std::iter::from_fn(|| ledger.next_event()).collect();
    assert!(
        before
            .iter()
            .any(|event| matches!(event, SessionEvent::UsageRecorded(_))),
        "{before:#?}"
    );

    let refused = ledger.response_usage(response, usage(40), now);

    assert_eq!(
        refused,
        Err(SessionError::ResponseNotInProgress(ResponseStateConflict {
            current: ResponseState::Completed
        })),
        "a report after the record is refused (story:contract-usage-and-naming-drift)"
    );
    let after: Vec<SessionEvent> = std::iter::from_fn(|| ledger.next_event()).collect();
    assert!(
        !after
            .iter()
            .any(|event| matches!(event, SessionEvent::ResponseUsageRecorded(_))),
        "no ResponseUsageRecorded carries counts the record does not: {after:#?}"
    );
    assert_eq!(ledger.usage(response), Some(usage(9)), "unchanged");
    assert_eq!(ledger.usage_records().len(), 1, "no second record");
    if let Err(refusal) = refused {
        ledger.dropped(refusal);
    }
    assert_eq!(ledger.dropped_count(), 1);
    assert_eq!(
        ledger.dropped_events(),
        [SessionError::ResponseNotInProgress(ResponseStateConflict {
            current: ResponseState::Completed
        })]
    );
}

#[test]
fn response_usage_and_response_usage_reported_refuse_alike_after_the_record() {
    let now = Instant::now();
    let mut ledger = open(now);
    let response = ResponseId::generate();
    ledger.response_created(response, now).expect("created");
    ledger.cancel(response).expect("the caller cancels");
    let next = ResponseId::generate();
    ledger.response_created(next, now).expect("created");
    assert_eq!(
        ledger.usage_records().len(),
        1,
        "the next response settles it"
    );
    let reported = ledger.response_usage_reported(
        response,
        Some(usage(3)),
        Vec::new(),
        inference_duplex::UsageTotal::Final,
        now,
    );
    let contract = ledger.response_usage(response, usage(3), now);
    assert_eq!(reported, contract);
    assert!(contract.is_err(), "{contract:?}");
}
