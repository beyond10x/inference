//! The mock model node of a text-only pipeline: a node of role `model` whose implementation is an
//! `inference-llm` [`Generator`] over an `llm` model, such as the
//! [`MockGenerator`](super::MockGenerator)'s, claiming `output_streaming` and
//! `concurrent_input_output`.
//!
//! When it starts, the node starts one `inference.stream.Generation` of the run on its generator
//! and emits its `GenerationStarted`, on a request carrying no items and the tools it was given,
//! if any; no turn runs before the first input. For each `inference.stream.TranscriptCommitted`
//! its input edge delivers it offers the generation its reply to the text, which the next turn
//! asks as user text, and emits the `TextDelta` and the `TextSegmentCommitted` of every segment
//! the model answers with in that turn. A turn that fails cancels the generation: the node emits
//! its `GenerationCancelled` and fails with the `llm` error, since its one generation of the run
//! can answer nothing more, and downstream learns that the segments it holds are cut. On the end
//! of input, a control event of kind `stop`, it completes the generation, emits
//! `GenerationCompleted` and passes the end of input on. Drained, it takes what its input edge still holds until the edge
//! is closed and then completes the generation the same way. Cancelled, it cancels the
//! generation, which discards its unplayed segments, and sends nothing more, a cancel taken while
//! a send waits for room ending the wait; scripted to emit after the cancel, it emits once into
//! the cancelled generation, which refuses it as stale.
//! Every event it emits is recorded in the run's [`RunLog`] in its envelope, the
//! `GenerationCancelled` that is not sent among them. Each envelope of a generation's event names
//! the generation, a `TextSegmentCommitted`'s too, whose payload names only its segment, so that
//! a reader of the envelope, such as a session cutting an interrupted generation, cuts the
//! segment with it. The node runs one generation per run, and only that generation's events name
//! it: its end of input names none.
//!
//! The node is generic over the payload of the run's edges ([`ModelPayload`]), the text
//! pipeline's [`PipelinePayload`] by default. Given tools ([`ModelNode::with_tools`]), it has a
//! `tool_call` output port and a `tool_result` input port, both Structured (design 581-584), and
//! runs the tool loop of its generation:
//!
//! * once the turn an input or a round of outcomes ran has delivered its segment, each `llm`
//!   tool call it ended with becomes `inference.tools.ProposeToolCall` of the generation, of the
//!   tool [`tool_id`] names, resting on the latest span the node generated from: the span of the
//!   last `TranscriptCommitted`, which that event says is Committed. The proposal is sent on
//!   `tool_call` and the generation awaits its outcome. A call naming a tool the node was not
//!   given is dropped: the node answers it at once as rejected `denied`, as `llm` requires every
//!   call of a round answered before the next turn, and the generation continues without it. The
//!   node generates from committed spans only, so the provisional-input barrier never bars its
//!   proposals; a proposal the state machine refuses all the same fails the node;
//! * one round is awaited at a time: an input arriving while a call is awaited is deferred, and
//!   answered, in arrival order, once the outcomes are integrated and no call is awaited, so a
//!   tool result is integrated at the end of the turn that proposed and no outcome is lost to a
//!   later proposal;
//! * an outcome of an awaited call arriving on `tool_result`, a result, a failure or a rejection,
//!   is integrated into the same generation as an `llm` tool result
//!   ([`ToolOutcome`](inference_llm::tools::ToolOutcome)), and once the last call of the round is
//!   answered, the next turn runs on them (design 402, 721), so every segment after it carries
//!   the generation's identity;
//! * a rejection `exhausted` is the loop's termination rule: the node integrates nothing more and
//!   completes the generation at once, and drops every later proposal. The node runs one
//!   generation per run, so an input that arrives once the generation is no longer Active is
//!   dropped and recorded ([`ModelHandle::dropped_inputs`]), and the node does not fail;
//! * a turn may end with several calls, the calls of one round
//!   ([`MockGenerator::proposing_round`](super::MockGenerator::proposing_round)): each is sent, in
//!   order, and the generation awaits every one of them; each outcome is integrated as it arrives,
//!   and the next turn runs once the last of them did;
//! * superseded through its handle ([`ModelHandle::supersede`]), the node runs
//!   `inference.stream.SupersedeGeneration` on its generation, which then emits nothing more, and
//!   sends its `GenerationSuperseded` on the text port and, given tools, on `tool_call`, so that
//!   the tool authority rejects the calls of the generation it still holds and the executor is told
//!   to cancel the one it runs. An outcome that arrives for a generation that is no longer Active,
//!   superseded or completed, is not reinjected and runs no turn (design 376, 637); it
//!   is recorded ([`ModelHandle::not_reinjected`]) and still ends the wait for its call;
//! * the end of input, or a drain once the input edge is closed, completes the generation only
//!   once no outcome is awaited and no input is deferred; the node then passes the end of input
//!   on, on both output ports, and ends once every edge into `tool_result` has ended, so that what
//!   the loop's other nodes send at their end still finds it.
//!
//! Every other step of a tool call that arrives, such as `inference.tools.ToolCallStarted`, is
//! taken and ignored. A node given tools offers them, in the order it was given them, on the
//! request it starts its generation with, as `llm` tool specifications ([`tool_spec`]).
//!
//! Given a turns port ([`ModelNode::with_turns`]) at which an edge arrives, the node starts its
//! generation for a turn at that turn's `inference.stream.TurnCompleted` (design 606, 778) rather
//! than when it starts; without such an edge, as in a text-only pipeline, which has no turns, it
//! starts as above. Inputs arriving before a turn's completion are held, and each
//! `TurnCompleted` arriving at the turns port first takes what the node's input already holds,
//! which its producer sent before the completion, then starts the generation if it has not
//! started and answers the turn's inputs in arrival order, as deferred inputs are answered. The
//! node still runs one generation per run: a later turn's inputs are answered on the same
//! generation. The end of input completes the generation only once every edge into the turns
//! port has ended; inputs of a turn that never completed are then dropped and recorded, and a
//! node whose turn never completed ends without a generation. A supersede asked before the
//! generation started does nothing.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::future;
use std::pin::pin;
use std::sync::{Arc, Weak};
use std::task::{Context, Poll, Waker};
use std::time::{SystemTime, UNIX_EPOCH};

