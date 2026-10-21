//! [`Declaration`]: the declared pipelines, nodes, ports, edges, exposures, feedback loops and
//! bindings, the seven declaration commands of `ess/domains/graph.yaml` over them, and
//! `ValidatePipeline`.

use std::error::Error;
use std::fmt;
use std::num::NonZeroUsize;

use uuid::Uuid;

use inference_core::model::{Adapter, AdapterId, Modality};
use inference_core::policy::{DeploymentProfile, ProfileId, ProfileNotFound};

use crate::{
    AdapterNotFound, AddNode, AddPort, AddSubgraphNode, BindNode, Binding, BindingId, ConnectPorts,
    DeclareFeedbackLoop, DeclarePipeline, Edge, EdgeDefect, EdgeId, EdgeNotFound, ExposePort,
    Exposure, ExposureId, FeedbackLoop, FeedbackLoopDeclared, FeedbackLoopId, GraphFinding,
    IncompatibleEdge, IncompatibleGraph, Node, NodeAdded, NodeBound, NodeId, NodeNotFound,
    OverloadPolicy, Pipeline, PipelineDeclared, PipelineId, PipelineNotFound, PipelineNotReady,
    PipelineState, PipelineStateConflict, PipelineValidated, Port, PortAdded, PortDirection,
    PortExposed, PortId, PortNotFound, PortsConnected, SubgraphNodeAdded, UnboundedEdge,
    UnboundedFeedback, ValidatePipeline,
};

/// What a declaration references and does not hold: the declared deployment profiles of
/// `inference.policy` and the declared adapters of `inference.model`.
pub trait Catalog {
    /// The profile carrying `profile_id`, if one was declared.
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile>;

    /// The adapter carrying `adapter_id`, if one was declared.
    fn adapter(&self, adapter_id: AdapterId) -> Option<&Adapter>;
}

/// Why `inference.graph.AddNode` added nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddNodeError {
    /// The `unknown-pipeline` outcome.
    UnknownPipeline(PipelineNotFound),
    /// The `not-declared` outcome: the pipeline was already validated.
    NotDeclared(PipelineNotReady),
}

impl fmt::Display for AddNodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownPipeline(error) => fmt::Display::fmt(error, f),
            Self::NotDeclared(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for AddNodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::UnknownPipeline(error) => Some(error),
            Self::NotDeclared(error) => Some(error),
        }
    }
}

/// Why `inference.graph.AddSubgraphNode` added nothing. The refusals are decided in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddSubgraphNodeError {
    /// The `unknown-pipeline` outcome: no pipeline carries the parent's identity.
    UnknownPipeline(PipelineNotFound),
    /// The `unknown-subgraph` outcome: no pipeline carries the child's identity.
    UnknownSubgraph(PipelineNotFound),
    /// The `not-declared` outcome: the parent was already validated.
    NotDeclared(PipelineNotReady),
    /// The `subgraph-not-planned` outcome: the child has no execution plan yet.
    SubgraphNotPlanned(PipelineNotReady),
}

impl fmt::Display for AddSubgraphNodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownPipeline(error) | Self::UnknownSubgraph(error) => {
                fmt::Display::fmt(error, f)
            }
            Self::NotDeclared(error) | Self::SubgraphNotPlanned(error) => {
                fmt::Display::fmt(error, f)
            }
        }
    }
}

impl Error for AddSubgraphNodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::UnknownPipeline(error) | Self::UnknownSubgraph(error) => Some(error),
            Self::NotDeclared(error) | Self::SubgraphNotPlanned(error) => Some(error),
        }
    }
}

/// Why a pipeline cannot be a subgraph node's child.
#[derive(Clone, Copy)]
pub(crate) enum SubgraphRefusal {
    Unknown(PipelineNotFound),
    NotPlanned(PipelineNotReady),
}

impl SubgraphRefusal {
    pub(crate) const fn into_add_error(self) -> AddSubgraphNodeError {
        match self {
            Self::Unknown(error) => AddSubgraphNodeError::UnknownSubgraph(error),
            Self::NotPlanned(error) => AddSubgraphNodeError::SubgraphNotPlanned(error),
        }
    }
}

/// Whether `port` is a usage port: a structured output port named [`crate::USAGE_PORT`].
pub(crate) fn carries_usage(port: &Port) -> bool {
    port.name == crate::USAGE_PORT
        && port.direction == PortDirection::Output
        && port.modality == Modality::Structured
}

