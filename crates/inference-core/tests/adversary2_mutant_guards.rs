//! Adversary cases for story:workspace-skeleton, pass 2: lines of the public API no earlier case
//! observes. Each is green on the tree and red on the mutant its name gives.

use inference_core::id::{GenerationId, NodeId, RunId, SessionId, StreamId};
use inference_core::model::{
    AdapterId, Capability, ExecutionForm, Locality, Modality, ModelId, NonPositiveConcurrency,
};
use serde_json::{Value, json};
use uuid::Uuid;

const CANONICAL: &str = "00000000-0000-4000-8000-dfb547682652";

#[test]
fn adversary2_identifier_into_uuid_returns_the_wrapped_uuid() {
    let uuid = Uuid::parse_str(CANONICAL).expect("canonical uuid");
    assert_eq!(Uuid::from(ModelId::from(uuid)), uuid);
    assert_eq!(Uuid::from(AdapterId::from(uuid)), uuid);
    assert_eq!(Uuid::from(RunId::from(uuid)), uuid);
    assert_eq!(Uuid::from(NodeId::from(uuid)), uuid);
    assert_eq!(Uuid::from(StreamId::from(uuid)), uuid);
    assert_eq!(Uuid::from(GenerationId::from(uuid)), uuid);
    assert_eq!(Uuid::from(SessionId::from(uuid)), uuid);
}

#[test]
fn adversary2_identifier_display_is_the_canonical_uuid_text() {
    let uuid = Uuid::parse_str(CANONICAL).expect("canonical uuid");
    assert_eq!(RunId::from(uuid).to_string(), CANONICAL);
    assert_eq!(NodeId::from(uuid).to_string(), CANONICAL);
    assert_eq!(StreamId::from(uuid).to_string(), CANONICAL);
    assert_eq!(GenerationId::from(uuid).to_string(), CANONICAL);
    assert_eq!(SessionId::from(uuid).to_string(), CANONICAL);
    assert_eq!(ModelId::from(uuid).to_string(), CANONICAL);
    assert_eq!(AdapterId::from(uuid).to_string(), CANONICAL);
}

#[test]
fn adversary2_all_lists_the_variants_in_the_order_ess_lists_them() {
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
        serde_json::to_value(Modality::ALL).expect("serializes"),
        json!(["Text", "Audio", "Structured", "Image", "Video", "Control"])
    );
    assert_eq!(
        serde_json::to_value(Locality::ALL).expect("serializes"),
        json!([
            "same_process",
            "local_worker",
            "self_hosted_cluster",
            "external_provider"
        ])
    );
    assert_eq!(
        serde_json::to_value(ExecutionForm::ALL).expect("serializes"),
        json!([
            "in_process",
            "native_ffi",
            "subprocess",
            "local_service",
            "hosted_service"
        ])
    );
}

#[test]
fn adversary2_refusal_message_carries_the_submitted_value() {
    let message = NonPositiveConcurrency { submitted: -5 }.to_string();
    assert!(message.ends_with("-5"), "{message}");
    let refused: Value =
        serde_json::to_value(NonPositiveConcurrency { submitted: 0 }).expect("error");
    assert_eq!(refused, json!({"submitted": 0}));
}
