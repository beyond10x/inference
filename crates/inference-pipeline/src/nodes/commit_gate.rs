//! The commit gate: a node of role `policy` that lets committed content through and holds back
//! what is not committed (`docs/design/inference.md` 618-628). It enforces commitment; it does not
//! decide it: the producer upstream commits, and the gate passes what it committed.
//!
//! A gate has one or more lanes, each an input port and the output port it passes on to; what
//! arrives on a lane leaves on that lane's output, in the order it arrived. A voice cascade puts
//! it in one of two places (759-778):
//!
//! * the input gate, between the STT node and the LLM, has two lanes: the transcript events on
//!   `input`, passed on `output`, and the turn events on `turns`, passed on `turns_out`. It passes
//!   each `inference.stream.TranscriptCommitted` and every turn event, and holds back every
//!   hypothesis and every revision of a span not yet committed: none of them is passed, ever, as
//!   the committed span carries its final text. A revision naming a span the gate already passed
//!   as Committed is the `wrong-state` outcome of `inference.stream.ReviseSpan`: it is refused,
//!   recorded on the handle ([`CommitGateHandle::refused_revisions`]) and never passed;
//! * the output gate, between the LLM and the TTS node, has one lane: the LLM node's output. It
//!   holds each `inference.stream.TextDelta`, the Tentative segment it emits, until the segment's
//!   `TextSegmentCommitted` arrives, and then passes both, so that the TTS node is never handed a
//!   Tentative segment and `inference.stream.QueueForSynthesis` is never asked of one. The deltas
//!   still held when their generation ends, by `GenerationCompleted`, `GenerationCancelled` or
//!   `GenerationSuperseded`, or when the lane ends, are dropped and recorded
//!   ([`CommitGateHandle::dropped_segments`]).
//!
//! The rules are the same in both places, since a transcript carries no delta and an LLM's output
//! no hypothesis. What a lane passes keeps the order it arrived in: an envelope behind a held
//! delta waits until the delta is passed or dropped. Every envelope passed is the envelope that
//! arrived, its producer, stream and sequence unchanged, as an edge of its own from the producer
//! would carry it. Content the gate does not read ([`Gated::Other`]), such as
//! `GenerationStarted`, and every control event but the end of input pass as they are.
//!
//! When a turn event arrives, the gate first takes what its other lanes already hold, which their
//! producer sent before it, so that a consumer of both outputs, such as the LLM node, has every
//! committed span of a turn before the turn's completion, as the STT node sent them. The time at
//! which each `TurnCompleted` was passed is recorded ([`CommitGateHandle::turns_passed`]).
//!
//! The end of input on a lane, a control event of kind `stop`, drops the deltas the lane holds,
//! passes the rest and then the `stop`, and ends the lane; a lane whose input edge is closed ends
//! the same way without a `stop`. The node ends once every lane ended. Cancelled, it sends nothing
//! more.
//!
//! The gate reads its payload through [`GatePayload`], which the payload of the run implements,
//! so that this crate depends on no contract crate.

use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::future;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::Instant;

use inference_core::endpoint::{InputEndpoint, OutputEndpoint};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId};
use inference_core::id::{GenerationId, NodeId};
use inference_text::{SegmentId, SpanId, TurnId};

use super::turn_policy::TurnEvent;
use super::{Task, Watched, block_on};
use crate::{Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation, PortId};

/// What a payload carries, as the commit gate reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gated {
    /// `inference.stream.TranscriptHypothesis` of the span `span_id`.
    Hypothesis { span_id: SpanId },
    /// `inference.stream.TranscriptRevision` of the span `span_id`.
    Revision { span_id: SpanId },
    /// `inference.stream.TranscriptCommitted` of the span `span_id`.
    SpanCommitted { span_id: SpanId },
    /// A turn event, `inference.stream.TurnOpened` or `inference.stream.TurnCompleted`.
    Turn(TurnEvent),
    /// `inference.stream.TextDelta`: the Tentative segment `segment_id` of the generation
    /// `generation_id`.
    Delta {
        segment_id: SegmentId,
        generation_id: GenerationId,
    },
    /// `inference.stream.TextSegmentCommitted` of the segment `segment_id`.
    SegmentCommitted { segment_id: SegmentId },
    /// The generation `generation_id` ended: `inference.stream.GenerationCompleted`,
    /// `GenerationCancelled` or `GenerationSuperseded`.
    GenerationEnded { generation_id: GenerationId },
    /// Anything else.
    Other,
}

