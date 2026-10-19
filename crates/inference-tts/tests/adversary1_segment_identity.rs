//! Adversary pass 1 of `story:tts-contract`: `inference.stream.QueueForSynthesis` moves the
//! segment instance named by `segment_id` from Committed to Queued, and its `wrong-state` outcome
//! refuses any instance not Committed (`ess/domains/stream.yaml`). Pass 1 found `offer` queueing a
//! copy, so the identity was held in two states at once. `offer` now moves the segment into the
//! synthesis: that a second synthesis cannot queue it is the `compile_fail` example of the crate
//! documentation, and a discard runs on the synthesis's own record, which then does not complete.
//! The last case guards `complete` against a segment offered and not yet taken, which no case of
//! the unit reaches.

use inference_tts::inference_core::generation::{Generation, StartGeneration};
use inference_tts::inference_core::id::{NodeId, RunId};
use inference_tts::inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_tts::inference_text::{OutputSegment, SegmentState, SegmentStateConflict};
use inference_tts::{CONTRACT_REVISION, CompletionRefused, SegmentNotTaken, SynthesisOutput};

fn adapter(capabilities: Vec<Capability>) -> Adapter {
    Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: "adversary".to_owned(),
        contract_revision: CONTRACT_REVISION.to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities,
        max_concurrency: 1,
    })
    .expect("a maximum concurrency of one is positive")
    .0
}

fn committed(text: &str) -> OutputSegment {
    let (generation, _) = Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    });
    let (tentative, _) =
        OutputSegment::emit(&generation, 1, text.to_owned()).expect("the generation is Active");
    tentative.commit().expect("a Tentative segment commits").0
}

#[test]
fn a_segment_discarded_after_it_was_offered_does_not_complete_synthesis() {
    let segment = committed("hello");
    let id = segment.segment_id();
    let mut output = SynthesisOutput::start(&adapter(Vec::new()));
    output.offer(segment).expect("queued");
    output
        .discard(id)
        .expect("the synthesis holds the segment Queued, so DiscardSegment accepts it");
    assert_eq!(
        output.segment(id).map(OutputSegment::state),
        Some(SegmentState::Discarded)
    );
    output.end_utterance().expect("ends once");
    output.take_input().expect("after the end");

    assert_eq!(
        output.complete(id, Vec::new()).map(|_| ()),
        Err(CompletionRefused::SegmentStateConflict(
            SegmentStateConflict {
                current: SegmentState::Discarded,
            }
        )),
        "a Discarded segment is terminal and CompleteSynthesis moves only a Queued one"
    );
}

#[test]
fn a_segment_offered_and_not_yet_taken_does_not_complete() {
    let segment = committed("hello");
    let id = segment.segment_id();
    let mut output = SynthesisOutput::start(&adapter(Vec::new()));
    output.offer(segment).expect("queued");

    assert_eq!(
        output.complete(id, Vec::new()),
        Err(CompletionRefused::SegmentNotTaken(SegmentNotTaken {
            segment_id: id
        }))
    );
    assert_eq!(
        output.segment(id).map(OutputSegment::state),
        Some(SegmentState::Queued)
    );
}
