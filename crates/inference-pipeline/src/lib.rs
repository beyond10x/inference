//! Pipeline graphs of the inference framework.
//!
//! `inference-pipeline` holds the declaration half of `ess/domains/graph.yaml`: a [`Pipeline`] of
//! named [`Node`]s with typed [`Port`]s, [`Edge`]s that connect an output port to an input port
//! with finite buffering, [`Exposure`]s that expose a port to the application under a name, an
//! ingress or an egress whose far end is outside the pipeline, [`FeedbackLoop`]s that bound the
//! cycles a feedback edge closes, and [`Binding`]s of nodes to adapters. A pipeline definition is
//! not an execution; this crate stops at the declaration, its validation and its plan.
//!
//! A [`Declaration`] holds the declared entities and runs the seven declaration commands
//! ([`Declaration::declare_pipeline`], [`Declaration::add_node`], [`Declaration::add_port`],
//! [`Declaration::connect_ports`], [`Declaration::expose_port`],
//! [`Declaration::declare_feedback_loop`], [`Declaration::bind_node`]), each refusing what
//! graph.yaml refuses and creating nothing then. The profiles and adapters a declaration
//! references and does not hold are read through a [`Catalog`]. The typed builder
//! ([`Declaration::build`]) declares the
//! same entities, reading its profile and adapters by identity through the same catalog and
//! refusing what the commands refuse, with compile-time help: a port handle carries its direction
//! and modality as types, an edge capacity is a [`NonZeroUsize`], and a handle cannot leave the
//! builder that made it, so an unbounded edge, an edge from an input port or a dangling identity
//! cannot be written through it.
//!
//! The sink port's existence and direction, the modality of the two ports of an edge and pipeline
//! membership are not refused by a command; they are checks of validation.
//! [`Declaration::validate_pipeline`] runs checks 1, 2, 3, 5, 6 and 9 of §10.2 over a Declared
//! pipeline and moves it to Validated only when none finds anything; check 5 admits a cycle only
//! when it runs through an edge a [`FeedbackLoop`] of the pipeline bounds; otherwise it answers
//! [`IncompatibleGraph`] naming each [`GraphFinding`]. A pipeline built through the typed builder
//! is validated by the identity [`Declaration::build`] returns, and [`Declaration::validate`]
//! answers the validated form, a [`ValidatedPipeline`], which nothing else produces.
//!
//! Planning is check 4 of §10.2 and the rule of §8.3: [`Declaration::plan`] takes a
//! [`ValidatedPipeline`], so a pipeline that was not validated cannot be planned through the
//! builder, and [`Declaration::plan_pipeline`] runs `inference.graph.PlanPipeline` by identity.
//! Both compute the pipeline's effective capabilities from the weakest node on every required path
//! and refuse an adapter running at a locality the pipeline's profile does not admit
//! ([`LocalityDenied`]), a node left without the adapter its requirements need ([`NodeUnbound`])
//! or an unmet mandatory capability ([`CapabilityMismatch`]); the planned form is a
//! [`PlannedPipeline`].
//!
//! A Planned pipeline may stand as a node of another ([`Declaration::add_subgraph_node`],
//! [`PipelineBuilder::subgraph`]): its exposures are the ingress and egress contracts the node's
//! ports are held to when the parent is planned.
//!
//! A pipeline may also be written declaratively, as JSON ([`PipelineDefinition`]), naming each
//! node by the kind of a [`NodeDescriptor`] its implementation publishes
//! (`inference.graph.DeclareNodeDescriptor`, held by [`NodeDescriptors`]);
//! [`Declaration::declare_definition`] checks it against the descriptors before anything is
//! declared and then declares it as the builder would, so validation and planning are shared. The
//! descriptors of the node implementations of this crate are in [`nodes::descriptors`].
//!
//! A node implementation writes one trait, [`NodeImplementation`] ([`node`]): describe, prepare,
//! start with its edge endpoints, drain or cancel, stop, which the runtime drives for each node
//! instance of a run. The node implementations that carry no model, the tool authority and the
//! tool executor, are in [`nodes`]. The crate depends on `inference-core` and, for those nodes,
//! `inference-tools`, and carries no concrete model, backend or provider. Serialized names are
//! the ESS names.

use std::error::Error;
use std::fmt;
use std::num::NonZeroUsize;

use serde::{Deserialize, Serialize};

use inference_core::model::{AdapterId, Capability, Locality, Modality};
use inference_core::policy::ProfileId;

mod builder;
mod declaration;
mod definition;
pub mod node;
pub mod nodes;
mod planning;
mod validation;

