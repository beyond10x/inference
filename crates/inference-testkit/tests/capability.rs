//! The capability record of an adapter, loaded from its JSON file and checked against the
//! adapter's declaration: every claimed capability has evidence, and no evidence names a
//! capability the adapter does not claim. The mock recogniser of both configurations is the
//! adapter; its records are under `tests/records/`.

#![cfg(feature = "stt")]

use std::path::PathBuf;

use inference_core::model::{
    Capability, CapabilityEvidence, DeclareAdapter, DeclareModel, ExecutionForm, Locality,
    Modality, ModelId,
};
use inference_stt::Recogniser;
use inference_testkit::model::ModelDomain;
use inference_testkit::model::capability::{self, RecordRefused};
use inference_testkit::stt::{MockConfiguration, MockRecogniser, script};

fn record(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/records")
        .join(name)
}

fn recogniser(configuration: MockConfiguration) -> MockRecogniser {
    MockRecogniser::new(configuration, ModelId::generate(), script())
}

#[test]
fn the_record_of_the_incremental_mock_recogniser_loads_and_passes() {
    let recogniser = recogniser(MockConfiguration::Incremental);
    let adapter = recogniser.adapter();

    let loaded = capability::load(record("mock-stt-incremental.json"), adapter)
        .expect("the record evidences exactly the claims");

    assert_eq!(loaded.adapter_id, adapter.adapter_id());
    assert_eq!(loaded.model_id, adapter.model_id());
    assert_eq!(loaded.hardware, "any host of the test suite");
    assert_eq!(loaded.languages, ["en"]);
    assert_eq!(loaded.audio_formats.len(), 1);
    assert_eq!(
        loaded
            .evidence
            .iter()
            .map(|evidence| evidence.capability)
            .collect::<Vec<_>>(),
        [Capability::IncrementalInput, Capability::OutputStreaming]
    );
    assert_eq!(loaded.resource_profile.gpu_memory_mib, None);
    assert_eq!(loaded.adapter_revision, "0.4.0");
    assert_eq!(capability::check(&loaded, adapter), Ok(()));
}

#[test]
fn the_record_of_the_batch_mock_recogniser_claims_nothing_and_needs_no_evidence() {
    let recogniser = recogniser(MockConfiguration::Batch);

    let loaded = capability::load(record("mock-stt-batch.json"), recogniser.adapter())
        .expect("no claim, no evidence");
    assert!(loaded.evidence.is_empty());
}

#[test]
fn a_loaded_record_declares_in_the_model_domain_against_its_adapter() {
    let mut domain = ModelDomain::new();
    let model = domain.declare_model(DeclareModel {
        family: "mock".to_owned(),
        checkpoint: "scripted".to_owned(),
        code_license: "Apache-2.0".to_owned(),
        weight_license: "none".to_owned(),
        checksum: "none".to_owned(),
        input_modalities: vec![Modality::Audio],
        output_modalities: vec![Modality::Text],
        provenance: "inference-testkit".to_owned(),
        source_revision: "0.4.0".to_owned(),
    });
    let declared = domain
        .declare_adapter(DeclareAdapter {
            model_id: model.model_id,
            backend: "inference-testkit mock recogniser".to_owned(),
            contract_revision: "1".to_owned(),
            execution_form: ExecutionForm::InProcess,
            locality: Locality::SameProcess,
            capabilities: MockConfiguration::Incremental.capabilities(),
            max_concurrency: 1,
        })
        .expect("declared");
    let adapter = domain
        .adapters()
        .get(declared.adapter_id)
        .expect("stored")
        .clone();

    let loaded = capability::load(record("mock-stt-incremental.json"), &adapter).expect("passes");
    let event = domain
        .declare_capability_record(loaded)
        .expect("the record names a declared adapter and model");

    assert_eq!(event.adapter_id, declared.adapter_id);
    assert_eq!(event.model_id, model.model_id);
    assert_eq!(domain.capability_records_view().len(), 1);
}

#[test]
fn a_claimed_capability_without_evidence_is_refused_and_named() {
    let recogniser = recogniser(MockConfiguration::Incremental);

    let refused = capability::load(record("claim-without-evidence.json"), recogniser.adapter());

    assert_eq!(
        refused,
        Err(RecordRefused::Unevidenced(Capability::OutputStreaming))
    );
    let message = refused.expect_err("refused").to_string();
    assert!(message.contains("`output_streaming`"), "{message}");
}

#[test]
fn evidence_naming_no_test_is_no_evidence() {
    let recogniser = recogniser(MockConfiguration::Incremental);

    assert_eq!(
        capability::load(
            record("claim-with-empty-evidence.json"),
            recogniser.adapter()
        ),
        Err(RecordRefused::Unevidenced(Capability::OutputStreaming))
    );
}

#[test]
fn evidence_for_an_unclaimed_capability_is_refused_and_named() {
    let recogniser = recogniser(MockConfiguration::Incremental);

    let refused = capability::load(record("evidence-for-unclaimed.json"), recogniser.adapter());

    assert_eq!(
        refused,
        Err(RecordRefused::Unclaimed(Capability::NativeDuplex))
    );
    let message = refused.expect_err("refused").to_string();
    assert!(message.contains("`native_duplex`"), "{message}");
}

#[test]
fn the_incremental_record_is_refused_for_the_batch_recogniser_which_claims_none_of_it() {
    let recogniser = recogniser(MockConfiguration::Batch);

    assert_eq!(
        capability::load(record("mock-stt-incremental.json"), recogniser.adapter()),
        Err(RecordRefused::Unclaimed(Capability::IncrementalInput))
    );
}

#[test]
fn check_reads_the_evidence_of_a_record_built_in_code() {
    let recogniser = recogniser(MockConfiguration::Incremental);
    let adapter = recogniser.adapter();
    let mut record =
        capability::load(record("mock-stt-incremental.json"), adapter).expect("passes");

    record.evidence.push(CapabilityEvidence {
        capability: Capability::ToolCalling,
        tests: vec!["somewhere".to_owned()],
    });
    assert_eq!(
        capability::check(&record, adapter),
        Err(RecordRefused::Unclaimed(Capability::ToolCalling))
    );

    record.evidence.clear();
    assert_eq!(
        capability::check(&record, adapter),
        Err(RecordRefused::Unevidenced(Capability::IncrementalInput))
    );
}

#[test]
fn a_file_naming_an_identity_an_undeclared_field_or_nothing_is_refused() {
    let recogniser = recogniser(MockConfiguration::Batch);
    let adapter = recogniser.adapter();

    for name in [
        "names-an-identity.json",
        "undeclared-field.json",
        "no-such-record.json",
    ] {
        let refused = capability::load(record(name), adapter);
        assert!(
            matches!(
                refused,
                Err(RecordRefused::Malformed { .. } | RecordRefused::Unreadable { .. })
            ),
            "{name}: {refused:?}"
        );
    }
    assert!(matches!(
        capability::load(record("no-such-record.json"), adapter),
        Err(RecordRefused::Unreadable { .. })
    ));
    let identity = capability::load(record("names-an-identity.json"), adapter)
        .expect_err("refused")
        .to_string();
    assert!(identity.contains("adapter_id"), "{identity}");
    let undeclared = capability::load(record("undeclared-field.json"), adapter)
        .expect_err("refused")
        .to_string();
    assert!(undeclared.contains("verified"), "{undeclared}");
}
