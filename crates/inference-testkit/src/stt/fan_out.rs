//! The fan-out of a voice pipeline: a node of role `fan_out` with one input edge and any number
//! of output edges, which hands every envelope its input delivers to each output edge, in the
//! order the runtime handed it the edges. A voice cascade puts it behind its microphone, so that
//! the voice activity detector and the STT node hear the same frames: the detector takes audio
//! and sends only speech activity on (`docs/design/inference.md` 466), so nothing downstream of it
//! carries the frames to the recogniser.
//!
//! The envelope is sent as it arrived, its producer, stream and sequence unchanged, as an edge of
//! its own from the producer would carry it. The end of input, a control event of kind `stop`, is
//! handed on like any envelope and then the node ends; it also ends, sending nothing more, once its
//! input edge is closed or it is cancelled; a cancel taken while a send waits for room ends the
//! wait, and nothing more is sent. An edge that refuses an envelope fails the node.

use std::fmt;
use std::future;
use std::sync::Arc;

use inference_core::envelope::ControlKind;
use inference_core::id::NodeId;
use inference_pipeline::{Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation};

use crate::graph::nodes::{Exchange, Watched, block_on, recv_unless, send_unless};

/// What the fan-out was asked, and what failed it.
#[derive(Debug, Default)]
struct State {
    cancel: bool,
    failure: Option<String>,
}

impl fmt::Debug for Watched<State> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.read(|state| state.fmt(f))
    }
}

/// The fan-out node; see the module documentation.
pub struct FanOut {
    node_id: Option<NodeId>,
    state: Arc<Watched<State>>,
    exchange: Exchange,
}

impl fmt::Debug for FanOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FanOut")
            .field("node_id", &self.node_id)
            .field("state", &self.state)
            .field("exchange", &self.exchange)
            .finish()
    }
}

impl Default for FanOut {
    fn default() -> Self {
        Self::new()
    }
}

impl FanOut {
    /// A fan-out not yet described.
    #[must_use]
    pub fn new() -> Self {
        Self {
            node_id: None,
            state: Arc::new(Watched::new(State::default())),
            exchange: Exchange::new(),
        }
    }

    fn failure(&self) -> Result<(), NodeError> {
        self.state
            .read(|state| state.failure.clone())
            .map_or(Ok(()), |failure| Err(NodeError::new(failure)))
    }
}

impl<P: Clone + Send + 'static> NodeImplementation<P> for FanOut {
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.node_id = Some(node.node_id());
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn start(&mut self, mut endpoints: NodeEndpoints<P>) -> NodeFuture<'_, Result<(), NodeError>> {
        let shaped = match (
            self.node_id,
            endpoints.inputs.len(),
            endpoints.outputs.len(),
        ) {
            (None, _, _) => Err(NodeError::new(
                "the node was started before it was described",
            )),
            (Some(_), 1, outputs) if outputs >= 1 => Ok(()),
            (Some(_), inputs, outputs) => Err(NodeError::new(format!(
                "the node takes 1 input edge and at least 1 output edge and was handed {inputs} \
                 and {outputs}"
            ))),
        };
        if shaped.is_ok() {
            let mut input = endpoints.inputs.remove(0).endpoint;
            let outputs: Vec<_> = endpoints
                .outputs
                .into_iter()
                .map(|output| output.endpoint)
                .collect();
            let state = Arc::clone(&self.state);
            self.exchange.spawn(move || {
                let failure = block_on(async {
                    loop {
                        let Some(Some(envelope)) =
                            recv_unless(input.as_mut(), &state, |state| state.cancel).await
                        else {
                            return None;
                        };
                        let stop = ControlKind::of(&envelope.kind) == Some(ControlKind::Stop);
                        for output in &outputs {
                            let sent = send_unless(
                                output.as_ref(),
                                &state,
                                |state| state.cancel,
                                envelope.clone(),
                            );
                            if let Err(refused) = sent.await {
                                return Some(refused);
                            }
                        }
                        if stop {
                            return None;
                        }
                    }
                });
                state.update(|state| state.failure = failure);
            });
        }
        Box::pin(future::ready(shaped))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(async move {
            self.exchange.finished().await;
            self.exchange.panic()?;
            self.failure()
        })
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        self.state.update(|state| state.cancel = true);
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(self.exchange.finished())
    }
}