/// Why `inference.graph.AddPort` added nothing. The refusals are decided in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddPortError {
    /// The `unknown-pipeline` outcome.
    UnknownPipeline(PipelineNotFound),
    /// The `unknown-node` outcome.
    UnknownNode(NodeNotFound),
    /// The `not-declared` outcome: the pipeline was already validated.
    NotDeclared(PipelineNotReady),
}

impl fmt::Display for AddPortError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownPipeline(error) => fmt::Display::fmt(error, f),
            Self::UnknownNode(error) => fmt::Display::fmt(error, f),
            Self::NotDeclared(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for AddPortError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::UnknownPipeline(error) => Some(error),
            Self::UnknownNode(error) => Some(error),
            Self::NotDeclared(error) => Some(error),
        }
    }
}

/// Why `inference.graph.ConnectPorts` created no edge. The refusals are decided in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectPortsError {
    /// The `unknown-pipeline` outcome.
    UnknownPipeline(PipelineNotFound),
    /// The `unknown-source` outcome: no port carries `from_port`.
    UnknownSource(PortNotFound),
    /// The `unbounded` outcome: the capacity was below one.
    Unbounded(UnboundedEdge),
    /// The `not-declared` outcome: the pipeline was already validated.
    NotDeclared(PipelineNotReady),
    /// The `source-not-output` outcome: the source port is an input port.
    SourceNotOutput(IncompatibleEdge),
}

impl fmt::Display for ConnectPortsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownPipeline(error) => fmt::Display::fmt(error, f),
            Self::UnknownSource(error) => fmt::Display::fmt(error, f),
            Self::Unbounded(error) => fmt::Display::fmt(error, f),
            Self::NotDeclared(error) => fmt::Display::fmt(error, f),
            Self::SourceNotOutput(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for ConnectPortsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::UnknownPipeline(error) => Some(error),
            Self::UnknownSource(error) => Some(error),
            Self::Unbounded(error) => Some(error),
            Self::NotDeclared(error) => Some(error),
            Self::SourceNotOutput(error) => Some(error),
        }
    }
}

/// Why `inference.graph.ExposePort` exposed nothing. The refusals are decided in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExposePortError {
    /// The `unknown-pipeline` outcome.
    UnknownPipeline(PipelineNotFound),
    /// The `unknown-port` outcome: no port carries `port_id`.
    UnknownPort(PortNotFound),
    /// The `unbounded` outcome: the capacity was below one.
    Unbounded(UnboundedEdge),
    /// The `not-declared` outcome: the pipeline was already validated.
    NotDeclared(PipelineNotReady),
}

impl fmt::Display for ExposePortError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownPipeline(error) => fmt::Display::fmt(error, f),
            Self::UnknownPort(error) => fmt::Display::fmt(error, f),
            Self::Unbounded(error) => fmt::Display::fmt(error, f),
            Self::NotDeclared(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for ExposePortError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::UnknownPipeline(error) => Some(error),
            Self::UnknownPort(error) => Some(error),
            Self::Unbounded(error) => Some(error),
            Self::NotDeclared(error) => Some(error),
        }
    }
}

/// Why `inference.graph.DeclareFeedbackLoop` declared nothing. The refusals are decided in this
/// order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclareFeedbackLoopError {
    /// The `unknown-pipeline` outcome.
    UnknownPipeline(PipelineNotFound),
    /// The `unknown-edge` outcome: no edge carries `edge_id`.
    UnknownEdge(EdgeNotFound),
    /// The `unbounded` outcome: the maximum was below one.
    Unbounded(UnboundedFeedback),
    /// The `not-declared` outcome: the pipeline was already validated.
    NotDeclared(PipelineNotReady),
}

impl fmt::Display for DeclareFeedbackLoopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownPipeline(error) => fmt::Display::fmt(error, f),
            Self::UnknownEdge(error) => fmt::Display::fmt(error, f),
            Self::Unbounded(error) => fmt::Display::fmt(error, f),
            Self::NotDeclared(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for DeclareFeedbackLoopError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::UnknownPipeline(error) => Some(error),
            Self::UnknownEdge(error) => Some(error),
            Self::Unbounded(error) => Some(error),
            Self::NotDeclared(error) => Some(error),
        }
    }
}

