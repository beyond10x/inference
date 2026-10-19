//! Adversary guard for the subgraph interface check of planning (`story:subgraph-declaration`):
//! ports and exposures match one to one, so two ports of one name cannot both take the child's
//! one exposure of that name. The runtime bridges each port to the exposure of its name once.

use std::num::NonZeroUsize;

use inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, NodeRole, OverloadPolicy,
    PipelineSpec, PlanPipelineError, PortTraits, SubgraphInterfaceMismatch, modality,
};
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

const TEXT: PortTraits = PortTraits {
    essentiality: Essentiality::Essential,
    delivery: Delivery::Reliable,
    incremental: true,
    end_marker_required: true,
    provisional_allowed: false,
};

fn spec(name: &str) -> PipelineSpec {
    PipelineSpec {
        name: name.to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: vec![Capability::OutputStreaming],
    }
}

fn four() -> NonZeroUsize {
    NonZeroUsize::new(4).expect("4")
}

#[test]
fn two_ports_of_one_name_cannot_both_take_the_childs_one_exposure_of_that_name() {
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
    let known = Known { profile, adapter };
    let profile_id = known.profile.profile_id();
    let mut declaration = Declaration::new();
    let (child, egress) = declaration
        .build(&known, profile_id, spec("child"), |pipeline| {
            let model = pipeline.node("model", NodeRole::Model, vec![Capability::OutputStreaming]);
            let prompt = pipeline.input::<modality::Text>(&model, "prompt", TEXT);
            let answer = pipeline.output::<modality::Text>(&model, "answer", TEXT);
            pipeline.expose(&prompt, "user text", four(), OverloadPolicy::Pause);
            let egress = pipeline.expose(&answer, "reply", four(), OverloadPolicy::Pause);
            pipeline
                .bind(&model, known.adapter.adapter_id())
                .expect("known");
            egress.id()
        })
        .expect("known");
    let validated = declaration.validate(child).expect("valid");
    let _planned = declaration.plan(validated, &known).expect("planned");
    let (parent, (node, second)) = declaration
        .build(&known, profile_id, spec("parent"), |pipeline| {
            let source = pipeline.node("source", NodeRole::Source, Vec::new());
            let emitted = pipeline.output::<modality::Text>(&source, "text", TEXT);
            let node = pipeline
                .subgraph("frontend", NodeRole::Transform, child)
                .expect("Planned");
            let into = pipeline.input::<modality::Text>(&node, "user text", TEXT);
            let first = pipeline.output::<modality::Text>(&node, "reply", TEXT);
            let second = pipeline.output::<modality::Text>(&node, "reply", TEXT);
            let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
            let collected = pipeline.input::<modality::Text>(&sink, "text", TEXT);
            pipeline.connect(&emitted, &into, four(), OverloadPolicy::Pause);
            pipeline.connect(&first, &collected, four(), OverloadPolicy::Pause);
            pipeline.connect(&second, &collected, four(), OverloadPolicy::Pause);
            (node.id(), second.id())
        })
        .expect("known");
    let validated = declaration.validate(parent).expect("valid");
    assert_eq!(
        declaration.plan(validated, &known).err(),
        Some(PlanPipelineError::InterfaceMismatch(
            SubgraphInterfaceMismatch {
                node_id: node,
                port_id: Some(second),
                exposure_id: Some(egress),
            }
        )),
        "the second `reply` port finds the child's one `reply` exposure taken"
    );
}
