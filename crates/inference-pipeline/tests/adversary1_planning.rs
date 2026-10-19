//! Adversary pass 1 against `inference.graph.PlanPipeline`: the refusal order between a node's
//! mismatch and the pipeline's, nodes off every required path, two refusing nodes, a node bound
//! twice, and a pipeline whose bound adapters lie on no required path.

use std::num::NonZeroUsize;

use inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    CapabilityMismatch, Catalog, Declaration, Essentiality, InteractionMode, NodeHandle, NodeRole,
    NodeUnbound, OverloadPolicy, PipelineBuilder, PipelineSpec, PlanPipelineError, PortTraits,
    modality,
};
use uuid::Uuid;

use Capability::{Interruptible, NativeDuplex, OutputStreaming, ToolCalling};

struct Known {
    profile: DeploymentProfile,
    adapters: Vec<Adapter>,
}

impl Catalog for Known {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.profile.profile_id() == profile_id).then_some(&self.profile)
    }

    fn adapter(&self, adapter_id: AdapterId) -> Option<&Adapter> {
        self.adapters
            .iter()
            .find(|adapter| adapter.adapter_id() == adapter_id)
    }
}

fn known(adapters: Vec<Vec<Capability>>) -> Known {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: vec![Locality::SameProcess],
    })
    .expect("a row of the table");
    Known {
        profile,
        adapters: adapters
            .into_iter()
            .map(|capabilities| {
                Adapter::declare(DeclareAdapter {
                    model_id: ModelId::from(Uuid::from_u128(7)),
                    backend: "mock".to_owned(),
                    contract_revision: "llm/1".to_owned(),
                    execution_form: ExecutionForm::InProcess,
                    locality: Locality::SameProcess,
                    capabilities,
                    max_concurrency: 1,
                })
                .expect("a positive concurrency")
                .0
            })
            .collect(),
    }
}

fn spec(required_capabilities: Vec<Capability>) -> PipelineSpec {
    PipelineSpec {
        name: "adversary".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities,
    }
}

fn chain<'b>(
    pipeline: &mut PipelineBuilder<'_, 'b>,
    nodes: &[&NodeHandle<'b>],
    essentiality: Essentiality,
) {
    let traits = PortTraits {
        essentiality,
        ..PortTraits::default()
    };
    for pair in nodes.windows(2) {
        let out = pipeline.output::<modality::Text>(pair[0], "out", traits);
        let into = pipeline.input::<modality::Text>(pair[1], "in", traits);
        pipeline.connect(&out, &into, NonZeroUsize::MIN, OverloadPolicy::Pause);
    }
}

#[test]
fn a_node_mismatch_is_named_before_a_mismatch_of_the_pipeline() {
    let known = known(vec![vec![OutputStreaming]]);
    let mut declaration = Declaration::counted();
    let (pipeline_id, model) = declaration
        .build(
            &known,
            known.profile.profile_id(),
            spec(vec![NativeDuplex]),
            |p| {
                let source = p.node("source", NodeRole::Source, Vec::new());
                let model = p.node("llm", NodeRole::Model, vec![ToolCalling]);
                let sink = p.node("sink", NodeRole::Sink, Vec::new());
                chain(p, &[&source, &model, &sink], Essentiality::Essential);
                p.bind(&model, known.adapters[0].adapter_id())
                    .expect("the adapter is declared");
                model.id()
            },
        )
        .expect("the profile is declared");
    let validated = declaration
        .validate(pipeline_id)
        .expect("the graph is valid");
    assert_eq!(
        declaration.plan(validated, &known),
        Err(PlanPipelineError::Mismatch(CapabilityMismatch {
            node_id: Some(model),
            capability: ToolCalling,
        })),
        "the node's own mismatch is decided before the pipeline's NativeDuplex"
    );
}

#[test]
fn a_node_off_every_required_path_is_still_held_to_its_requirements() {
    let known = known(vec![vec![OutputStreaming]]);
    let mut declaration = Declaration::counted();
    let (pipeline_id, (side, lone)) = declaration
        .build(&known, known.profile.profile_id(), spec(Vec::new()), |p| {
            let source = p.node("source", NodeRole::Source, Vec::new());
            let side = p.node("side", NodeRole::Model, vec![ToolCalling]);
            let sink = p.node("sink", NodeRole::Sink, Vec::new());
            let lone = p.node("lone", NodeRole::Tool, vec![Interruptible]);
            chain(p, &[&source, &sink], Essentiality::Essential);
            chain(p, &[&source, &side, &sink], Essentiality::Optional);
            p.bind(&side, known.adapters[0].adapter_id())
                .expect("the adapter is declared");
            (side.id(), lone.id())
        })
        .expect("the profile is declared");
    assert_eq!(declaration.findings(pipeline_id), []);
    assert_eq!(
        declaration.plan_of(pipeline_id, &known),
        Err(PlanPipelineError::Unbound(NodeUnbound { node_id: lone })),
        "a node on no path with a requirement and no binding is unbound"
    );

    let mut declaration = Declaration::counted();
    let (pipeline_id, side_only) = declaration
        .build(&known, known.profile.profile_id(), spec(Vec::new()), |p| {
            let source = p.node("source", NodeRole::Source, Vec::new());
            let side = p.node("side", NodeRole::Model, vec![ToolCalling]);
            let sink = p.node("sink", NodeRole::Sink, Vec::new());
            chain(p, &[&source, &sink], Essentiality::Essential);
            chain(p, &[&source, &side, &sink], Essentiality::Optional);
            p.bind(&side, known.adapters[0].adapter_id())
                .expect("the adapter is declared");
            side.id()
        })
        .expect("the profile is declared");
    assert_ne!(side, lone);
    assert_eq!(
        declaration.plan_of(pipeline_id, &known),
        Err(PlanPipelineError::Mismatch(CapabilityMismatch {
            node_id: Some(side_only),
            capability: ToolCalling,
        })),
        "a node on an optional branch is still refused for its own unmet requirement"
    );
}