/// Why `inference.graph.BindNode` bound nothing. The refusals are decided in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindNodeError {
    /// The `unknown-pipeline` outcome.
    UnknownPipeline(PipelineNotFound),
    /// The `unknown-node` outcome.
    UnknownNode(NodeNotFound),
    /// The `unknown-adapter` outcome: the catalog holds no adapter carrying the identity.
    UnknownAdapter(AdapterNotFound),
    /// The `not-declared` outcome: the pipeline was already validated.
    NotDeclared(PipelineNotReady),
}

impl fmt::Display for BindNodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownPipeline(error) => fmt::Display::fmt(error, f),
            Self::UnknownNode(error) => fmt::Display::fmt(error, f),
            Self::UnknownAdapter(error) => fmt::Display::fmt(error, f),
            Self::NotDeclared(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for BindNodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::UnknownPipeline(error) => Some(error),
            Self::UnknownNode(error) => Some(error),
            Self::UnknownAdapter(error) => Some(error),
            Self::NotDeclared(error) => Some(error),
        }
    }
}

/// Why `inference.graph.ValidatePipeline` moved nothing. The refusals are decided in this order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidatePipelineError {
    /// No pipeline carries the identity, so none is in the state the command acts from; the
    /// `wrong-state` outcome, with no state to report.
    UnknownPipeline(PipelineNotFound),
    /// The `wrong-state` outcome: the pipeline is not Declared.
    WrongState(PipelineStateConflict),
    /// The `rejected` outcome: a check failed, and the pipeline stays Declared.
    Rejected(IncompatibleGraph),
}

impl fmt::Display for ValidatePipelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownPipeline(error) => fmt::Display::fmt(error, f),
            Self::WrongState(error) => fmt::Display::fmt(error, f),
            Self::Rejected(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for ValidatePipelineError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::UnknownPipeline(error) => Some(error),
            Self::WrongState(error) => Some(error),
            Self::Rejected(error) => Some(error),
        }
    }
}

/// Where a declaration takes the identities of what it creates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Identities {
    /// A random (version 4) identity each time.
    #[default]
    Generated,
    /// The next value of a counter in the first eight hex digits' `space`,
    /// `{space:08x}-0000-4000-8000-000000000001` first; `issued` is the count issued so far.
    Counted { space: u32, issued: u128 },
}

/// The declared pipelines and what they own, in declaration order.
///
/// Every command checks first and creates after, so a refused command creates nothing and takes
/// no identity. A declaration from [`Declaration::new`] gives each entity a random identity; one
/// from [`Declaration::counted`] gives the next value of a counter, so that two runs of the same
/// commands report the same identities, as a store or a test needs. Two declarations are equal
/// when they hold the same entities and would issue the same next identity.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Declaration {
    pipelines: Vec<Pipeline>,
    nodes: Vec<Node>,
    ports: Vec<Port>,
    edges: Vec<Edge>,
    exposures: Vec<Exposure>,
    feedback_loops: Vec<FeedbackLoop>,
    bindings: Vec<Binding>,
    identities: Identities,
}

impl Declaration {
    /// An empty declaration giving random identities.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty declaration giving counted identities, `00000000-0000-4000-8000-000000000001`
    /// first.
    #[must_use]
    pub fn counted() -> Self {
        Self::counted_in(0)
    }

    /// An empty declaration giving counted identities whose first eight hex digits are `space`,
    /// `{space:08x}-0000-4000-8000-000000000001` first, so that stores counting in different
    /// spaces never issue the same identity.
    #[must_use]
    pub fn counted_in(space: u32) -> Self {
        Self {
            identities: Identities::Counted { space, issued: 0 },
            ..Self::default()
        }
    }

    /// Runs `inference.graph.DeclarePipeline`: the pipeline is Declared and holds no nodes, and
    /// its effective capabilities and downgrades are unset until planning.
    ///
    /// # Errors
    ///
    /// [`ProfileNotFound`] when `catalog` holds no profile carrying `profile_id`.
    pub fn declare_pipeline(
        &mut self,
        input: DeclarePipeline,
        catalog: &(impl Catalog + ?Sized),
    ) -> Result<PipelineDeclared, ProfileNotFound> {
        if catalog.profile(input.profile_id).is_none() {
            return Err(ProfileNotFound {
                profile_id: input.profile_id,
            });
        }
        Ok(self.insert_pipeline(input))
    }

