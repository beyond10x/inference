//! Nova Sonic as an `inference-duplex` native speech model, offline.
//!
//! The duplex conformance cases the testkit's scripted mock runs
//! (`crates/inference-testkit/src/duplex/mock.rs`) run here against an offline replay of the
//! provider (`tests/replay`), each in the form Nova Sonic's capabilities give it: a case of an
//! option the model does not take asserts the refusal the contract names. No network, no
//! credential.
#![cfg(feature = "duplex")]

mod replay;

use std::num::{NonZeroU16, NonZeroU32};
use std::time::{Duration, Instant};

use aws_sdk_bedrockruntime::config::{Credentials, Region};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use inference_aws::duplex::{CLAIMS, NovaSonicSpeech, capabilities, endpointing_applies};
use inference_aws::nova_sonic::{NovaSonic, ServiceError, ServiceErrorKind};
use inference_duplex::conformance::{NativeDuplexProbe, NoNativeDuplex, native_duplex_evidence};
use inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_duplex::inference_core::model::Capability;
use inference_duplex::inference_tools::{DeclareTool, SideEffectClass, Tool};
use inference_duplex::{
    NativeSpeechModel, OpenError, ResponseId, ResponseState, ResponseUsage, SessionConfig,
    SessionError, SessionEvent, SessionFailureKind, SpeechSessionHandle, SpeechSessionState,
    TokenCounts, TuningOption, TurnDetection, TurnSensitivity, TurnTuning, Unsupported,
};
use replay::{Faults, Replay, Reply, Turn, turn};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_millis(100);

fn pcm(sample_rate: u32) -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(sample_rate).expect("a positive rate"),
        channels: NonZeroU16::MIN,
    }
}

/// A 20 ms frame of `format` at sample `position`, every byte `byte`.
fn frame(format: AudioFormat, position: u64, byte: u8) -> AudioFrame {
    let samples = format.sample_rate.get() / 50;
    AudioFrame {
        format,
        position: SampleClockPosition::new(position).expect("a position"),
        sample_count: samples,
        data: vec![byte; usize::try_from(format.data_len(samples)).expect("a length")],
    }
}

/// A 20 ms frame that says `text` to the replay's recogniser: the text's bytes, then silence.
fn saying(format: AudioFormat, position: u64, text: &str) -> AudioFrame {
    let mut spoken = frame(format, position, 0);
    for (slot, byte) in spoken.data.iter_mut().zip(text.bytes()) {
        *slot = byte;
    }
    spoken
}

fn weather() -> Tool {
    Tool::declare(DeclareTool {
        name: "weather".to_owned(),
        schema: json!({"type": "object", "properties": {"city": {"type": "string"}}}),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0
}

/// The configuration Nova Sonic takes: its own turns, PCM at 16 kHz both ways.
fn config() -> SessionConfig {
    SessionConfig {
        voice: "tiffany".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: pcm(16_000),
        output_format: pcm(16_000),
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools: vec![weather()],
        idle_limit: Duration::from_secs(5),
    }
}

fn model(replay: &Replay) -> NovaSonicSpeech<Replay> {
    NovaSonicSpeech::with_connector(replay.clone()).expect("the binding's runtime starts")
}

/// Speaks `frames` frames of speech then one of silence, from sample `from`.
fn speak(session: &mut impl SpeechSessionHandle, from: u64, frames: u64) {
    for index in 0..=frames {
        let byte = if index < frames { 9 } else { 0 };
        session
            .append_audio(frame(pcm(16_000), from + index * 320, byte))
            .expect("input in the session's format");
    }
}

/// Every event until nothing arrives for one wait.
fn drain(session: &mut impl SpeechSessionHandle) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    while let Some(event) = session.next_event(WAIT) {
        events.push(event);
        assert!(events.len() < 500, "the session never went quiet");
    }
    events
}

/// Every event up to and including the first `stop` holds for.
fn until(
    session: &mut impl SpeechSessionHandle,
    stop: impl Fn(&SessionEvent) -> bool,
) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    let mut idle = 0;
    loop {
        match session.next_event(WAIT) {
            Some(event) => {
                let done = stop(&event);
                events.push(event);
                if done {
                    return events;
                }
            }
            None => {
                idle += 1;
                assert!(idle < 30, "the awaited event never came: {events:#?}");
            }
        }
    }
}

