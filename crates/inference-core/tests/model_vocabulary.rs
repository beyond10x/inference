//! The three checks of the workspace-skeleton story, which together observe the `inference.model`
//! vocabulary of `ess/domains/model.yaml` and the shared identifiers.

use std::collections::HashSet;
use std::fmt::Debug;

use inference_core::id::{GenerationId, NodeId, RunId, SessionId, StreamId};
use inference_core::model::{
    Adapter, AdapterDeclared, AdapterId, Capability, DeclareAdapter, DeclareModel, ExecutionForm,
    Locality, Modality, Model, ModelDeclared, ModelId, NonPositiveConcurrency,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;

fn declare_model_input() -> DeclareModel {
    DeclareModel {
        family: "whisper".to_owned(),
        checkpoint: "large-v3".to_owned(),
        code_license: "MIT".to_owned(),
        weight_license: "Apache-2.0".to_owned(),
        checksum: "sha256:0f1e2d3c".to_owned(),
        input_modalities: vec![Modality::Audio, Modality::Control],
        output_modalities: vec![Modality::Text, Modality::Structured],
    }
}

fn declare_adapter_input(model_id: ModelId, max_concurrency: i64) -> DeclareAdapter {
    DeclareAdapter {
        model_id,
        backend: "whisper-rs".to_owned(),
        contract_revision: "stt/1".to_owned(),
        execution_form: ExecutionForm::NativeFfi,
        locality: Locality::LocalWorker,
        capabilities: vec![Capability::OutputStreaming, Capability::IncrementalInput],
        max_concurrency,
    }
}

#[test]
fn check_1_declare_model_returns_the_model_and_model_declared() {
    let input = declare_model_input();

    let (model, event): (Model, ModelDeclared) = Model::declare(input.clone());

    assert_eq!(model.family(), input.family);
    assert_eq!(model.checkpoint(), input.checkpoint);
    assert_eq!(model.code_license(), input.code_license);
    assert_eq!(model.weight_license(), input.weight_license);
    assert_eq!(model.checksum(), input.checksum);
    assert_eq!(model.input_modalities(), input.input_modalities.as_slice());
    assert_eq!(
        model.output_modalities(),
        input.output_modalities.as_slice()
    );

    assert_eq!(
        event,
        ModelDeclared {
            model_id: model.model_id(),
            family: input.family.clone(),
            checkpoint: input.checkpoint.clone(),
        }
    );
    assert_ne!(model.model_id().as_uuid(), Uuid::nil());

    let (again, _) = Model::declare(input);
    assert_ne!(
        again.model_id(),
        model.model_id(),
        "model_id is generated per declaration"
    );
}

#[test]
fn check_2_declare_adapter_refuses_non_positive_concurrency_and_declares_at_one() {
    let model_id = ModelId::generate();

    for submitted in [0, -1, -7, i64::MIN] {
        assert_eq!(
            Adapter::declare(declare_adapter_input(model_id, submitted)),
            Err(NonPositiveConcurrency { submitted }),
            "max_concurrency {submitted} is refused and nothing is declared"
        );
    }

    let input = declare_adapter_input(model_id, 1);
    let (adapter, event): (Adapter, AdapterDeclared) =
        Adapter::declare(input.clone()).expect("max_concurrency 1 is declared");

    assert_eq!(adapter.model_id(), input.model_id);
    assert_eq!(adapter.backend(), input.backend);
    assert_eq!(adapter.contract_revision(), input.contract_revision);
    assert_eq!(adapter.execution_form(), input.execution_form);
    assert_eq!(adapter.locality(), input.locality);
    assert_eq!(adapter.capabilities(), input.capabilities.as_slice());
    assert_eq!(adapter.max_concurrency(), 1);
    assert_ne!(adapter.adapter_id().as_uuid(), Uuid::nil());

    assert_eq!(
        event,
        AdapterDeclared {
            adapter_id: adapter.adapter_id(),
            model_id: input.model_id,
            capabilities: input.capabilities.clone(),
        }
    );
}

fn capability_ess_name(capability: Capability) -> &'static str {
    match capability {
        Capability::OutputStreaming => "output_streaming",
        Capability::IncrementalInput => "incremental_input",
        Capability::ConcurrentInputOutput => "concurrent_input_output",
        Capability::NativeDuplex => "native_duplex",
        Capability::RevisableInput => "revisable_input",
        Capability::ToolCalling => "tool_calling",
        Capability::Interruptible => "interruptible",
        Capability::ResumableModelState => "resumable_model_state",
        Capability::AudioTextAlignment => "audio_text_alignment",
    }
}

