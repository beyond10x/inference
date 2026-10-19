//! Adversary pass 1 of `story:tts-contract`: the mock speaks "one frame of silence per word, at
//! least one". A Committed segment whose text has no word is still queued, so it still gets audio
//! the sink can acknowledge; otherwise it would stay Synthesized and never become Played. No case
//! of the unit offers such a segment.

#![cfg(feature = "tts")]

use std::num::{NonZeroU16, NonZeroU32};

use inference_core::generation::{Generation, StartGeneration};
use inference_core::id::{NodeId, RunId};
use inference_core::model::ModelId;
use inference_testkit::tts::{MockConfiguration, MockSink, MockSynthesiser};
use inference_text::{OutputSegment, SegmentState};
use inference_tts::inference_audio::{AudioEncoding, AudioFormat};
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};

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

#[test]
fn a_segment_without_words_gets_one_frame_and_can_be_played() {
    let format = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).expect("not zero"),
        channels: NonZeroU16::new(1).expect("not zero"),
    };
    for text in ["", "   "] {
        for configuration in MockConfiguration::ALL {
            let mut handle = MockSynthesiser::new(configuration, ModelId::generate(), format)
                .start()
                .expect("a fresh synthesiser runs nothing else");
            let segment = committed(text);
            let id = segment.segment_id();
            handle.offer_segment(segment).expect("queued");
            handle.end_utterance().expect("the utterance ends once");
            let mut sink = MockSink::new();
            for event in handle.by_ref() {
                if let SynthesisEvent::Audio(audio) = event {
                    sink.give(audio);
                }
            }
            assert_eq!(
                sink.frames_given(id),
                1,
                "{configuration:?}, {text:?}: one frame at least"
            );
            let held = handle.output().segment(id).expect("held").snapshot();
            assert_eq!(held.state(), SegmentState::Synthesized, "{configuration:?}");
            let played = sink
                .acknowledge(&mut handle, id)
                .map(|(played, _)| played.state());
            assert_eq!(
                played,
                Ok(SegmentState::Played),
                "{configuration:?}, {text:?}"
            );
        }
    }
}
