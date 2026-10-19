//! The `inference.graph` domain of `ess/domains/graph.yaml`, held in memory: the declaration, its
//! validation and planning, and the runs of planned pipelines with their node instances.
//!
//! [`GraphDomain`] holds one [`Declaration`] of `inference-pipeline` with identities counted in its
//! own [`IDENTITY_SPACE`], so that two runs of the same commands report the same identities and no
//! other domain issues one of them, and records every event its commands emit in order. The
//! declaration commands are the declaration's own, `ValidatePipeline` and `PlanPipeline` among
//! them; what a declaration references and does not hold, the declared profiles and adapters, it
//! reads through [`GraphCatalog`] over the [`PolicyDomain`] and [`ModelDomain`] stores. A refused
//! command stores nothing, records no event and takes no identity. The views
//! `inference.graph.Pipelines`, `Nodes`, `Ports`, `Edges` and `Bindings` are the declaration's
//! entities ([`GraphDomain::declaration`]), read at the call.
//!
//! A validation keeps the validated form of the pipeline ([`ValidatedPipeline`]) and a plan takes
//! it and keeps the Planned form ([`PlannedPipeline`]), so that `CreateRun` builds a [`Run`] of
//! `inference-runtime` from the Planned form, the only input [`Run::create_as`] takes; a pipeline
//! no record carries is refused with [`PipelineNotFound`] and one that is not Planned with
//! [`PipelineNotReady`] requiring Planned. The run commands move the run by its own transitions
//! and refuse a run no record carries with [`RunNotFound`]. `DescribeNodeInstance` refuses, in this
//! order, a run no record carries, a node no pipeline declares ([`NodeNotFound`]) and a run that is
//! neither Preparing nor Active ([`RunNotRunning`]); a node of another pipeline than the run's is
//! not refused, as the specification orders it. The six node instance commands, `PrepareNodeInstance`
//! through `FailNodeInstance`, move an instance by its own transitions and move no run, and a Run
//! command moves no instance: which run transition drives which instance is the run controller's
//! in `inference-runtime`. Behind the same feature, [`RecordingNode`] is the recording mock of the
//! node contract that controller drives. Runs and node instances take their identities
//! from a counter of their own in the same space, `{IDENTITY_SPACE:08x}-0001-4000-8000-000000000001`
//! first, so that no run or instance carries the identity of a declared entity. The views
//! `inference.graph.Runs` and `NodeInstances` are [`GraphDomain::runs`] and
//! [`GraphDomain::node_instances`], read at the call.
//!
//! What `CancelRun` does to the run's generations and segments is the `inference.stream` store's
//! ([`crate::stream::StreamDomain::cancel_run`]), which holds them.
//!
//! [`GraphDomain`]: crate::graph::GraphDomain
//! [`GraphDomain::declaration`]: crate::graph::GraphDomain::declaration
//! [`GraphDomain::runs`]: crate::graph::GraphDomain::runs
//! [`GraphDomain::node_instances`]: crate::graph::GraphDomain::node_instances
//! [`GraphCatalog`]: crate::graph::GraphCatalog
//! [`RecordingNode`]: crate::graph::RecordingNode
//! [`IDENTITY_SPACE`]: crate::graph::IDENTITY_SPACE
//! [`PolicyDomain`]: crate::policy::PolicyDomain
//! [`ModelDomain`]: crate::model::ModelDomain
//! [`Declaration`]: inference_pipeline::Declaration
//! [`ValidatedPipeline`]: inference_pipeline::ValidatedPipeline
//! [`PlannedPipeline`]: inference_pipeline::PlannedPipeline
//! [`PipelineNotFound`]: inference_pipeline::PipelineNotFound
//! [`PipelineNotReady`]: inference_pipeline::PipelineNotReady
//! [`NodeNotFound`]: inference_pipeline::NodeNotFound
//! [`Run`]: inference_runtime::run::Run
//! [`Run::create_as`]: inference_runtime::run::Run::create_as
//! [`RunNotFound`]: inference_runtime::run::RunNotFound
//! [`RunNotRunning`]: inference_runtime::run::RunNotRunning

use inference_core::model::{Adapter, AdapterId};
use inference_core::policy::{DeploymentProfile, ProfileId, ProfileNotFound};
#[cfg(feature = "run")]
use inference_pipeline::PlannedPipeline;
use inference_pipeline::{
    AddNode, AddNodeError, AddPort, AddPortError, BindNode, BindNodeError, Catalog, ConnectPorts,
    ConnectPortsError, Declaration, DeclarePipeline, NodeAdded, NodeBound, PipelineDeclared,
    PipelinePlanned, PipelineValidated, PlanPipeline, PlanPipelineError, PortAdded, PortsConnected,
    ValidatePipeline, ValidatePipelineError, ValidatedPipeline,
};
#[cfg(feature = "run")]
use inference_runtime::run::{
    NodeInstance, NodeInstanceCancelling, NodeInstanceDescribed, NodeInstanceDraining,
    NodeInstanceFailed, NodeInstancePrepared, NodeInstanceStarted, NodeInstanceStopped, Run,
    RunActivated, RunCancelled, RunCompleted, RunCreated, RunDraining, RunFailed, RunPreparing,
    RunValidating,
};

use crate::model::ModelDomain;
use crate::policy::PolicyDomain;

