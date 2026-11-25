//! Errors: an `Error` message, each documented close code, a malformed text message, a close
//! before the summary `Metadata`, a server silent after `CloseStream` and a handshake never
//! answered each fail the recognition with a typed error, never a panic or a hang, and the usage
//! record that ends it is not final.

mod support;

use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use inference_deepgram::{
    CLOSE_BOUND, CloseCause, ConnectError, DEFAULT_CONNECT_TIMEOUT, DEFAULT_KEEPALIVE_INTERVAL,
    DEFAULT_READ_TIMEOUT, DeepgramError, DeepgramRecogniser, DeepgramRecognition, Endpoint,
    ListenConfig,
};
use inference_stt::TranscriptEvent;
use inference_stt::inference_core::envelope::EventEnvelope;
use support::{FixedKey, FixtureServer, KEY, Script, describe, recognise, tone};

/// The connect timeout given to the open whose handshake is never answered.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);

/// That open must end before twice its connect timeout: a bound doubled or more fails the test,
/// a bound multiplied by less than two still passes. The 500 ms past the bound are room for a
/// loaded machine: over repeated runs on one, such an open ended at most 51 ms after its bound,
/// and tests that measure stalls saw one of 204 ms there.
const LATEST: Duration = CONNECT_TIMEOUT.saturating_mul(2);

/// The test's own deadline for an open that nothing ends: past it the test fails instead of
/// waiting.
const DEADLINE: Duration = Duration::from_secs(4);

/// How many connections wait in the queue of `listener`, which nothing accepted before. An
/// attempt is there from the moment its TCP connect completed, so the count needs no wait.
fn attempts(listener: &std::net::TcpListener) -> usize {
    listener.set_nonblocking(true).unwrap();
    let mut count = 0;
    loop {
        match listener.accept() {
            Ok(_) => count += 1,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return count,
            Err(error) => panic!("the listener failed: {error}"),
        }
    }
}

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
/// with no status: the detail and the time tell the two apart. The open connects once: a second
/// attempt after the timeout is a second connection in the listener's queue. The read timeout
/// stays at its default, so the handle's own wait for the summary ends later than [`DEADLINE`].
/// The wait is the test's own, so an open that no connect timeout ends fails here at
/// [`DEADLINE`].
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
        .recv_timeout(DEADLINE)
        .unwrap_or_else(|error| panic!("the recognition did not end within {DEADLINE:?}: {error}"));
    assert_eq!(
        failure,
        Some(DeepgramError::Connect(ConnectError {
            status: None,
            detail: format!("no handshake within {CONNECT_TIMEOUT:?}"),
        }))
    );
    assert_eq!(
        attempts(&listener),
        1,
        "the connections the recognition made"
    );
    assert!(
        took >= CONNECT_TIMEOUT,
        "the recognition failed after {took:?}, before its connect timeout"
    );
    assert!(
        took < LATEST,
        "the recognition failed after {took:?}, at twice its connect timeout or later"
    );
}

/// The bounds of the endpoint a caller gets from `Endpoint::new()`, the keep-alive interval of
/// `ListenConfig::new()`, their constants and the close bound, each compared with its number
/// written out: a changed default fails here. Outside the source, `quality/adapters.md`, entry
/// `deepgram-time-bounds/held`, states the read timeout (10 s), the keep-alive interval (4 s)
/// and the close bound (1 s). No document states the connect timeout: for it the number below is
/// the reference.
#[test]
fn the_time_bound_defaults_are_the_stated_numbers() {
    let endpoint = Endpoint::new();
    let defaults = [
        (
            "DEFAULT_CONNECT_TIMEOUT",
            DEFAULT_CONNECT_TIMEOUT,
            Duration::from_secs(10),
        ),
        (
            "Endpoint::new().connect_timeout",
            endpoint.connect_timeout,
            Duration::from_secs(10),
        ),
        (
            "DEFAULT_READ_TIMEOUT",
            DEFAULT_READ_TIMEOUT,
            Duration::from_secs(10),
        ),
        (
            "Endpoint::new().read_timeout",
            endpoint.read_timeout,
            Duration::from_secs(10),
        ),
        (
            "DEFAULT_KEEPALIVE_INTERVAL",
            DEFAULT_KEEPALIVE_INTERVAL,
            Duration::from_secs(4),
        ),
        (
            "ListenConfig::new().keepalive_interval",
            ListenConfig::new().keepalive_interval,
            Duration::from_secs(4),
        ),
        ("CLOSE_BOUND", CLOSE_BOUND, Duration::from_secs(1)),
    ];
    let changed: Vec<_> = defaults
        .iter()
        .filter(|(_, default, stated)| default != stated)
        .collect();
    assert!(
        changed.is_empty(),
        "each default that is not its stated number, as (name, default, stated): {changed:?}"
    );
}
