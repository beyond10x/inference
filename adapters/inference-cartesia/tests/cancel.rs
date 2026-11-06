//! Cancel on barge-in, mid-utterance: a discard of the segment being spoken sends the protocol's
//! cancel for its context. The fixture server goes on sending that context's audio, as a context
//! that had begun generating does, and no frame of it reaches the caller; the next segment is
//! spoken on the same connection.

mod support;

use std::time::{Duration, Instant};

use inference_cartesia::{CLOSE_BOUND, OutputFormat};
use inference_tts::inference_text::SegmentState;
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use support::{FixtureServer, Script, committed, within};

const FORMAT: OutputFormat = OutputFormat::MULAW_8000;

#[test]
fn a_discard_mid_segment_cancels_its_context_and_no_frame_of_it_follows() {
    within(|| {
        let server = FixtureServer::scripted(FORMAT, vec![Script::HoldAfterFirst, Script::Serve]);
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
        let context = server
            .next_cancel(Duration::from_secs(5))
            .expect("the server reads a cancel");
        assert_eq!(
            server.seen()[0].requests[0]["context_id"].as_str(),
            Some(context.as_str()),
            "the cancel names the context of the discarded segment"
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
            handle.output().segment(cancelled_id).map(|s| s.state()),
            Some(SegmentState::Discarded)
        );
        assert_eq!(
            handle.output().segment(next_id).map(|s| s.state()),
            Some(SegmentState::Synthesized)
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
            [401, 802, 1_202, 1_602],
            "the clock goes on past the cancel"
        );
        assert!(matches!(after.last(), Some(SynthesisEvent::Usage(_))));
        let seen = server.seen();
        assert_eq!(
            seen.len(),
            1,
            "the next segment is spoken on the same connection"
        );
        assert_eq!(seen[0].requests.len(), 2);
        assert_eq!(seen[0].cancels, [context]);
        assert!(
            !seen[0].client_closed,
            "a cancel does not close the connection"
        );
        assert_eq!(server.violations(), Vec::<String>::new());
    });
}

#[test]
fn dropping_the_handle_mid_segment_closes_the_socket() {
    within(|| {
        let server = FixtureServer::start(FORMAT, Script::HoldAfterFirst);
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
    });
}

#[test]
fn a_cancel_does_not_wait_for_the_server() {
    within(|| {
        let server = FixtureServer::start(FORMAT, Script::HoldAfterFirst);
        let mut synthesiser = server.synthesiser(FORMAT);
        let mut handle = synthesiser.start().unwrap();
        let segment = committed(&["Cut me short."]).remove(0);
        let id = segment.segment_id();
        handle.offer_segment(segment).unwrap();
        assert!(matches!(handle.next(), Some(SynthesisEvent::Audio(_))));
        let started = Instant::now();
        handle.discard(id).unwrap();
        assert!(
            started.elapsed() <= CLOSE_BOUND,
            "the cancel took {:?}",
            started.elapsed()
        );
        assert_eq!(handle.next(), None, "nothing more of the cancelled segment");
    });
}
