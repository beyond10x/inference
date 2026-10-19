//! Adversary pass 1 on check 5 with feedback loops (`story:tool-feedback-loop`): which cycles a
//! `FeedbackLoop` bounds, driven through the declaration commands, where the loop's own case only
//! names an edge on the cycle or one off it.
//!
//! * A loop declared under another pipeline, naming this pipeline's edge, bounds nothing here.
//! * A cycle that also closes through a parallel unbounded edge stays refused.
//! * Of two cycles only the bounded one is admitted; two cycles sharing one bounded edge, and a
//!   node feeding itself along a bounded edge, are admitted.
//! * `DeclareFeedbackLoop` refuses in the order graph.yaml lists its outcomes and admits the
//!   largest ESS `Integer`.

use inference_core::model::{Adapter, AdapterId};
use inference_core::model::{Locality, Modality};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    AddNode, AddPort, Catalog, ConnectPorts, Declaration, DeclareFeedbackLoop,
    DeclareFeedbackLoopError, DeclarePipeline, Delivery, EdgeId, EdgeNotFound, Essentiality,
    GraphDefect, GraphFinding, IncompatibleGraph, InteractionMode, NodeId, NodeRole,
    OverloadPolicy, PipelineId, PipelineNotReady, PipelineState, PortDirection, PortId,
    UnboundedFeedback, ValidatePipeline, ValidatePipelineError,
};
use uuid::Uuid;

struct Known {
    profile: DeploymentProfile,
}

impl Catalog for Known {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.profile.profile_id() == profile_id).then_some(&self.profile)
    }

    fn adapter(&self, _: AdapterId) -> Option<&Adapter> {
        None
    }
}

struct Graph {
    known: Known,
    declaration: Declaration,
}

impl Graph {
    fn new() -> Self {
        let (profile, _) = DeploymentProfile::declare(DeclareProfile {
            kind: ProfileKind::OfflineStrict,
            egress: Egress::Denied,
            admitted: vec![Locality::SameProcess],
        })
        .expect("a row of the table");
        Self {
            known: Known { profile },
            declaration: Declaration::counted(),
        }
    }

