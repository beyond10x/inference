//! §23.1 test 3 of `docs/design/inference.md` (line 1107): TTS input completion and audio emission
//! semantics are reflected accurately, against the mock synthesiser in the four configurations of
//! `incremental_input` and `output_streaming`. Input is accepted incrementally when segments are
//! taken, and audio produced, before the utterance is complete (line 385); audio is emitted
//! incrementally when a frame reaches the caller while its segment is still Queued, before its
//! `SynthesisCompleted` (line 386). Beside them: a Tentative segment is refused (line 390), and a
//! segment is Queued, then Synthesized, then Played only on the sink's acknowledgement, after which
//! it refuses discard (line 388). Every frame the mock emits carries `sample_count * channels *
//! width(encoding)` bytes (story `## Frame length`).

#![cfg(feature = "tts")]

use std::collections::HashSet;
use std::num::{NonZeroU16, NonZeroU32};

use inference_core::generation::{Generation, StartGeneration};
use inference_core::id::{NodeId, RunId};
use inference_core::model::{Capability, ModelId};
use inference_testkit::tts::{
    MockConfiguration, MockSink, MockSynthesis, MockSynthesiser, SinkRefused,
};
use inference_text::{AudioPlayed, OutputSegment, SegmentState, SegmentStateConflict};
use inference_tts::inference_audio::{AudioEncoding, AudioFormat};
use inference_tts::{DiscardRefused, OfferRefused, SynthesisEvent, SynthesisHandle, Synthesiser};

const TEXTS: [&str; 2] = ["Hello there.", "How are you today?"];

fn format(encoding: AudioEncoding, channels: u16) -> AudioFormat {
    AudioFormat {
        encoding,
        sample_rate: NonZeroU32::new(16_000).expect("16000 is not zero"),
        channels: NonZeroU16::new(channels).expect("the channel count is not zero"),
    }
}

fn synthesiser(configuration: MockConfiguration) -> MockSynthesiser {
    MockSynthesiser::new(
        configuration,
        ModelId::generate(),
        format(AudioEncoding::PcmS16le, 1),
    )
}

fn generation() -> Generation {
    Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    })
    .0
}

/// One Committed segment per text, emitted by one generation in order.
fn committed(texts: &[&str]) -> Vec<OutputSegment> {
    let generation = generation();
    texts
        .iter()
        .zip(1..)
        .map(|(text, sequence)| {
            let (tentative, _) = OutputSegment::emit(&generation, sequence, (*text).to_owned())
                .expect("the generation is Active");
            tentative.commit().expect("a Tentative segment commits").0
        })
        .collect()
}

/// The state the synthesis holds the segment of `event` in.
fn state_of(handle: &MockSynthesis, event: &SynthesisEvent) -> SegmentState {
    handle
        .output()
        .segment(event.segment_id())
        .expect("every event names a segment the synthesis holds")
        .state()
}

/// Every event delivered so far, each with the state of its segment when the caller received it.
fn drain(handle: &mut MockSynthesis) -> Vec<(SynthesisEvent, SegmentState)> {
    let mut seen = Vec::new();
    while let Some(event) = handle.next() {
        let state = state_of(handle, &event);
        seen.push((event, state));
    }
    seen
}

/// What one synthesis of [`TEXTS`] delivered before and after the end of the utterance.
struct Synthesis {
    handle: MockSynthesis,
    segments: Vec<OutputSegment>,
    before_end: Vec<(SynthesisEvent, SegmentState)>,
    taken_before_end: usize,
    after_end: Vec<(SynthesisEvent, SegmentState)>,
}

impl Synthesis {
    fn all(&self) -> impl Iterator<Item = &(SynthesisEvent, SegmentState)> {
        self.before_end.iter().chain(&self.after_end)
    }
}

/// Offers the segments one at a time, each Queued before any audio of it, draining what is
/// delivered after each; then ends the utterance and drains the rest.
fn synthesise(configuration: MockConfiguration) -> Synthesis {
    synthesise_with(synthesiser(configuration))
}

fn synthesise_with(mut synthesiser: MockSynthesiser) -> Synthesis {
    let segments = committed(&TEXTS);
    let mut handle = synthesiser.start();
    let mut before_end = Vec::new();
    for segment in &segments {
        let queued = handle
            .offer_segment(segment.clone())
            .expect("a Committed segment is queued");
        assert_eq!(queued.segment_id, segment.segment_id());
        assert_eq!(
            handle
                .output()
                .segment(segment.segment_id())
                .map(OutputSegment::state),
            Some(SegmentState::Queued),
            "an offered segment is Queued before synthesis"
        );
        before_end.extend(drain(&mut handle));
    }
    let taken_before_end = handle.output().segments_taken();
    handle.end_utterance().expect("the utterance ends once");
    let after_end = drain(&mut handle);
    Synthesis {
        handle,
        segments,
        before_end,
        taken_before_end,
        after_end,
    }
}