fn created(events: &[SessionEvent]) -> Vec<ResponseId> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ResponseCreated(created) => Some(created.response_id),
            _ => None,
        })
        .collect()
}

fn output_of(events: &[SessionEvent]) -> Vec<ResponseId> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::OutputAudio(audio) => Some(audio.response_id),
            SessionEvent::OutputTranscriptDelta(delta) => Some(delta.response_id),
            _ => None,
        })
        .collect()
}

fn transcript(events: &[SessionEvent], response_id: ResponseId) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::OutputTranscriptDelta(delta) if delta.response_id == response_id => {
                Some(delta.text.as_str())
            }
            _ => None,
        })
        .collect()
}

fn position(events: &[SessionEvent], what: impl Fn(&SessionEvent) -> bool) -> Option<usize> {
    events.iter().position(what)
}

/// The replay's input events named `name`, waiting up to a second for `count` of them.
fn wire(replay: &Replay, name: &str, count: usize) -> Vec<Value> {
    let started = Instant::now();
    loop {
        let found = replay.received_named(name);
        if found.len() >= count || started.elapsed() > Duration::from_secs(1) {
            return found;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn the_binding_declares_what_nova_sonic_takes() {
    let declared = capabilities("amazon.nova-2-sonic-v1:0");
    let lpcm = vec![pcm(8_000), pcm(16_000), pcm(24_000)];
    assert_eq!(declared.input_formats, lpcm);
    assert_eq!(declared.output_formats, lpcm);
    assert_eq!(declared.turn_detection, vec![TurnDetection::ServerVad]);
    assert!(declared.turn_tuning.sensitivity, "Nova 2 endpointing");
    assert!(!declared.turn_tuning.silence && !declared.turn_tuning.prefix_padding);
    assert!(!declared.manual_commit);
    assert!(!declared.manual_response);
    assert!(!declared.manual_interrupt);
    assert!(!declared.client_cancel);
    assert!(!declared.truncation);
    assert!(declared.tool_calling);

    assert!(
        !capabilities("amazon.nova-sonic-v1:0")
            .turn_tuning
            .sensitivity
    );
    assert!(endpointing_applies("amazon.nova-2-sonic-v1:0"));
    assert!(endpointing_applies(
        "arn:aws:bedrock:us-east-1:123456789012:inference-profile/us.amazon.nova-2-sonic-v1:0"
    ));
    assert!(!endpointing_applies("amazon.nova-sonic-v1:0"));

    assert_eq!(
        CLAIMS.to_vec(),
        vec![
            Capability::OutputStreaming,
            Capability::IncrementalInput,
            Capability::ConcurrentInputOutput,
            Capability::ToolCalling,
            Capability::Interruptible,
        ]
    );
    let mut replay = Replay::new(Vec::new());
    replay.model_id =
        "arn:aws:bedrock:us-east-1:123456789012:inference-profile/us.amazon.nova-2-sonic-v1:0"
            .to_owned();
    let model = model(&replay);
    assert_eq!(model.adapter().capabilities(), CLAIMS);
    assert!(
        !model
            .adapter()
            .capabilities()
            .contains(&Capability::NativeDuplex),
        "barge-in restarts the response: no native_duplex claim"
    );
    assert!(
        !model.adapter().backend().contains("123456789012"),
        "{}",
        model.adapter().backend()
    );
    assert_eq!(model.capabilities(), &capabilities(&replay.model_id));
}

/// The mock's `audio_in_gives_audio_and_transcript_out_on_one_response_id`, with the provider's
/// own turn detection.
#[test]
fn audio_in_gives_audio_and_transcript_out_on_one_response_id() {
    let replay = Replay::new(vec![turn("what time is it", "It is noon")]);
    let mut session = model(&replay)
        .open(config())
        .expect("the model takes the configuration");
    speak(&mut session, 0, 3);
    let events = drain(&mut session);

    let responses = created(&events);
    assert_eq!(responses.len(), 1, "{events:#?}");
    let response = responses[0];
    let committed = position(&events, |event| {
        matches!(event, SessionEvent::InputCommitted(_))
    })
    .expect("the provider's turn detection committed the input");
    let heard = position(&events, |event| {
        matches!(event, SessionEvent::InputTranscribed(heard) if heard.text == "what time is it")
    })
    .expect("the user transcript");
    let began = position(&events, |event| {
        matches!(event, SessionEvent::ResponseCreated(_))
    })
    .expect("a response");
    assert!(committed < heard && heard < began, "{events:#?}");

    let audio: Vec<&AudioFrame> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::OutputAudio(audio) => Some(&audio.frame),
            _ => None,
        })
        .collect();
    assert_eq!(audio.len(), 2, "two frames of output audio");
    assert!(audio.iter().all(|frame| frame.format == pcm(16_000)));
    assert_eq!(
        audio
            .iter()
            .map(|frame| (frame.position.get(), frame.sample_count))
            .collect::<Vec<_>>(),
        [(0, 320), (320, 320)]
    );
    assert_eq!(
        transcript(&events, response),
        "It is noon",
        "the final text once; the speculative preview is not delivered"
    );
    assert!(output_of(&events).iter().all(|id| *id == response));
    let completed = position(&events, |event| {
        matches!(event, SessionEvent::ResponseCompleted(done) if done.response_id == response)
    })
    .expect("the response completes");
    assert!(
        events[completed + 1..]
            .iter()
            .all(|event| matches!(event, SessionEvent::ResponseUsageRecorded(_))),
        "{events:#?}"
    );
    assert_eq!(
        session.ledger().response(response).map(|r| r.state()),
        Some(ResponseState::Completed)
    );
}

/// The mock's `speech_during_a_response_cancels_it_and_the_next_response_has_a_new_id`: Nova
/// Sonic's own barge-in, its late audio dropped.
#[test]
fn speech_during_a_response_cancels_it_and_the_next_response_has_a_new_id() {
    let mut replay = Replay::new(vec![
        turn("tell me a story", "Once upon a time"),
        turn("stop", "Sure"),
    ]);
    replay.pace = Duration::from_millis(10);
    replay.audio_frames = 20;
    let mut session = model(&replay)
        .open(config())
        .expect("the model takes the configuration");
    speak(&mut session, 0, 3);
    let events = until(&mut session, |event| {
        matches!(event, SessionEvent::OutputAudio(_))
    });
    let first = created(&events)[0];

    speak(&mut session, 1_280, 2);
    let rest = drain(&mut session);
    let cancelled = position(
        &rest,
        |event| matches!(event, SessionEvent::ResponseCancelled(c) if c.response_id == first),
    )
    .expect("the provider's interruption cancels the first response");
    assert!(
        output_of(&rest[cancelled..]).iter().all(|id| *id != first),
        "nothing of the cancelled response is delivered after it, its late audio neither: {rest:#?}"
    );
    let next = created(&rest);
    assert_eq!(next.len(), 1, "{rest:#?}");
    assert_ne!(next[0], first);
    assert_eq!(transcript(&rest, next[0]), "Sure");
    assert_eq!(
        session.ledger().response(first).map(|r| r.state()),
        Some(ResponseState::Cancelled)
    );
}

/// The mock's `barge_in_decided_by_the_caller_cancels_and_truncates_at_the_heard_sample`: Nova
/// Sonic takes neither, so each is refused by name and the response runs on.
#[test]
fn barge_in_by_the_caller_and_truncation_are_refused() {
    let replay = Replay::new(vec![turn("hello", "Hello there")]);
    let mut model = model(&replay);
    let mut caller = config();
    caller.auto_interrupt = false;
    assert_eq!(
        model.open(caller).err(),
        Some(OpenError::Unsupported(Unsupported::ManualInterrupt))
    );

    let mut session = model.open(config()).expect("the model takes it");
    assert_eq!(
        session.commit_input(),
        Err(SessionError::Unsupported(Unsupported::ManualCommit))
    );
    assert_eq!(
        session.create_response(),
        Err(SessionError::Unsupported(Unsupported::ManualResponse))
    );
    speak(&mut session, 0, 2);
    let events = until(&mut session, |event| {
        matches!(event, SessionEvent::OutputAudio(_))
    });
    let response = created(&events)[0];
    assert_eq!(
        session.cancel_response(response),
        Err(SessionError::Unsupported(Unsupported::ClientCancel))
    );
    assert_eq!(
        session.truncate_output(response, SampleClockPosition::new(160).expect("a position")),
        Err(SessionError::Unsupported(Unsupported::Truncation))
    );
    let rest = drain(&mut session);
    assert!(rest.iter().any(
        |event| matches!(event, SessionEvent::ResponseCompleted(done) if done.response_id == response)
    ));
    assert!(
        !rest
            .iter()
            .any(|event| matches!(event, SessionEvent::OutputTruncated(_)))
    );
}

/// The mock's `a_tool_call_answered_on_the_session_yields_a_response_that_uses_its_result`.
#[test]
fn a_tool_call_answered_on_the_session_yields_a_response_that_uses_its_result() {
    let replay = Replay::new(vec![Turn {
        heard: "how warm is it in Berlin".to_owned(),
        reply: Reply::Call {
            tool: "weather".to_owned(),
            arguments: json!({"city": "Berlin"}),
            then: "It is {result} degrees".to_owned(),
        },
    }]);
    let mut session = model(&replay)
        .open(config())
        .expect("the model takes the configuration");
    speak(&mut session, 0, 2);
    let events = drain(&mut session);
    let first = created(&events)[0];
    let (call_id, response_id, tool_id, arguments) = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::SpeechToolCallProposed(proposed) => Some((
                proposed.call_id.clone(),
                proposed.response_id,
                proposed.tool_id,
                proposed.arguments.clone(),
            )),
            _ => None,
        })
        .expect("a proposed call");
    assert!(
        call_id.as_str().starts_with("tooluse-"),
        "the provider's own id: {call_id}"
    );
    assert_eq!(response_id, first);
    assert_eq!(tool_id, session.ledger().tools()[0].tool_id());
    assert_eq!(arguments, json!({"city": "Berlin"}));
    assert!(events.iter().any(
        |event| matches!(event, SessionEvent::ResponseCompleted(done) if done.response_id == first)
    ));

    session
        .submit_tool_result(&call_id, json!({"celsius": 21}))
        .expect("the call awaits its result");
    let answer = drain(&mut session);
    assert!(answer.iter().any(|event| matches!(
        event,
        SessionEvent::SpeechToolCallAnswered(answered) if answered.call_id == call_id
    )));
    let second = created(&answer);
    assert_eq!(second.len(), 1, "{answer:#?}");
    assert_ne!(second[0], first);
    assert_eq!(
        transcript(&answer, second[0]),
        "It is {\"celsius\":21} degrees"
    );

    let starts = wire(&replay, "contentStart", 3);
    assert!(starts.iter().any(|start| start["type"] == "TOOL"
        && start["toolResultInputConfiguration"]["toolUseId"] == call_id.as_str()));
    let results = wire(&replay, "toolResult", 1);
    assert_eq!(results[0]["content"], "{\"celsius\":21}");
}

