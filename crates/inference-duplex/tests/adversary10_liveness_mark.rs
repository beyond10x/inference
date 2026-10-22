//! Adversary cases, pass 10, story `duplex-late-usage-counted` at b873a7e: the public
//! `SessionLedger::liveness_mark` / `read_in_part` pair. `read_in_part` assigns the mark's whole
//! `Liveness` (the instant last heard and the limit) back onto the ledger. Nothing ties a mark to
//! the ledger that took it, or to the frame being recorded, so a mark taken before a readable
//! frame rewinds the bound past that frame, and a mark of another session replaces this
//! session's idle limit and can move its bound forward. No adapter in this tree does either; the
//! cases pin what the API lets a binding do.

use std::num::{NonZeroU16, NonZeroU32};
use std::time::{Duration, Instant};

use inference_duplex::inference_audio::{AudioEncoding, AudioFormat, SampleClockPosition};
use inference_duplex::inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_duplex::{
    ResponseId, SessionConfig, SessionLedger, SpeechCapabilities, SpeechSessionId, TuningSupport,
    TurnDetection, TurnTuning,
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

fn open(now: Instant, idle_limit: Duration) -> SessionLedger {
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
        idle_limit,
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

/// A mark taken before a readable frame, handed to `read_in_part` for a later frame read in
/// part: the readable frame was a sign of life, so the bound runs from it, not from the mark.
#[test]
fn a_mark_taken_before_a_readable_frame_does_not_undo_it() {
    let start = Instant::now();
    let mut ledger = open(start, BOUND);
    ledger.request_response(start).expect("none in progress");
    let response = ResponseId::generate();
    ledger.response_created(response, start).expect("created");
    let stale = ledger.liveness_mark();
    let readable = start + BOUND * 3 / 4;
    ledger
        .speech_started(SampleClockPosition::new(0).expect("a position"), readable)
        .expect("open");
    ledger.read_in_part(
        stale,
        "`response.done` lacks the object field `usage`",
        start + BOUND * 7 / 8,
    );
    assert!(
        !ledger.check_liveness(start + BOUND),
        "the session stalled {} ms after a readable frame, inside its {} ms bound",
        (BOUND / 4).as_millis(),
        BOUND.as_millis()
    );
}

/// A mark of another session, with another idle limit and heard later: this session keeps its
/// own limit, and its bound does not move past what this session heard.
#[test]
fn a_mark_of_another_session_changes_neither_the_limit_nor_the_bound() {
    let start = Instant::now();
    let mut ledger = open(start, BOUND);
    ledger.request_response(start).expect("none in progress");
    let other = open(start + BOUND / 2, Duration::from_secs(10));
    ledger.read_in_part(
        other.liveness_mark(),
        "`response.done` lacks the object field `usage`",
        start + BOUND / 4,
    );
    assert_eq!(ledger.liveness().limit(), BOUND, "the idle limit changed");
    assert!(
        ledger.check_liveness(start + BOUND),
        "silent for the whole {} ms bound and not stalled",
        BOUND.as_millis()
    );
}
