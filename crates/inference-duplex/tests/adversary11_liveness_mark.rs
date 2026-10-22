//! Adversary cases, pass 11, story `duplex-late-usage-counted` at 895e820: the public
//! `SessionLedger::liveness_mark` / `read_in_part` pair after the round-9 fix, which restores only
//! the bound's instant, from the mark or from `heard_before`, and only when the bound stands at
//! the frame's `now`.
//!
//! Two cases pin lines no suite in the tree kills: the session check, and the
//! `now > last_heard` guard on `heard_before` for a frame that hears the provider twice at one
//! instant (a Realtime function-call frame: the binding hears it, then `tool_call` hears it
//! again). Three cases are red: two frames read in part under marks that overlap, in order and
//! out of order, and a frame read in part at the instant the provider was last heard, which
//! starts a bound that waited for the next check. No adapter in this tree overlaps marks or
//! records a frame at the instant of the last one; the cases pin what the API promises.

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

/// A frame whose recording hears the provider twice at its instant, read in part: the bound
/// stands where it stood before the frame, so the session stalls a whole bound after `start`.
#[test]
fn a_frame_read_in_part_that_hears_twice_at_its_instant_leaves_the_bound() {
    let start = Instant::now();
    let mut ledger = awaiting(start);
    let mark = ledger.liveness_mark();
    let frame = start + LIMIT / 2;
    ledger.heard(frame);
    ledger.heard(frame);
    ledger.read_in_part(mark, "a function call with an unreadable field", frame);
    assert!(
        ledger.check_liveness(start + LIMIT),
        "silent but for a frame read in part for the whole {} s bound and not stalled",
        LIMIT.as_secs()
    );
}

/// A mark of another session changes nothing: the bound stays where recording this frame put
/// it, so the session is not stalled a whole bound after `start`.
#[test]
fn a_mark_of_another_session_leaves_the_bound_where_the_frame_put_it() {
    let start = Instant::now();
    let mut ledger = awaiting(start);
    let other = open(start);
    let frame = start + LIMIT * 3 / 4;
    ledger.heard(frame);
    ledger.read_in_part(
        other.liveness_mark(),
        "`response.done` lacks `usage`",
        frame,
    );
    assert!(
        !ledger.check_liveness(start + LIMIT),
        "a mark of another session moved this session's bound back to {} s before",
        (LIMIT * 3 / 4).as_secs()
    );
}

/// Two frames read in part, each under a mark taken before either was recorded, resolved in
/// order: neither is a sign of life, so the session stalls a whole bound after `start`.
#[test]
fn two_frames_read_in_part_under_overlapping_marks_resolved_in_order_leave_the_bound() {
    let start = Instant::now();
    let mut ledger = awaiting(start);
    let first_mark = ledger.liveness_mark();
    let second_mark = ledger.liveness_mark();
    let first = start + LIMIT / 4;
    let second = start + LIMIT / 2;
    ledger.heard(first);
    ledger.heard(second);
    ledger.read_in_part(first_mark, "the first frame", first);
    ledger.read_in_part(second_mark, "the second frame", second);
    assert!(
        ledger.check_liveness(start + LIMIT),
        "silent but for two frames read in part for the whole {} s bound and not stalled: \
         the first frame, read in part, still counts as a sign of life",
        LIMIT.as_secs()
    );
}

/// The same two frames resolved out of order: the session still stalls a whole bound after
/// `start`.
#[test]
fn two_frames_read_in_part_under_overlapping_marks_resolved_out_of_order_leave_the_bound() {
    let start = Instant::now();
    let mut ledger = awaiting(start);
    let first_mark = ledger.liveness_mark();
    let second_mark = ledger.liveness_mark();
    let first = start + LIMIT / 4;
    let second = start + LIMIT / 2;
    ledger.heard(first);
    ledger.heard(second);
    ledger.read_in_part(second_mark, "the second frame", second);
    ledger.read_in_part(first_mark, "the first frame", first);
    assert!(
        ledger.check_liveness(start + LIMIT),
        "silent but for two frames read in part for the whole {} s bound and not stalled: \
         the first frame, read in part, still counts as a sign of life",
        LIMIT.as_secs()
    );
}

/// The bound waits for the next check after an answer; a readable frame heard after the mark
/// starts it, and a frame read in part after that leaves it running from the readable frame:
/// the session stalls a whole bound after the readable frame, and the check does not start the
/// bound afresh.
#[test]
fn a_readable_frame_after_the_mark_starts_a_bound_that_waited_for_the_check() {
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
    ledger
        .answer(&SpeechCallId::new("call-1"), json!({"celsius": 21}))
        .expect("answered");
    let mark = ledger.liveness_mark();
    let readable = start + LIMIT;
    ledger.heard(readable);
    let partial = readable + LIMIT / 2;
    ledger.heard(partial);
    ledger.read_in_part(mark, "`response.done` lacks `usage`", partial);
    assert!(
        ledger.check_liveness(readable + LIMIT),
        "silent for the whole {} s bound after a readable frame and not stalled: the bound \
         waited for the check again",
        LIMIT.as_secs()
    );
}

/// After the caller answers a call of a model that creates every response, the bound waits for
/// the next check. A frame read in part at the very instant the provider was last heard is no
/// sign of life, so the bound still waits: the first check starts it and the session is not
/// stalled at once.
#[test]
fn a_frame_read_in_part_at_the_instant_last_heard_keeps_the_bound_waiting_for_the_check() {
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
    ledger
        .answer(&SpeechCallId::new("call-1"), json!({"celsius": 21}))
        .expect("answered");
    let mark = ledger.liveness_mark();
    ledger.heard(start);
    ledger.read_in_part(mark, "`response.done` lacks `usage`", start);
    let checked = start + LIMIT * 3;
    assert!(
        !ledger.check_liveness(checked),
        "the session stalled at the first check after the answer: a frame read in part at the \
         instant last heard started the bound {} s before the check",
        (LIMIT * 3).as_secs()
    );
}
