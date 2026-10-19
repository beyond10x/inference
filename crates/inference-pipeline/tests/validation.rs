//! `inference.graph.ValidatePipeline`: checks 1, 2, 3, 5, 6 and 9 of §10.2 over a declared
//! pipeline (§23.3 tests 1 and 2). Each check refuses its minimal failing graph with the finding
//! it produces, the pipeline staying Declared; a chain from a source through a transform to a
//! sink validates, and the pipeline is then Validated.

use std::num::NonZeroUsize;

use inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality, Modality, ModelId,
};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    AddNode, AddPort, BindNode, Catalog, ConnectPorts, Declaration, DeclarePipeline, Delivery,
    Essentiality, GraphDefect, GraphFinding, IncompatibleGraph, InteractionMode, NodeId, NodeRole,
    OverloadPolicy, PipelineId, PipelineNotFound, PipelineSpec, PipelineState,
    PipelineStateConflict, PipelineValidated, PortDirection, PortId, PortTraits, ValidatePipeline,
    ValidatePipelineError, modality,
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

/// A port's traits through the commands: essential, reliable, whole frames, committed only.
#[derive(Clone, Copy)]
struct Traits {
    essentiality: Essentiality,
    incremental: bool,
    provisional_allowed: bool,
}

const COMMITTED: Traits = Traits {
    essentiality: Essentiality::Essential,
    incremental: false,
    provisional_allowed: false,
};

/// One pipeline declared through the commands, which refuse none of what validation checks.
struct Graph {
    known: Known,
    declaration: Declaration,
    pipeline_id: PipelineId,
}

impl Graph {
    fn new() -> Self {
        let known = known();
        let mut declaration = Declaration::counted();
        let pipeline_id = Self::declare(&mut declaration, &known);
        Self {
            known,
            declaration,
            pipeline_id,
        }
    }

    fn declare(declaration: &mut Declaration, known: &Known) -> PipelineId {
        declaration
            .declare_pipeline(
                DeclarePipeline {
                    name: "voice".to_owned(),
                    schema_revision: "1".to_owned(),
                    mode: InteractionMode::TurnBased,
                    profile_id: known.profile.profile_id(),
                    required_capabilities: Vec::new(),
                },
                known,
            )
            .expect("the profile is declared")
            .pipeline_id
    }

    /// Another pipeline in the same declaration.
    fn other_pipeline(&mut self) -> PipelineId {
        Self::declare(&mut self.declaration, &self.known)
    }