/// An event of `inference.graph`, as the domain emitted it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphEvent {
    /// `inference.graph.PipelineDeclared`.
    PipelineDeclared(PipelineDeclared),
    /// `inference.graph.NodeAdded`.
    NodeAdded(NodeAdded),
    /// `inference.graph.PortAdded`.
    PortAdded(PortAdded),
    /// `inference.graph.PortsConnected`.
    PortsConnected(PortsConnected),
    /// `inference.graph.NodeBound`.
    NodeBound(NodeBound),
    /// `inference.graph.PipelineValidated`.
    PipelineValidated(PipelineValidated),
    /// `inference.graph.PipelinePlanned`.
    PipelinePlanned(PipelinePlanned),
    /// `inference.graph.RunCreated`.
    #[cfg(feature = "run")]
    RunCreated(RunCreated),
    /// `inference.graph.RunValidating`.
    #[cfg(feature = "run")]
    RunValidating(RunValidating),
    /// `inference.graph.RunPreparing`.
    #[cfg(feature = "run")]
    RunPreparing(RunPreparing),
    /// `inference.graph.RunActivated`.
    #[cfg(feature = "run")]
    RunActivated(RunActivated),
    /// `inference.graph.RunDraining`.
    #[cfg(feature = "run")]
    RunDraining(RunDraining),
    /// `inference.graph.RunCompleted`.
    #[cfg(feature = "run")]
    RunCompleted(RunCompleted),
    /// `inference.graph.RunFailed`.
    #[cfg(feature = "run")]
    RunFailed(RunFailed),
    /// `inference.graph.RunCancelled`.
    #[cfg(feature = "run")]
    RunCancelled(RunCancelled),
    /// `inference.graph.NodeInstanceDescribed`.
    #[cfg(feature = "run")]
    NodeInstanceDescribed(NodeInstanceDescribed),
    /// `inference.graph.NodeInstancePrepared`.
    #[cfg(feature = "run")]
    NodeInstancePrepared(NodeInstancePrepared),
    /// `inference.graph.NodeInstanceStarted`.
    #[cfg(feature = "run")]
    NodeInstanceStarted(NodeInstanceStarted),
    /// `inference.graph.NodeInstanceDraining`.
    #[cfg(feature = "run")]
    NodeInstanceDraining(NodeInstanceDraining),
    /// `inference.graph.NodeInstanceCancelling`.
    #[cfg(feature = "run")]
    NodeInstanceCancelling(NodeInstanceCancelling),
    /// `inference.graph.NodeInstanceStopped`.
    #[cfg(feature = "run")]
    NodeInstanceStopped(NodeInstanceStopped),
    /// `inference.graph.NodeInstanceFailed`.
    #[cfg(feature = "run")]
    NodeInstanceFailed(NodeInstanceFailed),
}

/// The declared profiles and adapters a [`GraphDomain`] reads, from their own stores.
#[derive(Debug, Clone, Copy)]
pub struct GraphCatalog<'a> {
    pub policy: &'a PolicyDomain,
    pub model: &'a ModelDomain,
}

impl Catalog for GraphCatalog<'_> {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        self.policy.profile(profile_id)
    }

    fn adapter(&self, adapter_id: AdapterId) -> Option<&Adapter> {
        self.model.adapters().get(adapter_id)
    }
}

/// The first eight hex digits of every identity the domain issues, which no other domain of the
/// testkit issues under (see [`crate::policy::IDENTITY_SPACE`]).
pub const IDENTITY_SPACE: u32 = 3;

/// `inference.graph` in memory: the declared entities, the validated and planned forms of the
/// pipelines, the runs and their node instances, and the events their commands emitted.
#[derive(Debug)]
pub struct GraphDomain {
    declaration: Declaration,
    validated: Vec<ValidatedPipeline>,
    #[cfg(feature = "run")]
    planned: Vec<PlannedPipeline>,
    #[cfg(feature = "run")]
    runs: Vec<Run>,
    #[cfg(feature = "run")]
    instances: Vec<NodeInstance>,
    #[cfg(feature = "run")]
    issued: u128,
    events: Vec<GraphEvent>,
}

impl Default for GraphDomain {
    fn default() -> Self {
        Self {
            declaration: Declaration::counted_in(IDENTITY_SPACE),
            validated: Vec::new(),
            #[cfg(feature = "run")]
            planned: Vec::new(),
            #[cfg(feature = "run")]
            runs: Vec::new(),
            #[cfg(feature = "run")]
            instances: Vec::new(),
            #[cfg(feature = "run")]
            issued: 0,
            events: Vec::new(),
        }
    }
}

impl GraphDomain {
    /// An empty domain.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs `inference.graph.DeclarePipeline` and records its [`PipelineDeclared`].
    ///
    /// # Errors
    ///
    /// [`ProfileNotFound`] when `catalog` holds no profile carrying `profile_id`.
    pub fn declare_pipeline(
        &mut self,
        input: DeclarePipeline,
        catalog: &impl Catalog,
    ) -> Result<PipelineDeclared, ProfileNotFound> {
        let event = self.declaration.declare_pipeline(input, catalog)?;
        self.events
            .push(GraphEvent::PipelineDeclared(event.clone()));
        Ok(event)
    }

    /// Runs `inference.graph.AddNode` and records its [`NodeAdded`].
    ///
    /// # Errors
    ///
    /// As [`Declaration::add_node`].
    pub fn add_node(&mut self, input: AddNode) -> Result<NodeAdded, AddNodeError> {
        let event = self.declaration.add_node(input)?;
        self.events.push(GraphEvent::NodeAdded(event.clone()));
        Ok(event)
    }

    /// Runs `inference.graph.AddPort` and records its [`PortAdded`].
    ///
    /// # Errors
    ///
    /// As [`Declaration::add_port`].
    pub fn add_port(&mut self, input: AddPort) -> Result<PortAdded, AddPortError> {
        let event = self.declaration.add_port(input)?;
        self.events.push(GraphEvent::PortAdded(event.clone()));
        Ok(event)
    }

    /// Runs `inference.graph.ConnectPorts` and records its [`PortsConnected`].
    ///
    /// # Errors
    ///
    /// As [`Declaration::connect_ports`].
    pub fn connect_ports(
        &mut self,
        input: ConnectPorts,
    ) -> Result<PortsConnected, ConnectPortsError> {
        let event = self.declaration.connect_ports(input)?;
        self.events.push(GraphEvent::PortsConnected(event));
        Ok(event)
    }

    /// Runs `inference.graph.BindNode` and records its [`NodeBound`].
    ///
    /// # Errors
    ///
    /// As [`Declaration::bind_node`].
    pub fn bind_node(
        &mut self,
        input: BindNode,
        catalog: &impl Catalog,
    ) -> Result<NodeBound, BindNodeError> {
        let event = self.declaration.bind_node(input, catalog)?;
        self.events.push(GraphEvent::NodeBound(event));
        Ok(event)
    }

    /// Runs `inference.graph.ValidatePipeline` through [`Declaration::validate`], keeps the
    /// validated form for planning and records its [`PipelineValidated`].
    ///
    /// # Errors
    ///
    /// As [`Declaration::validate_pipeline`].
    pub fn validate_pipeline(
        &mut self,
        input: ValidatePipeline,
    ) -> Result<PipelineValidated, ValidatePipelineError> {
        let validated = self.declaration.validate(input.pipeline_id)?;
        let event = PipelineValidated {
            pipeline_id: validated.pipeline_id(),
        };
        self.validated.push(validated);
        self.events.push(GraphEvent::PipelineValidated(event));
        Ok(event)
    }

