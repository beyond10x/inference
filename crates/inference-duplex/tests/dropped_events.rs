//! The refusals a binding drops (`SessionLedger::dropped`): every one is counted, and only the
//! most recent `SessionLedger::DROPPED_EVENTS_KEPT` (64) are kept, oldest first, so a session
//! dropping events without end holds no more than that.

use std::num::{NonZeroU16, NonZeroU32};
use std::time::{Duration, Instant};

use inference_duplex::inference_audio::{AudioEncoding, AudioFormat};
use inference_duplex::inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_duplex::{
    SessionConfig, SessionError, SessionLedger, SpeechCapabilities, SpeechSessionId, TuningSupport,
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
        idle_limit: Duration::from_millis(400),
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

fn refusal(n: usize) -> SessionError {
    SessionError::ToolNotOffered {
        name: format!("tool-{n}"),
    }
}

fn names(ledger: &SessionLedger) -> Vec<String> {
    ledger
        .dropped_events()
        .iter()
        .map(|refusal| match refusal {
            SessionError::ToolNotOffered { name } => name.clone(),
            other => panic!("a refusal the test did not drop: {other:?}"),
        })
        .collect()
}

/// Fewer refusals than the bound: every one is kept, in order, and counted.
#[test]
fn every_refusal_under_the_bound_is_kept_and_counted() {
    let mut ledger = open(Instant::now());
    assert_eq!(ledger.dropped_count(), 0);
    assert!(ledger.dropped_events().is_empty());
    for n in 0..3 {
        ledger.dropped(refusal(n));
    }
    assert_eq!(ledger.dropped_count(), 3);
    assert_eq!(names(&ledger), ["tool-0", "tool-1", "tool-2"]);
}

/// More refusals than the bound: the count names every one, and only the most recent 64 are
/// kept, oldest first.
#[test]
fn only_the_most_recent_refusals_are_kept_while_every_one_is_counted() {
    assert_eq!(SessionLedger::DROPPED_EVENTS_KEPT, 64);
    let mut ledger = open(Instant::now());
    for n in 0..1_000 {
        ledger.dropped(refusal(n));
    }
    assert_eq!(ledger.dropped_count(), 1_000);
    let expected: Vec<String> = (1_000 - 64..1_000).map(|n| format!("tool-{n}")).collect();
    assert_eq!(names(&ledger), expected);
}
