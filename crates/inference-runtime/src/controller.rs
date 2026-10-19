//! The run controller: one [`Run`] of a planned pipeline and the [`NodeInstance`] of each of its
//! nodes, driven together through the node implementations ([`NodeImplementation`]) the caller
//! supplies (`docs/design/inference.md` lines 655 and 673).
//!
//! Each run transition drives the instances, as the command summaries of
//! `ess/domains/graph.yaml` read:
//!
//! | run transition | what each instance does |
//! |---|---|
//! | [`RunController::prepare`] | described, then prepared |
//! | [`RunController::activate`] | started with its edge endpoints |
//! | [`RunController::drain`] | drained |
//! | [`RunController::complete`] | stopped |
//! | [`RunController::cancel`], [`RunController::fail`] | cancelled, then stopped, unless terminal |
//!
//! A node whose prepare, start or drain returns a [`NodeError`], or whose exchange the caller
//! reports failed ([`RunController::fail_node`]), moves its instance to Failed and fails the run
//! with a [`FailureReason`] naming the node (`node_id`) and the error (`detail`); every other
//! instance that is not terminal is then cancelled and stopped (line 1006). A failed instance is
//! called no further.
//!
//! Every step checks the run and every instance before it moves anything: a step whose run is
//! in another state answers [`RunError::Conflict`], one with an instance outside the step's
//! source states answers [`RunError::Instance`] naming the node and the state it holds, and
//! nothing moves. An instance moves only once its node has answered the call. No step panics.
//!
//! The controller builds one [`EdgeQueue`] per edge of the pipeline when it is created and hands
//! each node, when it starts, the sender side of every edge leaving one of its ports and the
//! receiver side of every edge arriving at one. Its public methods are `async` over the boxed
//! futures of the node trait and commit no caller to one asynchronous executor (line 665).
//!
//! Admission is not decided here: `PrepareRun`'s `refused` outcome, which leaves the run
//! Validating with no instance, is the caller's to take before it calls
//! [`RunController::prepare`].

use std::error::Error;
use std::fmt;
use std::sync::Arc;

use inference_core::id::NodeId;
use inference_pipeline::{
    Declaration, Node, NodeEndpoints, NodeError, NodeImplementation, NodeInput, NodeNotFound,
    NodeOutput, PipelineNotFound, PlannedPipeline,
};

use crate::queue::{EdgeQueue, EpochLookup, UnsupportedOverload};
use crate::run::{
    FailureReason, NodeInstance, NodeInstanceId, NodeInstanceState, NodeInstanceStateConflict, Run,
    RunStateConflict,
};

/// An instance of the run is not in a state the step acts from, so nothing moved. `current` is
/// the state it holds, none when the node has no instance yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstanceConflict {
    pub node_id: NodeId,
    pub instance_id: Option<NodeInstanceId>,
    pub current: Option<NodeInstanceState>,
}

impl InstanceConflict {
    fn of(node_id: NodeId, instance: Option<&NodeInstance>) -> Self {
        Self {
            node_id,
            instance_id: instance.map(NodeInstance::instance_id),
            current: instance.map(NodeInstance::state),
        }
    }
}

impl fmt::Display for InstanceConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.instance_id, self.current) {
            (Some(instance_id), Some(current)) => write!(
                f,
                "the instance {instance_id} of node {} is {current:?}, a state this step does \
                 not act from, so nothing moved",
                self.node_id
            ),
            _ => write!(
                f,
                "node {} has no instance yet, so nothing moved",
                self.node_id
            ),
        }
    }
}

impl Error for InstanceConflict {}

