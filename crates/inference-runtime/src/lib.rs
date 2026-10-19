//! Graph execution of the inference framework.
//!
//! `inference-runtime` turns a planned pipeline into running nodes and routable streams. It holds
//! the edge queue ([`queue`]), the run and node instance state machines ([`run`]) and the run
//! controller that drives them together ([`controller`]). An [`EdgeQueue`] built from one
//! `inference.graph.Edge` holds at most the edge's capacity, and the edge's overload policy
//! decides what a full queue does with the next event. The sender side implements
//! [`OutputEndpoint`](inference_core::endpoint::OutputEndpoint) and the receiver side
//! [`InputEndpoint`](inference_core::endpoint::InputEndpoint), so a node sees an edge only through
//! those traits. A [`Run`] is created from the Planned form of a pipeline only and moves Created,
//! Validating, Preparing, Active, Draining, Completed by command, with Failed and Cancelled
//! explicit; a [`NodeInstance`] is described for a run that is Preparing or Active and moves
//! Described, Prepared, Started, Draining or Cancelling, Stopped, with Failed explicit. A
//! [`RunController`] builds one queue per edge and per exposure of the run, hands its caller the
//! outer end of each exposure ([`ExposedEnds`]), drives each node's
//! [`NodeImplementation`](inference_pipeline::NodeImplementation) through its instance as the run
//! moves, and fails the run with a reason naming a node that returned an error. Its run loop,
//! [`RunController::drive`], issues `DrainRun` once every source node has sent its end of input,
//! a control event of kind `stop`, and `CompleteRun` once every instance has drained, and takes
//! the cancel or drain its caller asks for through [`RunRequests`]; a [`QueueWatch`] shows that a
//! run's edge queues are gone once it ended. Every sender a node is handed refuses user content
//! whose sequence does not increase on its stream, and the run records the refusal
//! ([`SequenceNotIncreasing`]). A [`SubgraphNode`] ([`subgraph`]) is the implementation of a
//! subgraph node: it runs its child pipeline as a child run nested under its own instance
//! ([`RunController::create_child`]), bridges its ports to the child's exposures and keeps the run
//! boundary as the cancellation and tracing boundary. No public signature commits a caller to one
//! asynchronous executor.

pub mod controller;
mod drive;
pub mod queue;
pub mod run;
pub mod subgraph;

pub use controller::{
    CreateControllerError, Egress, ExposedEnds, FailNodeError, Ingress, InstanceConflict,
    RunController, RunError,
};
pub use drive::{RunRequests, SequenceNotIncreasing};
pub use inference_core::id::{NodeId, RunId};
pub use queue::{
    Channel, EdgeQueue, EpochLookup, ExposureWatch, QueueReceiver, QueueSender, QueueWatch,
    UnsupportedExposureOverload, UnsupportedOverload,
};
pub use run::{
    NodeInstance, NodeInstanceId, NodeInstanceState, NodeInstanceStateConflict, Run, RunState,
    RunStateConflict,
};
pub use subgraph::{ChildAtStop, ChildImplementations, SubgraphHandle, SubgraphNode};