use inference_core::endpoint::{EndpointFuture, InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::generation::{GenerationState, GenerationStateConflict};
use inference_core::id::{NodeId, RunId, StreamId};
use inference_llm::llm_core::{CallId, TurnRequest};
use inference_llm::tools::{ToolOutcome, tool_id, tool_spec};
use inference_llm::{GenerationEvent, GenerationHandle, GenerationRequest, Generator};
use inference_pipeline::nodes::{ToolCallEvent, ToolCallUpdate, ToolPayload};
use inference_pipeline::{Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation, PortId};
use inference_text::{OpenTurn, SegmentSnapshot, Span, TranscriptCommitted, Turn};
use inference_tools::{RejectionReason, Tool, ToolCall, ToolCallId, ToolId};

use crate::graph::nodes::{
    Emitter, Exchange, PipelinePayload, RunLog, Watched, block_on, emit, send_unless, started,
};
use crate::stream::StreamEvent;

/// What a payload of the edges around a [`ModelNode`] carries, as the node reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Carried {
    /// An `inference.stream` event.
    Stream(StreamEvent),
    /// A step of a tool call.
    Tool(ToolCallUpdate),
    /// A control event.
    Control(ControlKind),
}

/// The payload of the edges around a [`ModelNode`]: `inference.stream` events and control events,
/// and, for a node given tools, the steps of tool calls, which such a payload also carries as a
/// [`ToolPayload`].
pub trait ModelPayload: From<ControlKind> + Send + 'static {
    /// The payload carrying `event`.
    fn stream(event: StreamEvent) -> Self;

    /// What the payload carries.
    fn carried(self) -> Carried;
}

impl ModelPayload for PipelinePayload {
    fn stream(event: StreamEvent) -> Self {
        Self::Stream(event)
    }

    fn carried(self) -> Carried {
        match self {
            Self::Stream(event) => Carried::Stream(event),
            Self::Control(kind) => Carried::Control(kind),
        }
    }
}

/// What the node was asked to do by the runtime.
#[derive(Debug, Default)]
struct Signal {
    drain: bool,
    cancel: bool,
    /// The host asked for `SupersedeGeneration`: whether it did, and whether the node took it.
    supersede: bool,
    superseded: bool,
}

/// What the node's generation came to.
#[derive(Debug, Default)]
struct Record {
    state: Option<GenerationState>,
    segments: Vec<SegmentSnapshot>,
    late_refusal: Option<GenerationStateConflict>,
    failure: Option<String>,
    dropped: Vec<String>,
    not_reinjected: Vec<ToolCallUpdate>,
    supersede_refusal: Option<GenerationStateConflict>,
    requests: usize,
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

/// The tools a [`ModelNode`] was given, and its two tool ports.
struct Tools<P> {
    tools: Arc<HashMap<ToolId, Tool>>,
    /// The tools in the order the node was given them, the order it offers them in.
    offered: Arc<Vec<Tool>>,
    tool_call: PortId,
    tool_result: PortId,
    wrap: fn(ToolCallUpdate) -> P,
}

impl<P> Clone for Tools<P> {
    fn clone(&self) -> Self {
        Self {
            tools: Arc::clone(&self.tools),
            offered: Arc::clone(&self.offered),
            tool_call: self.tool_call,
            tool_result: self.tool_result,
            wrap: self.wrap,
        }
    }
}

impl<P> fmt::Debug for Tools<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tools")
            .field("tools", &self.tools.keys().collect::<Vec<_>>())
            .field("tool_call", &self.tool_call)
            .field("tool_result", &self.tool_result)
            .finish_non_exhaustive()
    }
}

/// The mock model node; see the module documentation.
pub struct ModelNode<P = PipelinePayload> {
    run_id: RunId,
    generator: Generator,
    reply: fn(&str) -> String,
    late: Option<String>,
    tools: Option<Tools<P>>,
    turns: Option<PortId>,
    log: RunLog,
    node_id: Option<NodeId>,
    signal: Arc<Watched<Signal>>,
    record: Arc<Watched<Record>>,
    exchange: Exchange,
}

impl<P> fmt::Debug for ModelNode<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelNode")
            .field("run_id", &self.run_id)
            .field("generator", &self.generator)
            .field("late", &self.late)
            .field("tools", &self.tools)
            .field("turns", &self.turns)
            .field("node_id", &self.node_id)
            .field("signal", &self.signal)
            .field("record", &self.record)
            .field("exchange", &self.exchange)
            .finish_non_exhaustive()
    }
}

impl ModelNode {
    /// A model node of the run `run_id` answering on `generator`, the mock's or any other
    /// `inference-llm` [`Generator`], whose reply to an input text is `reply` of it, recording
    /// what it emits in `log`. The generator is to claim `concurrent_input_output`: input reaches
    /// its running generation only through admission.
    #[must_use]
    pub fn new(
        run_id: RunId,
        generator: impl Into<Generator>,
        reply: fn(&str) -> String,
        log: RunLog,
    ) -> Self {
        Self::carrying(run_id, generator, reply, log)
    }
}

