//! Adversary case, pass 10, story `duplex-late-usage-counted` at b873a7e: a frame read only in
//! part (`ProviderFrame::read_in_part`) after a readable frame does not rewind the bound past
//! that readable frame. Frames are recorded one at a time through a frame scope that borrows the
//! ledger, so a frame cannot carry state of an earlier frame or of another session; the pass's
//! case of a mark taken from another session can no longer be written and was deleted.

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

/// A readable frame, then a later frame read in part: the readable frame was a sign of life, so
/// the bound runs from it, and the frame read in part keeps it there.
#[test]
fn a_frame_read_in_part_after_a_readable_frame_does_not_undo_it() {
    let start = Instant::now();
    let mut ledger = open(start, BOUND);
    ledger.request_response(start).expect("none in progress");
    let response = ResponseId::generate();
    ledger.response_created(response, start).expect("created");
    let readable = start + BOUND * 3 / 4;
    let mut frame = ledger.provider_frame();
    frame
        .speech_started(SampleClockPosition::new(0).expect("a position"), readable)
        .expect("open");
    drop(frame);
    ledger.provider_frame().read_in_part(
        "`response.done` lacks the object field `usage`",
        start + BOUND * 7 / 8,
    );
    assert_eq!(ledger.liveness().limit(), BOUND, "the idle limit changed");
    assert!(
        !ledger.check_liveness(start + BOUND),
        "the session stalled {} ms after a readable frame, inside its {} ms bound",
        (BOUND / 4).as_millis(),
        BOUND.as_millis()
    );
}
