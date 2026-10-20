//! The model node of a voice cascade that can be interrupted: a node of role `model` over an
//! `inference-llm` [`Generator`], the [`MockGenerator`](crate::llm::MockGenerator)'s among them,
//! with two input ports, the committed spans of a commit gate on one and the turn events of the
//! turn policy on the other, its turns port, and one output port carrying its output.
//!
//! It runs one `inference.stream.Generation` per turn (`docs/design/inference.md` 606, 778):
//!
//! * the committed spans that arrive while a turn is Open are held; at the turn's
//!   `TurnCompleted` the node first takes what its input edge already holds, which its producer
//!   sent before the completion, then starts the turn's generation, numbered with the turn's
//!   ordinal among the turns opened in the run as its epoch, emits its `GenerationStarted` and
//!   answers each span in arrival order with the generator's reply to its text, emitting the
//!   `TextDelta` and the `TextSegmentCommitted` of each segment. A turn whose completion finds no
//!   span starts no generation;
//! * the generation stays Active, the turn's current epoch, until the next turn opens. At the
//!   next `TurnOpened`, the interruption (632-641), the node runs
//!   `inference.stream.SupersedeGeneration` on it, which discards every segment of it not yet
//!   Played in the generation's own record and stops its running turn, sends its
//!   `GenerationSuperseded` on, so that every node downstream drops and discards what it holds of
//!   it, and then drops the generation's handle, so that the backend's turn is dropped and its
//!   place among the generator's running generations given back. It records the two apart
//!   ([`Superseded`]): the cancellation of delivery, when the `GenerationSuperseded` was sent, and
//!   the cancellation of computation, when the handle was dropped (641). No node instance changes
//!   state: the supersede is an event of the stream, not the `cancel` of the instance;
//! * a span may arrive before the `TurnOpened` of its turn, which reaches the node on another
//!   edge: every span of a turn is taken at the turn's completion, so a span held when a turn
//!   opens is the new turn's and is kept for it. Spans still held at the end of input belong to a
//!   turn that never completed: they are dropped and recorded ([`AgentHandle::dropped_inputs`]).
//!
//! Once both inputs ended, each by a control event of kind `stop` or by its edge closing, as on a
//! drain, the node completes the Active generation, emits `GenerationCompleted`, passes the end of
//! input on and ends. Cancelled, it cancels the Active generation, recording its
//! `GenerationCancelled` unsent, and sends nothing more; a cancel taken while a send waits for
//! room ends the wait. A turn of the model that fails cancels the generation: the node emits its
//! `GenerationCancelled` and fails with the `llm` error.
//!
//! Every event it emits is recorded in the run's [`RunLog`] in its envelope, which names the
//! event's generation. [`AgentHandle`] reads every generation, the turn it answered and its
//! state, as the node goes.

use std::collections::VecDeque;
use std::fmt;
use std::future;
use std::pin::pin;
use std::sync::{Arc, Weak};
use std::task::{Context, Poll, Waker};
use std::time::Instant;

use inference_core::endpoint::{InputEndpoint, OutputEndpoint};
use inference_core::envelope::{ControlKind, EventEnvelope};
use inference_core::generation::GenerationState;
use inference_core::id::{GenerationId, NodeId, RunId};
use inference_llm::llm_core::TurnRequest;
use inference_llm::{GenerationEvent, GenerationHandle, GenerationRequest, Generator};
use inference_pipeline::{Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation, PortId};
use inference_text::{SegmentSnapshot, TurnId};

use crate::graph::RunLog;
use crate::graph::nodes::{
    Emitter, Exchange, PipelinePayload, Watched, block_on, send_unless, started,
};
use crate::llm::{Carried, ModelPayload};
use crate::stream::StreamEvent;

/// One generation of the node: the turn it answers, its epoch, and its state and segments as
/// its own record holds them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answered {
    pub generation_id: GenerationId,
    pub turn_id: TurnId,
    pub epoch: i64,
    pub state: GenerationState,
    pub segments: Vec<SegmentSnapshot>,
}

