//! Adversary pass 1 on the binding: stream end, transcript mapping and input edges over the
//! local fixture server.

mod support;

use std::time::{Duration, Instant};

use inference_stt::{Recogniser, RecognitionHandle, TranscriptEvent};
use support::{Action, FixtureServer, Script, Step, Trigger, describe, request, results, tone};

fn metadata(duration: f64) -> String {
    serde_json::json!({
        "type": "Metadata",
        "request_id": "fixture-request",
        "sha256": "fixture",
        "created": "2026-11-05T00:00:00.000Z",
        "duration": duration,
        "channels": 1
    })
    .to_string()
}

fn send(on: Trigger, text: String) -> Step {
    Step {
        on,
        action: Action::Send(text),
    }
}

/// The module documentation says a `Metadata` before `CloseStream` is not the summary and is
/// ignored. Here one arrives while the stream is open and the caller ends input before iterating
/// again: the binding reads it as the summary, ends the recognition, and the final result the
/// server sends after `CloseStream` is lost.
#[test]
fn adversary1_a_metadata_before_close_stream_does_not_end_the_recognition() {
    let server = FixtureServer::start(Script::Steps(vec![
        send(Trigger::Audio(1), metadata(0.01)),
        send(Trigger::CloseStream, results("late words", true, true)),
        send(Trigger::CloseStream, metadata(0.02)),
    ]));
    let mut recogniser = server.recogniser();
    let mut handle = recogniser.start(request()).unwrap();
    let mut events = Vec::new();
    handle.offer_frame(tone(1).remove(0)).unwrap();
    events.extend(handle.by_ref());
    std::thread::sleep(Duration::from_millis(500));
    handle.end_input().unwrap();
    events.extend(handle.by_ref().take(1_000));
    let kinds: Vec<String> = events.iter().map(describe).collect();
    assert_eq!(
        kinds,
        ["hypothesis late words", "commit", "usage"],
        "failure: {:?}",
        handle.failure()
    );
}

/// An `UtteranceEnd` commits the open interim span. When the endpoint then finalizes that same
/// span (same `start`), the binding opens a second span and commits the same words again.
#[test]
fn adversary1_a_final_result_after_an_utterance_end_does_not_commit_the_words_twice() {
    let server = FixtureServer::start(Script::Steps(vec![
        send(Trigger::Audio(1), results("good morning", false, false)),
        send(
            Trigger::Audio(2),
            r#"{"type":"UtteranceEnd","channel":[0,1],"last_word_end":1.2}"#.to_owned(),
        ),
        send(Trigger::Audio(3), results("good morning", true, false)),
        send(Trigger::CloseStream, metadata(0.03)),
    ]));
    let mut recogniser = server.recogniser();
    let (handle, events) = support::recognise(&mut recogniser, tone(4));
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
    let committed: Vec<String> = events
        .iter()
        .filter_map(|envelope| match &envelope.payload {
            TranscriptEvent::Committed(event) => Some(
                handle
                    .output()
                    .span(event.span_id)
                    .map(|span| span.text().to_owned())
                    .unwrap_or_default(),
            ),
            _ => None,
        })
        .collect();
    assert_eq!(committed, ["good morning"]);
}

#[test]
fn adversary1_a_recognition_with_no_audio_ends_at_once_with_its_usage_and_no_connection() {
    let server = FixtureServer::start(Script::Steps(Vec::new()));
    let mut recogniser = server.recogniser();
    let started = Instant::now();
    let (handle, events) = support::recognise(&mut recogniser, Vec::new());
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(events.iter().map(describe).collect::<Vec<_>>(), ["usage"]);
    assert!(handle.failure().is_none());
    assert!(server.seen().is_empty(), "no audio, no connection");
}

#[test]
fn adversary1_a_frame_of_zero_bytes_is_never_sent_as_an_empty_binary_message() {
    let server = FixtureServer::start(Script::Steps(vec![send(
        Trigger::CloseStream,
        metadata(0.0),
    )]));
    let mut recogniser = server.recogniser();
    let mut frame = tone(1).remove(0);
    frame.data.clear();
    frame.sample_count = 0;
    let (handle, events) = support::recognise(&mut recogniser, vec![frame]);
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
    assert_eq!(events.iter().map(describe).collect::<Vec<_>>(), ["usage"]);
    assert_eq!(server.violations(), Vec::<String>::new());
    assert_eq!(server.seen()[0].audio_messages, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn adversary1_a_recognition_iterated_inside_a_current_thread_runtime_completes() {
    let server = FixtureServer::start(Script::fixture("conversation.jsonl"));
    let mut recogniser = server.recogniser();
    let (handle, events) = support::recognise(&mut recogniser, tone(6));
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
    assert_eq!(events.iter().map(describe).last().as_deref(), Some("usage"));
}