#[test]
fn of_two_refusing_nodes_the_first_declared_is_named() {
    let known = known(vec![vec![OutputStreaming]]);
    let mut declaration = Declaration::counted();
    let (pipeline_id, (first, _second)) = declaration
        .build(&known, known.profile.profile_id(), spec(Vec::new()), |p| {
            let source = p.node("source", NodeRole::Source, Vec::new());
            let first = p.node("first", NodeRole::Model, vec![ToolCalling]);
            let second = p.node("second", NodeRole::Model, vec![NativeDuplex]);
            let sink = p.node("sink", NodeRole::Sink, Vec::new());
            chain(
                p,
                &[&source, &first, &second, &sink],
                Essentiality::Essential,
            );
            p.bind(&first, known.adapters[0].adapter_id())
                .expect("the adapter is declared");
            p.bind(&second, known.adapters[0].adapter_id())
                .expect("the adapter is declared");
            (first.id(), second.id())
        })
        .expect("the profile is declared");
    assert_eq!(
        declaration.plan_of(pipeline_id, &known),
        Err(PlanPipelineError::Mismatch(CapabilityMismatch {
            node_id: Some(first),
            capability: ToolCalling,
        }))
    );

    let mut declaration = Declaration::counted();
    let (pipeline_id, first) = declaration
        .build(&known, known.profile.profile_id(), spec(Vec::new()), |p| {
            let first = p.node("first", NodeRole::Model, vec![ToolCalling]);
            let _second = p.node("second", NodeRole::Model, vec![NativeDuplex]);
            first.id()
        })
        .expect("the profile is declared");
    assert_eq!(
        declaration.plan_of(pipeline_id, &known),
        Err(PlanPipelineError::Unbound(NodeUnbound { node_id: first }))
    );
}

#[test]
fn a_node_bound_twice_is_held_to_both_adapters() {
    let known = known(vec![
        vec![OutputStreaming, ToolCalling, Interruptible],
        vec![OutputStreaming, Interruptible],
    ]);
    let mut declaration = Declaration::counted();
    let (pipeline_id, ()) = declaration
        .build(&known, known.profile.profile_id(), spec(Vec::new()), |p| {
            let source = p.node("source", NodeRole::Source, Vec::new());
            let model = p.node("llm", NodeRole::Model, Vec::new());
            let sink = p.node("sink", NodeRole::Sink, Vec::new());
            chain(p, &[&source, &model, &sink], Essentiality::Essential);
            p.bind(&model, known.adapters[0].adapter_id())
                .expect("the adapter is declared");
            p.bind(&model, known.adapters[1].adapter_id())
                .expect("the adapter is declared");
        })
        .expect("the profile is declared");
    assert_eq!(
        declaration.plan_of(pipeline_id, &known),
        Ok(vec![OutputStreaming, Interruptible]),
        "the module documents that each binding of a node contributes"
    );
}

#[test]
fn a_pipeline_whose_only_bound_adapter_lacks_its_requirement_is_not_planned() {
    let known = known(vec![vec![OutputStreaming]]);
    let mut declaration = Declaration::counted();
    let (pipeline_id, ()) = declaration
        .build(
            &known,
            known.profile.profile_id(),
            spec(vec![ToolCalling]),
            |p| {
                let model = p.node("llm", NodeRole::Model, Vec::new());
                p.bind(&model, known.adapters[0].adapter_id())
                    .expect("the adapter is declared");
            },
        )
        .expect("the profile is declared");
    let validated = declaration
        .validate(pipeline_id)
        .expect("a lone node is valid");
    let planned = declaration.plan(validated, &known);
    assert_eq!(
        planned
            .as_ref()
            .map(|planned| planned.effective_capabilities()),
        Err(&PlanPipelineError::Mismatch(CapabilityMismatch {
            node_id: None,
            capability: ToolCalling,
        })),
        "the pipeline requires tool_calling and the one adapter bound in it does not implement it"
    );
}