#[test]
fn a_tool_result_that_is_not_an_object_is_sent_as_one() {
    let replay = Replay::new(vec![Turn {
        heard: "how warm".to_owned(),
        reply: Reply::Call {
            tool: "weather".to_owned(),
            arguments: json!({}),
            then: "{result}".to_owned(),
        },
    }]);
    let mut session = model(&replay).open(config()).expect("opens");
    speak(&mut session, 0, 2);
    let call_id = drain(&mut session)
        .into_iter()
        .find_map(|event| match event {
            SessionEvent::SpeechToolCallProposed(proposed) => Some(proposed.call_id),
            _ => None,
        })
        .expect("a proposed call");
    session
        .submit_tool_result(&call_id, json!(21))
        .expect("the call awaits its result");
    assert_eq!(
        wire(&replay, "toolResult", 1)[0]["content"],
        "{\"result\":21}"
    );
}

/// The mock's `a_silent_provider_yields_a_stall_failure_at_the_bound_and_a_closed_transport_at_once`,
/// its stall half: the bound runs from the provider's commit of the input.
#[test]
fn a_silent_provider_yields_a_stall_failure_at_the_bound() {
    let mut replay = Replay::new(vec![turn("hi", "Hello")]);
    replay.faults = Faults {
        stall_after: Some(4),
        ..Faults::default()
    };
    let mut caller = config();
    caller.idle_limit = Duration::from_millis(300);
    let mut session = model(&replay).open(caller).expect("opens");
    speak(&mut session, 0, 2);
    let spoken = Instant::now();
    let mut waits = 0;
    let mut events = Vec::new();
    let failure = loop {
        match session.next_event(Duration::from_millis(50)) {
            Some(SessionEvent::SpeechSessionFailed(failed)) => break failed.failure,
            Some(event) => events.push(event),
            None => waits += 1,
        }
        assert!(waits < 100, "the session never failed: {events:#?}");
    };
    assert_eq!(failure.kind, SessionFailureKind::Stalled, "{failure}");
    assert!(
        spoken.elapsed() >= Duration::from_millis(280),
        "{:?}",
        spoken.elapsed()
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::InputCommitted(_)))
    );
    assert_eq!(
        session.ledger().session().state(),
        SpeechSessionState::Failed
    );
}

