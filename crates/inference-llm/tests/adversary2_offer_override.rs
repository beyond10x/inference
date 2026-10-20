//! `inference-llm`'s crate documentation: "A handle takes input only as the `AdmittedInput` that
//! admission returns [...] So no input the adapter does not claim reaches the model."
//! `GenerationHandle::offer_input` was once a provided method of a trait, and an implementation
//! could override it: the text then arrived as a `String`, admission never ran, and an adapter
//! claiming `output_streaming` only took input while its generation was Active. The handle is now
//! the one concrete wrapper over `llm_core::Model`, so no implementation defines the offer path;
//! what runs here is that path on a handle whose model records every request it is asked, and
//! the refused text reaches none of them.

use std::sync::{Arc, Mutex, PoisonError};

use inference_core::id::{NodeId, RunId};
use inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_llm::llm_core::{
    BoxFuture, Cancel, Capabilities, Error, Id, Item, Model, Protocol, Provenance, StopReason,
    StreamSink, TurnObservation, TurnOutcome, TurnRequest,
};
use inference_llm::{
    CONTRACT_REVISION, CapabilityNotClaimed, GenerationRequest, Generator, InputRefused,
};

/// A model answering nothing, recording every request it is asked.
struct Recording {
    provenance: Provenance,
    capabilities: Capabilities,
    taken: Arc<Mutex<Vec<TurnRequest>>>,
}

impl Model for Recording {
    fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn turn<'a>(
        &'a self,
        request: &'a TurnRequest,
        _sink: &'a mut dyn StreamSink,
        _cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<TurnOutcome, Error>> {
        Box::pin(async move {
            self.taken
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(request.clone());
            Ok(TurnOutcome {
                stop_reason: StopReason::EndTurn,
                items: Vec::new(),
                observation: TurnObservation {
                    final_usage: true,
                    ..TurnObservation::new(self.provenance.clone())
                },
            })
        })
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
    let id = |value: &str| Id::new(value).expect("a printable identifier");
    let taken = Arc::new(Mutex::new(Vec::new()));
    let model = Recording {
        provenance: Provenance {
            protocol: Protocol::ChatCompletions,
            provider: id("test"),
            account: id("test"),
            endpoint: id("in-process"),
            model: id("model"),
            binding_revision: id("1"),
        },
        capabilities: Capabilities::text(1024, 256),
        taken: Arc::clone(&taken),
    };
    let request = GenerationRequest {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
        turn: TurnRequest::new("model", vec![Item::user("prompt")]),
    };
    let mut handle = Generator::new(adapter, Arc::new(model))
        .expect("the model declares what the adapter claims")
        .start(request)
        .expect("a fresh generator runs nothing else");
    let offered = handle.offer_input("input during output".to_owned());
    handle.by_ref().for_each(drop);
    let reached = taken
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .any(|request| request.items.contains(&Item::user("input during output")));
    assert_eq!(
        (offered, reached),
        (
            Err(InputRefused::CapabilityNotClaimed(CapabilityNotClaimed {
                capability: Capability::ConcurrentInputOutput,
            })),
            false
        ),
        "the handle lets input its adapter does not claim reach the model"
    );
}
