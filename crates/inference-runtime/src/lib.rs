//! Graph execution of the inference framework.
//!
//! `inference-runtime` is to turn a declared pipeline into running nodes and routable streams.
//! Today it holds the edge queue ([`queue`]) and the run state machine ([`run`]). An [`EdgeQueue`]
//! built from one `inference.graph.Edge` holds at most the edge's capacity, and the edge's
//! overload policy decides what a full queue does with the next event. Nothing builds the queues
//! of a run yet; the story `node-instance-lifecycle` wires one queue per edge per run. The sender
//! side implements [`OutputEndpoint`](inference_core::endpoint::OutputEndpoint) and the receiver
//! side [`InputEndpoint`](inference_core::endpoint::InputEndpoint), so a node sees an edge only
//! through those traits. A [`Run`] is created from the Planned form of a pipeline only and moves
//! Created, Validating, Preparing, Active, Draining, Completed by command, with Failed and
//! Cancelled explicit; a [`NodeInstance`] is described for a run that is Preparing or Active. No
//! public signature commits a caller to one asynchronous executor.

pub mod queue;
pub mod run;

pub use inference_core::id::{NodeId, RunId};
pub use queue::{EdgeQueue, EpochLookup, QueueReceiver, QueueSender, UnsupportedOverload};
pub use run::{NodeInstance, NodeInstanceId, Run, RunState, RunStateConflict};