/// The idle session awaits nothing: no stall without a commit.
#[test]
fn a_session_nobody_awaits_anything_of_is_idle_not_stalled() {
    let replay = Replay::new(Vec::new());
    let mut caller = config();
    caller.idle_limit = Duration::from_millis(50);
    let mut session = model(&replay).open(caller).expect("opens");
    for _ in 0..4 {
        assert_eq!(session.next_event(Duration::from_millis(50)), None);
    }
    assert_eq!(session.ledger().session().state(), SpeechSessionState::Open);
}

/// The transport half of the mock's stall case.
#[test]
fn a_closed_transport_fails_the_session_at_once() {
    let mut replay = Replay::new(vec![turn("hi", "Hello")]);
    replay.faults = Faults {
        close_after: Some(0),
        ..Faults::default()
    };
    let mut session = model(&replay).open(config()).expect("opens");
    let event = session.next_event(Duration::from_secs(2));
    assert!(
        matches!(
            &event,
            Some(SessionEvent::SpeechSessionFailed(failed))
                if failed.failure.kind == SessionFailureKind::TransportClosed
        ),
        "{event:?}"
    );
    assert!(matches!(
        session.append_audio(frame(pcm(16_000), 0, 9)),
        Err(SessionError::NotOpen(_))
    ));
}

