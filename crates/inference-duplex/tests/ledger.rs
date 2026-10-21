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
    OpenError, Quantity, QuantityKind, ResponseId, ResponseState, ResponseStateConflict,
    ResponseUsage, SessionConfig, SessionError, SessionEvent, SessionFailure, SessionFailureKind,
    SessionLedger, SpeechCallId, SpeechCapabilities, SpeechSessionId, SpeechSessionState,
    SpeechSessionStateConflict, SpeechToolCallState, SpeechToolCallStateConflict, TokenCounts,
    TranscriptionUsage, TuningSupport, TurnDetection, TurnTuning, Unsupported, UsageRecorded,
    UsageTotal,
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

/// A model that creates every response owes one after the caller answers its call, as after a
/// commit: the bound runs from the first check after the answer, the time the caller took to
/// compute the result not counted, and a model that takes responses from the caller is not
/// awaited until the caller asks.
#[test]
fn answering_a_call_of_a_model_that_creates_every_response_arms_the_bound() {
    let start = Instant::now();
    let limit = Duration::from_secs(10);
    let mut caller = config(TurnDetection::ServerVad);
    caller.auto_response = true;
    caller.auto_interrupt = true;
    let mut ledger = open(&server_turns(), caller, start);
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
    let answered = start + limit * 3;
    assert!(!ledger.check_liveness(answered), "nothing awaited yet");
    ledger
        .answer(&SpeechCallId::new("call-1"), json!({"celsius": 21}))
        .expect("answered");
    assert!(
        !ledger.check_liveness(answered),
        "the bound starts at the first check after the answer"
    );
    assert!(!ledger.check_liveness(answered + limit - Duration::from_millis(1)));
    assert!(
        ledger.check_liveness(answered + limit),
        "the provider owes the response that answers the result"
    );

    let mut caller = config(TurnDetection::ServerVad);
    caller.auto_response = true;
    caller.auto_interrupt = true;
    let mut early = open(&server_turns(), caller, start);
    early.response_created(response, start).expect("created");
    early
        .tool_call(
            SpeechCallId::new("call-1"),
            response,
            "weather",
            json!({}),
            start,
        )
        .expect("proposed");
    early
        .answer(&SpeechCallId::new("call-1"), json!({"celsius": 21}))
        .expect("answered before the proposing response ended");
    let ended = start + limit * 2;
    early
        .response_completed(response, ended)
        .expect("completed");
    assert!(!early.check_liveness(ended + limit - Duration::from_millis(1)));
    assert!(
        early.check_liveness(ended + limit),
        "the response that answers the result is still owed after the proposing one ended"
    );

    let mut caller_led = open(&full(), config(TurnDetection::Disabled), start);
    caller_led
        .response_created(response, start)
        .expect("created");
    caller_led
        .tool_call(
            SpeechCallId::new("call-1"),
            response,
            "weather",
            json!({}),
            start,
        )
        .expect("proposed");
    caller_led
        .response_completed(response, start)
        .expect("completed");
    caller_led
        .answer(&SpeechCallId::new("call-1"), json!(21))
        .expect("answered");
    assert!(!caller_led.check_liveness(start));
    assert!(
        !caller_led.check_liveness(start + limit * 5),
        "the caller asks for the next response itself"
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

/// The token kinds every response record carries, unknown where the provider left them out.
const RESPONSE_TOKENS: [QuantityKind; 5] = [
    QuantityKind::InputTextTokens,
    QuantityKind::InputAudioTokens,
    QuantityKind::CachedInputTokens,
    QuantityKind::OutputTextTokens,
    QuantityKind::OutputAudioTokens,
];

fn records(events: &[SessionEvent]) -> Vec<&UsageRecorded> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::UsageRecorded(recorded) => Some(recorded),
            _ => None,
        })
        .collect()
}

fn quantity(record: &UsageRecorded, kind: QuantityKind) -> Quantity {
    let found: Vec<&Quantity> = record
        .quantities
        .iter()
        .filter(|quantity| quantity.kind() == kind)
        .collect();
    assert_eq!(found.len(), 1, "one {kind:?} quantity: {record:#?}");
    *found[0]
}

fn kinds(events: &[SessionEvent]) -> Vec<&'static str> {
    events.iter().map(kind).collect()
}