    /// Runs `inference.graph.AddNode`.
    ///
    /// # Errors
    ///
    /// [`AddNodeError::UnknownPipeline`], then [`AddNodeError::NotDeclared`] for a pipeline that
    /// is not Declared.
    pub fn add_node(&mut self, input: AddNode) -> Result<NodeAdded, AddNodeError> {
        let pipeline = self
            .pipeline(input.pipeline_id)
            .ok_or(AddNodeError::UnknownPipeline(PipelineNotFound {
                pipeline_id: input.pipeline_id,
            }))?;
        Self::declared(pipeline).map_err(AddNodeError::NotDeclared)?;
        Ok(self.insert_node(input))
    }

    /// Runs `inference.graph.AddSubgraphNode`: a node of the role the input names, requiring
    /// nothing of a binding, stands for the child pipeline in the parent's declaration. Its ports
    /// are added with [`Declaration::add_port`]; planning holds them to the child's exposures.
    ///
    /// # Errors
    ///
    /// In this order: [`AddSubgraphNodeError::UnknownPipeline`];
    /// [`AddSubgraphNodeError::UnknownSubgraph`]; [`AddSubgraphNodeError::NotDeclared`];
    /// [`AddSubgraphNodeError::SubgraphNotPlanned`].
    pub fn add_subgraph_node(
        &mut self,
        input: AddSubgraphNode,
    ) -> Result<SubgraphNodeAdded, AddSubgraphNodeError> {
        let parent =
            self.pipeline(input.pipeline_id)
                .ok_or(AddSubgraphNodeError::UnknownPipeline(PipelineNotFound {
                    pipeline_id: input.pipeline_id,
                }))?;
        let child = self.subgraph_of(input.subgraph_id);
        if let Err(SubgraphRefusal::Unknown(error)) = child {
            return Err(AddSubgraphNodeError::UnknownSubgraph(error));
        }
        Self::declared(parent).map_err(AddSubgraphNodeError::NotDeclared)?;
        let child = child.map_err(SubgraphRefusal::into_add_error)?;
        Ok(self.insert_subgraph_node(input.pipeline_id, input.name, input.role, child))
    }

    /// The child a subgraph node may stand for: a pipeline that exists and is Planned.
    pub(crate) fn subgraph_of(
        &self,
        subgraph_id: PipelineId,
    ) -> Result<PipelineId, SubgraphRefusal> {
        let child =
            self.pipeline(subgraph_id)
                .ok_or(SubgraphRefusal::Unknown(PipelineNotFound {
                    pipeline_id: subgraph_id,
                }))?;
        if child.state == PipelineState::Planned {
            Ok(subgraph_id)
        } else {
            Err(SubgraphRefusal::NotPlanned(PipelineNotReady {
                pipeline_id: subgraph_id,
                required: PipelineState::Planned,
            }))
        }
    }

    /// Runs `inference.graph.AddPort`: the port is declared on its node under the pipeline the
    /// input names, which must still be Declared. A node of another pipeline is not refused here;
    /// the port then belongs to the pipeline it names, not to its node's, and validation of that
    /// pipeline finds its node missing (check 1).
    ///
    /// # Errors
    ///
    /// In this order: [`AddPortError::UnknownPipeline`]; [`AddPortError::UnknownNode`] when no
    /// node carries `node_id`; [`AddPortError::NotDeclared`].
    pub fn add_port(&mut self, input: AddPort) -> Result<PortAdded, AddPortError> {
        let pipeline = self
            .pipeline(input.pipeline_id)
            .ok_or(AddPortError::UnknownPipeline(PipelineNotFound {
                pipeline_id: input.pipeline_id,
            }))?;
        if self.node(input.node_id).is_none() {
            return Err(AddPortError::UnknownNode(NodeNotFound {
                node_id: input.node_id,
            }));
        }
        Self::declared(pipeline).map_err(AddPortError::NotDeclared)?;
        Ok(self.insert_port(input))
    }

