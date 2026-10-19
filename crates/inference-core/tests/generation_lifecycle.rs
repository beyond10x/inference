//! `inference.stream.Generation` follows the lifecycle of `ess/domains/stream.yaml`: it starts
//! Active, and `complete`, `supersede` and `cancel` each move it from Active only, to a terminal
//! state that refuses all three with the state it holds.

use inference_core::generation::{
    Generation, GenerationCancelled, GenerationCompleted, GenerationNotFound, GenerationStarted,
    GenerationState, GenerationStateConflict, GenerationSuperseded, StartGeneration,
};
use inference_core::id::{GenerationId, NodeId, RunId};
use serde_json::json;

fn start_input() -> StartGeneration {
    StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 3,
    }
}

fn active() -> Generation {
    Generation::start(start_input()).0
}

/// A generation in each terminal state, reached through its own transition.
fn terminal() -> [Generation; 3] {
    let generation = active();
    [
        generation.complete().expect("Active completes").0,
        generation.supersede().expect("Active is superseded").0,
        generation.cancel().expect("Active is cancelled").0,
    ]
}

#[test]
fn a_started_generation_is_active_and_its_event_carries_the_input() {
    let input = start_input();
    let (generation, event) = Generation::start(input);

    assert_eq!(generation.state(), GenerationState::Active);
    assert_eq!(generation.run_id(), input.run_id);
    assert_eq!(generation.node_id(), input.node_id);
    assert_eq!(generation.epoch(), 3);
    assert_eq!(
        event,
        GenerationStarted {
            generation_id: generation.generation_id(),
            run_id: input.run_id,
            epoch: 3,
        }
    );
}

#[test]
fn start_as_uses_the_identity_it_is_given() {
    let generation_id = GenerationId::generate();
    let (generation, event) = Generation::start_as(generation_id, start_input());

    assert_eq!(generation.generation_id(), generation_id);
    assert_eq!(event.generation_id, generation_id);
}

#[test]
fn each_transition_moves_an_active_generation_and_keeps_its_fields() {
    let generation = active();
    let id = generation.generation_id();

    let (completed, event) = generation.complete().expect("Active completes");
    assert_eq!(completed.state(), GenerationState::Completed);
    assert_eq!(event, GenerationCompleted { generation_id: id });

    let (superseded, event) = generation.supersede().expect("Active is superseded");
    assert_eq!(superseded.state(), GenerationState::Superseded);
    assert_eq!(event, GenerationSuperseded { generation_id: id });

    let (cancelled, event) = generation.cancel().expect("Active is cancelled");
    assert_eq!(cancelled.state(), GenerationState::Cancelled);
    assert_eq!(event, GenerationCancelled { generation_id: id });

    for moved in [completed, superseded, cancelled] {
        assert_eq!(moved.generation_id(), id);
        assert_eq!(moved.run_id(), generation.run_id());
        assert_eq!(moved.node_id(), generation.node_id());
        assert_eq!(moved.epoch(), generation.epoch());
    }
    assert_eq!(
        generation.state(),
        GenerationState::Active,
        "a move returns a new generation and leaves the one it read"
    );
}

#[test]
fn a_terminal_generation_refuses_every_transition_with_the_state_it_holds() {
    for generation in terminal() {
        let current = generation.state();
        let conflict = GenerationStateConflict { current };
        assert_eq!(
            generation.complete(),
            Err(conflict),
            "{current:?} completes"
        );
        assert_eq!(
            generation.supersede(),
            Err(conflict),
            "{current:?} supersedes"
        );
        assert_eq!(generation.cancel(), Err(conflict), "{current:?} cancels");
    }
}

#[test]
fn states_events_and_errors_serialize_as_the_ess_names() {
    assert_eq!(
        serde_json::to_value(GenerationState::ALL).expect("serializes"),
        json!(["Active", "Completed", "Superseded", "Cancelled"])
    );

    let generation_id = GenerationId::generate();
    let (generation, started) = Generation::start_as(generation_id, start_input());
    assert_eq!(
        serde_json::to_value(started).expect("serializes"),
        json!({
            "generation_id": generation_id.to_string(),
            "run_id": generation.run_id().to_string(),
            "epoch": 3,
        })
    );
    assert_eq!(
        serde_json::to_value(GenerationStateConflict {
            current: GenerationState::Superseded
        })
        .expect("serializes"),
        json!({"current": "Superseded"})
    );
    assert_eq!(
        serde_json::to_value(GenerationNotFound { generation_id }).expect("serializes"),
        json!({"generation_id": generation_id.to_string()})
    );
    assert_eq!(
        serde_json::to_value(&generation).expect("serializes")["state"],
        json!("Active")
    );
}