/// A service error fails the response in progress, then the session, by kind.
#[test]
fn a_service_error_fails_the_response_then_the_session() {
    let mut replay = Replay::new(vec![turn("hi", "Hello")]);
    replay.faults = Faults {
        error_after: Some((
            6,
            ServiceError::new(ServiceErrorKind::Throttling, "ThrottlingException"),
        )),
        ..Faults::default()
    };
    let mut session = model(&replay).open(config()).expect("opens");
    speak(&mut session, 0, 2);
    let events = until(&mut session, |event| {
        matches!(event, SessionEvent::SpeechSessionFailed(_))
    });
    let response = created(&events)[0];
    let failed = position(
        &events,
        |event| matches!(event, SessionEvent::ResponseFailed(f) if f.response_id == response),
    )
    .expect("the response fails first");
    let SessionEvent::SpeechSessionFailed(session_failed) = &events[events.len() - 1] else {
        unreachable!("until stops at the failure");
    };
    assert!(failed < events.len() - 1);
    assert_eq!(session_failed.failure.kind, SessionFailureKind::Rejected);

    for (kind, expected) in [
        (
            ServiceErrorKind::ModelStreamError,
            SessionFailureKind::ProviderError,
        ),
        (ServiceErrorKind::Validation, SessionFailureKind::Rejected),
        (
            ServiceErrorKind::Other("Transport".to_owned()),
            SessionFailureKind::TransportClosed,
        ),
    ] {
        let mut replay = Replay::new(Vec::new());
        replay.faults = Faults {
            error_after: Some((0, ServiceError::new(kind.clone(), "it failed"))),
            ..Faults::default()
        };
        let mut session = model(&replay).open(config()).expect("opens");
        let event = session.next_event(Duration::from_secs(2));
        assert!(
            matches!(
                &event,
                Some(SessionEvent::SpeechSessionFailed(failed)) if failed.failure.kind == expected
            ),
            "{kind:?}: {event:?}"
        );
    }
}

