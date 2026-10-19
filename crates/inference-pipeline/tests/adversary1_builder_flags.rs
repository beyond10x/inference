//! Adversary pass 1 against the typed builder: the three Boolean traits of a port.
//!
//! `incremental`, `end_marker_required` and `provisional_allowed` share one type, so a builder
//! that hands one to the other still compiles. The equivalence case of `declaration.rs` sends
//! `true, true, false`, which no swap of the first two can change, and the conformance suite sends
//! `true, true, true`. Here each flag is set alone, through the builder and through the commands,
//! and the two ports must be equal and carry that one flag.

use inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality, Modality, ModelId,
};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    AddNode, AddPort, Catalog, Declaration, DeclarePipeline, Delivery, Essentiality,
    InteractionMode, NodeRole, PipelineSpec, Port, PortDirection, PortTraits, modality,
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

fn spec() -> PipelineSpec {
    PipelineSpec {
        name: "flags".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::Continuous,
        required_capabilities: Vec::new(),
    }
}

/// The one port declared through the commands with `traits`.
fn through_the_commands(known: &Known, traits: PortTraits) -> Port {
    let mut declaration = Declaration::counted();
    let pipeline_id = declaration
        .declare_pipeline(
            DeclarePipeline {
                name: "flags".to_owned(),
                schema_revision: "1".to_owned(),
                mode: InteractionMode::Continuous,
                profile_id: known.profile.profile_id(),
                required_capabilities: Vec::new(),
            },
            known,
        )
        .expect("the profile is declared")
        .pipeline_id;
    let node_id = declaration
        .add_node(AddNode {
            pipeline_id,
            name: "asr".to_owned(),
            role: NodeRole::Model,
            requires: Vec::new(),
        })
        .expect("the pipeline is declared")
        .node_id;
    let port_id = declaration
        .add_port(AddPort {
            pipeline_id,
            node_id,
            name: "audio".to_owned(),
            direction: PortDirection::Input,
            modality: Modality::Audio,
            essentiality: traits.essentiality,
            delivery: traits.delivery,
            incremental: traits.incremental,
            end_marker_required: traits.end_marker_required,
            provisional_allowed: traits.provisional_allowed,
        })
        .expect("the node exists")
        .port_id;
    declaration.port(port_id).expect("the port").clone()
}

/// The one port declared through the builder with `traits`.
fn through_the_builder(known: &Known, traits: PortTraits) -> Port {
    let mut declaration = Declaration::counted();
    let (_, port_id) = declaration
        .build(known, known.profile.profile_id(), spec(), |pipeline| {
            let node = pipeline.node("asr", NodeRole::Model, Vec::new());
            pipeline
                .input::<modality::Audio>(&node, "audio", traits)
                .id()
        })
        .expect("the profile is declared");
    declaration.port(port_id).expect("the port").clone()
}

#[test]
fn each_flag_set_alone_reaches_its_own_field_through_the_builder() {
    let known = known();
    for (incremental, end_marker_required, provisional_allowed) in [
        (true, false, false),
        (false, true, false),
        (false, false, true),
        (false, true, true),
        (true, false, true),
        (true, true, false),
    ] {
        let traits = PortTraits {
            essentiality: Essentiality::ControlOnly,
            delivery: Delivery::Coalescable,
            incremental,
            end_marker_required,
            provisional_allowed,
        };
        let built = through_the_builder(&known, traits);
        let commanded = through_the_commands(&known, traits);
        let flags = (incremental, end_marker_required, provisional_allowed);
        assert_eq!(built, commanded, "{flags:?}");
        assert_eq!(
            (
                built.incremental(),
                built.end_marker_required(),
                built.provisional_allowed()
            ),
            flags,
            "the builder's port carries the flags it was given"
        );
        assert_eq!(built.direction(), PortDirection::Input);
        assert_eq!(built.modality(), Modality::Audio);
        assert_eq!(built.essentiality(), Essentiality::ControlOnly);
        assert_eq!(built.delivery(), Delivery::Coalescable);
    }
}
