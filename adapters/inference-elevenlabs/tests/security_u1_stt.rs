//! Conformance checks of the realtime speech-to-text binding against the invariants its crate
//! documentation states: a bounded handshake whose outcome the recognition reports, bounded server
//! messages, never a panic, and span timings that lie in the audio sent.
#![cfg(feature = "stt")]

mod stt_support;

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use inference_elevenlabs::stt_protocol::{ServerMessage, parse_server_message};
use inference_elevenlabs::{ElevenLabsError, ElevenLabsRecogniser, Endpoint, InputFormat};
use inference_stt::TranscriptEvent;
use serde_json::json;
use stt_support::{
    Action, FixedKey, FixtureServer, KEY, Script, Step, Trigger, describe, manual, mulaw,
    recognise, session_started,
};

/// The largest message the crate's Deepgram recogniser reads (`MAX_MESSAGE_SIZE`, 4 MiB); the
/// crate's own agent binding reads at most 1 MiB.
const MODEL_ADAPTER_BOUND: usize = 4 << 20;

fn is_final(
    events: &[inference_stt::inference_core::envelope::EventEnvelope<TranscriptEvent>],
) -> bool {
    match events.last().map(|event| &event.payload) {
        Some(TranscriptEvent::Usage(record)) => record.r#final,
        other => panic!("the last event is not the usage record: {other:?}"),
    }
}

/// The handshake is bounded by the connect timeout and a refused or unanswered one fails the
/// recognition (`ElevenLabsError::Connect`). When input ends while the handshake is still
/// pending and the read timeout is the shorter bound, the recognition must still report the
/// handshake's outcome, not end as a clean, final recognition of audio that never left.
#[test]
fn a_handshake_still_pending_when_the_read_timeout_after_input_passes_fails_the_recognition() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let held = std::thread::spawn(move || {
        let accepted = listener.accept();
        std::thread::sleep(Duration::from_secs(4));
        drop(accepted);
    });
    let mut recogniser = ElevenLabsRecogniser::new(
        Endpoint::new()
            .with_url(url)
            .with_connect_timeout(Duration::from_secs(2))
            .with_read_timeout(Duration::from_millis(300)),
        manual(InputFormat::Ulaw8000),
        Arc::new(FixedKey(KEY)),
        "elevenlabs-api-key",
    )
    .unwrap();
    let (handle, events) = recognise(&mut recogniser, mulaw(3));
    let failure = handle.failure().cloned();
    let final_record = is_final(&events);
    drop(handle);
    held.join().unwrap();
    assert!(
        matches!(failure, Some(ElevenLabsError::Connect(_))),
        "the handshake was never answered, yet the recognition ended with failure {failure:?}"
    );
    assert!(
        !final_record,
        "a recognition whose audio never left must not carry a final usage record"
    );
}

/// A hostile or broken server must not make the binding hold an unbounded message: the crate's
/// other WebSocket recogniser refuses anything over 4 MiB.
#[test]
fn a_server_message_beyond_the_bound_fails_the_recognition_instead_of_being_delivered() {
    let oversized = json!({
        "message_type": "partial_transcript",
        "text": "a".repeat(MODEL_ADAPTER_BOUND + 1024),
    })
    .to_string();
    let server = FixtureServer::start(Script::sends_after_first(&oversized));
    let mut recogniser = server.recogniser(manual(InputFormat::Ulaw8000));
    let (handle, events) = recognise(&mut recogniser, mulaw(3));
    let longest = events
        .iter()
        .map(|event| match &event.payload {
            TranscriptEvent::Hypothesis(event) => event.text.len(),
            TranscriptEvent::Revision(event) => event.text.len(),
            TranscriptEvent::Committed(event) => event.text.len(),
            TranscriptEvent::Usage(_) => 0,
        })
        .max()
        .unwrap_or(0);
    assert!(
        longest <= MODEL_ADAPTER_BOUND,
        "a {longest}-byte transcript was delivered from a single server message; failure {:?}",
        handle.failure()
    );
    assert!(
        matches!(
            handle.failure(),
            Some(ElevenLabsError::Transport(_) | ElevenLabsError::Protocol(_))
        ),
        "{:?}",
        handle.failure()
    );
}

/// "never a panic or a hang": the TTS connection of the same crate takes a read timeout too large
/// for an instant as no deadline (`Connection::deadline`, `checked_add`); a recognition with such
/// an endpoint must not panic on the caller's thread.
#[test]
fn a_read_timeout_too_large_for_an_instant_does_not_panic_the_caller() {
    let server = FixtureServer::start(Script::Steps(vec![
        Step {
            on: Trigger::Open,
            action: Action::Send(session_started()),
        },
        Step {
            on: Trigger::Commit(1),
            action: Action::Send(
                json!({
                    "message_type": "committed_transcript_with_timestamps",
                    "text": "eins",
                    "words": [{"text": "eins", "start": 0.0, "end": 0.04, "type": "word"}]
                })
                .to_string(),
            ),
        },
    ]));
    let endpoint = server.endpoint().with_read_timeout(Duration::MAX);
    let (done, outcome) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut recogniser = ElevenLabsRecogniser::new(
            endpoint,
            manual(InputFormat::Ulaw8000),
            Arc::new(FixedKey(KEY)),
            "elevenlabs-api-key",
        )
        .unwrap();
        let (handle, events) = recognise(&mut recogniser, mulaw(3));
        let _ = done.send((
            handle.failure().cloned(),
            events.iter().map(describe).collect::<Vec<_>>(),
        ));
    });
    match outcome.recv_timeout(Duration::from_secs(10)) {
        Ok((failure, events)) => {
            assert_eq!(failure, None, "{events:?}");
            assert_eq!(events, ["hypothesis eins", "commit eins", "usage"]);
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            let panic = worker
                .join()
                .expect_err("the thread ended without an outcome");
            let message = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
                .unwrap_or_default();
            panic!("the recognition panicked on the caller's thread: {message}");
        }
        Err(mpsc::RecvTimeoutError::Timeout) => panic!("the recognition hung"),
    }
    drop(server);
}

/// `SpanTiming` is "where a committed span lies in the audio sent, in seconds from the first
/// sample sent". A word timing from the server that ends before it starts, or starts before the
/// first sample, does not lie in that audio and must not become a span's timing.
#[test]
fn a_committed_span_timing_never_ends_before_it_starts_or_starts_before_the_audio() {
    for words in [
        json!([{"text": "a", "start": 2.0, "end": 1.0, "type": "word"}]),
        json!([{"text": "a", "start": -3.0, "end": 0.5, "type": "word"}]),
    ] {
        let message = json!({
            "message_type": "committed_transcript_with_timestamps",
            "text": "a",
            "words": words,
        })
        .to_string();
        match parse_server_message(&message) {
            Err(ElevenLabsError::Protocol(_)) => {}
            Ok(ServerMessage::CommittedWithTimestamps { timing, .. }) => {
                assert!(
                    timing.is_none_or(|timing| timing.start >= 0.0 && timing.start <= timing.end),
                    "{words}: timing {timing:?} does not lie in the audio sent"
                );
            }
            other => panic!("{words}: {other:?}"),
        }
    }
}
