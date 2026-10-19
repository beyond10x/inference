//! The ledger rules added after the first adversary pass: identities never reused, a session's
//! end ending its response, cancellation as a capability of the caller, usage as a running
//! total, and turn tuning a model must honour.

use std::num::{NonZeroU16, NonZeroU32};
use std::time::{Duration, Instant};

use inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_duplex::inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_duplex::inference_tools::{DeclareTool, SideEffectClass, Tool};
use inference_duplex::{
    OpenError, ResponseId, ResponseIdReused, ResponseState, ResponseUsage, SessionConfig,
    SessionError, SessionEvent, SessionFailure, SessionFailureKind, SessionLedger, SpeechCallId,
    SpeechCallIdReused, SpeechCapabilities, SpeechSessionId, SpeechSessionState, TokenCounts,
    TuningOption, TuningSupport, TurnDetection, TurnSensitivity, TurnTuning, Unsupported,
};
use serde_json::json;

fn pcm() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).expect("a positive rate"),
        channels: NonZeroU16::MIN,
    }
}

fn frame() -> AudioFrame {
    AudioFrame {
        format: pcm(),
        position: SampleClockPosition::new(0).expect("a position"),
        sample_count: 320,
        data: vec![0; 640],
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

fn config() -> SessionConfig {
    SessionConfig {
        voice: "calm".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: pcm(),
        output_format: pcm(),
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: false,
        auto_interrupt: false,
        tools: vec![
            Tool::declare(DeclareTool {
                name: "weather".to_owned(),
                schema: json!({"type": "object"}),
                schema_revision: "1".to_owned(),
                side_effect: SideEffectClass::ReadOnly,
                requires_confirmation: false,
            })
            .0,
        ],
        idle_limit: Duration::from_secs(10),
    }
}

fn open(capabilities: &SpeechCapabilities, config: SessionConfig) -> SessionLedger {
    SessionLedger::open(
        SpeechSessionId::generate(),
        &adapter(),
        capabilities,
        config,
        Instant::now(),
    )
    .expect("the session opens")
    .0
}

fn drain(ledger: &mut SessionLedger) -> Vec<SessionEvent> {
    std::iter::from_fn(|| ledger.next_event()).collect()
}

#[test]
fn a_reused_response_or_call_id_is_refused_with_its_typed_error() {
    let now = Instant::now();
    let mut ledger = open(&capabilities(), config());
    let first = ResponseId::generate();
    ledger.response_created(first, now).expect("created");
    let call = SpeechCallId::new("call_1");
    ledger
        .tool_call(call.clone(), first, "weather", json!({}), now)
        .expect("proposed");
    assert_eq!(
        ledger.tool_call(call.clone(), first, "weather", json!({}), now),
        Err(SessionError::CallIdReused(SpeechCallIdReused {
            call_id: call
        }))
    );
    ledger.response_completed(first, now).expect("completed");
    assert_eq!(
        ledger.response_created(first, now),
        Err(SessionError::ResponseIdReused(ResponseIdReused {
            response_id: first
        }))
    );
}

#[test]
fn a_session_that_fails_mid_response_fails_the_response_first_and_records_nothing_after() {
    let now = Instant::now();
    let mut ledger = open(&capabilities(), config());
    let response = ResponseId::generate();
    ledger.response_created(response, now).expect("created");
    let failure = SessionFailure {
        kind: SessionFailureKind::TransportClosed,
        detail: "the socket closed".to_owned(),
    };
    ledger.fail(failure.clone()).expect("open");
    assert_eq!(
        ledger.response(response).map(|r| r.state()),
        Some(ResponseState::Failed)
    );
    assert!(
        ledger
            .output_transcript(response, "late".to_owned(), now)
            .is_err()
    );
    assert!(ledger.response_completed(response, now).is_err());
    let events = drain(&mut ledger);
    let tail: Vec<&SessionEvent> = events.iter().rev().take(2).collect();
    assert!(
        matches!(
            tail.as_slice(),
            [SessionEvent::SpeechSessionFailed(session), SessionEvent::ResponseFailed(failed)]
                if failed.response_id == response
                    && failed.failure == failure
                    && session.failure == failure
        ),
        "{events:#?}"
    );
    assert_eq!(ledger.session().state(), SpeechSessionState::Failed);
}

#[test]
fn without_client_cancel_the_caller_cannot_cancel_but_the_provider_can() {
    let now = Instant::now();
    let provider_only = SpeechCapabilities {
        manual_interrupt: true,
        client_cancel: false,
        ..capabilities()
    };
    assert_eq!(
        SessionLedger::open(
            SpeechSessionId::generate(),
            &adapter(),
            &provider_only,
            config(),
            now
        )
        .err(),
        Some(OpenError::Unsupported(Unsupported::ClientCancel)),
        "auto_interrupt off leaves barge-in to a caller that cannot cancel"
    );
    let mut interrupting = config();
    interrupting.auto_interrupt = true;
    let mut ledger = open(&provider_only, interrupting);
    let response = ResponseId::generate();
    ledger.response_created(response, now).expect("created");
    assert_eq!(
        ledger.cancel(response),
        Err(SessionError::Unsupported(Unsupported::ClientCancel))
    );
    ledger
        .response_cancelled(response, now)
        .expect("the provider's own interruption");
    assert!(ledger.output_audio(response, frame(), now).is_err());
}

#[test]
fn usage_is_the_running_total_of_its_response_and_the_latest_replaces_the_earlier() {
    let now = Instant::now();
    let mut ledger = open(&capabilities(), config());
    let response = ResponseId::generate();
    ledger.response_created(response, now).expect("created");
    let total = |text_tokens| ResponseUsage {
        input: TokenCounts {
            text_tokens,
            audio_tokens: 0,
            cached_tokens: 0,
        },
        output: TokenCounts::default(),
    };
    assert_eq!(ledger.usage(response), None);
    ledger
        .response_usage(response, total(5), now)
        .expect("recorded");
    ledger
        .response_usage(response, total(12), now)
        .expect("recorded");
    assert_eq!(ledger.usage(response), Some(total(12)));
}

#[test]
fn turn_tuning_is_honoured_only_where_the_model_says_and_only_with_detection_on() {
    let now = Instant::now();
    let sensitivity_only = SpeechCapabilities {
        turn_tuning: TuningSupport {
            sensitivity: true,
            ..TuningSupport::NONE
        },
        ..capabilities()
    };
    let refuse = |capabilities: &SpeechCapabilities, tuning: TurnTuning, mode: TurnDetection| {
        let mut tuned = config();
        tuned.turn_tuning = tuning;
        tuned.turn_detection = mode;
        SessionLedger::open(
            SpeechSessionId::generate(),
            &adapter(),
            capabilities,
            tuned,
            now,
        )
        .err()
    };
    let padded = TurnTuning {
        prefix_padding: Some(Duration::from_millis(300)),
        ..TurnTuning::default()
    };
    assert_eq!(
        refuse(&sensitivity_only, padded, TurnDetection::ServerVad),
        Some(OpenError::Unsupported(Unsupported::TurnTuning(
            TuningOption::PrefixPadding
        )))
    );
    let sensitive = TurnTuning {
        sensitivity: Some(TurnSensitivity::High),
        ..TurnTuning::default()
    };
    assert_eq!(
        refuse(&sensitivity_only, sensitive, TurnDetection::ServerVad),
        None
    );
    assert_eq!(
        refuse(&capabilities(), sensitive, TurnDetection::Disabled),
        Some(OpenError::Unsupported(Unsupported::TurnTuning(
            TuningOption::Sensitivity
        ))),
        "tuning with turn detection off"
    );

    let mut tuned = config();
    tuned.turn_tuning = TurnTuning {
        silence: Some(Duration::from_millis(700)),
        sensitivity: Some(TurnSensitivity::Low),
        prefix_padding: Some(Duration::from_millis(250)),
    };
    let ledger = open(&capabilities(), tuned);
    let record = ledger.session().config();
    assert_eq!(
        (
            record.turn_silence_ms,
            record.turn_sensitivity,
            record.turn_prefix_padding_ms
        ),
        (Some(700), Some(TurnSensitivity::Low), Some(250))
    );
}