/// Why a run transition driven through [`RunController`] did not end where it was asked to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// The run is in a state the transition does not leave; nothing moved.
    Conflict(RunStateConflict),
    /// An instance is in a state the step does not act from; nothing moved.
    Instance(InstanceConflict),
    /// The node's endpoints went with a start its caller abandoned, so the run cannot activate;
    /// nothing moved. Cancel or fail the run.
    EndpointsTaken(NodeId),
    /// A node returned an error: its instance is Failed, and so is the run, with this reason.
    NodeFailed(FailureReason),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict(conflict) => fmt::Display::fmt(conflict, f),
            Self::Instance(conflict) => fmt::Display::fmt(conflict, f),
            Self::EndpointsTaken(node_id) => write!(
                f,
                "the endpoints of node {node_id} went with an abandoned start, so nothing moved"
            ),
            Self::NodeFailed(failure) => match failure.node_id {
                Some(node_id) => write!(f, "node {node_id} failed: {}", failure.detail),
                None => write!(f, "the run failed: {}", failure.detail),
            },
        }
    }
}

impl Error for RunError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Conflict(conflict) => Some(conflict),
            Self::Instance(conflict) => Some(conflict),
            Self::EndpointsTaken(_) | Self::NodeFailed(_) => None,
        }
    }
}

/// Why [`RunController::create`] built no controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateControllerError {
    /// The declaration holds no pipeline carrying the planned pipeline's identity.
    UnknownPipeline(PipelineNotFound),
    /// An edge of the pipeline declares an overload policy no edge queue implements.
    UnsupportedOverload(UnsupportedOverload),
}

impl fmt::Display for CreateControllerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownPipeline(error) => fmt::Display::fmt(error, f),
            Self::UnsupportedOverload(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for CreateControllerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::UnknownPipeline(error) => Some(error),
            Self::UnsupportedOverload(error) => Some(error),
        }
    }
}

/// Why [`RunController::fail_node`] failed nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailNodeError {
    /// The run has no node carrying the identity, or none described yet.
    UnknownNode(NodeNotFound),
    /// The node's instance is Cancelling or terminal, so it cannot fail; or, while the others
    /// wound down, one was in a state that does not stop.
    Instance(InstanceConflict),
    /// The run is Created or terminal, so it cannot fail.
    Run(RunStateConflict),
}

impl fmt::Display for FailNodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownNode(error) => fmt::Display::fmt(error, f),
            Self::Instance(error) => fmt::Display::fmt(error, f),
            Self::Run(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for FailNodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::UnknownNode(error) => Some(error),
            Self::Instance(error) => Some(error),
            Self::Run(error) => Some(error),
        }
    }
}

/// One node of the run: its declaration, its implementation, its instance once described, and
/// the endpoints it is handed when it starts.
struct Driven<P> {
    node: Node,
    implementation: Box<dyn NodeImplementation<P>>,
    instance: Option<NodeInstance>,
    endpoints: Option<NodeEndpoints<P>>,
}

impl<P> Driven<P> {
    /// The instance's state; none before it is described.
    fn state(&self) -> Option<NodeInstanceState> {
        self.instance.as_ref().map(NodeInstance::state)
    }

    /// The conflict this node's instance makes with a step that does not act from its state.
    fn conflict(&self) -> InstanceConflict {
        InstanceConflict::of(self.node.node_id(), self.instance.as_ref())
    }

    /// The instance moved by `transition`, or the conflict naming it.
    fn moved<E>(
        &self,
        transition: impl FnOnce(&NodeInstance) -> Result<(NodeInstance, E), NodeInstanceStateConflict>,
    ) -> Result<NodeInstance, InstanceConflict> {
        self.instance
            .as_ref()
            .ok_or_else(|| self.conflict())
            .and_then(|instance| {
                transition(instance)
                    .map(|(moved, _)| moved)
                    .map_err(|_| self.conflict())
            })
    }
}