fn claims(configuration: MockConfiguration, capability: Capability) -> bool {
    configuration.capabilities().contains(&capability)
}

/// Whether input was taken, and audio produced, before the utterance was complete.
fn takes_input_incrementally(synthesis: &Synthesis) -> bool {
    synthesis.taken_before_end > 0
        && synthesis
            .before_end
            .iter()
            .any(|(event, _)| matches!(event, SynthesisEvent::Audio(_)))
}

/// Whether a frame of every segment reached the caller while that segment was still Queued.
fn streams_audio(synthesis: &Synthesis) -> bool {
    synthesis.segments.iter().all(|segment| {
        synthesis.all().any(|(event, state)| {
            matches!(event, SynthesisEvent::Audio(audio) if audio.segment_id == segment.segment_id())
                && *state == SegmentState::Queued
        })
    })
}

#[test]
fn input_is_accepted_before_the_utterance_is_complete_exactly_when_incremental_input_is_claimed() {
    for configuration in MockConfiguration::ALL {
        let synthesis = synthesise(configuration);
        if claims(configuration, Capability::IncrementalInput) {
            assert_eq!(
                synthesis.taken_before_end,
                TEXTS.len(),
                "{configuration:?}: claiming incremental_input, each segment is taken as it is offered"
            );
            assert!(
                takes_input_incrementally(&synthesis),
                "{configuration:?}: claiming incremental_input, audio is produced before the utterance is complete: {:?}",
                synthesis.before_end
            );
        } else {
            assert_eq!(
                synthesis.taken_before_end, 0,
                "{configuration:?}: without incremental_input, no segment is taken before the utterance is complete"
            );
            assert!(
                synthesis.before_end.is_empty(),
                "{configuration:?}: without incremental_input, nothing is delivered before the utterance is complete: {:?}",
                synthesis.before_end
            );
        }
        assert_eq!(
            synthesis.handle.output().segments_taken(),
            TEXTS.len(),
            "{configuration:?}: every segment is taken once the utterance is complete"
        );
    }
}

#[test]
fn audio_is_emitted_before_synthesis_completes_exactly_when_output_streaming_is_claimed() {
    for configuration in MockConfiguration::ALL {
        let synthesis = synthesise(configuration);
        for segment in &synthesis.segments {
            let id = segment.segment_id();
            let events: Vec<&(SynthesisEvent, SegmentState)> = synthesis
                .all()
                .filter(|(event, _)| event.segment_id() == id)
                .collect();
            let completions: Vec<usize> = events
                .iter()
                .enumerate()
                .filter(|(_, (event, _))| matches!(event, SynthesisEvent::Completed(_)))
                .map(|(index, _)| index)
                .collect();
            assert_eq!(
                completions.len(),
                1,
                "{configuration:?}: one SynthesisCompleted per segment: {events:?}"
            );
            let completed_at = completions[0];
            assert_eq!(
                events[completed_at].1,
                SegmentState::Synthesized,
                "{configuration:?}: SynthesisCompleted reaches the caller with the segment Synthesized"
            );
            let audio: Vec<(usize, SegmentState)> = events
                .iter()
                .enumerate()
                .filter(|(_, (event, _))| matches!(event, SynthesisEvent::Audio(_)))
                .map(|(index, (_, state))| (index, *state))
                .collect();
            assert!(
                !audio.is_empty(),
                "{configuration:?}: every segment has audio: {events:?}"
            );
            if claims(configuration, Capability::OutputStreaming) {
                assert!(
                    audio.iter().all(
                        |&(index, state)| index < completed_at && state == SegmentState::Queued
                    ),
                    "{configuration:?}: claiming output_streaming, every frame arrives while the segment is Queued, before SynthesisCompleted: {events:?}"
                );
            } else {
                assert!(
                    audio.iter().all(|&(index, state)| index > completed_at
                        && state == SegmentState::Synthesized),
                    "{configuration:?}: without output_streaming, frames arrive only after SynthesisCompleted: {events:?}"
                );
            }
        }
    }
}

