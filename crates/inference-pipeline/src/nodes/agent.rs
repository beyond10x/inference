//! The model node of a voice cascade that can be interrupted: a node of role `model` over an
//! `inference-llm` [`Generator`], a mock's among them,
//! with one input port, on which a commit gate sends the committed spans and, each right behind
//! the spans of its turn, the turn events, and one output port carrying its output.
//!
//! It runs one `inference.stream.Generation` per turn (`docs/design/inference.md` 606, 778):
//!
//! * the committed spans that arrive while a turn is Open are held; the edge carries them and
//!   the turn events in one order, and a turn's spans come before its `TurnCompleted`, so at the
//!   `TurnCompleted` the node holds every span of that turn, and may hold spans of a later turn
//!   that are already on the edge. A span whose envelope names a `TurnOpened` as its
//!   `causation_id`, as the STT node's do, belongs to the turn that event opened, and is answered
//!   at that turn's `TurnCompleted` only; a span that names none belongs to the turn open at its
//!   completion. It starts the turn's generation, numbered with the turn's
//!   ordinal among the turns opened in the run as its epoch, emits its `GenerationStarted` and
//!   answers each span in arrival order with the generator's reply to its text, emitting the
//!   `TextDelta` and the `TextSegmentCommitted` of each segment. A generator that claims
//!   `concurrent_input_output` is offered each reply as input to the running generation, which
//!   answers it in a turn of its own; one that does not, such as an adapter claiming output
//!   streaming only, refuses such input, and is asked every reply of the turn, in arrival order,
//!   as the user text of the request the generation starts with, answered in its first turn. A
//!   turn whose completion finds no span starts no generation;
//! * the generation stays Active, the turn's current epoch, until the next turn opens. A
//!   `TurnOpened` while output of the generation is not all Played is the interruption (632-641):
//!   the node runs `inference.stream.SupersedeGeneration` on it, which discards every segment of
//!   it not yet Played in the generation's own record and stops its running turn, sends its
//!   `GenerationSuperseded` on, so that every node downstream drops and discards what it holds of
//!   it, and then drops the generation's handle, so that the backend's turn is dropped and its
//!   place among the generator's running generations given back. It records the two apart
//!   ([`Superseded`]): the cancellation of delivery, when the `GenerationSuperseded` was sent, and
//!   the cancellation of computation, when the handle was dropped (641). No node instance changes
//!   state: the supersede is an event of the stream, not the `cancel` of the instance. A
//!   `TurnOpened` once every segment of the generation was Played interrupts nothing: the node
//!   completes the generation and emits its `GenerationCompleted`. The node learns what was
//!   Played on its playback port ([`AgentNode::with_playback`]), an input edge carrying the
//!   `AudioPlayed` of each segment from the node that acknowledges playback, which it takes ahead
//!   of the input; a node given no playback port learns nothing played, and supersedes its
//!   generation at every `TurnOpened`. A node keeping its replies
//!   ([`AgentNode::keeping_replies`]) supersedes nothing: at every `TurnOpened` it completes the
//!   generation, as it does once every segment was Played;
//! * a span may arrive before the `TurnOpened` of its turn: every span of a turn is taken at the
//!   turn's completion, so a span held when a turn opens is the new turn's and is kept for it.
//!   When a `TurnOpened` arrives while the turn before it is open and was never completed, the
//!   spans held that name the earlier `TurnOpened` are the new turn's: they are re-attributed to
//!   it and answered by its generation, in arrival order, with the spans that name it.
//!   Spans still held at the end of input belong to a turn that never completed: they are dropped
//!   and recorded ([`AgentHandle::dropped_inputs`]).
//!
//! Once the input ended, by a control event of kind `stop` or by its edge closing, as on a
//! drain, the node completes the Active generation, emits `GenerationCompleted` and passes the end
//! of input on; given a playback port, it then goes on taking what arrives there until the end of
//! input there, or until that edge is closed, so that the node sending it is never held; and it
//! ends. Cancelled, it cancels the Active generation, recording its
//! `GenerationCancelled` unsent, and sends nothing more; a cancel taken while a send waits for
//! room ends the wait. A turn of the model that fails cancels the generation: the node emits its
//! `GenerationCancelled` and fails with the `llm` error.
//!
//! Every event it emits is recorded in the run's [`RunLog`] in its envelope, which names the
//! event's generation. It has no usage port and sends no usage record; the usage of a cascade
//! is the [`ModelNode`](super::llm::ModelNode)'s to send. [`AgentHandle`] reads every generation, the turn it answered and its
//! state, as the node goes.

