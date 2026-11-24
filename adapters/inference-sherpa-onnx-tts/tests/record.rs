//! The capability record of the synthesiser on the machine it was measured on,
//! `records/tts-xeon-8375c-cpu.json`, loaded with the testkit's loader against the adapter the
//! synthesiser declares: every claim has evidence and no evidence names an unclaimed capability;
//! every test the evidence names is a test of this crate; its audio format is the checkpoint's;
//! its resource profile is a CPU one, measured by `tests/resources.rs`. Needs no weights.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use inference_sherpa_onnx_tts::inference_tts::inference_audio::{AudioEncoding, AudioFormat};
use inference_sherpa_onnx_tts::inference_tts::inference_core::model::{
    Adapter, Capability, DeclareCapabilityRecord,
};
use inference_sherpa_onnx_tts::{
    DEFAULT_MAX_CONCURRENCY, SAMPLE_RATE, declare_adapter, declare_model,
};
use inference_testkit::model::capability;

/// The record file of the machine the synthesiser was measured on, relative to the crate.
const RECORD: &str = "records/tts-xeon-8375c-cpu.json";

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

/// The names of the `#[test]` functions of the test module `tests/<module>.rs`.
fn tests_of(module: &str) -> BTreeSet<String> {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", &format!("{module}.rs")]
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
fn the_record_of_the_measured_machine_loads_and_evidences_exactly_the_claims() {
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
        record.hardware.contains("Xeon Platinum 8375C") && record.hardware.contains("CPU"),
        "the record names the machine and its CPU execution: {}",
        record.hardware
    );
    assert_eq!(record.languages, ["en"]);
    assert_eq!(record.adapter_revision, env!("CARGO_PKG_VERSION"));
}

#[test]
fn every_test_the_evidence_names_is_a_test_of_this_crate() {
    let record = load(&adapter());
    for evidence in &record.evidence {
        assert!(
            !evidence.tests.is_empty(),
            "{:?} has evidence",
            evidence.capability
        );
        for test in &evidence.tests {
            let (module, name) = test
                .split_once("::")
                .unwrap_or_else(|| panic!("{test} names its test module and function"));
            assert!(
                tests_of(module).contains(name),
                "{test}: tests/{module}.rs has a test {name}, the evidence of {:?}",
                evidence.capability
            );
        }
    }
}

#[test]
fn the_record_lists_the_one_format_the_checkpoint_speaks() {
    let record = load(&adapter());
    assert_eq!(
        record
            .audio_formats
            .iter()
            .map(|format| (
                format.encoding,
                format.sample_rate.get(),
                format.channels.get()
            ))
            .collect::<Vec<_>>(),
        [(AudioEncoding::PcmS16le, SAMPLE_RATE, 1)]
    );
    let _: &[AudioFormat] = &record.audio_formats;
}

/// The peak resident memory a record of this checkpoint on a CPU of the measured class holds, in
/// MiB: above the 82 million parameters as 32-bit floats, 313 MiB, and below four times the 483
/// MiB measured.
const MEMORY_MIB: std::ops::Range<i64> = 313..1_932;

/// The load time such a record holds, in ms: hashing and loading the checkpoint takes more than
/// 100 ms, and less than the 60 s a test may run; 8050 ms were measured.
const LOAD_MS: std::ops::Range<i64> = 100..60_000;

#[test]
fn the_resource_profile_is_a_measured_cpu_profile() {
    let profile = load(&adapter()).resource_profile;
    assert!(
        MEMORY_MIB.contains(&profile.peak_resident_memory_mib),
        "the process that loaded the checkpoint and spoke peaked within {MEMORY_MIB:?} MiB: \
         {profile:?}"
    );
    assert!(
        LOAD_MS.contains(&profile.load_time_ms),
        "checking and loading the checkpoint took within {LOAD_MS:?} ms: {profile:?}"
    );
    assert!(
        profile
            .cpu_real_time_factor_milli
            .is_some_and(|milli| (1..1_000).contains(&milli)),
        "the synthesiser generates faster than real time on CPU: {profile:?}"
    );
    assert_eq!(profile.gpu_memory_mib, None, "no GPU is used: {profile:?}");
}
