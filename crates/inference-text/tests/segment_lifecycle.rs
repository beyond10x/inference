//! `inference.stream.OutputSegment` follows the lifecycle of `ess/domains/stream.yaml`: emitted
//! Tentative into an Active generation only, then `commit`, `queue`, `synthesize` and `play` each
//! from the one state before it, and `discard` from every state but Played and Discarded. Every
//! other pair of state and transition is refused with the state the segment holds.

use inference_core::generation::{
    Generation, GenerationState, GenerationStateConflict, StartGeneration,
};
use inference_core::id::{NodeId, RunId};
use inference_text::{
    AudioPlayed, OutputSegment, SegmentDiscarded, SegmentId, SegmentQueued, SegmentState,
    SegmentStateConflict, SynthesisCompleted, TextDelta, TextSegmentCommitted,
};
use serde_json::json;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Move {
    Commit,
    Queue,
    Synthesize,
    Play,
    Discard,
}

/// The `from` states of each transition, as `ess/domains/stream.yaml` declares them.
fn sources(transition: Move) -> &'static [SegmentState] {
    match transition {
        Move::Commit => &[SegmentState::Tentative],
        Move::Queue => &[SegmentState::Committed],
        Move::Synthesize => &[SegmentState::Queued],
        Move::Play => &[SegmentState::Synthesized],
        Move::Discard => &[
            SegmentState::Tentative,
            SegmentState::Committed,
            SegmentState::Queued,
            SegmentState::Synthesized,
        ],
    }
}

fn target(transition: Move) -> SegmentState {
    match transition {
        Move::Commit => SegmentState::Committed,
        Move::Queue => SegmentState::Queued,
        Move::Synthesize => SegmentState::Synthesized,
        Move::Play => SegmentState::Played,
        Move::Discard => SegmentState::Discarded,
    }
}

fn apply(segment: &OutputSegment, transition: Move) -> Result<OutputSegment, SegmentStateConflict> {
    match transition {
        Move::Commit => segment.commit().map(|(moved, _)| moved),
        Move::Queue => segment.queue().map(|(moved, _)| moved),
        Move::Synthesize => segment.synthesize().map(|(moved, _)| moved),
        Move::Play => segment.play().map(|(moved, _)| moved),
        Move::Discard => segment.discard().map(|(moved, _)| moved),
    }
}

fn generation() -> Generation {
    Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    })
    .0
}

fn tentative() -> OutputSegment {
    OutputSegment::emit(&generation(), 7, "hello".to_owned())
        .expect("an Active generation admits a delta")
        .0
}

/// A segment in `state`, reached through the declared transitions.
fn in_state(state: SegmentState) -> OutputSegment {
    let path: &[Move] = match state {
        SegmentState::Tentative => &[],
        SegmentState::Committed => &[Move::Commit],
        SegmentState::Queued => &[Move::Commit, Move::Queue],
        SegmentState::Synthesized => &[Move::Commit, Move::Queue, Move::Synthesize],
        SegmentState::Played => &[Move::Commit, Move::Queue, Move::Synthesize, Move::Play],
        SegmentState::Discarded => &[Move::Discard],
    };
    path.iter().fold(tentative(), |segment, transition| {
        apply(&segment, *transition).expect("a declared path")
    })
}

#[test]
fn a_delta_into_an_active_generation_is_a_tentative_segment_carrying_its_generation_and_run() {
    let generation = generation();
    let (segment, event) =
        OutputSegment::emit(&generation, 7, "hello".to_owned()).expect("Active admits a delta");

    assert_eq!(segment.state(), SegmentState::Tentative);
    assert_eq!(segment.generation_id(), generation.generation_id());
    assert_eq!(segment.run_id(), generation.run_id());
    assert_eq!(segment.sequence(), 7);
    assert_eq!(segment.text(), "hello");
    assert_eq!(
        event,
        TextDelta {
            segment_id: segment.segment_id(),
            generation_id: generation.generation_id(),
            sequence: 7,
            text: "hello".to_owned(),
        }
    );

    let segment_id = SegmentId::generate();
    let (segment, event) = OutputSegment::emit_as(segment_id, &generation, 8, "again".to_owned())
        .expect("Active admits a delta");
    assert_eq!(segment.segment_id(), segment_id);
    assert_eq!(event.segment_id, segment_id);
}

