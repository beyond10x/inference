//! `inference.stream.CompleteGeneration` takes an optional `stop_reason`, and the
//! `GenerationCompleted` it emits carries it, serialized as written in ESS: a stop reason by its
//! ESS name, none as null. `inference.stream.RefuseGeneration` delivers a refusal into an Active
//! generation only, and moves it nowhere.

use inference_core::generation::{
    Generation, GenerationCompleted, GenerationRefused, GenerationState, GenerationStateConflict,
    StartGeneration, StopReason,
};

#[test]
fn a_refusal_is_delivered_into_an_active_generation_only() {
    let generation = active();
    assert_eq!(
        generation.refuse("No.".to_owned()),
        Ok(GenerationRefused {
            generation_id: generation.generation_id(),
            text: "No.".to_owned(),
        })
    );
    assert_eq!(generation.state(), GenerationState::Active);
    let ended = [
        generation.complete(None).expect("Active completes").0,
        generation.supersede().expect("Active is superseded").0,
        generation.cancel().expect("Active is cancelled").0,
    ];
    for ended in ended {
        assert_eq!(
            ended.refuse("No.".to_owned()),
            Err(GenerationStateConflict {
                current: ended.state()
            }),
            "a refusal into a generation that is not Active is stale"
        );
    }
}
use inference_core::id::{GenerationId, NodeId, RunId};
use serde_json::json;

fn active() -> Generation {
    Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    })
    .0
}

#[test]
fn completing_carries_the_stop_reason_it_is_given() {
    let generation = active();
    for stop_reason in StopReason::ALL.map(Some).into_iter().chain([None]) {
        let (completed, event) = generation.complete(stop_reason).expect("Active completes");
        assert_eq!(completed.state(), GenerationState::Completed);
        assert_eq!(
            event,
            GenerationCompleted {
                generation_id: generation.generation_id(),
                stop_reason,
            }
        );
        assert!(completed.complete(Some(StopReason::EndTurn)).is_err());
    }
}

#[test]
fn the_stop_reason_serializes_as_written_in_ess() {
    let names: Vec<serde_json::Value> = StopReason::ALL
        .iter()
        .map(|reason| serde_json::to_value(reason).expect("serializes"))
        .collect();
    assert_eq!(
        names,
        [
            json!("end_turn"),
            json!("tool_calls"),
            json!("max_output_tokens"),
            json!("incomplete")
        ]
    );
    let generation_id = GenerationId::generate();
    for (stop_reason, value) in [
        (None, json!(null)),
        (
            Some(StopReason::MaxOutputTokens),
            json!("max_output_tokens"),
        ),
    ] {
        let completed = GenerationCompleted {
            generation_id,
            stop_reason,
        };
        let serialized = serde_json::to_value(completed).expect("serializes");
        assert_eq!(
            serialized,
            json!({ "generation_id": generation_id, "stop_reason": value })
        );
        assert_eq!(
            serde_json::from_value::<GenerationCompleted>(serialized).expect("reads back"),
            completed
        );
    }
}

#[test]
fn a_refusal_serializes_with_its_generation_and_text() {
    let generation_id = GenerationId::generate();
    let refused = GenerationRefused {
        generation_id,
        text: "I cannot help with that.".to_owned(),
    };
    let value = serde_json::to_value(&refused).expect("serializes");
    assert_eq!(
        value,
        json!({ "generation_id": generation_id, "text": "I cannot help with that." })
    );
    assert_eq!(
        serde_json::from_value::<GenerationRefused>(value).expect("reads back"),
        refused
    );
}
