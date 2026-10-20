//! The mock STT node of a voice pipeline: a node of role `model` whose implementation is a
//! [`MockRecogniser`], with two input ports, the frames of the utterance on `audio` and the turn
//! events of the turn policy on `turns`, and the output ports [`SttPorts`] names.
//!
//! It recognises one turn at a time, under the turn the turn policy opened, on one transcript
//! stream of its own whose sequence each recognition continues:
//!
//! * at a turn's `TurnOpened` it starts a recognition of that turn and offers it every frame that
//!   arrived since the previous turn completed, frames before the onset among them; while the turn
//!   is Open it offers each frame as it arrives;
//! * at the turn's `TurnCompleted`, read on the audio frame clock, it first takes the frames up to
//!   that position, then ends the recognition's input, so that every cue left in the script runs,
//!   and delivers every event the recognition has left. Only then does it send the
//!   `TurnCompleted` on, on its `turns_out` port, so that a consumer of `committed_text` and
//!   `turns_out` has every committed span of a turn before the turn's completion;
//! * every transcript event is recorded in the run's [`RunLog`] in its envelope and sent on
//!   `transcript`, if that port is wired, and a `TranscriptCommitted` also on `committed_text`.
//!   The `TurnOpened` is passed on as well, on `turns_out`, before the turn's first transcript
//!   event.
//!
//! Once both inputs ended, each by a control event of kind `stop` or by its edge closing, a
//! recognition still running has its input ended and its events delivered, and the node passes
//! the end of input on, on every output port. Cancelled, it sends nothing more: a cancel taken
//! while a send waits for room ends the wait. Content other than a frame on `audio` fails the
//! node; on `turns` every event but the two turn events is taken and ignored. The spans each
//! recognition emitted are read from its handle ([`SttHandle::spans`]).

use std::collections::VecDeque;
use std::fmt;
use std::future;
use std::pin::pin;
use std::sync::{Arc, Weak};
use std::task::Poll;

use inference_core::endpoint::{InputEndpoint, OutputEndpoint};
use inference_core::envelope::{ControlKind, EventEnvelope, Timebase};
use inference_core::id::{NodeId, RunId, StreamId};
use inference_pipeline::{Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation, PortId};
use inference_stt::inference_audio::AudioFrame;
use inference_stt::{Recogniser, RecognitionHandle, RecognitionRequest, TranscriptEvent};
use inference_text::{Span, TurnId};

use super::{MockRecogniser, MockRecognition, stream_envelope};
use crate::graph::nodes::{
    Emitter, Exchange, PipelinePayload, RunLog, Watched, block_on, send_unless,
};
use crate::stream::StreamEvent;

/// What a payload of the edges around an [`SttNode`] carries, as the node reads it.
#[derive(Debug, Clone, PartialEq)]
pub enum Heard {
    /// A frame of audio.
    Audio(AudioFrame),
    /// An `inference.stream` event, such as a turn event.
    Stream(StreamEvent),
    /// A control event.
    Control(ControlKind),
    /// Anything else.
    Other,
}

/// The payload of the edges around an [`SttNode`]: frames, `inference.stream` events and control
/// events.
pub trait SttPayload: From<ControlKind> + Send + 'static {
    /// The payload carrying `event`.
    fn stream(event: StreamEvent) -> Self;

    /// What the payload carries.
    fn heard(self) -> Heard;
}

/// The ports of an [`SttNode`]: its inputs `audio` and `turns`, and its outputs `committed_text`,
/// `turns_out` and, optionally, `transcript`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SttPorts {
    pub audio: PortId,
    pub turns: PortId,
    pub committed_text: PortId,
    pub turns_out: PortId,
    pub transcript: Option<PortId>,
}

/// What the node was asked, and what its recognitions came to.
#[derive(Debug, Default)]
struct State {
    cancel: bool,
    spans: Vec<Span>,
    failure: Option<String>,
}

impl fmt::Debug for Watched<State> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.read(|state| state.fmt(f))
    }
}

/// The mock STT node; see the module documentation.
pub struct SttNode {
    run_id: RunId,
    recogniser: MockRecogniser,
    ports: SttPorts,
    log: RunLog,
    node_id: Option<NodeId>,
    state: Arc<Watched<State>>,
    exchange: Exchange,
}

impl fmt::Debug for SttNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SttNode")
            .field("run_id", &self.run_id)
            .field("recogniser", &self.recogniser)
            .field("ports", &self.ports)
            .field("node_id", &self.node_id)
            .field("state", &self.state)
            .field("exchange", &self.exchange)
            .finish_non_exhaustive()
    }
}

