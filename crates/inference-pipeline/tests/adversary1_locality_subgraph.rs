//! Adversary case for the locality check of planning (`story:cascade-offline-profile`, A-04 of
//! `docs/design/inference.md` at 1140): a pipeline under `offline_strict` that runs, through a
//! subgraph node, a child pipeline whose model is bound to an `external_provider` adapter.
//!
//! The child is declared under a `hybrid_opt_in` profile admitting the external provider, and
//! plans. The parent is declared under `offline_strict` admitting the same process only; its
//! subgraph node is bound to nothing and contributes the child as a bound node contributes its
//! adapter. A-04: the offline profile refuses a hosted adapter.

use std::num::NonZeroUsize;

use inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_core::policy::{DeclareProfile, DeploymentProfile, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, NodeRole, OverloadPolicy,
    PipelineSpec, PlanPipelineError, PortTraits, modality,
};
use uuid::Uuid;

struct Known {
    offline: DeploymentProfile,
    hybrid: DeploymentProfile,
    external: Adapter,
}

impl Catalog for Known {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        [&self.offline, &self.hybrid]
            .into_iter()
            .find(|profile| profile.profile_id() == profile_id)
    }

    fn adapter(&self, adapter_id: AdapterId) -> Option<&Adapter> {
        (self.external.adapter_id() == adapter_id).then_some(&self.external)
    }
}

fn profile(kind: ProfileKind, admitted: Vec<Locality>) -> DeploymentProfile {
    DeploymentProfile::declare(DeclareProfile {
        kind,
        egress: kind.egress(),
        admitted,
    })
    .expect("a row of the table")
    .0
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
        required_capabilities: Vec::new(),
    }
}

#[test]
fn a_hosted_adapter_inside_a_subgraph_is_refused_under_offline_strict() {
    let known = Known {
        offline: profile(ProfileKind::OfflineStrict, vec![Locality::SameProcess]),
        hybrid: profile(
            ProfileKind::HybridOptIn,
            vec![Locality::SameProcess, Locality::ExternalProvider],
        ),
        external: Adapter::declare(DeclareAdapter {
            model_id: ModelId::from(Uuid::from_u128(9)),
            backend: "hosted".to_owned(),
            contract_revision: "llm/1".to_owned(),
            execution_form: ExecutionForm::HostedService,
            locality: Locality::ExternalProvider,
            capabilities: vec![Capability::OutputStreaming],
            max_concurrency: 1,
        })
        .expect("a positive concurrency")
        .0,
    };
    let mut declaration = Declaration::new();
    let capacity = NonZeroUsize::new(4).expect("4");

    let (child, ()) = declaration
        .build(
            &known,
            known.hybrid.profile_id(),
            spec("hosted model"),
            |pipeline| {
                let model = pipeline.node("model", NodeRole::Model, Vec::new());
                let prompt = pipeline.input::<modality::Text>(&model, "prompt", TEXT);
                let answer = pipeline.output::<modality::Text>(&model, "answer", TEXT);
                pipeline.expose(&prompt, "user text", capacity, OverloadPolicy::Pause);
                pipeline.expose(&answer, "reply", capacity, OverloadPolicy::Pause);
                pipeline
                    .bind(&model, known.external.adapter_id())
                    .expect("the adapter is known");
            },
        )
        .expect("the profile is known");
    let validated = declaration.validate(child).expect("the child is valid");
    let _planned = declaration
        .plan(validated, &known)
        .expect("the child plans under its hybrid profile");

    let (parent, ()) = declaration
        .build(
            &known,
            known.offline.profile_id(),
            spec("offline parent"),
            |pipeline| {
                let source = pipeline.node("source", NodeRole::Source, Vec::new());
                let frontend = pipeline
                    .subgraph("frontend", NodeRole::Transform, child)
                    .expect("the child is Planned");
                let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
                let emitted = pipeline.output::<modality::Text>(&source, "text", TEXT);
                let asked = pipeline.input::<modality::Text>(&frontend, "user text", TEXT);
                let replied = pipeline.output::<modality::Text>(&frontend, "reply", TEXT);
                let collected = pipeline.input::<modality::Text>(&sink, "text", TEXT);
                pipeline.connect(&emitted, &asked, capacity, OverloadPolicy::Pause);
                pipeline.connect(&replied, &collected, capacity, OverloadPolicy::Pause);
            },
        )
        .expect("the profile is known");
    let validated = declaration.validate(parent).expect("the parent is valid");
    let planned = declaration.plan(validated, &known);
    assert!(
        matches!(planned, Err(PlanPipelineError::LocalityDenied(_))),
        "an offline_strict pipeline runs an external_provider adapter through its subgraph node, \
         and planning answers {planned:?}"
    );
}
