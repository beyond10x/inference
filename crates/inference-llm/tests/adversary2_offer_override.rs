//! `inference-llm`'s crate documentation: "A handle takes input only as the `AdmittedInput` that
//! admission returns [...] So no implementation accepts input its adapter does not claim."
//! `GenerationHandle::offer_input` was a provided method, and an implementation could override it:
//! the text then arrived as a `String`, admission never ran, and an adapter claiming
//! `output_streaming` only took input while its generation was Active. `offer_input` is now the
//! sealed `OfferInput`, implemented once for every handle; that an implementation defining
//! `offer_input` does not compile is pinned by the `compile_fail` examples on `GenerationHandle`
//! and `OfferInput`. What runs here is the public offer path on a handle that takes whatever its
//! `accept_input` is given.

use inference_core::generation::{
    GenerationCancelled, GenerationStateConflict, GenerationSuperseded,
};
use inference_core::id::{NodeId, RunId};
use inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_llm::{
    AdmittedInput, CONTRACT_REVISION, CapabilityNotClaimed, GenerationEvent, GenerationHandle,
    GenerationOutput, GenerationRequest, InputRefused, OfferInput,
};

struct Overriding {
    output: GenerationOutput,
    taken: Vec<String>,
}

impl Iterator for Overriding {
    type Item = GenerationEvent;

    fn next(&mut self) -> Option<GenerationEvent> {
        None
    }
}

impl GenerationHandle for Overriding {
    fn output(&self) -> &GenerationOutput {
        &self.output
    }

    fn accept_input(&mut self, input: AdmittedInput) -> Result<(), InputRefused> {
        let text = self.output.release(input)?;
        self.taken.push(text);
        Ok(())
    }

    fn supersede(&mut self) -> Result<GenerationSuperseded, GenerationStateConflict> {
        Ok(self.output.supersede()?.0)
    }

    fn cancel(&mut self) -> Result<GenerationCancelled, GenerationStateConflict> {
        Ok(self.output.cancel()?.0)
    }
}

#[test]
fn an_adapter_not_claiming_concurrent_input_output_takes_no_input_through_offer_input() {
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
    let mut handle = Overriding {
        output: GenerationOutput::start(&adapter, &request).0,
        taken: Vec::new(),
    };
    let offered = handle.offer_input("input during output".to_owned());
    assert_eq!(
        (offered, handle.taken.as_slice()),
        (
            Err(InputRefused::CapabilityNotClaimed(CapabilityNotClaimed {
                capability: Capability::ConcurrentInputOutput,
            })),
            &[][..]
        ),
        "the trait lets an implementation take input its adapter does not claim"
    );
}
