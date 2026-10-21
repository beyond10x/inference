//! The end of a `usage` port no edge leaves and no exposure carries (`story:usage-unconnected-port-counted`).
//!
//! A node's structured output port named [`USAGE_PORT`] carries the usage records of the model
//! calls the node made. When the pipeline gives that port no edge and no exposure, which
//! validation admits for an optional port, the run still hands the node a sending end for it, a
//! sink: what the node sends there is kept, never refused and never waited on, and the caller of
//! the run reads it through [`UnconnectedUsage`], so that the usage counts in the run's total
//! though no edge carries it. The sink answers every send at once: once its reading end is gone,
//! what is sent is dropped, as a caller that takes the run's ends and lets go of them asks for.
//!
//! A subgraph node with no `usage` port has nowhere to send the usage its child run kept in such
//! sinks; [`uncounted_usage`] names every such node of a pipeline, at any depth, so that a caller
//! can mark the total it keeps partial.

use std::collections::VecDeque;
use std::fmt;
use std::future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Poll, Waker};

use inference_core::endpoint::{EndpointFuture, InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::EventEnvelope;
use inference_core::id::NodeId;
use inference_core::model::Modality;
use inference_pipeline::{Declaration, PipelineId, Port, PortDirection, PortId, USAGE_PORT};

/// The caller's end of a `usage` port of the run that no edge leaves and no exposure carries:
/// where it receives the usage records the node `node_id` sends on its port `port_id`, until the
/// node's end is gone. Nothing delivers them: they are the caller's to count.
pub struct UnconnectedUsage<P> {
    pub node_id: NodeId,
    pub port_id: PortId,
    pub endpoint: Box<dyn InputEndpoint<P>>,
}

impl<P> fmt::Debug for UnconnectedUsage<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnconnectedUsage")
            .field("node_id", &self.node_id)
            .field("port_id", &self.port_id)
            .finish_non_exhaustive()
    }
}

/// Whether `port` carries usage records: a structured output port named [`USAGE_PORT`].
pub(crate) fn carries_usage(port: &Port) -> bool {
    port.name() == USAGE_PORT
        && port.direction() == PortDirection::Output
        && port.modality() == Modality::Structured
}

/// The `usage` ports of the nodes of `pipeline_id` that no edge leaves and no exposure carries,
/// each with its node, in the declaration order of the ports.
pub(crate) fn unconnected_ports(
    declaration: &Declaration,
    pipeline_id: PipelineId,
) -> Vec<(NodeId, PortId)> {
    declaration
        .nodes()
        .filter(|node| node.pipeline_id() == pipeline_id)
        .flat_map(|node| {
            declaration
                .ports()
                .filter(move |port| port.node_id() == node.node_id() && carries_usage(port))
        })
        .filter(|port| {
            !declaration
                .edges()
                .any(|edge| edge.from_port() == port.port_id())
                && !declaration
                    .exposures()
                    .any(|exposure| exposure.port_id() == port.port_id())
        })
        .map(|port| (port.node_id(), port.port_id()))
        .collect()
}

/// Every subgraph node of `pipeline_id`, and of the child pipelines of its subgraph nodes at any
/// depth, that has no `usage` port while its child pipeline has a `usage` port no edge leaves and
/// no exposure carries: what the child keeps in that port's sink reaches no run above it. The
/// child's node may send no record there; a node is named for the port it holds, not for what it
/// sent. Every such node is named, several over one child pipeline among them; each child
/// pipeline is searched once, however many nodes run it.
#[must_use]
pub fn uncounted_usage(declaration: &Declaration, pipeline_id: PipelineId) -> Vec<NodeId> {
    let mut uncounted = Vec::new();
    let mut searched = vec![pipeline_id];
    let mut pipelines = vec![pipeline_id];
    while let Some(pipeline_id) = pipelines.pop() {
        for node in declaration
            .nodes()
            .filter(|node| node.pipeline_id() == pipeline_id)
        {
            let Some(child) = node.subgraph_id() else {
                continue;
            };
            let has_usage_port = declaration
                .ports()
                .any(|port| port.node_id() == node.node_id() && carries_usage(port));
            if !has_usage_port && !unconnected_ports(declaration, child).is_empty() {
                uncounted.push(node.node_id());
            }
            if !searched.contains(&child) {
                searched.push(child);
                pipelines.push(child);
            }
        }
    }
    uncounted
}