/// One run of a planned pipeline and the instances of its nodes, carrying envelopes of payload
/// `P` along its edges.
///
/// # Abandoned steps
///
/// A caller may drop a step's future before it completes (a timeout, a `select!`). Each step
/// checks before it moves anything, and an instance moves only once its node has answered, so an
/// abandoned step leaves no later step to panic or to answer with the run ahead of its instances:
/// a later step either finds every instance where it acts from or answers a refusal and moves
/// nothing, and [`RunController::cancel`] and [`RunController::fail`] wind down whatever state an
/// abandoned step left. Where the order allows, a step commits the run's own transition after its
/// instances moved, so an abandoned step leaves the run where it was:
///
/// | step | abandoned mid-way, the run | why |
/// |---|---|---|
/// | [`activate`](RunController::activate) | stays Preparing | |
/// | [`complete`](RunController::complete) | stays Draining; calling it again finishes it | |
/// | [`cancel`](RunController::cancel), [`fail`](RunController::fail) | stays where it was; calling it again finishes it | |
/// | [`prepare`](RunController::prepare) | is Preparing, its instances Described, Prepared or not yet described | `DescribeNodeInstance` admits only a run that is Preparing or Active, so the run moves before its first instance exists |
/// | [`drain`](RunController::drain) | is Draining, its instances Started or Draining | `DrainRun` closes the run to new input before its nodes complete their in-flight events, so the run moves before its first instance drains |
/// | a node failure, in `prepare`, `activate`, `drain` or [`fail_node`](RunController::fail_node) | stays where it was, with the failed instance Failed and the reason not recorded on the run | the instance fails as its node answers, and the run fails only once the others are wound down |
///
/// After an abandoned `activate`, a node whose start was under way has lost its endpoints with
/// the dropped future, so `activate` answers [`RunError::EndpointsTaken`] from then on; after an
/// abandoned `prepare` or `drain`, the next forward step answers [`RunError::Instance`]. In each
/// of those cases the run is to be cancelled or failed.
pub struct RunController<P> {
    run: Run,
    nodes: Vec<Driven<P>>,
}

