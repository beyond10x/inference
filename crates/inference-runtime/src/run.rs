//! The `inference.graph.Run` and `inference.graph.NodeInstance` vocabulary of
//! `ess/domains/graph.yaml`: one execution of a planned pipeline, and a node as it participates
//! in one run.
//!
//! A [`Run`] is created from the Planned form of a pipeline, [`PlannedPipeline`], and from nothing
//! else, so a run of a pipeline that was not planned cannot be written (`docs/design/inference.md`
//! lines 519 and 651). It moves by command through the states of the design, Created,
//! Validating, Preparing, Active, Draining and Completed, with Failed and Cancelled explicit
//! (line 671 and 673):
//!
//! | transition | from | to |
//! |---|---|---|
//! | [`Run::validate`] | Created | Validating |
//! | [`Run::prepare`] | Validating | Preparing |
//! | [`Run::activate`] | Preparing | Active |
//! | [`Run::drain`] | Active | Draining |
//! | [`Run::complete`] | Draining | Completed |
//! | [`Run::fail`] | Validating, Preparing, Active, Draining | Failed |
//! | [`Run::cancel`] | Created, Validating, Preparing, Active, Draining | Cancelled |
//!
//! Each transition returns the moved run and the event its command emits, or [`RunStateConflict`]
//! carrying the state the run holds when the transition does not leave it; a refused move changes
//! nothing. [`Run::fail`] records the [`FailureReason`] on the run.
//!
//! A [`NodeInstance`] is described ([`NodeInstance::describe`]) for a run that is Preparing or
//! Active, and is refused with [`RunNotRunning`] otherwise; Described is its one state until the
//! node instance lifecycle lands.
//!
//! What only a store holding the runs, the pipelines and the nodes can decide is not here: a run,
//! a pipeline or a node no record carries ([`RunNotFound`]), a pipeline that is not Planned,
//! whether the runtime can admit a run's resources ([`AdmissionDenied`]), and what cancelling a
//! run does to its generations and segments. Serialized names are the ESS names.

use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use inference_core::id::{NodeId, RunId};
use inference_pipeline::{PipelineId, PlannedPipeline};

inference_core::uuid_newtype!(
    /// `inference.graph.NodeInstanceId`: identifies a [`NodeInstance`].
    NodeInstanceId
);

/// `inference.graph.Run.State`: where a run stands. Serialized as written in ESS (`Created`,
/// `Validating`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RunState {
    /// The run exists and has not started validating.
    Created,
    /// The run is validating its plan against the resources at hand.
    Validating,
    /// Node instances are being prepared and resources admitted.
    Preparing,
    /// Events flow along the edges.
    Active,
    /// No new input is admitted; in-flight events complete.
    Draining,
    /// The run drained and released its worker resources.
    Completed,
    /// The run failed with a typed reason.
    Failed,
    /// The run was cancelled.
    Cancelled,
}

impl RunState {
    /// Every state, in the order `ess/domains/graph.yaml` lists them.
    pub const ALL: [Self; 8] = [
        Self::Created,
        Self::Validating,
        Self::Preparing,
        Self::Active,
        Self::Draining,
        Self::Completed,
        Self::Failed,
        Self::Cancelled,
    ];

    /// Whether no transition leaves the state: Completed, Failed and Cancelled.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// `inference.graph.NodeInstance.State`: where a node instance stands. Described is its one state
/// so far. Serialized as written in ESS (`Described`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeInstanceState {
    /// The node has an instance in the run.
    Described,
}

/// `inference.graph.AdmissionRefusal`: the outcomes runtime admission must permit. Serialized as
/// written in ESS (`not_enough_vram`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionRefusal {
    /// GPU memory is short.
    NotEnoughVram,
    /// The backend is busy.
    BackendBusy,
    /// The deadline cannot be met.
    DeadlineUnachievable,
}

impl AdmissionRefusal {
    /// Every refusal, in the order `ess/domains/graph.yaml` lists them.
    pub const ALL: [Self; 3] = [
        Self::NotEnoughVram,
        Self::BackendBusy,
        Self::DeadlineUnachievable,
    ];
}

/// `inference.graph.FailureReason`: why a run failed. `node_id` names the failed node where a
/// node failed; `detail` is an actionable description.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FailureReason {
    pub node_id: Option<NodeId>,
    pub detail: String,
}

/// `inference.graph.RunCreated`: the run exists and has not started validating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunCreated {
    pub run_id: RunId,
    pub pipeline_id: PipelineId,
}

/// `inference.graph.RunValidating`: the run is validating its plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunValidating {
    pub run_id: RunId,
}

/// `inference.graph.RunPreparing`: node instances are being prepared and resources admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunPreparing {
    pub run_id: RunId,
}

/// `inference.graph.RunActivated`: events flow along the edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunActivated {
    pub run_id: RunId,
}

/// `inference.graph.RunDraining`: no new input is admitted; in-flight events complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunDraining {
    pub run_id: RunId,
}

/// `inference.graph.RunCompleted`: the run drained and released its worker resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunCompleted {
    pub run_id: RunId,
}

