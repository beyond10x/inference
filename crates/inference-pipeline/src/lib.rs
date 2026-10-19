//! Pipeline graphs of the inference framework.
//!
//! `inference-pipeline` holds the declaration half of `ess/domains/graph.yaml`: a [`Pipeline`] of
//! named [`Node`]s with typed [`Port`]s, [`Edge`]s that connect an output port to an input port
//! with finite buffering, and [`Binding`]s of nodes to adapters. A pipeline definition is not an
//! execution; this crate stops at the declaration, its validation and its plan.
//!
//! A [`Declaration`] holds the declared entities and runs the five declaration commands
//! ([`Declaration::declare_pipeline`], [`Declaration::add_node`], [`Declaration::add_port`],
//! [`Declaration::connect_ports`], [`Declaration::bind_node`]), each refusing what graph.yaml
//! refuses and creating nothing then. The profiles and adapters a declaration references and does
//! not hold are read through a [`Catalog`]. The typed builder ([`Declaration::build`]) declares the
//! same entities with compile-time help: a port handle carries its direction and modality as
//! types, an edge capacity is a [`NonZeroUsize`], and a handle cannot leave the builder that made
//! it, so an unbounded edge, an edge from an input port or a dangling identity cannot be written
//! through it.
//!
//! The sink port's existence and direction, the modality of the two ports of an edge and pipeline
//! membership are not refused by a command; they are checks of validation.
//! [`Declaration::validate_pipeline`] runs checks 1, 2, 3, 5, 6 and 9 of §10.2 over a Declared
//! pipeline and moves it to Validated only when none finds anything; otherwise it answers
//! [`IncompatibleGraph`] naming each [`GraphFinding`]. A pipeline built through the typed builder
//! is validated by the identity [`Declaration::build`] returns, and [`Declaration::validate`]
//! answers the validated form, a [`ValidatedPipeline`], which nothing else produces.
//!
//! Planning is check 4 of §10.2 and the rule of §8.3: [`Declaration::plan`] takes a
//! [`ValidatedPipeline`], so a pipeline that was not validated cannot be planned through the
//! builder, and [`Declaration::plan_pipeline`] runs `inference.graph.PlanPipeline` by identity.
//! Both compute the pipeline's effective capabilities from the weakest node on every required path
//! and refuse a node left without the adapter its requirements need ([`NodeUnbound`]) or an unmet
//! mandatory capability ([`CapabilityMismatch`]); the planned form is a [`PlannedPipeline`]. The
//! crate depends on `inference-core` only and carries no concrete model, backend or provider.
//! Serialized names are the ESS names.

use std::error::Error;
use std::fmt;
use std::num::NonZeroUsize;

use serde::{Deserialize, Serialize};

use inference_core::model::{AdapterId, Capability, Modality};
use inference_core::policy::ProfileId;

mod builder;
mod declaration;
mod planning;
mod validation;

pub use builder::{
    NodeHandle, PipelineBuilder, PipelineSpec, PortDirectionType, PortHandle, PortModality,
    PortTraits, direction, modality,
};
pub use declaration::{
    AddNodeError, BindNodeError, Catalog, ConnectPortsError, Declaration, ValidatePipelineError,
};
pub use inference_core::id::NodeId;
pub use planning::{PlanPipelineError, PlannedPipeline, ValidatedPipeline};

inference_core::uuid_newtype!(
    /// `inference.graph.PipelineId`: identifies a [`Pipeline`].
    PipelineId
);
inference_core::uuid_newtype!(
    /// `inference.graph.PortId`: identifies a [`Port`].
    PortId
);
inference_core::uuid_newtype!(
    /// `inference.graph.EdgeId`: identifies an [`Edge`].
    EdgeId
);
inference_core::uuid_newtype!(
    /// `inference.graph.BindingId`: identifies a [`Binding`].
    BindingId
);

/// `inference.graph.NodeRole`: the node types the design admits; a node is not necessarily a
/// model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeRole {
    Source,
    Model,
    Transform,
    Policy,
    Tool,
    FanOut,
    FanIn,
    Sink,
}

