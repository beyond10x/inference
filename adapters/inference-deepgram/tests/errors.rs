//! Errors: an `Error` message, each documented close code, a malformed text message, a close
//! before the summary `Metadata` and a server silent after `CloseStream` each fail the
//! recognition with a typed error, never a panic or a hang, and the usage record that ends it is
//! not final.

mod support;

use std::time::{Duration, Instant};

use inference_deepgram::{CloseCause, DeepgramError, DeepgramRecognition};
use inference_stt::TranscriptEvent;
use inference_stt::inference_core::envelope::EventEnvelope;
use support::{FixtureServer, Script, describe, recognise, tone};

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