impl<P: ModelPayload> ModelNode<P> {
    /// As [`ModelNode::new`], on edges carrying the payload `P`.
    #[must_use]
    pub fn carrying(
        run_id: RunId,
        generator: impl Into<Generator>,
        reply: fn(&str) -> String,
        log: RunLog,
    ) -> Self {
        Self {
            run_id,
            generator: generator.into(),
            reply,
            late: None,
            tools: None,
            turns: None,
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

    /// The same node given a turns port, `turns`: once an edge arrives there, the node starts its
    /// generation at the first `inference.stream.TurnCompleted` that edge delivers, and answers
    /// each input at the completion of the turn it arrived in; see the module documentation.
    #[must_use]
    pub const fn with_turns(mut self, turns: PortId) -> Self {
        self.turns = Some(turns);
        self
    }

    /// The test's side of the node.
    #[must_use]
    pub fn handle(&self) -> ModelHandle {
        ModelHandle {
            record: Arc::clone(&self.record),
            signal: Arc::clone(&self.signal),
            task: self.exchange.probe(),
        }
    }

    fn failure(&self) -> Result<(), NodeError> {
        self.record
            .read(|record| record.failure.clone())
            .map_or(Ok(()), |failure| Err(NodeError::new(failure)))
    }
}

impl<P: ModelPayload + ToolPayload> ModelNode<P> {
    /// The same node given `tools`, the declared tools its generator may propose: it sends its
    /// proposals on the output edge leaving its port `tool_call` and takes their outcomes from
    /// every input edge arriving at its port `tool_result`; see the module documentation. It is
    /// then handed one input edge besides those and two output edges.
    #[must_use]
    pub fn with_tools(
        mut self,
        tools: impl IntoIterator<Item = Tool>,
        tool_call: PortId,
        tool_result: PortId,
    ) -> Self {
        let offered: Vec<Tool> = tools.into_iter().collect();
        self.tools = Some(Tools {
            tools: Arc::new(
                offered
                    .iter()
                    .map(|tool| (tool.tool_id(), tool.clone()))
                    .collect(),
            ),
            offered: Arc::new(offered),
            tool_call,
            tool_result,
            wrap: P::from_tool_call,
        });
        self
    }
}

/// What a started node takes: its identity, its input edges, its text output edge and, given
/// tools, its `tool_call` output edge.
type Shaped<P> = (
    NodeId,
    Vec<Receiving<P>>,
    inference_pipeline::NodeOutput<P>,
    Option<inference_pipeline::NodeOutput<P>>,
);

/// The port an input edge of a started node arrives at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arrives {
    /// The node's input, which carries what it answers.
    Input,
    /// The `tool_result` port.
    ToolResult,
    /// The turns port.
    Turns,
}

/// One input edge of a started node.
struct Receiving<P> {
    endpoint: Box<dyn InputEndpoint<P>>,
    /// The port the edge arrives at.
    port: Arrives,
    /// Whether the edge delivered its end of input or is closed.
    ended: bool,
}

/// What the exchange acts on next.
enum Step<P> {
    /// The runtime cancelled the node.
    Cancel,
    /// The host asked for `SupersedeGeneration`.
    Supersede,
    /// The runtime drains the node, whose input edge is closed.
    Drain,
    /// The input edge at this index delivered an envelope, or none once it is closed.
    Input(usize, Option<EventEnvelope<P>>),
}

/// The tool loop of a started node given tools.
struct ToolLoop<P> {
    tools: Arc<HashMap<ToolId, Tool>>,
    wrap: fn(ToolCallUpdate) -> P,
    output: Box<dyn OutputEndpoint<P>>,
    run_id: RunId,
    producer: NodeId,
    port: String,
    stream_id: StreamId,
    sequence: i64,
    /// The node's own turn, in which it holds the spans it generated from.
    turn: Turn,
    /// The latest span the node generated from.
    basis: Option<Span>,
    /// The calls whose outcomes the generation awaits, in the order they were proposed, each
    /// with the `llm` call it proposes.
    awaiting: Vec<(ToolCallId, CallId)>,
    /// Whether the loop's iteration rule is exhausted.
    exhausted: bool,
}

impl<P: From<ControlKind>> ToolLoop<P> {
    /// The envelope of `update` on the node's `tool_call` stream, naming the call's generation.
    fn envelope(&mut self, update: ToolCallUpdate) -> EventEnvelope<P> {
        let kind = update.event.name().to_owned();
        let generation_id = Some(update.call.generation_id());
        self.next(kind, generation_id, (self.wrap)(update))
    }

    /// The end of input on the node's `tool_call` stream.
    fn end(&mut self) -> EventEnvelope<P> {
        let kind = ControlKind::Stop.as_str().to_owned();
        self.next(kind, None, P::from(ControlKind::Stop))
    }

    /// The next envelope on the node's `tool_call` stream.
    fn next(
        &mut self,
        kind: String,
        generation_id: Option<inference_core::id::GenerationId>,
        payload: P,
    ) -> EventEnvelope<P> {
        self.sequence += 1;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| {
                i64::try_from(since.as_micros()).unwrap_or(i64::MAX)
            });
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: self.run_id,
            session_id: None,
            stream_id: self.stream_id,
            sequence: self.sequence,
            producer: self.producer,
            port: self.port.clone(),
            kind,
            generation_id,
            timestamp,
            timebase: Timebase::WallTime,
            causation_id: None,
            payload,
        }
    }
}

