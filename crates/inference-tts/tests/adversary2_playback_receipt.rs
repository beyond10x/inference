//! Adversary pass 2 of `story:contracts-review-fixes`, on the correction round: playback is
//! acknowledged with the segment's identity and a [`PlaybackReceipt`] the sink makes by consuming
//! the frames it played; the receipt of no frames confirms only a segment that emitted none.
//! `inference.stream.AcknowledgePlayback` (`ess/domains/stream.yaml`) refuses a segment that is
//! not Synthesized as `wrong-state` and changes nothing, so the same command runs again once the
//! segment is Synthesized.

use std::num::{NonZeroU16, NonZeroU32};

use inference_tts::inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_tts::inference_core::generation::{Generation, StartGeneration};
use inference_tts::inference_core::id::{NodeId, RunId};
use inference_tts::inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_tts::inference_text::{OutputSegment, SegmentId, SegmentState, SegmentStateConflict};
use inference_tts::{
    CONTRACT_REVISION, FrameOfAnotherSegment, PlaybackReceipt, PlaybackRefused,
    PlayedFramesMismatch, SegmentAudio, SegmentNotHeld, SynthesisEvent, SynthesisOutput,
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

fn frame() -> AudioFrame {
    AudioFrame {
        format: AudioFormat {
            encoding: AudioEncoding::PcmS16le,
            sample_rate: NonZeroU32::new(16_000).expect("not zero"),
            channels: NonZeroU16::new(1).expect("not zero"),
        },
        position: SampleClockPosition::new(0).expect("in range"),
        sample_count: 2,
        data: vec![0; 4],
    }
}

fn audio_of(event: SynthesisEvent) -> Option<SegmentAudio> {
    match event {
        SynthesisEvent::Audio(audio) => Some(audio),
        SynthesisEvent::Completed(_) => None,
    }
}

fn frames_of(events: Vec<SynthesisEvent>) -> Vec<SegmentAudio> {
    events.into_iter().filter_map(audio_of).collect()
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

fn state(output: &SynthesisOutput, id: SegmentId) -> Option<SegmentState> {
    output.segment(id).map(OutputSegment::state)
}

/// Red. A streaming synthesiser emits both frames of a segment while it is Queued; the sink plays
/// them, makes its receipt and acknowledges before the synthesis completes. That is refused as
/// `wrong-state`, "nothing moves", and the segment then completes without further frames. The
/// sink played every frame of a Synthesized segment, and `AcknowledgePlayback` has a `played`
/// outcome for it; but the refusal consumed the receipt, the frames went with it, and the only
/// receipt left to make is the one of no frames, which the synthesis refuses for a segment with
/// frames. The segment can never be Played.
#[test]
fn a_receipt_refused_as_wrong_state_still_confirms_the_segment_once_it_is_synthesized() {
    let segment = committed("hello there");
    let id = segment.segment_id();
    let mut output = taken(vec![Capability::OutputStreaming], vec![segment]);
    let mut sink: Vec<SegmentAudio> = Vec::new();
    for _ in 0..2 {
        let event = output.audio(id, frame()).expect("a streamed frame");
        sink.extend(audio_of(event));
    }

    let early = PlaybackReceipt::from_frames(sink).expect("frames of one segment");
    assert_eq!(
        output.acknowledge_playback(id, early),
        Err(PlaybackRefused::SegmentStateConflict(
            SegmentStateConflict {
                current: SegmentState::Queued,
            }
        )),
        "a Queued segment is not yet played"
    );
    output
        .complete(id, Vec::new())
        .expect("Queued to Synthesized");
    assert_eq!(output.frames_emitted(id), Some(2));

    let left = PlaybackReceipt::from_frames(Vec::new()).expect("all the sink can still make");
    assert_eq!(
        output
            .acknowledge_playback(id, left)
            .map(|played| played.segment_id),
        Ok(id),
        "the sink played both frames of the Synthesized segment, and a refusal that changed \
         nothing left it no receipt to confirm them with"
    );
}

/// Red, the same loss through a sink's mistake: a receipt of segment A's frames passed with
/// segment B's identity is refused, and A, whose frames were all played, can never be Played.
#[test]
fn a_receipt_refused_for_another_segment_still_confirms_its_own() {
    let (one, two) = (committed("one"), committed("two"));
    let (a, b) = (one.segment_id(), two.segment_id());
    let mut output = taken(Vec::new(), vec![one, two]);
    let of_a = frames_of(
        output
            .complete(a, vec![frame(), frame()])
            .expect("A synthesized"),
    );
    drop(frames_of(
        output
            .complete(b, vec![frame(), frame()])
            .expect("B synthesized"),
    ));

    let receipt = PlaybackReceipt::from_frames(of_a).expect("frames of A");
    assert_eq!(
        output.acknowledge_playback(b, receipt),
        Err(PlaybackRefused::FrameOfAnotherSegment(
            FrameOfAnotherSegment {
                segment_id: b,
                other: a,
            }
        )),
        "A's frames do not confirm B, though both emitted two"
    );
    let left = PlaybackReceipt::from_frames(Vec::new()).expect("all the sink can still make");
    assert!(
        output.acknowledge_playback(a, left).is_ok(),
        "the sink played both frames of A, and the refusal for B left it no receipt for A"
    );
}

/// Guard: the receipt of no frames does not confirm a segment that is not Synthesized, even one
/// that has emitted no frame yet, nor a segment of another synthesis; and it confirms a segment
/// synthesized without frames once, after which a second acknowledgement is `wrong-state`.
#[test]
fn the_receipt_of_no_frames_confirms_only_a_synthesized_segment_without_frames_once() {
    let segment = committed("");
    let id = segment.segment_id();
    let mut output = taken(vec![Capability::OutputStreaming], vec![segment]);
    assert_eq!(output.frames_emitted(id), Some(0));
    assert_eq!(
        output.acknowledge_playback(id, PlaybackReceipt::from_frames(Vec::new()).expect("empty")),
        Err(PlaybackRefused::SegmentStateConflict(
            SegmentStateConflict {
                current: SegmentState::Queued,
            }
        )),
        "a Queued segment with no frame yet is not Played on the receipt of no frames"
    );
    assert_eq!(state(&output, id), Some(SegmentState::Queued));

    let elsewhere = committed("");
    let other = elsewhere.segment_id();
    let mut another = taken(Vec::new(), vec![elsewhere]);
    drop(another.complete(other, Vec::new()).expect("Synthesized"));
    assert_eq!(
        output.acknowledge_playback(
            other,
            PlaybackReceipt::from_frames(Vec::new()).expect("empty")
        ),
        Err(PlaybackRefused::SegmentNotHeld(SegmentNotHeld {
            segment_id: other
        })),
        "a segment of another synthesis is not held by this one"
    );

    drop(output.complete(id, Vec::new()).expect("Synthesized"));
    assert!(
        output
            .acknowledge_playback(id, PlaybackReceipt::from_frames(Vec::new()).expect("empty"))
            .is_ok()
    );
    assert_eq!(state(&output, id), Some(SegmentState::Played));
    assert_eq!(
        output.acknowledge_playback(id, PlaybackReceipt::from_frames(Vec::new()).expect("empty")),
        Err(PlaybackRefused::SegmentStateConflict(
            SegmentStateConflict {
                current: SegmentState::Played,
            }
        )),
        "a second acknowledgement is wrong-state"
    );
}

/// Guard: a receipt of the frames streamed while Queued, without those `complete` emitted after
/// them, is partial and refused; nothing moves.
#[test]
fn a_receipt_of_the_streamed_frames_alone_is_partial() {
    let segment = committed("hello there");
    let id = segment.segment_id();
    let mut output = taken(vec![Capability::OutputStreaming], vec![segment]);
    let streamed: Vec<SegmentAudio> = audio_of(output.audio(id, frame()).expect("streamed"))
        .into_iter()
        .collect();
    let rest = frames_of(output.complete(id, vec![frame()]).expect("Synthesized"));
    assert_eq!(rest.len(), 1);
    assert_eq!(
        output.acknowledge_playback(id, PlaybackReceipt::from_frames(streamed).expect("one")),
        Err(PlaybackRefused::PlayedFramesMismatch(
            PlayedFramesMismatch {
                segment_id: id,
                played: 1,
                emitted: 2,
            }
        ))
    );
    assert_eq!(state(&output, id), Some(SegmentState::Synthesized));
}

/// Guard: a receipt names one segment whatever the position of a foreign frame among its frames.
#[test]
fn a_receipt_with_a_foreign_frame_last_is_refused() {
    let (one, two) = (committed("one"), committed("two"));
    let (a, b) = (one.segment_id(), two.segment_id());
    let mut output = taken(Vec::new(), vec![one, two]);
    let mut frames = frames_of(output.complete(a, vec![frame(), frame()]).expect("A"));
    frames.extend(frames_of(output.complete(b, vec![frame()]).expect("B")));
    assert_eq!(
        PlaybackReceipt::from_frames(frames),
        Err(FrameOfAnotherSegment {
            segment_id: a,
            other: b,
        })
    );
}

/// Guard: two syntheses each hold a value of one segment identity (a transition consumes the
/// segment, so a Tentative one commits once, but `OutputSegment::emit_as` takes the identity it
/// is given, so a caller naming one identity twice holds two segments of it; the crate docs say
/// two syntheses do not see each other). A receipt of more frames than this synthesis emitted of
/// the segment is refused, as one of fewer is.
#[test]
fn a_receipt_of_more_frames_than_emitted_is_refused() {
    let (generation, _) = Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    });
    let (tentative, _) =
        OutputSegment::emit(&generation, 1, "twice".to_owned()).expect("the generation is Active");
    let (twin, _) =
        OutputSegment::emit_as(tentative.segment_id(), &generation, 1, "twice".to_owned())
            .expect("the generation is Active");
    let (first, _) = tentative.commit().expect("commits");
    let (second, _) = twin
        .commit()
        .expect("a second value of one identity commits");
    let id = first.segment_id();
    let mut x = taken(Vec::new(), vec![first]);
    let mut y = taken(Vec::new(), vec![second]);
    let of_x = frames_of(x.complete(id, vec![frame(), frame(), frame()]).expect("X"));
    drop(frames_of(y.complete(id, vec![frame()]).expect("Y")));
    assert_eq!(
        y.acknowledge_playback(id, PlaybackReceipt::from_frames(of_x).expect("three")),
        Err(PlaybackRefused::PlayedFramesMismatch(
            PlayedFramesMismatch {
                segment_id: id,
                played: 3,
                emitted: 1,
            }
        ))
    );
    assert_eq!(state(&y, id), Some(SegmentState::Synthesized));
}
