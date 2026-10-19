//! The mock model node of a text-only pipeline: a node of role `model` whose implementation is a
//! [`MockGenerator`] claiming `output_streaming` and `concurrent_input_output`.
//!
//! When it starts, the node starts one `inference.stream.Generation` of the run on its generator
//! and emits its `GenerationStarted`. For each `inference.stream.TranscriptCommitted` its input
//! edge delivers it offers the generator its reply to the text and emits the `TextDelta` and the
//! `TextSegmentCommitted` of the one segment the generator answers with. On the end of input, a
//! control event of kind `stop`, it completes the generation, emits `GenerationCompleted` and
//! passes the end of input on. Drained, it takes what its input edge still holds until the edge
//! is closed and then completes the generation the same way. Cancelled, it cancels the
//! generation, which discards its unplayed segments, and sends nothing more; scripted to emit
//! after the cancel, it emits once into the cancelled generation, which refuses it as stale.
//! Every event it emits is recorded in the run's [`RunLog`] in its envelope, the
//! `GenerationCancelled` that is not sent among them.

use std::fmt;
use std::future;
use std::sync::{Arc, Weak};

use inference_core::endpoint::{InputEndpoint, OutputEndpoint};
use inference_core::envelope::ControlKind;
use inference_core::generation::{GenerationState, GenerationStateConflict};
use inference_core::id::{NodeId, RunId};
use inference_llm::{GenerationEvent, GenerationHandle, GenerationRequest, Generator, OfferInput};
use inference_pipeline::{Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation};
use inference_text::SegmentSnapshot;

use super::{MockGeneration, MockGenerator};
use crate::graph::nodes::{
    Emitter, Exchange, PipelinePayload, RunLog, Watched, block_on, emit, recv_unless, started,
};
use crate::stream::StreamEvent;

/// What the node was asked to do by the runtime.
#[derive(Debug, Default)]
struct Signal {
    drain: bool,
    cancel: bool,
}

/// What the node's generation came to.
#[derive(Debug, Default)]
struct Record {
    state: Option<GenerationState>,
    segments: Vec<SegmentSnapshot>,
    late_refusal: Option<GenerationStateConflict>,
    failure: Option<String>,
}

impl fmt::Debug for Watched<Signal> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.read(|signal| signal.fmt(f))
    }
}

impl fmt::Debug for Watched<Record> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.read(|record| record.fmt(f))
    }
}

/// How the exchange ended.
enum Ended {
    Completed,
    Cancelled,
    Failed(String),
}

/// The mock model node; see the module documentation.
#[derive(Debug)]
pub struct ModelNode {
    run_id: RunId,
    generator: MockGenerator,
    reply: fn(&str) -> String,
    late: Option<String>,
    log: RunLog,
    node_id: Option<NodeId>,
    signal: Arc<Watched<Signal>>,
    record: Arc<Watched<Record>>,
    exchange: Exchange,
}

impl ModelNode {
    /// A model node of the run `run_id` answering on `generator`, whose reply to an input text is
    /// `reply` of it, recording what it emits in `log`. The generator is to claim
    /// `concurrent_input_output`: input reaches its running generation only through admission.
    #[must_use]
    pub fn new(
        run_id: RunId,
        generator: MockGenerator,
        reply: fn(&str) -> String,
        log: RunLog,
    ) -> Self {
        Self {
            run_id,
            generator,
            reply,
            late: None,
            log,
            node_id: None,
            signal: Arc::new(Watched::new(Signal::default())),
            record: Arc::new(Watched::new(Record::default())),
            exchange: Exchange::new(),
        }
    }

    /// The same node, which emits `text` into its generation once it was cancelled.
    #[must_use]
    pub fn emitting_after_cancel(mut self, text: impl Into<String>) -> Self {
        self.late = Some(text.into());
        self
    }

    /// The test's side of the node.
    #[must_use]
    pub fn handle(&self) -> ModelHandle {
        ModelHandle {
            record: Arc::clone(&self.record),
            task: self.exchange.probe(),
        }
    }

    fn failure(&self) -> Result<(), NodeError> {
        self.record
            .read(|record| record.failure.clone())
            .map_or(Ok(()), |failure| Err(NodeError::new(failure)))
    }
}

/// One started exchange of a [`ModelNode`].
struct Exchanging {
    generation: MockGeneration,
    reply: fn(&str) -> String,
    late: Option<String>,
    signal: Arc<Watched<Signal>>,
    record: Arc<Watched<Record>>,
    input: Box<dyn InputEndpoint<PipelinePayload>>,
    output: Box<dyn OutputEndpoint<PipelinePayload>>,
    emitter: Emitter,
}

impl Exchanging {
    /// Emits the generation's events until `last` is delivered, or until none is.
    async fn deliver(&mut self, last: fn(&GenerationEvent) -> bool) -> Result<(), String> {
        for event in self.generation.by_ref() {
            let done = last(&event);
            let event = match event {
                GenerationEvent::Started(started) => StreamEvent::GenerationStarted(started),
                GenerationEvent::TextDelta(delta) => StreamEvent::TextDelta(delta),
                GenerationEvent::SegmentCommitted(committed) => {
                    StreamEvent::TextSegmentCommitted(committed)
                }
                GenerationEvent::Completed(completed) => {
                    StreamEvent::GenerationCompleted(completed)
                }
            };
            emit(self.output.as_ref(), &mut self.emitter, event).await?;
            if done {
                break;
            }
        }
        Ok(())
    }