/// One started exchange of a [`ModelNode`].
struct Exchanging<P> {
    run_id: RunId,
    /// The node's generation, once it started.
    generation: Option<GenerationHandle>,
    /// What starts the generation at the first `TurnCompleted`, for a node whose turns port is
    /// wired, until it started.
    starting: Option<Starting>,
    /// Whether an edge arrives at the node's turns port.
    gated: bool,
    /// The inputs that arrived in the turn that is not yet complete, with the stream and sequence
    /// they arrived on.
    turn_inputs: VecDeque<(StreamId, i64, TranscriptCommitted)>,
    reply: fn(&str) -> String,
    late: Option<String>,
    signal: Arc<Watched<Signal>>,
    record: Arc<Watched<Record>>,
    inputs: Vec<Receiving<P>>,
    output: Box<dyn OutputEndpoint<P>>,
    emitter: Emitter,
    tool_loop: Option<ToolLoop<P>>,
    /// The inputs that arrived while a call was awaited, with the stream and sequence they
    /// arrived on, to be answered once it is not.
    deferred: VecDeque<(StreamId, i64, TranscriptCommitted)>,
    /// The text of every input that arrived once the generation was no longer Active.
    dropped: Vec<String>,
    /// Every outcome that arrived once the generation was no longer Active.
    not_reinjected: Vec<ToolCallUpdate>,
    /// The refusal of `SupersedeGeneration`, when the generation was no longer Active.
    supersede_refusal: Option<GenerationStateConflict>,
    /// How many model requests the node issued: inputs offered and rounds of outcomes answered.
    requests: usize,
    /// Whether the node's input delivered its end of input.
    input_ended: bool,
    /// Whether the node's input edge is closed.
    input_closed: bool,
}

/// What starts the generation of a node: its generator and the request, which carries the tools
/// it offers.
struct Starting {
    generator: Generator,
    request: GenerationRequest,
}

impl Starting {
    fn start(mut self) -> Result<GenerationHandle, inference_llm::ConcurrencyLimitReached> {
        self.generator.start(self.request)
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

/// An output edge of payload `P` taking the envelopes of the text pipeline's payload, each
/// carried as `P`; a refused one is handed back as it was sent. A cancel of the node taken while
/// a send waits for room ends the wait, and the envelope is not sent.
struct Carrying<'a, P>(&'a dyn OutputEndpoint<P>, &'a Watched<Signal>);

impl<P: ModelPayload> OutputEndpoint<PipelinePayload> for Carrying<'_, P> {
    fn send(
        &self,
        envelope: EventEnvelope<PipelinePayload>,
    ) -> EndpointFuture<'_, Result<(), SendError<PipelinePayload>>> {
        Box::pin(async move {
            let sent = envelope.clone();
            let mut sending = self.0.send(carry(envelope));
            future::poll_fn(|cx| {
                let cancelled = pin!(self.1.until(|signal| signal.cancel.then_some(())));
                if cancelled.poll(cx).is_ready() {
                    return Poll::Ready(Ok(()));
                }
                sending.as_mut().poll(cx)
            })
            .await
            .map_err(|refused| match refused {
                SendError::Full(_) => SendError::Full(sent),
                SendError::Closed(_) => SendError::Closed(sent),
                SendError::SequenceNotIncreasing { last, .. } => SendError::SequenceNotIncreasing {
                    envelope: sent,
                    last,
                },
            })
        })
    }
}

impl<P: ModelPayload> Exchanging<P> {
    /// Emits the generation's events until `last` is delivered, or until none is.
    async fn deliver(&mut self, last: fn(&GenerationEvent) -> bool) -> Result<(), String> {
        loop {
            let Some(generation) = self.generation.as_mut() else {
                return Ok(());
            };
            let Some(event) = generation.next_event().await else {
                return Ok(());
            };
            let done = last(&event);
            self.send(event).await?;
            if done {
                return Ok(());
            }
        }
    }

    /// Emits every event of the turns that can run now, until none is left to deliver.
    async fn deliver_turn(&mut self) -> Result<(), String> {
        loop {
            let Some(generation) = self.generation.as_mut() else {
                return Ok(());
            };
            let Some(event) = generation.next_turn_event().await else {
                return Ok(());
            };
            self.send(event).await?;
        }
    }

    /// Emits `event` on the text port. The cancellation of a generation whose turn failed is
    /// emitted as `GenerationCancelled`, and then fails the node with the `llm` error.
    async fn send(&mut self, event: GenerationEvent) -> Result<(), String> {
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
        emit(
            &Carrying(self.output.as_ref(), &self.signal),
            &mut self.emitter,
            event,
        )
        .await?;
        if !failed {
            return Ok(());
        }
        Err(self
            .generation
            .as_ref()
            .and_then(GenerationHandle::failure)
            .map_or_else(
                || "the model's turn failed".to_owned(),
                |error| format!("the model's turn failed: {error}"),
            ))
    }

    /// Completes the generation, whatever call is unanswered, and emits `GenerationCompleted`.
    async fn complete(&mut self) -> Result<(), String> {
        if let Some(generation) = self.generation.as_mut() {
            let _ = generation.complete();
        }
        self.deliver(|event| matches!(event, GenerationEvent::Completed(_)))
            .await
    }

    /// Emits every event of the turn just run, then sends the tool calls it ended with.
    async fn answer(&mut self) -> Result<(), String> {
        self.deliver_turn().await?;
        self.propose().await
    }

