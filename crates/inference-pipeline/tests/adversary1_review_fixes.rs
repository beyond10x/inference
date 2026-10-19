//! Adversarial cases on the review fixes: the typed builder writes only what the commands can
//! write (decision 2), and check 6 names every essential output that resolves to no input
//! (decision 5).

use std::num::NonZeroUsize;

use inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality, Modality, ModelId,
};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    AddNode, AddPort, Catalog, ConnectPorts, Declaration, DeclarePipeline, Delivery, Essentiality,
    GraphDefect, InteractionMode, NodeId, NodeRole, OverloadPolicy, PipelineId, PipelineSpec,
    PortDirection, PortId, PortTraits, modality,
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

/// Decision 2: the builder writes no pipeline the commands could not write. `ConnectPorts` takes
/// its capacity as the ESS `Integer` (`i64`), so no command writes an edge holding more than
/// `i64::MAX`; the builder's `NonZeroUsize` capacity reaches past it on a 64-bit target, and the
/// testkit's `Edges` row then reads `i64::MAX`, a value nothing stored.
#[test]
fn the_builder_writes_no_edge_capacity_connect_ports_cannot_carry() {
    let known = known();
    let mut declaration = Declaration::counted();
    let traits = PortTraits {
        essentiality: Essentiality::Essential,
        delivery: Delivery::Reliable,
        incremental: false,
        end_marker_required: false,
        provisional_allowed: false,
    };
    let built = declaration.build(
        &known,
        known.profile.profile_id(),
        PipelineSpec {
            name: "wide".to_owned(),
            schema_revision: "1".to_owned(),
            mode: InteractionMode::Batch,
            required_capabilities: Vec::new(),
        },
        |pipeline| {
            let source = pipeline.node("source", NodeRole::Source, Vec::new());
            let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
            let out = pipeline.output::<modality::Text>(&source, "out", traits);
            let into = pipeline.input::<modality::Text>(&sink, "in", traits);
            pipeline.connect(&out, &into, NonZeroUsize::MAX, OverloadPolicy::Pause);
            pipeline
                .bind(&source, known.adapter.adapter_id())
                .expect("the adapter is declared");
        },
    );
    assert!(built.is_ok(), "the profile is declared");
    let capacities: Vec<usize> = declaration
        .edges()
        .map(|edge| edge.capacity().get())
        .collect();
    assert_eq!(capacities.len(), 1);
    assert!(
        capacities
            .iter()
            .all(|capacity| i64::try_from(*capacity).is_ok()),
        "the builder wrote an edge of capacity {capacities:?}, which no `ConnectPorts` input \
         (capacity: i64) can write"
    );
}

fn port(
    declaration: &mut Declaration,
    pipeline_id: PipelineId,
    node_id: NodeId,
    direction: PortDirection,
) -> PortId {
    declaration
        .add_port(AddPort {
            pipeline_id,
            node_id,
            name: format!("{direction:?}"),
            direction,
            modality: Modality::Text,
            essentiality: Essentiality::Essential,
            delivery: Delivery::Reliable,
            incremental: false,
            end_marker_required: false,
            provisional_allowed: false,
        })
        .expect("the pipeline is declared and the node exists")
        .port_id
}

/// Decision 5 and design line 530 ("no unresolved mandatory outputs"): `ConnectPorts` refuses only
/// a source that is not an output, so an edge may end on another output port. That output
/// resolves to no input, and check 6 names it; it is not resolved by being an edge's sink.
#[test]
fn an_essential_output_that_is_only_an_edge_sink_is_unresolved() {
    let known = known();
    let mut declaration = Declaration::counted();
    let pipeline_id = declaration
        .declare_pipeline(
            DeclarePipeline {
                name: "outputs".to_owned(),
                schema_revision: "1".to_owned(),
                mode: InteractionMode::Batch,
                profile_id: known.profile.profile_id(),
                required_capabilities: Vec::new(),
            },
            &known,
        )
        .expect("the profile is declared")
        .pipeline_id;
    let mut node = |role: NodeRole| {
        declaration
            .add_node(AddNode {
                pipeline_id,
                name: format!("{role:?}"),
                role,
                requires: Vec::new(),
            })
            .expect("the pipeline is declared")
            .node_id
    };
    let first = node(NodeRole::Transform);
    let second = node(NodeRole::Transform);
    let from = port(&mut declaration, pipeline_id, first, PortDirection::Output);
    let to = port(&mut declaration, pipeline_id, second, PortDirection::Output);
    declaration
        .connect_ports(ConnectPorts {
            pipeline_id,
            from_port: from,
            to_port: to,
            capacity: 1,
            overload: OverloadPolicy::Pause,
        })
        .expect("ConnectPorts does not refuse an output sink");

    let unresolved: Vec<Option<PortId>> = declaration
        .findings(pipeline_id)
        .into_iter()
        .filter(|finding| finding.defect == GraphDefect::UnreachableMandatoryOutput)
        .map(|finding| finding.port_id)
        .collect();
    assert_eq!(
        unresolved,
        [Some(to)],
        "the essential output `to` feeds no input port"
    );
}