impl NodeRole {
    /// Every role, in the order `ess/domains/graph.yaml` lists them.
    pub const ALL: [Self; 8] = [
        Self::Source,
        Self::Model,
        Self::Transform,
        Self::Policy,
        Self::Tool,
        Self::FanOut,
        Self::FanIn,
        Self::Sink,
    ];
}

/// `inference.graph.PortDirection`: whether a port receives or emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortDirection {
    Input,
    Output,
}

impl PortDirection {
    /// Every direction, in the order `ess/domains/graph.yaml` lists them.
    pub const ALL: [Self; 2] = [Self::Input, Self::Output];
}

/// `inference.graph.Essentiality`: whether a port is essential, optional or control-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Essentiality {
    Essential,
    Optional,
    ControlOnly,
}

impl Essentiality {
    /// Every essentiality, in the order `ess/domains/graph.yaml` lists them.
    pub const ALL: [Self; 3] = [Self::Essential, Self::Optional, Self::ControlOnly];
}

/// `inference.graph.Delivery`: which events can be dropped or coalesced and which require
/// reliable delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    Reliable,
    Droppable,
    Coalescable,
}

impl Delivery {
    /// Every delivery, in the order `ess/domains/graph.yaml` lists them.
    pub const ALL: [Self; 3] = [Self::Reliable, Self::Droppable, Self::Coalescable];
}

/// `inference.graph.InteractionMode`: how a pipeline interacts with its caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionMode {
    Batch,
    TurnBased,
    Continuous,
    Duplex,
}

impl InteractionMode {
    /// Every mode, in the order `ess/domains/graph.yaml` lists them.
    pub const ALL: [Self; 4] = [Self::Batch, Self::TurnBased, Self::Continuous, Self::Duplex];
}

/// `inference.graph.OverloadPolicy`: what an edge does when its buffer is full.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverloadPolicy {
    Pause,
    Cancel,
    Drop,
    Coalesce,
    Supersede,
    Fail,
    DropStaleEpochs,
}

impl OverloadPolicy {
    /// Every overload policy, in the order `ess/domains/graph.yaml` lists them.
    pub const ALL: [Self; 7] = [
        Self::Pause,
        Self::Cancel,
        Self::Drop,
        Self::Coalesce,
        Self::Supersede,
        Self::Fail,
        Self::DropStaleEpochs,
    ];
}

/// `inference.graph.EdgeDefect`: why `ConnectPorts` refused an edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeDefect {
    /// An edge runs from an output port, and the source was an input port.
    SourceNotOutput,
}

/// `inference.graph.GraphDefect`: what validation found wrong with a graph, one variant per check
/// of §10.2 that it makes over the whole graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphDefect {
    /// Check 1: an edge or a binding names a node or port that is not part of the pipeline.
    MissingNodeOrPort,
    /// Checks 2 and 3: an edge's ports differ in direction, modality or commitment semantics.
    IncompatibleEdge,
    /// Check 5: a cycle, which no bounded feedback declaration admits yet.
    UnboundedCycle,
    /// Check 6: an essential port no edge connects, or a sink no source reaches.
    UnreachableMandatoryOutput,
    /// Check 9: provisional data flows into a tool that requires committed input.
    DisallowedDataFlow,
}

impl GraphDefect {
    /// Every defect, in the order `ess/domains/graph.yaml` lists them.
    pub const ALL: [Self; 5] = [
        Self::MissingNodeOrPort,
        Self::IncompatibleEdge,
        Self::UnboundedCycle,
        Self::UnreachableMandatoryOutput,
        Self::DisallowedDataFlow,
    ];
}

/// `inference.graph.GraphFinding`: one finding of validation, naming the node or port where it
/// is. A finding on an edge names the edge's source port and that port's node, since a finding
/// carries no edge identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphFinding {
    pub defect: GraphDefect,
    pub node_id: Option<NodeId>,
    pub port_id: Option<PortId>,
}

/// `inference.graph.ValidatePipeline`: the input that validates a [`Pipeline`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidatePipeline {
    pub pipeline_id: PipelineId,
}

/// `inference.graph.PipelineValidated`: the pipeline passed validation and is Validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineValidated {
    pub pipeline_id: PipelineId,
}

