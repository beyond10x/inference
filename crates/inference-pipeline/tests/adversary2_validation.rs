//! Adversary pass 1 against `inference.graph.ValidatePipeline` (checks 1, 2, 3, 5, 6 and 9 of
//! §10.2): what the unit's own cases leave unsaid.
//!
//! Check 3 is one-directional: provisional data may not flow into a committed-only port, and
//! incremental frames may not flow into a whole-frame port; the opposite flows are compatible.
//! Checks 3 and 9 read an edge only when check 2 passes it. A pipeline's checks read its own edges
//! and bindings, never another pipeline's. The findings come in check order: 1, then 2, 3 and 9,
//! then 5, then 6.

use inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality, Modality, ModelId,
};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    AddNode, AddPort, BindNode, Catalog, ConnectPorts, Declaration, DeclarePipeline, Delivery,
    Essentiality, GraphDefect, GraphFinding, IncompatibleGraph, InteractionMode, NodeId, NodeRole,
    OverloadPolicy, PipelineId, PipelineState, PipelineValidated, PortDirection, PortId,
    ValidatePipeline, ValidatePipelineError,
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

#[derive(Clone, Copy)]
struct Traits {
    incremental: bool,
    provisional_allowed: bool,
}

const WHOLE_COMMITTED: Traits = Traits {
    incremental: false,
    provisional_allowed: false,
};

struct Graph {
    known: Known,
    declaration: Declaration,
}

impl Graph {
    fn new() -> Self {
        Self {
            known: known(),
            declaration: Declaration::counted(),
        }
    }

    fn pipeline(&mut self) -> PipelineId {
        self.declaration
            .declare_pipeline(
                DeclarePipeline {
                    name: "voice".to_owned(),
                    schema_revision: "1".to_owned(),
                    mode: InteractionMode::TurnBased,
                    profile_id: self.known.profile.profile_id(),
                    required_capabilities: Vec::new(),
                },
                &self.known,
            )
            .expect("the profile is declared")
            .pipeline_id
    }

    fn node(&mut self, pipeline_id: PipelineId, role: NodeRole) -> NodeId {
        self.declaration
            .add_node(AddNode {
                pipeline_id,
                name: format!("{role:?}"),
                role,
                requires: Vec::new(),
            })
            .expect("the pipeline is declared")
            .node_id
    }

    fn port(
        &mut self,
        node_id: NodeId,
        direction: PortDirection,
        modality: Modality,
        traits: Traits,
    ) -> PortId {
        let pipeline_id = self
            .declaration
            .node(node_id)
            .expect("the node exists")
            .pipeline_id();
        self.declaration
            .add_port(AddPort {
                pipeline_id,
                node_id,
                name: format!("{direction:?}"),
                direction,
                modality,
                essentiality: Essentiality::Essential,
                delivery: Delivery::Reliable,
                incremental: traits.incremental,
                end_marker_required: false,
                provisional_allowed: traits.provisional_allowed,
            })
            .expect("the node exists")
            .port_id
    }

    fn output(&mut self, node_id: NodeId, traits: Traits) -> PortId {
        self.port(node_id, PortDirection::Output, Modality::Text, traits)
    }

    fn input(&mut self, node_id: NodeId, traits: Traits) -> PortId {
        self.port(node_id, PortDirection::Input, Modality::Text, traits)
    }

    fn edge(&mut self, pipeline_id: PipelineId, from_port: PortId, to_port: PortId) {
        self.declaration
            .connect_ports(ConnectPorts {
                pipeline_id,
                from_port,
                to_port,
                capacity: 4,
                overload: OverloadPolicy::Pause,
            })
            .expect("a bounded edge from an output port");
    }

    fn validate(
        &mut self,
        pipeline_id: PipelineId,
    ) -> Result<PipelineValidated, ValidatePipelineError> {
        self.declaration
            .validate_pipeline(ValidatePipeline { pipeline_id })
    }

    fn state(&self, pipeline_id: PipelineId) -> PipelineState {
        self.declaration
            .pipeline(pipeline_id)
            .expect("the pipeline is declared")
            .state()
    }
}

fn finding(defect: GraphDefect, node_id: Option<NodeId>, port_id: Option<PortId>) -> GraphFinding {
    GraphFinding {
        defect,
        node_id,
        port_id,
    }
}

#[test]
fn committed_data_into_a_port_admitting_provisional_data_is_compatible() {
    let mut graph = Graph::new();
    let pipeline_id = graph.pipeline();
    let source = graph.node(pipeline_id, NodeRole::Source);
    let committed = graph.output(source, WHOLE_COMMITTED);
    let sink = graph.node(pipeline_id, NodeRole::Sink);
    let speculative = graph.input(
        sink,
        Traits {
            provisional_allowed: true,
            ..WHOLE_COMMITTED
        },
    );
    graph.edge(pipeline_id, committed, speculative);
    assert_eq!(
        graph.declaration.findings(pipeline_id),
        Vec::<GraphFinding>::new()
    );
    assert_eq!(
        graph.validate(pipeline_id),
        Ok(PipelineValidated { pipeline_id })
    );
}