/// A generation the node superseded at the opening of the turn `turn_id`: when its delivery was
/// cancelled, its `GenerationSuperseded` sent on, and when its computation was, its handle
/// dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Superseded {
    pub generation_id: GenerationId,
    pub turn_id: TurnId,
    pub delivery: Instant,
    pub computation: Instant,
}

/// What the node was asked, and what its generations came to.
#[derive(Debug, Default)]
struct State {
    cancel: bool,
    generations: Vec<Answered>,
    superseded: Vec<Superseded>,
    dropped: Vec<String>,
    failure: Option<String>,
}

impl fmt::Debug for Watched<State> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.read(|state| state.fmt(f))
    }
}

/// The node; see the module documentation.
pub struct AgentNode {
    run_id: RunId,
    generator: Generator,
    reply: fn(&str) -> String,
    turns: PortId,
    log: RunLog,
    node_id: Option<NodeId>,
    state: Arc<Watched<State>>,
    exchange: Exchange,
}

impl fmt::Debug for AgentNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentNode")
            .field("run_id", &self.run_id)
            .field("generator", &self.generator)
            .field("turns", &self.turns)
            .field("node_id", &self.node_id)
            .field("state", &self.state)
            .field("exchange", &self.exchange)
            .finish_non_exhaustive()
    }
}

impl AgentNode {
    /// The node of the run `run_id` answering on `generator`, whose reply to an input text is
    /// `reply` of it, taking the turn events at its port `turns` and recording what it emits in
    /// `log`. The generator is to claim `concurrent_input_output`.
    #[must_use]
    pub fn new(
        run_id: RunId,
        generator: impl Into<Generator>,
        reply: fn(&str) -> String,
        turns: PortId,
        log: RunLog,
    ) -> Self {
        Self {
            run_id,
            generator: generator.into(),
            reply,
            turns,
            log,
            node_id: None,
            state: Arc::new(Watched::new(State::default())),
            exchange: Exchange::new(),
        }
    }

    /// The test's side of the node.
    #[must_use]
    pub fn handle(&self) -> AgentHandle {
        AgentHandle {
            state: Arc::clone(&self.state),
            task: self.exchange.probe(),
        }
    }

    fn failure(&self) -> Result<(), NodeError> {
        self.state
            .read(|state| state.failure.clone())
            .map_or(Ok(()), |failure| Err(NodeError::new(failure)))
    }
}

/// The test's side of an [`AgentNode`].
#[derive(Debug, Clone)]
pub struct AgentHandle {
    state: Arc<Watched<State>>,
    task: Weak<()>,
}

impl AgentHandle {
    /// Every generation the node started, in the order started, as it stands now.
    #[must_use]
    pub fn generations(&self) -> Vec<Answered> {
        self.state.read(|state| state.generations.clone())
    }

    /// Every generation the node superseded, in the order superseded.
    #[must_use]
    pub fn superseded(&self) -> Vec<Superseded> {
        self.state.read(|state| state.superseded.clone())
    }

    /// The text of every input of a turn that never completed, dropped.
    #[must_use]
    pub fn dropped_inputs(&self) -> Vec<String> {
        self.state.read(|state| state.dropped.clone())
    }

    /// The failure that ended the node's exchange, if one did.
    #[must_use]
    pub fn failure(&self) -> Option<String> {
        self.state.read(|state| state.failure.clone())
    }

    /// A weak reference to the node's exchange task, alive until the task ended.
    #[must_use]
    pub fn task(&self) -> Weak<()> {
        Weak::clone(&self.task)
    }
}

/// `envelope` carrying its payload as `P`.
fn carry<P: ModelPayload>(envelope: EventEnvelope<PipelinePayload>) -> EventEnvelope<P> {
    let EventEnvelope {
        schema_version,
        event_id,
        run_id,
        session_id,
        stream_id,
        sequence,
        producer,
        port,
        kind,
        generation_id,
        timestamp,
        timebase,
        causation_id,
        payload,
    } = envelope;
    EventEnvelope {
        schema_version,
        event_id,
        run_id,
        session_id,
        stream_id,
        sequence,
        producer,
        port,
        kind,
        generation_id,
        timestamp,
        timebase,
        causation_id,
        payload: match payload {
            PipelinePayload::Stream(event) => P::stream(event),
            PipelinePayload::Control(kind) => P::from(kind),
        },
    }
}