/// The payload of a run whose edges a [`CommitGate`] stands on: the gate reads what it carries
/// and passes it on unchanged. A control event is told from content by its envelope's `kind`
/// alone.
pub trait GatePayload: Send + 'static {
    /// What the payload carries.
    fn gated(&self) -> Gated;
}

/// One lane of a gate: the input port it takes and the output port it passes on to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GateLane {
    pub input: PortId,
    pub output: PortId,
}

/// A revision the gate refused: it names the span `span_id`, which the gate had passed as
/// Committed; `event_id` is the envelope it arrived in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefusedRevision {
    pub span_id: SpanId,
    pub event_id: EventId,
}

/// A `TurnCompleted` the gate passed, and when it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PassedTurn {
    pub turn_id: TurnId,
    pub at: Instant,
}

/// What the gate has held back, refused, dropped and passed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GateRecord {
    /// The hypotheses and the revisions of spans not yet committed, none of which was passed.
    pub held_back: usize,
    /// The revisions of spans already passed as Committed, in arrival order.
    pub refused: Vec<RefusedRevision>,
    /// The segments whose deltas were held and dropped, never committed, in the order dropped.
    pub dropped: Vec<SegmentId>,
    /// The segments whose commit arrived, in arrival order; each delta of them held was passed
    /// with it.
    pub committed: Vec<SegmentId>,
    /// Every `TurnCompleted` passed, in the order passed.
    pub turns: Vec<PassedTurn>,
}

/// An envelope a lane holds: `waiting` names the segment and generation of a delta whose commit
/// has not arrived.
struct Queued<P> {
    envelope: EventEnvelope<P>,
    waiting: Option<(SegmentId, GenerationId)>,
}

/// The rules of the gate over its lanes, apart from the edges: what one envelope arriving on a
/// lane lets the lane pass.
struct Gate<P> {
    lanes: Vec<VecDeque<Queued<P>>>,
    /// The spans passed as Committed.
    passed: HashSet<SpanId>,
}

impl<P: GatePayload> Gate<P> {
    fn new(lanes: usize) -> Self {
        Self {
            lanes: (0..lanes).map(|_| VecDeque::new()).collect(),
            passed: HashSet::new(),
        }
    }

    /// Takes `envelope` on the lane `lane`, recording in `record`, and answers what the lane
    /// passes now, in order.
    fn take(
        &mut self,
        lane: usize,
        envelope: EventEnvelope<P>,
        record: &mut GateRecord,
    ) -> Vec<EventEnvelope<P>> {
        let ready = |envelope| Queued {
            envelope,
            waiting: None,
        };
        let queue = &mut self.lanes[lane];
        match ControlKind::of(&envelope.kind) {
            Some(ControlKind::Stop) => {
                Self::drop_waiting(queue, record, |_| true);
                queue.push_back(ready(envelope));
            }
            Some(_) => queue.push_back(ready(envelope)),
            None => match envelope.payload.gated() {
                Gated::Hypothesis { .. } => record.held_back += 1,
                Gated::Revision { span_id } => {
                    if self.passed.contains(&span_id) {
                        record.refused.push(RefusedRevision {
                            span_id,
                            event_id: envelope.event_id,
                        });
                    } else {
                        record.held_back += 1;
                    }
                }
                Gated::SpanCommitted { span_id } => {
                    self.passed.insert(span_id);
                    queue.push_back(ready(envelope));
                }
                Gated::Delta {
                    segment_id,
                    generation_id,
                } => queue.push_back(Queued {
                    envelope,
                    waiting: Some((segment_id, generation_id)),
                }),
                Gated::SegmentCommitted { segment_id } => {
                    for queued in queue.iter_mut() {
                        if queued
                            .waiting
                            .is_some_and(|(waiting, _)| waiting == segment_id)
                        {
                            queued.waiting = None;
                        }
                    }
                    record.committed.push(segment_id);
                    queue.push_back(ready(envelope));
                }
                Gated::GenerationEnded { generation_id } => {
                    Self::drop_waiting(queue, record, |generation| generation == generation_id);
                    queue.push_back(ready(envelope));
                }
                Gated::Turn(_) | Gated::Other => queue.push_back(ready(envelope)),
            },
        }
        Self::release(queue)
    }