pub use builder::{
    Exposed, NodeHandle, PipelineBuilder, PipelineSpec, PortDirectionType, PortHandle,
    PortModality, PortTraits, direction, modality,
};
pub use declaration::{
    AddNodeError, AddPortError, AddSubgraphNodeError, BindNodeError, Catalog, ConnectPortsError,
    Declaration, DeclareFeedbackLoopError, ExposePortError, ValidatePipelineError,
};
pub use definition::{
    BindingDefinition, DeclaredDefinition, DefinitionDefect, DefinitionError, DefinitionFinding,
    DefinitionNotRead, EdgeDefinition, ExposureDefinition, FeedbackLoopDefinition, NodeDefinition,
    NodeDescriptors, PipelineDefinition, PortReference,
};
pub use inference_core::id::NodeId;
pub use node::{NodeEndpoints, NodeError, NodeFuture, NodeImplementation, NodeInput, NodeOutput};
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
    /// `inference.graph.ExposureId`: identifies an [`Exposure`].
    ExposureId
);
inference_core::uuid_newtype!(
    /// `inference.graph.BindingId`: identifies a [`Binding`].
    BindingId
);
inference_core::uuid_newtype!(
    /// `inference.graph.NodeInstanceId`: identifies an `inference.graph.NodeInstance`, a node as it
    /// participates in one run; the runtime describes it and hands it to the node implementation
    /// ([`NodeImplementation::described`]).
    NodeInstanceId
);
inference_core::uuid_newtype!(
    /// `inference.graph.FeedbackLoopId`: identifies a [`FeedbackLoop`].
    FeedbackLoopId
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
/// of §10.2 that it makes over the whole graph, and two exposures of one name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphDefect {
    /// Check 1: an edge, an exposure or a binding names a node or port that is not part of the
    /// pipeline.
    MissingNodeOrPort,
    /// Checks 2 and 3: an edge's ports differ in direction, modality or commitment semantics.
    IncompatibleEdge,
    /// Check 5: a cycle that runs through no edge a [`FeedbackLoop`] of the pipeline bounds.
    UnboundedCycle,
    /// Check 6: an essential output port that is the source of no edge and no egress, or a sink
    /// no source or ingress reaches.
    UnreachableMandatoryOutput,
    /// Check 9: provisional data flows into a tool that requires committed input.
    DisallowedDataFlow,
    /// An exposure carries a name another exposure of the pipeline declared before it carries,
    /// so the application cannot tell the two apart.
    DuplicateExposureName,
}

impl GraphDefect {
    /// Every defect, in the order `ess/domains/graph.yaml` lists them.
    pub const ALL: [Self; 6] = [
        Self::MissingNodeOrPort,
        Self::IncompatibleEdge,
        Self::UnboundedCycle,
        Self::UnreachableMandatoryOutput,
        Self::DisallowedDataFlow,
        Self::DuplicateExposureName,
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

/// `inference.graph.Pipeline.State`: a pipeline is Declared until validation passes, then
/// Validated until planning succeeds, then Planned. Serialized as written in ESS (`Declared`).
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

/// `inference.graph.AddSubgraphNode`: the input that adds to the pipeline `pipeline_id` a [`Node`]
/// of role `role` standing for the Planned pipeline `subgraph_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddSubgraphNode {
    pub pipeline_id: PipelineId,
    pub name: String,
    pub role: NodeRole,
    pub subgraph_id: PipelineId,
}

/// `inference.graph.SubgraphNodeAdded`: a node stands for its child pipeline in its parent's
/// declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubgraphNodeAdded {
    pub node_id: NodeId,
    pub pipeline_id: PipelineId,
    pub subgraph_id: PipelineId,
    pub name: String,
}

/// `inference.graph.SubgraphInterfaceMismatch`: the ports of the subgraph node `node_id` do not
/// match its child pipeline's exposures one to one by name, direction and modality. `port_id`
/// names a port no exposure matches, `exposure_id` an exposure no port matches; both, a port and
/// the exposure of its name that differs from it in direction or modality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubgraphInterfaceMismatch {
    pub node_id: NodeId,
    pub port_id: Option<PortId>,
    pub exposure_id: Option<ExposureId>,
}

impl fmt::Display for SubgraphInterfaceMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the subgraph node {} does not match its child's exposures: port {:?}, exposure {:?}",
            self.node_id, self.port_id, self.exposure_id
        )
    }
}

impl Error for SubgraphInterfaceMismatch {}