/// The mock's `a_model_whose_turns_are_its_own_refuses_caller_turns_at_open`.
#[test]
fn a_model_whose_turns_are_its_own_refuses_caller_turns_at_open() {
    let mut model = model(&Replay::new(Vec::new()));
    for (mode, refused) in [
        (
            TurnDetection::Disabled,
            Unsupported::TurnDetection(TurnDetection::Disabled),
        ),
        (
            TurnDetection::SemanticVad,
            Unsupported::TurnDetection(TurnDetection::SemanticVad),
        ),
    ] {
        let mut caller = config();
        caller.turn_detection = mode;
        assert_eq!(
            model.open(caller).err(),
            Some(OpenError::Unsupported(refused))
        );
    }
    let mut caller_responses = config();
    caller_responses.auto_response = false;
    assert_eq!(
        model.open(caller_responses).err(),
        Some(OpenError::Unsupported(Unsupported::ManualResponse))
    );
    let mut caller_barge_in = config();
    caller_barge_in.auto_interrupt = false;
    assert_eq!(
        model.open(caller_barge_in).err(),
        Some(OpenError::Unsupported(Unsupported::ManualInterrupt))
    );
    assert!(model.open(config()).is_ok());
}

/// The mock's `a_model_without_client_cancel_or_tuning_refuses_them`: silence and padding are
/// refused, the sensitivity is Nova 2's endpointing and reaches the session start.
#[test]
fn tuning_other_than_the_sensitivity_is_refused_and_the_sensitivity_is_sent() {
    let replay = Replay::new(Vec::new());
    let mut model = model(&replay);
    for (tuning, option) in [
        (
            TurnTuning {
                silence: Some(Duration::from_millis(500)),
                ..TurnTuning::default()
            },
            TuningOption::Silence,
        ),
        (
            TurnTuning {
                prefix_padding: Some(Duration::from_millis(300)),
                ..TurnTuning::default()
            },
            TuningOption::PrefixPadding,
        ),
    ] {
        let mut tuned = config();
        tuned.turn_tuning = tuning;
        assert_eq!(
            model.open(tuned).err(),
            Some(OpenError::Unsupported(Unsupported::TurnTuning(option)))
        );
    }

    let mut sessions = Vec::new();
    for sensitivity in TurnSensitivity::ALL {
        let mut sensitive = config();
        sensitive.turn_tuning.sensitivity = Some(sensitivity);
        sessions.push(model.open(sensitive).expect("the sensitivity is honoured"));
    }
    sessions.push(model.open(config()).expect("no tuning"));
    let starts = wire(&replay, "sessionStart", 4);
    assert_eq!(
        starts
            .iter()
            .map(|start| start["turnDetectionConfiguration"]["endpointingSensitivity"].clone())
            .collect::<Vec<_>>(),
        [json!("LOW"), json!("MEDIUM"), json!("HIGH"), Value::Null]
    );

    let mut first = Replay::new(Vec::new());
    first.model_id = "amazon.nova-sonic-v1:0".to_owned();
    let mut sensitive = config();
    sensitive.turn_tuning.sensitivity = Some(TurnSensitivity::Low);
    assert_eq!(
        self::model(&first).open(sensitive).err(),
        Some(OpenError::Unsupported(Unsupported::TurnTuning(
            TuningOption::Sensitivity
        ))),
        "endpointing is Nova 2's"
    );
}

#[test]
fn formats_other_than_lpcm_at_8_16_or_24_khz_are_refused() {
    let mut model = model(&Replay::new(Vec::new()));
    let mulaw = AudioFormat {
        encoding: AudioEncoding::PcmMulaw,
        ..pcm(8_000)
    };
    let mut telephony = config();
    telephony.input_format = mulaw;
    assert_eq!(
        model.open(telephony).err(),
        Some(OpenError::Unsupported(Unsupported::InputFormat(mulaw)))
    );
    let mut wideband = config();
    wideband.output_format = pcm(48_000);
    assert_eq!(
        model.open(wideband).err(),
        Some(OpenError::Unsupported(Unsupported::OutputFormat(pcm(
            48_000
        ))))
    );
    for rate in [8_000, 16_000, 24_000] {
        let mut lpcm = config();
        lpcm.input_format = pcm(rate);
        lpcm.output_format = pcm(rate);
        assert!(model.open(lpcm).is_ok(), "{rate}");
    }
}