impl SttNode {
    /// An STT node of the run `run_id` recognising on `recogniser` through `ports`, recording
    /// what it emits in `log`. The recogniser is to claim `incremental_input`, so that a turn's
    /// frames reach its recognition before the turn completes.
    #[must_use]
    pub fn new(run_id: RunId, recogniser: MockRecogniser, ports: SttPorts, log: RunLog) -> Self {
        Self {
            run_id,
            recogniser,
            ports,
            log,
            node_id: None,
            state: Arc::new(Watched::new(State::default())),
            exchange: Exchange::new(),
        }
    }

    /// The test's side of the node.
    #[must_use]
    pub fn handle(&self) -> SttHandle {
        SttHandle {
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

/// The test's side of an [`SttNode`].
#[derive(Debug, Clone)]
pub struct SttHandle {
    state: Arc<Watched<State>>,
    task: Weak<()>,
}

impl SttHandle {
    /// The spans every finished recognition emitted, as each left them, in emission order.
    #[must_use]
    pub fn spans(&self) -> Vec<Span> {
        self.state.read(|state| state.spans.clone())
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
fn carry<P: SttPayload>(envelope: EventEnvelope<PipelinePayload>) -> EventEnvelope<P> {
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

/// One output edge of the node, with the emitter of its port and the node's state, whose cancel
/// ends a send waiting for room.
struct Out<P> {
    endpoint: Box<dyn OutputEndpoint<P>>,
    emitter: Emitter,
    state: Arc<Watched<State>>,
}

impl<P: SttPayload> Out<P> {
    /// Sends `envelope`, unless the node is cancelled first or while the send waits for room.
    async fn send(&self, envelope: EventEnvelope<P>) -> Result<(), String> {
        send_unless(
            self.endpoint.as_ref(),
            &self.state,
            |state| state.cancel,
            envelope,
        )
        .await
    }

    /// Sends `event` in an envelope of the port's own stream, recorded in the run's log.
    async fn emit(&mut self, event: StreamEvent) -> Result<(), String> {
        let envelope = self
            .emitter
            .stream(event)
            .map_err(|refusal| refusal.to_string())?;
        self.send(carry(envelope)).await
    }

    async fn end(&mut self) -> Result<(), String> {
        let end = self.emitter.control(ControlKind::Stop);
        self.send(end).await
    }
}

/// What the exchange acts on next.
enum Step<P> {
    Cancel,
    Audio(Option<EventEnvelope<P>>),
    Turns(Option<EventEnvelope<P>>),
}

/// How the exchange ended.
enum Ended {
    Completed,
    Cancelled,
}

/// One started exchange of an [`SttNode`].
struct Exchanging<P> {
    run_id: RunId,
    node_id: NodeId,
    recogniser: MockRecogniser,
    log: RunLog,
    state: Arc<Watched<State>>,
    audio: Box<dyn InputEndpoint<P>>,
    turns: Box<dyn InputEndpoint<P>>,
    audio_ended: bool,
    turns_ended: bool,
    committed_text: Out<P>,
    turns_out: Out<P>,
    transcript: Option<Out<P>>,
    /// The node's transcript stream and the sequence its next event takes.
    stream_id: StreamId,
    next_sequence: i64,
    /// The frames that arrived and were offered to no recognition yet.
    heard: VecDeque<AudioFrame>,
    /// The sample clock position just past the last frame that arrived.
    heard_until: i64,
    recognition: Option<MockRecognition>,
}

impl<P: SttPayload> Exchanging<P> {
    /// The next thing to act on: a cancel first, then an envelope of the audio edge and, unless
    /// `audio_only`, of the turns edge.
    async fn step(&mut self, audio_only: bool) -> Step<P> {
        let state = Arc::clone(&self.state);
        let (audio_ended, turns_ended) = (self.audio_ended, self.turns_ended);
        let (audio, turns) = (&mut self.audio, &mut self.turns);
        future::poll_fn(|cx| {
            let cancelled = pin!(state.until(|state| state.cancel.then_some(())));
            if cancelled.poll(cx).is_ready() {
                return Poll::Ready(Step::Cancel);
            }
            if !audio_ended && let Poll::Ready(received) = audio.recv().as_mut().poll(cx) {
                return Poll::Ready(Step::Audio(received));
            }
            if !audio_only
                && !turns_ended
                && let Poll::Ready(received) = turns.recv().as_mut().poll(cx)
            {
                return Poll::Ready(Step::Turns(received));
            }
            Poll::Pending
        })
        .await
    }

    /// Records `envelope` in the run's log and sends it on `transcript`, if wired, and, a
    /// committed span, on `committed_text`.
    async fn deliver(&mut self, envelope: EventEnvelope<TranscriptEvent>) -> Result<(), String> {
        self.next_sequence = envelope.sequence + 1;
        let committed = matches!(envelope.payload, TranscriptEvent::Committed(_));
        let envelope = stream_envelope(envelope);
        self.log
            .record(envelope.clone())
            .map_err(|refusal| refusal.to_string())?;
        let carried = |envelope: &EventEnvelope<StreamEvent>| EventEnvelope {
            schema_version: envelope.schema_version,
            event_id: envelope.event_id,
            run_id: envelope.run_id,
            session_id: envelope.session_id,
            stream_id: envelope.stream_id,
            sequence: envelope.sequence,
            producer: envelope.producer,
            port: envelope.port.clone(),
            kind: envelope.kind.clone(),
            generation_id: envelope.generation_id,
            timestamp: envelope.timestamp,
            timebase: envelope.timebase,
            causation_id: envelope.causation_id,
            payload: P::stream(envelope.payload.clone()),
        };
        if let Some(transcript) = &self.transcript {
            transcript.send(carried(&envelope)).await?;
        }
        if committed {
            self.committed_text.send(carried(&envelope)).await?;
        }
        Ok(())
    }

    /// Delivers every event the running recognition has ready.
    async fn pump(&mut self) -> Result<(), String> {
        while let Some(envelope) = self.recognition.as_mut().and_then(Iterator::next) {
            self.deliver(envelope).await?;
        }
        Ok(())
    }

    /// Offers the running recognition every frame heard and not yet offered, then delivers what
    /// it has ready.
    async fn offer_heard(&mut self) -> Result<(), String> {
        if let Some(recognition) = self.recognition.as_mut() {
            for frame in self.heard.drain(..) {
                recognition
                    .offer_frame(frame)
                    .map_err(|ended| ended.to_string())?;
            }
        }
        self.pump().await
    }

    /// Takes a frame, or the end of input, from the audio edge.
    async fn audio(&mut self, received: Option<EventEnvelope<P>>) -> Result<(), String> {
        let Some(envelope) = received else {
            self.audio_ended = true;
            return Ok(());
        };
        match envelope.payload.heard() {
            Heard::Audio(frame) => {
                let end = frame.position.get() + u64::from(frame.sample_count);
                self.heard_until = i64::try_from(end).unwrap_or(i64::MAX);
                self.heard.push_back(frame);
                self.offer_heard().await
            }
            Heard::Control(ControlKind::Stop) => {
                self.audio_ended = true;
                Ok(())
            }
            other => Err(format!(
                "the node takes audio frames on its audio port and was handed {other:?}"
            )),
        }
    }

    /// Starts the recognition of `turn_id` on the node's transcript stream.
    fn start_recognition(&mut self, turn_id: TurnId) -> Result<(), String> {
        let request = RecognitionRequest::continuing(
            self.run_id,
            turn_id,
            self.stream_id,
            self.next_sequence,
            self.node_id,
            "transcript",
        )
        .map_err(|refused| refused.to_string())?;
        let recognition = self
            .recogniser
            .start(request)
            .map_err(|refused| refused.to_string())?;
        self.recognition = Some(recognition);
        Ok(())
    }

    /// Ends the running recognition's input, delivers what it has left and keeps its spans.
    async fn finish_recognition(&mut self) -> Result<(), String> {
        let Some(recognition) = self.recognition.as_mut() else {
            return Ok(());
        };
        recognition.end_input().map_err(|ended| ended.to_string())?;
        self.pump().await?;
        if let Some(recognition) = self.recognition.take() {
            let spans = recognition.output().spans().to_vec();
            self.state.update(|state| state.spans.extend(spans));
        }
        Ok(())
    }

    /// Takes the frames up to the sample clock position `until`, or until the audio ends.
    async fn hear_until(&mut self, until: i64) -> Result<Option<Ended>, String> {
        while !self.audio_ended && self.heard_until < until {
            match self.step(true).await {
                Step::Cancel => return Ok(Some(Ended::Cancelled)),
                Step::Audio(received) => self.audio(received).await?,
                Step::Turns(_) => {}
            }
        }
        Ok(None)
    }

    /// Takes a turn event, or the end of input, from the turns edge.
    async fn turn(&mut self, received: Option<EventEnvelope<P>>) -> Result<Option<Ended>, String> {
        let Some(envelope) = received else {
            self.turns_ended = true;
            return Ok(None);
        };
        let (timebase, timestamp) = (envelope.timebase, envelope.timestamp);
        match envelope.payload.heard() {
            Heard::Stream(StreamEvent::TurnOpened(opened)) => {
                self.finish_recognition().await?;
                self.turns_out.emit(StreamEvent::TurnOpened(opened)).await?;
                self.start_recognition(opened.turn_id)?;
                self.offer_heard().await?;
            }
            Heard::Stream(StreamEvent::TurnCompleted(completed)) => {
                if timebase == Timebase::AudioFrameTime
                    && let Some(ended) = self.hear_until(timestamp).await?
                {
                    return Ok(Some(ended));
                }
                self.finish_recognition().await?;
                self.turns_out
                    .emit(StreamEvent::TurnCompleted(completed))
                    .await?;
            }
            Heard::Control(ControlKind::Stop) => self.turns_ended = true,
            Heard::Audio(_) | Heard::Stream(_) | Heard::Control(_) | Heard::Other => {}
        }
        Ok(None)
    }

    async fn exchanged(&mut self) -> Result<Ended, String> {
        loop {
            if self.audio_ended && self.turns_ended {
                self.finish_recognition().await?;
                self.committed_text.end().await?;
                self.turns_out.end().await?;
                if let Some(transcript) = self.transcript.as_mut() {
                    transcript.end().await?;
                }
                return Ok(Ended::Completed);
            }
            match self.step(false).await {
                Step::Cancel => return Ok(Ended::Cancelled),
                Step::Audio(received) => self.audio(received).await?,
                Step::Turns(received) => {
                    if let Some(ended) = self.turn(received).await? {
                        return Ok(ended);
                    }
                }
            }
        }
    }

    fn run(mut self) {
        let failure = block_on(self.exchanged()).err();
        self.state.update(|state| state.failure = failure);
    }
}

impl SttNode {
    /// The node's exchange over `endpoints`, once it was handed one edge at each of its input
    /// ports and at each of its output ports, `transcript` optional; otherwise the error saying
    /// what it was handed.
    fn exchanging<P: SttPayload>(
        &self,
        endpoints: NodeEndpoints<P>,
    ) -> Result<Exchanging<P>, NodeError> {
        let node_id = self
            .node_id
            .ok_or_else(|| NodeError::new("the node was started before it was described"))?;
        let ports = self.ports;
        let shape = || {
            NodeError::new(format!(
                "the node takes one input edge at each of its ports {} and {} and one output \
                 edge at each of its ports {}, {} and, if wired, {:?}, and was handed {} and {}",
                ports.audio,
                ports.turns,
                ports.committed_text,
                ports.turns_out,
                ports.transcript,
                endpoints.inputs.len(),
                endpoints.outputs.len()
            ))
        };
        let wired = endpoints.inputs.len() == 2
            && endpoints.outputs.len() == 2 + usize::from(ports.transcript.is_some());
        if !wired {
            return Err(shape());
        }
        let NodeEndpoints { inputs, outputs } = endpoints;
        let (mut audio, mut turns) = (None, None);
        for input in inputs {
            match input.port_id {
                port if port == ports.audio && audio.is_none() => audio = Some(input.endpoint),
                port if port == ports.turns && turns.is_none() => turns = Some(input.endpoint),
                _ => {}
            }
        }
        let out = |endpoint, port: PortId| Out {
            endpoint,
            emitter: Emitter::new(self.run_id, node_id, port.to_string(), self.log.clone()),
            state: Arc::clone(&self.state),
        };
        let (mut committed_text, mut turns_out, mut transcript) = (None, None, None);
        for output in outputs {
            match output.port_id {
                port if port == ports.committed_text && committed_text.is_none() => {
                    committed_text = Some(out(output.endpoint, port));
                }
                port if port == ports.turns_out && turns_out.is_none() => {
                    turns_out = Some(out(output.endpoint, port));
                }
                port if Some(port) == ports.transcript && transcript.is_none() => {
                    transcript = Some(out(output.endpoint, port));
                }
                _ => {}
            }
        }
        let (Some(audio), Some(turns), Some(committed_text), Some(turns_out)) =
            (audio, turns, committed_text, turns_out)
        else {
            return Err(NodeError::new(
                "the node was not handed one edge at each of its ports",
            ));
        };
        if ports.transcript.is_some() && transcript.is_none() {
            return Err(NodeError::new(
                "the node was not handed one edge at each of its ports",
            ));
        }
        Ok(Exchanging {
            run_id: self.run_id,
            node_id,
            recogniser: self.recogniser.clone(),
            log: self.log.clone(),
            state: Arc::clone(&self.state),
            audio,
            turns,
            audio_ended: false,
            turns_ended: false,
            committed_text,
            turns_out,
            transcript,
            stream_id: StreamId::generate(),
            next_sequence: 1,
            heard: VecDeque::new(),
            heard_until: 0,
            recognition: None,
        })
    }
}

impl<P: SttPayload> NodeImplementation<P> for SttNode {
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
