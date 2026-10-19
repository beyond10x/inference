//! The structural checks of §10.2 that `inference.graph.ValidatePipeline` makes over one
//! pipeline's declaration: 1 (existence), 2 (direction and modality), 3 (commitment semantics),
//! 5 (cycles), 6 (reachability) and 9 (tool gating).
//!
//! The pipeline's graph is its nodes, the ports declared under it on those nodes, and the edges
//! and bindings that name the pipeline. A port declared under the pipeline on a node of another
//! is missing from it, as a bound node of another is. An edge joins two nodes of the graph only
//! when check 1 finds both of its ports there, and checks 2, 3 and 9 read only such an edge; check
//! 3 and check 9 read it only when check 2 passes it, since commitment semantics are compared
//! between an output port and an input port of one modality. Checks 5 and 6 follow every edge
//! that joins two nodes, compatible or not, from its source port's node to its sink port's node,
//! except that check 5 does not follow a feedback edge: an edge of the pipeline that a
//! [`FeedbackLoop`](crate::FeedbackLoop) of the pipeline names. A cycle is bounded when it runs
//! through such an edge, so the cycles check 5 finds are those left without them.
//!
//! The pipeline's exposures ([`Exposure`]) are edges whose far end is outside the pipeline: check
//! 1 finds an exposed port that is not a port of the pipeline missing, check 6 counts an exposed
//! input port as fed and an exposed output port as consumed, and the last check finds every
//! exposure whose name an exposure of the pipeline declared before it carries.
//!
//! Media encoding and timebase (the rest of check 3) arrive with `inference-audio`.

use std::collections::BTreeSet;

use crate::{
    Declaration, Edge, EdgeId, Essentiality, Exposure, GraphDefect, GraphFinding, Node, NodeId,
    NodeRole, PipelineId, Port, PortDirection,
};

/// The findings of checks 1, 2, 3, 9, 5 and 6 and of the exposure names over the pipeline
/// carrying `pipeline_id`; see the module documentation.
pub(crate) fn findings(declaration: &Declaration, pipeline_id: PipelineId) -> Vec<GraphFinding> {
    let graph = Graph::of(declaration, pipeline_id);
    let mut findings = Vec::new();
    graph.existence(&mut findings);
    graph.edges_between_ports(&mut findings);
    graph.cycles(&mut findings);
    graph.reachability(&mut findings);
    graph.exposure_names(&mut findings);
    findings
}

/// One pipeline's nodes, the ports declared under it on those nodes, the ports declared under it
/// on a node of another (`strays`), the edges, exposures and bindings naming it, and those of its
/// edges a feedback loop of it names (`feedback`).
struct Graph<'d> {
    declaration: &'d Declaration,
    pipeline_id: PipelineId,
    nodes: Vec<&'d Node>,
    ports: Vec<&'d Port>,
    strays: Vec<&'d Port>,
    edges: Vec<&'d Edge>,
    exposures: Vec<&'d Exposure>,
    feedback: BTreeSet<EdgeId>,
}

/// Which edges a walk of the graph follows.
#[derive(Clone, Copy)]
enum Follow {
    /// Every edge that joins two nodes.
    Every,
    /// Every edge that joins two nodes but a feedback edge.
    Unbounded,
}

/// An edge whose two ports are both ports of the pipeline's nodes.
struct Joined<'d> {
    source: &'d Port,
    sink: &'d Port,
}

impl<'d> Graph<'d> {
    fn of(declaration: &'d Declaration, pipeline_id: PipelineId) -> Self {
        let nodes: Vec<&Node> = declaration
            .nodes()
            .filter(|node| node.pipeline_id() == pipeline_id)
            .collect();
        let (ports, strays) = declaration
            .ports()
            .filter(|port| port.pipeline_id() == pipeline_id)
            .partition(|port| nodes.iter().any(|node| node.node_id() == port.node_id()));
        let edges: Vec<&Edge> = declaration
            .edges()
            .filter(|edge| edge.pipeline_id() == pipeline_id)
            .collect();
        let feedback = declaration
            .feedback_loops()
            .filter(|feedback| feedback.pipeline_id() == pipeline_id)
            .map(crate::FeedbackLoop::edge_id)
            .filter(|edge_id| edges.iter().any(|edge| edge.edge_id() == *edge_id))
            .collect();
        let exposures = declaration
            .exposures()
            .filter(|exposure| exposure.pipeline_id() == pipeline_id)
            .collect();
        Self {
            declaration,
            pipeline_id,
            nodes,
            ports,
            strays,
            edges,
            exposures,
            feedback,
        }
    }

