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
//! [`RunController`] builds one queue per edge of the run, drives each node's
//! [`NodeImplementation`](inference_pipeline::NodeImplementation) through its instance as the run
//! moves, and fails the run with a reason naming a node that returned an error. No public
//! signature commits a caller to one asynchronous executor.

pub mod controller;
pub mod queue;
pub mod run;

pub use controller::{
    CreateControllerError, FailNodeError, InstanceConflict, RunController, RunError,
};
pub use inference_core::id::{NodeId, RunId};
pub use queue::{EdgeQueue, EpochLookup, QueueReceiver, QueueSender, UnsupportedOverload};
pub use run::{
    NodeInstance, NodeInstanceId, NodeInstanceState, NodeInstanceStateConflict, Run, RunState,
    RunStateConflict,
};
