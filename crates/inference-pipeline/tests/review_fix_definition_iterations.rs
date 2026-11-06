//! Review fix N19 (`story:pipeline-tool-and-node-cancel-findings`): a definition declares a
//! feedback loop's `max_iterations` as the builder declares it, so a maximum beyond what the ESS
//! `Integer` carries is held as `i64::MAX`, as an edge's capacity is.

use std::num::NonZeroUsize;

use inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality, Modality, ModelId,
};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, DeclareNodeDescriptor, Delivery, Essentiality, InteractionMode,
    NodeDescriptors, NodeKind, NodeRole, OverloadPolicy, PipelineDefinition, PipelineSpec,
    PortDescriptor, PortDirection, PortTraits, modality,
};
use serde_json::json;
use uuid::Uuid;

struct Known {
    profile: DeploymentProfile,
    adapter: Adapter,
}

impl Catalog for Known {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.profile.profile_id() == profile_id).then_some(&self.profile)
    }

    fn adapter(&self, adapter_id: AdapterId) -> Option<&Adapter> {
        (self.adapter.adapter_id() == adapter_id).then_some(&self.adapter)
    }
}

fn known() -> Known {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: vec![Locality::SameProcess],
    })
    .expect("a row of the table");
    let (adapter, _) = Adapter::declare(DeclareAdapter {
        model_id: ModelId::from(Uuid::from_u128(9)),
        backend: "mock".to_owned(),
        contract_revision: "llm/1".to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities: vec![Capability::OutputStreaming],
        max_concurrency: 1,
    })
    .expect("a positive concurrency");
    Known { profile, adapter }
}

const TEXT: PortTraits = PortTraits {
    essentiality: Essentiality::Essential,
    delivery: Delivery::Reliable,
    incremental: true,
    end_marker_required: true,
    provisional_allowed: false,
};

fn text(name: &str, direction: PortDirection) -> PortDescriptor {
    PortDescriptor::new(name, direction, Modality::Text, TEXT)
}

/// `model` (prompt in, answer out).
fn descriptors() -> NodeDescriptors {
    NodeDescriptors::of([DeclareNodeDescriptor {
        kind: NodeKind::new("model"),
        role: NodeRole::Model,
        requires: vec![Capability::OutputStreaming],
        ports: vec![
            text("prompt", PortDirection::Input),
            text("answer", PortDirection::Output),
        ],
    }])
    .expect("one kind")
}

#[test]
fn a_definition_feedback_loop_maximum_beyond_the_ess_integer_is_held_as_the_builder_holds_it() {
    let known = known();
    let beyond = u64::try_from(i64::MAX).expect("positive") + 1;
    let json = json!({
        "name": "a loop",
        "schema_revision": "1",
        "mode": "turn_based",
        "profile_id": known.profile.profile_id(),
        "nodes": [{"name": "model", "kind": "model"}],
        "edges": [{
            "from": {"node": "model", "port": "answer"},
            "to": {"node": "model", "port": "prompt"},
            "capacity": 2,
            "overload": "pause"
        }],
        "feedback_loops": [{
            "from": {"node": "model", "port": "answer"},
            "to": {"node": "model", "port": "prompt"},
            "max_iterations": beyond
        }],
        "bindings": [{"node": "model", "adapter_id": known.adapter.adapter_id()}]
    });
    let definition = PipelineDefinition::from_json(&json.to_string()).expect("of the shape");
    let mut declared = Declaration::new();
    declared
        .declare_definition(&definition, &descriptors(), &known)
        .expect("the check passes");

    let mut built = Declaration::new();
    built
        .build(
            &known,
            known.profile.profile_id(),
            PipelineSpec {
                name: "a loop".to_owned(),
                schema_revision: "1".to_owned(),
                mode: InteractionMode::TurnBased,
                required_capabilities: Vec::new(),
            },
            |pipeline| {
                let model =
                    pipeline.node("model", NodeRole::Model, vec![Capability::OutputStreaming]);
                let prompt = pipeline.input::<modality::Text>(&model, "prompt", TEXT);
                let answer = pipeline.output::<modality::Text>(&model, "answer", TEXT);
                pipeline.connect_feedback(
                    &answer,
                    &prompt,
                    NonZeroUsize::new(2).expect("not zero"),
                    OverloadPolicy::Pause,
                    NonZeroUsize::new(usize::try_from(beyond).expect("64-bit")).expect("not zero"),
                );
            },
        )
        .expect("the profile is known");

    let held = |declaration: &Declaration| {
        declaration
            .feedback_loops()
            .map(|feedback| feedback.max_iterations().get())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        held(&built),
        [usize::try_from(i64::MAX).expect("64-bit")],
        "the builder holds a maximum beyond i64::MAX as i64::MAX"
    );
    assert_eq!(
        held(&declared),
        held(&built),
        "the definition declares the maximum the builder declares for the same loop"
    );
}
