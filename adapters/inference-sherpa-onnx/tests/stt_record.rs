//! The capability record of the recogniser on the reference machine, `records/stt-i9-10900k-cpu.json`,
//! loaded with the testkit's loader against the adapter the recogniser declares: every claim has
//! evidence and no evidence names an unclaimed capability; every test the evidence names is a
//! test of this crate; its audio formats are those the conformance tests stream; its resource
//! profile is a CPU one, measured by `tests/stt_resources.rs`. Needs no weights.
#![cfg(feature = "stt")]

mod support;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use inference_sherpa_onnx::stt::{DEFAULT_MAX_CONCURRENCY, declare_adapter, declare_model};
use inference_stt::inference_core::model::{Adapter, Capability, DeclareCapabilityRecord};
use inference_testkit::model::capability;

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

/// The names of the `#[test]` functions of the test file `tests/<binary>.rs`.
fn tests_of(binary: &str) -> BTreeSet<String> {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", &format!("{binary}.rs")]
        .iter()
        .collect();
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} cannot be read: {error}", path.display()));
    source
        .split("#[test]")
        .skip(1)
        .filter_map(|after| {
            let after = &after[after.find("fn ")? + 3..];
            Some(after[..after.find('(')?].trim().to_owned())
        })
        .collect()
}

#[test]
fn the_record_of_the_reference_machine_loads_and_evidences_exactly_the_claims() {
    let adapter = adapter();
    let record = load(&adapter);

    assert_eq!(record.adapter_id, adapter.adapter_id());
    assert_eq!(record.model_id, adapter.model_id());
    assert_eq!(capability::check(&record, &adapter), Ok(()));
    assert_eq!(
        record
            .evidence
            .iter()
            .map(|evidence| evidence.capability)
            .collect::<Vec<_>>(),
        [Capability::IncrementalInput, Capability::OutputStreaming]
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
fn every_test_the_evidence_names_is_a_test_of_this_crate() {
    let record = load(&adapter());
    for evidence in &record.evidence {
        for test in &evidence.tests {
            let (binary, name) = test
                .split_once("::")
                .unwrap_or_else(|| panic!("{test} names its test file and function"));
            assert!(
                tests_of(binary).contains(name),
                "{test}: tests/{binary}.rs has a test {name}, the evidence of {:?}",
                evidence.capability
            );
        }
    }
}

#[test]
fn the_record_lists_the_formats_the_conformance_tests_stream() {
    let record = load(&adapter());
    assert_eq!(record.audio_formats, support::FORMATS);
}

#[test]
fn the_resource_profile_is_a_measured_cpu_profile() {
    let profile = load(&adapter()).resource_profile;
    assert!(profile.peak_resident_memory_mib > 0, "{profile:?}");
    assert!(profile.load_time_ms > 0, "{profile:?}");
    assert!(
        (1..1_000).contains(&profile.cpu_real_time_factor_milli),
        "a streaming recogniser keeps up with real time on CPU: {profile:?}"
    );
    assert_eq!(profile.gpu_memory_mib, None, "no GPU is used: {profile:?}");
}