#[test]
fn a_delta_into_a_generation_that_is_not_active_is_refused_with_its_state() {
    let active = generation();
    let stale = [
        active.complete().expect("completes").0,
        active.supersede().expect("is superseded").0,
        active.cancel().expect("is cancelled").0,
    ];
    for generation in stale {
        let current = generation.state();
        assert_ne!(current, GenerationState::Active);
        assert_eq!(
            OutputSegment::emit(&generation, 1, "stale".to_owned()),
            Err(GenerationStateConflict { current })
        );
    }
}

#[test]
fn every_state_and_transition_pair_moves_or_refuses_as_declared() {
    let moves = [
        Move::Commit,
        Move::Queue,
        Move::Synthesize,
        Move::Play,
        Move::Discard,
    ];
    let mut moved = 0;
    let mut refused = 0;
    for state in SegmentState::ALL {
        let segment = in_state(state);
        assert_eq!(segment.state(), state);
        for transition in moves {
            let answer = apply(&segment, transition);
            if sources(transition).contains(&state) {
                let after = answer.unwrap_or_else(|conflict| {
                    panic!("{transition:?} from {state:?} is declared, got {conflict:?}")
                });
                assert_eq!(
                    after.state(),
                    target(transition),
                    "{transition:?} from {state:?}"
                );
                assert_eq!(after.segment_id(), segment.segment_id());
                assert_eq!(after.text(), segment.text());
                moved += 1;
            } else {
                assert_eq!(
                    answer,
                    Err(SegmentStateConflict { current: state }),
                    "{transition:?} from {state:?} is not declared"
                );
                refused += 1;
            }
            assert_eq!(segment.state(), state, "a move leaves the segment it read");
        }
    }
    assert_eq!((moved, refused), (8, 22));
}

#[test]
fn the_named_refusals_of_the_design() {
    assert_eq!(
        in_state(SegmentState::Queued).play(),
        Err(SegmentStateConflict {
            current: SegmentState::Queued
        }),
        "a queued segment is not played before its synthesis completes"
    );
    assert_eq!(
        in_state(SegmentState::Played).discard(),
        Err(SegmentStateConflict {
            current: SegmentState::Played
        }),
        "speech already played cannot be unsaid"
    );
    assert_eq!(
        in_state(SegmentState::Tentative).queue(),
        Err(SegmentStateConflict {
            current: SegmentState::Tentative
        }),
        "synthesis consumes committed segments"
    );
}

#[test]
fn each_transition_emits_its_event() {
    let segment = tentative();
    let id = segment.segment_id();
    let (committed, event) = segment.commit().expect("commits");
    assert_eq!(
        event,
        TextSegmentCommitted {
            segment_id: id,
            text: "hello".to_owned()
        }
    );
    let (queued, event) = committed.queue().expect("queues");
    assert_eq!(event, SegmentQueued { segment_id: id });
    let (synthesized, event) = queued.synthesize().expect("synthesizes");
    assert_eq!(event, SynthesisCompleted { segment_id: id });
    let (_, event) = synthesized.play().expect("plays");
    assert_eq!(event, AudioPlayed { segment_id: id });
    let (_, event) = synthesized.discard().expect("discards");
    assert_eq!(event, SegmentDiscarded { segment_id: id });
}

#[test]
fn states_events_and_identities_serialize_as_the_ess_names() {
    assert_eq!(
        serde_json::to_value(SegmentState::ALL).expect("serializes"),
        json!([
            "Tentative",
            "Committed",
            "Queued",
            "Synthesized",
            "Played",
            "Discarded"
        ])
    );
    let segment = tentative();
    let wire = serde_json::to_value(&segment).expect("serializes");
    assert_eq!(wire["state"], json!("Tentative"));
    assert_eq!(wire["segment_id"], json!(segment.segment_id().to_string()));
    assert_eq!(
        serde_json::to_value(SegmentStateConflict {
            current: SegmentState::Played
        })
        .expect("serializes"),
        json!({"current": "Played"})
    );

    let upper: SegmentId = serde_json::from_value(json!("0A1B2C3D-4E5F-4A6B-8C7D-9E0F1A2B3C4D"))
        .expect("the uppercase canonical form");
    assert_eq!(
        serde_json::to_value(upper).expect("serializes"),
        json!("0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d")
    );
    for refused in [
        "urn:uuid:0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d",
        "{0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d}",
        "0a1b2c3d4e5f4a6b8c7d9e0f1a2b3c4d",
        " 0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d",
    ] {
        assert!(
            serde_json::from_value::<SegmentId>(json!(refused)).is_err(),
            "`{refused}` is not the canonical spelling"
        );
    }
}
