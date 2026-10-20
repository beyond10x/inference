//! Adversary pass 1 on `story:usage-tts`: the late synthesiser of the cascade
//! (`cascade::late::LateSynthesiser`) over the mock. The contract makes the usage record the last
//! event of a synthesis; the wrapper holds back frames of one segment and lets every other event,
//! the record among them, pass. Frames of the held segment emitted after the utterance ended are
//! held past the record: discarded after the end, they are delivered after it; never discarded,
//! they are never delivered, while the record counts them.

#![cfg(all(feature = "stt", feature = "llm", feature = "tts", feature = "run"))]

use std::num::{NonZeroU16, NonZeroU32};

use inference_core::generation::{Generation, StartGeneration};
use inference_core::id::{NodeId, RunId};
use inference_core::model::ModelId;
use inference_core::usage::QuantityKind;
use inference_testkit::cascade::late::LateSynthesiser;
use inference_testkit::tts::{FRAME_SAMPLES, MockConfiguration, MockSynthesiser};
use inference_text::{OutputSegment, SegmentId};
use inference_tts::inference_audio::{AudioEncoding, AudioFormat};
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};

const RATE: u32 = 16_000;

/// A late synthesis over the mock in `configuration` holding back the second segment, offered
/// "one two" and "three four five" and ended before anything is delivered.
fn ended(
    configuration: MockConfiguration,
) -> (
    <LateSynthesiser<MockSynthesiser> as Synthesiser>::Handle,
    Vec<SegmentId>,
) {
    let format = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(RATE).expect("not zero"),
        channels: NonZeroU16::new(1).expect("not zero"),
    };
    let mut late = LateSynthesiser::new(
        MockSynthesiser::new(configuration, ModelId::generate(), format),
        1,
    );
    let mut synthesis = late.start().expect("one synthesis");
    let (generation, _) = Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    });
    let mut ids = Vec::new();
    for (sequence, text) in [(1, "one two"), (2, "three four five")] {
        let (tentative, _) =
            OutputSegment::emit(&generation, sequence, text.to_owned()).expect("Active");
        let (segment, _) = tentative.commit().expect("Tentative");
        ids.push(segment.segment_id());
        synthesis.offer_segment(segment).expect("Committed");
    }
    synthesis.end_utterance().expect("ends once");
    (synthesis, ids)
}

#[test]
fn frames_released_by_a_discard_after_the_end_do_not_follow_the_usage_record() {
    for configuration in MockConfiguration::ALL {
        let (mut synthesis, ids) = ended(configuration);
        let mut events: Vec<SynthesisEvent> = synthesis.by_ref().collect();
        synthesis
            .discard(ids[1])
            .expect("a Synthesized segment is discarded");
        events.extend(synthesis.by_ref());
        let usage_at = events
            .iter()
            .position(|event| matches!(event, SynthesisEvent::Usage(_)))
            .expect("one record");
        assert_eq!(
            usage_at,
            events.len() - 1,
            "{configuration:?}: the usage record is the last event: {:?}",
            events
                .iter()
                .map(|event| (event.name(), event.segment_id()))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn frames_counted_by_the_usage_record_are_delivered_when_nothing_is_discarded() {
    for configuration in MockConfiguration::ALL {
        let (mut synthesis, _) = ended(configuration);
        let events: Vec<SynthesisEvent> = synthesis.by_ref().collect();
        let delivered: u64 = events
            .iter()
            .filter_map(|event| match event {
                SynthesisEvent::Audio(audio) => Some(u64::from(audio.frame().sample_count)),
                _ => None,
            })
            .sum();
        let Some(SynthesisEvent::Usage(recorded)) = events.last() else {
            panic!("{configuration:?}: the record is last")
        };
        let counted = recorded
            .quantities
            .iter()
            .find(|quantity| quantity.kind() == QuantityKind::OutputAudioMs)
            .and_then(|quantity| quantity.value())
            .expect("measured");
        assert_eq!(
            counted,
            i64::try_from(5 * u64::from(FRAME_SAMPLES) * 1_000 / u64::from(RATE)).expect("short"),
            "{configuration:?}: five words, five frames"
        );
        assert_eq!(
            delivered,
            5 * u64::from(FRAME_SAMPLES),
            "{configuration:?}: the frames of the held segment, emitted after the end, are \
             delivered once the utterance ends, as the module documents"
        );
    }
}
