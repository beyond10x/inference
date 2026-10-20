//! The pinned checkpoint and the model and adapter the recogniser declares from it. Needs no
//! weights.
#![cfg(feature = "stt")]

use std::collections::HashSet;

use inference_sherpa_onnx::stt::{
    BACKEND, CHECKPOINT, CODE_LICENSE, FILES, FileRole, REFERENCE_CLIP, SOURCE_REVISION,
    SOURCE_URL, WEIGHT_LICENSE, checksum, declare_adapter, declare_model,
};
use inference_stt::CONTRACT_REVISION;
use inference_stt::inference_core::model::{
    Capability, ExecutionForm, Locality, Modality, NonPositiveConcurrency,
};

const ENCODER_SHA256: &str = "a423883ce5754507fd941755ab0b5bc426a84ac670cbe21cf060e9e2c66dc660";
const DECODER_SHA256: &str = "7bf787f90b194b307e5a4ad6a34fadb4e748304c35f78a8d66358a05b13ee6ef";
const JOINER_SHA256: &str = "210591f72b3c56b8364f85f345dca240bc2b4c00632848f4aa923630d5639d3b";
const TOKENS_SHA256: &str = "49e3c2646595fd907228b3c6787069658f67b17377c60aeb8619c4551b2316fb";

#[test]
fn the_pin_names_the_checkpoint_its_source_revision_and_licences() {
    assert_eq!(CHECKPOINT, "sherpa-onnx-streaming-zipformer-en-2023-06-26");
    assert_eq!(
        SOURCE_URL,
        "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-2023-06-26"
    );
    assert_eq!(SOURCE_REVISION, "672fbf1b30579d6585301139bb363f42a0ad4a24");
    assert_eq!(WEIGHT_LICENSE, "Apache-2.0");
    assert!(CODE_LICENSE.contains("Apache-2.0"), "{CODE_LICENSE}");
    assert!(CODE_LICENSE.contains("sherpa-onnx"), "{CODE_LICENSE}");
    assert!(CODE_LICENSE.contains("MIT"), "{CODE_LICENSE}");
    assert!(CODE_LICENSE.contains("onnxruntime"), "{CODE_LICENSE}");
    assert_eq!(BACKEND, "sherpa-onnx 1.13.8");
}

#[test]
fn the_pin_carries_the_sha256_of_each_file_the_recogniser_loads() {
    let pinned: Vec<(FileRole, &str, &str)> = FILES
        .iter()
        .map(|file| (file.role, file.name, file.sha256))
        .collect();
    assert_eq!(
        pinned,
        vec![
            (
                FileRole::Encoder,
                "encoder-epoch-99-avg-1-chunk-16-left-128.onnx",
                ENCODER_SHA256
            ),
            (
                FileRole::Decoder,
                "decoder-epoch-99-avg-1-chunk-16-left-128.onnx",
                DECODER_SHA256
            ),
            (
                FileRole::Joiner,
                "joiner-epoch-99-avg-1-chunk-16-left-128.onnx",
                JOINER_SHA256
            ),
            (FileRole::Tokens, "tokens.txt", TOKENS_SHA256),
        ]
    );
}

#[test]
fn the_reference_clip_is_pinned_with_its_transcript_and_is_not_loaded() {
    assert_eq!(REFERENCE_CLIP.name, "test_wavs/0.wav");
    assert_eq!(
        REFERENCE_CLIP.sha256,
        "6bc58a4efdf20daac252b6b1502632601a71efe0308f6757dc1eda34891a7e4f"
    );
    assert_eq!(
        REFERENCE_CLIP.transcript,
        "AFTER EARLY NIGHTFALL THE YELLOW LAMPS WOULD LIGHT UP HERE AND THERE THE SQUALID QUARTER \
         OF THE BROTHELS"
    );
    assert!(FILES.iter().all(|file| file.name != REFERENCE_CLIP.name));
}

#[test]
fn the_checksum_names_every_pinned_file_and_its_sha256() {
    let checksum = checksum();
    for file in FILES {
        assert!(
            checksum.contains(&format!("sha256:{} {}", file.sha256, file.name)),
            "{checksum} names {}",
            file.name
        );
    }
}

#[test]
fn the_declared_model_is_the_pinned_checkpoint_from_audio_to_text() {
    let (model, declared) = declare_model();
    assert_eq!(declared.model_id, model.model_id());
    assert_eq!(declared.checkpoint, CHECKPOINT);
    assert_eq!(model.checkpoint(), CHECKPOINT);
    assert_eq!(declared.family, model.family());
    assert!(!model.family().is_empty());
    assert_eq!(model.code_license(), CODE_LICENSE);
    assert_eq!(model.weight_license(), WEIGHT_LICENSE);
    assert_eq!(model.checksum(), checksum());
    assert_eq!(model.source_revision(), SOURCE_REVISION);
    assert!(
        model.provenance().contains(SOURCE_URL),
        "{}",
        model.provenance()
    );
    assert!(
        model.provenance().contains("LibriSpeech"),
        "{}",
        model.provenance()
    );
    assert_eq!(model.input_modalities(), [Modality::Audio]);
    assert_eq!(model.output_modalities(), [Modality::Text]);
}

#[test]
fn the_declared_adapter_runs_native_ffi_in_process_and_claims_streaming() {
    let (model, _) = declare_model();
    let (adapter, declared) = declare_adapter(model.model_id(), 3).expect("3 is positive");
    assert_eq!(adapter.model_id(), model.model_id());
    assert_eq!(
        adapter.backend(),
        "sherpa-onnx 1.13.8 (sherpa-onnx-streaming-zipformer-en-2023-06-26)"
    );
    assert_eq!(adapter.contract_revision(), CONTRACT_REVISION);
    assert_eq!(adapter.execution_form(), ExecutionForm::NativeFfi);
    assert_eq!(adapter.locality(), Locality::SameProcess);
    assert_eq!(adapter.max_concurrency(), 3);
    let claims: HashSet<Capability> = adapter.capabilities().iter().copied().collect();
    assert_eq!(
        claims,
        HashSet::from([Capability::IncrementalInput, Capability::OutputStreaming])
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
