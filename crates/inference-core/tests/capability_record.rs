//! `inference.model.CapabilityRecord`: the record of one adapter on one hardware target, its
//! declaring command, its event and its refusal, on the wire as `ess/domains/model.yaml` names
//! them.

use inference_audio::{AudioEncoding, AudioFormat};
use inference_core::model::{
    AdapterId, AdapterNotFound, Capability, CapabilityEvidence, CapabilityRecord,
    CapabilityRecordDeclared, CapabilityRecordId, DeclareCapabilityRecord, ResourceProfile,
};
use serde_json::{Value, json};
use uuid::Uuid;

const ADAPTER_ID: &str = "00000001-0000-4000-8000-000000000002";
const MODEL_ID: &str = "00000001-0000-4000-8000-000000000001";

fn record_wire() -> Value {
    json!({
        "adapter_id": ADAPTER_ID,
        "model_id": MODEL_ID,
        "hardware": "reference machine",
        "languages": ["en", "de"],
        "audio_formats": [{"encoding": "pcm_s16le", "sample_rate": 16000, "channels": 1}],
        "evidence": [
            {"capability": "incremental_input", "tests": ["stt::takes_frames_before_the_end"]},
            {"capability": "output_streaming", "tests": ["stt::hypotheses_before_the_end"]},
        ],
        "resource_profile": {
            "peak_resident_memory_mib": 512,
            "load_time_ms": 1200,
            "cpu_real_time_factor_milli": 250,
            "gpu_memory_mib": null,
        },
        "adapter_revision": "0.1.0",
    })
}

fn declare_record() -> DeclareCapabilityRecord {
    serde_json::from_value(record_wire()).expect("the record reads")
}

#[test]
fn a_declaration_reads_every_ess_field_by_its_ess_name() {
    let record = declare_record();

    assert_eq!(record.adapter_id.to_string(), ADAPTER_ID);
    assert_eq!(record.model_id.to_string(), MODEL_ID);
    assert_eq!(record.hardware, "reference machine");
    assert_eq!(record.languages, ["en", "de"]);
    assert_eq!(
        record.audio_formats,
        [AudioFormat {
            encoding: AudioEncoding::PcmS16le,
            sample_rate: 16000.try_into().expect("non-zero"),
            channels: 1.try_into().expect("non-zero"),
        }]
    );
    assert_eq!(
        record.evidence,
        [
            CapabilityEvidence {
                capability: Capability::IncrementalInput,
                tests: vec!["stt::takes_frames_before_the_end".to_owned()],
            },
            CapabilityEvidence {
                capability: Capability::OutputStreaming,
                tests: vec!["stt::hypotheses_before_the_end".to_owned()],
            },
        ]
    );
    assert_eq!(
        record.resource_profile,
        ResourceProfile {
            peak_resident_memory_mib: 512,
            load_time_ms: 1200,
            cpu_real_time_factor_milli: 250,
            gpu_memory_mib: None,
        }
    );
    assert_eq!(record.adapter_revision, "0.1.0");
    assert_eq!(
        serde_json::to_value(&record).expect("serializes"),
        record_wire()
    );
}

#[test]
fn a_gpu_figure_is_read_and_its_absence_is_none() {
    let mut wire = record_wire();
    wire["resource_profile"]["gpu_memory_mib"] = json!(2048);
    let with_gpu: DeclareCapabilityRecord = serde_json::from_value(wire).expect("reads");
    assert_eq!(with_gpu.resource_profile.gpu_memory_mib, Some(2048));

    let mut wire = record_wire();
    wire["resource_profile"]
        .as_object_mut()
        .expect("an object")
        .remove("gpu_memory_mib");
    let without: DeclareCapabilityRecord = serde_json::from_value(wire).expect("reads");
    assert_eq!(without.resource_profile.gpu_memory_mib, None);
}

#[test]
fn an_undeclared_field_is_refused_at_every_level() {
    let mut top = record_wire();
    top["known_limitations"] = json!("none");
    let mut evidence = record_wire();
    evidence["evidence"][0]["verified"] = json!(true);
    let mut profile = record_wire();
    profile["resource_profile"]["peak_vram_mib"] = json!(1);

    for (what, wire) in [
        ("record", top),
        ("evidence", evidence),
        ("resource profile", profile),
    ] {
        let error = serde_json::from_value::<DeclareCapabilityRecord>(wire)
            .expect_err("an undeclared field is refused");
        assert!(
            error.to_string().contains("unknown field"),
            "{what}: {error}"
        );
    }
}

#[test]
fn a_record_declared_as_an_identity_carries_the_input_and_emits_its_event() {
    let capability_record_id = CapabilityRecordId::from(Uuid::from_u128(7));
    let input = declare_record();

    let (record, event) = CapabilityRecord::declare_as(capability_record_id, input.clone());

    assert_eq!(record.capability_record_id(), capability_record_id);
    assert_eq!(record.adapter_id(), input.adapter_id);
    assert_eq!(record.model_id(), input.model_id);
    assert_eq!(record.hardware(), "reference machine");
    assert_eq!(record.languages(), input.languages);
    assert_eq!(record.audio_formats(), input.audio_formats);
    assert_eq!(record.evidence(), input.evidence);
    assert_eq!(record.resource_profile(), input.resource_profile);
    assert_eq!(record.adapter_revision(), "0.1.0");
    assert_eq!(
        event,
        CapabilityRecordDeclared {
            capability_record_id,
            adapter_id: input.adapter_id,
            model_id: input.model_id,
            hardware: "reference machine".to_owned(),
        }
    );
    assert_eq!(
        serde_json::to_value(&event).expect("serializes"),
        json!({
            "capability_record_id": capability_record_id.to_string(),
            "adapter_id": ADAPTER_ID,
            "model_id": MODEL_ID,
            "hardware": "reference machine",
        })
    );

    let (generated, _) = CapabilityRecord::declare(input);
    assert_ne!(generated.capability_record_id(), capability_record_id);
}

#[test]
fn adapter_not_found_serializes_as_its_adapter_id() {
    let adapter_id = AdapterId::generate();
    let refused = AdapterNotFound { adapter_id };

    let wire = serde_json::to_value(refused).expect("serializes");
    assert_eq!(wire, json!({"adapter_id": adapter_id.to_string()}));
    assert_eq!(
        serde_json::from_value::<AdapterNotFound>(wire).expect("deserializes"),
        refused
    );
    assert_eq!(
        refused.to_string(),
        format!("no adapter carries the identity {adapter_id}")
    );
}