#[test]
fn each_configuration_claims_exactly_what_it_was_observed_to_do() {
    for configuration in MockConfiguration::ALL {
        let synthesis = synthesise(configuration);
        let mut observed = HashSet::new();
        if takes_input_incrementally(&synthesis) {
            observed.insert(Capability::IncrementalInput);
        }
        if streams_audio(&synthesis) {
            observed.insert(Capability::OutputStreaming);
        }
        let claimed: HashSet<Capability> = synthesiser(configuration)
            .adapter()
            .capabilities()
            .iter()
            .copied()
            .collect();
        assert_eq!(
            claimed, observed,
            "{configuration:?} claims what it was observed to do"
        );
    }
    let distinct: HashSet<Vec<Capability>> = MockConfiguration::ALL
        .iter()
        .map(|configuration| configuration.capabilities())
        .collect();
    assert_eq!(distinct.len(), 4, "the four configurations claim apart");
}

#[test]
fn a_tentative_segment_is_refused_with_segment_state_conflict_and_nothing_is_queued() {
    for configuration in MockConfiguration::ALL {
        let generation = generation();
        let (tentative, _) = OutputSegment::emit(&generation, 1, "not yet safe".to_owned())
            .expect("the generation is Active");
        let mut handle = synthesiser(configuration).start();

        assert_eq!(
            handle.offer_segment(tentative.clone()),
            Err(OfferRefused::SegmentStateConflict(SegmentStateConflict {
                current: SegmentState::Tentative,
            })),
            "{configuration:?}: QueueForSynthesis refuses a Tentative segment"
        );
        assert!(handle.output().segments().is_empty(), "{configuration:?}");
        handle.end_utterance().expect("the utterance ends once");
        let delivered = drain(&mut handle);
        assert!(
            delivered.is_empty(),
            "{configuration:?}: a refused segment is never spoken: {delivered:?}"
        );
    }
}

#[test]
fn a_segment_is_played_only_on_the_sink_acknowledgement_after_synthesis_and_then_refuses_discard() {
    for configuration in MockConfiguration::ALL {
        let mut handle = synthesiser(configuration).start();
        let mut segments = committed(&TEXTS);
        let second = segments.pop().expect("two segments");
        let second_id = second.segment_id();
        let segment = segments.pop().expect("two segments");
        let id = segment.segment_id();
        handle.offer_segment(segment).expect("queued");
        let mut sink = MockSink::new();

        assert_eq!(
            handle.output().segment(id).map(OutputSegment::state),
            Some(SegmentState::Queued),
            "{configuration:?}"
        );
        assert_eq!(
            sink.acknowledge(&mut handle, id),
            Err(SinkRefused::SegmentStateConflict(SegmentStateConflict {
                current: SegmentState::Queued,
            })),
            "{configuration:?}: a Queued segment refuses the acknowledgement"
        );

        handle.offer_segment(second).expect("queued");
        handle.end_utterance().expect("the utterance ends once");
        while let Some(event) = handle.next() {
            if let SynthesisEvent::Audio(audio) = event {
                let segment_id = audio.segment_id;
                sink.give(audio);
                if handle
                    .output()
                    .segment(segment_id)
                    .map(OutputSegment::state)
                    == Some(SegmentState::Queued)
                {
                    assert_eq!(
                        sink.acknowledge(&mut handle, segment_id),
                        Err(SinkRefused::SegmentStateConflict(SegmentStateConflict {
                            current: SegmentState::Queued,
                        })),
                        "{configuration:?}: audio given while the segment is Queued is not yet playable"
                    );
                }
            }
        }
        assert!(sink.frames_given(id) > 0, "{configuration:?}");
        assert_eq!(
            handle.output().segment(id).map(OutputSegment::state),
            Some(SegmentState::Synthesized),
            "{configuration:?}: Synthesized once its audio is produced, and not Played without the sink"
        );

        let (played, event) = sink
            .acknowledge(&mut handle, id)
            .unwrap_or_else(|error| panic!("{configuration:?}: the sink acknowledges ({error:?})"));
        assert_eq!(played.state(), SegmentState::Played, "{configuration:?}");
        assert_eq!(event, AudioPlayed { segment_id: id });
        assert_eq!(
            handle.output().segment(id).map(OutputSegment::state),
            Some(SegmentState::Played),
            "{configuration:?}: the acknowledgement moves the synthesis's own record"
        );
        assert_eq!(
            sink.acknowledge(&mut handle, id),
            Err(SinkRefused::SegmentStateConflict(SegmentStateConflict {
                current: SegmentState::Played,
            })),
            "{configuration:?}: a segment is acknowledged once"
        );

        assert_eq!(
            handle.discard(id),
            Err(DiscardRefused::SegmentStateConflict(SegmentStateConflict {
                current: SegmentState::Played,
            })),
            "{configuration:?}: a Played segment cannot be unsaid"
        );
        assert_eq!(
            handle.output().segment(id).map(OutputSegment::state),
            Some(SegmentState::Played)
        );
        assert!(
            handle.discard(second_id).is_ok(),
            "{configuration:?}: a Synthesized segment not yet played is still discarded"
        );
        assert_eq!(
            handle.output().segment(second_id).map(OutputSegment::state),
            Some(SegmentState::Discarded)
        );
    }
}