    /// The lane `lane` ended without a `stop`: drops its held deltas and answers the rest.
    fn close(&mut self, lane: usize, record: &mut GateRecord) -> Vec<EventEnvelope<P>> {
        let queue = &mut self.lanes[lane];
        Self::drop_waiting(queue, record, |_| true);
        Self::release(queue)
    }

    /// Drops every delta `queue` holds whose generation `ends`, recording its segment once.
    fn drop_waiting(
        queue: &mut VecDeque<Queued<P>>,
        record: &mut GateRecord,
        ends: impl Fn(GenerationId) -> bool,
    ) {
        queue.retain(|queued| match queued.waiting {
            Some((segment_id, generation_id)) if ends(generation_id) => {
                if !record.dropped.contains(&segment_id) {
                    record.dropped.push(segment_id);
                }
                false
            }
            _ => true,
        });
    }

    /// Takes from the front of `queue` every envelope no held delta stands before.
    fn release(queue: &mut VecDeque<Queued<P>>) -> Vec<EventEnvelope<P>> {
        let mut passed = Vec::new();
        while queue.front().is_some_and(|queued| queued.waiting.is_none()) {
            if let Some(queued) = queue.pop_front() {
                passed.push(queued.envelope);
            }
        }
        passed
    }
}

/// What the gate was asked and what it has done.
#[derive(Debug, Default)]
struct GateState {
    record: GateRecord,
    cancelled: bool,
    failure: Option<String>,
}

impl fmt::Debug for Watched<GateState> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.read(|state| state.fmt(f))
    }
}

/// The commit gate node; see the module documentation.
pub struct CommitGate {
    lanes: Vec<GateLane>,
    node_id: Option<NodeId>,
    state: Arc<Watched<GateState>>,
    task: Task,
}

impl fmt::Debug for CommitGate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommitGate")
            .field("lanes", &self.lanes)
            .field("node_id", &self.node_id)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl CommitGate {
    /// A gate of `lanes`, each handed one edge at its input port and one at its output port.
    #[must_use]
    pub fn new(lanes: impl IntoIterator<Item = GateLane>) -> Self {
        Self {
            lanes: lanes.into_iter().collect(),
            node_id: None,
            state: Arc::new(Watched::new(GateState::default())),
            task: Task::new(),
        }
    }

    /// The host's side of the gate.
    #[must_use]
    pub fn handle(&self) -> CommitGateHandle {
        CommitGateHandle {
            state: Arc::clone(&self.state),
        }
    }

    fn failure(&self) -> Result<(), NodeError> {
        self.state
            .read(|state| state.failure.clone())
            .map_or(Ok(()), |failure| Err(NodeError::new(failure)))
    }
}

/// The host's side of a [`CommitGate`]. Clones share one gate.
#[derive(Debug, Clone)]
pub struct CommitGateHandle {
    state: Arc<Watched<GateState>>,
}

impl CommitGateHandle {
    /// Everything the gate recorded so far.
    #[must_use]
    pub fn record(&self) -> GateRecord {
        self.state.read(|state| state.record.clone())
    }

    /// The revisions the gate refused, each naming a span it had passed as Committed, in arrival
    /// order.
    #[must_use]
    pub fn refused_revisions(&self) -> Vec<RefusedRevision> {
        self.state.read(|state| state.record.refused.clone())
    }

    /// The segments whose held deltas the gate dropped, never committed.
    #[must_use]
    pub fn dropped_segments(&self) -> Vec<SegmentId> {
        self.state.read(|state| state.record.dropped.clone())
    }

    /// Every `TurnCompleted` the gate passed, and when.
    #[must_use]
    pub fn turns_passed(&self) -> Vec<PassedTurn> {
        self.state.read(|state| state.record.turns.clone())
    }