    /// Runs `inference.graph.PlanPipeline`: plans the validated form through
    /// [`Declaration::plan`], keeps the Planned form for `CreateRun` and records its
    /// [`PipelinePlanned`].
    ///
    /// Every Validated pipeline holds its validated form here, and only a Validated one does; a
    /// pipeline planning refuses keeps it, since the refusal is decided ([`Declaration::plan_of`])
    /// before the form is taken.
    ///
    /// # Errors
    ///
    /// As [`Declaration::plan_pipeline`]; a refused plan moves nothing.
    ///
    /// # Panics
    ///
    /// Never: a pipeline planned by identity here is one with no validated form, which is not
    /// Validated, and planning refuses it.
    pub fn plan_pipeline(
        &mut self,
        input: PlanPipeline,
        catalog: &impl Catalog,
    ) -> Result<PipelinePlanned, PlanPipelineError> {
        let pipeline_id = input.pipeline_id;
        let held = self
            .validated
            .iter()
            .position(|validated| validated.pipeline_id() == pipeline_id);
        let planned = match held {
            Some(at) if self.declaration.plan_of(pipeline_id, catalog).is_ok() => {
                let validated = self.validated.remove(at);
                self.declaration.plan(validated, catalog)?
            }
            _ => {
                return match self.declaration.plan_pipeline(input, catalog) {
                    Err(refused) => Err(refused),
                    Ok(_) => unreachable!("a pipeline with no validated form is not Validated"),
                };
            }
        };
        let event = PipelinePlanned {
            pipeline_id: planned.pipeline_id(),
        };
        #[cfg(feature = "run")]
        self.planned.push(planned);
        self.events.push(GraphEvent::PipelinePlanned(event));
        Ok(event)
    }

    /// The declared pipelines and what they own.
    #[must_use]
    pub const fn declaration(&self) -> &Declaration {
        &self.declaration
    }

    /// Every event emitted, in the order it was emitted.
    #[must_use]
    pub fn events(&self) -> &[GraphEvent] {
        &self.events
    }
}

/// The run half of the domain, behind the `run` feature: the run commands, their errors and
/// the views of the runs and node instances.
#[cfg(feature = "run")]
mod runs {
    use std::error::Error;
    use std::fmt;

    use inference_core::id::{NodeId, RunId};
    use inference_pipeline::{
        NodeNotFound, PipelineId, PipelineNotFound, PipelineNotReady, PipelineState,
    };
    use inference_runtime::run::{
        FailureReason, NodeInstance, NodeInstanceCancelling, NodeInstanceDescribed,
        NodeInstanceDraining, NodeInstanceFailed, NodeInstanceId, NodeInstancePrepared,
        NodeInstanceStarted, NodeInstanceStateConflict, NodeInstanceStopped, Run, RunActivated,
        RunCancelled, RunCompleted, RunCreated, RunDraining, RunFailed, RunNotFound, RunNotRunning,
        RunPreparing, RunStateConflict, RunValidating,
    };
    use uuid::Uuid;

    use super::{GraphDomain, GraphEvent, IDENTITY_SPACE};

    /// The second group of every run and node instance identity the domain issues, which no declared
    /// entity's identity carries.
    pub const RUN_IDENTITY_GROUP: u16 = 1;