use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::future;
use std::pin::pin;
use std::sync::{Arc, Weak};
use std::task::Poll;
use std::time::Instant;

use crate::{Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation, PortId};
use inference_core::endpoint::{InputEndpoint, OutputEndpoint};
use inference_core::envelope::EventId;
use inference_core::envelope::{ControlKind, EventEnvelope};
use inference_core::generation::GenerationState;
use inference_core::id::{GenerationId, NodeId, RunId};
use inference_core::model::Capability;
use inference_llm::llm_core::{Item, TurnRequest};
use inference_llm::{GenerationEvent, GenerationHandle, GenerationRequest, Generator};
use inference_text::{SegmentId, SegmentSnapshot, TurnId};

use super::llm::{Carried, ModelPayload};
use super::log::{self, RunLog, StreamEmitter, StreamEvent};
use super::tool_authority::send_unless;
use super::{Exchange, Watched, block_on, started};

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
    instructions: String,
    reply: fn(&str) -> String,
    playback: Option<PortId>,
    keeping: bool,
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
            .field("playback", &self.playback)
            .field("node_id", &self.node_id)
            .field("state", &self.state)
            .field("exchange", &self.exchange)
            .finish_non_exhaustive()
    }
}

impl AgentNode {
    /// The node of the run `run_id` answering on `generator`, whose reply to an input text is
    /// `reply` of it, taking the turn events and the committed spans from its one input edge and
    /// recording what it emits in `log`. The generator is to claim `output_streaming`; whether it
    /// claims `concurrent_input_output` decides how a turn's replies reach it (see the module
    /// documentation).
    #[must_use]
    pub fn new(
        run_id: RunId,
        generator: impl Into<Generator>,
        reply: fn(&str) -> String,
        log: RunLog,
    ) -> Self {
        Self {
            run_id,
            generator: generator.into(),
            instructions: String::new(),
            reply,
            playback: None,
            keeping: false,
            log,
            node_id: None,
            state: Arc::new(Watched::new(State::default())),
            exchange: Exchange::new(),
        }
    }

    /// The same node, whose every model request carries `instructions`.
    #[must_use]
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = instructions.into();
        self
    }

    /// The same node given a playback port, `playback`: an edge arriving there carries the
    /// `AudioPlayed` of each segment played; see the module documentation.
    #[must_use]
    pub const fn with_playback(mut self, playback: PortId) -> Self {
        self.playback = Some(playback);
        self
    }

    /// The same node keeping its replies: at a turn's opening it completes the generation of the
    /// turn before and emits its `GenerationCompleted`, as it does once every segment of it was
    /// Played, and never supersedes it, so nothing downstream drops what it holds of the reply.
    #[must_use]
    pub const fn keeping_replies(mut self) -> Self {
        self.keeping = true;
        self
    }

    /// The host's side of the node.
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

/// The host's side of an [`AgentNode`].
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
fn carry<P: ModelPayload>(envelope: EventEnvelope<StreamEvent>) -> EventEnvelope<P> {
    log::carry(envelope, P::stream)
}

