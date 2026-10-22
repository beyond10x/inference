//! Adversary cases, pass 13, story `duplex-late-usage-counted` at b6e0fe6: `ProviderFrame`, the
//! scope through which a binding records one provider frame and resolves it as readable or read
//! in part.
//!
//! A check made inside a frame, after the frame's own hearing, starts for the frame read in part
//! a bound that waited for it. The pass's case of a ledger swapped under an open frame
//! (`mem::swap` through `DerefMut`) can no longer be written: a frame hands out no
//! `&mut SessionLedger`, and the `compile_fail` example on `ProviderFrame` pins that. The other
//! cases pin what the scope promises and what no suite in the tree kills: a request or a check
//! inside a frame stays in the bound, nested frames, and a frame leaked or unwound.

use std::num::{NonZeroU16, NonZeroU32};
use std::panic::{AssertUnwindSafe, catch_unwind};
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

/// A model whose turns are the provider's (`manual_response` false) or the caller's.
fn capabilities(caller_turns: bool) -> SpeechCapabilities {
    SpeechCapabilities {
        input_formats: vec![pcm()],
        output_formats: vec![pcm()],
        turn_detection: vec![TurnDetection::ServerVad],
        turn_tuning: TuningSupport::ALL,
        manual_commit: caller_turns,
        manual_response: caller_turns,
        manual_interrupt: false,
        client_cancel: false,
        truncation: false,
        tool_calling: true,
    }
}

fn open(now: Instant, caller_turns: bool) -> SessionLedger {
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
        &capabilities(caller_turns),
        config,
        now,
    )
    .expect("the session opens")
    .0
}

/// A session awaiting a response in progress since `start`, the provider last heard at `start`.
fn awaiting(start: Instant) -> SessionLedger {
    let mut ledger = open(start, false);
    ledger
        .response_created(ResponseId::generate(), start)
        .expect("created");
    ledger
}