    /// Why `inference.graph.CreateRun` created no run.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum CreateRunError {
        /// The `unknown-pipeline` outcome: no pipeline carries the identity sent.
        UnknownPipeline(PipelineNotFound),
        /// The `not-planned` outcome: the pipeline is not Planned.
        NotPlanned(PipelineNotReady),
    }

    impl fmt::Display for CreateRunError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::UnknownPipeline(error) => fmt::Display::fmt(error, f),
                Self::NotPlanned(error) => fmt::Display::fmt(error, f),
            }
        }
    }

    impl Error for CreateRunError {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            match self {
                Self::UnknownPipeline(error) => Some(error),
                Self::NotPlanned(error) => Some(error),
            }
        }
    }

    /// Why a moving run command (`ValidateRun`, `PrepareRun`, `ActivateRun`, `DrainRun`,
    /// `CompleteRun`, `FailRun`, `CancelRun`) moved nothing. Both are the command's `wrong-state`
    /// outcome.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum RunMoveError {
        /// No stored run carries the identity sent; there is no state to report.
        Unknown(RunNotFound),
        /// The run is in a state the command's transition does not leave.
        Conflict(RunStateConflict),
    }

    impl fmt::Display for RunMoveError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Unknown(error) => fmt::Display::fmt(error, f),
                Self::Conflict(error) => fmt::Display::fmt(error, f),
            }
        }
    }

    impl Error for RunMoveError {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            match self {
                Self::Unknown(error) => Some(error),
                Self::Conflict(error) => Some(error),
            }
        }
    }

    /// Why `inference.graph.DescribeNodeInstance` described no instance, in the order it is decided.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum DescribeNodeInstanceError {
        /// The `unknown-run` outcome: no stored run carries the identity sent.
        UnknownRun(RunNotFound),
        /// The `unknown-node` outcome: no declared node carries the identity sent.
        UnknownNode(NodeNotFound),
        /// The `run-not-running` outcome: the run is neither Preparing nor Active.
        RunNotRunning(RunNotRunning),
    }

    impl fmt::Display for DescribeNodeInstanceError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::UnknownRun(error) => fmt::Display::fmt(error, f),
                Self::UnknownNode(error) => fmt::Display::fmt(error, f),
                Self::RunNotRunning(error) => fmt::Display::fmt(error, f),
            }
        }
    }

    impl Error for DescribeNodeInstanceError {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            match self {
                Self::UnknownRun(error) => Some(error),
                Self::UnknownNode(error) => Some(error),
                Self::RunNotRunning(error) => Some(error),
            }
        }
    }

    /// Why a node instance command (`PrepareNodeInstance`, `StartNodeInstance`,
    /// `DrainNodeInstance`, `CancelNodeInstance`, `StopNodeInstance`, `FailNodeInstance`) moved
    /// nothing. Both are the command's `wrong-state` outcome.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum NodeInstanceMoveError {
        /// No stored instance carries the identity sent; there is no state to report.
        Unknown(NodeInstanceId),
        /// The instance is in a state the command's transition does not leave.
        Conflict(NodeInstanceStateConflict),
    }

    impl fmt::Display for NodeInstanceMoveError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::Unknown(instance_id) => {
                    write!(f, "no node instance carries the identity {instance_id}")
                }
                Self::Conflict(error) => fmt::Display::fmt(error, f),
            }
        }
    }

    impl Error for NodeInstanceMoveError {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            match self {
                Self::Unknown(_) => None,
                Self::Conflict(error) => Some(error),
            }
        }
    }

    impl GraphDomain {
        /// Runs `inference.graph.PrepareNodeInstance`: Described to Prepared.
        ///
        /// # Errors
        ///
        /// [`NodeInstanceMoveError::Unknown`] when no stored instance carries `instance_id`,
        /// [`NodeInstanceMoveError::Conflict`] when it is not Described. Nothing moves and no
        /// event is recorded.
        pub fn prepare_node_instance(
            &mut self,
            instance_id: NodeInstanceId,
        ) -> Result<NodeInstancePrepared, NodeInstanceMoveError> {
            self.move_instance(
                instance_id,
                NodeInstance::prepare,
                GraphEvent::NodeInstancePrepared,
            )
        }

        /// Runs `inference.graph.StartNodeInstance`: Prepared to Started.
        ///
        /// # Errors
        ///
        /// As [`GraphDomain::prepare_node_instance`], when the instance is not Prepared.
        pub fn start_node_instance(
            &mut self,
            instance_id: NodeInstanceId,
        ) -> Result<NodeInstanceStarted, NodeInstanceMoveError> {
            self.move_instance(
                instance_id,
                NodeInstance::start,
                GraphEvent::NodeInstanceStarted,
            )
        }

        /// Runs `inference.graph.DrainNodeInstance`: Started to Draining.
        ///
        /// # Errors
        ///
        /// As [`GraphDomain::prepare_node_instance`], when the instance is not Started.
        pub fn drain_node_instance(
            &mut self,
            instance_id: NodeInstanceId,
        ) -> Result<NodeInstanceDraining, NodeInstanceMoveError> {
            self.move_instance(
                instance_id,
                NodeInstance::drain,
                GraphEvent::NodeInstanceDraining,
            )
        }

        /// Runs `inference.graph.CancelNodeInstance`: Described, Prepared, Started or Draining to
        /// Cancelling.
        ///
        /// # Errors
        ///
        /// As [`GraphDomain::prepare_node_instance`], when the instance is Cancelling or terminal.
        pub fn cancel_node_instance(
            &mut self,
            instance_id: NodeInstanceId,
        ) -> Result<NodeInstanceCancelling, NodeInstanceMoveError> {
            self.move_instance(
                instance_id,
                NodeInstance::cancel,
                GraphEvent::NodeInstanceCancelling,
            )
        }

        /// Runs `inference.graph.StopNodeInstance`: Prepared, Draining or Cancelling to Stopped.
        ///
        /// # Errors
        ///
        /// As [`GraphDomain::prepare_node_instance`], when the instance is Described, Started or
        /// terminal.
        pub fn stop_node_instance(
            &mut self,
            instance_id: NodeInstanceId,
        ) -> Result<NodeInstanceStopped, NodeInstanceMoveError> {
            self.move_instance(
                instance_id,
                NodeInstance::stop,
                GraphEvent::NodeInstanceStopped,
            )
        }

        /// Runs `inference.graph.FailNodeInstance`: Described, Prepared, Started or Draining to
        /// Failed, recording `reason` in its [`NodeInstanceFailed`].
        ///
        /// # Errors
        ///
        /// As [`GraphDomain::prepare_node_instance`], when the instance is Cancelling or terminal.
        pub fn fail_node_instance(
            &mut self,
            instance_id: NodeInstanceId,
            reason: String,
        ) -> Result<NodeInstanceFailed, NodeInstanceMoveError> {
            let slot = self.instance_slot(instance_id)?;
            let (moved, event) = slot.fail(reason).map_err(NodeInstanceMoveError::Conflict)?;
            *slot = moved;
            self.events
                .push(GraphEvent::NodeInstanceFailed(event.clone()));
            Ok(event)
        }

        /// Moves the instance carrying `instance_id` by `transition`, stores it and records its
        /// event.
        fn move_instance<E: Clone>(
            &mut self,
            instance_id: NodeInstanceId,
            transition: fn(&NodeInstance) -> Result<(NodeInstance, E), NodeInstanceStateConflict>,
            record: fn(E) -> GraphEvent,
        ) -> Result<E, NodeInstanceMoveError> {
            let slot = self.instance_slot(instance_id)?;
            let (moved, event) = transition(slot).map_err(NodeInstanceMoveError::Conflict)?;
            *slot = moved;
            self.events.push(record(event.clone()));
            Ok(event)
        }

        /// The stored instance carrying `instance_id`, to move it.
        fn instance_slot(
            &mut self,
            instance_id: NodeInstanceId,
        ) -> Result<&mut NodeInstance, NodeInstanceMoveError> {
            self.instances
                .iter_mut()
                .find(|instance| instance.instance_id() == instance_id)
                .ok_or(NodeInstanceMoveError::Unknown(instance_id))
        }
    }

    impl GraphDomain {
        /// Runs `inference.graph.CreateRun`: builds a Created [`Run`] from the Planned form of the
        /// pipeline, stores it and records its [`RunCreated`].
        ///
        /// # Errors
        ///
        /// [`CreateRunError::UnknownPipeline`] when no declared pipeline carries `pipeline_id`,
        /// otherwise [`CreateRunError::NotPlanned`] when it is not Planned. Nothing is stored, no
        /// event is recorded and no identity is issued.
        pub fn create_run(
            &mut self,
            pipeline_id: PipelineId,
        ) -> Result<RunCreated, CreateRunError> {
            if self.declaration.pipeline(pipeline_id).is_none() {
                return Err(CreateRunError::UnknownPipeline(PipelineNotFound {
                    pipeline_id,
                }));
            }
            let planned = self
                .planned
                .iter()
                .find(|planned| planned.pipeline_id() == pipeline_id)
                .ok_or(CreateRunError::NotPlanned(PipelineNotReady {
                    pipeline_id,
                    required: PipelineState::Planned,
                }))?;
            let (run, event) =
                Run::create_as(RunId::from(Self::run_identity(self.issued + 1)), planned);
            self.issued += 1;
            self.runs.push(run);
            self.events.push(GraphEvent::RunCreated(event));
            Ok(event)
        }

        /// Runs `inference.graph.ValidateRun`: Created to Validating.
        ///
        /// # Errors
        ///
        /// [`RunMoveError::Unknown`] when no stored run carries `run_id`,
        /// [`RunMoveError::Conflict`] when it is not Created. Nothing moves and no event is recorded.
        pub fn validate_run(&mut self, run_id: RunId) -> Result<RunValidating, RunMoveError> {
            self.move_run(run_id, Run::validate, GraphEvent::RunValidating)
        }

        /// Runs the `preparing` outcome of `inference.graph.PrepareRun`: Validating to Preparing.
        /// Its external `refused` outcome is not decided here: whether the runtime can admit the
        /// run's resources is the runtime's to observe.
        ///
        /// # Errors
        ///
        /// As [`GraphDomain::validate_run`], when the run is not Validating.
        pub fn prepare_run(&mut self, run_id: RunId) -> Result<RunPreparing, RunMoveError> {
            self.move_run(run_id, Run::prepare, GraphEvent::RunPreparing)
        }

        /// Runs `inference.graph.ActivateRun`: Preparing to Active.
        ///
        /// # Errors
        ///
        /// As [`GraphDomain::validate_run`], when the run is not Preparing.
        pub fn activate_run(&mut self, run_id: RunId) -> Result<RunActivated, RunMoveError> {
            self.move_run(run_id, Run::activate, GraphEvent::RunActivated)
        }

        /// Runs `inference.graph.DrainRun`: Active to Draining.
        ///
        /// # Errors
        ///
        /// As [`GraphDomain::validate_run`], when the run is not Active.
        pub fn drain_run(&mut self, run_id: RunId) -> Result<RunDraining, RunMoveError> {
            self.move_run(run_id, Run::drain, GraphEvent::RunDraining)
        }

        /// Runs `inference.graph.CompleteRun`: Draining to Completed.
        ///
        /// # Errors
        ///
        /// As [`GraphDomain::validate_run`], when the run is not Draining.
        pub fn complete_run(&mut self, run_id: RunId) -> Result<RunCompleted, RunMoveError> {
            self.move_run(run_id, Run::complete, GraphEvent::RunCompleted)
        }

        /// Runs `inference.graph.FailRun`: Validating, Preparing, Active or Draining to Failed, with
        /// `failure` set on the run.
        ///
        /// # Errors
        ///
        /// As [`GraphDomain::validate_run`], when the run is Created or terminal.
        pub fn fail_run(
            &mut self,
            run_id: RunId,
            failure: FailureReason,
        ) -> Result<RunFailed, RunMoveError> {
            let slot = self.run_slot(run_id)?;
            let (moved, event) = slot.fail(failure).map_err(RunMoveError::Conflict)?;
            *slot = moved;
            self.events.push(GraphEvent::RunFailed(event.clone()));
            Ok(event)
        }

        /// Runs the run half of `inference.graph.CancelRun`: any state but a terminal one to
        /// Cancelled. The stream half, the run's Active generations cancelled and its unplayed
        /// segments discarded, is [`crate::stream::StreamDomain::cancel_run`].
        ///
        /// # Errors
        ///
        /// As [`GraphDomain::validate_run`], when the run is terminal.
        pub fn cancel_run(&mut self, run_id: RunId) -> Result<RunCancelled, RunMoveError> {
            self.move_run(run_id, Run::cancel, GraphEvent::RunCancelled)
        }

        /// Runs `inference.graph.DescribeNodeInstance`: stores the Described instance of `node_id` in
        /// the run and records its [`NodeInstanceDescribed`].
        ///
        /// # Errors
        ///
        /// In this order: [`DescribeNodeInstanceError::UnknownRun`],
        /// [`DescribeNodeInstanceError::UnknownNode`] when no declared node carries `node_id`, and
        /// [`DescribeNodeInstanceError::RunNotRunning`]. Nothing is stored, no event is recorded and
        /// no identity is issued.
        pub fn describe_node_instance(
            &mut self,
            run_id: RunId,
            node_id: NodeId,
        ) -> Result<NodeInstanceDescribed, DescribeNodeInstanceError> {
            let run = self
                .run(run_id)
                .ok_or(DescribeNodeInstanceError::UnknownRun(RunNotFound {
                    run_id,
                }))?;
            if self.declaration.node(node_id).is_none() {
                return Err(DescribeNodeInstanceError::UnknownNode(NodeNotFound {
                    node_id,
                }));
            }
            let instance_id = NodeInstanceId::from(Self::run_identity(self.issued + 1));
            let (instance, event) = NodeInstance::describe_as(instance_id, run, node_id)
                .map_err(DescribeNodeInstanceError::RunNotRunning)?;
            self.issued += 1;
            self.instances.push(instance);
            self.events.push(GraphEvent::NodeInstanceDescribed(event));
            Ok(event)
        }

        /// Moves the run carrying `run_id` by `transition`, stores it and records its event.
        fn move_run<E: Clone>(
            &mut self,
            run_id: RunId,
            transition: fn(&Run) -> Result<(Run, E), RunStateConflict>,
            record: fn(E) -> GraphEvent,
        ) -> Result<E, RunMoveError> {
            let slot = self.run_slot(run_id)?;
            let (moved, event) = transition(slot).map_err(RunMoveError::Conflict)?;
            *slot = moved;
            self.events.push(record(event.clone()));
            Ok(event)
        }

        /// The stored run carrying `run_id`, to move it.
        fn run_slot(&mut self, run_id: RunId) -> Result<&mut Run, RunMoveError> {
            self.runs
                .iter_mut()
                .find(|run| run.run_id() == run_id)
                .ok_or(RunMoveError::Unknown(RunNotFound { run_id }))
        }

        /// The `count`th run or node instance identity: `{IDENTITY_SPACE:08x}-0001-4000-8000-` and
        /// `count` in the last twelve digits.
        fn run_identity(count: u128) -> Uuid {
            Uuid::from_u128(
                (u128::from(IDENTITY_SPACE) << 96)
                    | (u128::from(RUN_IDENTITY_GROUP) << 80)
                    | 0x0000_0000_0000_4000_8000_0000_0000_0000
                    | count,
            )
        }

        /// The run carrying `run_id`, if one was created.
        #[must_use]
        pub fn run(&self, run_id: RunId) -> Option<&Run> {
            self.runs.iter().find(|run| run.run_id() == run_id)
        }

        /// The view `inference.graph.Runs`: every run, in the order they were created.
        pub fn runs(&self) -> impl Iterator<Item = &Run> {
            self.runs.iter()
        }

        /// The view `inference.graph.NodeInstances`: every node instance, in the order they were
        /// described.
        pub fn node_instances(&self) -> impl Iterator<Item = &NodeInstance> {
            self.instances.iter()
        }
    }
}