    /// Runs `inference.graph.ConnectPorts`. The sink port's existence and direction, the modality
    /// of the two ports and their pipeline are not refused here; they are checks of validation.
    ///
    /// # Errors
    ///
    /// In this order: [`ConnectPortsError::UnknownPipeline`]; [`ConnectPortsError::UnknownSource`]
    /// when no port carries `from_port`; [`ConnectPortsError::Unbounded`] carrying the capacity
    /// sent when it is below one; [`ConnectPortsError::NotDeclared`];
    /// [`ConnectPortsError::SourceNotOutput`] when the source is an input port.
    pub fn connect_ports(
        &mut self,
        input: ConnectPorts,
    ) -> Result<PortsConnected, ConnectPortsError> {
        let pipeline =
            self.pipeline(input.pipeline_id)
                .ok_or(ConnectPortsError::UnknownPipeline(PipelineNotFound {
                    pipeline_id: input.pipeline_id,
                }))?;
        let source = self
            .port(input.from_port)
            .ok_or(ConnectPortsError::UnknownSource(PortNotFound {
                port_id: input.from_port,
            }))?;
        let capacity =
            bounded(input.capacity).ok_or(ConnectPortsError::Unbounded(UnboundedEdge {
                submitted: input.capacity,
            }))?;
        Self::declared(pipeline).map_err(ConnectPortsError::NotDeclared)?;
        if source.direction() != PortDirection::Output {
            return Err(ConnectPortsError::SourceNotOutput(IncompatibleEdge {
                from_port: input.from_port,
                to_port: input.to_port,
                defect: EdgeDefect::SourceNotOutput,
            }));
        }
        Ok(self.insert_edge(
            input.pipeline_id,
            input.from_port,
            input.to_port,
            capacity,
            input.overload,
        ))
    }

    /// Runs `inference.graph.ExposePort`: the port is exposed to the application under the name
    /// the input carries, with finite buffering and an overload behaviour. A port of another
    /// pipeline is not refused here; validation of the pipeline finds it missing (check 1). Nor is
    /// a second exposure of one name; validation finds it (`duplicate_exposure_name`).
    ///
    /// # Errors
    ///
    /// In this order: [`ExposePortError::UnknownPipeline`]; [`ExposePortError::UnknownPort`] when
    /// no port carries `port_id`; [`ExposePortError::Unbounded`] carrying the capacity sent when
    /// it is below one; [`ExposePortError::NotDeclared`].
    pub fn expose_port(&mut self, input: ExposePort) -> Result<PortExposed, ExposePortError> {
        let pipeline = self
            .pipeline(input.pipeline_id)
            .ok_or(ExposePortError::UnknownPipeline(PipelineNotFound {
                pipeline_id: input.pipeline_id,
            }))?;
        if self.port(input.port_id).is_none() {
            return Err(ExposePortError::UnknownPort(PortNotFound {
                port_id: input.port_id,
            }));
        }
        let capacity =
            bounded(input.capacity).ok_or(ExposePortError::Unbounded(UnboundedEdge {
                submitted: input.capacity,
            }))?;
        Self::declared(pipeline).map_err(ExposePortError::NotDeclared)?;
        Ok(self.insert_exposure(
            input.pipeline_id,
            input.port_id,
            input.name,
            capacity,
            input.overload,
        ))
    }

    /// Runs `inference.graph.DeclareFeedbackLoop`: the edge is a feedback edge of the pipeline,
    /// bounded by the iteration rule the input carries. An edge of another pipeline is not refused
    /// here; such a loop bounds no cycle of the pipeline it names (check 5).
    ///
    /// # Errors
    ///
    /// In this order: [`DeclareFeedbackLoopError::UnknownPipeline`];
    /// [`DeclareFeedbackLoopError::UnknownEdge`] when no edge carries `edge_id`;
    /// [`DeclareFeedbackLoopError::Unbounded`] carrying the maximum sent when it is below one;
    /// [`DeclareFeedbackLoopError::NotDeclared`].
    pub fn declare_feedback_loop(
        &mut self,
        input: DeclareFeedbackLoop,
    ) -> Result<FeedbackLoopDeclared, DeclareFeedbackLoopError> {
        let pipeline =
            self.pipeline(input.pipeline_id)
                .ok_or(DeclareFeedbackLoopError::UnknownPipeline(
                    PipelineNotFound {
                        pipeline_id: input.pipeline_id,
                    },
                ))?;
        if !self.edges.iter().any(|edge| edge.edge_id == input.edge_id) {
            return Err(DeclareFeedbackLoopError::UnknownEdge(EdgeNotFound {
                edge_id: input.edge_id,
            }));
        }
        let max_iterations = bounded(input.max_iterations).ok_or(
            DeclareFeedbackLoopError::Unbounded(UnboundedFeedback {
                submitted: input.max_iterations,
            }),
        )?;
        Self::declared(pipeline).map_err(DeclareFeedbackLoopError::NotDeclared)?;
        Ok(self.insert_feedback_loop(input.pipeline_id, input.edge_id, max_iterations))
    }