/// A session of a model that creates every response, whose caller has just answered a call at
/// `start`: the bound waits for the next check.
fn answered(start: Instant) -> SessionLedger {
    let mut ledger = open(start, false);
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

/// The bound waits for the next check after an answer. A frame hears the provider, a check is
/// made inside the frame, and the frame is read in part. Without the frame's hearing that check
/// would have started the bound, so it runs from the check: the session stalls a whole bound
/// after it, not at a later check.
#[test]
fn a_check_inside_a_frame_read_in_part_starts_the_bound_that_waited_for_it() {
    let start = Instant::now();
    let mut ledger = answered(start);
    let heard = start + LIMIT;
    let checked = heard + LIMIT / 2;
    {
        let mut frame = ledger.provider_frame();
        frame.heard(heard);
        assert!(!frame.check_liveness(checked), "stalled inside the frame");
        frame.read_in_part("`response.done` lacks `usage`", checked);
    }
    assert!(
        !ledger.check_liveness(checked + LIMIT / 2),
        "stalled half a bound after the check"
    );
    assert!(
        ledger.check_liveness(checked + LIMIT),
        "silent but for a frame read in part for a whole {} s bound after the check that would \
         have started it, and not stalled: the frame's hearing kept the check from starting the \
         bound",
        LIMIT.as_secs()
    );
}

/// A caller's request inside a frame read in part moves the bound, and that stays: the frame's
/// own hearing before it is taken out, the request is not.
#[test]
fn a_request_inside_a_frame_read_in_part_stays_in_the_bound() {
    let start = Instant::now();
    let mut ledger = open(start, true);
    let requested = start + LIMIT * 3 / 4;
    {
        let mut frame = ledger.provider_frame();
        frame.heard(start + LIMIT / 4);
        frame.request_response(requested).expect("none in progress");
        frame.read_in_part("`response.done` lacks `usage`", requested);
    }
    assert!(
        !ledger.check_liveness(start + LIMIT * 3 / 2),
        "stalled {} s after the request with a {} s bound: the frame read in part took the \
         request out with its own hearing",
        (LIMIT * 3 / 4).as_secs(),
        LIMIT.as_secs()
    );
    assert!(
        ledger.check_liveness(requested + LIMIT),
        "silent for a whole bound after the request and not stalled"
    );
}

/// A check inside a frame that starts a bound that waited for it stays, and so does the end of
/// the wait: the frame's hearing after the check is taken out, and the session stalls a whole
/// bound after the check, not at a later check.
#[test]
fn a_check_inside_a_frame_that_starts_a_waiting_bound_stays() {
    let start = Instant::now();
    let mut ledger = answered(start);
    let checked = start + LIMIT * 2;
    {
        let mut frame = ledger.provider_frame();
        assert!(!frame.check_liveness(checked), "stalled at the first check");
        let partial = checked + LIMIT / 2;
        frame.heard(partial);
        frame.read_in_part("`response.done` lacks `usage`", partial);
    }
    assert!(
        !ledger.check_liveness(checked + LIMIT * 3 / 4),
        "stalled before a whole bound after the check that started it"
    );
    assert!(
        ledger.check_liveness(checked + LIMIT),
        "silent but for a frame read in part for a whole {} s bound after the check that \
         started it, and not stalled",
        LIMIT.as_secs()
    );
}

/// A frame opened through another is part of it: the outer frame read in part takes the inner
/// frame's hearing out with its own, though the inner frame was readable.
#[test]
fn an_outer_frame_read_in_part_takes_a_readable_inner_frame_out() {
    let start = Instant::now();
    let mut ledger = awaiting(start);
    {
        let mut outer = ledger.provider_frame();
        outer.heard(start + LIMIT / 4);
        {
            let mut inner = outer.provider_frame();
            inner.heard(start + LIMIT / 2);
        }
        outer.read_in_part("`response.done` lacks `usage`", start + LIMIT / 2);
    }
    assert!(
        ledger.check_liveness(start + LIMIT),
        "silent but for an outer frame read in part for the whole bound and not stalled"
    );
}

/// An inner frame read in part inside a readable outer frame takes out its own hearing only:
/// the outer frame's hearing before it stays.
#[test]
fn an_inner_frame_read_in_part_leaves_the_outer_frames_hearing() {
    let start = Instant::now();
    let mut ledger = awaiting(start);
    let readable = start + LIMIT / 2;
    {
        let mut outer = ledger.provider_frame();
        outer.heard(readable);
        let mut inner = outer.provider_frame();
        inner.heard(start + LIMIT * 3 / 4);
        inner.read_in_part("`response.done` lacks `usage`", start + LIMIT * 3 / 4);
    }
    assert!(
        !ledger.check_liveness(start + LIMIT),
        "stalled although the outer, readable frame was heard {} s before",
        (LIMIT / 2).as_secs()
    );
    assert!(
        ledger.check_liveness(readable + LIMIT),
        "silent for a whole bound after the readable frame and not stalled: the inner frame \
         read in part stayed in the bound"
    );
}

/// A frame leaked with `mem::forget` stays readable, and a later frame read in part still takes
/// out its own hearing only.
#[test]
fn a_frame_after_a_leaked_frame_takes_out_its_own_hearing() {
    let start = Instant::now();
    let mut ledger = awaiting(start);
    let readable = start + LIMIT / 2;
    let mut leaked = ledger.provider_frame();
    leaked.heard(readable);
    std::mem::forget(leaked);
    {
        let mut frame = ledger.provider_frame();
        frame.heard(start + LIMIT * 3 / 4);
        frame.read_in_part("`response.done` lacks `usage`", start + LIMIT * 3 / 4);
    }
    assert!(
        !ledger.check_liveness(start + LIMIT),
        "stalled although a readable frame was heard {} s before",
        (LIMIT / 2).as_secs()
    );
    assert!(
        ledger.check_liveness(readable + LIMIT),
        "silent but for a frame read in part for a whole bound after the leaked frame"
    );
}

/// A frame unwound by a panic is dropped, so readable; a later frame read in part takes out its
/// own hearing only.
#[test]
fn a_frame_after_a_frame_unwound_by_a_panic_takes_out_its_own_hearing() {
    let start = Instant::now();
    let mut ledger = awaiting(start);
    let readable = start + LIMIT / 2;
    let unwound = catch_unwind(AssertUnwindSafe(|| {
        let mut frame = ledger.provider_frame();
        frame.heard(readable);
        panic!("the binding panicked while recording a frame");
    }));
    assert!(unwound.is_err(), "the closure did not panic");
    {
        let mut frame = ledger.provider_frame();
        frame.heard(start + LIMIT * 3 / 4);
        frame.read_in_part("`response.done` lacks `usage`", start + LIMIT * 3 / 4);
    }
    assert!(
        !ledger.check_liveness(start + LIMIT),
        "stalled although a readable frame was heard {} s before",
        (LIMIT / 2).as_secs()
    );
    assert!(
        ledger.check_liveness(readable + LIMIT),
        "silent but for a frame read in part for a whole bound after the unwound frame"
    );
}