#[cfg(feature = "run")]
pub use runs::{
    CreateRunError, DescribeNodeInstanceError, NodeInstanceMoveError, RUN_IDENTITY_GROUP,
    RunMoveError,
};

/// The recording mock of the node contract, behind the `run` feature: a [`RecordingNode`]
/// implements [`NodeImplementation`] for any payload, records every call the runtime makes in a
/// [`NodeCalls`] shared with the test, fails on demand at one fallible step and stalls once on
/// demand at one step.
///
/// [`NodeImplementation`]: inference_pipeline::NodeImplementation
#[cfg(feature = "run")]
mod recording {
    use std::sync::{Arc, Mutex, PoisonError};

    use inference_pipeline::{Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation};

    /// One call the runtime made on a [`RecordingNode`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum NodeCall {
        /// `describe`.
        Describe,
        /// `prepare`, whether or not it returned an error.
        Prepare,
        /// `start`, with how many input and output endpoints the node was handed.
        Start { inputs: usize, outputs: usize },
        /// `drain`, whether or not it returned an error.
        Drain,
        /// `cancel`.
        Cancel,
        /// `stop`.
        Stop,
    }

    /// A step of the lifecycle that may return an error, where a [`RecordingNode`] can be told to.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum FallibleStep {
        Prepare,
        Start,
        Drain,
    }

    /// A call of the lifecycle, where a [`RecordingNode`] can be told to stall.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum LifecycleStep {
        Describe,
        Prepare,
        Start,
        Drain,
        Cancel,
        Stop,
    }

    /// The calls a [`RecordingNode`] received, in order; a clone reads the same record.
    #[derive(Debug, Clone, Default)]
    pub struct NodeCalls(Arc<Mutex<Vec<NodeCall>>>);

    impl NodeCalls {
        /// Every call so far, in the order it was made.
        #[must_use]
        pub fn calls(&self) -> Vec<NodeCall> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        fn record(&self, call: NodeCall) {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(call);
        }
    }

    /// A node that does nothing but record each call and answer it, failing at one step or
    /// stalling once at one step on demand. Its endpoints are dropped as soon as it is started.
    #[derive(Debug, Clone, Default)]
    pub struct RecordingNode {
        calls: NodeCalls,
        failing: Option<(FallibleStep, NodeError)>,
        stalling: Option<LifecycleStep>,
    }

    impl RecordingNode {
        /// A node that answers every call.
        #[must_use]
        pub fn new() -> Self {
            Self::default()
        }

        /// A node that returns `error` at `step` and answers every other call.
        #[must_use]
        pub fn failing_at(step: FallibleStep, error: NodeError) -> Self {
            Self {
                calls: NodeCalls::default(),
                failing: Some((step, error)),
                stalling: None,
            }
        }

        /// The record of the node's calls, shared with the node.
        #[must_use]
        pub fn calls(&self) -> NodeCalls {
            self.calls.clone()
        }

        /// Records `call` and answers it, with the error set for `step` if there is one.
        fn answer(&self, call: NodeCall, step: FallibleStep) -> Result<(), NodeError> {
            self.calls.record(call);
            match &self.failing {
                Some((failing, error)) if *failing == step => Err(error.clone()),
                _ => Ok(()),
            }
        }

        /// The same node, whose first call of `step` never completes, as a caller's timeout or
        /// `select!` sees a slow node; later calls of `step` answer as usual. The call is recorded.
        #[must_use]
        pub fn stalling_at(mut self, step: LifecycleStep) -> Self {
            self.stalling = Some(step);
            self
        }

        /// `value`, now, or never if this is the stalled call of `step`.
        fn reply<T: Send + 'static>(&mut self, step: LifecycleStep, value: T) -> NodeFuture<'_, T> {
            if self.stalling == Some(step) {
                self.stalling = None;
                Box::pin(std::future::pending())
            } else {
                Box::pin(std::future::ready(value))
            }
        }
    }

    impl<P> NodeImplementation<P> for RecordingNode {
        fn describe(&mut self, _: &Node) -> NodeFuture<'_, ()> {
            self.calls.record(NodeCall::Describe);
            self.reply(LifecycleStep::Describe, ())
        }

        fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
            let answer = self.answer(NodeCall::Prepare, FallibleStep::Prepare);
            self.reply(LifecycleStep::Prepare, answer)
        }

        fn start(&mut self, endpoints: NodeEndpoints<P>) -> NodeFuture<'_, Result<(), NodeError>> {
            let call = NodeCall::Start {
                inputs: endpoints.inputs.len(),
                outputs: endpoints.outputs.len(),
            };
            drop(endpoints);
            let answer = self.answer(call, FallibleStep::Start);
            self.reply(LifecycleStep::Start, answer)
        }

        fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
            let answer = self.answer(NodeCall::Drain, FallibleStep::Drain);
            self.reply(LifecycleStep::Drain, answer)
        }

        fn cancel(&mut self) -> NodeFuture<'_, ()> {
            self.calls.record(NodeCall::Cancel);
            self.reply(LifecycleStep::Cancel, ())
        }

        fn stop(&mut self) -> NodeFuture<'_, ()> {
            self.calls.record(NodeCall::Stop);
            self.reply(LifecycleStep::Stop, ())
        }
    }
}

