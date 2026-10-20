//! `GenerationOutput` holds one generation and its segments under the transitions of
//! `inference-core` and `inference-text`: input is admitted into an Active generation only when
//! the adapter claims `concurrent_input_output` and released only to that generation while it is
//! still Active, segments are numbered from 1 and emitted only while the generation is Active, a
//! Tentative segment commits whatever the generation's state, as `CommitSegment` declares, and
//! superseding or cancelling discards every segment it owns.

use inference_core::generation::{Generation, GenerationState, GenerationStateConflict};
use inference_core::id::{NodeId, RunId};
use inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_llm::llm_core::{Item, TurnRequest};
use inference_llm::{
    AdmittedByAnotherGeneration, CONTRACT_REVISION, CapabilityNotClaimed, GenerationOutput,
    GenerationRequest, InputRefused,
};
use inference_text::{OutputSegment, SegmentSnapshot, SegmentState};

fn adapter(capabilities: Vec<Capability>) -> Adapter {
    Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: "test".to_owned(),
        contract_revision: CONTRACT_REVISION.to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities,
        max_concurrency: 1,
    })
    .expect("one is positive")
    .0
}

fn request() -> GenerationRequest {
    GenerationRequest {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 2,
        turn: TurnRequest::new("model", vec![Item::user("prompt")]),
    }
}

/// Everything a `GenerationOutput` holds, for comparing it before and after a move; the output
/// itself is not `Clone`.
fn snapshot(output: &GenerationOutput) -> (Generation, Vec<SegmentSnapshot>, Vec<Capability>) {
    (
        output.generation().clone(),
        output
            .segments()
            .iter()
            .map(OutputSegment::snapshot)
            .collect(),
        output.capabilities().to_vec(),
    )
}

fn started(capabilities: Vec<Capability>) -> GenerationOutput {
    GenerationOutput::start(&adapter(capabilities), &request()).0
}

#[test]
fn start_carries_the_request_and_the_adapter_claims() {
    let request = request();
    let (output, event) =
        GenerationOutput::start(&adapter(vec![Capability::OutputStreaming]), &request);
    let generation = output.generation();
    assert_eq!(generation.state(), GenerationState::Active);
    assert_eq!(generation.run_id(), request.run_id);
    assert_eq!(generation.node_id(), request.node_id);
    assert_eq!(generation.epoch(), 2);
    assert_eq!(event.generation_id, generation.generation_id());
    assert_eq!(output.capabilities(), [Capability::OutputStreaming]);
}

#[test]
fn input_while_active_is_admitted_only_with_concurrent_input_output() {
    let claimed = started(vec![Capability::ConcurrentInputOutput]);
    let input = claimed
        .admit_input("more".to_owned())
        .expect("a claiming adapter admits input while Active");
    assert_eq!(input.generation_id(), claimed.generation().generation_id());
    assert_eq!(claimed.release(input), Ok("more".to_owned()));

    for capabilities in [
        Vec::new(),
        vec![Capability::OutputStreaming],
        vec![Capability::IncrementalInput, Capability::NativeDuplex],
    ] {
        let output = started(capabilities.clone());
        assert_eq!(
            output.admit_input("more".to_owned()).err(),
            Some(InputRefused::CapabilityNotClaimed(CapabilityNotClaimed {
                capability: Capability::ConcurrentInputOutput,
            })),
            "claiming {capabilities:?}, input while Active is refused naming the missing capability"
        );
    }
}

#[test]
fn input_to_a_generation_that_is_not_active_is_refused_with_its_state() {
    for capabilities in [Vec::new(), vec![Capability::ConcurrentInputOutput]] {
        let mut completed = started(capabilities.clone());
        completed.complete().expect("Active completes");
        let mut cancelled = started(capabilities.clone());
        cancelled.cancel().expect("Active is cancelled");
        let mut superseded = started(capabilities.clone());
        superseded.supersede().expect("Active is superseded");
        for (output, state) in [
            (completed, GenerationState::Completed),
            (cancelled, GenerationState::Cancelled),
            (superseded, GenerationState::Superseded),
        ] {
            assert_eq!(
                output.admit_input("late".to_owned()).err(),
                Some(InputRefused::GenerationStateConflict(
                    GenerationStateConflict { current: state }
                )),
                "claiming {capabilities:?}, input to a {state:?} generation is refused with its state"
            );
        }
    }
}