/// `inference.graph.RunFailed`: the run is Failed with the reason it carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunFailed {
    pub run_id: RunId,
    pub failure: FailureReason,
}

/// `inference.graph.RunCancelled`: the run is Cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunCancelled {
    pub run_id: RunId,
}

/// `inference.graph.NodeInstanceDescribed`: the node has an instance in the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeInstanceDescribed {
    pub instance_id: NodeInstanceId,
    pub run_id: RunId,
    pub node_id: NodeId,
}

/// `inference.graph.RunStateConflict`: the run is not in a state the command acts from, so
/// nothing moved. `current` is the state it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunStateConflict {
    pub current: RunState,
}

impl fmt::Display for RunStateConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the run is {:?}, a state this command does not act from, so nothing moved",
            self.current
        )
    }
}

impl Error for RunStateConflict {}

/// `inference.graph.RunNotFound`: no run carries the identity the command named. Only a store
/// holding the runs can decide it; no method of [`Run`] returns it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunNotFound {
    pub run_id: RunId,
}

impl fmt::Display for RunNotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no run carries the identity {}", self.run_id)
    }
}

impl Error for RunNotFound {}

/// `inference.graph.RunNotRunning`: the run is not preparing or running its nodes, so no node
/// instance was described.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunNotRunning {
    pub run_id: RunId,
}

impl fmt::Display for RunNotRunning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the run {} is not Preparing or Active, so no node instance was described",
            self.run_id
        )
    }
}

impl Error for RunNotRunning {}

/// `inference.graph.AdmissionDenied`: the runtime denied the run's resources, and `reason` says
/// which limit was hit. No method of [`Run`] decides it: whether resources suffice is the
/// runtime's to observe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionDenied {
    pub reason: AdmissionRefusal,
}

impl fmt::Display for AdmissionDenied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "admission was denied: {:?}", self.reason)
    }
}

impl Error for AdmissionDenied {}

/// `inference.graph.Run`: one execution of a planned pipeline. It references its pipeline by
/// `pipeline_id` and carries the [`FailureReason`] it failed with, if it failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Run {
    run_id: RunId,
    pipeline_id: PipelineId,
    failure: Option<FailureReason>,
    state: RunState,
}

impl Run {
    /// Runs the `created` outcome of `inference.graph.CreateRun` for the planned pipeline:
    /// generates a [`RunId`] and returns the Created run and its [`RunCreated`].
    #[must_use]
    pub fn create(pipeline: &PlannedPipeline) -> (Self, RunCreated) {
        Self::create_as(RunId::generate(), pipeline)
    }

    /// Runs the `created` outcome of `inference.graph.CreateRun` under `run_id` instead of a
    /// generated one, and otherwise as [`Run::create`]. It is for a store that assigns the
    /// identities itself, such as the counter of `inference-testkit`.
    #[must_use]
    pub fn create_as(run_id: RunId, pipeline: &PlannedPipeline) -> (Self, RunCreated) {
        let pipeline_id = pipeline.pipeline_id();
        let run = Self {
            run_id,
            pipeline_id,
            failure: None,
            state: RunState::Created,
        };
        (
            run,
            RunCreated {
                run_id,
                pipeline_id,
            },
        )
    }

    /// The run moved to `to`, if it is in one of `from`; otherwise the state it holds.
    fn moved(&self, from: &[RunState], to: RunState) -> Result<Self, RunStateConflict> {
        if !from.contains(&self.state) {
            return Err(RunStateConflict {
                current: self.state,
            });
        }
        Ok(Self {
            state: to,
            ..self.clone()
        })
    }

    /// Runs the `validating` outcome of `inference.graph.ValidateRun`: Created to Validating.
    ///
    /// # Errors
    ///
    /// [`RunStateConflict`] carrying the held state when the run is not Created.
    pub fn validate(&self) -> Result<(Self, RunValidating), RunStateConflict> {
        let run = self.moved(&[RunState::Created], RunState::Validating)?;
        Ok((
            run,
            RunValidating {
                run_id: self.run_id,
            },
        ))
    }

    /// Runs the `preparing` outcome of `inference.graph.PrepareRun`: Validating to Preparing.
    ///
    /// # Errors
    ///
    /// [`RunStateConflict`] carrying the held state when the run is not Validating.
    pub fn prepare(&self) -> Result<(Self, RunPreparing), RunStateConflict> {
        let run = self.moved(&[RunState::Validating], RunState::Preparing)?;
        Ok((
            run,
            RunPreparing {
                run_id: self.run_id,
            },
        ))
    }

    /// Runs the `active` outcome of `inference.graph.ActivateRun`: Preparing to Active.
    ///
    /// # Errors
    ///
    /// [`RunStateConflict`] carrying the held state when the run is not Preparing.
    pub fn activate(&self) -> Result<(Self, RunActivated), RunStateConflict> {
        let run = self.moved(&[RunState::Preparing], RunState::Active)?;
        Ok((
            run,
            RunActivated {
                run_id: self.run_id,
            },
        ))
    }