    /// Turns each `llm` tool call the latest turn ended with into `ProposeToolCall` of the
    /// generation and sends it on `tool_call`, unless it is dropped; see the module
    /// documentation.
    async fn propose(&mut self) -> Result<(), String> {
        let (Some(tool_loop), Some(running)) = (self.tool_loop.as_mut(), self.generation.as_mut())
        else {
            return Ok(());
        };
        while let Some(proposal) = running.take_proposal() {
            if tool_loop.exhausted
                || running.output().generation().state() != GenerationState::Active
            {
                continue;
            }
            let Some(tool) = tool_id(&proposal.name).and_then(|id| tool_loop.tools.get(&id)) else {
                let denied = ToolOutcome::Rejected(RejectionReason::Denied);
                running
                    .integrate(&proposal.call_id, denied.output(), denied.failed())
                    .map_err(|refused| refused.to_string())?;
                continue;
            };
            let basis = tool_loop.basis.as_ref().ok_or_else(|| {
                "a proposal was made before any input it could rest on".to_owned()
            })?;
            let generation = running.output().generation();
            let (call, event) = ToolCall::propose(tool, generation, basis, proposal.arguments)
                .map_err(|refused| refused.to_string())?;
            tool_loop.awaiting.push((call.call_id(), proposal.call_id));
            let envelope = tool_loop.envelope(ToolCallUpdate {
                call,
                event: ToolCallEvent::Proposed(event),
            });
            send_unless(
                tool_loop.output.as_ref(),
                &self.signal,
                |signal| signal.cancel,
                envelope,
            )
            .await?;
        }
        Ok(())
    }

    /// Runs `inference.stream.SupersedeGeneration` on the generation and sends its
    /// `GenerationSuperseded` on the text port and, given tools, on `tool_call`; a generation no
    /// longer Active refuses it, and the refusal is recorded and nothing is sent.
    async fn supersede(&mut self) -> Result<(), String> {
        let Some(generation) = self.generation.as_mut() else {
            return Ok(());
        };
        let superseded = match generation.supersede() {
            Ok(superseded) => superseded,
            Err(refused) => {
                self.supersede_refusal = Some(refused);
                return Ok(());
            }
        };
        let event = StreamEvent::GenerationSuperseded(superseded);
        emit(
            &Carrying(self.output.as_ref(), &self.signal),
            &mut self.emitter,
            event.clone(),
        )
        .await?;
        if let Some(tool_loop) = self.tool_loop.as_mut() {
            let envelope = tool_loop.next(
                event.name().to_owned(),
                Some(superseded.generation_id),
                P::stream(event),
            );
            send_unless(
                tool_loop.output.as_ref(),
                &self.signal,
                |signal| signal.cancel,
                envelope,
            )
            .await?;
        }
        Ok(())
    }

    /// Holds the span `committed` names as the latest the node generates from: a span of the
    /// node's own turn, Committed, as the event says it is.
    fn generating_from(
        &mut self,
        envelope_stream: StreamId,
        sequence: i64,
        committed: &inference_text::TranscriptCommitted,
    ) -> Result<(), String> {
        let Some(tool_loop) = self.tool_loop.as_mut() else {
            return Ok(());
        };
        let (span, _) = Span::emit_as(
            committed.span_id,
            &tool_loop.turn,
            self.run_id,
            envelope_stream,
            sequence,
            committed.text.clone(),
        )
        .map_err(|refused| refused.to_string())?;
        let (span, _) = span.commit().map_err(|refused| refused.to_string())?;
        tool_loop.basis = Some(span);
        Ok(())
    }

    /// Acts on a step of a tool call that arrived on `tool_result`.
    async fn outcome(&mut self, update: ToolCallUpdate) -> Result<(), String> {
        let Some(tool_loop) = self.tool_loop.as_mut() else {
            return Ok(());
        };
        let call_id = update.call.call_id();
        let Some(awaited) = tool_loop.awaiting.iter().position(|(id, _)| *id == call_id) else {
            return Ok(());
        };
        if matches!(
            update.event,
            ToolCallEvent::Proposed(_) | ToolCallEvent::Authorized(_) | ToolCallEvent::Started(_)
        ) {
            return Ok(());
        }
        let (_, proposed) = tool_loop.awaiting.remove(awaited);
        let Some(generation) = self.generation.as_mut().filter(|generation| {
            generation.output().generation().state() == GenerationState::Active
        }) else {
            self.not_reinjected.push(update);
            return Ok(());
        };
        let outcome = match update.event {
            ToolCallEvent::Result(result) => ToolOutcome::Result(result.result),
            ToolCallEvent::Failed(failed) => ToolOutcome::Failed(failed.failure),
            ToolCallEvent::Rejected(rejected) if rejected.reason == RejectionReason::Exhausted => {
                tool_loop.exhausted = true;
                return self.complete().await;
            }
            ToolCallEvent::Rejected(rejected) => ToolOutcome::Rejected(rejected.reason),
            ToolCallEvent::Proposed(_)
            | ToolCallEvent::Authorized(_)
            | ToolCallEvent::Started(_) => return Ok(()),
        };
        generation
            .integrate(&proposed, outcome.output(), outcome.failed())
            .map_err(|refused| refused.to_string())?;
        if !tool_loop.awaiting.is_empty() {
            return Ok(());
        }
        self.requests += 1;
        self.answer().await
    }