    /// Completes the generation and emits `GenerationCompleted`.
    async fn complete(&mut self) -> Result<(), String> {
        self.deliver(|event| matches!(event, GenerationEvent::Completed(_)))
            .await
    }

    async fn exchange(&mut self) -> Ended {
        if let Err(failure) = self
            .deliver(|event| matches!(event, GenerationEvent::Started(_)))
            .await
        {
            return Ended::Failed(failure);
        }
        loop {
            let received =
                recv_unless(self.input.as_mut(), &self.signal, |signal| signal.cancel).await;
            let answered = match received {
                None => return Ended::Cancelled,
                Some(Some(envelope)) => match envelope.payload {
                    PipelinePayload::Stream(StreamEvent::TranscriptCommitted(committed)) => {
                        match self.generation.offer_input((self.reply)(&committed.text)) {
                            Ok(()) => {
                                self.deliver(|event| {
                                    matches!(event, GenerationEvent::SegmentCommitted(_))
                                })
                                .await
                            }
                            Err(refused) => Err(refused.to_string()),
                        }
                    }
                    PipelinePayload::Control(ControlKind::Stop) => {
                        let ended = match self.complete().await {
                            Ok(()) => {
                                let end = self.emitter.control(ControlKind::Stop);
                                self.output.send(end).await.map_or_else(
                                    |refused| Ended::Failed(refused.to_string()),
                                    |()| Ended::Completed,
                                )
                            }
                            Err(failure) => Ended::Failed(failure),
                        };
                        return ended;
                    }
                    PipelinePayload::Stream(_) | PipelinePayload::Control(_) => Ok(()),
                },
                Some(None) => {
                    let cancel = self
                        .signal
                        .until(|signal| (signal.cancel || signal.drain).then_some(signal.cancel))
                        .await;
                    if cancel {
                        return Ended::Cancelled;
                    }
                    return match self.complete().await {
                        Ok(()) => Ended::Completed,
                        Err(failure) => Ended::Failed(failure),
                    };
                }
            };
            if let Err(failure) = answered {
                return Ended::Failed(failure);
            }
        }
    }

    /// Cancels the generation, recording its `GenerationCancelled` unsent, and emits the late
    /// text into it if the node was scripted to.
    fn cancel(&mut self) -> Option<GenerationStateConflict> {
        if let Ok(cancelled) = self.generation.cancel() {
            let _ = self
                .emitter
                .stream(StreamEvent::GenerationCancelled(cancelled));
        }
        let late = self.late.take()?;
        self.generation.emit(late).err()
    }

    fn run(mut self) {
        let ended = block_on(self.exchange());
        let (late_refusal, failure) = match ended {
            Ended::Completed => (None, None),
            Ended::Cancelled => (self.cancel(), None),
            Ended::Failed(failure) => (None, Some(failure)),
        };
        let output = self.generation.output();
        let state = output.generation().state();
        let segments = output.segments().iter().map(|s| s.snapshot()).collect();
        self.record.update(|record| {
            record.state = Some(state);
            record.segments = segments;
            record.late_refusal = late_refusal;
            record.failure = failure;
        });
    }
}

impl NodeImplementation<PipelinePayload> for ModelNode {
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.node_id = Some(node.node_id());
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn start(
        &mut self,
        mut endpoints: NodeEndpoints<PipelinePayload>,
    ) -> NodeFuture<'_, Result<(), NodeError>> {
        let started = started(self.node_id, &endpoints, 1, 1).and_then(|node_id| {
            let generation = self
                .generator
                .start(GenerationRequest {
                    run_id: self.run_id,
                    node_id,
                    epoch: 1,
                    prompt: String::new(),
                })
                .map_err(|refused| NodeError::new(refused.to_string()))?;
            let output = endpoints.outputs.remove(0);
            let exchanging = Exchanging {
                generation,
                reply: self.reply,
                late: self.late.clone(),
                signal: Arc::clone(&self.signal),
                record: Arc::clone(&self.record),
                input: endpoints.inputs.remove(0).endpoint,
                emitter: Emitter::new(
                    self.run_id,
                    node_id,
                    output.port_id.to_string(),
                    self.log.clone(),
                ),
                output: output.endpoint,
            };
            self.exchange.spawn(move || exchanging.run());
            Ok(())
        });
        Box::pin(future::ready(started))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        self.signal.update(|signal| signal.drain = true);
        Box::pin(async move {
            self.exchange.finished().await;
            self.failure()
        })
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        self.signal.update(|signal| signal.cancel = true);
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(self.exchange.finished())
    }
}

/// The test's side of a [`ModelNode`]: what its generation came to once its exchange ended.
#[derive(Debug, Clone)]
pub struct ModelHandle {
    record: Arc<Watched<Record>>,
    task: Weak<()>,
}

impl ModelHandle {
    /// The state the generation ended in; none before the exchange ended.
    #[must_use]
    pub fn generation_state(&self) -> Option<GenerationState> {
        self.record.read(|record| record.state)
    }

    /// The segments the generation owned when the exchange ended, in emission order.
    #[must_use]
    pub fn segments(&self) -> Vec<SegmentSnapshot> {
        self.record.read(|record| record.segments.clone())
    }

    /// The refusal of the text the node emitted after the cancel, if it was scripted to.
    #[must_use]
    pub fn late_refusal(&self) -> Option<GenerationStateConflict> {
        self.record.read(|record| record.late_refusal)
    }

    /// A weak reference to the node's exchange task, alive until the task ended.
    #[must_use]
    pub fn task(&self) -> Weak<()> {
        Weak::clone(&self.task)
    }
}