/// What the exchange acts on next.
enum Step<P> {
    Cancel,
    Played(Option<EventEnvelope<P>>),
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
    instructions: String,
    reply: fn(&str) -> String,
    input: Box<dyn InputEndpoint<P>>,
    output: Box<dyn OutputEndpoint<P>>,
    emitter: StreamEmitter,
    state: Arc<Watched<State>>,
    current: Option<Current>,
    /// The turn opened last, the envelope of its `TurnOpened` and its ordinal among the turns
    /// opened.
    open: Option<(TurnId, EventId, i64)>,
    opened: i64,
    /// The spans not answered yet, in arrival order: the `TurnOpened` each names, if it does, and
    /// its text.
    held: VecDeque<(Option<EventId>, String)>,
    input_ended: bool,
    /// The edge at the playback port, if the node was given one, until it ended, and every
    /// segment it said was Played.
    playback: Option<Box<dyn InputEndpoint<P>>>,
    played: HashSet<SegmentId>,
    /// Whether the node was given a playback port.
    watches_playback: bool,
    /// Whether a turn opening completes the current generation instead of superseding it.
    keeping: bool,
}

impl<P: ModelPayload> Exchanging<P> {
    /// The next thing to act on: a cancel first, then what was played, then the input.
    async fn step(&mut self) -> Step<P> {
        let state = Arc::clone(&self.state);
        let input_ended = self.input_ended;
        let (input, playback) = (&mut self.input, &mut self.playback);
        future::poll_fn(|cx| {
            let cancelled = pin!(state.until(|state| state.cancel.then_some(())));
            if cancelled.poll(cx).is_ready() {
                return Poll::Ready(Step::Cancel);
            }
            if let Some(playback) = playback.as_mut()
                && let Poll::Ready(received) = playback.recv().as_mut().poll(cx)
            {
                return Poll::Ready(Step::Played(received));
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
            &self.state,
            |state| state.cancel,
            self.output.as_ref(),
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
    /// node with the `llm` error. The node has no usage port, so the usage record of a turn is
    /// not sent.
    async fn deliver(&mut self, event: GenerationEvent) -> Result<(), String> {
        let (event, failed) = match event {
            GenerationEvent::Usage(_) => return Ok(()),
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
            GenerationEvent::Refused(refused) => (StreamEvent::GenerationRefused(refused), false),
            _ => return Ok(()),
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

    /// Acts on what the input edge delivered: an envelope, or none once it is closed.
    async fn received(&mut self, received: Option<EventEnvelope<P>>) -> Result<(), String> {
        let Some(envelope) = received else {
            self.input_ended = true;
            return Ok(());
        };
        let (event_id, cause) = (envelope.event_id, envelope.causation_id);
        match envelope.payload.carried() {
            Carried::Control(ControlKind::Stop) => self.input_ended = true,
            Carried::Stream(StreamEvent::TranscriptCommitted(committed)) => {
                self.held.push_back((cause, committed.text));
            }
            Carried::Stream(StreamEvent::TurnOpened(opened)) => {
                self.turn_opened(opened.turn_id, event_id).await?;
            }
            Carried::Stream(StreamEvent::TurnCompleted(_)) => self.turn_completed().await?,
            Carried::Stream(_) | Carried::Tool(_) | Carried::Usage(_) | Carried::Control(_) => {}
        }
        Ok(())
    }

    /// Acts on what the playback edge delivered: an envelope, or none once it is closed.
    fn played(&mut self, received: Option<EventEnvelope<P>>) {
        let Some(envelope) = received else {
            self.playback = None;
            return;
        };
        match envelope.payload.carried() {
            Carried::Stream(StreamEvent::AudioPlayed(played)) => {
                self.played.insert(played.segment_id);
            }
            Carried::Control(ControlKind::Stop) => self.playback = None,
            Carried::Stream(_) | Carried::Tool(_) | Carried::Usage(_) | Carried::Control(_) => {}
        }
    }

    /// Whether every segment of the current generation was Played, as the playback port said;
    /// never without a playback port.
    fn all_played(&self) -> bool {
        let (Some(current), true) = (&self.current, self.watches_playback) else {
            return false;
        };
        current
            .handle
            .output()
            .segments()
            .iter()
            .all(|segment| self.played.contains(&segment.segment_id()))
    }

    /// The opening of `turn_id`: the interruption of the current generation while output of it is
    /// not all Played, and otherwise, or for a node keeping its replies, its completion. The spans
    /// held are the new turn's: every span of the turn before was taken at its completion.
    async fn turn_opened(&mut self, turn_id: TurnId, event_id: EventId) -> Result<(), String> {
        if let Some((_, old, _)) = self.open {
            for (cause, _) in &mut self.held {
                if *cause == Some(old) {
                    *cause = Some(event_id);
                }
            }
        }
        self.opened += 1;
        self.open = Some((turn_id, event_id, self.opened));
        if self.keeping || self.all_played() {
            self.complete().await?;
            self.current = None;
            return Ok(());
        }
        self.supersede(turn_id).await
    }

    /// The completion of the open turn: starts its generation and answers its spans, which are
    /// the ones the edge carried before the completion.
    async fn turn_completed(&mut self) -> Result<(), String> {
        let Some((turn_id, opened_by, epoch)) = self.open.take() else {
            return Ok(());
        };
        let (mut held, later): (VecDeque<_>, VecDeque<_>) = std::mem::take(&mut self.held)
            .into_iter()
            .partition(|(cause, _)| cause.is_none_or(|cause| cause == opened_by));
        self.held = later;
        if held.is_empty() {
            return Ok(());
        }
        self.complete().await?;
        self.current = None;
        let concurrent = self
            .generator
            .adapter()
            .capabilities()
            .contains(&Capability::ConcurrentInputOutput);
        let asked: Vec<Item> = if concurrent {
            Vec::new()
        } else {
            held.drain(..)
                .map(|(_, text)| Item::user((self.reply)(&text)))
                .collect()
        };
        let mut turn = TurnRequest::new(self.generator.model().provenance().model.as_str(), asked);
        turn.instructions.clone_from(&self.instructions);
        let request = GenerationRequest {
            run_id: self.run_id,
            node_id: self.node_id,
            epoch,
            turn,
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
        while let Some((_, text)) = held.pop_front() {
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
        while !self.input_ended {
            match self.step().await {
                Step::Cancel => return Ok(false),
                Step::Input(received) => self.received(received).await?,
                Step::Played(received) => self.played(received),
            }
        }
        let dropped: Vec<String> = self.held.drain(..).map(|(_, text)| text).collect();
        if !dropped.is_empty() {
            self.state.update(|state| state.dropped.extend(dropped));
        }
        self.complete().await?;
        self.current = None;
        let end = self.emitter.control(ControlKind::Stop);
        send_unless(&self.state, |state| state.cancel, self.output.as_ref(), end).await?;
        while self.playback.is_some() {
            match self.step().await {
                Step::Cancel => return Ok(false),
                Step::Played(received) => self.played(received),
                Step::Input(_) => {}
            }
        }
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
        let inputs_wanted = 1 + usize::from(self.playback.is_some());
        let started = started(self.node_id, &endpoints, inputs_wanted, 1).and_then(|node_id| {
            let NodeEndpoints {
                inputs,
                mut outputs,
            } = endpoints;
            let (mut input, mut playback) = (None, None);
            for edge in inputs {
                if Some(edge.port_id) == self.playback {
                    playback = Some(edge.endpoint);
                } else {
                    input = Some(edge.endpoint);
                }
            }
            let (Some(input), true) = (input, playback.is_some() == self.playback.is_some()) else {
                return Err(NodeError::new(
                    "the node takes one edge at its playback port if it was given one, and one \
                     at its input",
                ));
            };
            let output = outputs.remove(0);
            let exchanging = Exchanging {
                run_id: self.run_id,
                node_id,
                generator: self.generator.clone(),
                instructions: self.instructions.clone(),
                reply: self.reply,
                input,
                emitter: StreamEmitter::new(
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
                watches_playback: playback.is_some(),
                keeping: self.keeping,
                playback,
                played: HashSet::new(),
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