    fn node_in(&mut self, pipeline_id: PipelineId, role: NodeRole) -> NodeId {
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

    fn node(&mut self, role: NodeRole) -> NodeId {
        self.node_in(self.pipeline_id, role)
    }

    fn port(
        &mut self,
        node_id: NodeId,
        direction: PortDirection,
        modality: Modality,
        traits: Traits,
    ) -> PortId {
        self.declaration
            .add_port(AddPort {
                node_id,
                name: format!("{direction:?}"),
                direction,
                modality,
                essentiality: traits.essentiality,
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

    fn edge(&mut self, from_port: PortId, to_port: PortId) {
        self.declaration
            .connect_ports(ConnectPorts {
                pipeline_id: self.pipeline_id,
                from_port,
                to_port,
                capacity: 4,
                overload: OverloadPolicy::Pause,
            })
            .expect("a bounded edge from an output port");
    }

    fn validate(&mut self) -> Result<PipelineValidated, ValidatePipelineError> {
        self.declaration.validate_pipeline(ValidatePipeline {
            pipeline_id: self.pipeline_id,
        })
    }

    fn state(&self) -> PipelineState {
        self.declaration
            .pipeline(self.pipeline_id)
            .expect("the pipeline is declared")
            .state()
    }

    /// Validation refuses with exactly `findings`, and the pipeline stays Declared.
    fn refuses_with(&mut self, findings: &[GraphFinding]) {
        assert_eq!(
            self.validate(),
            Err(ValidatePipelineError::Rejected(IncompatibleGraph {
                findings: findings.to_vec(),
            }))
        );
        assert_eq!(self.declaration.findings(self.pipeline_id), findings);
        assert_eq!(self.state(), PipelineState::Declared);
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
fn a_chain_from_a_source_through_a_transform_to_a_sink_validates_through_the_builder() {
    let known = known();
    let traits = PortTraits {
        incremental: true,
        ..PortTraits::default()
    };
    let mut declaration = Declaration::counted();
    let (pipeline_id, ()) = declaration.build(
        &known.profile,
        PipelineSpec {
            name: "chain".to_owned(),
            schema_revision: "1".to_owned(),
            mode: InteractionMode::Batch,
            required_capabilities: Vec::new(),
        },
        |pipeline| {
            let source = pipeline.node("source", NodeRole::Source, Vec::new());
            let transform = pipeline.node("transform", NodeRole::Transform, Vec::new());
            let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
            let read = pipeline.output::<modality::Text>(&source, "out", traits);
            let into = pipeline.input::<modality::Text>(&transform, "in", traits);
            let out = pipeline.output::<modality::Text>(&transform, "out", traits);
            let write = pipeline.input::<modality::Text>(&sink, "in", traits);
            pipeline.connect(&read, &into, NonZeroUsize::MIN, OverloadPolicy::Pause);
            pipeline.connect(&out, &write, NonZeroUsize::MIN, OverloadPolicy::Pause);
            pipeline.bind(&transform, &known.adapter);
        },
    );
    assert!(declaration.findings(pipeline_id).is_empty());
    assert_eq!(
        declaration.validate_pipeline(ValidatePipeline { pipeline_id }),
        Ok(PipelineValidated { pipeline_id })
    );
    let pipeline = declaration.pipeline(pipeline_id).expect("declared");
    assert_eq!(pipeline.state(), PipelineState::Validated);
    assert_eq!(
        declaration.validate_pipeline(ValidatePipeline { pipeline_id }),
        Err(ValidatePipelineError::WrongState(PipelineStateConflict {
            current: PipelineState::Validated
        })),
        "a Validated pipeline is not validated again"
    );
}

#[test]
fn every_check_is_vacuous_over_an_empty_pipeline() {
    let mut graph = Graph::new();
    let pipeline_id = graph.pipeline_id;
    assert_eq!(graph.validate(), Ok(PipelineValidated { pipeline_id }));
    assert_eq!(graph.state(), PipelineState::Validated);
}

#[test]
fn an_unknown_pipeline_is_refused_and_nothing_moves() {
    let mut declaration = Declaration::counted();
    let pipeline_id = PipelineId::from(Uuid::from_u128(0xdead));
    let before = declaration.clone();
    assert_eq!(
        declaration.validate_pipeline(ValidatePipeline { pipeline_id }),
        Err(ValidatePipelineError::UnknownPipeline(PipelineNotFound {
            pipeline_id
        }))
    );
    assert!(declaration.findings(pipeline_id).is_empty());
    assert_eq!(declaration, before);
}

#[test]
fn check_1_an_edge_to_a_port_no_node_holds_names_that_port() {
    let mut graph = Graph::new();
    let source = graph.node(NodeRole::Source);
    let out = graph.output(source, COMMITTED);
    let nowhere = PortId::from(Uuid::from_u128(0xbeef));
    graph.edge(out, nowhere);
    graph.refuses_with(&[finding(GraphDefect::MissingNodeOrPort, None, Some(nowhere))]);
}

#[test]
fn check_1_a_port_or_a_bound_node_of_another_pipeline_is_missing_from_this_one() {
    let mut graph = Graph::new();
    let other = graph.other_pipeline();
    let source = graph.node(NodeRole::Source);
    let out = graph.output(source, COMMITTED);
    let elsewhere = graph.node_in(other, NodeRole::Sink);
    let foreign = graph.input(elsewhere, COMMITTED);
    graph.edge(out, foreign);
    let adapter_id = graph.known.adapter.adapter_id();
    graph
        .declaration
        .bind_node(
            BindNode {
                pipeline_id: graph.pipeline_id,
                node_id: elsewhere,
                adapter_id,
            },
            &graph.known,
        )
        .expect("a node of another pipeline is bound");
    graph.refuses_with(&[
        finding(GraphDefect::MissingNodeOrPort, None, Some(foreign)),
        finding(GraphDefect::MissingNodeOrPort, Some(elsewhere), None),
    ]);
}

#[test]
fn check_2_an_edge_into_an_output_port_is_incompatible_and_names_its_source_port() {
    let mut graph = Graph::new();
    let source = graph.node(NodeRole::Source);
    let out = graph.output(source, COMMITTED);
    let transform = graph.node(NodeRole::Transform);
    let also_out = graph.output(transform, COMMITTED);
    graph.edge(out, also_out);
    graph.refuses_with(&[finding(
        GraphDefect::IncompatibleEdge,
        Some(source),
        Some(out),
    )]);
}

#[test]
fn check_2_an_edge_between_two_modalities_is_incompatible() {
    let mut graph = Graph::new();
    let source = graph.node(NodeRole::Source);
    let text = graph.output(source, COMMITTED);
    let sink = graph.node(NodeRole::Sink);
    let audio = graph.port(sink, PortDirection::Input, Modality::Audio, COMMITTED);
    graph.edge(text, audio);
    graph.refuses_with(&[finding(
        GraphDefect::IncompatibleEdge,
        Some(source),
        Some(text),
    )]);
}

#[test]
fn check_3_provisional_data_into_a_committed_only_port_is_incompatible() {
    let mut graph = Graph::new();
    let source = graph.node(NodeRole::Source);
    let hypotheses = graph.output(
        source,
        Traits {
            provisional_allowed: true,
            ..COMMITTED
        },
    );
    let sink = graph.node(NodeRole::Sink);
    let committed = graph.input(sink, COMMITTED);
    graph.edge(hypotheses, committed);
    graph.refuses_with(&[finding(
        GraphDefect::IncompatibleEdge,
        Some(source),
        Some(hypotheses),
    )]);
}

#[test]
fn check_3_incremental_frames_into_a_whole_frame_port_are_incompatible() {
    let mut graph = Graph::new();
    let source = graph.node(NodeRole::Source);
    let deltas = graph.output(
        source,
        Traits {
            incremental: true,
            ..COMMITTED
        },
    );
    let sink = graph.node(NodeRole::Sink);
    let whole = graph.input(sink, COMMITTED);
    graph.edge(deltas, whole);
    graph.refuses_with(&[finding(
        GraphDefect::IncompatibleEdge,
        Some(source),
        Some(deltas),
    )]);
}

#[test]
fn check_5_a_cycle_is_unbounded_and_names_its_first_node() {
    let mut graph = Graph::new();
    let first = graph.node(NodeRole::Transform);
    let second = graph.node(NodeRole::Transform);
    let first_in = graph.input(first, COMMITTED);
    let first_out = graph.output(first, COMMITTED);
    let second_in = graph.input(second, COMMITTED);
    let second_out = graph.output(second, COMMITTED);
    graph.edge(first_out, second_in);
    graph.edge(second_out, first_in);
    graph.refuses_with(&[finding(GraphDefect::UnboundedCycle, Some(first), None)]);
}

#[test]
fn check_5_a_node_feeding_itself_is_a_cycle() {
    let mut graph = Graph::new();
    let node = graph.node(NodeRole::Policy);
    let into = graph.input(node, COMMITTED);
    let out = graph.output(node, COMMITTED);
    graph.edge(out, into);
    graph.refuses_with(&[finding(GraphDefect::UnboundedCycle, Some(node), None)]);
}

#[test]
fn check_6_an_unconnected_essential_port_is_unreachable_and_optional_ones_may_be() {
    let mut graph = Graph::new();
    let source = graph.node(NodeRole::Source);
    let essential = graph.output(source, COMMITTED);
    graph.output(
        source,
        Traits {
            essentiality: Essentiality::Optional,
            ..COMMITTED
        },
    );
    graph.port(
        source,
        PortDirection::Input,
        Modality::Control,
        Traits {
            essentiality: Essentiality::ControlOnly,
            ..COMMITTED
        },
    );
    graph.refuses_with(&[finding(
        GraphDefect::UnreachableMandatoryOutput,
        Some(source),
        Some(essential),
    )]);
}

#[test]
fn check_6_a_sink_no_source_reaches_is_unreachable() {
    let mut graph = Graph::new();
    let transform = graph.node(NodeRole::Transform);
    let out = graph.output(transform, COMMITTED);
    let sink = graph.node(NodeRole::Sink);
    let into = graph.input(sink, COMMITTED);
    graph.edge(out, into);
    graph.refuses_with(&[finding(
        GraphDefect::UnreachableMandatoryOutput,
        Some(sink),
        None,
    )]);
}

#[test]
fn check_9_provisional_text_into_a_tool_is_a_disallowed_data_flow() {
    let known = known();
    let provisional = PortTraits {
        provisional_allowed: true,
        ..PortTraits::default()
    };
    let mut declaration = Declaration::counted();
    let (pipeline_id, (llm, proposal)) = declaration.build(
        &known.profile,
        PipelineSpec {
            name: "agent".to_owned(),
            schema_revision: "1".to_owned(),
            mode: InteractionMode::TurnBased,
            required_capabilities: Vec::new(),
        },
        |pipeline| {
            let llm = pipeline.node("llm", NodeRole::Model, Vec::new());
            let tool = pipeline.node("tool", NodeRole::Tool, Vec::new());
            let proposal = pipeline.output::<modality::Structured>(&llm, "call", provisional);
            let call = pipeline.input::<modality::Structured>(&tool, "call", PortTraits::default());
            pipeline.connect(&proposal, &call, NonZeroUsize::MIN, OverloadPolicy::Pause);
            (llm.id(), proposal.id())
        },
    );
    let rejected = Err(ValidatePipelineError::Rejected(IncompatibleGraph {
        findings: vec![finding(
            GraphDefect::DisallowedDataFlow,
            Some(llm),
            Some(proposal),
        )],
    }));
    assert_eq!(
        declaration.validate_pipeline(ValidatePipeline { pipeline_id }),
        rejected
    );
    assert_eq!(
        declaration.pipeline(pipeline_id).expect("declared").state(),
        PipelineState::Declared
    );
}

#[test]
fn check_9_a_tool_input_explicitly_admitting_provisional_data_validates() {
    let mut graph = Graph::new();
    let llm = graph.node(NodeRole::Model);
    let speculative = Traits {
        provisional_allowed: true,
        ..COMMITTED
    };
    let proposal = graph.output(llm, speculative);
    let tool = graph.node(NodeRole::Tool);
    let call = graph.input(tool, speculative);
    graph.edge(proposal, call);
    let pipeline_id = graph.pipeline_id;
    assert_eq!(graph.validate(), Ok(PipelineValidated { pipeline_id }));
}

#[test]
fn a_refused_pipeline_stays_declared_and_validates_once_repaired() {
    let mut graph = Graph::new();
    let source = graph.node(NodeRole::Source);
    let out = graph.output(source, COMMITTED);
    graph.refuses_with(&[finding(
        GraphDefect::UnreachableMandatoryOutput,
        Some(source),
        Some(out),
    )]);
    let sink = graph.node(NodeRole::Sink);
    let into = graph.input(sink, COMMITTED);
    graph.edge(out, into);
    let pipeline_id = graph.pipeline_id;
    assert_eq!(graph.validate(), Ok(PipelineValidated { pipeline_id }));
}

#[test]
fn the_findings_serialize_with_the_ess_names() {
    assert_eq!(
        serde_json::to_value(GraphDefect::ALL).expect("serializes"),
        json!([
            "missing_node_or_port",
            "incompatible_edge",
            "unbounded_cycle",
            "unreachable_mandatory_output",
            "disallowed_data_flow"
        ])
    );
    let port_id = PortId::from(Uuid::from_u128(1));
    assert_eq!(
        serde_json::to_value(IncompatibleGraph {
            findings: vec![finding(GraphDefect::MissingNodeOrPort, None, Some(port_id))],
        })
        .expect("serializes"),
        json!({"findings": [{
            "defect": "missing_node_or_port",
            "node_id": null,
            "port_id": "00000000-0000-0000-0000-000000000001"
        }]})
    );
    assert_eq!(
        serde_json::to_value(PipelineStateConflict {
            current: PipelineState::Validated
        })
        .expect("serializes"),
        json!({"current": "Validated"})
    );
}

#[test]
fn a_port_left_to_its_defaults_admits_committed_whole_frames_only() {
    assert_eq!(
        PortTraits::default(),
        PortTraits {
            essentiality: Essentiality::Essential,
            delivery: Delivery::Reliable,
            incremental: false,
            end_marker_required: false,
            provisional_allowed: false,
        }
    );
}