#[test]
fn the_sink_refuses_to_acknowledge_audio_it_was_never_given() {
    let mut handle = synthesiser(MockConfiguration::Neither).start();
    let segment = committed(&TEXTS[..1]).remove(0);
    let id = segment.segment_id();
    handle.offer_segment(segment).expect("queued");
    handle.end_utterance().expect("the utterance ends once");
    drain(&mut handle);
    assert_eq!(
        handle.output().segment(id).map(OutputSegment::state),
        Some(SegmentState::Synthesized)
    );

    assert_eq!(
        MockSink::new().acknowledge(&mut handle, id),
        Err(SinkRefused::NothingGiven { segment_id: id })
    );
}

#[test]
fn without_output_streaming_a_segment_is_played_only_once_all_its_frames_are_given_and_none_follow()
{
    for configuration in MockConfiguration::ALL {
        if claims(configuration, Capability::OutputStreaming) {
            continue;
        }
        let mut handle = synthesiser(configuration).start();
        let segment = committed(&TEXTS[1..]).remove(0);
        let id = segment.segment_id();
        handle.offer_segment(segment).expect("queued");
        handle.end_utterance().expect("the utterance ends once");
        let mut sink = MockSink::new();

        let completed = handle.next();
        assert!(
            matches!(completed, Some(SynthesisEvent::Completed(_))),
            "{configuration:?}: SynthesisCompleted comes first: {completed:?}"
        );
        let emitted = handle
            .output()
            .frames_emitted(id)
            .expect("the segment is held");
        assert!(emitted > 1, "{configuration:?}: {emitted} frames");
        let Some(SynthesisEvent::Audio(first)) = handle.next() else {
            panic!("{configuration:?}: a frame follows SynthesisCompleted");
        };
        sink.give(first);
        assert_eq!(
            sink.acknowledge(&mut handle, id),
            Err(SinkRefused::FramesOutstanding {
                segment_id: id,
                given: 1,
                emitted,
            }),
            "{configuration:?}: a Synthesized segment is not played before all its frames are given"
        );

        while sink.frames_given(id) < emitted {
            match handle.next() {
                Some(SynthesisEvent::Audio(audio)) => sink.give(audio),
                other => panic!("{configuration:?}: a frame was expected, got {other:?}"),
            }
        }
        sink.acknowledge(&mut handle, id)
            .unwrap_or_else(|error| panic!("{configuration:?}: the sink acknowledges ({error:?})"));
        let after: Vec<SynthesisEvent> = handle.by_ref().collect();
        assert!(
            after.iter().all(|event| event.segment_id() != id),
            "{configuration:?}: no frame of a Played segment is emitted afterwards: {after:?}"
        );
        assert_eq!(handle.output().frames_emitted(id), Some(emitted));
    }
}

#[test]
fn every_frame_carries_sample_count_times_channels_times_encoding_width_bytes() {
    for encoding in AudioEncoding::ALL {
        let width = match encoding {
            AudioEncoding::PcmS16le => 2,
            AudioEncoding::PcmF32le => 4,
        };
        for channels in [1_u16, 2] {
            for configuration in MockConfiguration::ALL {
                let synthesis = synthesise_with(MockSynthesiser::new(
                    configuration,
                    ModelId::generate(),
                    format(encoding, channels),
                ));
                let mut frames = 0;
                for (event, _) in synthesis.all() {
                    if let SynthesisEvent::Audio(audio) = event {
                        let frame = &audio.frame;
                        assert_eq!(frame.format, format(encoding, channels));
                        assert!(frame.sample_count > 0);
                        assert_eq!(
                            frame.data.len(),
                            frame.sample_count as usize * usize::from(channels) * width,
                            "{configuration:?}, {encoding:?}, {channels} channel(s)"
                        );
                        assert_eq!(
                            frame.format.data_len(frame.sample_count),
                            frame.data.len() as u64,
                            "inference-audio computes the same length"
                        );
                        frames += 1;
                    }
                }
                assert!(frames > 0, "{configuration:?}: frames were emitted");
            }
        }
    }
}