#[cfg(feature = "run")]
pub use recording::{FallibleStep, LifecycleStep, NodeCall, NodeCalls, RecordingNode};

#[cfg(test)]
mod tests {
    use inference_core::model::{
        Capability, DeclareAdapter, DeclareModel, ExecutionForm, Locality, Modality,
    };
    use inference_core::policy::{DeclareProfile, Egress, ProfileKind, ProfileNotFound};
    use inference_pipeline::{
        AddNode, AddNodeError, DeclarePipeline, InteractionMode, NodeRole, PipelineId,
        PipelineNotFound,
    };
    use uuid::Uuid;

    use super::{GraphCatalog, GraphDomain, GraphEvent};
    use crate::model::ModelDomain;
    use crate::policy::PolicyDomain;

    fn stores() -> (PolicyDomain, ModelDomain) {
        let mut policy = PolicyDomain::new();
        policy
            .declare_profile(DeclareProfile {
                kind: ProfileKind::OfflineStrict,
                egress: Egress::Denied,
                admitted: vec![Locality::SameProcess],
            })
            .expect("a row of the table");
        let mut model = ModelDomain::new();
        let declared = model.declare_model(DeclareModel {
            family: "f".to_owned(),
            checkpoint: "c".to_owned(),
            code_license: "l".to_owned(),
            weight_license: "w".to_owned(),
            checksum: "s".to_owned(),
            input_modalities: vec![Modality::Text],
            output_modalities: vec![Modality::Text],
            provenance: "p".to_owned(),
            source_revision: "r".to_owned(),
        });
        model
            .declare_adapter(DeclareAdapter {
                model_id: declared.model_id,
                backend: "b".to_owned(),
                contract_revision: "c".to_owned(),
                execution_form: ExecutionForm::InProcess,
                locality: Locality::SameProcess,
                capabilities: vec![Capability::OutputStreaming],
                max_concurrency: 1,
            })
            .expect("declared");
        (policy, model)
    }

