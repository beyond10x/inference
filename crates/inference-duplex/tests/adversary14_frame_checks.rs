//! Adversary cases, pass 14, story `duplex-late-usage-counted` at 8dca44a: a liveness check made
//! while a `ProviderFrame` is open. The bound a frame read in part leaves must stand where it
//! would without the frame's hearings, with several checks inside one frame, with a check inside
//! or after a nested frame, and with a frame resolved as readable.

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
const PARTIAL: &str = "`response.done` lacks `usage`";

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

/// A model that creates every response itself (`manual_response` false).
fn capabilities() -> SpeechCapabilities {
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

/// A session whose caller has just answered a call at `start`: the bound waits for the next
/// check.
fn answered(start: Instant) -> SessionLedger {
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
    let mut ledger = SessionLedger::open(
        SpeechSessionId::generate(),
        &adapter(),
        &capabilities(),
        config,
        start,
    )
    .expect("the session opens")
    .0;
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
    ledger
        .answer(&SpeechCallId::new("call-1"), json!({"celsius": 21}))
        .expect("answered");
    ledger
}

/// The bound must run from `from`: not stalled just before a whole bound after it, stalled at
/// a whole bound after it.
fn assert_bound_from(ledger: &mut SessionLedger, from: Instant, what: &str) {
    assert!(
        !ledger.check_liveness(from + LIMIT * 9 / 10),
        "{what}: stalled before a whole bound after the instant the bound should run from"
    );
    assert!(
        ledger.check_liveness(from + LIMIT),
        "{what}: silent for a whole {} s bound after the instant the bound should run from, \
         and not stalled",
        LIMIT.as_secs()
    );
}

/// Two checks inside one frame read in part, each after a hearing of the frame. Without the
/// frame's hearings the first check starts the waiting bound and the second only finds it
/// running, so the bound runs from the first check, not the second.
#[test]
fn several_checks_inside_one_frame_read_in_part_start_the_bound_at_the_first() {
    let start = Instant::now();
    let mut ledger = answered(start);
    let first = start + LIMIT * 3 / 2;
    {
        let mut frame = ledger.provider_frame();
        frame.heard(start + LIMIT);
        assert!(!frame.check_liveness(first), "stalled at the first check");
        frame.heard(first + LIMIT / 4);
        let second = first + LIMIT * 3 / 4;
        assert!(!frame.check_liveness(second), "stalled at the second check");
        frame.read_in_part(PARTIAL, second);
    }
    assert_bound_from(
        &mut ledger,
        first,
        "two checks inside one frame read in part",
    );
}

/// A check inside an inner frame, which the inner frame's hearing kept from starting a waiting
/// bound: the outer frame read in part takes the inner frame's hearing out, and the bound runs
/// from that check.
#[test]
fn a_check_inside_an_inner_frame_starts_the_outer_frames_waiting_bound() {
    let start = Instant::now();
    let mut ledger = answered(start);
    let checked = start + LIMIT * 3 / 2;
    {
        let mut outer = ledger.provider_frame();
        {
            let mut inner = outer.provider_frame();
            inner.heard(start + LIMIT);
            assert!(
                !inner.check_liveness(checked),
                "stalled inside the inner frame"
            );
        }
        outer.read_in_part(PARTIAL, checked);
    }
    assert_bound_from(
        &mut ledger,
        checked,
        "a check inside an inner, readable frame",
    );
}

/// A check after a nested, readable frame closed, inside the outer frame: the inner frame's
/// hearing is the outer frame's, so the outer frame read in part leaves the bound running from
/// that check.
#[test]
fn a_check_after_a_nested_frame_starts_the_outer_frames_waiting_bound() {
    let start = Instant::now();
    let mut ledger = answered(start);
    let checked = start + LIMIT * 3 / 2;
    {
        let mut outer = ledger.provider_frame();
        {
            let mut inner = outer.provider_frame();
            inner.heard(start + LIMIT);
        }
        assert!(
            !outer.check_liveness(checked),
            "stalled after the inner frame"
        );
        outer.read_in_part(PARTIAL, checked);
    }
    assert_bound_from(
        &mut ledger,
        checked,
        "a check after a nested, readable frame",
    );
}

/// A frame that hears the provider and then sees a check is dropped, so readable: its hearing
/// stands, and the bound runs from it, not from the check.
#[test]
fn a_check_inside_a_readable_frame_leaves_the_frames_hearing() {
    let start = Instant::now();
    let mut ledger = answered(start);
    let heard = start + LIMIT;
    {
        let mut frame = ledger.provider_frame();
        frame.heard(heard);
        assert!(
            !frame.check_liveness(heard + LIMIT / 2),
            "stalled inside the frame"
        );
    }
    assert_bound_from(&mut ledger, heard, "a check inside a readable frame");
}

/// The outer frame's hearing ends the wait before the inner frame opens, so a check inside the
/// inner frame starts nothing for it: the inner frame read in part leaves the bound running
/// from the outer frame's hearing, not from the check.
#[test]
fn a_check_inside_an_inner_frame_read_in_part_leaves_the_outer_frames_hearing() {
    let start = Instant::now();
    let mut ledger = answered(start);
    let readable = start + LIMIT;
    {
        let mut outer = ledger.provider_frame();
        outer.heard(readable);
        let mut inner = outer.provider_frame();
        inner.heard(readable + LIMIT / 4);
        let checked = readable + LIMIT / 2;
        assert!(
            !inner.check_liveness(checked),
            "stalled inside the inner frame"
        );
        inner.read_in_part(PARTIAL, checked);
    }
    assert_bound_from(
        &mut ledger,
        readable,
        "a check inside an inner frame read in part, the outer readable",
    );
}
