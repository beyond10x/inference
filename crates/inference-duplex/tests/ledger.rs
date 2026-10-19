//! The session ledger: what every implementation of the contract refuses, and the events it
//! records, in order.

use std::num::{NonZeroU16, NonZeroU32};
use std::time::{Duration, Instant};

use inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_duplex::inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality,
};
use inference_duplex::inference_core::model::{AdapterId, ModelId};
use inference_duplex::inference_tools::{DeclareTool, SideEffectClass, Tool};
use inference_duplex::{
    OpenError, ResponseId, ResponseState, ResponseStateConflict, ResponseUsage, SessionConfig,
    SessionError, SessionEvent, SessionFailure, SessionFailureKind, SessionLedger, SpeechCallId,
    SpeechCapabilities, SpeechSessionId, SpeechSessionState, SpeechSessionStateConflict,
    SpeechToolCallState, SpeechToolCallStateConflict, TokenCounts, TranscriptionUsage,
    TuningSupport, TurnDetection, TurnTuning, Unsupported,
};
use serde_json::json;

fn pcm(sample_rate: u32) -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(sample_rate).expect("a positive rate"),
        channels: NonZeroU16::MIN,
    }
}

/// `count` 20 ms frames of `format`, every byte `byte`, the first at sample `from`.
fn frame(format: AudioFormat, from: u64, byte: u8) -> AudioFrame {
    let samples = format.sample_rate.get() / 50;
    AudioFrame {
        format,
        position: SampleClockPosition::new(from).expect("a position"),
        sample_count: samples,
        data: vec![byte; usize::try_from(format.data_len(samples)).expect("a length")],
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

/// Everything a telephony caller asks for.
fn full() -> SpeechCapabilities {
    SpeechCapabilities {
        input_formats: vec![pcm(16_000), pcm(24_000)],
        output_formats: vec![pcm(16_000), pcm(24_000)],
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

/// A model whose turns are the provider's alone: server detection, automatic responses and
/// interruption, no truncation.
fn server_turns() -> SpeechCapabilities {
    SpeechCapabilities {
        input_formats: vec![pcm(16_000)],
        output_formats: vec![pcm(16_000), pcm(24_000)],
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

fn config(turn_detection: TurnDetection) -> SessionConfig {
    SessionConfig {
        voice: "calm".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: pcm(16_000),
        output_format: pcm(16_000),
        turn_detection,
        turn_tuning: TurnTuning::default(),
        auto_response: false,
        auto_interrupt: false,
        tools: vec![weather()],
        idle_limit: Duration::from_secs(10),
    }
}

fn open(capabilities: &SpeechCapabilities, config: SessionConfig, now: Instant) -> SessionLedger {
    SessionLedger::open(
        SpeechSessionId::generate(),
        &adapter(),
        capabilities,
        config,
        now,
    )
    .expect("the session opens")
    .0
}

fn drain(ledger: &mut SessionLedger) -> Vec<SessionEvent> {
    std::iter::from_fn(|| ledger.next_event()).collect()
}

#[test]
fn a_configuration_outside_the_capabilities_is_refused_with_the_option_named() {
    let now = Instant::now();
    let refuse = |capabilities: &SpeechCapabilities, config: SessionConfig| {
        SessionLedger::open(
            SpeechSessionId::generate(),
            &adapter(),
            capabilities,
            config,
            now,
        )
        .err()
    };

    let mut wide = config(TurnDetection::ServerVad);
    wide.input_format = pcm(24_000);
    assert_eq!(
        refuse(&server_turns(), wide),
        Some(OpenError::Unsupported(Unsupported::InputFormat(pcm(
            24_000
        ))))
    );
    let mut wide_out = config(TurnDetection::ServerVad);
    wide_out.output_format = pcm(8_000);
    assert_eq!(
        refuse(&server_turns(), wide_out),
        Some(OpenError::Unsupported(Unsupported::OutputFormat(pcm(
            8_000
        ))))
    );
    assert_eq!(
        refuse(&server_turns(), config(TurnDetection::Disabled)),
        Some(OpenError::Unsupported(Unsupported::TurnDetection(
            TurnDetection::Disabled
        )))
    );
    assert_eq!(
        refuse(&server_turns(), config(TurnDetection::ServerVad)),
        Some(OpenError::Unsupported(Unsupported::ManualResponse)),
        "auto_response off needs manual responses"
    );
    let mut caller_barge_in = config(TurnDetection::ServerVad);
    caller_barge_in.auto_response = true;
    assert_eq!(
        refuse(&server_turns(), caller_barge_in.clone()),
        Some(OpenError::Unsupported(Unsupported::ManualInterrupt)),
        "auto_interrupt off needs the provider to leave a response running"
    );
    caller_barge_in.auto_interrupt = true;
    let no_tools = SpeechCapabilities {
        tool_calling: false,
        ..server_turns()
    };
    assert_eq!(
        refuse(&no_tools, caller_barge_in.clone()),
        Some(OpenError::Unsupported(Unsupported::ToolCalling))
    );
    assert!(refuse(&server_turns(), caller_barge_in).is_none());

    let mut unbounded = config(TurnDetection::Disabled);
    unbounded.idle_limit = Duration::from_micros(10);
    assert!(matches!(
        refuse(&full(), unbounded),
        Some(OpenError::NoLivenessBound(error)) if error.submitted == 0
    ));
}

#[test]
fn input_passes_as_it_is_in_the_session_format_and_any_other_format_is_refused() {
    let now = Instant::now();
    let mut ledger = open(&full(), config(TurnDetection::Disabled), now);
    let sent = frame(pcm(16_000), 0, 7);
    ledger
        .accept_input(sent.clone())
        .expect("the session's own format");
    assert_eq!(
        ledger.accept_input(frame(pcm(24_000), 320, 7)),
        Err(SessionError::FormatMismatch {
            expected: pcm(16_000),
            found: pcm(24_000),
        })
    );
    ledger.commit_input(now).expect("a manual commit");
    let events = drain(&mut ledger);
    assert_eq!(events.len(), 2, "{events:#?}");
    assert!(
        matches!(&events[0], SessionEvent::InputAccepted(accepted) if accepted.frame == sent),
        "the frame is accepted byte for byte: {events:#?}"
    );
    assert!(matches!(events[1], SessionEvent::InputCommitted(_)));
}

#[test]
fn one_response_runs_at_a_time_and_input_is_accepted_while_it_does() {
    let now = Instant::now();
    let mut ledger = open(&full(), config(TurnDetection::Disabled), now);
    ledger.request_response(now).expect("none in progress");
    let first = ResponseId::generate();
    ledger.response_created(first, now).expect("created");
    assert_eq!(ledger.in_progress(), Some(first));
    assert_eq!(
        ledger.request_response(now),
        Err(SessionError::ResponseInProgress { response_id: first })
    );
    assert_eq!(
        ledger.response_created(ResponseId::generate(), now),
        Err(SessionError::ResponseInProgress { response_id: first })
    );
    ledger
        .output_transcript(first, "The weather".to_owned(), now)
        .expect("in progress");
    ledger
        .accept_input(frame(pcm(16_000), 0, 7))
        .expect("input during output");
    ledger
        .output_transcript(first, " in Berlin".to_owned(), now)
        .expect("the same response goes on");
    ledger.response_completed(first, now).expect("completed");
    assert_eq!(ledger.in_progress(), None);
    assert_eq!(ledger.responses().len(), 1);
    let created = drain(&mut ledger)
        .into_iter()
        .filter(|event| matches!(event, SessionEvent::ResponseCreated(_)))
        .count();
    assert_eq!(created, 1, "no second response id");
}

#[test]
fn barge_in_cancels_the_response_drops_its_late_audio_and_truncates_at_the_heard_sample() {
    let now = Instant::now();
    let mut ledger = open(&full(), config(TurnDetection::Disabled), now);
    let response = ResponseId::generate();
    ledger.response_created(response, now).expect("created");
    ledger
        .output_audio(response, frame(pcm(16_000), 0, 0), now)
        .expect("in progress");
    assert_eq!(
        ledger.output_audio(response, frame(pcm(24_000), 320, 0), now),
        Err(SessionError::FormatMismatch {
            expected: pcm(16_000),
            found: pcm(24_000),
        })
    );
    ledger
        .speech_started(SampleClockPosition::new(4_000).expect("a position"), now)
        .expect("observed");
    ledger.cancel(response).expect("barge-in");
    assert_eq!(
        ledger.output_audio(response, frame(pcm(16_000), 320, 0), now),
        Err(SessionError::ResponseNotInProgress(ResponseStateConflict {
            current: ResponseState::Cancelled
        }))
    );
    let heard = SampleClockPosition::new(200).expect("a position");
    ledger
        .truncate(response, heard)
        .expect("within the emitted audio");
    assert_eq!(
        ledger.truncate(response, SampleClockPosition::new(321).expect("a position")),
        Err(SessionError::BeyondOutput {
            heard: SampleClockPosition::new(321).expect("a position"),
            emitted: 320,
        })
    );
    let events = drain(&mut ledger);
    let kinds: Vec<&str> = events.iter().map(kind).collect();
    assert_eq!(
        kinds,
        [
            "ResponseCreated",
            "OutputAudio",
            "SpeechStarted",
            "ResponseCancelled",
            "OutputTruncated"
        ]
    );
    assert!(matches!(
        events[4],
        SessionEvent::OutputTruncated(truncated) if truncated.heard == heard
    ));

    let next = ResponseId::generate();
    ledger.response_created(next, now).expect("a new response");
    assert_ne!(next, response);
}

#[test]
fn a_model_without_manual_turns_refuses_the_operations_that_need_them() {
    let now = Instant::now();
    let mut server = config(TurnDetection::ServerVad);
    server.auto_response = true;
    server.auto_interrupt = true;
    let mut ledger = open(&server_turns(), server, now);
    assert_eq!(
        ledger.commit_input(now),
        Err(SessionError::Unsupported(Unsupported::ManualCommit))
    );
    assert_eq!(
        ledger.request_response(now),
        Err(SessionError::Unsupported(Unsupported::ManualResponse))
    );
    let response = ResponseId::generate();
    ledger
        .response_created(response, now)
        .expect("the provider's own");
    assert_eq!(
        ledger.truncate(response, SampleClockPosition::new(0).expect("a position")),
        Err(SessionError::Unsupported(Unsupported::Truncation))
    );
}

#[test]
fn a_tool_call_names_an_offered_tool_and_is_answered_once_by_its_call_id() {
    let now = Instant::now();
    let mut ledger = open(&full(), config(TurnDetection::Disabled), now);
    let tool_id = ledger.tools()[0].tool_id();
    let response = ResponseId::generate();
    ledger.response_created(response, now).expect("created");
    let call = SpeechCallId::new("call_1");
    assert_eq!(
        ledger.tool_call(call.clone(), response, "stocks", json!({}), now),
        Err(SessionError::ToolNotOffered {
            name: "stocks".to_owned()
        })
    );
    ledger
        .tool_call(
            call.clone(),
            response,
            "weather",
            json!({"city": "Berlin"}),
            now,
        )
        .expect("an offered tool");
    ledger.response_completed(response, now).expect("completed");
    assert_eq!(
        ledger.answer(&SpeechCallId::new("call_9"), json!({})),
        Err(SessionError::UnknownCall {
            call_id: SpeechCallId::new("call_9")
        })
    );
    ledger
        .answer(&call, json!({"celsius": 21}))
        .expect("answered");
    assert_eq!(
        ledger.answer(&call, json!({"celsius": 22})),
        Err(SessionError::CallConflict(SpeechToolCallStateConflict {
            current: SpeechToolCallState::Answered
        }))
    );
    let events = drain(&mut ledger);
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::SpeechToolCallProposed(proposed)
            if proposed.call_id == call
                && proposed.tool_id == tool_id
                && proposed.arguments == json!({"city": "Berlin"})
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::SpeechToolCallAnswered(answered)
            if answered.call_id == call && answered.result == json!({"celsius": 21})
    )));
}

#[test]
fn usage_per_response_and_of_transcription_is_recorded_for_a_meter() {
    let now = Instant::now();
    let mut ledger = open(&full(), config(TurnDetection::Disabled), now);
    let response = ResponseId::generate();
    ledger.response_created(response, now).expect("created");
    ledger.cancel(response).expect("cancelled");
    let usage = ResponseUsage {
        input: TokenCounts {
            text_tokens: 12,
            audio_tokens: 40,
            cached_tokens: 8,
        },
        output: TokenCounts {
            text_tokens: 5,
            audio_tokens: 60,
            cached_tokens: 0,
        },
    };
    ledger
        .response_usage(response, usage, now)
        .expect("a cancelled response is metered too");
    let transcription = TranscriptionUsage {
        input_tokens: 40,
        output_tokens: 6,
    };
    ledger
        .transcription_usage(transcription, now)
        .expect("recorded");
    let events = drain(&mut ledger);
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::ResponseUsageRecorded(recorded)
            if recorded.response_id == response && recorded.usage == usage
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::TranscriptionUsageRecorded(recorded) if recorded.usage == transcription
    )));
}

