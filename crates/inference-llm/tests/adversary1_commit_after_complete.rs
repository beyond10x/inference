//! `inference.stream.CommitSegment` (`ess/domains/stream.yaml`) moves a Tentative segment to
//! Committed and refuses only with `SegmentStateConflict`; it has no guard on the generation's
//! state. A segment emitted into an Active generation that then completes is still Tentative, and
//! committing it is the `committed` outcome. Nothing else can move it afterwards: supersede and
//! cancel refuse a Completed generation, so a refusal here strands the segment Tentative.

use inference_core::id::{NodeId, RunId};
use inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_llm::{CONTRACT_REVISION, GenerationOutput, GenerationRequest};
use inference_text::SegmentState;

fn started() -> GenerationOutput {
    let adapter = Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: "test".to_owned(),
        contract_revision: CONTRACT_REVISION.to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities: vec![Capability::OutputStreaming],
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

#[test]
fn a_tentative_segment_of_a_completed_generation_commits_as_ess_declares() {
    let mut output = started();
    let delta = output
        .emit("last".to_owned())
        .expect("Active takes a delta");
    output.complete().expect("Active completes");

    let committed = output.commit_tentative();

    let ids = committed.map(|events| {
        events
            .iter()
            .map(|event| event.segment_id)
            .collect::<Vec<_>>()
    });
    assert_eq!(
        ids,
        Ok(vec![delta.segment_id]),
        "CommitSegment has no generation-state guard in ess; the Tentative segment commits"
    );
    assert_eq!(output.segments()[0].state(), SegmentState::Committed);
}