struct State<P> {
    queue: VecDeque<EventEnvelope<P>>,
    waker: Option<Waker>,
    sending: bool,
    receiving: bool,
}

fn lock<P>(state: &Mutex<State<P>>) -> MutexGuard<'_, State<P>> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The node's end of a sink: every send is answered at once and never refused.
pub(crate) struct SinkSender<P> {
    state: Arc<Mutex<State<P>>>,
}

/// The caller's end of a sink: what the node sent, in order; none once the node's end is gone
/// and nothing more is held.
pub(crate) struct SinkReceiver<P> {
    state: Arc<Mutex<State<P>>>,
}

/// A sink: its sending end for the node and its reading end for the caller.
pub(crate) fn sink<P>() -> (SinkSender<P>, SinkReceiver<P>) {
    let state = Arc::new(Mutex::new(State {
        queue: VecDeque::new(),
        waker: None,
        sending: true,
        receiving: true,
    }));
    (
        SinkSender {
            state: Arc::clone(&state),
        },
        SinkReceiver { state },
    )
}

impl<P: Send> OutputEndpoint<P> for SinkSender<P> {
    fn send(&self, envelope: EventEnvelope<P>) -> EndpointFuture<'_, Result<(), SendError<P>>> {
        let waker = {
            let mut state = lock(&self.state);
            if state.receiving {
                state.queue.push_back(envelope);
            }
            state.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
        Box::pin(future::ready(Ok(())))
    }
}

impl<P> Drop for SinkSender<P> {
    fn drop(&mut self) {
        let waker = {
            let mut state = lock(&self.state);
            state.sending = false;
            state.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

impl<P: Send> InputEndpoint<P> for SinkReceiver<P> {
    fn recv(&mut self) -> EndpointFuture<'_, Option<EventEnvelope<P>>> {
        let state = &self.state;
        Box::pin(future::poll_fn(move |cx| {
            let mut state = lock(state);
            if let Some(envelope) = state.queue.pop_front() {
                Poll::Ready(Some(envelope))
            } else if state.sending {
                state.waker = Some(cx.waker().clone());
                Poll::Pending
            } else {
                Poll::Ready(None)
            }
        }))
    }
}

impl<P> Drop for SinkReceiver<P> {
    fn drop(&mut self) {
        let mut state = lock(&self.state);
        state.receiving = false;
        state.queue.clear();
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::pin;
    use std::sync::Arc;
    use std::task::{Context, Poll, Waker};

    use inference_core::endpoint::OutputEndpoint;
    use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
    use inference_core::id::{NodeId, RunId, StreamId};

    use super::{lock, sink};

    fn stop(sequence: i64) -> EventEnvelope<ControlKind> {
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: RunId::generate(),
            session_id: None,
            stream_id: StreamId::generate(),
            sequence,
            producer: NodeId::generate(),
            port: "usage".to_owned(),
            kind: ControlKind::Stop.as_str().to_owned(),
            generation_id: None,
            timestamp: 0,
            timebase: Timebase::WallTime,
            causation_id: None,
            payload: ControlKind::Stop,
        }
    }

    fn sent(sender: &dyn OutputEndpoint<ControlKind>, sequence: i64) -> bool {
        let mut future = pin!(sender.send(stop(sequence)));
        matches!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Ok(()))
        )
    }

    #[test]
    fn a_send_after_the_reader_is_dropped_is_taken_at_once_and_kept_nowhere() {
        let (sender, receiver) = sink();
        let state = Arc::clone(&sender.state);
        assert!(sent(&sender, 1));
        assert_eq!(
            lock(&state).queue.len(),
            1,
            "a held reader keeps what was sent"
        );
        drop(receiver);
        assert!(
            lock(&state).queue.is_empty(),
            "the dropped reader's backlog goes"
        );
        for sequence in 2..1_000 {
            assert!(sent(&sender, sequence), "a send is never refused");
        }
        assert!(
            lock(&state).queue.is_empty(),
            "nothing sent after the reader is gone is kept"
        );
    }
}