#[test]
fn a_silent_provider_fails_the_session_as_stalled_at_the_bound_and_not_before() {
    let start = Instant::now();
    let mut ledger = open(&full(), config(TurnDetection::Disabled), start);
    let limit = Duration::from_secs(10);
    ledger
        .request_response(start)
        .expect("the caller awaits the provider");
    ledger.heard(start + Duration::from_secs(1));
    assert!(!ledger.check_liveness(start + limit));
    assert_eq!(ledger.session().state(), SpeechSessionState::Open);
    assert!(ledger.check_liveness(start + Duration::from_secs(1) + limit));
    assert_eq!(ledger.session().state(), SpeechSessionState::Failed);
    let events = drain(&mut ledger);
    assert!(
        matches!(
            events.as_slice(),
            [SessionEvent::SpeechSessionFailed(failed)]
                if failed.failure.kind == SessionFailureKind::Stalled
        ),
        "{events:#?}"
    );
    assert_eq!(
        ledger.accept_input(frame(pcm(16_000), 0, 7)),
        Err(SessionError::NotOpen(SpeechSessionStateConflict {
            current: SpeechSessionState::Failed
        }))
    );
    assert!(
        !ledger.check_liveness(start + limit * 3),
        "a failed session fails once"
    );
}

#[test]
fn the_bound_runs_only_while_the_caller_awaits_the_provider() {
    let start = Instant::now();
    let limit = Duration::from_secs(10);
    let mut idle = open(&full(), config(TurnDetection::Disabled), start);
    assert!(
        !idle.check_liveness(start + limit * 5),
        "a session nobody awaits anything of is idle, not stalled"
    );
    let response = ResponseId::generate();
    idle.response_created(response, start + limit * 5)
        .expect("created");
    assert!(!idle.check_liveness(start + limit * 6 - Duration::from_millis(1)));
    assert!(
        idle.check_liveness(start + limit * 6),
        "a response in progress awaits the provider"
    );
}