    /// The next thing to act on: a cancel first, then a drain once the input edge is closed,
    /// then an envelope of the first input edge that delivers one.
    async fn step(&mut self, input_closed: bool) -> Step<P> {
        let signal = Arc::clone(&self.signal);
        let inputs = &mut self.inputs;
        future::poll_fn(|cx| {
            let wanted = pin!(signal.until(|signal| {
                if signal.cancel {
                    Some(Step::Cancel)
                } else if signal.supersede && !signal.superseded {
                    signal.superseded = true;
                    Some(Step::Supersede)
                } else if input_closed && signal.drain {
                    Some(Step::Drain)
                } else {
                    None
                }
            }));
            if let Poll::Ready(step) = wanted.poll(cx) {
                return Poll::Ready(step);
            }
            for (index, input) in inputs.iter_mut().enumerate() {
                if input.ended {
                    continue;
                }
                let mut recv = input.endpoint.recv();
                if let Poll::Ready(received) = recv.as_mut().poll(cx) {
                    return Poll::Ready(Step::Input(index, received));
                }
            }
            Poll::Pending
        })
        .await
    }

    /// Passes the end of input on, on the text port and, given tools, on `tool_call`.
    async fn end(&mut self) -> Result<(), String> {
        let end = carry(self.emitter.control(ControlKind::Stop));
        send_unless(
            self.output.as_ref(),
            &self.signal,
            |signal| signal.cancel,
            end,
        )
        .await?;
        if let Some(tool_loop) = self.tool_loop.as_mut() {
            let end = tool_loop.end();
            send_unless(
                tool_loop.output.as_ref(),
                &self.signal,
                |signal| signal.cancel,
                end,
            )
            .await?;
        }
        Ok(())
    }

    async fn exchange(&mut self) -> Ended {
        match self.exchanged().await {
            Ok(ended) => ended,
            Err(failure) => Ended::Failed(failure),
        }
    }

    /// Whether the generation started and is Active.
    fn active(&self) -> bool {
        self.generation.as_ref().is_some_and(|generation| {
            generation.output().generation().state() == GenerationState::Active
        })
    }

    /// Starts the generation, if it has not started, and emits its `GenerationStarted`.
    async fn start_generation(&mut self) -> Result<(), String> {
        let Some(starting) = self.starting.take() else {
            return Ok(());
        };
        self.generation = Some(starting.start().map_err(|refused| refused.to_string())?);
        self.deliver(|event| matches!(event, GenerationEvent::Started(_)))
            .await
    }

    /// Acts on the end of the turn whose `TurnCompleted` arrived at the turns port: takes what
    /// the node's input already holds, which its producer sent before the completion, then starts
    /// the generation, if it has not started, and queues the turn's inputs to be answered.
    async fn turn_completed(&mut self) -> Result<(), String> {
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        for index in 0..self.inputs.len() {
            while self.inputs[index].port == Arrives::Input && !self.inputs[index].ended {
                let Poll::Ready(received) =
                    self.inputs[index].endpoint.recv().as_mut().poll(&mut cx)
                else {
                    break;
                };
                self.received_input(index, received).await?;
            }
        }
        self.start_generation().await?;
        self.deferred.extend(self.turn_inputs.drain(..));
        Ok(())
    }

    /// Whether the generation awaits the outcome of a call.
    fn awaiting(&self) -> bool {
        self.tool_loop
            .as_ref()
            .is_some_and(|tool_loop| !tool_loop.awaiting.is_empty())
    }

    /// Answers the input `committed`, which arrived on `stream_id` at `sequence`: the generator's
    /// reply to it, and the proposal that reply ends with. Once the generation is no longer
    /// Active, the input is dropped and recorded instead.
    async fn input(
        &mut self,
        stream_id: StreamId,
        sequence: i64,
        committed: TranscriptCommitted,
    ) -> Result<(), String> {
        if !self.active() {
            self.dropped.push(committed.text);
            return Ok(());
        }
        self.generating_from(stream_id, sequence, &committed)?;
        self.generation
            .as_mut()
            .ok_or_else(|| "the generation was checked Active".to_owned())?
            .offer_input((self.reply)(&committed.text))
            .map_err(|refused| refused.to_string())?;
        self.requests += 1;
        self.answer().await
    }

    /// Answers the inputs deferred while a call was awaited, in arrival order, until one of them
    /// awaits a call again.
    async fn deferred(&mut self) -> Result<(), String> {
        while !self.awaiting() {
            let Some((stream_id, sequence, committed)) = self.deferred.pop_front() else {
                break;
            };
            self.input(stream_id, sequence, committed).await?;
        }
        Ok(())
    }

    /// Whether every edge arriving at the turns port has ended, so that no turn completes later.
    fn turns_ended(&self) -> bool {
        self.inputs
            .iter()
            .all(|input| input.ended || input.port != Arrives::Turns)
    }

    /// Acts on what the input edge at `index`, arriving at the node's input, delivered: an
    /// envelope, or none once it is closed.
    async fn received_input(
        &mut self,
        index: usize,
        received: Option<EventEnvelope<P>>,
    ) -> Result<(), String> {
        let Some(envelope) = received else {
            self.inputs[index].ended = true;
            self.input_closed = true;
            return Ok(());
        };
        let (stream_id, sequence) = (envelope.stream_id, envelope.sequence);
        match envelope.payload.carried() {
            Carried::Control(ControlKind::Stop) => {
                self.inputs[index].ended = true;
                self.input_ended = true;
            }
            Carried::Stream(StreamEvent::TranscriptCommitted(committed)) if !self.input_ended => {
                if self.gated {
                    self.turn_inputs.push_back((stream_id, sequence, committed));
                } else if self.awaiting() {
                    self.deferred.push_back((stream_id, sequence, committed));
                } else {
                    self.input(stream_id, sequence, committed).await?;
                }
            }
            Carried::Stream(_) | Carried::Tool(_) | Carried::Control(_) => {}
        }
        Ok(())
    }