    fn pipeline(&mut self) -> PipelineId {
        self.declaration
            .declare_pipeline(
                DeclarePipeline {
                    name: "loop".to_owned(),
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

    fn node(&mut self, pipeline_id: PipelineId) -> NodeId {
        self.declaration
            .add_node(AddNode {
                pipeline_id,
                name: "transform".to_owned(),
                role: NodeRole::Transform,
                requires: Vec::new(),
            })
            .expect("the pipeline is declared")
            .node_id
    }

    fn port(
        &mut self,
        pipeline_id: PipelineId,
        node_id: NodeId,
        direction: PortDirection,
    ) -> PortId {
        self.declaration
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
            .expect("the node exists")
            .port_id
    }

    /// An edge from a new output port of `from` to a new input port of `to`.
    fn edge(&mut self, pipeline_id: PipelineId, from: NodeId, to: NodeId) -> EdgeId {
        let from_port = self.port(pipeline_id, from, PortDirection::Output);
        let to_port = self.port(pipeline_id, to, PortDirection::Input);
        self.declaration
            .connect_ports(ConnectPorts {
                pipeline_id,
                from_port,
                to_port,
                capacity: 4,
                overload: OverloadPolicy::Fail,
            })
            .expect("a bounded edge from an output port")
            .edge_id
    }

    fn bound(&mut self, pipeline_id: PipelineId, edge_id: EdgeId) {
        self.declaration
            .declare_feedback_loop(DeclareFeedbackLoop {
                pipeline_id,
                edge_id,
                max_iterations: 3,
            })
            .expect("a known pipeline and edge, a positive maximum");
    }

    fn validate(&mut self, pipeline_id: PipelineId) -> Result<(), ValidatePipelineError> {
        self.declaration
            .validate_pipeline(ValidatePipeline { pipeline_id })
            .map(|_| ())
    }

    fn state(&self, pipeline_id: PipelineId) -> PipelineState {
        self.declaration
            .pipeline(pipeline_id)
            .expect("the pipeline is declared")
            .state()
    }
}

fn unbounded(node_ids: &[NodeId]) -> Result<(), ValidatePipelineError> {
    Err(ValidatePipelineError::Rejected(IncompatibleGraph {
        findings: node_ids
            .iter()
            .map(|node_id| GraphFinding {
                defect: GraphDefect::UnboundedCycle,
                node_id: Some(*node_id),
                port_id: None,
            })
            .collect(),
    }))
}

#[test]
fn a_feedback_loop_declared_under_another_pipeline_bounds_no_cycle_of_this_one() {
    let mut graph = Graph::new();
    let pipeline = graph.pipeline();
    let other = graph.pipeline();
    let (first, second) = (graph.node(pipeline), graph.node(pipeline));
    let forward = graph.edge(pipeline, first, second);
    graph.edge(pipeline, second, first);
    graph.bound(other, forward);
    assert_eq!(graph.validate(pipeline), unbounded(&[first]));
    assert_eq!(graph.state(pipeline), PipelineState::Declared);
}

#[test]
fn a_cycle_also_closing_through_a_parallel_unbounded_edge_stays_refused() {
    let mut graph = Graph::new();
    let pipeline = graph.pipeline();
    let (first, second) = (graph.node(pipeline), graph.node(pipeline));
    let bounded = graph.edge(pipeline, first, second);
    graph.edge(pipeline, first, second);
    graph.edge(pipeline, second, first);
    graph.bound(pipeline, bounded);
    assert_eq!(graph.validate(pipeline), unbounded(&[first]));
}

#[test]
fn of_two_cycles_only_the_bounded_one_is_admitted() {
    let mut graph = Graph::new();
    let pipeline = graph.pipeline();
    let nodes: Vec<NodeId> = (0..4).map(|_| graph.node(pipeline)).collect();
    let bounded = graph.edge(pipeline, nodes[0], nodes[1]);
    graph.edge(pipeline, nodes[1], nodes[0]);
    graph.edge(pipeline, nodes[2], nodes[3]);
    graph.edge(pipeline, nodes[3], nodes[2]);
    graph.bound(pipeline, bounded);
    assert_eq!(graph.validate(pipeline), unbounded(&[nodes[2]]));
}

#[test]
fn two_cycles_sharing_one_bounded_edge_and_a_bounded_self_loop_validate() {
    let mut graph = Graph::new();
    let pipeline = graph.pipeline();
    let nodes: Vec<NodeId> = (0..3).map(|_| graph.node(pipeline)).collect();
    let shared = graph.edge(pipeline, nodes[0], nodes[1]);
    graph.edge(pipeline, nodes[1], nodes[0]);
    graph.edge(pipeline, nodes[1], nodes[2]);
    graph.edge(pipeline, nodes[2], nodes[0]);
    graph.bound(pipeline, shared);
    let itself = graph.edge(pipeline, nodes[2], nodes[2]);
    graph.bound(pipeline, itself);
    assert_eq!(graph.validate(pipeline), Ok(()));
    assert_eq!(graph.state(pipeline), PipelineState::Validated);
}

#[test]
fn declare_feedback_loop_refuses_in_the_order_graph_yaml_lists_and_admits_the_largest_integer() {
    let mut graph = Graph::new();
    let pipeline = graph.pipeline();
    let (first, second) = (graph.node(pipeline), graph.node(pipeline));
    let forward = graph.edge(pipeline, first, second);
    let unknown = EdgeId::from(Uuid::from_u128(u128::MAX));
    let declare = |edge_id, max_iterations| DeclareFeedbackLoop {
        pipeline_id: pipeline,
        edge_id,
        max_iterations,
    };
    assert_eq!(
        graph.declaration.declare_feedback_loop(declare(unknown, 0)),
        Err(DeclareFeedbackLoopError::UnknownEdge(EdgeNotFound {
            edge_id: unknown
        }))
    );
    for refused in [0, -1, i64::MIN] {
        assert_eq!(
            graph
                .declaration
                .declare_feedback_loop(declare(forward, refused)),
            Err(DeclareFeedbackLoopError::Unbounded(UnboundedFeedback {
                submitted: refused
            }))
        );
    }
    assert_eq!(graph.declaration.feedback_loops().count(), 0);
    let declared = graph
        .declaration
        .declare_feedback_loop(declare(forward, i64::MAX))
        .expect("the largest Integer is a bound");
    assert_eq!(declared.max_iterations, i64::MAX);
    graph
        .validate(pipeline)
        .expect("an acyclic pipeline validates");
    assert_eq!(
        graph.declaration.declare_feedback_loop(declare(forward, 0)),
        Err(DeclareFeedbackLoopError::Unbounded(UnboundedFeedback {
            submitted: 0
        })),
        "unbounded is decided before not-declared"
    );
    assert_eq!(
        graph.declaration.declare_feedback_loop(declare(forward, 1)),
        Err(DeclareFeedbackLoopError::NotDeclared(PipelineNotReady {
            pipeline_id: pipeline,
            required: PipelineState::Declared,
        }))
    );
}
