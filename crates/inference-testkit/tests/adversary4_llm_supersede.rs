//! The mock generator's `supersede` and `cancel`, which the unit's suite never calls. The
//! `GenerationHandle` contract: "Runs `inference.stream.SupersedeGeneration`; nothing more is
//! delivered", and `ess/domains/stream.yaml` discards every unplayed segment of the generation in
//! the same outcome. A streaming generation interrupted between a `TextDelta` and the
//! `TextSegmentCommitted` queued behind it delivers neither that commit nor anything after it.

#![cfg(feature = "llm")]

use inference_core::generation::GenerationState;
use inference_core::id::{NodeId, RunId};
use inference_core::model::ModelId;
use inference_llm::llm_core::{Item, TurnRequest};
use inference_llm::{GenerationEvent, GenerationRequest};
use inference_testkit::llm::{MockConfiguration, MockGenerator};
use inference_text::SegmentState;

fn interrupted(supersede: bool) {
    let mut handle = MockGenerator::new(
        MockConfiguration::StreamingOnly,
        ModelId::generate(),
        ["a", "b", "c"],
    )
    .start(GenerationRequest {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
        turn: TurnRequest::new("mock", vec![Item::user("prompt")]),
    })
    .expect("a fresh generator runs nothing else");
    assert!(matches!(handle.next(), Some(GenerationEvent::Started(_))));
    assert!(matches!(handle.next(), Some(GenerationEvent::TextDelta(_))));
    let generation_id = handle.output().generation().generation_id();

    let (state, id) = if supersede {
        let event = handle.supersede().expect("Active is superseded");
        (GenerationState::Superseded, event.generation_id)
    } else {
        let event = handle.cancel().expect("Active is cancelled");
        (GenerationState::Cancelled, event.generation_id)
    };

    assert_eq!(id, generation_id);
    assert_eq!(handle.output().generation().state(), state);
    assert!(
        handle
            .output()
            .segments()
            .iter()
            .all(|segment| segment.state() == SegmentState::Discarded),
        "{state:?}: every segment is Discarded: {:?}",
        handle.output().segments()
    );
    let after: Vec<GenerationEvent> = handle.by_ref().collect();
    assert_eq!(after, [], "{state:?}: nothing more is delivered");
    assert!(handle.supersede().is_err() && handle.cancel().is_err());
}

#[test]
fn a_superseded_mock_generation_delivers_nothing_more() {
    interrupted(true);
}

#[test]
fn a_cancelled_mock_generation_delivers_nothing_more() {
    interrupted(false);
}
