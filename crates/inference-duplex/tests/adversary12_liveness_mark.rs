//! Adversary cases, pass 12, story `duplex-late-usage-counted` at 0f35799: frames read only in
//! part (`ProviderFrame::read_in_part`), recorded one frame at a time, against the bound that
//! runs from the provider's last readable frame and the bound that waits for the next check.
//!
//! The pass's red case, a readable frame and a frame read in part heard at one instant under
//! marks that overlap, can no longer be written: a frame borrows its ledger mutably, so
//! overlapping frames do not compile (the `compile_fail` example on `ProviderFrame`). It stands
//! here as the two frames one after the other at that instant. A frame read in part is taken out
//! whole however often it heard the provider; nothing before it is.

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

/// A session awaiting a response in progress since `start`, the provider last heard at `start`.
fn awaiting(start: Instant) -> SessionLedger {
    let mut ledger = open(start);
    ledger
        .response_created(ResponseId::generate(), start)
        .expect("created");
    ledger
}

/// A readable frame, then a frame read in part, both heard at one instant: the readable frame
/// stays, so the session is not stalled a whole bound after `start`.
#[test]
fn a_readable_frame_at_the_instant_of_one_read_in_part_after_it_stays() {
    let start = Instant::now();
    let mut ledger = awaiting(start);
    let instant = start + LIMIT / 2;
    ledger.provider_frame().heard(instant);
    let mut frame = ledger.provider_frame();
    frame.heard(instant);
    frame.read_in_part("`response.done` lacks `usage`", instant);
    assert!(
        !ledger.check_liveness(start + LIMIT),
        "stalled {} s after a readable frame with a {} s bound: the frame read in part at the \
         same instant took the readable frame out with it",
        (LIMIT / 2).as_secs(),
        LIMIT.as_secs()
    );
}

/// Many readable hearings, then a frame read in part that heard the provider as often: the
/// frame is taken out whole, and the bound runs from the last hearing before it, never from the
/// session's opening, so the session is not stalled a whole bound after `start`.
#[test]
fn the_bound_runs_from_the_last_hearing_before_a_frame_read_in_part() {
    let start = Instant::now();
    let mut ledger = awaiting(start);
    let hearings = 64_u32;
    for step in 1..=hearings {
        ledger.heard(start + LIMIT * step / (2 * hearings));
    }
    let mut frame = ledger.provider_frame();
    let instant = start + LIMIT * 3 / 4;
    for _ in 0..hearings {
        frame.heard(instant);
    }
    frame.read_in_part("a frame that heard the provider 64 times", instant);
    assert!(
        !ledger.check_liveness(start + LIMIT),
        "stalled a whole bound after the opening although readable frames were heard {} s \
         after it",
        (LIMIT / 2).as_secs()
    );
}

/// A frame read in part that heard the provider 64 times at its instant is taken out whole:
/// the session stalls a whole bound after `start`.
#[test]
fn a_frame_read_in_part_heard_64_times_is_taken_out_whole() {
    let start = Instant::now();
    let mut ledger = awaiting(start);
    let mut frame = ledger.provider_frame();
    let instant = start + LIMIT / 2;
    for _ in 0..64 {
        frame.heard(instant);
    }
    frame.read_in_part("a frame that heard the provider 64 times", instant);
    assert!(
        ledger.check_liveness(start + LIMIT),
        "silent but for a frame read in part for the whole {} s bound and not stalled",
        LIMIT.as_secs()
    );
}

/// However often a frame read in part heard the provider, it is taken out whole: the session
/// stalls a whole bound after `start`.
#[test]
fn a_frame_read_in_part_is_taken_out_whole_however_often_heard() {
    let start = Instant::now();
    let mut ledger = awaiting(start);
    let mut frame = ledger.provider_frame();
    let instant = start + LIMIT / 2;
    for _ in 0..1_000 {
        frame.heard(instant);
    }
    frame.read_in_part("a frame that heard the provider 1000 times", instant);
    assert!(
        ledger.check_liveness(start + LIMIT),
        "silent but for a frame read in part for the whole {} s bound and not stalled: some of \
         its hearings stayed in the bound",
        LIMIT.as_secs()
    );
}

/// The caller answers a call while a frame read in part is open, and the model creates every
/// response: the bound waits for the next check. Resolving the frame, opened when nothing
/// waited, keeps it waiting, so the first check starts the bound and the session is not stalled
/// at once.
#[test]
fn a_wait_set_while_the_frame_is_open_survives_the_frame_read_in_part() {
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
    let mut frame = ledger.provider_frame();
    let instant = start + LIMIT / 2;
    frame.heard(instant);
    frame
        .answer(&SpeechCallId::new("call-1"), json!({"celsius": 21}))
        .expect("answered");
    frame.read_in_part("`response.done` lacks `usage`", instant);
    let checked = start + LIMIT * 3;
    assert!(
        !ledger.check_liveness(checked),
        "stalled at the first check after the answer: resolving the frame cleared the wait the \
         answer set"
    );
}
