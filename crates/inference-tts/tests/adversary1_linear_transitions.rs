//! Adversary pass 1 of `story:segment-linear-transitions`, against `SynthesisOutput` now that a
//! transition of `OutputSegment` consumes the segment: the synthesis takes the segment out of its
//! record, moves it and puts the moved one, or the refused one, back in the same place.
//!
//! `inference.stream.CompleteSynthesis` and `inference.stream.AcknowledgePlayback`
//! (`ess/domains/stream.yaml`) refuse a segment outside their source state as `wrong-state`; the
//! documented order of refusals puts that one before the frame checks, as it was when the
//! transition ran first.

use std::num::{NonZeroU16, NonZeroU32};

use inference_tts::inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_tts::inference_core::generation::{Generation, StartGeneration};
use inference_tts::inference_core::id::{NodeId, RunId};
use inference_tts::inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_tts::inference_text::{OutputSegment, SegmentId, SegmentState, SegmentStateConflict};
use inference_tts::{
    CONTRACT_REVISION, CompletionRefused, DiscardRefused, PlaybackReceipt, PlaybackRefused,
    SegmentAudio, SynthesisEvent, SynthesisOutput,
};

fn adapter(capabilities: Vec<Capability>) -> Adapter {
    Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: "adversary".to_owned(),
        contract_revision: CONTRACT_REVISION.to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities,
        max_concurrency: 1,
    })
    .expect("a maximum concurrency of one is positive")
    .0
}

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

/// A frame of two samples whose data is `data_len` bytes; four is the length that agrees.
fn frame(data_len: usize) -> AudioFrame {
    AudioFrame {
        format: AudioFormat {
            encoding: AudioEncoding::PcmS16le,
            sample_rate: NonZeroU32::new(16_000).expect("not zero"),
            channels: NonZeroU16::new(1).expect("not zero"),
        },
        position: SampleClockPosition::new(0).expect("in range"),
        sample_count: 2,
        data: vec![0; data_len],
    }
}

fn frames_of(events: Vec<SynthesisEvent>) -> Vec<SegmentAudio> {
    events
        .into_iter()
        .filter_map(|event| match event {
            SynthesisEvent::Audio(audio) => Some(audio),
            SynthesisEvent::Completed(_) => None,
        })
        .collect()
}

/// A synthesis of `capabilities` holding `segments`, all taken.
fn taken(capabilities: Vec<Capability>, segments: Vec<OutputSegment>) -> SynthesisOutput {
    let mut output = SynthesisOutput::start(&adapter(capabilities));
    for segment in segments {
        output.offer(segment).expect("queued");
    }
    output.end_utterance().expect("ends once");
    output.take_input().expect("after the end");
    output
}

fn ids(output: &SynthesisOutput) -> Vec<SegmentId> {
    output
        .segments()
        .iter()
        .map(OutputSegment::segment_id)
        .collect()
}

/// Guard: the second `compile_fail,E0382` doctest of the crate, its second commit and second offer
/// left out, compiles and runs; so the doctest fails on the second use of the segment and not on
/// an import or a signature.
#[test]
fn the_commit_twice_doctest_compiles_without_its_second_value() {
    fn commit_once_and_offer(adapter: &Adapter, tentative: OutputSegment) {
        let (segment, _) = tentative.commit().unwrap();
        let _ = SynthesisOutput::start(adapter).offer(segment);
    }
    let (generation, _) = Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    });
    let (tentative, _) = OutputSegment::emit(&generation, 1, "once".to_owned()).unwrap();
    commit_once_and_offer(&adapter(Vec::new()), tentative);
}

/// `CompleteSynthesis` on a segment that is not Queued is `wrong-state` whatever its frames are:
/// the state is read before the frames, as the documented order of refusals has it.
#[test]
fn completing_a_segment_that_is_not_queued_is_wrong_state_before_a_malformed_frame() {
    let (done, gone) = (committed("done"), committed("gone"));
    let (done_id, gone_id) = (done.segment_id(), gone.segment_id());
    let mut output = taken(Vec::new(), vec![done, gone]);
    output
        .complete(done_id, vec![frame(4)])
        .expect("Queued to Synthesized");
    output.discard(gone_id).expect("a Queued segment discards");
    let order = ids(&output);

    for (id, current) in [
        (done_id, SegmentState::Synthesized),
        (gone_id, SegmentState::Discarded),
    ] {
        assert_eq!(
            output.complete(id, vec![frame(3)]),
            Err(CompletionRefused::SegmentStateConflict(
                SegmentStateConflict { current }
            )),
            "a {current:?} segment is refused as wrong-state before its frames are read"
        );
    }
    assert_eq!(
        ids(&output),
        order,
        "a refusal leaves every segment in place"
    );
    assert_eq!(output.frames_emitted(done_id), Some(1));
}