    fn declare_pipeline(policy: &PolicyDomain) -> DeclarePipeline {
        DeclarePipeline {
            name: "p".to_owned(),
            schema_revision: "1".to_owned(),
            mode: InteractionMode::Batch,
            profile_id: policy.profiles().next().expect("a profile").profile_id(),
            required_capabilities: Vec::new(),
        }
    }

    #[test]
    fn identities_come_from_a_counter_and_a_refusal_records_and_takes_nothing() {
        let (policy, model) = stores();
        let catalog = GraphCatalog {
            policy: &policy,
            model: &model,
        };
        let mut domain = GraphDomain::new();
        let profile_id = Uuid::from_u128(0xdead).into();

        assert_eq!(
            domain.declare_pipeline(
                DeclarePipeline {
                    profile_id,
                    ..declare_pipeline(&policy)
                },
                &catalog
            ),
            Err(ProfileNotFound { profile_id })
        );
        let unknown = PipelineId::from(Uuid::from_u128(0xbeef));
        assert_eq!(
            domain.add_node(AddNode {
                pipeline_id: unknown,
                name: "n".to_owned(),
                role: NodeRole::Source,
                requires: Vec::new(),
            }),
            Err(AddNodeError::UnknownPipeline(PipelineNotFound {
                pipeline_id: unknown
            }))
        );
        assert!(domain.events().is_empty());

        let declared = domain
            .declare_pipeline(declare_pipeline(&policy), &catalog)
            .expect("the profile is declared");
        let added = domain
            .add_node(AddNode {
                pipeline_id: declared.pipeline_id,
                name: "n".to_owned(),
                role: NodeRole::Source,
                requires: Vec::new(),
            })
            .expect("the pipeline is declared");
        assert_eq!(
            declared.pipeline_id.to_string(),
            "00000003-0000-4000-8000-000000000001"
        );
        assert_eq!(
            added.node_id.to_string(),
            "00000003-0000-4000-8000-000000000002"
        );
        assert_eq!(
            domain.events(),
            [
                GraphEvent::PipelineDeclared(declared),
                GraphEvent::NodeAdded(added)
            ]
        );
    }

    #[test]
    fn a_validation_records_its_event_and_a_refused_one_records_nothing() {
        use inference_pipeline::{
            PipelineState, PipelineStateConflict, PipelineValidated, ValidatePipeline,
            ValidatePipelineError,
        };

        let (policy, model) = stores();
        let catalog = GraphCatalog {
            policy: &policy,
            model: &model,
        };
        let mut domain = GraphDomain::new();
        let pipeline_id = domain
            .declare_pipeline(declare_pipeline(&policy), &catalog)
            .expect("the profile is declared")
            .pipeline_id;
        let validate = ValidatePipeline { pipeline_id };

        assert_eq!(
            domain.validate_pipeline(validate),
            Ok(PipelineValidated { pipeline_id })
        );
        assert_eq!(
            domain.validate_pipeline(validate),
            Err(ValidatePipelineError::WrongState(PipelineStateConflict {
                current: PipelineState::Validated
            }))
        );
        assert_eq!(domain.events().len(), 2);
        assert_eq!(
            domain.events()[1],
            GraphEvent::PipelineValidated(PipelineValidated { pipeline_id })
        );
    }

    #[test]
    fn a_plan_records_its_event_and_a_refused_one_records_nothing() {
        use inference_pipeline::{
            CapabilityMismatch, PipelinePlanned, PipelineState, PipelineStateConflict,
            PlanPipeline, PlanPipelineError, ValidatePipeline,
        };

        let (policy, model) = stores();
        let catalog = GraphCatalog {
            policy: &policy,
            model: &model,
        };
        let mut domain = GraphDomain::new();
        let pipeline_id = domain
            .declare_pipeline(
                DeclarePipeline {
                    required_capabilities: vec![Capability::OutputStreaming],
                    ..declare_pipeline(&policy)
                },
                &catalog,
            )
            .expect("the profile is declared")
            .pipeline_id;
        let plan = PlanPipeline { pipeline_id };

        assert_eq!(
            domain.plan_pipeline(plan, &catalog),
            Err(PlanPipelineError::WrongState(PipelineStateConflict {
                current: PipelineState::Declared
            }))
        );
        domain
            .validate_pipeline(ValidatePipeline { pipeline_id })
            .expect("an empty graph is valid");
        assert_eq!(
            domain.plan_pipeline(plan, &catalog),
            Ok(PipelinePlanned { pipeline_id })
        );
        assert_eq!(
            domain.events().last(),
            Some(&GraphEvent::PipelinePlanned(PipelinePlanned {
                pipeline_id
            }))
        );
        assert_eq!(domain.events().len(), 3);
        let pipeline = domain
            .declaration()
            .pipeline(pipeline_id)
            .expect("declared");
        assert_eq!(pipeline.state(), PipelineState::Planned);
        assert_eq!(
            pipeline.effective_capabilities(),
            Some([].as_slice()),
            "a pipeline with no node has no effective capability"
        );

        let short = domain
            .declare_pipeline(
                DeclarePipeline {
                    required_capabilities: vec![Capability::ToolCalling],
                    ..declare_pipeline(&policy)
                },
                &catalog,
            )
            .expect("the profile is declared")
            .pipeline_id;
        let source = domain
            .add_node(AddNode {
                pipeline_id: short,
                name: "llm".to_owned(),
                role: NodeRole::Model,
                requires: vec![Capability::ToolCalling],
            })
            .expect("the pipeline is declared")
            .node_id;
        let adapter_id = model
            .adapters()
            .iter()
            .next()
            .expect("an adapter")
            .adapter_id();
        domain
            .bind_node(
                inference_pipeline::BindNode {
                    pipeline_id: short,
                    node_id: source,
                    adapter_id,
                },
                &catalog,
            )
            .expect("the adapter is declared");
        domain
            .validate_pipeline(ValidatePipeline { pipeline_id: short })
            .expect("a lone node with no port is valid");
        let recorded = domain.events().len();
        assert_eq!(
            domain.plan_pipeline(PlanPipeline { pipeline_id: short }, &catalog),
            Err(PlanPipelineError::Mismatch(CapabilityMismatch {
                node_id: Some(source),
                capability: Capability::ToolCalling,
            }))
        );
        assert_eq!(domain.events().len(), recorded);
    }

