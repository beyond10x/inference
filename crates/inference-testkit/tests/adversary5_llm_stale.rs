//! The mock generation after `SupersedeGeneration`: `ess/domains/stream.yaml` says "a stale
//! generation admits no new output", and `GenerationOutput::release` refuses a generation no
//! longer Active. Input admitted before the generation was superseded is refused by its handle,
//! and input offered after it is refused and delivers no output for the superseded generation.
//! A copy of the handle taken while Active cannot be made: `MockGeneration` is not `Clone`,
//! pinned by the `compile_fail` example on it.

#![cfg(feature = "llm")]

use inference_core::generation::{GenerationState, GenerationStateConflict};
use inference_core::id::{NodeId, RunId};
use inference_core::model::ModelId;
use inference_llm::llm_core::{Item, TurnRequest};
use inference_llm::{GenerationEvent, GenerationRequest, InputRefused};
use inference_testkit::llm::{MockConfiguration, MockGeneration, MockGenerator};

const INPUT: &str = "late input";

fn start() -> MockGeneration {
    MockGenerator::new(
        MockConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        ["a", "b"],
    )
    .start(GenerationRequest {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
        turn: TurnRequest::new("mock", vec![Item::user("prompt")]),
    })
    .expect("a fresh generator runs nothing else")
}

#[test]
fn input_admitted_before_supersede_or_cancel_is_refused_after_and_never_delivered() {
    for supersede in [true, false] {
        let mut handle = start();
        assert!(matches!(handle.next(), Some(GenerationEvent::Started(_))));
        let input = handle
            .output()
            .admit_input(INPUT.to_owned())
            .expect("the claiming generation admits input while Active");
        let state = if supersede {
            handle.supersede().expect("Active is superseded");
            GenerationState::Superseded
        } else {
            handle.cancel().expect("Active is cancelled");
            GenerationState::Cancelled
        };
        assert_eq!(
            handle.accept_input(input),
            Err(InputRefused::GenerationStateConflict(
                GenerationStateConflict { current: state }
            ))
        );
        let after: Vec<GenerationEvent> = handle.by_ref().collect();
        assert!(
            matches!(after.as_slice(), [GenerationEvent::Usage(recorded)] if recorded.r#final),
            "{state:?}: no output is delivered, only the usage record of the turn that ended: \
             {after:?}"
        );
    }
}

#[test]
fn a_superseded_handle_delivers_no_output_for_the_superseded_generation() {
    let mut handle = start();
    assert!(matches!(handle.next(), Some(GenerationEvent::Started(_))));
    let superseded = handle
        .supersede()
        .expect("Active is superseded")
        .generation_id;

    let offered = handle.offer_input(INPUT.to_owned());
    let stale: Vec<String> = handle
        .filter_map(|event| match event {
            GenerationEvent::TextDelta(delta) if delta.generation_id == superseded => {
                Some(delta.text)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        (offered, stale),
        (
            Err(InputRefused::GenerationStateConflict(
                GenerationStateConflict {
                    current: GenerationState::Superseded
                }
            )),
            Vec::<String>::new()
        ),
        "a stale generation admits no new output"
    );
}