    /// The ports of the pipeline its exposures name in `direction`.
    fn exposed(&self, direction: PortDirection) -> impl Iterator<Item = &'d Port> + '_ {
        self.exposures
            .iter()
            .filter_map(|exposure| self.port(exposure.port_id()))
            .filter(move |port| port.direction() == direction)
    }

    fn port(&self, port_id: crate::PortId) -> Option<&'d Port> {
        self.ports
            .iter()
            .copied()
            .find(|port| port.port_id() == port_id)
    }

    fn node(&self, node_id: NodeId) -> Option<&'d Node> {
        self.nodes
            .iter()
            .copied()
            .find(|node| node.node_id() == node_id)
    }

    fn joined(&self, edge: &Edge) -> Option<Joined<'d>> {
        Some(Joined {
            source: self.port(edge.from_port())?,
            sink: self.port(edge.to_port())?,
        })
    }

    /// Check 1: every port an edge or an exposure names is a port of the pipeline, the node of
    /// every port declared under it is a node of it, and every bound node is a node of it. A
    /// missing port is named by its identity, a port whose node is missing by both identities, a
    /// missing bound node by its own.
    fn existence(&self, findings: &mut Vec<GraphFinding>) {
        let named = self
            .edges
            .iter()
            .flat_map(|edge| [edge.from_port(), edge.to_port()])
            .chain(self.exposures.iter().map(|exposure| exposure.port_id()));
        for port_id in named {
            if self.port(port_id).is_none() {
                findings.push(GraphFinding {
                    defect: GraphDefect::MissingNodeOrPort,
                    node_id: None,
                    port_id: Some(port_id),
                });
            }
        }
        for port in &self.strays {
            findings.push(GraphFinding {
                defect: GraphDefect::MissingNodeOrPort,
                node_id: Some(port.node_id()),
                port_id: Some(port.port_id()),
            });
        }
        for binding in self
            .declaration
            .bindings()
            .filter(|binding| binding.pipeline_id() == self.pipeline_id)
        {
            if self.node(binding.node_id()).is_none() {
                findings.push(GraphFinding {
                    defect: GraphDefect::MissingNodeOrPort,
                    node_id: Some(binding.node_id()),
                    port_id: None,
                });
            }
        }
    }

    /// Checks 2, 3 and 9 over each edge joining two ports of the pipeline, in declaration order.
    ///
    /// Check 2: the source is an output port, the sink an input port, and both carry one modality.
    /// Check 3: a source that may carry provisional data feeds a sink that admits it, and a source
    /// of incremental frames feeds a sink that takes them. Check 9: provisional data into an input
    /// port of a tool is a disallowed data flow rather than an incompatible edge, because a
    /// side-effecting tool requires committed input unless its port admits provisional data
    /// explicitly. Each finding names the edge's source port and its node.
    fn edges_between_ports(&self, findings: &mut Vec<GraphFinding>) {
        for edge in &self.edges {
            let Some(Joined { source, sink }) = self.joined(edge) else {
                continue;
            };
            let at = |defect| GraphFinding {
                defect,
                node_id: Some(source.node_id()),
                port_id: Some(source.port_id()),
            };
            if source.direction() != PortDirection::Output
                || sink.direction() != PortDirection::Input
                || source.modality() != sink.modality()
            {
                findings.push(at(GraphDefect::IncompatibleEdge));
                continue;
            }
            let provisional = source.provisional_allowed() && !sink.provisional_allowed();
            let into_tool = self
                .node(sink.node_id())
                .is_some_and(|node| node.role() == NodeRole::Tool);
            let incremental = source.incremental() && !sink.incremental();
            if incremental || (provisional && !into_tool) {
                findings.push(at(GraphDefect::IncompatibleEdge));
            }
            if provisional && into_tool {
                findings.push(at(GraphDefect::DisallowedDataFlow));
            }
        }
    }

    /// The nodes an edge that `follow` admits leads to from `node_id` in one step.
    fn successors(&self, node_id: NodeId, follow: Follow) -> impl Iterator<Item = NodeId> + '_ {
        self.edges
            .iter()
            .filter(move |edge| match follow {
                Follow::Every => true,
                Follow::Unbounded => !self.feedback.contains(&edge.edge_id()),
            })
            .filter_map(|edge| self.joined(edge))
            .filter(move |joined| joined.source.node_id() == node_id)
            .map(|joined| joined.sink.node_id())
    }

    /// Every node reachable from `starts` by one or more edges that `follow` admits.
    fn reached(
        &self,
        starts: impl IntoIterator<Item = NodeId>,
        follow: Follow,
    ) -> BTreeSet<NodeId> {
        let mut reached = BTreeSet::new();
        let mut pending: Vec<NodeId> = starts
            .into_iter()
            .flat_map(|start| self.successors(start, follow))
            .collect();
        while let Some(node_id) = pending.pop() {
            if reached.insert(node_id) {
                pending.extend(self.successors(node_id, follow));
            }
        }
        reached
    }

    /// Check 5: every cycle that runs through no feedback edge is refused, one finding per
    /// strongly connected set of nodes that reach themselves without one, naming its first node
    /// in declaration order.
    fn cycles(&self, findings: &mut Vec<GraphFinding>) {
        let mut named = BTreeSet::new();
        for node in &self.nodes {
            let node_id = node.node_id();
            if named.contains(&node_id) {
                continue;
            }
            let reached = self.reached([node_id], Follow::Unbounded);
            if !reached.contains(&node_id) {
                continue;
            }
            for other in &reached {
                if self.reached([*other], Follow::Unbounded).contains(&node_id) {
                    named.insert(*other);
                }
            }
            findings.push(GraphFinding {
                defect: GraphDefect::UnboundedCycle,
                node_id: Some(node_id),
                port_id: None,
            });
        }
    }

    /// Check 6, reachable source and sink paths with no unresolved mandatory output (design 530):
    /// every essential output port is the source of at least one edge or exposed as an egress,
    /// which the application consumes, and every sink is reached from a source or from the node of
    /// an ingress, which the application feeds. An output that is only an edge's sink feeds
    /// nothing and stays unresolved. Optional and control-only outputs may stay unconnected, and
    /// so may any input port, which leaves no output unresolved.
    fn reachability(&self, findings: &mut Vec<GraphFinding>) {
        let egresses: BTreeSet<crate::PortId> = self
            .exposed(PortDirection::Output)
            .map(Port::port_id)
            .collect();
        for port in &self.ports {
            let mandatory = port.direction() == PortDirection::Output
                && port.essentiality() == Essentiality::Essential;
            let connected = self
                .edges
                .iter()
                .any(|edge| edge.from_port() == port.port_id())
                || egresses.contains(&port.port_id());
            if mandatory && !connected {
                findings.push(GraphFinding {
                    defect: GraphDefect::UnreachableMandatoryOutput,
                    node_id: Some(port.node_id()),
                    port_id: Some(port.port_id()),
                });
            }
        }
        let fed: BTreeSet<NodeId> = self
            .exposed(PortDirection::Input)
            .map(Port::node_id)
            .collect();
        let starts: BTreeSet<NodeId> = self
            .nodes
            .iter()
            .filter(|node| node.role() == NodeRole::Source)
            .map(|node| node.node_id())
            .chain(fed.iter().copied())
            .collect();
        let reached = self.reached(starts.iter().copied(), Follow::Every);
        for node in &self.nodes {
            let node_id = node.node_id();
            if node.role() == NodeRole::Sink
                && !reached.contains(&node_id)
                && !fed.contains(&node_id)
            {
                findings.push(GraphFinding {
                    defect: GraphDefect::UnreachableMandatoryOutput,
                    node_id: Some(node_id),
                    port_id: None,
                });
            }
        }
    }

    /// Every exposure whose name an exposure of the pipeline declared before it carries, named by
    /// its port and that port's node, or by its port alone when the port is missing (check 1).
    fn exposure_names(&self, findings: &mut Vec<GraphFinding>) {
        let mut names = BTreeSet::new();
        for exposure in &self.exposures {
            if !names.insert(exposure.name()) {
                findings.push(GraphFinding {
                    defect: GraphDefect::DuplicateExposureName,
                    node_id: self.port(exposure.port_id()).map(Port::node_id),
                    port_id: Some(exposure.port_id()),
                });
            }
        }
    }
}
