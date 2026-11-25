//! Errors: an `Error` message, each documented close code, a malformed text message, a close
//! before the summary `Metadata`, a server silent after `CloseStream` and a handshake never
//! answered each fail the recognition with a typed error, never a panic or a hang, and the usage
//! record that ends it is not final.

mod support;

use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use inference_deepgram::{
    CloseCause, ConnectError, DeepgramError, DeepgramRecogniser, DeepgramRecognition, Endpoint,
    ListenConfig,
};
use inference_stt::TranscriptEvent;
use inference_stt::inference_core::envelope::EventEnvelope;
use support::{FixedKey, FixtureServer, KEY, Script, describe, recognise, tone};

/// The connect timeout given to the open whose handshake is never answered.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(300);

/// The longest that open may take; past it the test fails instead of waiting. Wide enough for a
/// loaded machine, and under the default connect timeout.
const CEILING: Duration = Duration::from_secs(4);

fn failed(script: Script) -> (DeepgramRecognition, Vec<EventEnvelope<TranscriptEvent>>) {
    let server = FixtureServer::start(script);
    let started = Instant::now();
    let (handle, events) = recognise(&mut server.recogniser(), tone(8));
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "the failure took {:?}",
        started.elapsed()
    );
    match events.last().map(|event| &event.payload) {
        Some(TranscriptEvent::Usage(record)) => assert!(!record.r#final, "a failed call's usage"),
        other => panic!("the last event is not the usage record: {other:?}"),
    }
    (handle, events)
}

fn closed(code: u16, reason: &str) -> DeepgramError {
    let (handle, _) = failed(Script::closes_after_first(code, reason));
    handle.failure().cloned().expect("a failure")
}

#[test]
fn an_error_message_is_a_server_error_with_its_code_and_description() {
    let (handle, events) = failed(Script::fixture("error.jsonl"));
    assert_eq!(
        handle.failure(),
        Some(&DeepgramError::Server {
            code: Some("INVALID_AUDIO".to_owned()),
            description: "the audio could not be processed".to_owned(),
        })
    );
    let words: Vec<String> = events.iter().map(describe).collect();
    assert_eq!(
        words,
        ["hypothesis hello", "usage"],
        "the open span stays open"
    );
}

#[test]
fn close_1008_data_0000_is_undecodable_audio() {
    assert_eq!(
        closed(1008, "DATA-0000"),
        DeepgramError::Closed {
            code: Some(1008),
            reason: "DATA-0000".to_owned(),
            cause: CloseCause::UndecodableAudio,
        }
    );
}

#[test]
fn close_1011_net_0000_is_a_server_timeout() {
    assert!(matches!(
        closed(1011, "NET-0000"),
        DeepgramError::Closed {
            code: Some(1011),
            cause: CloseCause::ServerTimeout,
            ..
        }
    ));
}

#[test]
fn close_1011_net_0001_is_no_frame_after_open() {
    assert!(matches!(
        closed(1011, "NET-0001"),
        DeepgramError::Closed {
            cause: CloseCause::NoFrameAfterOpen,
            ..
        }
    ));
}

#[test]
fn close_1011_net_0002_is_no_audio() {
    assert!(matches!(
        closed(1011, "NET-0002"),
        DeepgramError::Closed {
            cause: CloseCause::NoAudio,
            ..
        }
    ));
}

#[test]
fn a_text_message_that_is_not_json_is_a_protocol_error() {
    let (handle, events) = failed(Script::fixture("malformed.jsonl"));
    assert!(
        matches!(handle.failure(), Some(DeepgramError::Protocol(_))),
        "{:?}",
        handle.failure()
    );
    assert!(
        !format!("{:?}", handle.failure()).contains("this is not json"),
        "the error does not quote the message"
    );
    assert_eq!(events.len(), 1);
}

#[test]
fn a_socket_that_closes_before_the_metadata_is_closed() {
    let (handle, _) = failed(Script::fixture("no-metadata.jsonl"));
    assert!(
        matches!(
            handle.failure(),
            Some(DeepgramError::Closed {
                code: Some(1000),
                cause: CloseCause::Other,
                ..
            })
        ),
        "{:?}",
        handle.failure()
    );
}

#[test]
fn a_server_silent_after_close_stream_stalls_within_the_read_timeout() {
    let (handle, _) = failed(Script::fixture("silent-after-close.jsonl"));
    assert_eq!(
        handle.failure(),
        Some(&DeepgramError::Stalled(Duration::from_secs(3)))
    );
}

/// The listener takes the TCP connection into its queue and nothing reads it or answers the
/// upgrade, so only the connect timeout ends the open. A refused connection is the same variant
/// with no status: the detail and the time tell the two apart. The read timeout stays at its
/// default, so the handle's own wait for the summary ends later than [`CEILING`]. The wait is
/// the test's own, so an open that no connect timeout ends fails here at [`CEILING`].
#[test]
fn a_handshake_never_answered_fails_the_recognition_at_the_connect_timeout() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = Endpoint::new()
        .with_url(format!("ws://{}", listener.local_addr().unwrap()))
        .with_connect_timeout(CONNECT_TIMEOUT);
    let (done, waited) = mpsc::channel();
    std::thread::spawn(move || {
        let mut recogniser = DeepgramRecogniser::new(
            endpoint,
            ListenConfig::new(),
            Arc::new(FixedKey(KEY)),
            "deepgram-api-key",
        )
        .unwrap();
        let started = Instant::now();
        let (handle, _) = recognise(&mut recogniser, tone(2));
        let _ = done.send((handle.failure().cloned(), started.elapsed()));
    });
    let (failure, took) = waited
        .recv_timeout(CEILING)
        .unwrap_or_else(|error| panic!("the recognition did not end within {CEILING:?}: {error}"));
    assert_eq!(
        failure,
        Some(DeepgramError::Connect(ConnectError {
            status: None,
            detail: format!("no handshake within {CONNECT_TIMEOUT:?}"),
        }))
    );
    assert!(
        took >= CONNECT_TIMEOUT,
        "the recognition failed after {took:?}, before its connect timeout"
    );
    assert!(took < CEILING, "the recognition took {took:?}");
}