    /// The failure that ended the gate's exchange, if one did.
    #[must_use]
    pub fn failure(&self) -> Option<String> {
        self.state.read(|state| state.failure.clone())
    }
}

/// What the exchange acts on next.
enum Step<P> {
    Cancelled,
    Received(usize, Option<EventEnvelope<P>>),
}

/// One started exchange of a [`CommitGate`].
struct Exchanging<P> {
    gate: Gate<P>,
    /// The input of each lane, `None` once the lane ended.
    inputs: Vec<Option<Box<dyn InputEndpoint<P>>>>,
    outputs: Vec<Box<dyn OutputEndpoint<P>>>,
    state: Arc<Watched<GateState>>,
}

impl<P: GatePayload> Exchanging<P> {
    /// The next envelope of any lane still going, the lanes asked in order, unless the gate is
    /// cancelled first.
    async fn step(&mut self) -> Step<P> {
        let state = Arc::clone(&self.state);
        let inputs = &mut self.inputs;
        future::poll_fn(|cx| {
            {
                let mut guard = state.lock();
                if guard.0.cancelled {
                    return Poll::Ready(Step::Cancelled);
                }
                guard.1 = Some(cx.waker().clone());
            }
            for (lane, input) in inputs.iter_mut().enumerate() {
                if let Some(input) = input
                    && let Poll::Ready(received) = input.recv().as_mut().poll(cx)
                {
                    return Poll::Ready(Step::Received(lane, received));
                }
            }
            Poll::Pending
        })
        .await
    }

    /// Sends what the lane `lane` passes, recording each `TurnCompleted` once it was passed.
    async fn pass(&mut self, lane: usize, passed: Vec<EventEnvelope<P>>) -> Result<(), String> {
        for envelope in passed {
            if self.state.read(|state| state.cancelled) {
                return Ok(());
            }
            let completed = match envelope.payload.gated() {
                Gated::Turn(TurnEvent::Completed(completed))
                    if ControlKind::of(&envelope.kind).is_none() =>
                {
                    Some(completed.turn_id)
                }
                _ => None,
            };
            self.outputs[lane]
                .send(envelope)
                .await
                .map_err(|refused| refused.to_string())?;
            if let Some(turn_id) = completed {
                let at = Instant::now();
                self.state
                    .update(|state| state.record.turns.push(PassedTurn { turn_id, at }));
            }
        }
        Ok(())
    }

    /// Takes `received` on the lane `lane` and passes what it lets through.
    async fn take(
        &mut self,
        lane: usize,
        received: Option<EventEnvelope<P>>,
    ) -> Result<(), String> {
        let gate = &mut self.gate;
        let passed = match received {
            Some(envelope) => {
                let stop = ControlKind::of(&envelope.kind) == Some(ControlKind::Stop);
                let passed = self
                    .state
                    .update(|state| gate.take(lane, envelope, &mut state.record));
                if stop {
                    self.inputs[lane] = None;
                }
                passed
            }
            None => {
                self.inputs[lane] = None;
                self.state
                    .update(|state| gate.close(lane, &mut state.record))
            }
        };
        self.pass(lane, passed).await
    }

    /// Takes what the lanes other than `lane` already hold, without waiting.
    async fn take_held(&mut self, lane: usize) -> Result<(), String> {
        let mut context = Context::from_waker(Waker::noop());
        for other in (0..self.inputs.len()).filter(|other| *other != lane) {
            while let Some(input) = self.inputs[other].as_mut() {
                let Poll::Ready(received) = input.recv().as_mut().poll(&mut context) else {
                    break;
                };
                self.take(other, received).await?;
            }
        }
        Ok(())
    }

    async fn exchange(&mut self) -> Result<(), String> {
        while self.inputs.iter().any(Option::is_some) {
            match self.step().await {
                Step::Cancelled => return Ok(()),
                Step::Received(lane, received) => {
                    let turn = received.as_ref().is_some_and(|envelope| {
                        ControlKind::of(&envelope.kind).is_none()
                            && matches!(envelope.payload.gated(), Gated::Turn(_))
                    });
                    if turn {
                        self.take_held(lane).await?;
                    }
                    self.take(lane, received).await?;
                }
            }
        }
        Ok(())
    }

