//! Adversary pass 1 of `story:segment-linear-transitions`: the transitions of `OutputSegment`
//! consume the segment and a refused one hands it back in `SegmentMoveRefused`.
//!
//! The `compile_fail,E0382` doctest on `OutputSegment::commit` proves its point only if the
//! snippet fails on the second commit and nowhere else; error codes of a `compile_fail` doctest
//! are checked on nightly only, so on stable any compile error passes it. The guard here is the
//! same snippet without the second commit, which must compile and run.

use std::error::Error;

use inference_core::generation::{Generation, StartGeneration};
use inference_core::id::{NodeId, RunId};
use inference_text::{OutputSegment, SegmentState, SegmentStateConflict};

/// Guard: the doctest on `OutputSegment::commit`, its second commit left out.
#[test]
fn the_commit_twice_doctest_compiles_without_its_second_commit() {
    let (generation, _) = Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    });
    let (tentative, _) = OutputSegment::emit(&generation, 1, "hello".to_owned()).unwrap();
    let (first, _) = tentative.commit().unwrap();
    assert_eq!(first.state(), SegmentState::Committed);
}

/// Guard: a refusal reads as the conflict it carries, names it as its source, and hands back the
/// segment by reference and by value as it was.
#[test]
fn a_refusal_reads_as_its_conflict_and_hands_back_the_segment() {
    let (generation, _) = Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    });
    let (tentative, _) = OutputSegment::emit(&generation, 7, "held".to_owned()).unwrap();
    let before = tentative.snapshot();
    let refused = tentative
        .play()
        .expect_err("a Tentative segment is not played");
    let conflict = SegmentStateConflict {
        current: SegmentState::Tentative,
    };
    assert_eq!(refused.conflict(), conflict);
    assert_eq!(refused.to_string(), conflict.to_string());
    assert_eq!(
        refused
            .source()
            .and_then(|source| source.downcast_ref::<SegmentStateConflict>()),
        Some(&conflict)
    );
    assert_eq!(refused.segment().snapshot(), before);
    assert_eq!(SegmentStateConflict::from(refused), conflict);

    let (tentative, _) = OutputSegment::emit(&generation, 8, "again".to_owned()).unwrap();
    let before = tentative.snapshot();
    let back = tentative
        .queue()
        .expect_err("a Tentative segment is not queued")
        .into_segment();
    assert_eq!(back.snapshot(), before);
    let (committed, _) = back
        .commit()
        .expect("the segment handed back still commits");
    assert_eq!(committed.segment_id(), before.segment_id());
    assert_eq!(committed.sequence(), 8);
}