/// `inference.graph.LocalityDenied`: the adapter `adapter_id` bound to the node `node_id` runs at
/// `locality`, which the pipeline's deployment profile `profile_id` does not admit, so planning
/// failed and the pipeline stays Validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalityDenied {
    pub node_id: NodeId,
    pub adapter_id: AdapterId,
    pub locality: Locality,
    pub profile_id: ProfileId,
}

impl fmt::Display for LocalityDenied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the node {} is bound to the adapter {}, which runs at {:?}, a locality the profile {} \
             does not admit",
            self.node_id, self.adapter_id, self.locality, self.profile_id
        )
    }
}

impl Error for LocalityDenied {}

/// `inference.graph.AddPort`: the input that declares a [`Port`] on a node, under the pipeline it
/// names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddPort {
    pub pipeline_id: PipelineId,
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

/// `inference.graph.ExposePort`: the input that exposes the port `port_id` of a pipeline to the
/// application under `name`, as an [`Exposure`]. The capacity is the ESS `Integer` sent; the
/// command refuses one below one, as `ConnectPorts` does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExposePort {
    pub pipeline_id: PipelineId,
    pub port_id: PortId,
    pub name: String,
    pub capacity: i64,
    pub overload: OverloadPolicy,
}

/// `inference.graph.DeclareFeedbackLoop`: the input that declares the edge `edge_id` a feedback
/// edge of the pipeline, bounded by the iteration rule `max_iterations`. The maximum is the ESS
/// `Integer` sent; the command refuses one below one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclareFeedbackLoop {
    pub pipeline_id: PipelineId,
    pub edge_id: EdgeId,
    pub max_iterations: i64,
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

/// `inference.graph.PortExposed`: a port of a pipeline is exposed to the application under a
/// name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortExposed {
    pub exposure_id: ExposureId,
    pub pipeline_id: PipelineId,
    pub port_id: PortId,
    pub name: String,
}

/// `inference.graph.FeedbackLoopDeclared`: the edge is a feedback edge of its pipeline, bounded by
/// its iteration rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedbackLoopDeclared {
    pub feedback_id: FeedbackLoopId,
    pub edge_id: EdgeId,
    pub max_iterations: i64,
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

/// `inference.graph.EdgeNotFound`: no edge carries the identity the command named.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeNotFound {
    pub edge_id: EdgeId,
}

impl fmt::Display for EdgeNotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no edge carries the identity {}", self.edge_id)
    }
}

impl Error for EdgeNotFound {}

/// `inference.graph.UnboundedFeedback`: a feedback loop's iteration rule must allow at least one
/// iteration; a maximum below one is refused. `submitted` is the value that was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnboundedFeedback {
    pub submitted: i64,
}

impl fmt::Display for UnboundedFeedback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "a feedback loop must allow at least one iteration, got {}",
            self.submitted
        )
    }
}

impl Error for UnboundedFeedback {}

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
    subgraph_id: Option<PipelineId>,
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

    /// The pipeline the node stands for when it is a subgraph node; none for any other node.
    #[must_use]
    pub const fn subgraph_id(&self) -> Option<PipelineId> {
        self.subgraph_id
    }
}

/// `inference.graph.Port`: a named, typed input or output endpoint of a node, declared under a
/// pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Port {
    port_id: PortId,
    pipeline_id: PipelineId,
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

    /// The pipeline the port was declared under; its node may belong to another, which
    /// validation finds (check 1).
    #[must_use]
    pub const fn pipeline_id(&self) -> PipelineId {
        self.pipeline_id
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

/// `inference.graph.Exposure`: a named source or sink interface of a pipeline, exposed to the
/// application: one port of the pipeline and a bounded channel whose far end is outside it. Its
/// direction is its port's: an exposed input port is an ingress, which the application feeds, and
/// an exposed output port an egress, which the application consumes. Its capacity is at least one
/// by type, the invariant `capacity > 0` of graph.yaml.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Exposure {
    exposure_id: ExposureId,
    pipeline_id: PipelineId,
    port_id: PortId,
    name: String,
    capacity: NonZeroUsize,
    overload: OverloadPolicy,
}

impl Exposure {
    #[must_use]
    pub const fn exposure_id(&self) -> ExposureId {
        self.exposure_id
    }

    #[must_use]
    pub const fn pipeline_id(&self) -> PipelineId {
        self.pipeline_id
    }

    /// The exposed port; it may have been declared under another pipeline, which validation finds
    /// (check 1).
    #[must_use]
    pub const fn port_id(&self) -> PortId {
        self.port_id
    }

