//! Graph execution of the inference framework.
//!
//! `inference-runtime` is to turn a declared pipeline into running nodes and routable streams.
//! Today it holds the edge queue ([`queue`]): an [`EdgeQueue`] built from one
//! `inference.graph.Edge` holds at most the edge's capacity, and the edge's overload policy
//! decides what a full queue does with the next event. Nothing builds the queues of a run yet;
//! the story `node-instance-lifecycle` wires one queue per edge per run. The sender side
//! implements [`OutputEndpoint`](inference_core::endpoint::OutputEndpoint) and the receiver side
//! [`InputEndpoint`](inference_core::endpoint::InputEndpoint), so a node sees an edge only
//! through those traits. No public signature commits a caller to one asynchronous executor.

pub mod queue;

pub use queue::{EdgeQueue, EpochLookup, QueueReceiver, QueueSender, UnsupportedOverload};
