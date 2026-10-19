//! The two ends of an edge as a node sees them: [`OutputEndpoint`], where a node sends the
//! [`EventEnvelope`]s of one output port, and [`InputEndpoint`], where it receives those of one
//! input port.
//!
//! An edge of a pipeline is a bounded channel (`inference.graph.Edge`); the runtime's edge queue
//! implements both traits, its sender side for the producing node and its receiver side for the
//! consuming node. The runtime's run controller builds one queue per edge of a run and hands
//! each node its endpoints when it starts. The traits are declared here,
//! beside the envelope they carry, so that a node implementation names an endpoint without
//! depending on the runtime that implements it.
//! Both are dyn-compatible, so a node receives them as trait objects, and both return a boxed
//! [`Future`] of the standard library, so that neither commits a caller to one asynchronous
//! executor.
//!
//! A send that the edge cannot accept returns the envelope to the producer in a [`SendError`]:
//! under the overload policy `fail` when the edge is full, and under every policy when the
//! receiving side is gone. What a full edge does under the other policies (wait, discard,
//! coalesce, drop stale epochs) is the implementation's and is not visible in these signatures.

use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;

use crate::envelope::EventEnvelope;

/// The future an endpoint returns: boxed, `Send`, and borrowing the endpoint for `'a`.
pub type EndpointFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The sending end of an edge, held by the node that produces on its output port; `Sync`, so one
/// endpoint can be shared across the tasks of a multi-threaded executor.
pub trait OutputEndpoint<P>: Send + Sync {
    /// Sends `envelope` along the edge. The future completes once the edge has taken the
    /// envelope, discarded it under its overload policy, or refused it.
    ///
    /// # Errors
    ///
    /// [`SendError`] carrying `envelope` when the edge refused it: [`SendError::Full`] when the
    /// edge is full under the overload policy `fail`, [`SendError::Closed`] when the receiving
    /// side is gone.
    fn send(&self, envelope: EventEnvelope<P>) -> EndpointFuture<'_, Result<(), SendError<P>>>;
}

/// The receiving end of an edge, held by the node that consumes on its input port.
pub trait InputEndpoint<P>: Send {
    /// Receives the next envelope of the edge, waiting while the edge is empty. Completes with
    /// `None` once the edge is empty and every sending side is gone, so nothing more can arrive.
    fn recv(&mut self) -> EndpointFuture<'_, Option<EventEnvelope<P>>>;
}

/// Why an edge refused an envelope. Each variant carries the refused envelope, so the producer
/// loses nothing it sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendError<P> {
    /// The edge is full and its overload policy is `fail`: deliver or return an explicit
    /// failure, never silently discard.
    Full(EventEnvelope<P>),
    /// The receiving side is gone; nothing sent along the edge can be delivered.
    Closed(EventEnvelope<P>),
}

impl<P> SendError<P> {
    /// The refused envelope.
    #[must_use]
    pub fn into_envelope(self) -> EventEnvelope<P> {
        match self {
            Self::Full(envelope) | Self::Closed(envelope) => envelope,
        }
    }

    /// The refused envelope, borrowed.
    #[must_use]
    pub const fn envelope(&self) -> &EventEnvelope<P> {
        match self {
            Self::Full(envelope) | Self::Closed(envelope) => envelope,
        }
    }
}

impl<P> fmt::Display for SendError<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full(envelope) => write!(
                f,
                "the edge is full and fails on overload; event {} was not sent",
                envelope.event_id
            ),
            Self::Closed(envelope) => write!(
                f,
                "the receiving side of the edge is gone; event {} was not sent",
                envelope.event_id
            ),
        }
    }
}

impl<P: fmt::Debug> Error for SendError<P> {}
