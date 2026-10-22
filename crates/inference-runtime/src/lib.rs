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
//! outer end of each exposure ([`ExposedEnds`]), and the reading end of each `usage` port no edge
//! leaves and no exposure carries ([`UnconnectedUsage`], [`usage_sink`]), drives each node's
//! [`NodeImplementation`](inference_pipeline::NodeImplementation) through its instance as the run
//! moves, and fails the run with a reason naming a node that returned an error. Its run loop,
//! [`RunController::drive`], issues `DrainRun` once every source node has sent its end of input,
//! a control event of kind `stop`, and `CompleteRun` once every instance has drained, and takes
//! the cancel, drain or failure its caller asks for through [`RunRequests`]; a [`QueueWatch`]
//! shows that a run's edge queues are gone once it ended. On Linux, the egress watch (`egress`)
//! fails a run whose pipeline profile denies egress once a socket of the process is connected to
//! an address that is neither loopback nor unspecified; it is a tripwire, not a sandbox. Every sender a node is handed refuses user content
//! whose sequence does not increase on its stream, and the run records the refusal
//! ([`SequenceNotIncreasing`]). A [`SubgraphNode`] ([`subgraph`]) is the implementation of a
//! subgraph node: it runs its child pipeline as a child run nested under its own instance
//! ([`RunController::create_child`]), bridges its ports to the child's exposures and keeps the run
//! boundary as the cancellation and tracing boundary. A [`Recorder`](record::Recorder)
//! ([`record`]) sits on a run's event path, wrapping its nodes and its ingresses, and records every
//! envelope with the merge order it assigns, and the control decisions among them; a
//! [`Replay`](replay::Replay) ([`replay`]) feeds a recording's inputs into a new run of the same
//! pipeline and reports the first control decision that diverged; a
//! [`TelemetrySink`](telemetry::TelemetrySink) ([`telemetry`]) takes telemetry on a bounded, lossy
//! channel that drops and counts what a slow consumer has no room for. No public signature
//! commits a caller to one asynchronous executor; [`Replay::feed`](replay::Replay::feed) blocks
//! the calling thread.

pub mod controller;
mod drive;
#[cfg(target_os = "linux")]
pub mod egress;
pub mod queue;
pub mod record;
pub mod replay;
pub mod run;
pub mod subgraph;
pub mod telemetry;
pub mod usage_sink;

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
pub use usage_sink::{UnconnectedUsage, uncounted_usage};
