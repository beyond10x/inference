//! Cancel on barge-in: a discard of the segment being spoken sends the close of its context, so
//! the server stops generating it, and the binding drops every later message of it. No frame of
//! the cancelled segment arrives after the discard, though the server had sent more, and the
//! connection stays open for the next segment. Dropping the handle closes the connection.

mod support;

use std::time::Duration;

use inference_elevenlabs::OutputFormat;
use inference_tts::inference_text::SegmentState;
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use support::{FixtureServer, Script, committed};

const FORMAT: OutputFormat = OutputFormat::Ulaw8000;

/// story:elevenlabs-tts-provider-cancel: after a cancel the server receives the close of the
/// cancelled context while it is still generating it, and so stops; the caller receives no frame
/// of it; and the next segment speaks on the same connection.
#[test]
fn a_cancel_closes_the_context_at_the_server_mid_generation_and_the_next_segment_speaks() {
    let server = FixtureServer::paced(
        FORMAT,
        vec![Script::Serve, Script::Serve],
        Duration::ZERO,
        Duration::from_millis(100),
    );
    let mut synthesiser = server.synthesiser(FORMAT);
    let mut handle = synthesiser.start().unwrap();
    let mut segments = committed(&["Barge in on me.", "After the barge-in."]).into_iter();
    let cancelled = segments.next().unwrap();
    let cancelled_id = cancelled.segment_id();
    handle.offer_segment(cancelled).unwrap();
    let Some(SynthesisEvent::Audio(first)) = handle.next() else {
        panic!(
            "no first frame of the segment to cancel: {:?}",
            handle.failure()
        );
    };
    assert_eq!(first.segment_id(), cancelled_id);
    handle.discard(cancelled_id).unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while server.seen()[0].closed.is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let seen = server.seen();
    assert_eq!(
        seen[0].closed,
        ["s0"],
        "the cancel sends the close of the cancelled context"
    );
    assert_eq!(
        seen[0].cut.len(),
        1,
        "the close reaches the server while it still generates the context: {:?}",
        seen[0].cut
    );
    let (cut, sent) = &seen[0].cut[0];
    assert_eq!(cut, "s0");
    assert!(
        (1..4).contains(sent),
        "the server sent {sent} of the context's 4 audio messages before the close"
    );

    let next = segments.next().unwrap();
    let next_id = next.segment_id();
    handle.offer_segment(next).unwrap();
    handle.end_utterance().unwrap();
    let after: Vec<SynthesisEvent> = handle.by_ref().collect();
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
    assert!(
        after
            .iter()
            .all(|event| event.segment_id() != Some(cancelled_id)),
        "a frame of the cancelled segment arrived after the cancel"
    );
    assert_eq!(handle.output().frames_emitted(cancelled_id), Some(1));
    assert_eq!(
        after
            .iter()
            .filter(|event| matches!(event, SynthesisEvent::Audio(_)))
            .count(),
        4,
        "the next segment is spoken whole"
    );
    assert_eq!(
        handle.output().segment(next_id).map(|s| s.state()),
        Some(SegmentState::Synthesized)
    );
    let seen = server.seen();
    assert_eq!(
        seen.len(),
        1,
        "the next segment speaks on the connection of the cancelled one"
    );
    assert_eq!(seen[0].contexts, ["s0", "s1"]);
    assert_eq!(seen[0].cut.len(), 1, "the next segment was not cut");
    assert_eq!(server.violations(), Vec::<String>::new());
}