    fn run(mut self) {
        let ended = block_on(self.exchange());
        self.state.update(|state| state.failure = ended.err());
    }
}

impl CommitGate {
    /// The gate's exchange over `endpoints`, once it was described and handed one edge at the
    /// input and one at the output port of each lane; otherwise the error saying what it was
    /// handed.
    fn exchanging<P: GatePayload>(
        &self,
        endpoints: NodeEndpoints<P>,
    ) -> Result<Exchanging<P>, NodeError> {
        if self.node_id.is_none() {
            return Err(NodeError::new(
                "the node was started before it was described",
            ));
        }
        let lanes = self.lanes.len();
        if lanes == 0 || endpoints.inputs.len() != lanes || endpoints.outputs.len() != lanes {
            return Err(NodeError::new(format!(
                "the gate of {lanes} lane(s) takes one input and one output edge per lane and \
                 was handed {} and {}",
                endpoints.inputs.len(),
                endpoints.outputs.len()
            )));
        }
        let NodeEndpoints { inputs, outputs } = endpoints;
        let mut by_lane_in: Vec<Option<Box<dyn InputEndpoint<P>>>> =
            (0..lanes).map(|_| None).collect();
        let mut by_lane_out: Vec<Option<Box<dyn OutputEndpoint<P>>>> =
            (0..lanes).map(|_| None).collect();
        for input in inputs {
            if let Some(lane) = self
                .lanes
                .iter()
                .position(|lane| lane.input == input.port_id)
                && by_lane_in[lane].is_none()
            {
                by_lane_in[lane] = Some(input.endpoint);
            }
        }
        for output in outputs {
            if let Some(lane) = self
                .lanes
                .iter()
                .position(|lane| lane.output == output.port_id)
                && by_lane_out[lane].is_none()
            {
                by_lane_out[lane] = Some(output.endpoint);
            }
        }
        let outputs: Option<Vec<_>> = by_lane_out.into_iter().collect();
        match outputs {
            Some(outputs) if by_lane_in.iter().all(Option::is_some) => Ok(Exchanging {
                gate: Gate::new(lanes),
                inputs: by_lane_in,
                outputs,
                state: Arc::clone(&self.state),
            }),
            _ => Err(NodeError::new(
                "the gate was not handed one edge at each port of its lanes",
            )),
        }
    }
}

impl<P: GatePayload> NodeImplementation<P> for CommitGate {
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.node_id = Some(node.node_id());
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn start(&mut self, endpoints: NodeEndpoints<P>) -> NodeFuture<'_, Result<(), NodeError>> {
        let started = self.exchanging(endpoints).map(|exchanging| {
            self.task.spawn(move || exchanging.run());
        });
        Box::pin(future::ready(started))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(async move {
            self.task.finished().await.map_err(NodeError::new)?;
            self.failure()
        })
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        self.state.update(|state| state.cancelled = true);
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(async move {
            let _already_reported_by_drain = self.task.finished().await;
        })
    }
}

#[cfg(test)]
mod tests {
    use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
    use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
    use inference_text::{SegmentId, SpanId, TurnCompleted, TurnId, TurnOpened};

    use super::{Gate, GatePayload, GateRecord, Gated, RefusedRevision, TurnEvent};

