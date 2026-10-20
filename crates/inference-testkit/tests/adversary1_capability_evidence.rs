//! Adversary cases of the capability record check: what counts as evidence, and the serialized
//! names of the record against `ess/domains/model.yaml`.

use inference_core::model::{
    Adapter, Capability, CapabilityRecord, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_testkit::model::capability::{self, RecordRefused};
use serde_json::Value;

fn streaming_adapter() -> Adapter {
    Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: "backend".to_owned(),
        contract_revision: "1".to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities: vec![Capability::OutputStreaming],
        max_concurrency: 1,
    })
    .expect("one slot")
    .0
}

fn record_with_tests(tests: &str) -> String {
    format!(
        r#"{{
            "hardware": "reference machine",
            "languages": ["en"],
            "audio_formats": [],
            "evidence": [{{"capability": "output_streaming", "tests": {tests}}}],
            "resource_profile": {{"peak_resident_memory_mib": 1, "load_time_ms": 1}},
            "adapter_revision": "0.1.0"
        }}"#
    )
}

/// The module says an entry naming no test is no evidence. A test whose name is empty, or only
/// blank, names no test either: nothing can be run under it.
#[test]
fn an_evidence_entry_whose_only_test_name_is_empty_or_blank_is_no_evidence() {
    let adapter = streaming_adapter();
    for tests in [r#"[""]"#, r#"["   "]"#] {
        assert_eq!(
            capability::parse(&record_with_tests(tests), &adapter).map(drop),
            Err(RecordRefused::Unevidenced(Capability::OutputStreaming)),
            "tests {tests} name no conformance test"
        );
    }
}

/// An evidence entry naming a capability the specification does not declare is not a record.
#[test]
fn evidence_for_a_capability_outside_the_matrix_is_malformed() {
    let adapter = streaming_adapter();
    let text = record_with_tests(r#"["stream::partials"]"#)
        .replace("\"output_streaming\"", "\"telepathy\"");
    assert!(
        matches!(
            capability::parse(&text, &adapter),
            Err(RecordRefused::Malformed { .. })
        ),
        "an undeclared capability name is refused"
    );
}

/// The entity `inference.model.CapabilityRecord` of `ess/domains/model.yaml` names its identity
/// and eight fields; the record serializes under exactly those names.
#[test]
fn a_declared_record_serializes_under_the_ess_field_names() {
    let adapter = streaming_adapter();
    let input = capability::parse(&record_with_tests(r#"["stream::partials"]"#), &adapter)
        .expect("a valid record");
    let (record, _) = CapabilityRecord::declare(input);
    let Value::Object(fields) = serde_json::to_value(&record).expect("serializes") else {
        panic!("a record serializes as an object")
    };
    let mut names: Vec<&str> = fields.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "adapter_id",
            "adapter_revision",
            "audio_formats",
            "capability_record_id",
            "evidence",
            "hardware",
            "languages",
            "model_id",
            "resource_profile",
        ]
    );
    let Value::Object(profile) = &fields["resource_profile"] else {
        panic!("the profile is an object")
    };
    let mut parts: Vec<&str> = profile.keys().map(String::as_str).collect();
    parts.sort_unstable();
    assert_eq!(
        parts,
        [
            "cpu_real_time_factor_milli",
            "gpu_memory_mib",
            "load_time_ms",
            "peak_resident_memory_mib",
        ]
    );
}