#[test]
fn a_discard_mid_segment_keeps_the_socket_and_no_frame_of_it_follows() {
    let server = FixtureServer::scripted(FORMAT, vec![Script::NoFinal, Script::Serve]);
    let mut synthesiser = server.synthesiser(FORMAT);
    let mut handle = synthesiser.start().unwrap();
    let mut segments = committed(&["Barge in on me.", "After the barge-in."]).into_iter();
    let cancelled = segments.next().unwrap();
    let cancelled_id = cancelled.segment_id();
    handle.offer_segment(cancelled).unwrap();

    let Some(SynthesisEvent::Audio(first)) = handle.next() else {
        panic!("no first frame: {:?}", handle.failure());
    };
    assert_eq!(first.segment_id(), cancelled_id);
    handle.discard(cancelled_id).unwrap();
    assert_eq!(
        server.next_client_close(Duration::from_millis(300)),
        None,
        "story:elevenlabs-multi-context-tts-connection: a cancel ends the context, not the connection"
    );
    assert!(!server.seen()[0].client_closed);

    let next = segments.next().unwrap();
    let next_id = next.segment_id();
    handle.offer_segment(next).unwrap();
    handle.end_utterance().unwrap();
    let after: Vec<SynthesisEvent> = handle.by_ref().collect();
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
    assert!(
        after
            .iter()
            .all(|event| event.segment_id() != Some(cancelled_id)),
        "a frame of the cancelled segment arrived after the cancel"
    );
    assert_eq!(handle.output().frames_emitted(cancelled_id), Some(1));
    assert_eq!(
        handle.output().segment(cancelled_id).map(|s| s.state()),
        Some(SegmentState::Discarded)
    );
    assert_eq!(
        handle.output().segment(next_id).map(|s| s.state()),
        Some(SegmentState::Synthesized),
        "the next segment is spoken"
    );
    assert_eq!(
        server.seen().len(),
        1,
        "story:elevenlabs-multi-context-tts-connection: the next segment speaks on the same connection"
    );
    assert_eq!(server.seen()[0].contexts, ["s0", "s1"]);
    assert_eq!(
        server.next_client_close(Duration::from_secs(5)),
        Some(0),
        "the end of the utterance closes the connection"
    );
    assert!(
        server.seen()[0].close_socket,
        "the utterance ends with close_socket"
    );
    let positions: Vec<u64> = after
        .iter()
        .filter_map(|event| match event {
            SynthesisEvent::Audio(audio) => Some(audio.frame().position.get()),
            _ => None,
        })
        .collect();
    assert_eq!(
        positions,
        [400, 800, 1_200, 1_600],
        "the clock goes on past the cancel"
    );
    assert!(matches!(after.last(), Some(SynthesisEvent::Usage(_))));
    assert_eq!(server.violations(), Vec::<String>::new());
}

#[test]
fn dropping_the_handle_mid_segment_closes_the_socket() {
    let server = FixtureServer::start(FORMAT, Script::NoFinal);
    let mut synthesiser = server.synthesiser(FORMAT);
    let mut handle = synthesiser.start().unwrap();
    handle
        .offer_segment(committed(&["Speak and be dropped."]).remove(0))
        .unwrap();
    assert!(matches!(handle.next(), Some(SynthesisEvent::Audio(_))));
    drop(handle);
    assert_eq!(server.next_client_close(Duration::from_secs(5)), Some(0));
    assert!(
        synthesiser.start().is_ok(),
        "the dropped handle gave its slot back"
    );
}

#[test]
fn a_cancel_does_not_wait_for_the_server() {
    let server = FixtureServer::start(FORMAT, Script::NoFinal);
    let mut synthesiser = server.synthesiser(FORMAT);
    let mut handle = synthesiser.start().unwrap();
    let segment = committed(&["Cut me short."]).remove(0);
    let id = segment.segment_id();
    handle.offer_segment(segment).unwrap();
    assert!(matches!(handle.next(), Some(SynthesisEvent::Audio(_))));
    let started = std::time::Instant::now();
    handle.discard(id).unwrap();
    assert!(
        started.elapsed() <= inference_elevenlabs::CLOSE_BOUND,
        "the cancel took {:?}",
        started.elapsed()
    );
    assert_eq!(handle.next(), None, "nothing more of the cancelled segment");
}
