//! The one trait a node implementation writes, [`NodeImplementation`]: the lifecycle describe,
//! prepare, start, exchange, drain or cancel, stop of `docs/design/inference.md` line 673, as the
//! runtime drives it for each node instance of a run (`inference.graph.NodeInstance`).
//!
//! The runtime owns the instance's state machine and calls each method once, in the order of the
//! lifecycle: [`describe`](NodeImplementation::describe) when the run describes the instance,
//! [`prepare`](NodeImplementation::prepare) while the run is Preparing,
//! [`start`](NodeImplementation::start) when it activates, handing the node its edge endpoints,
//! then [`drain`](NodeImplementation::drain) when it drains or
//! [`cancel`](NodeImplementation::cancel) when it is cancelled or failed, and
//! [`stop`](NodeImplementation::stop) last. Exchange is what a started node does along its
//! endpoints, not a call of its own. A [`NodeError`] from `prepare`, `start` or `drain` fails the
//! instance, and the runtime fails the run with a reason naming the node and the error; a failed
//! instance is called no further.
//!
//! The endpoints are trait objects of [`InputEndpoint`] and [`OutputEndpoint`] of
//! `inference-core`, so a node names no runtime type, and every method returns a boxed [`Future`]
//! of the standard library, so the trait is dyn-compatible and commits no caller to one
//! asynchronous executor (line 665).

use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;

use inference_core::endpoint::{InputEndpoint, OutputEndpoint};

use crate::{Node, PortId};

/// The future a lifecycle method returns: boxed, `Send`, and borrowing the node for `'a`.
pub type NodeFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The error a node returned from a lifecycle step: an actionable description, which becomes the
/// `detail` of the run's `inference.graph.FailureReason`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NodeError {
    detail: String,
}

impl NodeError {
    /// An error described by `detail`.
    #[must_use]
    pub fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }

    /// The description the node gave.
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for NodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail)
    }
}

impl Error for NodeError {}

/// The receiving end of one edge into an input port of the node.
pub struct NodeInput<P> {
    /// The input port the edge arrives at.
    pub port_id: PortId,
    /// Where the node receives the envelopes of the edge.
    pub endpoint: Box<dyn InputEndpoint<P>>,
}

/// The sending end of one edge out of an output port of the node.
pub struct NodeOutput<P> {
    /// The output port the edge leaves from.
    pub port_id: PortId,
    /// Where the node sends the envelopes of the edge.
    pub endpoint: Box<dyn OutputEndpoint<P>>,
}

/// What a node is handed when it starts: one endpoint per edge of the run that arrives at or
/// leaves from one of its ports, in the declaration order of the edges.
pub struct NodeEndpoints<P> {
    pub inputs: Vec<NodeInput<P>>,
    pub outputs: Vec<NodeOutput<P>>,
}

impl<P> Default for NodeEndpoints<P> {
    fn default() -> Self {
        Self {
            inputs: Vec::new(),
            outputs: Vec::new(),
        }
    }
}

/// A node as the runtime drives it through one run, carrying envelopes of payload `P` along its
/// edges. Each method is called at most once per run, in lifecycle order.
pub trait NodeImplementation<P>: Send {
    /// The node has an instance in the run: `node` is its declaration.
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()>;

    /// Acquires what the node needs to start.
    ///
    /// # Errors
    ///
    /// [`NodeError`] when the node cannot be prepared; the instance fails and so does the run.
    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>>;

    /// Starts the node with its edge endpoints; events flow along them from here.
    ///
    /// # Errors
    ///
    /// [`NodeError`] when the node cannot start; the instance fails and so does the run.
    fn start(&mut self, endpoints: NodeEndpoints<P>) -> NodeFuture<'_, Result<(), NodeError>>;

    /// Admits no new input and completes the events in flight.
    ///
    /// # Errors
    ///
    /// [`NodeError`] when an event in flight fails; the instance fails and so does the run.
    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>>;

    /// Abandons the events in flight; the node is stopped next.
    fn cancel(&mut self) -> NodeFuture<'_, ()>;

    /// Releases what the node holds; the last call of the run.
    fn stop(&mut self) -> NodeFuture<'_, ()>;
}