#[test]
fn a_response_ending_with_its_final_usage_is_recorded_once_just_before_its_end() {
    let start = Instant::now();
    let adapter = adapter();
    let session_id = SpeechSessionId::generate();
    let (mut ledger, _) = SessionLedger::open(
        session_id,
        &adapter,
        &full(),
        config(TurnDetection::Disabled),
        start,
    )
    .expect("the session opens");
    let response = ResponseId::generate();
    ledger
        .response_created(response, start + Duration::from_millis(100))
        .expect("created");
    ledger
        .response_usage_reported(
            response,
            None,
            vec![
                Quantity::provider_reported(QuantityKind::InputTextTokens, 12),
                Quantity::provider_reported(QuantityKind::OutputAudioTokens, 60),
            ],
            UsageTotal::Final,
            start + Duration::from_millis(300),
        )
        .expect("a response of the session");
    ledger
        .response_completed(response, start + Duration::from_millis(400))
        .expect("in progress");
    let events = drain(&mut ledger);
    assert_eq!(
        kinds(&events),
        ["ResponseCreated", "UsageRecorded", "ResponseCompleted"]
    );
    let recorded = records(&events);
    assert_eq!(recorded.len(), 1);
    let record = recorded[0];
    assert_eq!(record.speech_session_id, Some(session_id));
    assert_eq!(record.response_id, Some(response));
    assert_eq!(record.adapter_id, adapter.adapter_id());
    assert_eq!(record.model_id, adapter.model_id());
    assert_eq!(
        (record.run_id, record.node_id, record.session_id),
        (None, None, None),
        "a speech session runs in no graph run"
    );
    assert_eq!(record.upstream_model, None);
    assert!(record.r#final);
    assert_eq!(record.ended_at_ms - record.started_at_ms, 300);
    assert_eq!(
        quantity(record, QuantityKind::InputTextTokens),
        Quantity::provider_reported(QuantityKind::InputTextTokens, 12)
    );
    assert_eq!(
        quantity(record, QuantityKind::OutputAudioTokens),
        Quantity::provider_reported(QuantityKind::OutputAudioTokens, 60)
    );
    for kind in [
        QuantityKind::InputAudioTokens,
        QuantityKind::CachedInputTokens,
        QuantityKind::OutputTextTokens,
    ] {
        assert_eq!(
            quantity(record, kind),
            Quantity::unknown(kind),
            "a count the provider left out is unknown, never zero"
        );
    }
    assert_eq!(
        quantity(record, QuantityKind::Requests),
        Quantity::measured(QuantityKind::Requests, 1)
    );
    assert_eq!(record.quantities.len(), 6, "{record:#?}");
    assert_eq!(ledger.usage_records().len(), 1);
    assert_eq!(&ledger.usage_records()[0].recorded(), record);

    assert_eq!(
        ledger.response_usage_reported(
            response,
            None,
            vec![Quantity::provider_reported(
                QuantityKind::InputTextTokens,
                99
            )],
            UsageTotal::Final,
            start + Duration::from_millis(500),
        ),
        Err(SessionError::ResponseNotInProgress(ResponseStateConflict {
            current: ResponseState::Completed
        })),
        "a response is recorded once"
    );
    assert!(records(&drain(&mut ledger)).is_empty());
    assert_eq!(ledger.usage_records().len(), 1);
}

#[test]
fn a_response_cancelled_before_any_usage_is_recorded_unknown_when_the_next_begins() {
    let now = Instant::now();
    let mut ledger = open(&server_turns(), server_config(), now);
    let first = ResponseId::generate();
    ledger.response_created(first, now).expect("created");
    ledger
        .response_cancelled(first, now)
        .expect("the provider's barge-in");
    assert!(
        records(&drain(&mut ledger)).is_empty(),
        "usage may still come after the end"
    );
    let second = ResponseId::generate();
    ledger.response_created(second, now).expect("created");
    let events = drain(&mut ledger);
    assert_eq!(kinds(&events), ["UsageRecorded", "ResponseCreated"]);
    let record = records(&events)[0];
    assert_eq!(record.response_id, Some(first));
    for kind in RESPONSE_TOKENS {
        assert_eq!(quantity(record, kind), Quantity::unknown(kind));
    }
    assert_eq!(
        quantity(record, QuantityKind::Requests),
        Quantity::measured(QuantityKind::Requests, 1)
    );
}

#[test]
fn a_final_report_after_the_callers_cancellation_records_the_response_at_once() {
    let now = Instant::now();
    let mut ledger = open(&full(), config(TurnDetection::Disabled), now);
    let response = ResponseId::generate();
    ledger.response_created(response, now).expect("created");
    ledger.cancel(response).expect("barge-in by the caller");
    assert!(records(&drain(&mut ledger)).is_empty());
    ledger
        .response_usage_reported(
            response,
            None,
            vec![Quantity::provider_reported(
                QuantityKind::OutputAudioTokens,
                7,
            )],
            UsageTotal::Final,
            now,
        )
        .expect("a cancelled response is metered too");
    let events = drain(&mut ledger);
    assert_eq!(kinds(&events), ["UsageRecorded"]);
    assert_eq!(
        quantity(records(&events)[0], QuantityKind::OutputAudioTokens),
        Quantity::provider_reported(QuantityKind::OutputAudioTokens, 7)
    );
}

#[test]
fn running_totals_replace_each_other_until_the_response_is_settled() {
    let now = Instant::now();
    let mut ledger = open(&server_turns(), server_config(), now);
    let running = |ledger: &mut SessionLedger, response, output| {
        ledger
            .response_usage_reported(
                response,
                None,
                vec![Quantity::provider_reported(
                    QuantityKind::OutputTextTokens,
                    output,
                )],
                UsageTotal::Running,
                now,
            )
            .expect("a response of the session");
    };
    let first = ResponseId::generate();
    ledger.response_created(first, now).expect("created");
    running(&mut ledger, first, 5);
    ledger.response_completed(first, now).expect("in progress");
    running(&mut ledger, first, 8);
    let second = ResponseId::generate();
    ledger.response_created(second, now).expect("created");
    running(&mut ledger, second, 3);
    ledger.close().expect("open");
    let events = drain(&mut ledger);
    let at = |events: &[SessionEvent], name| {
        events
            .iter()
            .position(|event| kind(event) == name)
            .expect("delivered")
    };
    assert_eq!(
        kinds(&events[events.len() - 3..]),
        ["UsageRecorded", "ResponseCancelled", "SpeechSessionClosed"],
        "the session's end settles the response it cancels, recorded before its end"
    );
    let recorded = records(&events);
    assert_eq!(recorded.len(), 2, "one record per response: {events:#?}");
    assert_eq!(recorded[0].response_id, Some(first));
    assert_eq!(
        quantity(recorded[0], QuantityKind::OutputTextTokens),
        Quantity::provider_reported(QuantityKind::OutputTextTokens, 8),
        "the last running total, reported after the response ended"
    );
    assert!(
        matches!(
            &events[at(&events, "UsageRecorded") + 1],
            SessionEvent::ResponseCreated(created) if created.response_id == second
        ),
        "the next response settles the first: {events:#?}"
    );
    assert_eq!(recorded[1].response_id, Some(second));
    assert_eq!(
        quantity(recorded[1], QuantityKind::OutputTextTokens),
        Quantity::provider_reported(QuantityKind::OutputTextTokens, 3)
    );
}

#[test]
fn a_failed_session_records_the_response_it_fails_before_its_own_failure() {
    let now = Instant::now();
    let mut ledger = open(&full(), config(TurnDetection::Disabled), now);
    let response = ResponseId::generate();
    ledger.response_created(response, now).expect("created");
    ledger
        .fail(SessionFailure {
            kind: SessionFailureKind::TransportClosed,
            detail: "the socket closed".to_owned(),
        })
        .expect("open");
    let events = drain(&mut ledger);
    assert_eq!(
        kinds(&events),
        [
            "ResponseCreated",
            "UsageRecorded",
            "ResponseFailed",
            "SpeechSessionFailed"
        ]
    );
    let record = records(&events)[0];
    for kind in RESPONSE_TOKENS {
        assert_eq!(quantity(record, kind), Quantity::unknown(kind));
    }
}

#[test]
fn the_contracts_usage_forms_are_recorded_as_reported_quantities() {
    let now = Instant::now();
    let mut ledger = open(&full(), config(TurnDetection::Disabled), now);
    let response = ResponseId::generate();
    ledger.response_created(response, now).expect("created");
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
        .expect("a response of the session");
    ledger
        .response_completed(response, now)
        .expect("in progress");
    ledger
        .transcription_usage(
            TranscriptionUsage {
                input_tokens: 40,
                output_tokens: 6,
            },
            now,
        )
        .expect("recorded");
    let events = drain(&mut ledger);
    assert_eq!(
        kinds(&events),
        [
            "ResponseCreated",
            "ResponseUsageRecorded",
            "UsageRecorded",
            "ResponseCompleted",
            "TranscriptionUsageRecorded",
            "UsageRecorded"
        ]
    );
    let recorded = records(&events);
    assert_eq!(
        recorded[0].quantities,
        vec![
            Quantity::provider_reported(QuantityKind::InputTextTokens, 12),
            Quantity::provider_reported(QuantityKind::InputAudioTokens, 40),
            Quantity::provider_reported(QuantityKind::CachedInputTokens, 8),
            Quantity::provider_reported(QuantityKind::OutputTextTokens, 5),
            Quantity::provider_reported(QuantityKind::OutputAudioTokens, 60),
            Quantity::measured(QuantityKind::Requests, 1),
        ]
    );
    assert_eq!(recorded[1].response_id, None);
    assert_eq!(
        recorded[1].speech_session_id,
        Some(ledger.session().session_id())
    );
    assert_eq!(
        recorded[1].quantities,
        vec![
            Quantity::provider_reported(QuantityKind::InputAudioTokens, 40),
            Quantity::provider_reported(QuantityKind::OutputTextTokens, 6),
        ],
        "a transcription is no request of a response"
    );
}

#[test]
fn a_transcription_in_seconds_and_a_session_total_are_records_of_the_session() {
    let start = Instant::now();
    let mut ledger = open(&full(), config(TurnDetection::Disabled), start);
    ledger.close().expect("open");
    ledger
        .transcription_usage_reported(
            None,
            vec![Quantity::provider_reported(
                QuantityKind::InputAudioMs,
                1_500,
            )],
            start + Duration::from_secs(2),
        )
        .expect("in any state of the session");
    ledger
        .session_usage(
            vec![Quantity::provider_reported(
                QuantityKind::InputAudioMs,
                4_500,
            )],
            false,
            start + Duration::from_secs(5),
        )
        .expect("in any state of the session");
    let events = drain(&mut ledger);
    assert_eq!(
        kinds(&events),
        ["SpeechSessionClosed", "UsageRecorded", "UsageRecorded"],
        "a usage in seconds has no TranscriptionUsageRecorded"
    );
    let recorded = records(&events);
    assert_eq!(
        recorded[0].quantities,
        vec![Quantity::provider_reported(
            QuantityKind::InputAudioMs,
            1_500
        )]
    );
    assert_eq!(recorded[0].started_at_ms, recorded[0].ended_at_ms);
    let session = recorded[1];
    assert_eq!(session.response_id, None);
    assert_eq!(
        session.speech_session_id,
        Some(ledger.session().session_id())
    );
    assert_eq!(
        session.ended_at_ms - session.started_at_ms,
        5_000,
        "from the session's opening"
    );
    assert!(!session.r#final, "not confirmed final");
    assert_eq!(
        session.quantities,
        vec![Quantity::provider_reported(
            QuantityKind::InputAudioMs,
            4_500
        )]
    );
    assert_eq!(ledger.usage_records().len(), 2);
}

/// A session billed as a whole, such as seconds of session: the latest total is recorded once, as
/// usage of the session, after the end events of its responses and just before its own, whether
/// a close, a failure or a stall ends it; given after the end, it is recorded at once.
#[test]
fn a_session_total_is_recorded_just_before_the_session_ends() {
    let seconds = |ms| vec![Quantity::provider_reported(QuantityKind::InputAudioMs, ms)];
    let ends: [fn(&mut SessionLedger); 2] = [
        |ledger| ledger.close().expect("open"),
        |ledger| {
            ledger
                .fail(SessionFailure {
                    kind: SessionFailureKind::ProviderError,
                    detail: "the provider ended the session: expired".to_owned(),
                })
                .expect("open");
        },
    ];
    for (end, (ended, last)) in ends.into_iter().zip([
        ("ResponseCancelled", "SpeechSessionClosed"),
        ("ResponseFailed", "SpeechSessionFailed"),
    ]) {
        let start = Instant::now();
        let mut ledger = open(&full(), config(TurnDetection::Disabled), start);
        let response = ResponseId::generate();
        ledger.response_created(response, start).expect("created");
        ledger.session_usage_at_end(seconds(3_000), false);
        ledger.session_usage_at_end(seconds(4_500), true);
        end(&mut ledger);
        let events = drain(&mut ledger);
        assert_eq!(
            kinds(&events),
            [
                "ResponseCreated",
                "UsageRecorded",
                ended,
                "UsageRecorded",
                last
            ]
        );
        let recorded = records(&events);
        assert_eq!(recorded[0].response_id, Some(response));
        assert_eq!(recorded[1].response_id, None);
        assert_eq!(recorded[1].quantities, seconds(4_500), "the latest total");
        assert!(recorded[1].r#final);
        ledger.session_usage_at_end(seconds(5_000), true);
        let after = drain(&mut ledger);
        assert_eq!(kinds(&after), ["UsageRecorded"], "after the end, at once");
        assert_eq!(ledger.usage_records().len(), 3);
    }
}

/// Usage a provider reported before the response it counts toward began, such as input billed
/// before a response the session never reaches: the latest quantities held are recorded once, as
/// usage of the session, before the end event of a close, a failure or a stall.
#[test]
fn usage_held_for_no_response_is_recorded_before_any_end_of_the_session() {
    let held = |input_audio| {
        vec![
            Quantity::provider_reported(QuantityKind::InputTextTokens, 0),
            Quantity::provider_reported(QuantityKind::InputAudioTokens, input_audio),
        ]
    };
    let start = Instant::now();
    let limit = Duration::from_secs(10);

    let mut closed = open(&server_turns(), server_config(), start);
    closed.input_committed(start).expect("open");
    closed.hold_usage(held(20));
    closed.hold_usage(held(50));
    let _ = drain(&mut closed);
    closed.close().expect("open");
    let mut failed = open(&server_turns(), server_config(), start);
    failed.input_committed(start).expect("open");
    failed.hold_usage(held(50));
    let _ = drain(&mut failed);
    failed
        .fail(SessionFailure {
            kind: SessionFailureKind::TransportClosed,
            detail: "gone".to_owned(),
        })
        .expect("open");
    let mut stalled = open(&server_turns(), server_config(), start);
    stalled.input_committed(start).expect("open");
    stalled.hold_usage(held(50));
    let _ = drain(&mut stalled);
    assert!(stalled.check_liveness(start + limit));

    for (ledger, end) in [
        (&mut closed, "SpeechSessionClosed"),
        (&mut failed, "SpeechSessionFailed"),
        (&mut stalled, "SpeechSessionFailed"),
    ] {
        let events = drain(ledger);
        assert_eq!(kinds(&events), ["UsageRecorded", end], "{events:#?}");
        let recorded = records(&events)[0];
        assert_eq!(recorded.response_id, None);
        assert_eq!(
            recorded.speech_session_id,
            Some(ledger.session().session_id())
        );
        assert_eq!(
            recorded.quantities,
            held(50),
            "the latest held replaces the rest"
        );
        assert_eq!(ledger.usage_records().len(), 1);
    }
}

#[test]
fn usage_held_and_then_released_is_recorded_by_nobody_at_the_end() {
    let start = Instant::now();
    let mut ledger = open(&server_turns(), server_config(), start);
    ledger.hold_usage(vec![Quantity::provider_reported(
        QuantityKind::InputAudioTokens,
        50,
    )]);
    ledger.hold_usage(Vec::new());
    ledger.close().expect("open");
    assert_eq!(kinds(&drain(&mut ledger)), ["SpeechSessionClosed"]);
    assert!(ledger.usage_records().is_empty());
}

#[test]
fn usage_held_after_the_session_ended_is_recorded_at_once() {
    let start = Instant::now();
    let mut ledger = open(&server_turns(), server_config(), start);
    ledger.close().expect("open");
    ledger.hold_usage(vec![Quantity::provider_reported(
        QuantityKind::InputAudioTokens,
        50,
    )]);
    assert_eq!(
        kinds(&drain(&mut ledger)),
        ["SpeechSessionClosed", "UsageRecorded"]
    );
    ledger.hold_usage(Vec::new());
    assert_eq!(ledger.usage_records().len(), 1);
}

fn server_config() -> SessionConfig {
    SessionConfig {
        turn_detection: TurnDetection::ServerVad,
        auto_response: true,
        auto_interrupt: true,
        ..config(TurnDetection::ServerVad)
    }
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
        SessionEvent::UsageRecorded(_) => "UsageRecorded",
    }
}
