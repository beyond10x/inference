//! Adversary cases for story:core-model-sync, pass 1: the new `inference.model` declarations driven
//! from the literal inputs and expectations of the synthesized `DeclareAdapter/outcome/unknown-model`
//! scenario. Each is green on the tree and red on the mutant its name gives.

use std::error::Error;

use inference_core::model::{DeclareModel, Model, ModelId, ModelNotFound};
use serde_json::{Value, json};
use uuid::Uuid;

const SCENARIO_MISSING_MODEL_ID: &str = "00000000-0000-4000-8000-50f408d7384c";

fn scenario_declare_model_input() -> Value {
    json!({
        "checkpoint": "checkpoint-138599",
        "checksum": "checksum-138599",
        "code_license": "code_license-138599",
        "family": "family-138599",
        "input_modalities": ["Control"],
        "output_modalities": ["Control"],
        "provenance": "provenance-138599",
        "source_revision": "source_revision-138599",
        "weight_license": "weight_license-138599",
    })
}

#[test]
fn adversary3_declare_model_requires_provenance_and_source_revision_mutant_serde_default() {
    let parsed: DeclareModel =
        serde_json::from_value(scenario_declare_model_input()).expect("the suite input parses");
    let (model, _) = Model::declare(parsed);
    assert_eq!(model.provenance(), "provenance-138599");
    assert_eq!(model.source_revision(), "source_revision-138599");

    for required in ["provenance", "source_revision"] {
        let mut input = scenario_declare_model_input();
        input
            .as_object_mut()
            .expect("an object")
            .remove(required)
            .expect("the field was present");
        let parsed: Result<DeclareModel, _> = serde_json::from_value(input);
        assert!(
            parsed.is_err(),
            "DeclareModel without {required} parsed, but model.yaml declares it a String input \
             with no default: {parsed:?}"
        );
    }
}

#[test]
fn adversary3_model_not_found_reads_the_suite_expectation_and_names_the_identity_mutant_display() {
    let expected = json!({"model_id": SCENARIO_MISSING_MODEL_ID});
    let model_id = ModelId::from(Uuid::parse_str(SCENARIO_MISSING_MODEL_ID).expect("uuid"));

    let read: ModelNotFound =
        serde_json::from_value(expected.clone()).expect("the suite's expect_error fields parse");
    assert_eq!(read, ModelNotFound { model_id });
    assert_eq!(serde_json::to_value(read).expect("serializes"), expected);

    let refused: Box<dyn Error> = Box::new(read);
    let message = refused.to_string();
    assert!(
        message.contains(SCENARIO_MISSING_MODEL_ID),
        "the refusal message does not name the identity that was sent: {message:?}"
    );

    let empty: Result<ModelNotFound, _> = serde_json::from_value(json!({}));
    assert!(
        empty.is_err(),
        "ModelNotFound without model_id parsed: {empty:?}"
    );
}
