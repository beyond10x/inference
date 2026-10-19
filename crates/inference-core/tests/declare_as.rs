//! `Model::declare_as` and `Adapter::declare_as`: the declaring commands under an identity the
//! caller assigns.

use inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, DeclareModel, ExecutionForm, Locality,
    Modality, Model, ModelId, NonPositiveConcurrency,
};
use uuid::Uuid;

fn declare_model() -> DeclareModel {
    DeclareModel {
        family: "family".to_owned(),
        checkpoint: "checkpoint".to_owned(),
        code_license: "code_license".to_owned(),
        weight_license: "weight_license".to_owned(),
        checksum: "checksum".to_owned(),
        input_modalities: vec![Modality::Text],
        output_modalities: vec![Modality::Audio],
        provenance: "provenance".to_owned(),
        source_revision: "source_revision".to_owned(),
    }
}

fn declare_adapter(model_id: ModelId, max_concurrency: i64) -> DeclareAdapter {
    DeclareAdapter {
        model_id,
        backend: "backend".to_owned(),
        contract_revision: "contract_revision".to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities: vec![Capability::OutputStreaming],
        max_concurrency,
    }
}

#[test]
fn a_model_declared_as_an_identity_carries_it_and_otherwise_matches_declare() {
    let model_id = ModelId::from(Uuid::from_u128(1));
    let (model, event) = Model::declare_as(model_id, declare_model());
    let (generated, _) = Model::declare(declare_model());

    assert_eq!(model.model_id(), model_id);
    assert_eq!(event.model_id, model_id);
    assert_eq!(event.family, "family");
    assert_eq!(event.checkpoint, "checkpoint");
    assert_eq!(model.family(), generated.family());
    assert_eq!(model.checksum(), generated.checksum());
    assert_eq!(model.input_modalities(), generated.input_modalities());
    assert_eq!(model.provenance(), generated.provenance());
    assert_eq!(model.source_revision(), generated.source_revision());
}

#[test]
fn an_adapter_declared_as_an_identity_carries_it() {
    let model_id = ModelId::from(Uuid::from_u128(1));
    let adapter_id = AdapterId::from(Uuid::from_u128(2));
    let (adapter, event) =
        Adapter::declare_as(adapter_id, declare_adapter(model_id, 3)).expect("positive");

    assert_eq!(adapter.adapter_id(), adapter_id);
    assert_eq!(adapter.model_id(), model_id);
    assert_eq!(adapter.max_concurrency(), 3);
    assert_eq!(event.adapter_id, adapter_id);
    assert_eq!(event.model_id, model_id);
    assert_eq!(event.capabilities, [Capability::OutputStreaming]);
}

#[test]
fn an_adapter_declared_as_an_identity_refuses_a_non_positive_concurrency() {
    let model_id = ModelId::from(Uuid::from_u128(1));
    let adapter_id = AdapterId::from(Uuid::from_u128(2));
    for submitted in [0, -1] {
        assert_eq!(
            Adapter::declare_as(adapter_id, declare_adapter(model_id, submitted)),
            Err(NonPositiveConcurrency { submitted })
        );
    }
}