    /// The payload of the cases: what the gate reads, or a control event.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Payload {
        Content(Gated),
        Control(ControlKind),
    }

    impl GatePayload for Payload {
        fn gated(&self) -> Gated {
            match self {
                Self::Content(gated) => *gated,
                Self::Control(_) => Gated::Other,
            }
        }
    }

    /// Envelopes of one stream, numbered from 1.
    struct Stream {
        stream_id: StreamId,
        sequence: i64,
    }

    impl Stream {
        fn new() -> Self {
            Self {
                stream_id: StreamId::generate(),
                sequence: 0,
            }
        }

        fn envelope(&mut self, payload: Payload) -> EventEnvelope<Payload> {
            self.sequence += 1;
            let kind = match payload {
                Payload::Control(kind) => kind.as_str().to_owned(),
                Payload::Content(_) => "content".to_owned(),
            };
            EventEnvelope {
                schema_version: 1,
                event_id: EventId::generate(),
                run_id: RunId::generate(),
                session_id: None,
                stream_id: self.stream_id,
                sequence: self.sequence,
                producer: NodeId::generate(),
                port: "out".to_owned(),
                kind,
                generation_id: None,
                timestamp: 0,
                timebase: Timebase::WallTime,
                causation_id: None,
                payload,
            }
        }

        fn content(&mut self, gated: Gated) -> EventEnvelope<Payload> {
            self.envelope(Payload::Content(gated))
        }
    }

    fn payloads(passed: Vec<EventEnvelope<Payload>>) -> Vec<Payload> {
        passed
            .into_iter()
            .map(|envelope| envelope.payload)
            .collect()
    }

    #[test]
    fn the_input_gate_passes_committed_spans_and_turn_events_and_holds_back_hypotheses() {
        let mut gate = Gate::new(2);
        let mut record = GateRecord::default();
        let (mut transcript, mut turns) = (Stream::new(), Stream::new());
        let span_id = SpanId::generate();
        let turn_id = TurnId::generate();
        let opened = Gated::Turn(TurnEvent::Opened(TurnOpened {
            turn_id,
            run_id: RunId::generate(),
        }));
        let completed = Gated::Turn(TurnEvent::Completed(TurnCompleted { turn_id }));

        assert_eq!(
            payloads(gate.take(1, turns.content(opened), &mut record)),
            [Payload::Content(opened)]
        );
        let hypothesis = Gated::Hypothesis { span_id };
        assert_eq!(
            gate.take(0, transcript.content(hypothesis), &mut record),
            [],
            "a hypothesis is held back"
        );
        let revised = Gated::Revision { span_id };
        assert_eq!(
            gate.take(0, transcript.content(revised), &mut record),
            [],
            "and so is the revision of a span not yet committed"
        );
        let committed = Gated::SpanCommitted { span_id };
        assert_eq!(
            payloads(gate.take(0, transcript.content(committed), &mut record)),
            [Payload::Content(committed)]
        );
        assert_eq!(
            payloads(gate.take(1, turns.content(completed), &mut record)),
            [Payload::Content(completed)]
        );
        assert_eq!(record.held_back, 2);
        assert_eq!(record.refused, []);
    }

    #[test]
    fn a_revision_of_a_span_the_gate_passed_as_committed_is_refused_and_never_passed() {
        let mut gate = Gate::new(1);
        let mut record = GateRecord::default();
        let mut transcript = Stream::new();
        let span_id = SpanId::generate();
        let committed = Gated::SpanCommitted { span_id };
        assert_eq!(
            payloads(gate.take(0, transcript.content(committed), &mut record)),
            [Payload::Content(committed)]
        );
        let revision = transcript.content(Gated::Revision { span_id });
        let event_id = revision.event_id;
        assert_eq!(gate.take(0, revision, &mut record), [], "never passed");
        assert_eq!(record.refused, [RefusedRevision { span_id, event_id }]);
        assert_eq!(record.held_back, 0, "refused, not held back");

        let other = SpanId::generate();
        let committed = Gated::SpanCommitted { span_id: other };
        assert_eq!(
            payloads(gate.take(0, transcript.content(committed), &mut record)),
            [Payload::Content(committed)],
            "the next committed span still passes"
        );
    }

    #[test]
    fn a_tentative_delta_is_held_until_its_commit_and_then_passed_with_it() {
        let mut gate = Gate::new(1);
        let mut record = GateRecord::default();
        let mut text = Stream::new();
        let (segment_id, generation_id) = (SegmentId::generate(), GenerationId::generate());
        let delta = Gated::Delta {
            segment_id,
            generation_id,
        };
        assert_eq!(
            gate.take(0, text.content(delta), &mut record),
            [],
            "a Tentative segment never reaches the TTS node"
        );
        let committed = Gated::SegmentCommitted { segment_id };
        let passed = gate.take(0, text.content(committed), &mut record);
        assert_eq!(
            passed
                .iter()
                .map(|envelope| (envelope.sequence, envelope.payload))
                .collect::<Vec<_>>(),
            [
                (1, Payload::Content(delta)),
                (2, Payload::Content(committed))
            ],
            "passed with its commit, in arrival order, the envelopes unchanged"
        );
        assert_eq!(record.committed, [segment_id]);
        assert_eq!(record.dropped, []);
    }

    #[test]
    fn a_tentative_delta_never_committed_is_dropped_when_its_generation_ends() {
        let mut gate = Gate::new(1);
        let mut record = GateRecord::default();
        let mut text = Stream::new();
        let generation_id = GenerationId::generate();
        let (first, tentative) = (SegmentId::generate(), SegmentId::generate());
        let delta = |segment_id| Gated::Delta {
            segment_id,
            generation_id,
        };
        let started = Gated::Other;
        assert_eq!(
            payloads(gate.take(0, text.content(started), &mut record)),
            [Payload::Content(started)]
        );
        assert_eq!(gate.take(0, text.content(delta(first)), &mut record), []);
        let committed = Gated::SegmentCommitted { segment_id: first };
        assert_eq!(
            payloads(gate.take(0, text.content(committed), &mut record)),
            [Payload::Content(delta(first)), Payload::Content(committed)]
        );
        assert_eq!(
            gate.take(0, text.content(delta(tentative)), &mut record),
            [],
            "the delta injected directly, never committed"
        );
        let other_generation = Gated::GenerationEnded {
            generation_id: GenerationId::generate(),
        };
        assert_eq!(
            gate.take(0, text.content(other_generation), &mut record),
            [],
            "another generation's end waits behind the held delta and drops nothing"
        );
        assert_eq!(record.dropped, []);
        let ended = Gated::GenerationEnded { generation_id };
        assert_eq!(
            payloads(gate.take(0, text.content(ended), &mut record)),
            [Payload::Content(other_generation), Payload::Content(ended)],
            "the Tentative segment is never passed"
        );
        assert_eq!(record.dropped, [tentative]);
        assert_eq!(record.committed, [first]);
    }

    #[test]
    fn what_arrives_behind_a_held_delta_waits_so_a_lane_keeps_its_order() {
        let mut gate = Gate::new(1);
        let mut record = GateRecord::default();
        let mut text = Stream::new();
        let generation_id = GenerationId::generate();
        let (first, second) = (SegmentId::generate(), SegmentId::generate());
        let delta = |segment_id| Gated::Delta {
            segment_id,
            generation_id,
        };
        assert_eq!(gate.take(0, text.content(delta(first)), &mut record), []);
        assert_eq!(gate.take(0, text.content(delta(second)), &mut record), []);
        let commit = |segment_id| Gated::SegmentCommitted { segment_id };
        assert_eq!(
            payloads(gate.take(0, text.content(commit(first)), &mut record)),
            [Payload::Content(delta(first))],
            "the first commit waits behind the second delta, still Tentative"
        );
        let passed = gate.take(0, text.content(commit(second)), &mut record);
        assert_eq!(
            passed
                .iter()
                .map(|envelope| envelope.sequence)
                .collect::<Vec<_>>(),
            [2, 3, 4],
            "the rest in arrival order once the second is committed"
        );
    }

    #[test]
    fn the_end_of_input_drops_the_held_deltas_and_passes_the_stop() {
        let mut gate = Gate::new(1);
        let mut record = GateRecord::default();
        let mut text = Stream::new();
        let segment_id = SegmentId::generate();
        let delta = Gated::Delta {
            segment_id,
            generation_id: GenerationId::generate(),
        };
        assert_eq!(gate.take(0, text.content(delta), &mut record), []);
        let stop = Payload::Control(ControlKind::Stop);
        assert_eq!(
            payloads(gate.take(0, text.envelope(stop), &mut record)),
            [stop]
        );
        assert_eq!(record.dropped, [segment_id]);

        let mut closed = Gate::new(1);
        let mut record = GateRecord::default();
        assert_eq!(closed.take(0, text.content(delta), &mut record), []);
        assert_eq!(closed.close(0, &mut record), []);
        assert_eq!(record.dropped, [segment_id], "a closed lane drops it too");
    }
}
