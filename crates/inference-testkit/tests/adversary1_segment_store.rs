//! Adversary pass 1 of `story:segment-linear-transitions`, against the `inference.stream` store of
//! the testkit: a transition of `OutputSegment` consumes the segment, so the store takes it out of
//! its slot and puts the moved one, or the refused one, back. No refusal loses, duplicates or
//! reorders a stored segment, and a refused command records no event.

use inference_core::generation::StartGeneration;
use inference_core::id::{GenerationId, NodeId, RunId};
use inference_testkit::stream::{SegmentMoveError, StreamDomain};
use inference_text::{EmitTextDelta, SegmentId, SegmentState, SegmentStateConflict};
use uuid::Uuid;

const RUN: u128 = 0x0a1b_2c3d_4e5f_4a6b_8c7d_9e0f_1a2b_3c4d;

fn start(domain: &mut StreamDomain, epoch: i64) -> GenerationId {
    domain
        .start_generation(StartGeneration {
            run_id: RunId::from(Uuid::from_u128(RUN)),
            node_id: NodeId::from(Uuid::from_u128(0x1a1b_2c3d_4e5f_4a6b_8c7d_9e0f_1a2b_3c4d)),
            epoch,
        })
        .generation_id
}

fn emit(domain: &mut StreamDomain, generation_id: GenerationId, sequence: i64) -> SegmentId {
    domain
        .emit_text_delta(EmitTextDelta {
            generation_id,
            sequence,
            text: format!("t{sequence}"),
        })
        .expect("an Active generation admits a delta")
        .segment_id
}

/// A segment command expected to be refused, and its refusal.
type RefusedMove = fn(&mut StreamDomain, SegmentId) -> Option<SegmentMoveError>;

/// The stored segments in store order, each with its state.
fn stored(domain: &StreamDomain) -> Vec<(SegmentId, SegmentState)> {
    domain
        .segments()
        .iter()
        .map(|segment| (segment.segment_id(), segment.state()))
        .collect()
}

#[test]
fn a_refused_move_keeps_the_segment_in_its_slot_and_records_nothing() {
    let mut domain = StreamDomain::new();
    let generation = start(&mut domain, 1);
    let ids: Vec<SegmentId> = (1..=4)
        .map(|sequence| emit(&mut domain, generation, sequence))
        .collect();
    domain.commit_segment(ids[1]).expect("commits");
    domain.commit_segment(ids[2]).expect("commits");
    domain.queue_for_synthesis(ids[2]).expect("queues");

    let refusals: [(SegmentId, RefusedMove, SegmentState); 5] = [
        (
            ids[0],
            |d, id| d.queue_for_synthesis(id).err(),
            SegmentState::Tentative,
        ),
        (
            ids[1],
            |d, id| d.commit_segment(id).err(),
            SegmentState::Committed,
        ),
        (
            ids[2],
            |d, id| d.acknowledge_playback(id).err(),
            SegmentState::Queued,
        ),
        (
            ids[3],
            |d, id| d.complete_synthesis(id).err(),
            SegmentState::Tentative,
        ),
        (
            ids[1],
            |d, id| d.acknowledge_playback(id).err(),
            SegmentState::Committed,
        ),
    ];
    for (id, refused_move, current) in refusals {
        let before = stored(&domain);
        let events = domain.events().len();
        assert_eq!(
            refused_move(&mut domain, id),
            Some(SegmentMoveError::Conflict(SegmentStateConflict { current })),
            "{id:?} in {current:?}"
        );
        assert_eq!(stored(&domain), before, "a refused move changes no slot");
        assert_eq!(
            domain.events().len(),
            events,
            "a refused move records no event"
        );
    }

    domain.complete_synthesis(ids[2]).expect("synthesizes");
    domain.acknowledge_playback(ids[2]).expect("plays");
    let before = stored(&domain);
    assert_eq!(
        domain.discard_segment(ids[2]).err(),
        Some(SegmentMoveError::Conflict(SegmentStateConflict {
            current: SegmentState::Played
        }))
    );
    assert_eq!(
        stored(&domain),
        before,
        "a refused discard keeps the Played segment in place"
    );
    domain
        .discard_segment(ids[0])
        .expect("a Tentative segment discards");
    assert_eq!(
        stored(&domain),
        vec![
            (ids[0], SegmentState::Discarded),
            (ids[1], SegmentState::Committed),
            (ids[2], SegmentState::Played),
            (ids[3], SegmentState::Tentative),
        ],
        "a move keeps every segment in emission order"
    );
}

#[test]
fn cancelling_the_run_or_superseding_keeps_played_and_discarded_segments_in_place() {
    for cancel_run in [true, false] {
        let mut domain = StreamDomain::new();
        let generation = start(&mut domain, 1);
        let ids: Vec<SegmentId> = (1..=4)
            .map(|sequence| emit(&mut domain, generation, sequence))
            .collect();
        domain.discard_segment(ids[0]).expect("discards");
        domain.commit_segment(ids[1]).expect("commits");
        domain.queue_for_synthesis(ids[1]).expect("queues");
        domain.complete_synthesis(ids[1]).expect("synthesizes");
        domain.acknowledge_playback(ids[1]).expect("plays");
        domain.commit_segment(ids[3]).expect("commits");

        if cancel_run {
            domain.cancel_run(RunId::from(Uuid::from_u128(RUN)));
        } else {
            domain
                .supersede_generation(generation)
                .expect("Active supersedes");
        }
        assert_eq!(
            stored(&domain),
            vec![
                (ids[0], SegmentState::Discarded),
                (ids[1], SegmentState::Played),
                (ids[2], SegmentState::Discarded),
                (ids[3], SegmentState::Discarded),
            ],
            "cancel_run {cancel_run}: the refused discards of a Played and a Discarded segment keep them, in order"
        );
    }
}