/// `inference.graph.IncompatibleGraph`: validation found the graph invalid; each finding is named
/// and the pipeline stays Declared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncompatibleGraph {
    pub findings: Vec<GraphFinding>,
}

impl fmt::Display for IncompatibleGraph {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the graph is not valid: {:?}", self.findings)
    }
}

impl Error for IncompatibleGraph {}

/// `inference.graph.PlanPipeline`: the input that plans a [`Pipeline`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanPipeline {
    pub pipeline_id: PipelineId,
}

/// `inference.graph.PipelinePlanned`: the pipeline has an execution plan and its effective
/// capabilities are known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelinePlanned {
    pub pipeline_id: PipelineId,
}

/// `inference.graph.CapabilityMismatch`: a mandatory capability is not satisfied by the bound
/// adapters, so planning failed rather than downgrade silently. `node_id` names the node whose
/// binding falls short; a capability the pipeline requires as a whole names no node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityMismatch {
    pub node_id: Option<NodeId>,
    pub capability: Capability,
}

impl fmt::Display for CapabilityMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.node_id {
            Some(node_id) => write!(
                f,
                "the node {node_id} requires {:?}, which its bound adapter does not implement",
                self.capability
            ),
            None => write!(
                f,
                "the pipeline requires {:?}, which its effective capabilities lack",
                self.capability
            ),
        }
    }
}

impl Error for CapabilityMismatch {}

/// `inference.graph.NodeUnbound`: a node whose requirements need an adapter has no binding, so
/// the pipeline is not runnable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeUnbound {
    pub node_id: NodeId,
}

impl fmt::Display for NodeUnbound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the node {} has no adapter bound", self.node_id)
    }
}

impl Error for NodeUnbound {}

/// `inference.graph.PipelineStateConflict`: the pipeline is not in a state the command acts from,
/// so nothing moved. `current` is the state it is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineStateConflict {
    pub current: PipelineState,
}

impl fmt::Display for PipelineStateConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the pipeline is {:?}, so nothing moved", self.current)
    }
}

impl Error for PipelineStateConflict {}

/// `inference.graph.Pipeline.State`: a pipeline is Declared until validated, then Planned.
/// Serialized as written in ESS (`Declared`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PipelineState {
    /// The declaration can still change.
    Declared,
    /// The graph passed validation.
    Validated,
    /// The effective capabilities are computed; the pipeline is runnable.
    Planned,
}

impl PipelineState {
    /// Every state, in the order `ess/domains/graph.yaml` lists them.
    pub const ALL: [Self; 3] = [Self::Declared, Self::Validated, Self::Planned];
}

/// `inference.graph.DeclarePipeline`: the input that declares a [`Pipeline`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclarePipeline {
    pub name: String,
    pub schema_revision: String,
    pub mode: InteractionMode,
    pub profile_id: ProfileId,
    pub required_capabilities: Vec<Capability>,
}

/// `inference.graph.AddNode`: the input that adds a [`Node`] to a pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddNode {
    pub pipeline_id: PipelineId,
    pub name: String,
    pub role: NodeRole,
    pub requires: Vec<Capability>,
}

/// `inference.graph.AddPort`: the input that declares a [`Port`] on a node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddPort {
    pub node_id: NodeId,
    pub name: String,
    pub direction: PortDirection,
    pub modality: Modality,
    pub essentiality: Essentiality,
    pub delivery: Delivery,
    pub incremental: bool,
    pub end_marker_required: bool,
    pub provisional_allowed: bool,
}

/// `inference.graph.ConnectPorts`: the input that connects two ports with an [`Edge`]. The
/// capacity is the ESS `Integer` sent; the command refuses one below one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectPorts {
    pub pipeline_id: PipelineId,
    pub from_port: PortId,
    pub to_port: PortId,
    pub capacity: i64,
    pub overload: OverloadPolicy,
}

/// `inference.graph.BindNode`: the input that binds a node to an adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindNode {
    pub pipeline_id: PipelineId,
    pub node_id: NodeId,
    pub adapter_id: AdapterId,
}

/// `inference.graph.PipelineDeclared`: a pipeline was declared and holds no nodes yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineDeclared {
    pub pipeline_id: PipelineId,
    pub name: String,
}