/// What the exchange acts on next.
enum Step<P> {
    Cancel,
    Turns(Option<EventEnvelope<P>>),
    Input(Option<EventEnvelope<P>>),
}

/// The generation of the current turn and its place among the recorded generations.
struct Current {
    handle: GenerationHandle,
    index: usize,
}

/// One started exchange of an [`AgentNode`].
struct Exchanging<P> {
    run_id: RunId,
    node_id: NodeId,
    generator: Generator,
    reply: fn(&str) -> String,
    input: Box<dyn InputEndpoint<P>>,
    turns: Box<dyn InputEndpoint<P>>,
    output: Box<dyn OutputEndpoint<P>>,
    emitter: Emitter,
    state: Arc<Watched<State>>,
    current: Option<Current>,
    /// The turn opened last and its ordinal among the turns opened.
    open: Option<(TurnId, i64)>,
    opened: i64,
    /// The texts of the spans of the open turn, in arrival order.
    held: VecDeque<String>,
    input_ended: bool,
    turns_ended: bool,
}

impl<P: ModelPayload> Exchanging<P> {
    /// The next thing to act on: a cancel first, then the turn events, then the input.
    async fn step(&mut self) -> Step<P> {
        let state = Arc::clone(&self.state);
        let (input_ended, turns_ended) = (self.input_ended, self.turns_ended);
        let (input, turns) = (&mut self.input, &mut self.turns);
        future::poll_fn(|cx| {
            let cancelled = pin!(state.until(|state| state.cancel.then_some(())));
            if cancelled.poll(cx).is_ready() {
                return Poll::Ready(Step::Cancel);
            }
            if !turns_ended && let Poll::Ready(received) = turns.recv().as_mut().poll(cx) {
                return Poll::Ready(Step::Turns(received));
            }
            if !input_ended && let Poll::Ready(received) = input.recv().as_mut().poll(cx) {
                return Poll::Ready(Step::Input(received));
            }
            Poll::Pending
        })
        .await
    }

    /// Sends `event` on, recorded in the run's log, unless the node is cancelled first.
    async fn emit(&mut self, event: StreamEvent) -> Result<(), String> {
        let envelope = self
            .emitter
            .stream(event)
            .map_err(|refusal| refusal.to_string())?;
        send_unless(
            self.output.as_ref(),
            &self.state,
            |state| state.cancel,
            carry(envelope),
        )
        .await
    }

    /// Records where the current generation stands.
    fn record(&self) {
        let Some(current) = &self.current else {
            return;
        };
        let output = current.handle.output();
        let state = output.generation().state();
        let segments: Vec<SegmentSnapshot> =
            output.segments().iter().map(|s| s.snapshot()).collect();
        let index = current.index;
        self.state.update(|recorded| {
            if let Some(answered) = recorded.generations.get_mut(index) {
                answered.state = state;
                answered.segments = segments;
            }
        });
    }

    /// Emits `event` of the current generation; a cancellation, of a turn that failed, fails the
    /// node with the `llm` error.
    async fn deliver(&mut self, event: GenerationEvent) -> Result<(), String> {
        let (event, failed) = match event {
            GenerationEvent::Started(started) => (StreamEvent::GenerationStarted(started), false),
            GenerationEvent::TextDelta(delta) => (StreamEvent::TextDelta(delta), false),
            GenerationEvent::SegmentCommitted(committed) => {
                (StreamEvent::TextSegmentCommitted(committed), false)
            }
            GenerationEvent::Completed(completed) => {
                (StreamEvent::GenerationCompleted(completed), false)
            }
            GenerationEvent::Cancelled(cancelled, _) => {
                (StreamEvent::GenerationCancelled(cancelled), true)
            }
        };
        self.emit(event).await?;
        self.record();
        if !failed {
            return Ok(());
        }
        Err(self
            .current
            .as_ref()
            .and_then(|current| current.handle.failure())
            .map_or_else(
                || "the model's turn failed".to_owned(),
                |error| format!("the model's turn failed: {error}"),
            ))
    }

