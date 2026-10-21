//! The TTS node of a voice pipeline: a node of role `model` whose implementation is an
//! `inference-tts` [`Synthesiser`], the [`MockSynthesiser`] by default and a local adapter's
//! otherwise, with one input edge carrying the output of an LLM node, as the testkit's model node
//! emits it, and one output edge carrying audio.
//!
//! When it starts, the node starts one synthesis on its synthesiser, which runs until the end of
//! input. It rebuilds each output segment from the events that name it: a `GenerationStarted`
//! makes the generation known, its producer being the envelope's; a `TextDelta` of a known
//! generation emits the Tentative segment under the delta's identity; the segment's
//! `TextSegmentCommitted` commits it and offers it to the synthesis, which queues it, and the
//! node emits its `SegmentQueued`. TTS consumes committed segments only (`docs/design/inference.md`
//! 626): a segment never committed is never offered. Every frame the synthesis emits is sent on
//! as an audio frame, in an envelope of the node's audio stream on the audio frame clock naming
//! the segment's generation, and each segment's `SynthesisCompleted` is emitted once its last
//! frame was sent.
//!
//! Playing ([`TtsNode::playing`]), the node is also its pipeline's playback: it gives every frame
//! to a [`MockSink`] as it sends it, and once a segment is Synthesized it acknowledges its
//! playback through the sink, `inference.stream.AcknowledgePlayback`, which moves the segment to
//! Played, and emits its `AudioPlayed`. The Played segments are read from the node's handle
//! ([`TtsHandle::played`]).
//!
//! Given a usage port ([`TtsNode::with_usage`]), the output port its pipeline declares under the
//! name `usage`, the node sends on the edge or exposure leaving it the usage record of its
//! synthesis, as `inference-tts` delivers it once the utterance has ended, after the synthesis's
//! last segment: an envelope of kind `inference.usage.UsageRecorded` on a stream of its own,
//! whose record the node places on the run, itself as the node and the session its envelopes
//! name, none. The record is neither recorded in the [`RunLog`], which holds `inference.stream`
//! events, nor sent on the audio port. The port convention is the model node's
//! (`story:usage-llm`, design 684-688): the edge or exposure leaving it uses an overload policy
//! that never drops (`pause` or `fail`). A node with no edge leaving its usage port, or given
//! none, or on a payload that carries no usage ([`TtsPayload::usage`]), sends no record. A
//! synthesis the node drops before its utterance ended, as a cancel does, yields none.
//!
//! Every `inference.stream` event it emits is recorded in the run's [`RunLog`] in its envelope. On
//! the end of input, a control event of kind `stop`, or once its input edge is closed, it ends the
//! utterance, delivers what the synthesis has left, its usage record last, and passes the end of
//! input on, on every output port, `usage` first, so that a consumer that has the end of `audio`
//! has the end of `usage` too. Cancelled, it sends nothing more but the usage record of a
//! synthesis it already holds, which a cancel never loses: a cancel taken while a send waits for
//! room ends the wait, a send on `usage` excepted. Every other
//! event, such as `GenerationCompleted`, is taken and ignored; a segment the synthesis refuses
//! fails the node.

use std::collections::HashMap;
use std::fmt;
use std::future;
use std::sync::{Arc, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use inference_core::endpoint::{InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::generation::{Generation, StartGeneration};
use inference_core::id::{GenerationId, NodeId, RunId, SessionId, StreamId};
use inference_core::usage::UsageRecorded;
use inference_pipeline::{Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation, PortId};
use inference_text::{OutputSegment, SegmentId, SegmentSnapshot};
use inference_tts::inference_audio::AudioFrame;
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};

use super::{MockSink, MockSynthesiser};
use crate::graph::nodes::{
    Emitter, Exchange, PipelinePayload, RunLog, Watched, block_on, recv_unless, send_unless,
    started,
};
use crate::stream::StreamEvent;

/// What a payload of the edge into a [`TtsNode`] carries, as the node reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Spoken {
    /// An `inference.stream` event.
    Stream(StreamEvent),
    /// A control event.
    Control(ControlKind),
    /// Anything else.
    Other,
}

/// The payload of the edges around a [`TtsNode`]: `inference.stream` events, frames of audio
/// and control events.
pub trait TtsPayload: From<ControlKind> + Send + 'static {
    /// The payload carrying `event`.
    fn stream(event: StreamEvent) -> Self;

    /// The payload carrying `frame`.
    fn audio(frame: AudioFrame) -> Self;

    /// The payload carrying the usage record `recorded`; none, the default, for a payload that
    /// carries no usage, on whose edges the node sends no record.
    fn usage(recorded: UsageRecorded) -> Option<Self> {
        let _ = recorded;
        None
    }

    /// What the payload carries.
    fn spoken(self) -> Spoken;
}