/// `inference.graph.NodeAdded`: a node is part of its pipeline's declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeAdded {
    pub node_id: NodeId,
    pub pipeline_id: PipelineId,
    pub name: String,
}

/// `inference.graph.PortAdded`: a port is declared on its node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortAdded {
    pub port_id: PortId,
    pub node_id: NodeId,
    pub name: String,
}

/// `inference.graph.PortsConnected`: an edge exists with finite buffering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortsConnected {
    pub edge_id: EdgeId,
    pub from_port: PortId,
    pub to_port: PortId,
}

/// `inference.graph.NodeBound`: a node has a concrete adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeBound {
    pub binding_id: BindingId,
    pub node_id: NodeId,
    pub adapter_id: AdapterId,
}

/// `inference.graph.PipelineNotFound`: no pipeline carries the identity the command named.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineNotFound {
    pub pipeline_id: PipelineId,
}

impl fmt::Display for PipelineNotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no pipeline carries the identity {}", self.pipeline_id)
    }
}

impl Error for PipelineNotFound {}

/// `inference.graph.PipelineNotReady`: the pipeline is not in the state the command needs, so
/// nothing was created. `required` is that state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineNotReady {
    pub pipeline_id: PipelineId,
    pub required: PipelineState,
}

impl fmt::Display for PipelineNotReady {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the pipeline {} is not {:?}",
            self.pipeline_id, self.required
        )
    }
}

impl Error for PipelineNotReady {}

/// `inference.graph.NodeNotFound`: no node carries the identity the command named.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeNotFound {
    pub node_id: NodeId,
}

impl fmt::Display for NodeNotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no node carries the identity {}", self.node_id)
    }
}

impl Error for NodeNotFound {}

/// `inference.graph.PortNotFound`: no port carries the identity the command named.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortNotFound {
    pub port_id: PortId,
}

impl fmt::Display for PortNotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no port carries the identity {}", self.port_id)
    }
}

impl Error for PortNotFound {}

/// `inference.graph.UnboundedEdge`: every edge must specify finite buffering; a capacity below one
/// is refused. `submitted` is the value that was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnboundedEdge {
    pub submitted: i64,
}

impl fmt::Display for UnboundedEdge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "an edge's capacity must be at least one, got {}",
            self.submitted
        )
    }
}

impl Error for UnboundedEdge {}

/// `inference.graph.IncompatibleEdge`: the edge does not run from an output port to an input
/// port, so no edge was created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncompatibleEdge {
    pub from_port: PortId,
    pub to_port: PortId,
    pub defect: EdgeDefect,
}

impl fmt::Display for IncompatibleEdge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "no edge from {} to {}: {:?}",
            self.from_port, self.to_port, self.defect
        )
    }
}

impl Error for IncompatibleEdge {}

/// `inference.graph.AdapterNotFound`: no adapter carries the identity the command named, so
/// nothing was bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterNotFound {
    pub adapter_id: AdapterId,
}

impl fmt::Display for AdapterNotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no adapter carries the identity {}", self.adapter_id)
    }
}

impl Error for AdapterNotFound {}

/// `inference.graph.Pipeline`: a directed, typed graph of nodes and edges; a declaration, not an
/// execution. It owns its nodes, edges and bindings and references its deployment profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Pipeline {
    pipeline_id: PipelineId,
    name: String,
    schema_revision: String,
    mode: InteractionMode,
    profile_id: ProfileId,
    required_capabilities: Vec<Capability>,
    effective_capabilities: Option<Vec<Capability>>,
    downgrades: Option<Vec<Capability>>,
    state: PipelineState,
}