    /// Emits every event of the turns the current generation can run now.
    async fn deliver_turn(&mut self) -> Result<(), String> {
        loop {
            let Some(current) = self.current.as_mut() else {
                return Ok(());
            };
            let Some(event) = current.handle.next_turn_event().await else {
                return Ok(());
            };
            self.deliver(event).await?;
        }
    }

    /// Completes the current generation, if it is Active, and emits its `GenerationCompleted`.
    async fn complete(&mut self) -> Result<(), String> {
        let Some(current) = self.current.as_mut() else {
            return Ok(());
        };
        if current.handle.complete().is_err() {
            self.record();
            return Ok(());
        }
        loop {
            let Some(current) = self.current.as_mut() else {
                return Ok(());
            };
            let Some(event) = current.handle.next_event().await else {
                return Ok(());
            };
            let done = matches!(event, GenerationEvent::Completed(_));
            self.deliver(event).await?;
            if done {
                return Ok(());
            }
        }
    }

    /// Supersedes the current generation at the opening of `turn_id`; see the module
    /// documentation.
    async fn supersede(&mut self, turn_id: TurnId) -> Result<(), String> {
        let Some(current) = self.current.as_mut() else {
            return Ok(());
        };
        let Ok(superseded) = current.handle.supersede() else {
            self.record();
            self.current = None;
            return Ok(());
        };
        self.record();
        self.emit(StreamEvent::GenerationSuperseded(superseded))
            .await?;
        let delivery = Instant::now();
        self.current = None;
        let computation = Instant::now();
        self.state.update(|state| {
            state.superseded.push(Superseded {
                generation_id: superseded.generation_id,
                turn_id,
                delivery,
                computation,
            });
        });
        Ok(())
    }

    /// Takes what the input edge already holds, without waiting.
    fn take_input(&mut self) {
        let mut context = Context::from_waker(Waker::noop());
        while !self.input_ended {
            let Poll::Ready(received) = self.input.recv().as_mut().poll(&mut context) else {
                break;
            };
            self.received(received);
        }
    }

    /// Acts on what the input edge delivered: an envelope, or none once it is closed.
    fn received(&mut self, received: Option<EventEnvelope<P>>) {
        let Some(envelope) = received else {
            self.input_ended = true;
            return;
        };
        match envelope.payload.carried() {
            Carried::Control(ControlKind::Stop) => self.input_ended = true,
            Carried::Stream(StreamEvent::TranscriptCommitted(committed)) => {
                self.held.push_back(committed.text);
            }
            Carried::Stream(_) | Carried::Tool(_) | Carried::Control(_) => {}
        }
    }

    /// The opening of `turn_id`: the interruption of the current generation. The spans held are
    /// the new turn's: every span of the turn before was taken at its completion.
    async fn turn_opened(&mut self, turn_id: TurnId) -> Result<(), String> {
        self.opened += 1;
        self.open = Some((turn_id, self.opened));
        self.supersede(turn_id).await
    }

    /// The completion of the open turn: starts its generation and answers its spans.
    async fn turn_completed(&mut self) -> Result<(), String> {
        self.take_input();
        let Some((turn_id, epoch)) = self.open.take() else {
            return Ok(());
        };
        if self.held.is_empty() {
            return Ok(());
        }
        self.complete().await?;
        self.current = None;
        let request = GenerationRequest {
            run_id: self.run_id,
            node_id: self.node_id,
            epoch,
            turn: TurnRequest::new(
                self.generator.model().provenance().model.as_str(),
                Vec::new(),
            ),
        };
        let handle = self
            .generator
            .start(request)
            .map_err(|refused| refused.to_string())?;
        let generation_id = handle.output().generation().generation_id();
        let index = self.state.update(|state| {
            state.generations.push(Answered {
                generation_id,
                turn_id,
                epoch,
                state: GenerationState::Active,
                segments: Vec::new(),
            });
            state.generations.len() - 1
        });
        self.current = Some(Current { handle, index });
        self.deliver_turn().await?;
        while let Some(text) = self.held.pop_front() {
            let reply = (self.reply)(&text);
            let Some(current) = self.current.as_mut() else {
                break;
            };
            current
                .handle
                .offer_input(reply)
                .map_err(|refused| refused.to_string())?;
            self.deliver_turn().await?;
        }
        Ok(())
    }