/// `AcknowledgePlayback` on a segment that is not Synthesized is `wrong-state` whatever the
/// receipt says: the state is read before the receipt's segment and its frame count.
#[test]
fn acknowledging_a_segment_that_is_not_synthesized_is_wrong_state_before_the_receipt() {
    let (queued, other) = (committed("queued"), committed("other"));
    let (queued_id, other_id) = (queued.segment_id(), other.segment_id());
    let mut output = taken(vec![Capability::OutputStreaming], vec![queued, other]);
    output
        .audio(queued_id, frame(4))
        .expect("a Queued segment streams");
    let of_other = frames_of(
        output
            .complete(other_id, vec![frame(4)])
            .expect("Queued to Synthesized"),
    );

    let receipt = PlaybackReceipt::from_frames(of_other).expect("frames of one segment");
    assert_eq!(
        output.acknowledge_playback(queued_id, receipt),
        Err(PlaybackRefused::SegmentStateConflict(
            SegmentStateConflict {
                current: SegmentState::Queued
            }
        )),
        "a Queued segment is wrong-state before the receipt is found to name another"
    );
    let empty = PlaybackReceipt::from_frames(Vec::new()).expect("the receipt of no frames");
    assert_eq!(
        output.acknowledge_playback(queued_id, empty),
        Err(PlaybackRefused::SegmentStateConflict(
            SegmentStateConflict {
                current: SegmentState::Queued
            }
        )),
        "a Queued segment with one frame emitted is wrong-state before its frames are counted"
    );
    assert_eq!(
        output.segment(queued_id).map(OutputSegment::state),
        Some(SegmentState::Queued)
    );
}

/// A move takes the segment out of the record and puts the moved one back: each segment keeps
/// its place among the others, and the frames counted of it stay its own.
#[test]
fn every_move_and_refusal_keeps_each_segment_in_its_place() {
    let segments = vec![committed("a"), committed("b"), committed("c")];
    let order: Vec<SegmentId> = segments.iter().map(OutputSegment::segment_id).collect();
    let (a, b, c) = (order[0], order[1], order[2]);
    let mut output = taken(Vec::new(), segments);

    let of_a = frames_of(
        output
            .complete(a, vec![frame(4), frame(4)])
            .expect("A synthesized"),
    );
    let of_b = frames_of(output.complete(b, vec![frame(4)]).expect("B synthesized"));
    assert_eq!(ids(&output), order, "after completing A and B");
    assert_eq!(
        [a, b, c].map(|id| output.frames_emitted(id)),
        [Some(2), Some(1), Some(0)]
    );

    let receipt = PlaybackReceipt::from_frames(of_a).expect("frames of A");
    output.acknowledge_playback(a, receipt).expect("A played");
    assert_eq!(ids(&output), order, "after playing A");
    assert_eq!(
        output.discard(a),
        Err(DiscardRefused::SegmentStateConflict(SegmentStateConflict {
            current: SegmentState::Played
        }))
    );
    assert_eq!(ids(&output), order, "after the refused discard of A");
    output.discard(c).expect("C discards");
    assert_eq!(ids(&output), order, "after discarding C");
    assert_eq!(
        [a, b, c].map(|id| output.segment(id).map(OutputSegment::state)),
        [
            Some(SegmentState::Played),
            Some(SegmentState::Synthesized),
            Some(SegmentState::Discarded),
        ]
    );
    assert_eq!(
        [a, b, c].map(|id| output.frames_played(id)),
        [Some(2), Some(0), Some(0)]
    );
    let receipt = PlaybackReceipt::from_frames(of_b).expect("frames of B");
    output.acknowledge_playback(b, receipt).expect("B played");
    assert_eq!(ids(&output), order, "after playing B");
}