#[test]
fn whole_frames_into_a_port_taking_incremental_frames_are_compatible() {
    let mut graph = Graph::new();
    let pipeline_id = graph.pipeline();
    let source = graph.node(pipeline_id, NodeRole::Source);
    let whole = graph.output(source, WHOLE_COMMITTED);
    let sink = graph.node(pipeline_id, NodeRole::Sink);
    let deltas = graph.input(
        sink,
        Traits {
            incremental: true,
            ..WHOLE_COMMITTED
        },
    );
    graph.edge(pipeline_id, whole, deltas);
    assert_eq!(
        graph.declaration.findings(pipeline_id),
        Vec::<GraphFinding>::new()
    );
    assert_eq!(
        graph.validate(pipeline_id),
        Ok(PipelineValidated { pipeline_id })
    );
}

#[test]
fn an_edge_check_2_refuses_is_not_read_by_checks_3_and_9() {
    let mut graph = Graph::new();
    let pipeline_id = graph.pipeline();
    let llm = graph.node(pipeline_id, NodeRole::Model);
    let hypotheses = graph.port(
        llm,
        PortDirection::Output,
        Modality::Text,
        Traits {
            incremental: true,
            provisional_allowed: true,
        },
    );
    let tool = graph.node(pipeline_id, NodeRole::Tool);
    let audio = graph.port(tool, PortDirection::Input, Modality::Audio, WHOLE_COMMITTED);
    graph.edge(pipeline_id, hypotheses, audio);
    let findings = vec![finding(
        GraphDefect::IncompatibleEdge,
        Some(llm),
        Some(hypotheses),
    )];
    assert_eq!(
        graph.validate(pipeline_id),
        Err(ValidatePipelineError::Rejected(IncompatibleGraph {
            findings
        }))
    );
}

#[test]
fn another_pipelines_edges_and_bindings_are_not_read() {
    let mut graph = Graph::new();
    let this = graph.pipeline();
    let other = graph.pipeline();
    let source = graph.node(other, NodeRole::Source);
    let out = graph.output(source, WHOLE_COMMITTED);
    let sink = graph.node(other, NodeRole::Sink);
    let into = graph.input(sink, WHOLE_COMMITTED);
    graph.edge(other, out, into);
    let adapter_id = graph.known.adapter.adapter_id();
    graph
        .declaration
        .bind_node(
            BindNode {
                pipeline_id: other,
                node_id: sink,
                adapter_id,
            },
            &graph.known,
        )
        .expect("a node of its own pipeline is bound");

    assert_eq!(graph.declaration.findings(this), Vec::<GraphFinding>::new());
    assert_eq!(
        graph.validate(this),
        Ok(PipelineValidated { pipeline_id: this })
    );
    assert_eq!(
        graph.validate(other),
        Ok(PipelineValidated { pipeline_id: other })
    );
}

#[test]
fn findings_come_in_check_order_whatever_the_declaration_order() {
    let mut graph = Graph::new();
    let pipeline_id = graph.pipeline();

    let lonely = graph.node(pipeline_id, NodeRole::Source);
    let unconnected = graph.output(lonely, WHOLE_COMMITTED);

    let first = graph.node(pipeline_id, NodeRole::Transform);
    let second = graph.node(pipeline_id, NodeRole::Transform);
    let first_in = graph.input(first, WHOLE_COMMITTED);
    let first_out = graph.output(first, WHOLE_COMMITTED);
    let second_in = graph.input(second, WHOLE_COMMITTED);
    let second_out = graph.output(second, WHOLE_COMMITTED);
    graph.edge(pipeline_id, first_out, second_in);
    graph.edge(pipeline_id, second_out, first_in);

    let text = graph.node(pipeline_id, NodeRole::Transform);
    let text_out = graph.output(text, WHOLE_COMMITTED);
    let audio = graph.node(pipeline_id, NodeRole::Transform);
    let audio_in = graph.port(
        audio,
        PortDirection::Input,
        Modality::Audio,
        WHOLE_COMMITTED,
    );
    graph.edge(pipeline_id, text_out, audio_in);

    let dangling = graph.node(pipeline_id, NodeRole::Transform);
    let dangling_out = graph.output(dangling, WHOLE_COMMITTED);
    let nowhere = PortId::from(Uuid::from_u128(0xbeef));
    graph.edge(pipeline_id, dangling_out, nowhere);

    let findings = vec![
        finding(GraphDefect::MissingNodeOrPort, None, Some(nowhere)),
        finding(GraphDefect::IncompatibleEdge, Some(text), Some(text_out)),
        finding(GraphDefect::UnboundedCycle, Some(first), None),
        finding(
            GraphDefect::UnreachableMandatoryOutput,
            Some(lonely),
            Some(unconnected),
        ),
    ];
    assert_eq!(
        graph.validate(pipeline_id),
        Err(ValidatePipelineError::Rejected(IncompatibleGraph {
            findings
        }))
    );
    assert_eq!(graph.state(pipeline_id), PipelineState::Declared);
}