    async fn exchanged(&mut self) -> Result<bool, String> {
        while !(self.input_ended && self.turns_ended) {
            match self.step().await {
                Step::Cancel => return Ok(false),
                Step::Turns(None) => self.turns_ended = true,
                Step::Turns(Some(envelope)) => match envelope.payload.carried() {
                    Carried::Stream(StreamEvent::TurnOpened(opened)) => {
                        self.turn_opened(opened.turn_id).await?;
                    }
                    Carried::Stream(StreamEvent::TurnCompleted(_)) => {
                        self.turn_completed().await?;
                    }
                    Carried::Control(ControlKind::Stop) => self.turns_ended = true,
                    Carried::Stream(_) | Carried::Tool(_) | Carried::Control(_) => {}
                },
                Step::Input(received) => self.received(received),
            }
        }
        let dropped: Vec<String> = self.held.drain(..).collect();
        if !dropped.is_empty() {
            self.state.update(|state| state.dropped.extend(dropped));
        }
        self.complete().await?;
        self.current = None;
        let end = carry(self.emitter.control(ControlKind::Stop));
        send_unless(self.output.as_ref(), &self.state, |state| state.cancel, end).await?;
        Ok(true)
    }

    /// Cancels the current generation, recording its `GenerationCancelled` unsent.
    fn cancel(&mut self) {
        let Some(current) = self.current.as_mut() else {
            return;
        };
        if let Ok(cancelled) = current.handle.cancel() {
            let _ = self
                .emitter
                .stream(StreamEvent::GenerationCancelled(cancelled));
        }
        self.record();
        self.current = None;
    }

    fn run(mut self) {
        match block_on(self.exchanged()) {
            Ok(true) => {}
            Ok(false) => self.cancel(),
            Err(failure) => {
                self.record();
                self.state.update(|state| state.failure = Some(failure));
            }
        }
    }
}

impl<P: ModelPayload> NodeImplementation<P> for AgentNode {
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.node_id = Some(node.node_id());
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn start(&mut self, endpoints: NodeEndpoints<P>) -> NodeFuture<'_, Result<(), NodeError>> {
        let started = started(self.node_id, &endpoints, 2, 1).and_then(|node_id| {
            let NodeEndpoints {
                inputs,
                mut outputs,
            } = endpoints;
            let (mut input, mut turns) = (None, None);
            for edge in inputs {
                if edge.port_id == self.turns {
                    turns = Some(edge.endpoint);
                } else {
                    input = Some(edge.endpoint);
                }
            }
            let (Some(input), Some(turns)) = (input, turns) else {
                return Err(NodeError::new(format!(
                    "the node takes one edge at its turns port {} and one at its input",
                    self.turns
                )));
            };
            let output = outputs.remove(0);
            let exchanging = Exchanging {
                run_id: self.run_id,
                node_id,
                generator: self.generator.clone(),
                reply: self.reply,
                input,
                turns,
                emitter: Emitter::new(
                    self.run_id,
                    node_id,
                    output.port_id.to_string(),
                    self.log.clone(),
                ),
                output: output.endpoint,
                state: Arc::clone(&self.state),
                current: None,
                open: None,
                opened: 0,
                held: VecDeque::new(),
                input_ended: false,
                turns_ended: false,
            };
            self.exchange.spawn(move || exchanging.run());
            Ok(())
        });
        Box::pin(future::ready(started))
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
