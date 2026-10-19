//! Adversary pass 1: identities reused, the liveness bound around turns the provider ends, a
//! session closed mid-response, and the boundaries of truncation and the bound.

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
    ResponseId, ResponseState, SessionConfig, SessionError, SessionEvent, SessionFailure,
    SessionFailureKind, SessionLedger, SpeechCallId, SpeechCapabilities, SpeechSessionId,
    SpeechSessionState, TuningSupport, TurnDetection, TurnTuning,
};
use serde_json::json;

const LIMIT: Duration = Duration::from_secs(10);

fn pcm(sample_rate: u32) -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(sample_rate).expect("a positive rate"),
        channels: NonZeroU16::MIN,
    }
}

/// A 20 ms frame of 16 kHz PCM, 320 samples, at sample `from`.
fn frame(from: u64) -> AudioFrame {
    let format = pcm(16_000);
    AudioFrame {
        format,
        position: SampleClockPosition::new(from).expect("a position"),
        sample_count: 320,
        data: vec![0; usize::try_from(format.data_len(320)).expect("a length")],
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

fn full() -> SpeechCapabilities {
    SpeechCapabilities {
        input_formats: vec![pcm(16_000)],
        output_formats: vec![pcm(16_000)],
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

fn config(turn_detection: TurnDetection, auto: bool) -> SessionConfig {
    SessionConfig {
        voice: "calm".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: pcm(16_000),
        output_format: pcm(16_000),
        turn_detection,
        turn_tuning: TurnTuning::default(),
        auto_response: auto,
        auto_interrupt: auto,
        tools: vec![weather()],
        idle_limit: LIMIT,
    }
}

fn open(config: SessionConfig, now: Instant) -> SessionLedger {
    SessionLedger::open(
        SpeechSessionId::generate(),
        &adapter(),
        &full(),
        config,
        now,
    )
    .expect("the session opens")
    .0
}

fn drain(ledger: &mut SessionLedger) -> Vec<SessionEvent> {
    std::iter::from_fn(|| ledger.next_event()).collect()
}

fn at(sample: u64) -> SampleClockPosition {
    SampleClockPosition::new(sample).expect("a position")
}

// Identities.

#[test]
fn a_response_id_already_used_is_not_created_again() {
    let now = Instant::now();
    let mut ledger = open(config(TurnDetection::Disabled, false), now);
    let reused = ResponseId::generate();
    ledger.response_created(reused, now).expect("created");
    ledger.response_completed(reused, now).expect("completed");

    let again = ledger.response_created(reused, now);
    assert!(
        again.is_err(),
        "ResponseCreated says `a new id`; a second response under {reused} was recorded"
    );
    assert_eq!(ledger.responses().len(), 1, "{:#?}", ledger.responses());
}

#[test]
fn a_reused_response_id_leaves_a_response_in_progress_that_takes_no_output() {
    let now = Instant::now();
    let mut ledger = open(config(TurnDetection::Disabled, false), now);
    let reused = ResponseId::generate();
    ledger.response_created(reused, now).expect("created");
    ledger.response_completed(reused, now).expect("completed");
    if ledger.response_created(reused, now).is_err() {
        return;
    }
    assert_eq!(ledger.in_progress(), Some(reused));
    assert_eq!(
        ledger.output_audio(reused, frame(0), now),
        Ok(()),
        "the ledger reports {reused} in progress and refuses its audio"
    );
}

#[test]
fn a_call_id_already_proposed_is_not_proposed_again() {
    let now = Instant::now();
    let mut ledger = open(config(TurnDetection::Disabled, false), now);
    let call = SpeechCallId::new("call_1");
    let first = ResponseId::generate();
    ledger.response_created(first, now).expect("created");
    ledger
        .tool_call(
            call.clone(),
            first,
            "weather",
            json!({"city": "Berlin"}),
            now,
        )
        .expect("proposed");
    ledger.response_completed(first, now).expect("completed");
    let second = ResponseId::generate();
    ledger.response_created(second, now).expect("created");

    let again = ledger.tool_call(
        call.clone(),
        second,
        "weather",
        json!({"city": "Paris"}),
        now,
    );
    assert!(
        again.is_err(),
        "a second call under `{call}` was proposed; answer() can reach only the first"
    );
    assert_eq!(ledger.calls().len(), 1, "{:#?}", ledger.calls());
}

// The liveness bound.

#[test]
fn the_bound_runs_once_the_providers_turn_detection_ends_a_turn_with_auto_response_on() {
    let start = Instant::now();
    let mut ledger = open(config(TurnDetection::ServerVad, true), start);
    ledger.speech_stopped(at(16_000), start).expect("observed");
    ledger.input_committed(start).expect("committed");
    assert!(
        ledger.check_liveness(start + LIMIT * 3),
        "auto_response is on and the provider ended the turn: the caller awaits its response, \
         and a provider silent for three bounds leaves the session {:?}",
        ledger.session().state()
    );
}

#[test]
fn a_request_after_a_long_idle_starts_the_bound_from_the_request() {
    let start = Instant::now();
    let mut ledger = open(config(TurnDetection::Disabled, false), start);
    let asked = start + LIMIT * 5;
    ledger.request_response(asked).expect("none in progress");
    assert!(
        !ledger.check_liveness(asked + Duration::from_millis(1)),
        "a request made a millisecond ago is not a stall"
    );
    assert!(ledger.check_liveness(asked + LIMIT));
}

#[test]
fn an_earlier_instant_heard_late_does_not_move_the_bound_back() {
    let start = Instant::now();
    let mut ledger = open(config(TurnDetection::Disabled, false), start);
    ledger.request_response(start).expect("none in progress");
    ledger.heard(start + Duration::from_secs(5));
    ledger.heard(start + Duration::from_secs(1));
    assert!(!ledger.check_liveness(start + Duration::from_secs(1) + LIMIT));
    assert!(
        !ledger.check_liveness(start.checked_sub(Duration::from_secs(1)).unwrap_or(start)),
        "time passed in going backwards is not silence"
    );
    assert!(ledger.check_liveness(start + Duration::from_secs(5) + LIMIT));
}

#[test]
fn a_session_whose_requested_response_ended_is_idle_again() {
    let start = Instant::now();
    let mut ledger = open(config(TurnDetection::Disabled, false), start);
    ledger.request_response(start).expect("none in progress");
    let response = ResponseId::generate();
    ledger.response_created(response, start).expect("created");
    ledger
        .response_completed(response, start)
        .expect("completed");
    assert!(
        !ledger.check_liveness(start + LIMIT * 5),
        "nothing is awaited after the response ended"
    );
    assert_eq!(ledger.session().state(), SpeechSessionState::Open);
}

// Close and failure mid-response.

#[test]
fn a_session_closed_mid_response_takes_no_more_output_of_it() {
    let now = Instant::now();
    let mut ledger = open(config(TurnDetection::Disabled, false), now);
    let response = ResponseId::generate();
    ledger.response_created(response, now).expect("created");
    ledger.close().expect("open");
    let late = ledger.output_audio(response, frame(0), now);
    let events = drain(&mut ledger);
    let closed = events
        .iter()
        .position(|event| matches!(event, SessionEvent::SpeechSessionClosed(_)))
        .expect("closed");
    assert!(
        late.is_err() && closed == events.len() - 1,
        "output of a closed session was recorded after SpeechSessionClosed ({late:?}): {events:#?}"
    );
}

// Truncation boundaries and output after a response ended.

#[test]
fn truncation_at_zero_and_at_exactly_the_emitted_sample_is_within_the_output() {
    let now = Instant::now();
    let mut ledger = open(config(TurnDetection::Disabled, false), now);
    let silent = ResponseId::generate();
    ledger.response_created(silent, now).expect("created");
    ledger
        .truncate(silent, at(0))
        .expect("nothing emitted, nothing heard");
    assert_eq!(
        ledger.truncate(silent, at(1)),
        Err(SessionError::BeyondOutput {
            heard: at(1),
            emitted: 0
        })
    );
    ledger.response_completed(silent, now).expect("completed");

    let spoken = ResponseId::generate();
    ledger.response_created(spoken, now).expect("created");
    ledger.output_audio(spoken, frame(0), now).expect("emitted");
    ledger
        .output_audio(spoken, frame(320), now)
        .expect("emitted");
    ledger
        .truncate(spoken, at(500))
        .expect("within the second frame");
    ledger
        .truncate(spoken, at(640))
        .expect("the listener heard all of it");
    assert_eq!(
        ledger.truncate(spoken, at(641)),
        Err(SessionError::BeyondOutput {
            heard: at(641),
            emitted: 640
        })
    );
}

#[test]
fn a_cancelled_responses_late_transcript_is_refused() {
    let now = Instant::now();
    let mut ledger = open(config(TurnDetection::Disabled, false), now);
    let response = ResponseId::generate();
    ledger.response_created(response, now).expect("created");
    ledger.cancel(response).expect("in progress");
    assert!(matches!(
        ledger.output_transcript(response, "late".to_owned(), now),
        Err(SessionError::ResponseNotInProgress(conflict))
            if conflict.current == ResponseState::Cancelled
    ));
    assert!(
        !drain(&mut ledger)
            .iter()
            .any(|event| matches!(event, SessionEvent::OutputTranscriptDelta(_)))
    );
}

#[test]
fn a_failed_response_is_recorded_as_failed_and_takes_no_more_output() {
    let now = Instant::now();
    let mut ledger = open(config(TurnDetection::Disabled, false), now);
    let response = ResponseId::generate();
    ledger.response_created(response, now).expect("created");
    let failure = SessionFailure {
        kind: SessionFailureKind::ProviderError,
        detail: "server_error".to_owned(),
    };
    ledger
        .response_failed(response, failure.clone(), now)
        .expect("in progress");
    assert_eq!(
        ledger.response(response).map(|r| r.state()),
        Some(ResponseState::Failed)
    );
    assert!(ledger.output_audio(response, frame(0), now).is_err());
    let events = drain(&mut ledger);
    assert!(
        matches!(
            events.last(),
            Some(SessionEvent::ResponseFailed(failed))
                if failed.response_id == response && failed.failure == failure
        ),
        "{events:#?}"
    );
    assert_eq!(ledger.session().state(), SpeechSessionState::Open);
}
