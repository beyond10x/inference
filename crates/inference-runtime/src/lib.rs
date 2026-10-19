//! Graph execution of the inference framework.
//!
//! `inference-runtime` turns a declared pipeline into running nodes and routable streams. Its
//! first module is the edge queue ([`queue`]): every `inference.graph.Edge` of a run becomes one
//! [`EdgeQueue`] holding at most the edge's capacity, whose overload policy decides what a full
//! queue does with the next event. The sender side implements
//! [`OutputEndpoint`](inference_core::endpoint::OutputEndpoint) and the receiver side
//! [`InputEndpoint`](inference_core::endpoint::InputEndpoint), so a node sees an edge only
//! through those traits. No public signature commits a caller to one asynchronous executor.

pub mod queue;

pub use queue::{EdgeQueue, EpochLookup, QueueReceiver, QueueSender, UnsupportedOverload};