    /// Acts on what the input edge at `index` delivered: an envelope, or none once it is closed.
    async fn received(
        &mut self,
        index: usize,
        received: Option<EventEnvelope<P>>,
    ) -> Result<(), String> {
        let port = self.inputs[index].port;
        if port == Arrives::Input {
            return self.received_input(index, received).await;
        }
        let Some(envelope) = received else {
            self.inputs[index].ended = true;
            return Ok(());
        };
        match (port, envelope.payload.carried()) {
            (_, Carried::Control(ControlKind::Stop)) => self.inputs[index].ended = true,
            (Arrives::ToolResult, Carried::Tool(update)) => self.outcome(update).await?,
            (Arrives::Turns, Carried::Stream(StreamEvent::TurnCompleted(_))) => {
                self.turn_completed().await?;
            }
            _ => {}
        }
        Ok(())
    }

    async fn exchanged(&mut self) -> Result<Ended, String> {
        self.deliver(|event| matches!(event, GenerationEvent::Started(_)))
            .await?;
        let mut ended = false;
        loop {
            self.deferred().await?;
            if self.input_ended
                && !self.awaiting()
                && self.deferred.is_empty()
                && self.turns_ended()
                && !ended
            {
                self.dropped
                    .extend(self.turn_inputs.drain(..).map(|(_, _, input)| input.text));
                self.complete().await?;
                self.end().await?;
                ended = true;
            }
            if ended
                && self
                    .inputs
                    .iter()
                    .all(|input| input.ended || input.port == Arrives::Input)
            {
                return Ok(Ended::Completed);
            }
            match self.step(self.input_closed && !self.input_ended).await {
                Step::Cancel => return Ok(Ended::Cancelled),
                Step::Supersede => self.supersede().await?,
                Step::Drain if self.tool_loop.is_some() || self.gated => self.input_ended = true,
                Step::Drain => {
                    self.complete().await?;
                    return Ok(Ended::Completed);
                }
                Step::Input(index, received) => self.received(index, received).await?,
            }
        }
    }

    /// Cancels the generation, recording its `GenerationCancelled` unsent, and emits the late
    /// text into it if the node was scripted to.
    fn cancel(&mut self) -> Option<GenerationStateConflict> {
        let generation = self.generation.as_mut()?;
        if let Ok(cancelled) = generation.cancel() {
            let _ = self
                .emitter
                .stream(StreamEvent::GenerationCancelled(cancelled));
        }
        let late = self.late.take()?;
        generation.emit(late).err()
    }

    fn run(mut self) {
        let ended = block_on(self.exchange());
        let (late_refusal, failure) = match ended {
            Ended::Completed => (None, None),
            Ended::Cancelled => (self.cancel(), None),
            Ended::Failed(failure) => (None, Some(failure)),
        };
        let dropped = std::mem::take(&mut self.dropped);
        let not_reinjected = std::mem::take(&mut self.not_reinjected);
        let (supersede_refusal, requests) = (self.supersede_refusal, self.requests);
        let output = self.generation.as_ref().map(GenerationHandle::output);
        let state = output.map(|output| output.generation().state());
        let segments = output.map_or_else(Vec::new, |output| {
            output.segments().iter().map(|s| s.snapshot()).collect()
        });
        self.record.update(|record| {
            record.state = state;
            record.segments = segments;
            record.late_refusal = late_refusal;
            record.failure = failure;
            record.dropped = dropped;
            record.not_reinjected = not_reinjected;
            record.supersede_refusal = supersede_refusal;
            record.requests = requests;
        });
    }
}

impl<P: ModelPayload> ModelNode<P> {
    /// The node's input edges and its text and `tool_call` output edges, once it was handed
    /// what its tools, if any, require; otherwise the error saying what it was handed.
    fn shaped(
        &self,
        node_id: Option<NodeId>,
        mut endpoints: NodeEndpoints<P>,
    ) -> Result<Shaped<P>, NodeError> {
        let arrives = |port_id: PortId| {
            if self
                .tools
                .as_ref()
                .is_some_and(|tools| tools.tool_result == port_id)
            {
                Arrives::ToolResult
            } else if self.turns == Some(port_id) {
                Arrives::Turns
            } else {
                Arrives::Input
            }
        };
        let turns = endpoints
            .inputs
            .iter()
            .filter(|input| arrives(input.port_id) == Arrives::Turns)
            .count();
        let Some(tools) = &self.tools else {
            let node_id = started(node_id, &endpoints, 1 + turns, 1)?;
            let inputs = endpoints
                .inputs
                .into_iter()
                .map(|input| Receiving {
                    port: arrives(input.port_id),
                    endpoint: input.endpoint,
                    ended: false,
                })
                .collect();
            return Ok((node_id, inputs, endpoints.outputs.remove(0), None));
        };
        let node_id = node_id
            .ok_or_else(|| NodeError::new("the node was started before it was described"))?;
        let results = endpoints
            .inputs
            .iter()
            .filter(|input| input.port_id == tools.tool_result)
            .count();
        let call = endpoints
            .outputs
            .iter()
            .position(|output| output.port_id == tools.tool_call);
        let (Some(call), 2, true, true) = (
            call,
            endpoints.outputs.len(),
            results >= 1,
            endpoints.inputs.len() == results + turns + 1,
        ) else {
            return Err(NodeError::new(format!(
                "the node takes 1 input edge besides those into its port {}, at least one of \
                 those, and 2 output edges, one of them from its port {}, and was handed {} and {}",
                tools.tool_result,
                tools.tool_call,
                endpoints.inputs.len(),
                endpoints.outputs.len()
            )));
        };
        let tool_call = endpoints.outputs.remove(call);
        let text = endpoints.outputs.remove(0);
        let inputs = endpoints
            .inputs
            .into_iter()
            .map(|input| Receiving {
                port: arrives(input.port_id),
                endpoint: input.endpoint,
                ended: false,
            })
            .collect();
        Ok((node_id, inputs, text, Some(tool_call)))
    }
}

