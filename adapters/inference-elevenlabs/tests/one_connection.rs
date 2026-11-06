//! One connection per synthesis: the segments of an utterance are spoken as contexts of one
//! multi-stream connection, so the segments after the first pay no handshake, and a barge-in
//! closes the context being spoken without closing the connection.

mod support;

use std::time::Duration;

use inference_elevenlabs::OutputFormat;
use inference_tts::inference_text::SegmentState;
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use support::{FixtureServer, Script, committed, within};

const FORMAT: OutputFormat = OutputFormat::Ulaw8000;
const DEADLINE: Duration = Duration::from_secs(20);

#[test]
fn three_segments_are_spoken_on_one_connection() {
    let server = FixtureServer::start(FORMAT, Script::Serve);
    let mut synthesiser = server.synthesiser(FORMAT);
    let (failure, frames, states) = within(DEADLINE, move || {
        let mut handle = synthesiser.start().unwrap();
        let segments = committed(&["One.", "Two.", "Three."]);
        let ids: Vec<_> = segments
            .iter()
            .map(|segment| segment.segment_id())
            .collect();
        for segment in segments {
            handle.offer_segment(segment).unwrap();
        }
        handle.end_utterance().unwrap();
        let events: Vec<SynthesisEvent> = handle.by_ref().take(100).collect();
        let frames: Vec<usize> = ids
            .iter()
            .map(|id| {
                events
                    .iter()
                    .filter(|event| {
                        matches!(event, SynthesisEvent::Audio(audio) if audio.segment_id() == *id)
                    })
                    .count()
            })
            .collect();
        let states: Vec<_> = ids
            .iter()
            .map(|id| handle.output().segment(*id).map(|segment| segment.state()))
            .collect();
        (handle.failure().cloned(), frames, states)
    });
    assert_eq!(
        server.seen().len(),
        1,
        "three segments open one connection, against one per segment before"
    );
    assert_eq!(failure, None);
    assert_eq!(frames, [4, 4, 4], "every segment is spoken whole");
    assert_eq!(states, [Some(SegmentState::Synthesized); 3]);
    assert_eq!(server.violations(), Vec::<String>::new());
}

#[test]
fn a_cancel_mid_segment_yields_no_further_frame_and_the_next_segment_speaks_on_the_same_connection()
{
    let server = FixtureServer::scripted(FORMAT, vec![Script::NoFinal, Script::Serve]);
    let mut synthesiser = server.synthesiser(FORMAT);
    let (failure, after, cancelled, next) = within(DEADLINE, move || {
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
        let next = segments.next().unwrap();
        let next_id = next.segment_id();
        handle.offer_segment(next).unwrap();
        handle.end_utterance().unwrap();
        let after: Vec<SynthesisEvent> = handle.by_ref().take(100).collect();
        let cancelled = (
            after
                .iter()
                .filter(|event| event.segment_id() == Some(cancelled_id))
                .count(),
            handle.output().frames_emitted(cancelled_id),
            handle.output().segment(cancelled_id).map(|s| s.state()),
        );
        let next = (
            after
                .iter()
                .filter(|event| {
                    matches!(event, SynthesisEvent::Audio(audio) if audio.segment_id() == next_id)
                })
                .count(),
            handle.output().segment(next_id).map(|s| s.state()),
        );
        (handle.failure().cloned(), after, cancelled, next)
    });
    assert_eq!(failure, None);
    assert_eq!(
        cancelled,
        (0, Some(1), Some(SegmentState::Discarded)),
        "no frame of the cancelled segment follows the cancel"
    );
    assert_eq!(next, (4, Some(SegmentState::Synthesized)));
    assert!(matches!(after.last(), Some(SynthesisEvent::Usage(record)) if record.r#final));
    assert_eq!(
        server.seen().len(),
        1,
        "the next segment speaks on the connection of the cancelled one"
    );
    assert_eq!(server.violations(), Vec::<String>::new());
}