impl Pipeline {
    #[must_use]
    pub const fn pipeline_id(&self) -> PipelineId {
        self.pipeline_id
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn schema_revision(&self) -> &str {
        &self.schema_revision
    }

    #[must_use]
    pub const fn mode(&self) -> InteractionMode {
        self.mode
    }

    #[must_use]
    pub const fn profile_id(&self) -> ProfileId {
        self.profile_id
    }

    #[must_use]
    pub fn required_capabilities(&self) -> &[Capability] {
        &self.required_capabilities
    }

    /// None until the pipeline is planned.
    #[must_use]
    pub fn effective_capabilities(&self) -> Option<&[Capability]> {
        self.effective_capabilities.as_deref()
    }

    /// None until the pipeline is planned.
    #[must_use]
    pub fn downgrades(&self) -> Option<&[Capability]> {
        self.downgrades.as_deref()
    }

    #[must_use]
    pub const fn state(&self) -> PipelineState {
        self.state
    }
}

/// `inference.graph.Node`: a processor with named typed ports; a model, a transform, a policy, a
/// tool, a source or a sink.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Node {
    node_id: NodeId,
    pipeline_id: PipelineId,
    name: String,
    role: NodeRole,
    requires: Vec<Capability>,
}

impl Node {
    #[must_use]
    pub const fn node_id(&self) -> NodeId {
        self.node_id
    }

    #[must_use]
    pub const fn pipeline_id(&self) -> PipelineId {
        self.pipeline_id
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn role(&self) -> NodeRole {
        self.role
    }

    #[must_use]
    pub fn requires(&self) -> &[Capability] {
        &self.requires
    }
}

/// `inference.graph.Port`: a named, typed input or output endpoint of a node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Port {
    port_id: PortId,
    node_id: NodeId,
    name: String,
    direction: PortDirection,
    modality: Modality,
    essentiality: Essentiality,
    delivery: Delivery,
    incremental: bool,
    end_marker_required: bool,
    provisional_allowed: bool,
}

impl Port {
    #[must_use]
    pub const fn port_id(&self) -> PortId {
        self.port_id
    }

    #[must_use]
    pub const fn node_id(&self) -> NodeId {
        self.node_id
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn direction(&self) -> PortDirection {
        self.direction
    }

    #[must_use]
    pub const fn modality(&self) -> Modality {
        self.modality
    }

    #[must_use]
    pub const fn essentiality(&self) -> Essentiality {
        self.essentiality
    }

    #[must_use]
    pub const fn delivery(&self) -> Delivery {
        self.delivery
    }

    /// Whether frames arrive incrementally.
    #[must_use]
    pub const fn incremental(&self) -> bool {
        self.incremental
    }

    /// Whether an end-of-input marker is required.
    #[must_use]
    pub const fn end_marker_required(&self) -> bool {
        self.end_marker_required
    }

    /// Whether provisional data and revisions may arrive.
    #[must_use]
    pub const fn provisional_allowed(&self) -> bool {
        self.provisional_allowed
    }
}

/// `inference.graph.Edge`: a bounded channel from an output port to a compatible input port. Its
/// capacity is at least one by type, the invariant `capacity > 0` of graph.yaml.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Edge {
    edge_id: EdgeId,
    pipeline_id: PipelineId,
    from_port: PortId,
    to_port: PortId,
    capacity: NonZeroUsize,
    overload: OverloadPolicy,
}

impl Edge {
    #[must_use]
    pub const fn edge_id(&self) -> EdgeId {
        self.edge_id
    }

    #[must_use]
    pub const fn pipeline_id(&self) -> PipelineId {
        self.pipeline_id
    }

    #[must_use]
    pub const fn from_port(&self) -> PortId {
        self.from_port
    }

    #[must_use]
    pub const fn to_port(&self) -> PortId {
        self.to_port
    }

    #[must_use]
    pub const fn capacity(&self) -> NonZeroUsize {
        self.capacity
    }

    #[must_use]
    pub const fn overload(&self) -> OverloadPolicy {
        self.overload
    }
}

/// `inference.graph.Binding`: a node bound to a concrete adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Binding {
    binding_id: BindingId,
    pipeline_id: PipelineId,
    node_id: NodeId,
    adapter_id: AdapterId,
}

impl Binding {
    #[must_use]
    pub const fn binding_id(&self) -> BindingId {
        self.binding_id
    }

    #[must_use]
    pub const fn pipeline_id(&self) -> PipelineId {
        self.pipeline_id
    }

    #[must_use]
    pub const fn node_id(&self) -> NodeId {
        self.node_id
    }

    #[must_use]
    pub const fn adapter_id(&self) -> AdapterId {
        self.adapter_id
    }
}