    /// Runs `inference.graph.BindNode`. A node of another pipeline is not refused here.
    ///
    /// # Errors
    ///
    /// In this order: [`BindNodeError::UnknownPipeline`]; [`BindNodeError::UnknownNode`];
    /// [`BindNodeError::UnknownAdapter`] when `catalog` holds no adapter carrying `adapter_id`;
    /// [`BindNodeError::NotDeclared`].
    pub fn bind_node(
        &mut self,
        input: BindNode,
        catalog: &(impl Catalog + ?Sized),
    ) -> Result<NodeBound, BindNodeError> {
        let pipeline = self
            .pipeline(input.pipeline_id)
            .ok_or(BindNodeError::UnknownPipeline(PipelineNotFound {
                pipeline_id: input.pipeline_id,
            }))?;
        if self.node(input.node_id).is_none() {
            return Err(BindNodeError::UnknownNode(NodeNotFound {
                node_id: input.node_id,
            }));
        }
        if catalog.adapter(input.adapter_id).is_none() {
            return Err(BindNodeError::UnknownAdapter(AdapterNotFound {
                adapter_id: input.adapter_id,
            }));
        }
        Self::declared(pipeline).map_err(BindNodeError::NotDeclared)?;
        Ok(self.insert_binding(input.pipeline_id, input.node_id, input.adapter_id))
    }

    /// Runs `inference.graph.ValidatePipeline`: checks 1, 2, 3, 5, 6 and 9 of §10.2 over the
    /// pipeline's declaration ([`Declaration::findings`]), and the pipeline moves from Declared to
    /// Validated only when none finds anything.
    ///
    /// # Errors
    ///
    /// In this order: [`ValidatePipelineError::UnknownPipeline`];
    /// [`ValidatePipelineError::WrongState`] for a pipeline that is not Declared;
    /// [`ValidatePipelineError::Rejected`] carrying every finding.
    pub fn validate_pipeline(
        &mut self,
        input: ValidatePipeline,
    ) -> Result<PipelineValidated, ValidatePipelineError> {
        let pipeline_id = input.pipeline_id;
        let pipeline = self
            .pipeline(pipeline_id)
            .ok_or(ValidatePipelineError::UnknownPipeline(PipelineNotFound {
                pipeline_id,
            }))?;
        if pipeline.state != PipelineState::Declared {
            return Err(ValidatePipelineError::WrongState(PipelineStateConflict {
                current: pipeline.state,
            }));
        }
        let findings = self.findings(pipeline_id);
        if !findings.is_empty() {
            return Err(ValidatePipelineError::Rejected(IncompatibleGraph {
                findings,
            }));
        }
        if let Some(pipeline) = self
            .pipelines
            .iter_mut()
            .find(|pipeline| pipeline.pipeline_id == pipeline_id)
        {
            pipeline.state = PipelineState::Validated;
        }
        Ok(PipelineValidated { pipeline_id })
    }

    /// What checks 1, 2, 3 and 9, 5 and 6 find over the declaration of the pipeline carrying
    /// `pipeline_id`, in that order and each in declaration order; empty when every check passes
    /// or no pipeline carries it. A finding on an edge names the edge's source port and its node.
    #[must_use]
    pub fn findings(&self, pipeline_id: PipelineId) -> Vec<GraphFinding> {
        crate::validation::findings(self, pipeline_id)
    }

    /// The pipeline carrying `pipeline_id`, if one was declared.
    #[must_use]
    pub fn pipeline(&self, pipeline_id: PipelineId) -> Option<&Pipeline> {
        self.pipelines
            .iter()
            .find(|pipeline| pipeline.pipeline_id == pipeline_id)
    }

    /// The node carrying `node_id`, if one was added.
    #[must_use]
    pub fn node(&self, node_id: NodeId) -> Option<&Node> {
        self.nodes.iter().find(|node| node.node_id == node_id)
    }

    /// The port carrying `port_id`, if one was added.
    #[must_use]
    pub fn port(&self, port_id: PortId) -> Option<&Port> {
        self.ports.iter().find(|port| port.port_id == port_id)
    }

    /// Every declared pipeline, in declaration order.
    pub fn pipelines(&self) -> impl Iterator<Item = &Pipeline> {
        self.pipelines.iter()
    }

