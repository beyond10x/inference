//! Bounds on what the server sends: a message past the size bound fails the recognition typed,
//! many messages the caller has not taken arrive whole and in order through the bounded queue,
//! and messages trickling in after `CloseStream` do not stretch the one deadline of the summary.

mod support;

use std::time::{Duration, Instant};

use inference_deepgram::DeepgramError;
use inference_deepgram::listen::{INBOUND_DEPTH, MAX_MESSAGE_SIZE};
use inference_stt::{Recogniser, RecognitionHandle};
use support::{Action, FixtureServer, Script, Step, Trigger, describe, request, results, tone};

const SUMMARY: &str = r#"{"type":"Metadata","duration":0.1}"#;

fn send(on: Trigger, text: String) -> Step {
    Step {
        on,
        action: Action::Send(text),
    }
}

#[test]
fn a_message_past_the_size_bound_is_a_transport_failure() {
    let server = FixtureServer::start(Script::Steps(vec![
        send(
            Trigger::Audio(1),
            results(&"a".repeat(MAX_MESSAGE_SIZE + 1), false, false),
        ),
        send(Trigger::CloseStream, SUMMARY.to_owned()),
    ]));
    let started = Instant::now();
    let (handle, events) = support::recognise(&mut server.recogniser(), tone(2));
    assert!(started.elapsed() < Duration::from_secs(6));
    assert!(
        matches!(handle.failure(), Some(DeepgramError::Transport(_))),
        "{:?}",
        handle.failure()
    );
    assert_eq!(events.iter().map(describe).collect::<Vec<_>>(), ["usage"]);
}

#[test]
fn messages_the_caller_has_not_taken_arrive_whole_and_in_order_through_the_bounded_queue() {
    let count = INBOUND_DEPTH * 16;
    let mut steps: Vec<Step> = (0..count)
        .map(|n| send(Trigger::Audio(1), results(&format!("w{n}"), false, false)))
        .collect();
    steps.push(send(Trigger::CloseStream, SUMMARY.to_owned()));
    let server = FixtureServer::start(Script::Steps(steps));
    let mut recogniser = server.recogniser();
    let mut handle = recogniser.start(request()).unwrap();
    handle.offer_frame(tone(1).remove(0)).unwrap();
    let mut events: Vec<String> = handle.by_ref().map(|event| describe(&event)).collect();
    std::thread::sleep(Duration::from_millis(500));
    handle.end_input().unwrap();
    events.extend(
        handle
            .by_ref()
            .take(count * 2)
            .map(|event| describe(&event)),
    );
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
    let mut expected = vec!["hypothesis w0".to_owned()];
    expected.extend((1..count).map(|n| format!("revision w{n}")));
    expected.extend(["commit".to_owned(), "usage".to_owned()]);
    assert_eq!(events, expected);
}

#[test]
fn messages_trickling_in_after_close_stream_do_not_stretch_the_summary_deadline() {
    let mut steps = Vec::new();
    for n in 0..20 {
        steps.push(send(
            Trigger::CloseStream,
            results(&format!("w{n}"), false, false),
        ));
        steps.push(Step {
            on: Trigger::CloseStream,
            action: Action::Wait(Duration::from_millis(400)),
        });
    }
    steps.push(send(Trigger::CloseStream, SUMMARY.to_owned()));
    let server = FixtureServer::start(Script::Steps(steps));
    let mut recogniser = server.recogniser();
    let mut handle = recogniser.start(request()).unwrap();
    handle.offer_frame(tone(1).remove(0)).unwrap();
    let _ = handle.by_ref().count();
    handle.end_input().unwrap();
    let ended = Instant::now();
    let _ = handle.by_ref().take(1_000).count();
    let waited = ended.elapsed();
    let read_timeout = server.endpoint().read_timeout;
    assert!(
        matches!(handle.failure(), Some(DeepgramError::Stalled(_))),
        "{:?}",
        handle.failure()
    );
    assert!(
        waited < read_timeout + Duration::from_millis(1_500),
        "the end took {waited:?}, past the read timeout {read_timeout:?}"
    );
}
