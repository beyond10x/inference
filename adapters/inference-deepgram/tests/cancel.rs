//! Cancel: a cancel mid-recognition closes the socket, and no event follows it, though the server
//! had more to send; a dropped handle closes its socket the same way.

mod support;

use std::time::{Duration, Instant};

use inference_stt::{Recogniser, RecognitionHandle};
use support::{FixtureServer, Script, describe, request, tone};

#[test]
fn a_cancel_closes_the_socket_and_nothing_follows() {
    let server = FixtureServer::start(Script::fixture("conversation.jsonl"));
    let mut recogniser = server.recogniser();
    let mut handle = recogniser.start(request()).unwrap();
    for frame in tone(3) {
        handle.offer_frame(frame).unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let first = loop {
        if let Some(event) = handle.next() {
            break event;
        }
        assert!(Instant::now() < deadline, "no hypothesis arrived");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(describe(&first), "hypothesis hello");
    handle.cancel();
    assert_eq!(
        server.next_client_close(Duration::from_secs(5)),
        Some(0),
        "the server sees the connection close"
    );
    assert!(server.seen()[0].client_closed);
    assert!(handle.next().is_none());
    handle.end_input().unwrap();
    assert!(handle.next().is_none(), "no usage record after a cancel");
    assert!(handle.failure().is_none());
    assert_eq!(server.seen()[0].close_streams, 0);
}

#[test]
fn a_dropped_handle_closes_its_socket_and_frees_its_slot() {
    let server = FixtureServer::start(Script::fixture("conversation.jsonl"));
    let mut recogniser = server.recogniser();
    let mut handle = recogniser.start(request()).unwrap();
    handle.offer_frame(tone(1).remove(0)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while handle.next().is_none() {
        assert!(
            Instant::now() < deadline,
            "no hypothesis arrived, so the socket never opened"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(recogniser.start(request()).is_err(), "one at a time");
    drop(handle);
    assert_eq!(server.next_client_close(Duration::from_secs(5)), Some(0));
    assert!(recogniser.start(request()).is_ok());
}