/// Voice, instructions, rates and tools reach the opening events; frames pass untouched.
#[test]
fn the_session_configuration_reaches_the_provider() {
    let replay = Replay::new(vec![turn("hi", "Hello")]);
    let mut telephony = config();
    telephony.input_format = pcm(8_000);
    telephony.output_format = pcm(8_000);
    let mut session = model(&replay).open(telephony).expect("opens");

    let prompt = &wire(&replay, "promptStart", 1)[0];
    assert_eq!(prompt["audioOutputConfiguration"]["voiceId"], "tiffany");
    assert_eq!(prompt["audioOutputConfiguration"]["sampleRateHertz"], 8000);
    let tool = &prompt["toolConfiguration"]["tools"][0]["toolSpec"];
    assert_eq!(tool["name"], "weather");
    assert_eq!(
        serde_json::from_str::<Value>(tool["inputSchema"]["json"].as_str().expect("a string"))
            .expect("the schema as JSON"),
        *weather().schema()
    );
    let system = wire(&replay, "textInput", 1);
    assert_eq!(system[0]["content"], "answer briefly");

    let spoken = frame(pcm(8_000), 0, 7);
    session.append_audio(spoken.clone()).expect("8 kHz input");
    session
        .append_audio(frame(pcm(8_000), 160, 0))
        .expect("8 kHz silence");
    let starts = wire(&replay, "contentStart", 2);
    assert_eq!(starts[1]["type"], "AUDIO");
    assert_eq!(
        starts[1]["audioInputConfiguration"]["sampleRateHertz"],
        8000
    );
    let audio = wire(&replay, "audioInput", 1);
    assert_eq!(
        STANDARD
            .decode(audio[0]["content"].as_str().expect("base64"))
            .expect("base64"),
        spoken.data
    );
    assert!(matches!(
        session.append_audio(frame(pcm(16_000), 320, 0)),
        Err(SessionError::FormatMismatch { .. })
    ));

    let events = drain(&mut session);
    let audio: Vec<&AudioFrame> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::OutputAudio(audio) => Some(&audio.frame),
            _ => None,
        })
        .collect();
    assert!(!audio.is_empty(), "{events:#?}");
    assert!(
        audio
            .iter()
            .all(|frame| frame.format == pcm(8_000) && frame.sample_count == 160)
    );
}

/// Nova Sonic reports usage as deltas; a response's usage is their running total, the input
/// reported before it began counted as its input.
#[test]
fn usage_is_a_running_total_per_response() {
    let replay = Replay::new(vec![turn("hi", "Hello"), turn("again", "Hello again")]);
    let mut session = model(&replay).open(config()).expect("opens");
    speak(&mut session, 0, 2);
    let first_turn = drain(&mut session);
    speak(&mut session, 960, 2);
    let second_turn = drain(&mut session);
    let expected = ResponseUsage {
        input: TokenCounts {
            text_tokens: 0,
            audio_tokens: 50,
            cached_tokens: 0,
        },
        output: TokenCounts {
            text_tokens: 8,
            audio_tokens: 30,
            cached_tokens: 0,
        },
    };
    for events in [&first_turn, &second_turn] {
        let response = created(events)[0];
        assert_eq!(session.ledger().usage(response), Some(expected));
        let recorded: Vec<ResponseUsage> = events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::ResponseUsageRecorded(usage) if usage.response_id == response => {
                    Some(usage.usage)
                }
                _ => None,
            })
            .collect();
        assert_eq!(recorded.last(), Some(&expected), "{events:#?}");
        assert!(
            recorded.windows(2).all(|pair| pair[0].output.audio_tokens
                <= pair[1].output.audio_tokens
                && pair[0].output.text_tokens <= pair[1].output.text_tokens),
            "running totals never fall: {recorded:?}"
        );
    }
}