    #[cfg(feature = "run")]
    #[test]
    fn runs_take_identities_apart_from_the_declaration_and_a_refusal_takes_none() {
        use inference_core::id::{NodeId, RunId};
        use inference_pipeline::{
            NodeNotFound, PipelineNotReady, PipelineState, PlanPipeline, ValidatePipeline,
        };
        use inference_runtime::run::{
            NodeInstanceDescribed, RunCreated, RunNotFound, RunNotRunning, RunState,
            RunStateConflict,
        };

        use super::{CreateRunError, DescribeNodeInstanceError, RunMoveError};

        let (policy, model) = stores();
        let catalog = GraphCatalog {
            policy: &policy,
            model: &model,
        };
        let mut domain = GraphDomain::new();
        let pipeline_id = domain
            .declare_pipeline(declare_pipeline(&policy), &catalog)
            .expect("the profile is declared")
            .pipeline_id;
        let node_id = domain
            .add_node(AddNode {
                pipeline_id,
                name: "n".to_owned(),
                role: NodeRole::Source,
                requires: Vec::new(),
            })
            .expect("the pipeline is declared")
            .node_id;
        domain
            .validate_pipeline(ValidatePipeline { pipeline_id })
            .expect("a lone node is valid");
        assert_eq!(
            domain.create_run(pipeline_id),
            Err(CreateRunError::NotPlanned(PipelineNotReady {
                pipeline_id,
                required: PipelineState::Planned,
            }))
        );
        let unknown_pipeline = PipelineId::from(Uuid::from_u128(0xbeef));
        assert_eq!(
            domain.create_run(unknown_pipeline),
            Err(CreateRunError::UnknownPipeline(PipelineNotFound {
                pipeline_id: unknown_pipeline
            }))
        );
        domain
            .plan_pipeline(PlanPipeline { pipeline_id }, &catalog)
            .expect("nothing is required");
        let recorded = domain.events().len();

        let created = domain.create_run(pipeline_id).expect("planned");
        assert_eq!(
            created,
            RunCreated {
                run_id: created.run_id,
                pipeline_id
            }
        );
        assert_eq!(
            created.run_id.to_string(),
            "00000003-0001-4000-8000-000000000001"
        );
        let unknown_run = RunId::from(Uuid::from_u128(0xdead));
        assert_eq!(
            domain.validate_run(unknown_run),
            Err(RunMoveError::Unknown(RunNotFound {
                run_id: unknown_run
            }))
        );
        assert_eq!(
            domain.activate_run(created.run_id),
            Err(RunMoveError::Conflict(RunStateConflict {
                current: RunState::Created
            }))
        );
        assert_eq!(
            domain.describe_node_instance(created.run_id, node_id),
            Err(DescribeNodeInstanceError::RunNotRunning(RunNotRunning {
                run_id: created.run_id
            }))
        );
        let unknown_node = NodeId::from(Uuid::from_u128(0xdead));
        assert_eq!(
            domain.describe_node_instance(created.run_id, unknown_node),
            Err(DescribeNodeInstanceError::UnknownNode(NodeNotFound {
                node_id: unknown_node
            }))
        );
        assert_eq!(domain.events().len(), recorded + 1);

        domain.validate_run(created.run_id).expect("Created");
        domain.prepare_run(created.run_id).expect("Validating");
        let described = domain
            .describe_node_instance(created.run_id, node_id)
            .expect("Preparing");
        assert_eq!(
            described.instance_id.to_string(),
            "00000003-0001-4000-8000-000000000002",
            "the refusals took no identity"
        );
        assert_eq!(
            domain.events().last(),
            Some(&GraphEvent::NodeInstanceDescribed(NodeInstanceDescribed {
                instance_id: described.instance_id,
                run_id: created.run_id,
                node_id,
            }))
        );
        assert_eq!(
            domain.declaration().pipelines().count() + domain.declaration().nodes().count(),
            2,
            "a run takes no identity of the declaration's counter"
        );
        assert_eq!(domain.runs().count(), 1);
        assert_eq!(domain.node_instances().count(), 1);
        assert_eq!(
            domain.run(created.run_id).map(|run| run.state()),
            Some(RunState::Preparing)
        );
    }

    #[test]
    fn the_catalog_reads_the_profile_and_adapter_stores() {
        use inference_pipeline::Catalog;

        let (policy, model) = stores();
        let catalog = GraphCatalog {
            policy: &policy,
            model: &model,
        };
        let profile = policy.profiles().next().expect("a profile").profile_id();
        let adapter = model
            .adapters()
            .iter()
            .next()
            .expect("an adapter")
            .adapter_id();
        assert!(catalog.profile(profile).is_some());
        assert!(catalog.adapter(adapter).is_some());
        assert!(catalog.profile(Uuid::from_u128(0xdead).into()).is_none());
        assert!(catalog.adapter(Uuid::from_u128(0xdead).into()).is_none());
    }
}
