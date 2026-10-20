//! The TTS node of a cascade whose playback a media sink acknowledges: a node of role `model`
//! over any [`Synthesiser`] of `inference-tts`, the [`MockSynthesiser`](crate::tts::MockSynthesiser)
//! among them, with two input ports and one output port, as [`SpeechPorts`] names them.
//!
//! When it starts, the node starts one synthesis on its synthesiser, which runs until the end of
//! input. On `text` it takes the output of an LLM node, as a commit gate passes it, and rebuilds
//! each output segment from the events that name it, as the testkit's
//! [`TtsNode`](crate::tts::TtsNode) does: a `GenerationStarted` makes the generation known, its
//! producer being the envelope's, and is sent on, on `audio`, so that it reaches the sink ahead of
//! every frame of the generation and names the generation's epoch there; a `TextDelta` of a known
//! generation emits the Tentative segment
//! under the delta's identity; the segment's `TextSegmentCommitted` commits it and offers it to
//! the synthesis, `inference.stream.QueueForSynthesis`, and the node emits its `SegmentQueued`. A
//! segment never committed is never offered (`docs/design/inference.md` 626).
//!
//! On `audio` it sends what the synthesis emits: every frame as the synthesis's
//! [`SegmentAudio`], which is not `Clone`, in an envelope of the node's audio stream on the audio
//! frame clock naming the segment's generation, so that the sink it reaches is the only holder of
//! the frames; and each segment's `SynthesisCompleted` once the last frame of the segment was
//! sent, whether the synthesis emitted the frames before its completion or with it. It never
//! acknowledges playback itself. On `played`, over a feedback edge from the sink, it takes the
//! sink's receipts, each the [`PlaybackReceipt`] of one segment, and runs
//! `inference.stream.AcknowledgePlayback` with it through its synthesis handle, which moves the
//! segment from Synthesized to Played; the node records the segment as the acknowledgement left
//! it ([`SpeechHandle::played`]) and its `AudioPlayed` in the run's [`RunLog`]. Given a playback
//! port ([`SpeechNode::with_playback`]), an output port of its own, it also sends each
//! `AudioPlayed` there, so that the node whose output it speaks learns what was played. A
//! receipt the synthesis refuses fails the node, but for the receipt of a segment the node
//! discarded, which the sink made before it learnt of the discard: it is recorded
//! ([`SpeechHandle::receipts_of_discarded`]) and moves nothing.
//!
//! A `GenerationSuperseded` on `text` is the interruption of that generation
//! (`docs/design/inference.md` 632-641): the node marks the generation Superseded, so that it
//! takes no further delta of it, sends the `GenerationSuperseded` on, on `audio`, and discards
//! every segment of the generation not yet Played, `inference.stream.DiscardSegment`, a Tentative
//! one it holds and each one in its synthesis, through the synthesis handle; it sends each
//! `SegmentDiscarded` on and records the segment as the discard left it
//! ([`SpeechHandle::discarded`]). A discarded segment is never offered again, and its
//! `SynthesisCompleted` is not sent. The node does not judge frames: one the synthesis delivers
//! after the discard, of a discarded segment, is sent like any other, and the sink, the
//! irreversible end of the output, refuses it as stale. A `GenerationCancelled` is taken the same
//! way: the generation is marked Cancelled, the event sent on and its unplayed segments discarded
//! (`ess/domains/stream.yaml`: superseding or cancelling a generation discards its unplayed
//! segments).
//!
//! Every `inference.stream` event it emits is recorded in the run's log in its envelope. On the
//! end of input on `text`, a control event of kind `stop`, or once that edge is closed, it ends
//! the utterance, sends what the synthesis has left and passes the end of input on, on `audio`;
//! it then goes on taking receipts until the end of input on `played`, or until that edge is
//! closed, passes the end of input on, on its playback port if it has one, and ends. Cancelled,
//! it sends nothing more: a cancel taken while a send waits for room
//! ends the wait. Content other than an `inference.stream` event on `text`, or a receipt on
//! `played`, is taken and ignored; a segment the synthesis refuses fails the node.

use std::collections::HashMap;
use std::fmt;
use std::future;
use std::pin::pin;
use std::sync::{Arc, Weak};
use std::task::Poll;