    /// Every added node, in declaration order.
    pub fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.iter()
    }

    /// Every added port, in declaration order.
    pub fn ports(&self) -> impl Iterator<Item = &Port> {
        self.ports.iter()
    }

    /// Every edge, in declaration order.
    pub fn edges(&self) -> impl Iterator<Item = &Edge> {
        self.edges.iter()
    }

    /// Every exposure, in declaration order.
    pub fn exposures(&self) -> impl Iterator<Item = &Exposure> {
        self.exposures.iter()
    }

    /// The exposure carrying `exposure_id`, if one was declared.
    #[must_use]
    pub fn exposure(&self, exposure_id: ExposureId) -> Option<&Exposure> {
        self.exposures
            .iter()
            .find(|exposure| exposure.exposure_id == exposure_id)
    }

    /// The egresses of the child of the subgraph node `node_id` that the node forwards on its
    /// structured output port [`USAGE_PORT`](crate::USAGE_PORT) besides the egress of that name,
    /// which crosses to it by name as any exposure does: in the child's exposure order, every
    /// egress of the child exposing a structured output port named `usage` whose name no port of
    /// the node carries (the usage roll-up of `inference.graph.PlanPipeline`). Planning holds them
    /// matched by that port, and the runtime forwards their usage records on it, so the usage of
    /// every model call of the child reaches the parent, each record once. None for a node that is
    /// not a subgraph node or has no such port.
    #[must_use]
    pub fn forwarded_usage(&self, node_id: NodeId) -> Vec<&Exposure> {
        let Some((node, child)) = self
            .node(node_id)
            .and_then(|node| node.subgraph_id().map(|child| (node, child)))
        else {
            return Vec::new();
        };
        let ports: Vec<&Port> = self
            .ports
            .iter()
            .filter(|port| port.node_id == node_id && port.pipeline_id == node.pipeline_id())
            .collect();
        if !ports.iter().any(|port| carries_usage(port)) {
            return Vec::new();
        }
        self.exposures
            .iter()
            .filter(|exposure| exposure.pipeline_id == child && self.is_usage_egress(exposure))
            .filter(|exposure| ports.iter().all(|port| port.name != exposure.name))
            .collect()
    }

    /// Whether `exposure` is a usage egress: the exposure of a structured output port named
    /// [`USAGE_PORT`](crate::USAGE_PORT), on which a node sends the usage records of its calls,
    /// whatever name the exposure carries.
    #[must_use]
    pub fn is_usage_egress(&self, exposure: &Exposure) -> bool {
        self.port(exposure.port_id).is_some_and(carries_usage)
    }

    /// Every feedback loop, in declaration order.
    pub fn feedback_loops(&self) -> impl Iterator<Item = &FeedbackLoop> {
        self.feedback_loops.iter()
    }

    /// Every binding, in declaration order.
    pub fn bindings(&self) -> impl Iterator<Item = &Binding> {
        self.bindings.iter()
    }

    /// Refuses a pipeline that is not Declared: a declaration changes only while it is.
    fn declared(pipeline: &Pipeline) -> Result<(), PipelineNotReady> {
        if pipeline.state == PipelineState::Declared {
            Ok(())
        } else {
            Err(PipelineNotReady {
                pipeline_id: pipeline.pipeline_id,
                required: PipelineState::Declared,
            })
        }
    }

    /// The pipeline carrying `pipeline_id`, to move it.
    pub(crate) fn pipeline_mut(&mut self, pipeline_id: PipelineId) -> Option<&mut Pipeline> {
        self.pipelines
            .iter_mut()
            .find(|pipeline| pipeline.pipeline_id == pipeline_id)
    }

    /// The next identity; only a command that creates takes one.
    fn issue(&mut self) -> Uuid {
        match &mut self.identities {
            Identities::Generated => Uuid::new_v4(),
            Identities::Counted { space, issued } => {
                *issued += 1;
                Uuid::from_u128(
                    (u128::from(*space) << 96)
                        | 0x0000_0000_0000_4000_8000_0000_0000_0000
                        | *issued,
                )
            }
        }
    }

    pub(crate) fn insert_pipeline(&mut self, input: DeclarePipeline) -> PipelineDeclared {
        let pipeline_id = PipelineId::from(self.issue());
        self.pipelines.push(Pipeline {
            pipeline_id,
            name: input.name.clone(),
            schema_revision: input.schema_revision,
            mode: input.mode,
            profile_id: input.profile_id,
            required_capabilities: input.required_capabilities,
            effective_capabilities: None,
            downgrades: None,
            state: PipelineState::Declared,
        });
        PipelineDeclared {
            pipeline_id,
            name: input.name,
        }
    }

    pub(crate) fn insert_node(&mut self, input: AddNode) -> NodeAdded {
        let node_id = NodeId::from(self.issue());
        self.nodes.push(Node {
            node_id,
            pipeline_id: input.pipeline_id,
            name: input.name.clone(),
            role: input.role,
            requires: input.requires,
            subgraph_id: None,
        });
        NodeAdded {
            node_id,
            pipeline_id: input.pipeline_id,
            name: input.name,
        }
    }

    pub(crate) fn insert_subgraph_node(
        &mut self,
        pipeline_id: PipelineId,
        name: String,
        role: crate::NodeRole,
        subgraph_id: PipelineId,
    ) -> SubgraphNodeAdded {
        let node_id = NodeId::from(self.issue());
        self.nodes.push(Node {
            node_id,
            pipeline_id,
            name: name.clone(),
            role,
            requires: Vec::new(),
            subgraph_id: Some(subgraph_id),
        });
        SubgraphNodeAdded {
            node_id,
            pipeline_id,
            subgraph_id,
            name,
        }
    }

    pub(crate) fn insert_port(&mut self, input: AddPort) -> PortAdded {
        let port_id = PortId::from(self.issue());
        self.ports.push(Port {
            port_id,
            pipeline_id: input.pipeline_id,
            node_id: input.node_id,
            name: input.name.clone(),
            direction: input.direction,
            modality: input.modality,
            essentiality: input.essentiality,
            delivery: input.delivery,
            incremental: input.incremental,
            end_marker_required: input.end_marker_required,
            provisional_allowed: input.provisional_allowed,
        });
        PortAdded {
            port_id,
            node_id: input.node_id,
            name: input.name,
        }
    }

    pub(crate) fn insert_edge(
        &mut self,
        pipeline_id: PipelineId,
        from_port: PortId,
        to_port: PortId,
        capacity: NonZeroUsize,
        overload: OverloadPolicy,
    ) -> PortsConnected {
        let edge_id = EdgeId::from(self.issue());
        self.edges.push(Edge {
            edge_id,
            pipeline_id,
            from_port,
            to_port,
            capacity,
            overload,
        });
        PortsConnected {
            edge_id,
            from_port,
            to_port,
        }
    }

    pub(crate) fn insert_exposure(
        &mut self,
        pipeline_id: PipelineId,
        port_id: PortId,
        name: String,
        capacity: NonZeroUsize,
        overload: OverloadPolicy,
    ) -> PortExposed {
        let exposure_id = ExposureId::from(self.issue());
        self.exposures.push(Exposure {
            exposure_id,
            pipeline_id,
            port_id,
            name: name.clone(),
            capacity,
            overload,
        });
        PortExposed {
            exposure_id,
            pipeline_id,
            port_id,
            name,
        }
    }

    pub(crate) fn insert_feedback_loop(
        &mut self,
        pipeline_id: PipelineId,
        edge_id: EdgeId,
        max_iterations: NonZeroUsize,
    ) -> FeedbackLoopDeclared {
        let feedback_id = FeedbackLoopId::from(self.issue());
        self.feedback_loops.push(FeedbackLoop {
            feedback_id,
            pipeline_id,
            edge_id,
            max_iterations,
        });
        FeedbackLoopDeclared {
            feedback_id,
            edge_id,
            max_iterations: i64::try_from(max_iterations.get()).unwrap_or(i64::MAX),
        }
    }

    pub(crate) fn insert_binding(
        &mut self,
        pipeline_id: PipelineId,
        node_id: NodeId,
        adapter_id: AdapterId,
    ) -> NodeBound {
        let binding_id = BindingId::from(self.issue());
        self.bindings.push(Binding {
            binding_id,
            pipeline_id,
            node_id,
            adapter_id,
        });
        NodeBound {
            binding_id,
            node_id,
            adapter_id,
        }
    }
}

/// The capacity an ESS `Integer` gives an edge, or the maximum it gives a feedback loop: none
/// below one. On a target whose `usize` is narrower than `i64`, a value beyond `usize::MAX` is
/// held as `usize::MAX`.
fn bounded(capacity: i64) -> Option<NonZeroUsize> {
    if capacity < 1 {
        return None;
    }
    Some(
        usize::try_from(capacity).map_or(NonZeroUsize::MAX, |capacity| {
            NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::MIN)
        }),
    )
}
