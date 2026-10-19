//! `inference-llm`'s crate documentation: "A handle takes input only as the `AdmittedInput` that
//! admission returns, so no implementation accepts input its adapter does not claim." Admission is
//! not bound to the generation that admitted it: input admitted by a generation whose adapter
//! claims `concurrent_input_output` is taken by a handle whose adapter claims `output_streaming`
//! only, and the generation reflects it in a delta.

#![cfg(feature = "llm")]

use inference_core::id::{NodeId, RunId};
use inference_core::model::{Capability, ModelId};
use inference_llm::{
    GenerationEvent, GenerationHandle, GenerationRequest, Generator, InputRefused,
};
use inference_testkit::llm::{MockConfiguration, MockGenerator};

const INPUT: &str = "smuggled input";

fn request() -> GenerationRequest {
    GenerationRequest {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
        prompt: "prompt".to_owned(),
    }
}

fn start(configuration: MockConfiguration) -> <MockGenerator as Generator>::Handle {
    MockGenerator::new(configuration, ModelId::generate(), ["a", "b"]).start(request())
}

#[test]
fn input_admitted_by_another_generation_is_not_taken_by_an_adapter_not_claiming_it() {
    let claiming = start(MockConfiguration::StreamingAndConcurrentInput);
    let admitted = claiming
        .output()
        .admit_input(INPUT.to_owned())
        .expect("the claiming generation admits input while Active");

    let mut refusing = start(MockConfiguration::StreamingOnly);
    assert!(
        !refusing
            .output()
            .capabilities()
            .contains(&Capability::ConcurrentInputOutput)
    );
    assert!(matches!(refusing.next(), Some(GenerationEvent::Started(_))));
    assert!(
        matches!(
            refusing.accept_input(admitted),
            Err(InputRefused::AdmittedByAnotherGeneration(_))
        ),
        "input admitted by another generation is refused with a typed error"
    );

    let deltas: Vec<String> = refusing
        .filter_map(|event| match event {
            GenerationEvent::TextDelta(delta) => Some(delta.text),
            _ => None,
        })
        .collect();
    assert!(
        !deltas.iter().any(|text| text == INPUT),
        "an adapter claiming output_streaming only took input during output: {deltas:?}"
    );
}

#[test]
fn one_admission_is_taken_once() {
    let mut handle = start(MockConfiguration::StreamingAndConcurrentInput);
    assert!(matches!(handle.next(), Some(GenerationEvent::Started(_))));
    let admitted = handle
        .output()
        .admit_input(INPUT.to_owned())
        .expect("the claiming generation admits input while Active");
    assert_eq!(handle.accept_input(admitted), Ok(()));
    // A second take does not compile: `AdmittedInput` is not `Clone` and acceptance consumes
    // it (the `compile_fail` example on `inference_llm::AdmittedInput` pins that).

    let reflected = handle
        .filter(|event| matches!(event, GenerationEvent::TextDelta(delta) if delta.text == INPUT))
        .count();
    assert_eq!(reflected, 1, "one admission was accepted {reflected} times");
}
