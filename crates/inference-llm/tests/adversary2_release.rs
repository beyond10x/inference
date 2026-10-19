//! `inference-llm`'s `AdmittedInput`: "the text inside is reachable only through
//! `GenerationOutput::release` of the generation that admitted it, while that generation is still
//! Active", its `Debug` form "shows the admitting generation, not the text", and `release` refuses
//! another generation's input before it looks at the state. `CommitSegment`
//! (`ess/domains/stream.yaml`) refuses only on the segment's state, so a stale generation, whose
//! segments are all Discarded, commits nothing.

use std::marker::PhantomData;

use inference_core::generation::{Generation, GenerationState, GenerationStateConflict};
use inference_core::id::{NodeId, RunId};
use inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_llm::{
    AdmittedByAnotherGeneration, AdmittedInput, CONTRACT_REVISION, GenerationOutput,
    GenerationRequest, InputRefused,
};
use inference_text::{OutputSegment, SegmentState};

const SECRET: &str = "admitted text 7f3a";

fn started() -> GenerationOutput {
    let adapter = Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: "test".to_owned(),
        contract_revision: CONTRACT_REVISION.to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities: vec![
            Capability::OutputStreaming,
            Capability::ConcurrentInputOutput,
        ],
        max_concurrency: 1,
    })
    .expect("one is positive")
    .0;
    let request = GenerationRequest {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
        prompt: "prompt".to_owned(),
    };
    GenerationOutput::start(&adapter, &request).0
}

/// Everything a `GenerationOutput` holds; it is not `Clone`.
fn snapshot(output: &GenerationOutput) -> (Generation, Vec<OutputSegment>, Vec<Capability>) {
    (
        output.generation().clone(),
        output.segments().to_vec(),
        output.capabilities().to_vec(),
    )
}

fn admit(output: &GenerationOutput, text: &str) -> AdmittedInput {
    output
        .admit_input(text.to_owned())
        .expect("a claiming adapter admits input while Active")
}

#[test]
fn input_admitted_before_supersede_or_cancel_is_not_released_after() {
    for supersede in [true, false] {
        let mut output = started();
        let input = admit(&output, SECRET);
        let state = if supersede {
            output.supersede().expect("Active is superseded");
            GenerationState::Superseded
        } else {
            output.cancel().expect("Active is cancelled");
            GenerationState::Cancelled
        };
        assert_eq!(
            output.release(input),
            Err(InputRefused::GenerationStateConflict(
                GenerationStateConflict { current: state }
            )),
            "a {state:?} generation releases nothing"
        );
    }
}

#[test]
fn another_generations_input_is_refused_as_such_even_when_this_one_is_not_active() {
    let admitting = started();
    let mut other = started();
    other.complete().expect("Active completes");
    let input = admit(&admitting, SECRET);
    assert_eq!(
        other.release(input),
        Err(InputRefused::AdmittedByAnotherGeneration(
            AdmittedByAnotherGeneration {
                admitted_by: admitting.generation().generation_id(),
                offered_to: other.generation().generation_id(),
            }
        )),
        "the admitting generation is checked before the state, as release documents"
    );
}

#[test]
fn debug_shows_the_admitting_generation_and_never_the_text() {
    let output = started();
    let input = admit(&output, SECRET);
    let id = output.generation().generation_id().to_string();
    for shown in [
        format!("{input:?}"),
        format!("{input:#?}"),
        format!("{:?}", Ok::<&AdmittedInput, InputRefused>(&input)),
    ] {
        assert!(!shown.contains(SECRET), "Debug leaks the text: {shown}");
        assert!(shown.contains(&id), "Debug names no generation: {shown}");
    }
}

struct Probe<T>(PhantomData<T>);

trait NotComparable {
    fn comparable(&self) -> bool {
        false
    }
}

impl<T> NotComparable for Probe<T> {}

impl<T: PartialEq> Probe<T> {
    fn comparable(&self) -> bool {
        true
    }
}

// `AdmittedInput` is not `PartialEq`, so two admissions cannot be compared at all; the
// `compile_fail` example on `AdmittedInput` pins it, and the probe below observes it.
#[test]
fn the_text_is_not_observable_by_comparing_admissions() {
    struct Opaque;
    assert!(
        !Probe::<Opaque>(PhantomData).comparable() && Probe::<String>(PhantomData).comparable(),
        "the probe tells PartialEq types apart"
    );
    let oracle = Probe::<AdmittedInput>(PhantomData).comparable();
    assert!(
        !oracle,
        "AdmittedInput is PartialEq, so a holder of the handle confirms a guessed text without \
         release"
    );
}

// A copy of the output taken while Active cannot be made: `GenerationOutput` is not `Clone`,
// pinned by the `compile_fail` example on it. The output itself refuses after supersede:
// `input_admitted_before_supersede_or_cancel_is_not_released_after`.

#[test]
fn a_superseded_or_cancelled_generation_commits_nothing() {
    for supersede in [true, false] {
        let mut output = started();
        output.emit("a".to_owned()).expect("Active takes a delta");
        output.commit_tentative().expect("Tentative commits");
        output.emit("b".to_owned()).expect("Active takes a delta");
        if supersede {
            output.supersede().expect("Active is superseded");
        } else {
            output.cancel().expect("Active is cancelled");
        }
        let before = snapshot(&output);
        assert_eq!(output.commit_tentative(), Ok(Vec::new()));
        assert_eq!(
            snapshot(&output),
            before,
            "commit after discard changes nothing"
        );
        assert!(
            output
                .segments()
                .iter()
                .all(|segment| segment.state() == SegmentState::Discarded)
        );
    }
}