impl<P: Send + 'static> RunController<P> {
    /// Creates the run of `planned` (`inference.graph.CreateRun`), takes the implementation of
    /// each of its nodes from `implement`, in declaration order, and builds the queue of each of
    /// its edges, which read epochs through `epochs`. `declaration` is the one that planned
    /// `planned`.
    ///
    /// # Errors
    ///
    /// [`CreateControllerError::UnknownPipeline`] when `declaration` holds no pipeline carrying
    /// the planned identity, [`CreateControllerError::UnsupportedOverload`] when an edge's policy
    /// is `cancel` or `supersede`. No run is created.
    pub fn create(
        planned: &PlannedPipeline,
        declaration: &Declaration,
        epochs: impl EpochLookup + 'static,
        mut implement: impl FnMut(&Node) -> Box<dyn NodeImplementation<P>>,
    ) -> Result<Self, CreateControllerError> {
        let pipeline_id = planned.pipeline_id();
        if declaration.pipeline(pipeline_id).is_none() {
            return Err(CreateControllerError::UnknownPipeline(PipelineNotFound {
                pipeline_id,
            }));
        }
        let epochs: Arc<dyn EpochLookup> = Arc::new(epochs);
        let mut queues = Vec::new();
        for edge in declaration
            .edges()
            .filter(|edge| edge.pipeline_id() == pipeline_id)
        {
            let queue = EdgeQueue::new(edge, Arc::clone(&epochs))
                .map_err(CreateControllerError::UnsupportedOverload)?;
            let from = declaration
                .port(edge.from_port())
                .map(|port| port.node_id());
            let to = declaration.port(edge.to_port()).map(|port| port.node_id());
            queues.push((edge.from_port(), from, edge.to_port(), to, queue));
        }
        let mut nodes: Vec<Driven<P>> = declaration
            .nodes()
            .filter(|node| node.pipeline_id() == pipeline_id)
            .map(|node| Driven {
                node: node.clone(),
                implementation: implement(node),
                instance: None,
                endpoints: Some(NodeEndpoints::default()),
            })
            .collect();
        for (from_port, from, to_port, to, queue) in queues {
            let (sender, receiver) = queue.split();
            if let Some(endpoints) = Self::endpoints_of(&mut nodes, from) {
                endpoints.outputs.push(NodeOutput {
                    port_id: from_port,
                    endpoint: Box::new(sender),
                });
            }
            if let Some(endpoints) = Self::endpoints_of(&mut nodes, to) {
                endpoints.inputs.push(NodeInput {
                    port_id: to_port,
                    endpoint: Box::new(receiver),
                });
            }
        }
        let (run, _) = Run::create(planned);
        Ok(Self { run, nodes })
    }

    /// Runs `inference.graph.ValidateRun`: Created to Validating. No instance exists yet.
    ///
    /// # Errors
    ///
    /// [`RunStateConflict`] when the run is not Created; nothing moves.
    pub fn validate(&mut self) -> Result<(), RunStateConflict> {
        self.run = self.run.validate()?.0;
        Ok(())
    }

    /// Runs `inference.graph.PrepareRun`: Validating to Preparing, then describes an instance of
    /// each node and prepares each in turn. The run moves first; see [`RunController`] on an
    /// abandoned prepare.
    ///
    /// # Errors
    ///
    /// [`RunError::Conflict`] when the run is not Validating, [`RunError::Instance`] when a node
    /// already has an instance; nothing moves. [`RunError::NodeFailed`] when a node's prepare
    /// returned an error: the run is Failed.
    pub async fn prepare(&mut self) -> Result<(), RunError> {
        let preparing = self.run.prepare().map_err(RunError::Conflict)?.0;
        if let Some(described) = self.nodes.iter().find(|driven| driven.instance.is_some()) {
            return Err(RunError::Instance(described.conflict()));
        }
        self.run = preparing;
        for index in 0..self.nodes.len() {
            let driven = &mut self.nodes[index];
            driven.implementation.describe(&driven.node).await;
            let (instance, _) =
                NodeInstance::describe(&self.run, driven.node.node_id()).map_err(|_| {
                    RunError::Conflict(RunStateConflict {
                        current: self.run.state(),
                    })
                })?;
            self.nodes[index].instance = Some(instance);
        }
        for index in 0..self.nodes.len() {
            let answer = self.nodes[index].implementation.prepare().await;
            self.answered(index, answer, NodeInstance::prepare).await?;
        }
        Ok(())
    }

    /// Runs `inference.graph.ActivateRun`: starts each instance with its edge endpoints, then
    /// moves the run from Preparing to Active.
    ///
    /// # Errors
    ///
    /// [`RunError::Conflict`] when the run is not Preparing, [`RunError::Instance`] when an
    /// instance is not Prepared, [`RunError::EndpointsTaken`] when a node lost its endpoints to an
    /// abandoned start; nothing moves. [`RunError::NodeFailed`] when a node's start returned an
    /// error: the run is Failed.
    pub async fn activate(&mut self) -> Result<(), RunError> {
        let active = self.run.activate().map_err(RunError::Conflict)?.0;
        self.require(&[NodeInstanceState::Prepared])?;
        if let Some(taken) = self.nodes.iter().find(|driven| driven.endpoints.is_none()) {
            return Err(RunError::EndpointsTaken(taken.node.node_id()));
        }
        for index in 0..self.nodes.len() {
            let driven = &mut self.nodes[index];
            let endpoints = driven.endpoints.take().unwrap_or_default();
            let answer = driven.implementation.start(endpoints).await;
            self.answered(index, answer, NodeInstance::start).await?;
        }
        self.run = active;
        Ok(())
    }

    /// Runs `inference.graph.DrainRun`: Active to Draining, then drains each instance. The run
    /// moves first; see [`RunController`] on an abandoned drain.
    ///
    /// # Errors
    ///
    /// [`RunError::Conflict`] when the run is not Active, [`RunError::Instance`] when an instance
    /// is not Started; nothing moves. [`RunError::NodeFailed`] when a node's drain returned an
    /// error: the run is Failed.
    pub async fn drain(&mut self) -> Result<(), RunError> {
        let draining = self.run.drain().map_err(RunError::Conflict)?.0;
        self.require(&[NodeInstanceState::Started])?;
        self.run = draining;
        for index in 0..self.nodes.len() {
            let answer = self.nodes[index].implementation.drain().await;
            self.answered(index, answer, NodeInstance::drain).await?;
        }
        Ok(())
    }

    /// Runs `inference.graph.CompleteRun`: stops each Draining instance, then moves the run from
    /// Draining to Completed. An abandoned complete is finished by calling it again.
    ///
    /// # Errors
    ///
    /// [`RunError::Conflict`] when the run is not Draining, [`RunError::Instance`] when an
    /// instance is neither Draining nor Stopped; nothing moves.
    pub async fn complete(&mut self) -> Result<(), RunError> {
        let completed = self.run.complete().map_err(RunError::Conflict)?.0;
        self.require(&[NodeInstanceState::Draining, NodeInstanceState::Stopped])?;
        for index in 0..self.nodes.len() {
            if self.nodes[index].state() == Some(NodeInstanceState::Draining) {
                self.stop(index).await.map_err(RunError::Instance)?;
            }
        }
        self.run = completed;
        Ok(())
    }

    /// Runs `inference.graph.CancelRun`: cancels and stops each instance that is not terminal,
    /// then moves the run from any state but a terminal one to Cancelled. An abandoned cancel is
    /// finished by calling it again.
    ///
    /// # Errors
    ///
    /// [`RunError::Conflict`] when the run is terminal; nothing moves.
    pub async fn cancel(&mut self) -> Result<(), RunError> {
        let cancelled = self.run.cancel().map_err(RunError::Conflict)?.0;
        self.wind_down().await.map_err(RunError::Instance)?;
        self.run = cancelled;
        Ok(())
    }

    /// Runs `inference.graph.FailRun` with `failure`: cancels and stops each instance that is not
    /// terminal, then moves the run from Validating, Preparing, Active or Draining to Failed. An
    /// abandoned fail is finished by calling it again.
    ///
    /// # Errors
    ///
    /// [`RunError::Conflict`] when the run is Created or terminal; nothing moves.
    pub async fn fail(&mut self, failure: FailureReason) -> Result<(), RunError> {
        let failed = self.run.fail(failure).map_err(RunError::Conflict)?.0;
        self.wind_down().await.map_err(RunError::Instance)?;
        self.run = failed;
        Ok(())
    }

    /// Reports that the node carrying `node_id` failed in exchange with `error`: its instance
    /// moves to Failed, every other instance that is not terminal is cancelled and stopped, and
    /// the run fails with a reason naming the node and the error.
    ///
    /// # Errors
    ///
    /// [`FailNodeError::UnknownNode`] when no described instance belongs to `node_id`,
    /// [`FailNodeError::Instance`] when it is Cancelling or terminal, [`FailNodeError::Run`] when
    /// the run is Created or terminal. Nothing moves.
    pub async fn fail_node(
        &mut self,
        node_id: NodeId,
        error: &NodeError,
    ) -> Result<(), FailNodeError> {
        let index = self
            .nodes
            .iter()
            .position(|driven| driven.node.node_id() == node_id && driven.instance.is_some())
            .ok_or(FailNodeError::UnknownNode(NodeNotFound { node_id }))?;
        let failure = Self::failure(node_id, error);
        let instance = self.nodes[index]
            .moved(|instance| instance.fail(error.detail()))
            .map_err(FailNodeError::Instance)?;
        let failed = self.run.fail(failure).map_err(FailNodeError::Run)?.0;
        self.nodes[index].instance = Some(instance);
        self.wind_down().await.map_err(FailNodeError::Instance)?;
        self.run = failed;
        Ok(())
    }

    /// The run.
    #[must_use]
    pub const fn run(&self) -> &Run {
        &self.run
    }

    /// The instance of the node carrying `node_id`, once described.
    #[must_use]
    pub fn instance(&self, node_id: NodeId) -> Option<&NodeInstance> {
        self.nodes
            .iter()
            .find(|driven| driven.node.node_id() == node_id)
            .and_then(|driven| driven.instance.as_ref())
    }

    /// Every described instance, in the declaration order of the nodes.
    pub fn instances(&self) -> impl Iterator<Item = &NodeInstance> {
        self.nodes
            .iter()
            .filter_map(|driven| driven.instance.as_ref())
    }

    /// Nothing, when every node has an instance in one of `from`; otherwise the conflict naming
    /// the first that has not.
    fn require(&self, from: &[NodeInstanceState]) -> Result<(), RunError> {
        match self
            .nodes
            .iter()
            .find(|driven| !driven.state().is_some_and(|state| from.contains(&state)))
        {
            Some(driven) => Err(RunError::Instance(driven.conflict())),
            None => Ok(()),
        }
    }

    /// Takes the answer of the node at `index` to a fallible step: moves its instance by
    /// `transition` when it is `Ok`; otherwise fails the instance, winds the others down and then
    /// fails the run.
    async fn answered<E>(
        &mut self,
        index: usize,
        answer: Result<(), NodeError>,
        transition: fn(&NodeInstance) -> Result<(NodeInstance, E), NodeInstanceStateConflict>,
    ) -> Result<(), RunError> {
        match answer {
            Ok(()) => {
                let moved = self.nodes[index]
                    .moved(transition)
                    .map_err(RunError::Instance)?;
                self.nodes[index].instance = Some(moved);
                Ok(())
            }
            Err(error) => {
                let failure = Self::failure(self.nodes[index].node.node_id(), &error);
                let instance = self.nodes[index]
                    .moved(|instance| instance.fail(error.detail()))
                    .map_err(RunError::Instance)?;
                let failed = self
                    .run
                    .fail(failure.clone())
                    .map_err(RunError::Conflict)?
                    .0;
                self.nodes[index].instance = Some(instance);
                self.wind_down().await.map_err(RunError::Instance)?;
                self.run = failed;
                Err(RunError::NodeFailed(failure))
            }
        }
    }

    /// Cancels and stops each instance that is not terminal, in declaration order; an instance
    /// already Cancelling is only stopped.
    async fn wind_down(&mut self) -> Result<(), InstanceConflict> {
        for index in 0..self.nodes.len() {
            let Some(state) = self.nodes[index].state() else {
                continue;
            };
            if state.is_terminal() {
                continue;
            }
            if state != NodeInstanceState::Cancelling {
                let cancelling = self.nodes[index].moved(NodeInstance::cancel)?;
                self.nodes[index].implementation.cancel().await;
                self.nodes[index].instance = Some(cancelling);
            }
            self.stop(index).await?;
        }
        Ok(())
    }

    /// Stops the instance of the node at `index` once its node has stopped.
    ///
    /// # Errors
    ///
    /// The conflict naming the instance when it is in a state that does not stop; nothing moves
    /// and the node is not called.
    async fn stop(&mut self, index: usize) -> Result<(), InstanceConflict> {
        let stopped = self.nodes[index].moved(NodeInstance::stop)?;
        self.nodes[index].implementation.stop().await;
        let driven = &mut self.nodes[index];
        driven.instance = Some(stopped);
        driven.endpoints = None;
        Ok(())
    }

    /// The endpoints not yet handed to the node carrying `node_id`, if one of `nodes` does.
    fn endpoints_of(
        nodes: &mut [Driven<P>],
        node_id: Option<NodeId>,
    ) -> Option<&mut NodeEndpoints<P>> {
        nodes
            .iter_mut()
            .find(|driven| Some(driven.node.node_id()) == node_id)
            .and_then(|driven| driven.endpoints.as_mut())
    }

    /// The reason a run fails with when the node carrying `node_id` returned `error`.
    fn failure(node_id: NodeId, error: &NodeError) -> FailureReason {
        FailureReason {
            node_id: Some(node_id),
            detail: error.detail().to_owned(),
        }
    }
}

impl<P> fmt::Debug for RunController<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunController")
            .field("run", &self.run)
            .field(
                "instances",
                &self.nodes.iter().map(Driven::state).collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}
