//! A closed or failed stream: the server closes mid-segment, sends text that is not JSON, JSON
//! that is not a message, audio that is not base64, an error, or nothing at all. Each ends the
//! synthesis with a typed failure, no panic and no hang, and its usage record is not final.

mod support;

use std::time::{Duration, Instant};

use inference_elevenlabs::{
    ElevenLabsError, ElevenLabsSynthesis, ElevenLabsSynthesiser, Endpoint, OutputFormat,
    VoiceConfig,
};
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use support::{FixedKey, FixtureServer, KEY, Script, VOICE, committed};

const FORMAT: OutputFormat = OutputFormat::Ulaw8000;

/// Speaks one segment against `script`, ends the utterance, drains the handle and returns it
/// with its events.
fn run(script: Script) -> (ElevenLabsSynthesis, Vec<SynthesisEvent>) {
    let server = FixtureServer::start(FORMAT, script);
    let mut synthesiser = ElevenLabsSynthesiser::new(
        server
            .endpoint()
            .with_read_timeout(Duration::from_millis(300)),
        VoiceConfig::new(VOICE, FORMAT),
        std::sync::Arc::new(FixedKey(KEY)),
        "elevenlabs-api-key",
    )
    .unwrap();
    drain(&mut synthesiser)
}

fn drain(synthesiser: &mut ElevenLabsSynthesiser) -> (ElevenLabsSynthesis, Vec<SynthesisEvent>) {
    let mut handle = synthesiser.start().unwrap();
    handle
        .offer_segment(committed(&["This will not finish."]).remove(0))
        .unwrap();
    handle.end_utterance().unwrap();
    let started = Instant::now();
    let events: Vec<SynthesisEvent> = handle.by_ref().take(100).collect();
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the failure took {:?}",
        started.elapsed()
    );
    (handle, events)
}

/// The synthesis failed, ended with a usage record that is not final, and yields nothing more.
fn assert_failed_and_ended(handle: &mut ElevenLabsSynthesis, events: &[SynthesisEvent]) {
    assert!(handle.output().failed());
    match events.last() {
        Some(SynthesisEvent::Usage(record)) => assert!(!record.r#final),
        other => panic!("the last event is not the usage record: {other:?}"),
    }
    assert_eq!(handle.next(), None);
}

#[test]
fn the_server_closing_mid_segment_is_a_closed_failure() {
    let (mut handle, events) = run(Script::CloseAfterFirst);
    assert!(matches!(events.first(), Some(SynthesisEvent::Audio(_))));
    assert!(
        matches!(
            handle.failure(),
            Some(ElevenLabsError::Closed {
                code: Some(1011),
                ..
            })
        ),
        "{:?}",
        handle.failure()
    );
    assert_failed_and_ended(&mut handle, &events);
}

#[test]
fn a_frame_that_is_not_json_is_a_protocol_failure() {
    let (mut handle, events) = run(Script::SendAfterFirst("this is not json"));
    assert!(
        matches!(handle.failure(), Some(ElevenLabsError::Protocol(_))),
        "{:?}",
        handle.failure()
    );
    assert_failed_and_ended(&mut handle, &events);
}

#[test]
fn malformed_json_is_a_protocol_failure() {
    for malformed in [
        r#"{"audio": 5}"#,
        r#"{"isFinal": "yes"}"#,
        "[1, 2]",
        r#"{"x": 1}"#,
    ] {
        let (mut handle, events) = run(Script::SendAfterFirst(malformed));
        assert!(
            matches!(handle.failure(), Some(ElevenLabsError::Protocol(_))),
            "{malformed}: {:?}",
            handle.failure()
        );
        assert_failed_and_ended(&mut handle, &events);
    }
}

#[test]
fn audio_that_is_not_base64_is_a_protocol_failure() {
    let (mut handle, events) = run(Script::SendAfterFirst(r#"{"audio": "@@ not base64 @@"}"#));
    match handle.failure() {
        Some(ElevenLabsError::Protocol(detail)) => assert!(detail.contains("base64"), "{detail}"),
        other => panic!("{other:?}"),
    }
    assert_failed_and_ended(&mut handle, &events);
}

#[test]
fn an_error_message_is_a_server_failure() {
    let (mut handle, events) = run(Script::SendAfterFirst(
        r#"{"message": "quota exceeded", "error": "quota_exceeded"}"#,
    ));
    assert_eq!(
        handle.failure(),
        Some(&ElevenLabsError::Server("quota exceeded".to_owned()))
    );
    assert_failed_and_ended(&mut handle, &events);
}

#[test]
fn a_silent_server_is_a_stall_within_the_read_timeout() {
    let started = Instant::now();
    let (mut handle, events) = run(Script::Silent);
    assert_eq!(
        handle.failure(),
        Some(&ElevenLabsError::Stalled(Duration::from_millis(300)))
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_failed_and_ended(&mut handle, &events);
}

#[test]
fn no_server_at_all_is_a_connect_failure() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut synthesiser = ElevenLabsSynthesiser::new(
        Endpoint::new()
            .with_url(format!("ws://127.0.0.1:{port}"))
            .with_connect_timeout(Duration::from_secs(2)),
        VoiceConfig::new(VOICE, FORMAT),
        std::sync::Arc::new(FixedKey(KEY)),
        "elevenlabs-api-key",
    )
    .unwrap();
    let (mut handle, events) = drain(&mut synthesiser);
    match handle.failure() {
        Some(ElevenLabsError::Connect(error)) => assert_eq!(error.status, None),
        other => panic!("{other:?}"),
    }
    assert_failed_and_ended(&mut handle, &events);
}

#[test]
fn a_segment_offered_after_a_failure_is_not_spoken_and_the_record_still_comes() {
    let server = FixtureServer::start(FORMAT, Script::CloseAfterFirst);
    let mut synthesiser = server.synthesiser(FORMAT);
    let mut handle = synthesiser.start().unwrap();
    handle
        .offer_segment(committed(&["First."]).remove(0))
        .unwrap();
    let _: Vec<SynthesisEvent> = handle.by_ref().take(100).collect();
    assert!(handle.failure().is_some());
    handle
        .offer_segment(committed(&["Second."]).remove(0))
        .unwrap();
    handle.end_utterance().unwrap();
    let after: Vec<SynthesisEvent> = handle.by_ref().take(100).collect();
    assert!(matches!(after.as_slice(), [SynthesisEvent::Usage(record)] if !record.r#final));
    assert_eq!(server.seen().len(), 1, "no connection after the failure");
}

#[test]
fn a_synthesis_driven_inside_an_async_runtime_runs_instead_of_panicking() {
    let server = FixtureServer::start(FORMAT, Script::Serve);
    let mut synthesiser = server.synthesiser(FORMAT);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let mut handle = synthesiser.start().unwrap();
    handle
        .offer_segment(committed(&["Inside a runtime."]).remove(0))
        .unwrap();
    handle.end_utterance().unwrap();
    let events: Vec<SynthesisEvent> =
        runtime.block_on(async { handle.by_ref().take(100).collect() });
    assert_eq!(
        handle.failure(),
        None,
        "story:elevenlabs-streaming-tts-adapter: inside a runtime the synthesis runs on a thread of its own"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, SynthesisEvent::Audio(_)))
            .count(),
        4,
        "story:elevenlabs-streaming-tts-adapter: every chunk is spoken inside a runtime"
    );
    assert!(
        matches!(events.last(), Some(SynthesisEvent::Usage(record)) if record.r#final),
        "story:elevenlabs-streaming-tts-adapter: the record of a synthesis inside a runtime is final"
    );
    runtime.block_on(async { drop(handle) });
}