    /// Runs the `draining` outcome of `inference.graph.DrainRun`: Active to Draining.
    ///
    /// # Errors
    ///
    /// [`RunStateConflict`] carrying the held state when the run is not Active.
    pub fn drain(&self) -> Result<(Self, RunDraining), RunStateConflict> {
        let run = self.moved(&[RunState::Active], RunState::Draining)?;
        Ok((
            run,
            RunDraining {
                run_id: self.run_id,
            },
        ))
    }

    /// Runs the `completed` outcome of `inference.graph.CompleteRun`: Draining to Completed.
    ///
    /// # Errors
    ///
    /// [`RunStateConflict`] carrying the held state when the run is not Draining.
    pub fn complete(&self) -> Result<(Self, RunCompleted), RunStateConflict> {
        let run = self.moved(&[RunState::Draining], RunState::Completed)?;
        Ok((
            run,
            RunCompleted {
                run_id: self.run_id,
            },
        ))
    }

    /// Runs the `failed` outcome of `inference.graph.FailRun`: Validating, Preparing, Active or
    /// Draining to Failed, with `failure` set on the run.
    ///
    /// # Errors
    ///
    /// [`RunStateConflict`] carrying the held state when the run is Created or terminal.
    pub fn fail(&self, failure: FailureReason) -> Result<(Self, RunFailed), RunStateConflict> {
        let mut run = self.moved(
            &[
                RunState::Validating,
                RunState::Preparing,
                RunState::Active,
                RunState::Draining,
            ],
            RunState::Failed,
        )?;
        run.failure = Some(failure.clone());
        Ok((
            run,
            RunFailed {
                run_id: self.run_id,
                failure,
            },
        ))
    }

    /// Runs the `cancelled` outcome of `inference.graph.CancelRun`: any state but a terminal one
    /// to Cancelled. Cancelling the run's Active generations and discarding its unplayed segments
    /// is the store's, which holds them.
    ///
    /// # Errors
    ///
    /// [`RunStateConflict`] carrying the held state when the run is terminal.
    pub fn cancel(&self) -> Result<(Self, RunCancelled), RunStateConflict> {
        let run = self.moved(
            &[
                RunState::Created,
                RunState::Validating,
                RunState::Preparing,
                RunState::Active,
                RunState::Draining,
            ],
            RunState::Cancelled,
        )?;
        Ok((
            run,
            RunCancelled {
                run_id: self.run_id,
            },
        ))
    }

    #[must_use]
    pub const fn run_id(&self) -> RunId {
        self.run_id
    }

    #[must_use]
    pub const fn pipeline_id(&self) -> PipelineId {
        self.pipeline_id
    }

    /// The reason the run failed with; none unless it is Failed.
    #[must_use]
    pub const fn failure(&self) -> Option<&FailureReason> {
        self.failure.as_ref()
    }

    #[must_use]
    pub const fn state(&self) -> RunState {
        self.state
    }
}

/// `inference.graph.NodeInstance`: a node as it participates in one run. It references its run by
/// `run_id` and its node by `node_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct NodeInstance {
    instance_id: NodeInstanceId,
    run_id: RunId,
    node_id: NodeId,
    state: NodeInstanceState,
}

impl NodeInstance {
    /// Runs the `described` outcome of `inference.graph.DescribeNodeInstance` for `node_id` in
    /// `run`: generates a [`NodeInstanceId`] and returns the Described instance and its
    /// [`NodeInstanceDescribed`].
    ///
    /// Whether a node carries `node_id` is decided by a store holding the nodes before it calls
    /// this; a node of another pipeline than the run's is not refused.
    ///
    /// # Errors
    ///
    /// [`RunNotRunning`] when the run is neither Preparing nor Active.
    pub fn describe(
        run: &Run,
        node_id: NodeId,
    ) -> Result<(Self, NodeInstanceDescribed), RunNotRunning> {
        Self::describe_as(NodeInstanceId::generate(), run, node_id)
    }

    /// As [`NodeInstance::describe`], under `instance_id` instead of a generated one.
    ///
    /// # Errors
    ///
    /// [`RunNotRunning`] when the run is neither Preparing nor Active.
    pub fn describe_as(
        instance_id: NodeInstanceId,
        run: &Run,
        node_id: NodeId,
    ) -> Result<(Self, NodeInstanceDescribed), RunNotRunning> {
        if !matches!(run.state, RunState::Preparing | RunState::Active) {
            return Err(RunNotRunning { run_id: run.run_id });
        }
        let instance = Self {
            instance_id,
            run_id: run.run_id,
            node_id,
            state: NodeInstanceState::Described,
        };
        let event = NodeInstanceDescribed {
            instance_id,
            run_id: run.run_id,
            node_id,
        };
        Ok((instance, event))
    }

    #[must_use]
    pub const fn instance_id(&self) -> NodeInstanceId {
        self.instance_id
    }

    #[must_use]
    pub const fn run_id(&self) -> RunId {
        self.run_id
    }

    #[must_use]
    pub const fn node_id(&self) -> NodeId {
        self.node_id
    }

    #[must_use]
    pub const fn state(&self) -> NodeInstanceState {
        self.state
    }
}