#[test]
fn a_closed_transport_fails_the_session_at_once_and_a_closed_session_takes_nothing() {
    let now = Instant::now();
    let mut ledger = open(&full(), config(TurnDetection::Disabled), now);
    ledger
        .fail(SessionFailure {
            kind: SessionFailureKind::TransportClosed,
            detail: "the socket closed".to_owned(),
        })
        .expect("open");
    let events = drain(&mut ledger);
    assert!(matches!(
        events.as_slice(),
        [SessionEvent::SpeechSessionFailed(failed)]
            if failed.failure.kind == SessionFailureKind::TransportClosed
    ));
    assert_eq!(
        ledger.request_response(now),
        Err(SessionError::NotOpen(SpeechSessionStateConflict {
            current: SpeechSessionState::Failed
        }))
    );

    let mut closing = open(&full(), config(TurnDetection::Disabled), now);
    closing.close().expect("open");
    assert_eq!(
        closing.close(),
        Err(SessionError::NotOpen(SpeechSessionStateConflict {
            current: SpeechSessionState::Closed
        }))
    );
    assert_eq!(
        closing.commit_input(now),
        Err(SessionError::NotOpen(SpeechSessionStateConflict {
            current: SpeechSessionState::Closed
        }))
    );
}

fn kind(event: &SessionEvent) -> &'static str {
    match event {
        SessionEvent::InputAccepted(_) => "InputAccepted",
        SessionEvent::InputCommitted(_) => "InputCommitted",
        SessionEvent::SpeechStarted(_) => "SpeechStarted",
        SessionEvent::SpeechStopped(_) => "SpeechStopped",
        SessionEvent::InputTranscribed(_) => "InputTranscribed",
        SessionEvent::TranscriptionUsageRecorded(_) => "TranscriptionUsageRecorded",
        SessionEvent::ResponseCreated(_) => "ResponseCreated",
        SessionEvent::OutputAudio(_) => "OutputAudio",
        SessionEvent::OutputTranscriptDelta(_) => "OutputTranscriptDelta",
        SessionEvent::ResponseCompleted(_) => "ResponseCompleted",
        SessionEvent::ResponseCancelled(_) => "ResponseCancelled",
        SessionEvent::ResponseFailed(_) => "ResponseFailed",
        SessionEvent::OutputTruncated(_) => "OutputTruncated",
        SessionEvent::ResponseUsageRecorded(_) => "ResponseUsageRecorded",
        SessionEvent::SpeechToolCallProposed(_) => "SpeechToolCallProposed",
        SessionEvent::SpeechToolCallAnswered(_) => "SpeechToolCallAnswered",
        SessionEvent::SpeechSessionClosed(_) => "SpeechSessionClosed",
        SessionEvent::SpeechSessionFailed(_) => "SpeechSessionFailed",
    }
}