/// Closing cancels the response in progress, ends the prompt and the session on the wire, and
/// nothing the provider sends after it is delivered.
#[test]
fn closing_cancels_the_response_in_progress_and_drops_what_follows() {
    let mut replay = Replay::new(vec![turn("tell me a story", "Once upon a time")]);
    replay.pace = Duration::from_millis(10);
    replay.audio_frames = 20;
    let mut session = model(&replay).open(config()).expect("opens");
    speak(&mut session, 0, 2);
    let events = until(&mut session, |event| {
        matches!(event, SessionEvent::OutputAudio(_))
    });
    let response = created(&events)[0];
    session.close().expect("open");
    assert!(matches!(
        session.next_event(WAIT),
        Some(SessionEvent::ResponseCancelled(c)) if c.response_id == response
    ));
    assert!(matches!(
        session.next_event(WAIT),
        Some(SessionEvent::SpeechSessionClosed(_))
    ));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(session.next_event(WAIT), None);
    assert_eq!(wire(&replay, "promptEnd", 1).len(), 1);
    assert_eq!(wire(&replay, "sessionEnd", 1).len(), 1);
    assert!(matches!(session.close(), Err(SessionError::NotOpen(_))));
    assert!(matches!(
        session.append_audio(frame(pcm(16_000), 0, 0)),
        Err(SessionError::NotOpen(_))
    ));
}

#[test]
fn a_provider_that_refuses_or_cannot_be_reached_opens_nothing() {
    let mut refusing = Replay::new(Vec::new());
    refusing.faults.refuse = true;
    let refused = model(&refusing).open(config()).err();
    assert!(
        matches!(&refused, Some(OpenError::Failed(failure)) if failure.kind == SessionFailureKind::Rejected),
        "{refused:?}"
    );

    let unreachable = aws_sdk_bedrockruntime::Config::builder()
        .behavior_version_latest()
        .region(Region::new("us-east-1"))
        .endpoint_url("http://127.0.0.1:9")
        .credentials_provider(Credentials::new(
            "AKIDOFFLINE",
            "offline",
            None,
            None,
            "test",
        ))
        .build();
    let nova = NovaSonic::new(
        aws_sdk_bedrockruntime::Client::from_conf(unreachable),
        "amazon.nova-2-sonic-v1:0",
    );
    let mut model = NovaSonicSpeech::new(nova).expect("the binding's runtime starts");
    let failed = model.open(config()).err();
    assert!(
        matches!(&failed, Some(OpenError::Failed(failure)) if failure.kind == SessionFailureKind::TransportClosed),
        "{failed:?}"
    );
}

/// The probe of `native_duplex_evidence`: a question, then during the answer a place the
/// replay's recogniser hears from the audio.
fn probe(auto_interrupt: bool) -> NativeDuplexProbe {
    let mut probe_config = config();
    probe_config.auto_interrupt = auto_interrupt;
    NativeDuplexProbe {
        config: probe_config,
        opening: vec![
            saying(pcm(16_000), 0, "what is the weather"),
            frame(pcm(16_000), 320, 0),
        ],
        during: vec![
            saying(pcm(16_000), 640, "in Lisbon"),
            frame(pcm(16_000), 960, 0),
        ],
        marker: "lisbon".to_owned(),
        wait: WAIT,
        patience: 20,
    }
}

fn weather_script() -> Vec<Turn> {
    vec![
        turn("a question", "The weather is"),
        turn("a place", " mild {heard}"),
    ]
}

/// The mock's `the_native_mock_gives_evidence_for_native_duplex_from_what_it_heard`, which Nova
/// Sonic cannot pass: the probe's caller-decided barge-in is refused at open.
#[test]
fn the_native_duplex_probe_is_refused_at_open() {
    let refused = native_duplex_evidence(&mut model(&Replay::new(weather_script())), &probe(false));
    assert_eq!(
        refused,
        Err(NoNativeDuplex::Open(OpenError::Unsupported(
            Unsupported::ManualInterrupt
        )))
    );
}

/// The mock's `a_mock_that_restarts_on_input_during_output_gives_no_evidence_for_native_duplex`:
/// with the barge-in Nova Sonic takes, input during output cancels the response and answers in a
/// new one.
#[test]
fn with_its_own_barge_in_input_during_output_restarts_the_response() {
    let mut replay = Replay::new(weather_script());
    replay.pace = Duration::from_millis(10);
    replay.audio_frames = 20;
    let refused = native_duplex_evidence(&mut model(&replay), &probe(true));
    assert!(
        matches!(refused, Err(NoNativeDuplex::Restarted { first, second }) if first != second),
        "{refused:?}"
    );
}