fn modality_ess_name(modality: Modality) -> &'static str {
    match modality {
        Modality::Text => "Text",
        Modality::Audio => "Audio",
        Modality::Structured => "Structured",
        Modality::Image => "Image",
        Modality::Video => "Video",
        Modality::Control => "Control",
    }
}

fn locality_ess_name(locality: Locality) -> &'static str {
    match locality {
        Locality::SameProcess => "same_process",
        Locality::LocalWorker => "local_worker",
        Locality::SelfHostedCluster => "self_hosted_cluster",
        Locality::ExternalProvider => "external_provider",
    }
}

fn execution_form_ess_name(form: ExecutionForm) -> &'static str {
    match form {
        ExecutionForm::InProcess => "in_process",
        ExecutionForm::NativeFfi => "native_ffi",
        ExecutionForm::Subprocess => "subprocess",
        ExecutionForm::LocalService => "local_service",
        ExecutionForm::HostedService => "hosted_service",
    }
}

fn assert_enum_matches_ess<T>(all: &[T], ess_count: usize, ess_name: fn(T) -> &'static str)
where
    T: Copy + Debug + PartialEq + Serialize + DeserializeOwned,
{
    assert_eq!(all.len(), ess_count, "every variant ESS lists is present");
    let names: HashSet<&str> = all.iter().map(|variant| ess_name(*variant)).collect();
    assert_eq!(names.len(), ess_count, "each variant has its own ESS name");

    for variant in all {
        let expected = ess_name(*variant);
        let wire = serde_json::to_value(variant).expect("variant serializes");
        assert_eq!(
            wire,
            serde_json::Value::String(expected.to_owned()),
            "{variant:?}"
        );
        let back: T = serde_json::from_value(wire).expect("ESS name deserializes");
        assert_eq!(back, *variant);
    }
}

fn assert_uuid_newtype_round_trips<T>(wrap: fn(Uuid) -> T, unwrap: fn(&T) -> Uuid)
where
    T: Debug + PartialEq + Serialize + DeserializeOwned,
{
    let uuid = Uuid::new_v4();
    let id = wrap(uuid);
    assert_eq!(unwrap(&id), uuid);
    let wire = serde_json::to_value(&id).expect("identifier serializes");
    assert_eq!(wire, serde_json::Value::String(uuid.to_string()));
    let back: T = serde_json::from_value(wire).expect("Uuid string deserializes");
    assert_eq!(back, id);
}

#[test]
fn check_3_vocabulary_serializes_to_ess_names_and_round_trips() {
    assert_enum_matches_ess(&Capability::ALL, 9, capability_ess_name);
    assert_enum_matches_ess(&Modality::ALL, 6, modality_ess_name);
    assert_enum_matches_ess(&Locality::ALL, 4, locality_ess_name);
    assert_enum_matches_ess(&ExecutionForm::ALL, 5, execution_form_ess_name);

    assert_uuid_newtype_round_trips(ModelId::from, |id| id.as_uuid());
    assert_uuid_newtype_round_trips(AdapterId::from, |id| id.as_uuid());
    assert_uuid_newtype_round_trips(RunId::from, |id| id.as_uuid());
    assert_uuid_newtype_round_trips(NodeId::from, |id| id.as_uuid());
    assert_uuid_newtype_round_trips(StreamId::from, |id| id.as_uuid());
    assert_uuid_newtype_round_trips(GenerationId::from, |id| id.as_uuid());
    assert_uuid_newtype_round_trips(SessionId::from, |id| id.as_uuid());
}
