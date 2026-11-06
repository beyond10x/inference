//! Cancel mid-utterance: the speech endpoint has no cancel message, so a discard of the segment
//! being spoken drops its response and closes its connection. The fixture server, which holds
//! the response open, sees the close; no frame of the cancelled segment arrives after it; and
//! the utterance still yields one usage record, with the audio emitted before the cancel.

mod speech_support;

use std::time::{Duration, Instant};

use inference_tts::inference_audio::AudioEncoding;
use inference_tts::inference_core::usage::QuantityKind;
use inference_tts::inference_text::SegmentState;
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use speech_support::{FixtureServer, Script, committed, format, within_deadline};

#[test]
fn a_cancel_mid_utterance_yields_no_further_frame_and_one_usage_record() {
    within_deadline(|| {
        let server = FixtureServer::scripted(vec![Script::Stall, Script::Serve]);
        let wanted = format(AudioEncoding::PcmMulaw, 8_000);
        let mut synthesiser = server.synthesiser(wanted);
        let mut handle = synthesiser.start().unwrap();
        let mut segments = committed(&["Barge in on me.", "Never spoken."]).into_iter();
        let cancelled = segments.next().unwrap();
        let cancelled_id = cancelled.segment_id();
        handle.offer_segment(cancelled).unwrap();

        let Some(SynthesisEvent::Audio(first)) = handle.next() else {
            panic!("no first frame: {:?}", handle.failure());
        };
        assert_eq!(first.segment_id(), cancelled_id);
        assert_eq!(first.frame().format, wanted);
        let started = Instant::now();
        handle.discard(cancelled_id).unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the cancel waited {:?}",
            started.elapsed()
        );
        assert_eq!(
            server.next_client_close(Duration::from_secs(5)),
            Some(0),
            "the server sees the cancelled response's connection close"
        );

        let never = segments.next().unwrap();
        let never_id = never.segment_id();
        handle.offer_segment(never).unwrap();
        handle.discard(never_id).unwrap();
        handle.end_utterance().unwrap();
        let after: Vec<SynthesisEvent> = handle.by_ref().collect();
        assert!(handle.failure().is_none(), "{:?}", handle.failure());
        assert!(
            after
                .iter()
                .all(|event| !matches!(event, SynthesisEvent::Audio(_))),
            "a frame arrived after the cancel: {after:?}"
        );
        assert_eq!(handle.output().frames_emitted(cancelled_id), Some(1));
        assert_eq!(
            handle.output().segment(cancelled_id).map(|s| s.state()),
            Some(SegmentState::Discarded)
        );
        let usage: Vec<_> = after
            .iter()
            .filter_map(|event| match event {
                SynthesisEvent::Usage(usage) => Some(usage),
                _ => None,
            })
            .collect();
        assert_eq!(usage.len(), 1, "one usage record: {after:?}");
        assert!(matches!(after.last(), Some(SynthesisEvent::Usage(_))));
        let audio_ms = usage[0]
            .quantities
            .iter()
            .find(|quantity| quantity.kind() == QuantityKind::OutputAudioMs)
            .and_then(|quantity| quantity.value());
        let emitted_ms = i64::from(first.frame().sample_count) / 8;
        assert_eq!(
            audio_ms,
            Some(emitted_ms),
            "the audio emitted before the cancel"
        );
        assert!(usage[0].r#final);
        assert_eq!(handle.next(), None, "and nothing after the record");
        assert_eq!(
            server.seen().len(),
            1,
            "the discarded segment is never requested"
        );
        assert_eq!(server.violations(), Vec::<String>::new());
    });
}

#[test]
fn dropping_the_handle_mid_segment_closes_the_connection_and_frees_the_slot() {
    within_deadline(|| {
        let server = FixtureServer::start(Script::Stall);
        let mut synthesiser = server.synthesiser(format(AudioEncoding::PcmMulaw, 8_000));
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
fn after_a_cancel_the_next_segment_is_spoken_on_the_same_sample_clock() {
    within_deadline(|| {
        let server = FixtureServer::scripted(vec![Script::Stall, Script::Serve]);
        let mut synthesiser = server.synthesiser(format(AudioEncoding::PcmMulaw, 8_000));
        let mut handle = synthesiser.start().unwrap();
        let mut segments = committed(&["Barge in on me.", "After the barge-in."]).into_iter();
        let cancelled = segments.next().unwrap();
        let cancelled_id = cancelled.segment_id();
        handle.offer_segment(cancelled).unwrap();
        let Some(SynthesisEvent::Audio(first)) = handle.next() else {
            panic!("no first frame: {:?}", handle.failure());
        };
        handle.discard(cancelled_id).unwrap();
        let next = segments.next().unwrap();
        let next_id = next.segment_id();
        handle.offer_segment(next).unwrap();
        handle.end_utterance().unwrap();
        let after: Vec<SynthesisEvent> = handle.by_ref().collect();
        assert!(handle.failure().is_none(), "{:?}", handle.failure());
        let positions: Vec<(u64, u32)> = after
            .iter()
            .filter_map(|event| match event {
                SynthesisEvent::Audio(audio) => {
                    assert_eq!(audio.segment_id(), next_id);
                    Some((audio.frame().position.get(), audio.frame().sample_count))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            positions.first().map(|&(position, _)| position),
            Some(u64::from(first.frame().sample_count)),
            "the clock goes on past the cancel"
        );
        assert_eq!(
            positions
                .iter()
                .map(|&(_, count)| u64::from(count))
                .sum::<u64>(),
            1_600
        );
        assert_eq!(
            handle.output().segment(next_id).map(|s| s.state()),
            Some(SegmentState::Synthesized)
        );
        assert_eq!(server.violations(), Vec::<String>::new());
    });
}