impl<P: ModelPayload> NodeImplementation<P> for ModelNode<P> {
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.node_id = Some(node.node_id());
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn start(&mut self, endpoints: NodeEndpoints<P>) -> NodeFuture<'_, Result<(), NodeError>> {
        let started = self.shaped(self.node_id, endpoints).and_then(
            |(node_id, inputs, output, tool_call)| {
                let mut turn = TurnRequest::new(
                    self.generator.model().provenance().model.as_str(),
                    Vec::new(),
                );
                if let Some(tools) = &self.tools {
                    turn.tools = tools.offered.iter().map(tool_spec).collect();
                }
                let starting = Starting {
                    generator: self.generator.clone(),
                    request: GenerationRequest {
                        run_id: self.run_id,
                        node_id,
                        epoch: 1,
                        turn,
                    },
                };
                let gated = inputs.iter().any(|input| input.port == Arrives::Turns);
                let (generation, starting) = if gated {
                    (None, Some(starting))
                } else {
                    let generation = starting
                        .start()
                        .map_err(|refused| NodeError::new(refused.to_string()))?;
                    (Some(generation), None)
                };
                let tool_loop = match (&self.tools, tool_call) {
                    (Some(tools), Some(tool_call)) => Some(ToolLoop {
                        tools: Arc::clone(&tools.tools),
                        wrap: tools.wrap,
                        run_id: self.run_id,
                        producer: node_id,
                        port: tool_call.port_id.to_string(),
                        output: tool_call.endpoint,
                        stream_id: StreamId::generate(),
                        sequence: 0,
                        turn: Turn::open(OpenTurn {
                            run_id: self.run_id,
                        })
                        .0,
                        basis: None,
                        awaiting: Vec::new(),
                        exhausted: false,
                    }),
                    _ => None,
                };
                let exchanging = Exchanging {
                    run_id: self.run_id,
                    generation,
                    starting,
                    gated,
                    turn_inputs: VecDeque::new(),
                    reply: self.reply,
                    late: self.late.clone(),
                    signal: Arc::clone(&self.signal),
                    record: Arc::clone(&self.record),
                    inputs,
                    emitter: Emitter::new(
                        self.run_id,
                        node_id,
                        output.port_id.to_string(),
                        self.log.clone(),
                    ),
                    output: output.endpoint,
                    tool_loop,
                    deferred: VecDeque::new(),
                    dropped: Vec::new(),
                    not_reinjected: Vec::new(),
                    supersede_refusal: None,
                    requests: 0,
                    input_ended: false,
                    input_closed: false,
                };
                self.exchange.spawn(move || exchanging.run());
                Ok(())
            },
        );
        Box::pin(future::ready(started))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        self.signal.update(|signal| signal.drain = true);
        Box::pin(async move {
            self.exchange.finished().await;
            self.exchange.panic()?;
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
    signal: Arc<Watched<Signal>>,
    task: Weak<()>,
}

impl ModelHandle {
    /// The state the generation ended in; none before the exchange ended, or when the node, given
    /// a turns input, saw no turn complete.
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

    /// The text of every input the node dropped because it arrived once its generation was no
    /// longer Active, such as after the iteration rule completed it; empty before the exchange
    /// ended.
    #[must_use]
    pub fn dropped_inputs(&self) -> Vec<String> {
        self.record.read(|record| record.dropped.clone())
    }

    /// The failure that ended the node's exchange, if one did.
    #[must_use]
    pub fn failure(&self) -> Option<String> {
        self.record.read(|record| record.failure.clone())
    }

    /// A weak reference to the node's exchange task, alive until the task ended.
    #[must_use]
    pub fn task(&self) -> Weak<()> {
        Weak::clone(&self.task)
    }

    /// Asks the node for `inference.stream.SupersedeGeneration` on its generation, which it runs
    /// at its next step; see the module documentation. False, asking nothing, once it was asked
    /// or the node was cancelled.
    #[must_use]
    pub fn supersede(&self) -> bool {
        self.signal.update(|signal| {
            if signal.supersede || signal.cancel {
                return false;
            }
            signal.supersede = true;
            true
        })
    }

    /// The refusal of the supersede the host asked for, when the generation was no longer
    /// Active; none before the exchange ended.
    #[must_use]
    pub fn supersede_refusal(&self) -> Option<GenerationStateConflict> {
        self.record.read(|record| record.supersede_refusal)
    }

    /// Every outcome of an awaited call that arrived once the generation was no longer Active, in
    /// arrival order, none of them reinjected; empty before the exchange ended.
    #[must_use]
    pub fn not_reinjected(&self) -> Vec<ToolCallUpdate> {
        self.record.read(|record| record.not_reinjected.clone())
    }

    /// How many model requests the node issued on its generation, each input offered and each
    /// round of outcomes answered, its turn run; zero before the exchange ended.
    #[must_use]
    pub fn model_requests(&self) -> usize {
        self.record.read(|record| record.requests)
    }
}