#[test]
fn admitted_input_is_released_only_to_the_generation_that_admitted_it() {
    let admitting = started(vec![Capability::ConcurrentInputOutput]);
    let other = started(vec![Capability::ConcurrentInputOutput]);
    let input = admitting
        .admit_input("more".to_owned())
        .expect("a claiming adapter admits input while Active");
    assert_eq!(
        other.release(input),
        Err(InputRefused::AdmittedByAnotherGeneration(
            AdmittedByAnotherGeneration {
                admitted_by: admitting.generation().generation_id(),
                offered_to: other.generation().generation_id(),
            }
        ))
    );
}

#[test]
fn admitted_input_is_not_released_once_the_generation_is_not_active() {
    let mut completed = started(vec![Capability::ConcurrentInputOutput]);
    let input = completed
        .admit_input("more".to_owned())
        .expect("a claiming adapter admits input while Active");
    completed.complete().expect("Active completes");
    assert_eq!(
        completed.release(input),
        Err(InputRefused::GenerationStateConflict(
            GenerationStateConflict {
                current: GenerationState::Completed
            }
        ))
    );
}

#[test]
fn segments_are_numbered_from_one_emitted_while_active_only_and_committed_after_completion() {
    let mut output = started(Vec::new());
    let first = output.emit("a".to_owned()).expect("Active takes a delta");
    let second = output.emit("b".to_owned()).expect("Active takes a delta");
    assert_eq!((first.sequence, second.sequence), (1, 2));
    let committed = output.commit_tentative().expect("Active commits");
    assert_eq!(
        committed
            .iter()
            .map(|event| event.segment_id)
            .collect::<Vec<_>>(),
        [first.segment_id, second.segment_id]
    );
    assert!(
        output
            .commit_tentative()
            .expect("Active commits")
            .is_empty(),
        "a committed segment is not committed twice"
    );
    let third = output.emit("c".to_owned()).expect("Active takes a delta");
    output.complete().expect("Active completes");
    let before = snapshot(&output);
    assert_eq!(
        output.emit("d".to_owned()),
        Err(GenerationStateConflict {
            current: GenerationState::Completed
        })
    );
    assert_eq!(snapshot(&output), before, "a refused move changes nothing");
    let committed = output
        .commit_tentative()
        .expect("CommitSegment has no generation-state guard");
    assert_eq!(
        committed
            .iter()
            .map(|event| event.segment_id)
            .collect::<Vec<_>>(),
        [third.segment_id],
        "the Tentative segment of the Completed generation commits, and only it"
    );
    assert!(
        output
            .segments()
            .iter()
            .all(|segment| segment.state() == SegmentState::Committed),
        "every segment is Committed: {:?}",
        output.segments()
    );
}

#[test]
fn superseding_or_cancelling_discards_every_segment() {
    for supersede in [true, false] {
        let mut output = started(Vec::new());
        let first = output.emit("a".to_owned()).expect("Active takes a delta");
        output.commit_tentative().expect("Active commits");
        let second = output.emit("b".to_owned()).expect("Active takes a delta");
        let discarded = if supersede {
            output.supersede().expect("Active is superseded").1
        } else {
            output.cancel().expect("Active is cancelled").1
        };
        assert_eq!(
            discarded
                .iter()
                .map(|event| event.segment_id)
                .collect::<Vec<_>>(),
            [first.segment_id, second.segment_id]
        );
        assert!(
            output
                .segments()
                .iter()
                .all(|segment| segment.state() == SegmentState::Discarded),
            "every segment is Discarded: {:?}",
            output.segments()
        );
        let before = snapshot(&output);
        assert!(output.cancel().is_err() && output.supersede().is_err());
        assert_eq!(snapshot(&output), before, "a refused move changes nothing");
    }
}