/// What the node was asked, and what its synthesis came to.
#[derive(Debug, Default)]
struct State {
    cancel: bool,
    played: Vec<SegmentSnapshot>,
    failure: Option<String>,
}

impl fmt::Debug for Watched<State> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.read(|state| state.fmt(f))
    }
}

/// The TTS node over the synthesiser `S`; see the module documentation.
pub struct TtsNode<S = MockSynthesiser> {
    run_id: RunId,
    synthesiser: S,
    playing: bool,
    usage: Option<PortId>,
    log: RunLog,
    node_id: Option<NodeId>,
    state: Arc<Watched<State>>,
    exchange: Exchange,
}

impl<S: fmt::Debug> fmt::Debug for TtsNode<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TtsNode")
            .field("run_id", &self.run_id)
            .field("synthesiser", &self.synthesiser)
            .field("playing", &self.playing)
            .field("usage", &self.usage)
            .field("node_id", &self.node_id)
            .field("state", &self.state)
            .field("exchange", &self.exchange)
            .finish_non_exhaustive()
    }
}

impl<S: Synthesiser> TtsNode<S> {
    /// A TTS node of the run `run_id` synthesising on `synthesiser`, recording what it emits in
    /// `log`.
    #[must_use]
    pub fn new(run_id: RunId, synthesiser: S, log: RunLog) -> Self {
        Self {
            run_id,
            synthesiser,
            playing: false,
            usage: None,
            log,
            node_id: None,
            state: Arc::new(Watched::new(State::default())),
            exchange: Exchange::new(),
        }
    }

    /// The same node, which plays what it synthesises through a [`MockSink`] and acknowledges
    /// the playback of each segment; see the module documentation.
    #[must_use]
    pub const fn playing(mut self) -> Self {
        self.playing = true;
        self
    }

    /// The same node given a usage port, `usage`, the output port its pipeline declares under the
    /// name `usage`: once an edge or exposure leaves it, the node sends there the usage record of
    /// its synthesis; see the module documentation.
    #[must_use]
    pub const fn with_usage(mut self, usage: PortId) -> Self {
        self.usage = Some(usage);
        self
    }

