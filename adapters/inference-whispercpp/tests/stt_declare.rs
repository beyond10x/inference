//! The pinned checkpoint and the model and adapter the recogniser declares from it: a batch
//! adapter claiming neither incremental input nor output streaming. Needs no weights.

use inference_stt::CONTRACT_REVISION;
use inference_stt::inference_core::model::{
    ExecutionForm, Locality, Modality, NonPositiveConcurrency,
};
use inference_whispercpp::stt::{
    BACKEND, CHECKPOINT, CODE_LICENSE, DEFAULT_MAX_CONCURRENCY, FAMILY, FILE_NAME, NATIVE_VERSION,
    PROVENANCE, SHA256, SIZE_BYTES, SOURCE_REVISION, SOURCE_URL, WEIGHT_LICENSE,
    WHISPER_RS_VERSION, checksum, declare_adapter, declare_model, native_version,
};

#[test]
fn the_pin_names_the_checkpoint_its_source_revision_hash_and_licences() {
    assert_eq!(CHECKPOINT, "ggml-base.en");
    assert_eq!(FILE_NAME, "ggml-base.en.bin");
    assert_eq!(FAMILY, "whisper");
    assert_eq!(
        SOURCE_URL,
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/\
         5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base.en.bin"
    );
    assert_eq!(SOURCE_REVISION, "5359861c739e955e79d9a303bcbc70fb988958b1");
    assert!(SOURCE_URL.contains(SOURCE_REVISION));
    assert!(SOURCE_URL.ends_with(FILE_NAME));
    assert_eq!(
        SHA256,
        "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002"
    );
    assert_eq!(SIZE_BYTES, 147_964_211);
    assert_eq!(WEIGHT_LICENSE, "MIT");
    assert!(CODE_LICENSE.contains("MIT (whisper.cpp)"), "{CODE_LICENSE}");
    assert!(
        CODE_LICENSE.contains("Unlicense (whisper-rs 0.16.0)"),
        "{CODE_LICENSE}"
    );
    assert_eq!(WHISPER_RS_VERSION, "0.16.0");
    assert_eq!(BACKEND, "whisper.cpp via whisper-rs 0.16.0");
    assert_eq!(DEFAULT_MAX_CONCURRENCY, 1);
}

#[test]
fn the_checksum_names_the_file_and_its_sha256() {
    assert_eq!(checksum(), format!("sha256:{SHA256} {FILE_NAME}"));
}

#[test]
fn the_compiled_whisper_cpp_is_the_pinned_version() {
    assert_eq!(NATIVE_VERSION, "1.8.3");
    assert_eq!(native_version(), NATIVE_VERSION);
}

#[test]
fn the_declared_model_is_the_pinned_checkpoint_from_audio_to_text() {
    let (model, declared) = declare_model();
    assert_eq!(declared.model_id, model.model_id());
    assert_eq!(declared.checkpoint, CHECKPOINT);
    assert_eq!(model.checkpoint(), CHECKPOINT);
    assert_eq!(model.family(), FAMILY);
    assert_eq!(declared.family, FAMILY);
    assert_eq!(model.code_license(), CODE_LICENSE);
    assert_eq!(model.weight_license(), WEIGHT_LICENSE);
    assert_eq!(model.checksum(), checksum());
    assert_eq!(model.source_revision(), SOURCE_REVISION);
    assert_eq!(model.provenance(), PROVENANCE);
    for named in [SOURCE_URL, SOURCE_REVISION, "OpenAI", "whisper.cpp"] {
        assert!(
            model.provenance().contains(named),
            "the provenance names {named}: {}",
            model.provenance()
        );
    }
    assert_eq!(model.input_modalities(), [Modality::Audio]);
    assert_eq!(model.output_modalities(), [Modality::Text]);
}

#[test]
fn the_declared_adapter_runs_native_ffi_in_process_and_claims_no_streaming() {
    let (model, _) = declare_model();
    let (adapter, declared) = declare_adapter(model.model_id(), 3).expect("3 is positive");
    assert_eq!(adapter.model_id(), model.model_id());
    assert_eq!(
        adapter.backend(),
        "whisper.cpp via whisper-rs 0.16.0 (ggml-base.en)"
    );
    assert_eq!(adapter.contract_revision(), CONTRACT_REVISION);
    assert_eq!(adapter.execution_form(), ExecutionForm::NativeFfi);
    assert_eq!(adapter.locality(), Locality::SameProcess);
    assert_eq!(adapter.max_concurrency(), 3);
    assert_eq!(
        adapter.capabilities(),
        [],
        "neither incremental input nor output streaming is claimed"
    );
    assert_eq!(declared.capabilities, adapter.capabilities());
    assert_eq!(declared.model_id, model.model_id());
}

#[test]
fn a_non_positive_maximum_concurrency_declares_no_adapter() {
    let (model, _) = declare_model();
    assert_eq!(
        declare_adapter(model.model_id(), 0).map(|_| ()),
        Err(NonPositiveConcurrency { submitted: 0 })
    );
}