use inference_core::endpoint::{InputEndpoint, OutputEndpoint};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::generation::{Generation, GenerationState, StartGeneration};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_pipeline::{Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation, PortId};
use inference_text::{OutputSegment, SegmentId, SegmentSnapshot, SegmentState, SynthesisCompleted};
use inference_tts::{PlaybackReceipt, SegmentAudio, SynthesisEvent, SynthesisHandle, Synthesiser};

use crate::graph::nodes::{
    Emitter, Exchange, PipelinePayload, RunLog, Watched, block_on, send_unless,
};
use crate::stream::StreamEvent;

/// What a payload of the edges around a [`SpeechNode`] carries, as the node reads it.
#[derive(Debug)]
pub enum Said {
    /// An `inference.stream` event.
    Stream(StreamEvent),
    /// The sink's receipt of the playback of the segment `segment_id`.
    Receipt {
        segment_id: SegmentId,
        receipt: PlaybackReceipt,
    },
    /// A control event.
    Control(ControlKind),
    /// Anything else.
    Other,
}

/// The payload of the edges around a [`SpeechNode`]: `inference.stream` events, the frames of a
/// synthesis, the receipts of their playback and control events.
pub trait SpeechPayload: From<ControlKind> + Send + 'static {
    /// The payload carrying `event`.
    fn stream(event: StreamEvent) -> Self;

    /// The payload carrying `audio`, a frame of a segment as the synthesis emitted it.
    fn speech(audio: SegmentAudio) -> Self;

    /// What the payload carries, taken out of it.
    fn said(self) -> Said;
}

/// The ports of a [`SpeechNode`]: its inputs `text` and `played`, and its output `audio`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeechPorts {
    pub text: PortId,
    pub played: PortId,
    pub audio: PortId,
}

/// A receipt the node acknowledged playback with: the segment and how many frames it was made
/// of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Acknowledged {
    pub segment_id: SegmentId,
    pub frames: usize,
}

/// What the node was asked, and what its synthesis came to.
#[derive(Debug, Default)]
struct State {
    cancel: bool,
    queued: Vec<SegmentId>,
    acknowledged: Vec<Acknowledged>,
    played: Vec<SegmentSnapshot>,
    discarded: Vec<SegmentSnapshot>,
    receipts_of_discarded: Vec<SegmentId>,
    failure: Option<String>,
}

impl fmt::Debug for Watched<State> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.read(|state| state.fmt(f))
    }
}

/// The TTS node; see the module documentation.
pub struct SpeechNode<S> {
    run_id: RunId,
    synthesiser: S,
    ports: SpeechPorts,
    playback: Option<PortId>,
    log: RunLog,
    node_id: Option<NodeId>,
    state: Arc<Watched<State>>,
    exchange: Exchange,
}

impl<S: fmt::Debug> fmt::Debug for SpeechNode<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpeechNode")
            .field("run_id", &self.run_id)
            .field("synthesiser", &self.synthesiser)
            .field("ports", &self.ports)
            .field("playback", &self.playback)
            .field("node_id", &self.node_id)
            .field("state", &self.state)
            .field("exchange", &self.exchange)
            .finish_non_exhaustive()
    }
}

impl<S: Synthesiser> SpeechNode<S> {
    /// A TTS node of the run `run_id` synthesising on `synthesiser` through `ports`, recording
    /// what it emits in `log`.
    #[must_use]
    pub fn new(run_id: RunId, synthesiser: S, ports: SpeechPorts, log: RunLog) -> Self {
        Self {
            run_id,
            synthesiser,
            ports,
            playback: None,
            log,
            node_id: None,
            state: Arc::new(Watched::new(State::default())),
            exchange: Exchange::new(),
        }
    }

    /// The same node given a playback port, `playback`, an output port of its own at which it
    /// sends each `AudioPlayed`; see the module documentation.
    #[must_use]
    pub fn with_playback(mut self, playback: PortId) -> Self {
        self.playback = Some(playback);
        self
    }