    /// The name the application knows the interface by.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
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

/// `inference.graph.NodeKind`: the name a node implementation publishes its [`NodeDescriptor`]
/// under, such as `vad`; a declarative graph names the kind of each of its nodes. Serialized as
/// the plain string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeKind(String);

impl NodeKind {
    /// The kind named `kind`.
    #[must_use]
    pub fn new(kind: impl Into<String>) -> Self {
        Self(kind.into())
    }

    /// The kind's name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for NodeKind {
    fn from(kind: &str) -> Self {
        Self::new(kind)
    }
}

impl fmt::Display for NodeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// `inference.graph.PortDescriptor`: one named input or output port a node implementation offers,
/// with the traits of §9.2 an [`AddPort`] declares of a [`Port`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortDescriptor {
    pub name: String,
    pub direction: PortDirection,
    pub modality: Modality,
    pub essentiality: Essentiality,
    pub delivery: Delivery,
    pub incremental: bool,
    pub end_marker_required: bool,
    pub provisional_allowed: bool,
}

impl PortDescriptor {
    /// The port named `name` of `direction` and `modality`, with the traits `traits`.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        direction: PortDirection,
        modality: Modality,
        traits: PortTraits,
    ) -> Self {
        Self {
            name: name.into(),
            direction,
            modality,
            essentiality: traits.essentiality,
            delivery: traits.delivery,
            incremental: traits.incremental,
            end_marker_required: traits.end_marker_required,
            provisional_allowed: traits.provisional_allowed,
        }
    }
}

/// `inference.graph.DeclareNodeDescriptor`: the input that publishes a [`NodeDescriptor`] under
/// its kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclareNodeDescriptor {
    pub kind: NodeKind,
    pub role: NodeRole,
    pub requires: Vec<Capability>,
    pub ports: Vec<PortDescriptor>,
}

/// `inference.graph.NodeDescriptorDeclared`: a descriptor is published under its kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeDescriptorDeclared {
    pub kind: NodeKind,
    pub role: NodeRole,
}

/// `inference.graph.NodeDescriptorExists`: a descriptor is already declared under the kind, so
/// nothing was declared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeDescriptorExists {
    pub kind: NodeKind,
}

impl fmt::Display for NodeDescriptorExists {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "a descriptor is already declared under the kind {}",
            self.kind
        )
    }
}

impl Error for NodeDescriptorExists {}

/// `inference.graph.NodeDescriptor`: the role, required capabilities and named typed ports a node
/// implementation offers, under its kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeDescriptor {
    kind: NodeKind,
    role: NodeRole,
    requires: Vec<Capability>,
    ports: Vec<PortDescriptor>,
}

impl NodeDescriptor {
    #[must_use]
    pub const fn kind(&self) -> &NodeKind {
        &self.kind
    }

    #[must_use]
    pub const fn role(&self) -> NodeRole {
        self.role
    }

    #[must_use]
    pub fn requires(&self) -> &[Capability] {
        &self.requires
    }

    /// The ports, in the order the descriptor declares them.
    #[must_use]
    pub fn ports(&self) -> &[PortDescriptor] {
        &self.ports
    }

    /// The port named `name`, if the descriptor declares one.
    #[must_use]
    pub fn port(&self, name: &str) -> Option<&PortDescriptor> {
        self.ports.iter().find(|port| port.name == name)
    }
}

/// `inference.graph.FeedbackLoop`: a feedback edge of a pipeline with the iteration rule that
/// bounds the loop it closes: how many times one generation may go round the loop along the edge
/// before the loop terminates. The edge's capacity is the loop's queue bound. Its maximum is at
/// least one by type, the invariant `max_iterations > 0` of graph.yaml.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FeedbackLoop {
    feedback_id: FeedbackLoopId,
    pipeline_id: PipelineId,
    edge_id: EdgeId,
    max_iterations: NonZeroUsize,
}

impl FeedbackLoop {
    #[must_use]
    pub const fn feedback_id(&self) -> FeedbackLoopId {
        self.feedback_id
    }

    #[must_use]
    pub const fn pipeline_id(&self) -> PipelineId {
        self.pipeline_id
    }

    /// The feedback edge; it may belong to another pipeline than the loop's, and then bounds no
    /// cycle of the loop's pipeline.
    #[must_use]
    pub const fn edge_id(&self) -> EdgeId {
        self.edge_id
    }

    #[must_use]
    pub const fn max_iterations(&self) -> NonZeroUsize {
        self.max_iterations
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
