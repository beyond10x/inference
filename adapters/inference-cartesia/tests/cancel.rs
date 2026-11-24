//! Cancel on barge-in, mid-utterance: a discard of the segment being spoken sends the protocol's
//! cancel for its context. The fixture server goes on sending that context's audio, as a context
//! that had begun generating does, and no frame of it reaches the caller; the next segment is
//! spoken on the same connection.
//!
//! And the drop of the handle: mid-segment it ends the connection, and idle inside an async
//! runtime it holds the runtime's thread for less than the ticker's bound, beside the ticker and
//! through the relay of the ElevenLabs binding's tests, which this file shares.

mod support;

#[path = "../../inference-elevenlabs/tests/support/dropped.rs"]
mod dropped;
#[path = "../../inference-elevenlabs/tests/support/ticker.rs"]
mod ticker;

use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dropped::{CLOSE, Relay, TEXT, threads_named};
use inference_cartesia::{CLOSE_BOUND, CartesiaSynthesiser, OutputFormat, VoiceConfig};
use inference_tts::inference_text::SegmentState;
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use support::{FixtureServer, Script, committed, within};
use ticker::{GAP_BOUND, with_ticker};

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

/// What a session awaits before and after it drops its handle, so the ticker runs on both sides.
const LEAD: Duration = Duration::from_millis(35);

/// A synthesiser of the fixture voice at `relay`, which passes on to `server`.
fn through(relay: &Relay, server: &FixtureServer) -> CartesiaSynthesiser {
    CartesiaSynthesiser::new(
        server.endpoint().with_url(&relay.url),
        VoiceConfig::new(support::VOICE, FORMAT),
        Arc::new(support::FixedKey(support::KEY)),
        "cartesia-api-key",
    )
    .expect("the fixture configuration is valid")
}

/// The kernel's name of a synthesis's own thread: the first 15 bytes of `inference-cartesia`.
const OWN_THREAD: &str = "inference-carte";

/// One synthesis of `synthesiser` that speaks a segment and is then idle, its utterance open
/// and its connection with it, and is dropped there on the runtime's thread: how long the drop
/// held that thread, and how many threads of its own the synthesis had before.
async fn drop_idle(mut synthesiser: CartesiaSynthesiser) -> (Duration, Option<usize>) {
    let mut handle = synthesiser
        .start()
        .expect("a fresh synthesiser runs nothing");
    handle
        .offer_segment(committed(&["Speak, then idle."]).remove(0))
        .expect("the utterance is open");
    let mut completed = false;
    while let Some(event) = handle.event().await {
        completed |= matches!(event, SynthesisEvent::Completed(_));
    }
    assert!(completed, "the segment completes");
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
    let own = threads_named(OWN_THREAD, 1, Duration::ZERO);
    tokio::time::sleep(LEAD).await;
    let started = Instant::now();
    drop(handle);
    let held = started.elapsed();
    tokio::time::sleep(LEAD).await;
    (held, own)
}

/// The drop of an idle synthesis inside a runtime: it holds the runtime's thread for less than
/// the ticker's bound, the server gets a close frame, which the relay tells from a socket that
/// only ended, and the synthesis's own thread ends after it. How the socket ends after the close
/// frame is not asserted. This fixture server sends no close frame in answer, it stops reading
/// at the client's and lets its socket go; a server that answers makes the socket end with a
/// reset when its frame reaches the client before the client lets go of its socket.
#[test]
fn dropping_an_idle_synthesis_does_not_block_the_runtime() {
    within(|| {
        let server = FixtureServer::start(FORMAT, Script::Serve);
        let relay = Relay::to(&server.url);
        let (dropped, largest) = with_ticker(vec![drop_idle(through(&relay, &server))]);
        let (held, own) = dropped[0];
        println!("the drop held the thread {held:?}, the ticker's largest gap is {largest:?}");
        assert!(largest < GAP_BOUND, "the ticker's largest gap {largest:?}");
        assert!(
            matches!(own, None | Some(1)),
            "an idle synthesis has one thread of its own: {own:?}"
        );
        let side = relay
            .next_client_side(Duration::from_secs(5))
            .expect("the client's side of the connection ends");
        assert_eq!(
            side.opcodes,
            [TEXT, CLOSE],
            "the request, then a close frame, are what the client sent: {side:?}"
        );
        assert_eq!(
            side.unframed, 0,
            "nothing follows the close frame: {side:?}"
        );
        assert_eq!(server.next_client_close(Duration::from_secs(5)), Some(0));
        assert_eq!(server.violations(), Vec::<String>::new());
        let left = threads_named(OWN_THREAD, 0, Duration::from_secs(5));
        assert!(
            matches!(left, None | Some(0)),
            "the synthesis's own thread ends after its close: {left:?}"
        );
    });
}

/// One synthesis of `synthesiser`, whose server answers nothing, dropped on the runtime's thread
/// while its async form waits for the first message of its segment.
async fn drop_waiting(mut synthesiser: CartesiaSynthesiser, server: Rc<FixtureServer>) {
    let mut handle = synthesiser
        .start()
        .expect("a fresh synthesiser runs nothing");
    handle
        .offer_segment(committed(&["Speak into silence."]).remove(0))
        .expect("the utterance is open");
    let mut polls = 0;
    while polls == 0
        || server
            .seen()
            .first()
            .is_none_or(|seen| seen.requests.is_empty())
    {
        let waited = tokio::time::timeout(Duration::from_millis(10), handle.event()).await;
        assert!(waited.is_err(), "the server answers nothing: {waited:?}");
        polls += 1;
    }
    tokio::time::sleep(LEAD).await;
    drop(handle);
    tokio::time::sleep(LEAD).await;
}

/// The drop of a synthesis inside a runtime while a read is in flight: it does not hold the
/// runtime's thread either, and the server gets no close frame, only the end of the socket.
#[test]
fn dropping_a_synthesis_that_waits_for_the_server_ends_the_socket_without_a_close_frame() {
    within(|| {
        let server = Rc::new(FixtureServer::start(FORMAT, Script::Silent));
        let relay = Relay::to(&server.url);
        let session = drop_waiting(through(&relay, &server), Rc::clone(&server));
        let (_, largest) = with_ticker(vec![session]);
        assert!(largest < GAP_BOUND, "the ticker's largest gap {largest:?}");
        let side = relay
            .next_client_side(Duration::from_secs(5))
            .expect("the client's side of the connection ends");
        assert_eq!(
            (side.opcodes.as_slice(), side.unframed),
            ([TEXT].as_slice(), 0),
            "the request and no close frame are what the client sent: {side:?}"
        );
        assert_eq!(server.next_client_close(Duration::from_secs(5)), Some(0));
        assert_eq!(server.violations(), Vec::<String>::new());
    });
}