    /// The test's side of the node.
    #[must_use]
    pub fn handle(&self) -> TtsHandle {
        TtsHandle {
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

/// The test's side of a [`TtsNode`].
#[derive(Debug, Clone)]
pub struct TtsHandle {
    state: Arc<Watched<State>>,
    task: Weak<()>,
}

impl TtsHandle {
    /// Every segment whose playback the node acknowledged, as the acknowledgement left it, in
    /// the order it was played.
    #[must_use]
    pub fn played(&self) -> Vec<SegmentSnapshot> {
        self.state.read(|state| state.played.clone())
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
fn carry<P: TtsPayload>(envelope: EventEnvelope<PipelinePayload>) -> EventEnvelope<P> {
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
            PipelinePayload::Usage(_) => {
                unreachable!("the emitter of this node wraps no usage record")
            }
        },
    }
}

/// The `usage` port of a started node given one: the edge or exposure leaving it, on which the
/// node sends the usage record of its synthesis as one stream of its own, numbered from 1.
struct UsageOut<P> {
    output: Box<dyn OutputEndpoint<P>>,
    run_id: RunId,
    producer: NodeId,
    /// The session the node's envelopes name: none, as every envelope of the node names none.
    session_id: Option<SessionId>,
    port: String,
    stream_id: StreamId,
    sequence: i64,
}

impl<P: TtsPayload> UsageOut<P> {
    /// The envelope of `recorded`, placed on the run, node and session the envelope names; none
    /// when the payload carries no usage.
    fn envelope(&mut self, mut recorded: UsageRecorded) -> Option<EventEnvelope<P>> {
        recorded.run_id = Some(self.run_id);
        recorded.node_id = Some(self.producer);
        recorded.session_id = self.session_id;
        let payload = P::usage(recorded)?;
        Some(self.next(UsageRecorded::KIND.to_owned(), payload))
    }

    /// The next envelope on the node's `usage` stream, stamped on the wall clock.
    fn next(&mut self, kind: String, payload: P) -> EventEnvelope<P> {
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
            session_id: self.session_id,
            stream_id: self.stream_id,
            sequence: self.sequence,
            producer: self.producer,
            port: self.port.clone(),
            kind,
            generation_id: None,
            timestamp,
            timebase: Timebase::WallTime,
            causation_id: None,
            payload,
        }
    }
}

/// One started exchange of a [`TtsNode`].
struct Exchanging<P, H> {
    run_id: RunId,
    producer: NodeId,
    port: String,
    synthesis: H,
    sink: Option<MockSink>,
    input: Box<dyn InputEndpoint<P>>,
    output: Box<dyn OutputEndpoint<P>>,
    /// The node's `usage` port, given one with an edge or exposure leaving it.
    usage: Option<UsageOut<P>>,
    emitter: Emitter,
    /// The node's audio stream and the last sequence sent on it.
    audio_stream: StreamId,
    audio_sequence: i64,
    generations: HashMap<GenerationId, Generation>,
    tentative: HashMap<SegmentId, OutputSegment>,
    /// The generation of every segment offered to the synthesis.
    offered: HashMap<SegmentId, GenerationId>,
    state: Arc<Watched<State>>,
}

impl<P: TtsPayload, H: SynthesisHandle> Exchanging<P, H> {
    /// Sends `envelope`, unless the node is cancelled first or while the send waits for room.
    async fn send(&self, envelope: EventEnvelope<P>) -> Result<(), String> {
        send_unless(
            self.output.as_ref(),
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

    /// Sends `frame` of the segment `segment_id` on the node's audio stream.
    async fn frame(&mut self, segment_id: SegmentId, frame: AudioFrame) -> Result<(), String> {
        self.audio_sequence += 1;
        let timestamp = i64::try_from(frame.position.get()).unwrap_or(i64::MAX);
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
            payload: P::audio(frame),
        };
        self.send(envelope).await
    }

    /// Delivers every event the synthesis has ready: frames sent on and given to the sink, each
    /// completion emitted and, playing, the segment's playback acknowledged.
    async fn pump(&mut self) -> Result<(), String> {
        while let Some(event) = self.synthesis.next() {
            match event {
                SynthesisEvent::Audio(audio) => {
                    let (segment_id, frame) = (audio.segment_id(), audio.frame().clone());
                    if let Some(sink) = self.sink.as_mut() {
                        sink.give(audio);
                    }
                    self.frame(segment_id, frame).await?;
                }
                SynthesisEvent::Completed(completed) => {
                    self.emit(StreamEvent::SynthesisCompleted(completed))
                        .await?;
                    if let Some(sink) = self.sink.as_mut() {
                        let (played, event) = sink
                            .acknowledge(&mut self.synthesis, completed.segment_id)
                            .map_err(|refused| format!("{refused:?}"))?;
                        self.state.update(|state| state.played.push(played));
                        self.emit(StreamEvent::AudioPlayed(event)).await?;
                    }
                }
                SynthesisEvent::Usage(recorded) => self.send_usage(recorded).await?,
            }
        }
        Ok(())
    }

    /// Sends `recorded`, the usage of the synthesis, on the `usage` port, placed on the run, the
    /// node and the session of its envelope; a node with no edge leaving its `usage` port sends
    /// nothing.
    /// It sends the record whether or not the node was cancelled, waiting for room as long as
    /// the edge takes it, so a cancel never loses the usage of a call the node already holds;
    /// only an edge that is closed once the node was cancelled takes it no more.
    async fn send_usage(&mut self, recorded: UsageRecorded) -> Result<(), String> {
        let Some(usage) = self.usage.as_mut() else {
            return Ok(());
        };
        let Some(envelope) = usage.envelope(recorded) else {
            return Ok(());
        };
        match usage.output.send(envelope).await {
            Ok(()) => Ok(()),
            Err(SendError::Closed(_)) if self.state.read(|state| state.cancel) => Ok(()),
            Err(refused) => Err(refused.to_string()),
        }
    }

    /// Passes the end of input on, on the `usage` port, given one with an edge leaving it.
    async fn end_usage(&mut self) -> Result<(), String> {
        let Some(usage) = self.usage.as_mut() else {
            return Ok(());
        };
        let end = usage.next(
            ControlKind::Stop.as_str().to_owned(),
            P::from(ControlKind::Stop),
        );
        send_unless(
            usage.output.as_ref(),
            &self.state,
            |state| state.cancel,
            end,
        )
        .await
    }

    /// Acts on `event`, which arrived from `producer`.
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
            }
            StreamEvent::TextDelta(delta) => {
                if let Some(generation) = self.generations.get(&delta.generation_id) {
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
                    self.emit(StreamEvent::SegmentQueued(queued)).await?;
                    self.pump().await?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn exchanged(&mut self) -> Result<(), String> {
        loop {
            let state = Arc::clone(&self.state);
            let received = recv_unless(self.input.as_mut(), &state, |state| state.cancel).await;
            let Some(received) = received else {
                return Ok(());
            };
            let Some(envelope) = received else {
                break;
            };
            let producer = envelope.producer;
            match envelope.payload.spoken() {
                Spoken::Stream(event) => self.heard(producer, event).await?,
                Spoken::Control(ControlKind::Stop) => break,
                Spoken::Control(_) | Spoken::Other => {}
            }
        }
        let _ = self.synthesis.end_utterance();
        self.pump().await?;
        self.end_usage().await?;
        let end = self.emitter.control(ControlKind::Stop);
        self.send(end).await
    }

    fn run(mut self) {
        let failure = block_on(self.exchanged()).err();
        self.state.update(|state| state.failure = failure);
    }
}

/// The synthesis runs on the exchange's thread.
impl<P, S> NodeImplementation<P> for TtsNode<S>
where
    P: TtsPayload,
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

    fn start(&mut self, mut endpoints: NodeEndpoints<P>) -> NodeFuture<'_, Result<(), NodeError>> {
        let usage = self
            .usage
            .and_then(|port| {
                endpoints
                    .outputs
                    .iter()
                    .position(|output| output.port_id == port)
            })
            .map(|at| endpoints.outputs.remove(at));
        let started = started(self.node_id, &endpoints, 1, 1).and_then(|producer| {
            let synthesis = self
                .synthesiser
                .start()
                .map_err(|refused| NodeError::new(refused.to_string()))?;
            let input = endpoints.inputs.remove(0).endpoint;
            let output = endpoints.outputs.remove(0);
            let port = output.port_id.to_string();
            let exchanging = Exchanging {
                run_id: self.run_id,
                producer,
                emitter: Emitter::new(self.run_id, producer, port.clone(), self.log.clone()),
                port,
                synthesis,
                sink: self.playing.then(MockSink::new),
                input,
                output: output.endpoint,
                usage: usage.map(|usage| UsageOut {
                    run_id: self.run_id,
                    producer,
                    session_id: None,
                    port: usage.port_id.to_string(),
                    output: usage.endpoint,
                    stream_id: StreamId::generate(),
                    sequence: 0,
                }),
                audio_stream: StreamId::generate(),
                audio_sequence: 0,
                generations: HashMap::new(),
                tentative: HashMap::new(),
                offered: HashMap::new(),
                state: Arc::clone(&self.state),
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

#[cfg(test)]
mod tests {
    use std::num::{NonZeroU16, NonZeroU32};

    use inference_core::model::{Adapter, ModelId};
    use inference_tts::ConcurrencyLimitReached;
    use inference_tts::inference_audio::{AudioEncoding, AudioFormat};

    use super::*;
    use crate::tts::MockConfiguration;

    /// A payload of stream events, frames and control events.
    #[derive(Debug, Clone)]
    enum Payload {
        Stream(StreamEvent),
        Audio,
        Control(ControlKind),
    }

    impl From<ControlKind> for Payload {
        fn from(kind: ControlKind) -> Self {
            Self::Control(kind)
        }
    }

    impl TtsPayload for Payload {
        fn stream(event: StreamEvent) -> Self {
            Self::Stream(event)
        }

        fn audio(_frame: AudioFrame) -> Self {
            Self::Audio
        }

        fn spoken(self) -> Spoken {
            match self {
                Self::Stream(event) => Spoken::Stream(event),
                Self::Control(kind) => Spoken::Control(kind),
                Self::Audio => Spoken::Other,
            }
        }
    }

    /// A synthesiser that is not the mock: the mock behind a type of its own, as an adapter's
    /// synthesiser is.
    #[derive(Debug, Clone)]
    struct Other(MockSynthesiser);

    impl Synthesiser for Other {
        type Handle = <MockSynthesiser as Synthesiser>::Handle;

        fn adapter(&self) -> &Adapter {
            self.0.adapter()
        }

        fn start(&mut self) -> Result<Self::Handle, ConcurrencyLimitReached> {
            self.0.start()
        }
    }

    fn implementation<P, N: NodeImplementation<P>>(node: N) -> N {
        node
    }

    #[test]
    fn the_node_synthesises_on_any_synthesiser_and_on_the_mock_by_default() {
        let mock = MockSynthesiser::new(
            MockConfiguration::IncrementalStreaming,
            ModelId::generate(),
            AudioFormat {
                encoding: AudioEncoding::PcmS16le,
                sample_rate: NonZeroU32::new(16_000).expect("16000"),
                channels: NonZeroU16::new(1).expect("1"),
            },
        );
        let other: TtsNode<Other> = implementation::<Payload, _>(
            TtsNode::new(RunId::generate(), Other(mock.clone()), RunLog::new()).playing(),
        );
        let default: TtsNode =
            implementation::<Payload, _>(TtsNode::new(RunId::generate(), mock, RunLog::new()));
        assert!(other.handle().played().is_empty());
        assert!(default.handle().played().is_empty());
        assert_eq!(other.synthesiser.adapter(), default.synthesiser.adapter());
    }
}
