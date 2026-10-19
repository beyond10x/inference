//! Adversary cases for story:workspace-skeleton, pass 1: the `inference.model` vocabulary driven
//! from `ess/domains/model.yaml` and from the literal inputs of the synthesized scenarios.

use std::collections::BTreeSet;

use inference_core::id::{GenerationId, NodeId, RunId, SessionId, StreamId};
use inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, DeclareModel, ExecutionForm, Locality,
    Modality, Model, ModelId, NonPositiveConcurrency,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

const SCENARIO_MODEL_ID: &str = "00000000-0000-4000-8000-dfb547682652";

fn scenario_declare_adapter_input(max_concurrency: Value) -> Value {
    json!({
        "backend": "backend",
        "capabilities": ["output_streaming"],
        "contract_revision": "contract_revision",
        "execution_form": "in_process",
        "locality": "same_process",
        "max_concurrency": max_concurrency,
        "model_id": SCENARIO_MODEL_ID,
    })
}

fn keys(value: &Value) -> BTreeSet<String> {
    value
        .as_object()
        .expect("serializes as an object")
        .keys()
        .cloned()
        .collect()
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

const TESTKIT_CONVERTS: &str = "inference-core reads ESS Integer as i64 and refuses a float on the \
     wire; story:testkit-conformance-target converts the suite's integral floats before replay";

#[test]
fn adversary_suite_float_max_concurrency_one_is_unparseable_and_integer_one_is_declared() {
    let float: Result<DeclareAdapter, _> =
        serde_json::from_value(scenario_declare_adapter_input(json!(1.0)));
    assert!(
        float.is_err(),
        "max_concurrency 1.0 parsed as a DeclareAdapter: {TESTKIT_CONVERTS}"
    );

    let parsed: DeclareAdapter = serde_json::from_value(scenario_declare_adapter_input(json!(1)))
        .unwrap_or_else(|error| {
            panic!("max_concurrency 1 is a DeclareAdapter ({error}): {TESTKIT_CONVERTS}")
        });
    let (adapter, event) = Adapter::declare(parsed).expect("max_concurrency 1 is declared");
    assert_eq!(adapter.max_concurrency(), 1);
    assert_eq!(event.capabilities, vec![Capability::OutputStreaming]);
}

#[test]
fn adversary_suite_float_max_concurrency_zero_is_unparseable_and_integer_zero_is_refused() {
    let float: Result<DeclareAdapter, _> =
        serde_json::from_value(scenario_declare_adapter_input(json!(0.0)));
    assert!(
        float.is_err(),
        "max_concurrency 0.0 parsed as a DeclareAdapter: {TESTKIT_CONVERTS}"
    );

    let parsed: DeclareAdapter = serde_json::from_value(scenario_declare_adapter_input(json!(0)))
        .unwrap_or_else(|error| {
            panic!("max_concurrency 0 is a DeclareAdapter ({error}): {TESTKIT_CONVERTS}")
        });
    assert_eq!(
        Adapter::declare(parsed),
        Err(NonPositiveConcurrency { submitted: 0 }),
        "{TESTKIT_CONVERTS}"
    );
}

#[test]
fn adversary_suite_declare_model_input_and_event_payload_match_ess() {
    let input = json!({
        "checkpoint": "checkpoint",
        "checksum": "checksum",
        "code_license": "code_license",
        "family": "family",
        "input_modalities": ["Text"],
        "output_modalities": ["Audio"],
        "weight_license": "weight_license",
        "provenance": "provenance",
        "source_revision": "source_revision",
    });
    let parsed: DeclareModel = serde_json::from_value(input).expect("suite input parses");
    let (model, event) = Model::declare(parsed);

    let wire = serde_json::to_value(&event).expect("event serializes");
    assert_eq!(keys(&wire), set(&["model_id", "family", "checkpoint"]));
    assert_eq!(wire["family"], json!("family"));
    assert_eq!(wire["checkpoint"], json!("checkpoint"));
    assert_eq!(wire["model_id"], json!(model.model_id().to_string()));

    let entity = serde_json::to_value(&model).expect("entity serializes");
    assert_eq!(
        keys(&entity),
        set(&[
            "model_id",
            "family",
            "checkpoint",
            "code_license",
            "weight_license",
            "checksum",
            "input_modalities",
            "output_modalities",
            "provenance",
            "source_revision",
        ])
    );
    assert_eq!(entity["input_modalities"], json!(["Text"]));
    assert_eq!(entity["output_modalities"], json!(["Audio"]));
    assert_eq!(entity["provenance"], json!("provenance"));
    assert_eq!(entity["source_revision"], json!("source_revision"));
}

fn integer_adapter_input(model_id: ModelId, max_concurrency: i64) -> DeclareAdapter {
    DeclareAdapter {
        model_id,
        backend: "backend".to_owned(),
        contract_revision: "contract_revision".to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities: vec![Capability::OutputStreaming, Capability::NativeDuplex],
        max_concurrency,
    }
}

#[test]
fn adversary_adapter_entity_and_event_carry_exactly_the_ess_fields() {
    let model_id = ModelId::generate();
    let (adapter, event) =
        Adapter::declare(integer_adapter_input(model_id, 3)).expect("3 is declared");

    let wire = serde_json::to_value(&event).expect("event serializes");
    assert_eq!(
        keys(&wire),
        set(&["adapter_id", "model_id", "capabilities"])
    );
    assert_eq!(wire["model_id"], json!(model_id.to_string()));
    assert_eq!(wire["adapter_id"], json!(adapter.adapter_id().to_string()));
    assert_eq!(
        wire["capabilities"],
        json!(["output_streaming", "native_duplex"])
    );

    let entity = serde_json::to_value(&adapter).expect("entity serializes");
    assert_eq!(
        keys(&entity),
        set(&[
            "adapter_id",
            "model_id",
            "backend",
            "contract_revision",
            "execution_form",
            "locality",
            "capabilities",
            "max_concurrency",
        ])
    );
    assert_eq!(entity["execution_form"], json!("in_process"));
    assert_eq!(entity["locality"], json!("same_process"));
    assert_eq!(entity["max_concurrency"], json!(3));
}

#[test]
fn adversary_adapter_id_is_fresh_per_declaration() {
    let model_id = ModelId::generate();
    let mut seen = BTreeSet::new();
    for _ in 0..16 {
        let (adapter, event) =
            Adapter::declare(integer_adapter_input(model_id, 1)).expect("1 is declared");
        assert_eq!(event.adapter_id, adapter.adapter_id());
        assert!(
            seen.insert(adapter.adapter_id()),
            "adapter_id repeated across declarations"
        );
    }
}

#[test]
fn adversary_max_concurrency_boundaries() {
    let model_id = ModelId::generate();
    for submitted in [0, -1, i64::MIN] {
        assert_eq!(
            Adapter::declare(integer_adapter_input(model_id, submitted)),
            Err(NonPositiveConcurrency { submitted })
        );
    }
    for accepted in [1, 2, i64::MAX] {
        let (adapter, _) = Adapter::declare(integer_adapter_input(model_id, accepted))
            .expect("a positive value is declared");
        assert_eq!(adapter.max_concurrency(), accepted);
    }
    let refused = serde_json::to_value(NonPositiveConcurrency { submitted: -5 }).expect("error");
    assert_eq!(refused, json!({"submitted": -5}));
}

fn assert_rejects<T: DeserializeOwned + std::fmt::Debug>(name: &str) {
    for bad in [
        json!(""),
        json!("not-a-uuid"),
        json!("00000000-0000-4000-8000"),
        json!(1),
        json!(null),
    ] {
        let parsed: Result<T, _> = serde_json::from_value(bad.clone());
        assert!(parsed.is_err(), "{name} accepted {bad}");
    }
}

#[test]
fn adversary_identifiers_reject_non_uuid_values() {
    assert_rejects::<ModelId>("ModelId");
    assert_rejects::<AdapterId>("AdapterId");
    assert_rejects::<RunId>("RunId");
    assert_rejects::<NodeId>("NodeId");
    assert_rejects::<StreamId>("StreamId");
    assert_rejects::<GenerationId>("GenerationId");
    assert_rejects::<SessionId>("SessionId");
}

#[test]
fn adversary_enums_reject_names_ess_does_not_list() {
    for bad in ["OutputStreaming", "outputStreaming", "OUTPUT_STREAMING"] {
        assert!(
            serde_json::from_value::<Capability>(json!(bad)).is_err(),
            "{bad}"
        );
    }
    for bad in ["text", "TEXT", "control"] {
        assert!(
            serde_json::from_value::<Modality>(json!(bad)).is_err(),
            "{bad}"
        );
    }
    for bad in ["SameProcess", "same-process"] {
        assert!(
            serde_json::from_value::<Locality>(json!(bad)).is_err(),
            "{bad}"
        );
    }
    for bad in ["InProcess", "native_f_f_i", "nativeffi"] {
        assert!(
            serde_json::from_value::<ExecutionForm>(json!(bad)).is_err(),
            "{bad}"
        );
    }
}
