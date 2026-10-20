//! The capability record of the recogniser on the reference machine,
//! `records/stt-i9-10900k-cpu.json`, loaded with the testkit's loader against the adapter the
//! recogniser declares: the adapter claims nothing, so the record names no evidence; its audio
//! formats are those the conformance tests offer; its resource profile is a CPU one, measured by
//! `tests/stt_resources.rs`. Needs no weights.

mod support;

use std::path::Path;

use inference_stt::inference_core::model::{Adapter, DeclareCapabilityRecord};
use inference_testkit::model::capability;
use inference_whispercpp::stt::{DEFAULT_MAX_CONCURRENCY, declare_adapter, declare_model};

/// The record file of the reference machine, relative to the crate.
const RECORD: &str = "records/stt-i9-10900k-cpu.json";

fn adapter() -> Adapter {
    let (model, _) = declare_model();
    declare_adapter(model.model_id(), DEFAULT_MAX_CONCURRENCY)
        .expect("the default maximum concurrency is positive")
        .0
}

fn load(adapter: &Adapter) -> DeclareCapabilityRecord {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(RECORD);
    capability::load(&path, adapter).unwrap_or_else(|refused| panic!("{refused}"))
}

#[test]
fn the_record_of_the_reference_machine_loads_and_names_no_evidence_for_no_claim() {
    let adapter = adapter();
    let record = load(&adapter);

    assert_eq!(record.adapter_id, adapter.adapter_id());
    assert_eq!(record.model_id, adapter.model_id());
    assert_eq!(capability::check(&record, &adapter), Ok(()));
    assert!(adapter.capabilities().is_empty());
    assert!(
        record.evidence.is_empty(),
        "a batch adapter claims nothing, so nothing is evidenced: {:?}",
        record.evidence
    );
    assert!(
        record.hardware.contains("i9-10900K") && record.hardware.contains("CPU"),
        "the record names the reference machine and its CPU execution: {}",
        record.hardware
    );
    assert_eq!(record.languages, ["en"]);
    assert_eq!(record.adapter_revision, env!("CARGO_PKG_VERSION"));
}

#[test]
fn the_record_lists_the_formats_the_conformance_tests_offer() {
    let record = load(&adapter());
    assert_eq!(record.audio_formats, support::FORMATS);
}

#[test]
fn the_resource_profile_is_a_measured_cpu_profile() {
    let profile = load(&adapter()).resource_profile;
    assert!(profile.peak_resident_memory_mib > 0, "{profile:?}");
    assert!(profile.load_time_ms > 0, "{profile:?}");
    assert!(
        profile
            .cpu_real_time_factor_milli
            .is_some_and(|milli| (1..1_000).contains(&milli)),
        "recognition keeps up with real time on CPU: {profile:?}"
    );
    assert_eq!(profile.gpu_memory_mib, None, "no GPU is used: {profile:?}");
}
