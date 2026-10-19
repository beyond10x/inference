//! Adversary pass 1 of `story:tts-contract`: the acceptance says a segment becomes Played only on
//! the sink acknowledgement "and then refuses discard", and `ess/domains/stream.yaml` makes Played
//! terminal (`inference.stream.OutputSegment`, transition `discard` from Tentative, Committed,
//! Queued or Synthesized only; `DiscardSegment`: "it cannot unsay what the user heard"). The
//! synthesis is the one record of the segment that follows it through synthesis; once the sink
//! has acknowledged playback, that record must not still offer the segment for discard as
//! unplayed audio.

#![cfg(feature = "tts")]

use std::num::{NonZeroU16, NonZeroU32};

use inference_core::generation::{Generation, StartGeneration};
use inference_core::id::{NodeId, RunId};
use inference_core::model::ModelId;
use inference_testkit::tts::{MockConfiguration, MockSink, MockSynthesiser};
use inference_text::{OutputSegment, SegmentState, SegmentStateConflict};
use inference_tts::inference_audio::{AudioEncoding, AudioFormat};
use inference_tts::{DiscardRefused, SynthesisEvent, SynthesisHandle, Synthesiser};

fn committed(text: &str) -> OutputSegment {
    let (generation, _) = Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    });
    let (tentative, _) =
        OutputSegment::emit(&generation, 1, text.to_owned()).expect("the generation is Active");
    tentative.commit().expect("a Tentative segment commits").0
}

fn format() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).expect("not zero"),
        channels: NonZeroU16::new(1).expect("not zero"),
    }
}

#[test]
fn once_the_sink_acknowledges_playback_the_synthesis_record_refuses_discard() {
    for configuration in MockConfiguration::ALL {
        let mut handle = MockSynthesiser::new(configuration, ModelId::generate(), format())
            .start()
            .expect("a fresh synthesiser runs nothing else");
        let segment = committed("Hello there.");
        let id = segment.segment_id();
        handle.offer_segment(segment).expect("queued");
        handle.end_utterance().expect("the utterance ends once");
        let mut sink = MockSink::new();
        for event in handle.by_ref() {
            if let SynthesisEvent::Audio(audio) = event {
                sink.give(audio);
            }
        }
        let (played, _) = sink
            .acknowledge(&mut handle, id)
            .unwrap_or_else(|error| panic!("{configuration:?}: acknowledged ({error:?})"));
        assert_eq!(played.state(), SegmentState::Played, "{configuration:?}");

        let held = handle.output().segment(id).expect("held").state();
        assert_eq!(
            handle.discard(id),
            Err(DiscardRefused::SegmentStateConflict(SegmentStateConflict {
                current: SegmentState::Played,
            })),
            "{configuration:?}: after AcknowledgePlayback the synthesis still holds the segment as {held:?} and lets cancellation discard audio the user heard"
        );
        assert_eq!(
            handle.output().segment(id).map(OutputSegment::state),
            Some(SegmentState::Played),
            "{configuration:?}: the refused discard leaves the record Played"
        );
    }
}
