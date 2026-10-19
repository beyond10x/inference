//! The three checks of the core-model-sync story: `inference-core` follows the `inference.model`
//! vocabulary of `ess/domains/model.yaml` as the specification fixes changed it.

use inference_core::model::{
    Capability, DeclareModel, Modality, Model, ModelDeclared, ModelId, ModelNotFound,
};
use serde_json::json;

#[test]
fn check_1_capability_all_serializes_to_the_ten_ess_names_in_order() {
    assert_eq!(
        serde_json::to_value(Capability::ALL).expect("serializes"),
        json!([
            "output_streaming",
            "incremental_input",
            "concurrent_input_output",
            "native_duplex",
            "revisable_input",
            "tool_calling",
            "interruptible",
            "resumable_model_state",
            "audio_text_alignment",
            "orchestrated_duplex",
        ])
    );
    assert_eq!(
        serde_json::from_value::<Capability>(json!("orchestrated_duplex")).expect("deserializes"),
        Capability::OrchestratedDuplex
    );
}

#[test]
fn check_2_a_model_declared_with_provenance_and_source_revision_carries_both() {
    let input = DeclareModel {
        family: "whisper".to_owned(),
        checkpoint: "large-v3".to_owned(),
        code_license: "MIT".to_owned(),
        weight_license: "Apache-2.0".to_owned(),
        checksum: "sha256:0f1e2d3c".to_owned(),
        input_modalities: vec![Modality::Audio],
        output_modalities: vec![Modality::Text],
        provenance: "upstream release".to_owned(),
        source_revision: "rev-2026-10".to_owned(),
    };

    let (model, event) = Model::declare(input);

    assert_eq!(model.provenance(), "upstream release");
    assert_eq!(model.source_revision(), "rev-2026-10");

    let entity = serde_json::to_value(&model).expect("entity serializes");
    assert_eq!(entity["provenance"], json!("upstream release"));
    assert_eq!(entity["source_revision"], json!("rev-2026-10"));

    assert_eq!(
        event,
        ModelDeclared {
            model_id: model.model_id(),
            family: "whisper".to_owned(),
            checkpoint: "large-v3".to_owned(),
        },
        "the ModelDeclared payload is unchanged: model_id, family and checkpoint"
    );
}

#[test]
fn check_3_model_not_found_serializes_as_its_model_id() {
    let model_id = ModelId::generate();
    let refused = ModelNotFound { model_id };

    let wire = serde_json::to_value(refused).expect("error serializes");
    assert_eq!(wire, json!({"model_id": model_id.to_string()}));

    let back: ModelNotFound = serde_json::from_value(wire).expect("error deserializes");
    assert_eq!(back, refused);
}
