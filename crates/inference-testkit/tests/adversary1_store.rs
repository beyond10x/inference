//! Adversary pass 1 against the `inference.model` store of `inference-testkit`: the counter after
//! an `unknown-model` refusal, the declaration order of the views, and the identities of two runs.

use inference_core::model::{
    Capability, DeclareAdapter, DeclareModel, ExecutionForm, Locality, Modality, ModelId,
};
use inference_testkit::model::{DeclareAdapterError, ModelDomain};

fn declare_model(family: &str) -> DeclareModel {
    DeclareModel {
        family: family.to_owned(),
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

fn declare_adapter(model_id: ModelId, backend: &str, max_concurrency: i64) -> DeclareAdapter {
    DeclareAdapter {
        model_id,
        backend: backend.to_owned(),
        contract_revision: "contract_revision".to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities: vec![Capability::OutputStreaming],
        max_concurrency,
    }
}

fn unknown_model_id() -> ModelId {
    serde_json::from_value(serde_json::json!("00000000-0000-4000-8000-50f408d7384c"))
        .expect("a canonical Uuid")
}

#[test]
fn an_unknown_model_refusal_takes_no_identity() {
    let mut domain = ModelDomain::new();
    let model = domain.declare_model(declare_model("family"));

    let refused = domain.declare_adapter(declare_adapter(unknown_model_id(), "backend", 1));
    assert!(matches!(refused, Err(DeclareAdapterError::UnknownModel(_))));

    let adapter = domain
        .declare_adapter(declare_adapter(model.model_id, "backend", 1))
        .expect("declared");
    assert_eq!(
        adapter.adapter_id.to_string(),
        "00000001-0000-4000-8000-000000000002",
        "the unknown-model refusal consumed an identity"
    );
    assert_eq!(domain.events().len(), 2);
}

#[test]
fn the_views_list_rows_in_declaration_order() {
    let mut domain = ModelDomain::new();
    let first = domain.declare_model(declare_model("first"));
    let second = domain.declare_model(declare_model("second"));
    let third = domain.declare_model(declare_model("third"));
    domain
        .declare_adapter(declare_adapter(third.model_id, "a", 1))
        .expect("declared");
    domain
        .declare_adapter(declare_adapter(first.model_id, "b", 1))
        .expect("declared");

    let families: Vec<_> = domain
        .models_view()
        .into_iter()
        .map(|row| row.family)
        .collect();
    assert_eq!(families, ["first", "second", "third"]);
    let ids: Vec<_> = domain
        .models_view()
        .into_iter()
        .map(|row| row.model_id)
        .collect();
    assert_eq!(ids, [first.model_id, second.model_id, third.model_id]);

    let backends: Vec<_> = domain
        .adapters_view()
        .into_iter()
        .map(|row| (row.backend, row.model_id))
        .collect();
    assert_eq!(
        backends,
        [
            ("a".to_owned(), third.model_id),
            ("b".to_owned(), first.model_id)
        ]
    );
}

#[test]
fn two_runs_of_the_same_commands_issue_the_same_identities() {
    let run = || {
        let mut domain = ModelDomain::new();
        let mut issued = Vec::new();
        for family in ["a", "b"] {
            issued.push(
                domain
                    .declare_model(declare_model(family))
                    .model_id
                    .to_string(),
            );
        }
        let first = domain.models().iter().next().expect("a model").model_id();
        let _ = domain.declare_adapter(declare_adapter(unknown_model_id(), "x", 1));
        let _ = domain.declare_adapter(declare_adapter(first, "x", 0));
        issued.push(
            domain
                .declare_adapter(declare_adapter(first, "x", 3))
                .expect("declared")
                .adapter_id
                .to_string(),
        );
        issued
    };
    let expected = [
        "00000001-0000-4000-8000-000000000001",
        "00000001-0000-4000-8000-000000000002",
        "00000001-0000-4000-8000-000000000003",
    ];
    assert_eq!(run(), expected);
    assert_eq!(run(), expected);
}
