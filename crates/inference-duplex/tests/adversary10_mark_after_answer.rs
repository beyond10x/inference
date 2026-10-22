//! Adversary case, pass 10, story `duplex-late-usage-counted` at b873a7e: `read_in_part`
//! restores the mark's `clock_pending` as well as its `Liveness`. Once the caller answers a call
//! of a model that creates every response, the bound starts at the first check after the answer
//! (`answering_a_call_of_a_model_that_creates_every_response_arms_the_bound`). A frame read only
//! in part that arrives before that first check is no sign of life, so the bound still starts at
//! the first check, not at the provider's last readable frame long before the answer. No case
//! in the tree pins the restore of `clock_pending`: dropping it survives every suite.

use std::num::{NonZeroU16, NonZeroU32};
use std::time::{Duration, Instant};

use inference_duplex::inference_audio::{AudioEncoding, AudioFormat};
use inference_duplex::inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_duplex::inference_tools::{DeclareTool, SideEffectClass, Tool};
use inference_duplex::{
    ResponseId, SessionConfig, SessionLedger, SpeechCallId, SpeechCapabilities, SpeechSessionId,
    TuningSupport, TurnDetection, TurnTuning,
};
use serde_json::json;

const LIMIT: Duration = Duration::from_secs(10);

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
            capabilities: vec![Capability::ConcurrentInputOutput, Capability::ToolCalling],
            max_concurrency: 1,
        },
    )
    .expect("a positive concurrency")
    .0
}

fn weather() -> Tool {
    Tool::declare(DeclareTool {
        name: "weather".to_owned(),
        schema: json!({"type": "object"}),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0
}

/// A model whose turns are the provider's alone: it creates every response itself.
fn server_turns() -> SpeechCapabilities {
    SpeechCapabilities {
        input_formats: vec![pcm()],
        output_formats: vec![pcm()],
        turn_detection: vec![TurnDetection::ServerVad],
        turn_tuning: TuningSupport::ALL,
        manual_commit: false,
        manual_response: false,
        manual_interrupt: false,
        client_cancel: false,
        truncation: false,
        tool_calling: true,
    }
}

fn open(now: Instant) -> SessionLedger {
    let config = SessionConfig {
        voice: "calm".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: pcm(),
        output_format: pcm(),
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools: vec![weather()],
        idle_limit: LIMIT,
    };
    SessionLedger::open(
        SpeechSessionId::generate(),
        &adapter(),
        &server_turns(),
        config,
        now,
    )
    .expect("the session opens")
    .0
}

/// The caller answers long after the provider's last frame; a frame read only in part arrives
/// before the first check after the answer. The bound starts at that check, as it would had the
/// frame not arrived, so the session is not stalled at once.
#[test]
fn a_frame_read_in_part_before_the_first_check_after_an_answer_does_not_start_the_bound_early() {
    let start = Instant::now();
    let mut ledger = open(start);
    let response = ResponseId::generate();
    ledger.response_created(response, start).expect("created");
    ledger
        .tool_call(
            SpeechCallId::new("call-1"),
            response,
            "weather",
            json!({}),
            start,
        )
        .expect("proposed");
    ledger
        .response_completed(response, start)
        .expect("completed");
    let answered = start + LIMIT * 3;
    ledger
        .answer(&SpeechCallId::new("call-1"), json!({"celsius": 21}))
        .expect("answered");
    let mark = ledger.liveness_mark();
    ledger.heard(answered);
    ledger.read_in_part(
        mark,
        "`response.done` lacks the object field `usage`",
        answered,
    );
    assert!(
        !ledger.check_liveness(answered),
        "the session stalled at the first check after the answer: a frame read in part moved \
         the bound back to the provider's last readable frame, {} s before",
        (LIMIT * 3).as_secs()
    );
    assert!(!ledger.check_liveness(answered + LIMIT - Duration::from_millis(1)));
    assert!(
        ledger.check_liveness(answered + LIMIT),
        "the provider owes the response that answers the result"
    );
}