    /// The test's side of the node.
    #[must_use]
    pub fn handle(&self) -> SpeechHandle {
        SpeechHandle {
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

/// The test's side of a [`SpeechNode`].
#[derive(Debug, Clone)]
pub struct SpeechHandle {
    state: Arc<Watched<State>>,
    task: Weak<()>,
}

impl SpeechHandle {
    /// Every segment the node offered to its synthesis, `QueueForSynthesis` asked of it, in the
    /// order offered.
    #[must_use]
    pub fn queued(&self) -> Vec<SegmentId> {
        self.state.read(|state| state.queued.clone())
    }

    /// Every receipt the synthesis took, in the order taken.
    #[must_use]
    pub fn acknowledged(&self) -> Vec<Acknowledged> {
        self.state.read(|state| state.acknowledged.clone())
    }

    /// Every segment whose playback a receipt acknowledged, as the acknowledgement left it, in
    /// the order played.
    #[must_use]
    pub fn played(&self) -> Vec<SegmentSnapshot> {
        self.state.read(|state| state.played.clone())
    }

    /// Every segment the node discarded on a supersede, as the discard left it, in the order
    /// discarded.
    #[must_use]
    pub fn discarded(&self) -> Vec<SegmentSnapshot> {
        self.state.read(|state| state.discarded.clone())
    }

    /// Every segment a receipt named after the node discarded it, in the order taken; such a
    /// receipt moves nothing.
    #[must_use]
    pub fn receipts_of_discarded(&self) -> Vec<SegmentId> {
        self.state.read(|state| state.receipts_of_discarded.clone())
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
fn carry<P: SpeechPayload>(envelope: EventEnvelope<PipelinePayload>) -> EventEnvelope<P> {
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
    Text(Option<EventEnvelope<P>>),
    Played(Option<EventEnvelope<P>>),
}

/// One started exchange of a [`SpeechNode`].
struct Exchanging<P, H> {
    run_id: RunId,
    producer: NodeId,
    port: String,
    synthesis: H,
    text: Box<dyn InputEndpoint<P>>,
    played: Box<dyn InputEndpoint<P>>,
    audio: Box<dyn OutputEndpoint<P>>,
    /// The edge at the playback port, if the node was given one, and its emitter.
    playback: Option<(Box<dyn OutputEndpoint<P>>, Emitter)>,
    text_ended: bool,
    played_ended: bool,
    emitter: Emitter,
    /// The node's audio stream and the last sequence sent on it.
    audio_stream: StreamId,
    audio_sequence: i64,
    generations: HashMap<GenerationId, Generation>,
    tentative: HashMap<SegmentId, OutputSegment>,
    /// The generation of every segment offered to the synthesis.
    offered: HashMap<SegmentId, GenerationId>,
    /// The frames sent of each segment.
    sent: HashMap<SegmentId, usize>,
    /// The completions whose segment still has frames to send, in the order they came.
    completing: Vec<SynthesisCompleted>,
    state: Arc<Watched<State>>,
}

impl<P: SpeechPayload, H: SynthesisHandle> Exchanging<P, H> {
    /// The next thing to act on: a cancel first, then an envelope of `played`, then of `text`.
    async fn step(&mut self) -> Step<P> {
        let state = Arc::clone(&self.state);
        let (text_ended, played_ended) = (self.text_ended, self.played_ended);
        let (text, played) = (&mut self.text, &mut self.played);
        future::poll_fn(|cx| {
            let cancelled = pin!(state.until(|state| state.cancel.then_some(())));
            if cancelled.poll(cx).is_ready() {
                return Poll::Ready(Step::Cancel);
            }
            if !played_ended && let Poll::Ready(received) = played.recv().as_mut().poll(cx) {
                return Poll::Ready(Step::Played(received));
            }
            if !text_ended && let Poll::Ready(received) = text.recv().as_mut().poll(cx) {
                return Poll::Ready(Step::Text(received));
            }
            Poll::Pending
        })
        .await
    }

    /// Sends `envelope` on `audio`, unless the node is cancelled first or while the send waits
    /// for room.
    async fn send(&self, envelope: EventEnvelope<P>) -> Result<(), String> {
        send_unless(
            self.audio.as_ref(),
            &self.state,
            |state| state.cancel,
            envelope,
        )
        .await
    }

    /// Sends `event` on, recorded in the run's log.
    async fn emit(&mut self, event: StreamEvent) -> Result<(), String> {
        let envelope = self
            .emitter
            .stream(event)
            .map_err(|refusal| refusal.to_string())?;
        self.send(carry(envelope)).await
    }

    /// Sends `audio` on the node's audio stream.
    async fn frame(&mut self, audio: SegmentAudio) -> Result<(), String> {
        let segment_id = audio.segment_id();
        self.audio_sequence += 1;
        let timestamp = i64::try_from(audio.frame().position.get()).unwrap_or(i64::MAX);
        let envelope = EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: self.run_id,
            session_id: None,
            stream_id: self.audio_stream,
            sequence: self.audio_sequence,
            producer: self.producer,
            port: self.port.clone(),
            kind: "inference.audio.AudioFrame".to_owned(),
            generation_id: self.offered.get(&segment_id).copied(),
            timestamp,
            timebase: Timebase::AudioFrameTime,
            causation_id: None,
            payload: P::speech(audio),
        };
        self.send(envelope).await?;
        *self.sent.entry(segment_id).or_default() += 1;
        Ok(())
    }

    /// Emits each completion held whose segment has no frame left to send, in the order they
    /// came.
    async fn complete(&mut self) -> Result<(), String> {
        let output = self.synthesis.output();
        let sent = &self.sent;
        let (ready, waiting): (Vec<_>, Vec<_>) = self.completing.drain(..).partition(|completed| {
            let emitted = output
                .frames_emitted(completed.segment_id)
                .unwrap_or_default();
            sent.get(&completed.segment_id).copied().unwrap_or_default() >= emitted
        });
        self.completing = waiting;
        for completed in ready {
            self.emit(StreamEvent::SynthesisCompleted(completed))
                .await?;
        }
        Ok(())
    }

    /// Sends every event the synthesis has ready.
    async fn pump(&mut self) -> Result<(), String> {
        while let Some(event) = self.synthesis.next() {
            match event {
                SynthesisEvent::Audio(audio) => self.frame(audio).await?,
                SynthesisEvent::Completed(completed) => {
                    let discarded = self
                        .synthesis
                        .output()
                        .segment(completed.segment_id)
                        .is_some_and(|segment| segment.state() == SegmentState::Discarded);
                    if !discarded {
                        self.completing.push(completed);
                    }
                }
            }
            self.complete().await?;
        }
        Ok(())
    }

    /// Acts on `event`, which arrived on `text` from `producer`.
    async fn heard(&mut self, producer: NodeId, event: StreamEvent) -> Result<(), String> {
        match event {
            StreamEvent::GenerationStarted(started) => {
                let (generation, _) = Generation::start_as(
                    started.generation_id,
                    StartGeneration {
                        run_id: started.run_id,
                        node_id: producer,
                        epoch: started.epoch,
                    },
                );
                self.generations.insert(started.generation_id, generation);
                self.emit(StreamEvent::GenerationStarted(started)).await?;
            }
            StreamEvent::GenerationSuperseded(superseded) => {
                self.interrupted(
                    superseded.generation_id,
                    StreamEvent::GenerationSuperseded(superseded),
                )
                .await?;
            }
            StreamEvent::GenerationCancelled(cancelled) => {
                self.interrupted(
                    cancelled.generation_id,
                    StreamEvent::GenerationCancelled(cancelled),
                )
                .await?;
            }
            StreamEvent::TextDelta(delta) => {
                if let Some(generation) = self
                    .generations
                    .get(&delta.generation_id)
                    .filter(|generation| generation.state() == GenerationState::Active)
                {
                    let (segment, _) = OutputSegment::emit_as(
                        delta.segment_id,
                        generation,
                        delta.sequence,
                        delta.text,
                    )
                    .map_err(|refused| refused.to_string())?;
                    self.tentative.insert(delta.segment_id, segment);
                }
            }
            StreamEvent::TextSegmentCommitted(committed) => {
                if let Some(segment) = self.tentative.remove(&committed.segment_id) {
                    let generation_id = segment.snapshot().generation_id();
                    let (segment, _) = segment.commit().map_err(|refused| refused.to_string())?;
                    let queued = self
                        .synthesis
                        .offer_segment(segment)
                        .map_err(|refused| format!("{refused:?}"))?;
                    self.offered.insert(committed.segment_id, generation_id);
                    self.state
                        .update(|state| state.queued.push(committed.segment_id));
                    self.emit(StreamEvent::SegmentQueued(queued)).await?;
                    self.pump().await?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// The generation `generation_id` was superseded or cancelled, as `event` says: marks it so,
    /// discards every segment of it not yet Played, sends `event` on and then each
    /// `SegmentDiscarded`, and sends what the synthesis delivers after the discards.
    async fn interrupted(
        &mut self,
        generation_id: GenerationId,
        event: StreamEvent,
    ) -> Result<(), String> {
        if let Some(generation) = self.generations.get_mut(&generation_id) {
            let moved = match event {
                StreamEvent::GenerationCancelled(_) => generation.cancel().map(|(moved, _)| moved),
                _ => generation.supersede().map(|(moved, _)| moved),
            };
            if let Ok(moved) = moved {
                *generation = moved;
            }
        }
        self.emit(event).await?;
        let mut tentative: Vec<SegmentId> = self
            .tentative
            .iter()
            .filter(|(_, segment)| segment.snapshot().generation_id() == generation_id)
            .map(|(segment_id, _)| *segment_id)
            .collect();
        tentative.sort_by_key(|segment_id| {
            self.tentative
                .get(segment_id)
                .map(|segment| segment.snapshot().sequence())
        });
        for segment_id in tentative {
            let Some(segment) = self.tentative.remove(&segment_id) else {
                continue;
            };
            let (segment, event) = segment.discard().map_err(|refused| refused.to_string())?;
            self.state
                .update(|state| state.discarded.push(segment.snapshot()));
            self.emit(StreamEvent::SegmentDiscarded(event)).await?;
        }
        let offered: Vec<SegmentId> = self
            .synthesis
            .output()
            .segments()
            .iter()
            .filter(|segment| self.offered.get(&segment.segment_id()) == Some(&generation_id))
            .filter(|segment| {
                !matches!(
                    segment.state(),
                    SegmentState::Played | SegmentState::Discarded
                )
            })
            .map(OutputSegment::segment_id)
            .collect();
        for segment_id in offered {
            let event = self
                .synthesis
                .discard(segment_id)
                .map_err(|refused| format!("{refused:?}"))?;
            let discarded = self
                .synthesis
                .output()
                .segment(segment_id)
                .map(OutputSegment::snapshot)
                .ok_or_else(|| format!("the synthesis no longer holds segment {segment_id}"))?;
            self.completing
                .retain(|completed| completed.segment_id != segment_id);
            self.state.update(|state| state.discarded.push(discarded));
            self.emit(StreamEvent::SegmentDiscarded(event)).await?;
        }
        self.pump().await
    }

    /// Runs `inference.stream.AcknowledgePlayback` on `segment_id` with the sink's `receipt`, and
    /// sends its `AudioPlayed` on the playback port, if the node has one. A receipt of a segment
    /// the node discarded, made before the sink learnt of the discard, is recorded and moves
    /// nothing.
    async fn acknowledge(
        &mut self,
        segment_id: SegmentId,
        receipt: PlaybackReceipt,
    ) -> Result<(), String> {
        if self
            .synthesis
            .output()
            .segment(segment_id)
            .is_some_and(|segment| segment.state() == SegmentState::Discarded)
        {
            self.state
                .update(|state| state.receipts_of_discarded.push(segment_id));
            return Ok(());
        }
        let frames = receipt.frames();
        let event = self
            .synthesis
            .acknowledge_playback(segment_id, receipt)
            .map_err(|refused| format!("the synthesis refused the sink's receipt: {refused:?}"))?;
        let played = self
            .synthesis
            .output()
            .segment(segment_id)
            .map(OutputSegment::snapshot)
            .ok_or_else(|| format!("the synthesis no longer holds segment {segment_id}"))?;
        let event = StreamEvent::AudioPlayed(event);
        if let Some((output, emitter)) = self.playback.as_mut() {
            let envelope = emitter
                .stream(event)
                .map_err(|refusal| refusal.to_string())?;
            send_unless(
                output.as_ref(),
                &self.state,
                |state| state.cancel,
                carry(envelope),
            )
            .await?;
        } else {
            self.emitter
                .stream(event)
                .map_err(|refusal| refusal.to_string())?;
        }
        self.state.update(|state| {
            state.acknowledged.push(Acknowledged { segment_id, frames });
            state.played.push(played);
        });
        Ok(())
    }

    /// The end of input on `text`: ends the utterance, sends what the synthesis has left and
    /// passes the end of input on.
    async fn end_of_text(&mut self) -> Result<(), String> {
        self.text_ended = true;
        let _ = self.synthesis.end_utterance();
        self.pump().await?;
        self.complete().await?;
        let end = self.emitter.control(ControlKind::Stop);
        self.send(end).await
    }

    async fn exchanged(&mut self) -> Result<(), String> {
        while !(self.text_ended && self.played_ended) {
            match self.step().await {
                Step::Cancel => return Ok(()),
                Step::Played(None) => self.played_ended = true,
                Step::Played(Some(envelope)) => match envelope.payload.said() {
                    Said::Receipt {
                        segment_id,
                        receipt,
                    } => self.acknowledge(segment_id, receipt).await?,
                    Said::Control(ControlKind::Stop) => self.played_ended = true,
                    Said::Stream(_) | Said::Control(_) | Said::Other => {}
                },
                Step::Text(None) => self.end_of_text().await?,
                Step::Text(Some(envelope)) => {
                    let producer = envelope.producer;
                    match envelope.payload.said() {
                        Said::Stream(event) => self.heard(producer, event).await?,
                        Said::Control(ControlKind::Stop) => self.end_of_text().await?,
                        Said::Receipt { .. } | Said::Control(_) | Said::Other => {}
                    }
                }
            }
        }
        if let Some((output, emitter)) = self.playback.as_mut() {
            let end = emitter.control(ControlKind::Stop);
            send_unless(output.as_ref(), &self.state, |state| state.cancel, end).await?;
        }
        Ok(())
    }

    fn run(mut self) {
        let failure = block_on(self.exchanged()).err();
        self.state.update(|state| state.failure = failure);
    }
}

impl<S> SpeechNode<S>
where
    S: Synthesiser,
    S::Handle: Send + 'static,
{
    /// The node's exchange over `endpoints`, once it was described and handed one edge at each
    /// of its ports; otherwise the error saying what it was handed.
    fn exchanging<P: SpeechPayload>(
        &mut self,
        endpoints: NodeEndpoints<P>,
    ) -> Result<Exchanging<P, S::Handle>, NodeError> {
        let producer = self
            .node_id
            .ok_or_else(|| NodeError::new("the node was started before it was described"))?;
        let ports = self.ports;
        let outputs_wanted = 1 + usize::from(self.playback.is_some());
        if endpoints.inputs.len() != 2 || endpoints.outputs.len() != outputs_wanted {
            return Err(NodeError::new(format!(
                "the node takes one input edge at each of its ports {} and {} and one output \
                 edge at its port {} and at its playback port, if it was given one, and was \
                 handed {} and {}",
                ports.text,
                ports.played,
                ports.audio,
                endpoints.inputs.len(),
                endpoints.outputs.len()
            )));
        }
        let NodeEndpoints { inputs, outputs } = endpoints;
        let (mut text, mut played) = (None, None);
        for input in inputs {
            match input.port_id {
                port if port == ports.text && text.is_none() => text = Some(input.endpoint),
                port if port == ports.played && played.is_none() => {
                    played = Some(input.endpoint);
                }
                _ => {}
            }
        }
        let (mut audio, mut playback) = (None, None);
        for output in outputs {
            match output.port_id {
                port if port == ports.audio && audio.is_none() => audio = Some(output.endpoint),
                port if Some(port) == self.playback && playback.is_none() => {
                    let emitter =
                        Emitter::new(self.run_id, producer, port.to_string(), self.log.clone());
                    playback = Some((output.endpoint, emitter));
                }
                _ => {}
            }
        }
        let (Some(text), Some(played), Some(audio), true) = (
            text,
            played,
            audio,
            playback.is_some() == self.playback.is_some(),
        ) else {
            return Err(NodeError::new(
                "the node was not handed one edge at each of its ports",
            ));
        };
        let synthesis = self
            .synthesiser
            .start()
            .map_err(|refused| NodeError::new(refused.to_string()))?;
        let port = ports.audio.to_string();
        Ok(Exchanging {
            run_id: self.run_id,
            producer,
            emitter: Emitter::new(self.run_id, producer, port.clone(), self.log.clone()),
            port,
            synthesis,
            text,
            played,
            audio,
            playback,
            text_ended: false,
            played_ended: false,
            audio_stream: StreamId::generate(),
            audio_sequence: 0,
            generations: HashMap::new(),
            tentative: HashMap::new(),
            offered: HashMap::new(),
            sent: HashMap::new(),
            completing: Vec::new(),
            state: Arc::clone(&self.state),
        })
    }
}

impl<P, S> NodeImplementation<P> for SpeechNode<S>
where
    P: SpeechPayload,
    S: Synthesiser + Send,
    S::Handle: Send + 'static,
{
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.node_id = Some(node.node_id());
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn start(&mut self, endpoints: NodeEndpoints<P>) -> NodeFuture<'_, Result<(), NodeError>> {
        let started = self.exchanging(endpoints).map(|exchanging| {
            self.exchange.spawn(move || exchanging.run());
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
