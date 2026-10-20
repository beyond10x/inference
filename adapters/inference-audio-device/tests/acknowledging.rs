//! The acknowledging sink against the fake device and in-test edges (`story:cascade-commit-gates`):
//! it plays the `SegmentAudio` a synthesis emitted and sends the `PlaybackReceipt` of a segment
//! only once its synthesis completed and the speaker's played position passed its last frame, a
//! receipt the synthesis takes to move the segment to Played; at the end of input it waits for
//! the last playback, then passes the end on and closes the speaker.

use std::collections::VecDeque;
use std::future::{self, Future};
use std::num::{NonZeroU16, NonZeroU32, NonZeroUsize};
use std::pin::pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_audio_device::acknowledging::{
    ACKNOWLEDGE_PLAYBACK, AcknowledgingSink, Playable, PlaybackPayload,
};
use inference_audio_device::convert::encode;
use inference_audio_device::fake::FakeHost;
use inference_audio_device::{DeviceSelector, PlayoutConfig, Speaker};
use inference_core::endpoint::{EndpointFuture, InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::generation::{Generation, StartGeneration};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, InteractionMode, Node, NodeEndpoints, NodeImplementation, NodeInput,
    NodeOutput, NodeRole, OverloadPolicy, PipelineSpec, PortId, PortTraits, modality,
};
use inference_tts::inference_text::{OutputSegment, SegmentId, SegmentState, TurnId};
use inference_tts::{CONTRACT_REVISION, PlaybackReceipt, SynthesisEvent, SynthesisOutput};

const PATIENCE: Duration = Duration::from_secs(5);

/// Samples per channel of each frame: 20 ms at 16 kHz.
const FRAME: u32 = 320;

/// Device frames of the fake laptop's speaker, at 48 kHz, holding one frame of the test.
const DEVICE_FRAME: usize = 960;

/// What the edges of the test carry: no variant is `Clone`, as `SegmentAudio` is not.
#[derive(Debug)]
enum Payload {
    Audio(inference_tts::SegmentAudio),
    Completed(SegmentId),
    Receipt(SegmentId, PlaybackReceipt),
    TurnOpened(TurnId),
    Superseded(GenerationId),
    Started(GenerationId, i64),
    Control(ControlKind),
    Other,
}

impl PlaybackPayload for Payload {
    fn playable(self) -> Playable {
        match self {
            Self::Audio(audio) => Playable::Audio(audio),
            Self::Completed(segment_id) => Playable::Completed(segment_id),
            Self::TurnOpened(turn_id) => Playable::TurnOpened(turn_id),
            Self::Superseded(generation_id) => Playable::Superseded(generation_id),
            Self::Started(generation_id, epoch) => Playable::Started {
                generation_id,
                epoch,
            },
            Self::Receipt(..) | Self::Control(_) | Self::Other => Playable::Other,
        }
    }

    fn receipt(segment_id: SegmentId, receipt: PlaybackReceipt) -> Self {
        Self::Receipt(segment_id, receipt)
    }

    fn control(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
            return output;
        }
        thread::park_timeout(Duration::from_millis(10));
    }
}

#[derive(Debug, Default)]
struct Queue {
    envelopes: VecDeque<EventEnvelope<Payload>>,
    closed: bool,
    waker: Option<Waker>,
}

/// An edge of the test: a queue the test reads and writes, unbounded.
#[derive(Debug, Clone, Default)]
struct Edge(Arc<Mutex<Queue>>);

impl Edge {
    fn with<T>(&self, change: impl FnOnce(&mut Queue) -> T) -> T {
        let mut queue = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let answer = change(&mut queue);
        if let Some(waker) = queue.waker.take() {
            waker.wake();
        }
        answer
    }

    fn put(&self, envelope: EventEnvelope<Payload>) {
        self.with(|queue| queue.envelopes.push_back(envelope));
    }

    fn close(&self) {
        self.with(|queue| queue.closed = true);
    }

    fn taken(&self) -> Vec<EventEnvelope<Payload>> {
        self.with(|queue| queue.envelopes.drain(..).collect())
    }

    fn len(&self) -> usize {
        self.with(|queue| queue.envelopes.len())
    }
}

impl OutputEndpoint<Payload> for Edge {
    fn send(
        &self,
        envelope: EventEnvelope<Payload>,
    ) -> EndpointFuture<'_, Result<(), SendError<Payload>>> {
        self.put(envelope);
        Box::pin(future::ready(Ok(())))
    }
}

impl InputEndpoint<Payload> for Edge {
    fn recv(&mut self) -> EndpointFuture<'_, Option<EventEnvelope<Payload>>> {
        let queue = Arc::clone(&self.0);
        Box::pin(future::poll_fn(move |cx| {
            let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(envelope) = queue.envelopes.pop_front() {
                Poll::Ready(Some(envelope))
            } else if queue.closed {
                Poll::Ready(None)
            } else {
                queue.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }))
    }
}

struct Profiles(DeploymentProfile);

impl Catalog for Profiles {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.0.profile_id() == profile_id).then_some(&self.0)
    }

    fn adapter(&self, _: AdapterId) -> Option<&Adapter> {
        None
    }
}

/// The declared sink node, its input port of speech and its output port of receipts.
struct Declared {
    sink: Node,
    speech: PortId,
    played: PortId,
    turns: PortId,
}

fn declared() -> Declared {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: vec![Locality::SameProcess],
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    let catalog = Profiles(profile);
    let spec = PipelineSpec {
        name: "speech to speaker".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let mut declaration = Declaration::new();
    let (_, (sink, speech, played, turns)) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let tts = pipeline.node("tts", NodeRole::Model, Vec::new());
            let sink = pipeline.node("speaker", NodeRole::Sink, Vec::new());
            let out = pipeline.output::<modality::Audio>(&tts, "audio", PortTraits::default());
            let speech = pipeline.input::<modality::Audio>(&sink, "audio", PortTraits::default());
            let played =
                pipeline.output::<modality::Control>(&sink, "played", PortTraits::default());
            let back = pipeline.input::<modality::Control>(&tts, "played", PortTraits::default());
            pipeline.connect(&out, &speech, NonZeroUsize::MIN, OverloadPolicy::Pause);
            pipeline.connect_feedback(
                &played,
                &back,
                NonZeroUsize::MIN,
                OverloadPolicy::Pause,
                NonZeroUsize::MIN,
            );
            let policy = pipeline.node("turn policy", NodeRole::Policy, Vec::new());
            let opened =
                pipeline.output::<modality::Control>(&policy, "turns", PortTraits::default());
            let turns = pipeline.input::<modality::Control>(&sink, "turns", PortTraits::default());
            pipeline.connect(&opened, &turns, NonZeroUsize::MIN, OverloadPolicy::Pause);
            (sink.id(), speech.id(), played.id(), turns.id())
        })
        .expect("the profile is declared");
    Declared {
        sink: declaration.node(sink).expect("a declared node").clone(),
        speech,
        played,
        turns,
    }
}

/// Waits at most [`PATIENCE`] until `ready` holds.
fn eventually(ready: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if ready() {
            return true;
        }
        thread::sleep(Duration::from_millis(1));
    }
    ready()
}

/// Plays the speaker on, one frame's time per pull, until the sink sent `count` envelopes, for
/// at most [`PATIENCE`].
fn played_until(run: &Started, count: usize) -> bool {
    eventually(|| {
        let _ = run.host.pull(DEVICE_FRAME);
        run.receipts.len() >= count
    }) && run.receipts.len() == count
}

/// Waits a while, to let the sink look at its speaker again, and answers what `edge` holds.
fn settled(edge: &Edge) -> usize {
    thread::sleep(Duration::from_millis(30));
    edge.len()
}

/// A synthesis claiming incremental input and output streaming, to make the frames a sink is
/// given.
struct Synthesis {
    output: SynthesisOutput,
    generation: Generation,
    sequence: i64,
    next_sample: u64,
}

impl Synthesis {
    fn new() -> Self {
        let (adapter, _) = Adapter::declare(DeclareAdapter {
            model_id: ModelId::generate(),
            backend: "acknowledging sink test".to_owned(),
            contract_revision: CONTRACT_REVISION.to_owned(),
            execution_form: ExecutionForm::InProcess,
            locality: Locality::SameProcess,
            capabilities: vec![Capability::IncrementalInput, Capability::OutputStreaming],
            max_concurrency: 1,
        })
        .expect("a positive concurrency");
        let (generation, _) = Generation::start(StartGeneration {
            run_id: RunId::generate(),
            node_id: NodeId::generate(),
            epoch: 1,
        });
        Self {
            output: SynthesisOutput::start(&adapter),
            generation,
            sequence: 0,
            next_sample: 0,
        }
    }

    fn generation_id(&self) -> GenerationId {
        self.generation.generation_id()
    }

    /// A committed segment, queued and taken.
    fn segment(&mut self, text: &str) -> SegmentId {
        self.sequence += 1;
        let (tentative, _) = OutputSegment::emit(&self.generation, self.sequence, text.to_owned())
            .expect("an Active generation");
        let (committed, _) = tentative.commit().expect("a Tentative segment commits");
        let segment_id = committed.segment_id();
        self.output
            .offer(committed)
            .expect("a Committed segment queues");
        self.output
            .take_input()
            .expect("incremental input is claimed");
        segment_id
    }

    /// The next frame of `segment_id`, at a level heard.
    fn frame(&mut self, segment_id: SegmentId) -> Payload {
        let frame = AudioFrame {
            format: AudioFormat {
                encoding: AudioEncoding::PcmS16le,
                sample_rate: NonZeroU32::new(16_000).expect("a rate"),
                channels: NonZeroU16::MIN,
            },
            position: SampleClockPosition::new(self.next_sample).expect("a position"),
            sample_count: FRAME,
            data: encode(&[0.25; FRAME as usize], AudioEncoding::PcmS16le),
        };
        self.next_sample += u64::from(FRAME);
        match self.output.audio(segment_id, frame) {
            Ok(SynthesisEvent::Audio(audio)) => Payload::Audio(audio),
            other => panic!("the synthesis emits a frame: {other:?}"),
        }
    }

    /// Completes `segment_id`, whose frames were all emitted.
    fn complete(&mut self, segment_id: SegmentId) -> Payload {
        let events = self
            .output
            .complete(segment_id, Vec::new())
            .expect("a Queued segment completes");
        assert!(matches!(events.as_slice(), [SynthesisEvent::Completed(_)]));
        Payload::Completed(segment_id)
    }

    fn state(&self, segment_id: SegmentId) -> Option<SegmentState> {
        self.output.segment(segment_id).map(OutputSegment::state)
    }
}

/// Envelopes of the TTS node's stream feeding the sink.
struct Feed {
    edge: Edge,
    stream_id: StreamId,
    sequence: i64,
    generation_id: GenerationId,
}

impl Feed {
    fn send(&mut self, payload: Payload) {
        self.sequence += 1;
        let kind = match &payload {
            Payload::Control(kind) => kind.as_str().to_owned(),
            Payload::Completed(_) => "inference.stream.SynthesisCompleted".to_owned(),
            Payload::Superseded(_) => "inference.stream.GenerationSuperseded".to_owned(),
            Payload::Started(..) => "inference.stream.GenerationStarted".to_owned(),
            _ => "inference.audio.AudioFrame".to_owned(),
        };
        self.edge.put(EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: RunId::generate(),
            session_id: None,
            stream_id: self.stream_id,
            sequence: self.sequence,
            producer: NodeId::generate(),
            port: "audio".to_owned(),
            kind,
            generation_id: Some(self.generation_id),
            timestamp: 0,
            timebase: Timebase::AudioFrameTime,
            causation_id: None,
            payload,
        });
    }
}

/// A started sink on the fake laptop's speaker, the edge feeding it and the edge of its receipts.
struct Started {
    host: FakeHost,
    sink: AcknowledgingSink,
    feed: Feed,
    receipts: Edge,
    synthesis: Synthesis,
}

fn started() -> Started {
    let nodes = declared();
    let host = FakeHost::laptop();
    let speaker = Speaker::open(&host, &DeviceSelector::Default, PlayoutConfig::default())
        .expect("the default output opens");
    let mut sink = AcknowledgingSink::new(RunId::generate(), speaker);
    let (speech, receipts) = (Edge::default(), Edge::default());
    let node: &mut dyn NodeImplementation<Payload> = &mut sink;
    block_on(node.describe(&nodes.sink));
    block_on(node.prepare()).expect("the sink prepares");
    block_on(node.start(NodeEndpoints {
        inputs: vec![NodeInput {
            port_id: nodes.speech,
            endpoint: Box::new(speech.clone()),
        }],
        outputs: vec![NodeOutput {
            port_id: nodes.played,
            endpoint: Box::new(receipts.clone()),
        }],
    }))
    .expect("the sink starts");
    let synthesis = Synthesis::new();
    Started {
        host,
        sink,
        feed: Feed {
            edge: speech,
            stream_id: StreamId::generate(),
            sequence: 0,
            generation_id: synthesis.generation_id(),
        },
        receipts,
        synthesis,
    }
}

/// The receipt `envelope` carries, and the segment it names.
fn receipt(envelope: EventEnvelope<Payload>) -> (SegmentId, PlaybackReceipt) {
    assert_eq!(envelope.kind, ACKNOWLEDGE_PLAYBACK);
    match envelope.payload {
        Payload::Receipt(segment_id, receipt) => (segment_id, receipt),
        other => panic!("a receipt, not {other:?}"),
    }
}

#[test]
fn a_segment_is_acknowledged_once_completed_and_played_past_its_last_frame() {
    let mut run = started();
    let handle = run.sink.handle();
    let segment = run.synthesis.segment("hello there");
    let first = run.synthesis.frame(segment);
    run.feed.send(first);
    let second = run.synthesis.frame(segment);
    run.feed.send(second);
    assert!(
        eventually(|| handle.pushed() == 2),
        "both frames reach the speaker"
    );
    assert_eq!(
        settled(&run.receipts),
        0,
        "nothing played yet, nothing acknowledged"
    );

    let _ = run.host.pull(4 * DEVICE_FRAME);
    assert_eq!(
        settled(&run.receipts),
        0,
        "played past both frames, but the synthesis has not completed the segment"
    );
    let completed = run.synthesis.complete(segment);
    run.feed.send(completed);
    assert!(
        played_until(&run, 1),
        "now that its last sample, finished, played, it is acknowledged"
    );

    let (named, receipt) = receipt(run.receipts.taken().remove(0));
    assert_eq!(named, segment);
    assert_eq!(
        receipt.frames(),
        2,
        "made of the two frames the sink was given"
    );
    assert_eq!(
        run.synthesis.state(segment),
        Some(SegmentState::Synthesized)
    );
    run.synthesis
        .output
        .acknowledge_playback(segment, receipt)
        .expect("the synthesis takes the sink's receipt");
    assert_eq!(run.synthesis.state(segment), Some(SegmentState::Played));
    let receipts = handle.receipts();
    assert_eq!(
        receipts
            .iter()
            .map(|sent| (sent.segment_id, sent.frames))
            .collect::<Vec<_>>(),
        [(segment, 2)]
    );
    assert_eq!(
        handle
            .first_frames()
            .iter()
            .map(|first| first.segment_id)
            .collect::<Vec<_>>(),
        [segment]
    );
    assert!(handle.first_frames()[0].at <= receipts[0].at);
    let node: &mut dyn NodeImplementation<Payload> = &mut run.sink;
    block_on(node.cancel());
    block_on(node.stop());
}

#[test]
fn a_completed_segment_waits_until_the_speaker_played_past_its_last_frame() {
    let mut run = started();
    let segment = run.synthesis.segment("one two three");
    for _ in 0..3 {
        let frame = run.synthesis.frame(segment);
        run.feed.send(frame);
    }
    let completed = run.synthesis.complete(segment);
    run.feed.send(completed);
    let handle = run.sink.handle();
    assert!(eventually(|| handle.pushed() == 3));
    let _ = run.host.pull(2 * DEVICE_FRAME);
    assert_eq!(
        settled(&run.receipts),
        0,
        "two of the three frames played: the segment is not yet heard"
    );
    assert!(played_until(&run, 1));
    let (named, receipt) = receipt(run.receipts.taken().remove(0));
    assert_eq!((named, receipt.frames()), (segment, 3));
    let node: &mut dyn NodeImplementation<Payload> = &mut run.sink;
    block_on(node.cancel());
    block_on(node.stop());
}

#[test]
fn a_segment_completed_without_frames_takes_the_receipt_of_no_frames() {
    let mut run = started();
    let segment = run.synthesis.segment("silent");
    let completed = run.synthesis.complete(segment);
    run.feed.send(completed);
    assert!(eventually(|| run.receipts.len() == 1));
    let (named, receipt) = receipt(run.receipts.taken().remove(0));
    assert_eq!((named, receipt.frames()), (segment, 0));
    run.synthesis
        .output
        .acknowledge_playback(segment, receipt)
        .expect("the receipt of no frames acknowledges a segment without frames");
    assert_eq!(run.synthesis.state(segment), Some(SegmentState::Played));
    let node: &mut dyn NodeImplementation<Payload> = &mut run.sink;
    block_on(node.cancel());
    block_on(node.stop());
}

#[test]
fn the_end_of_input_waits_for_the_last_playback_then_passes_stop_and_closes_the_speaker() {
    let mut run = started();
    let handle = run.sink.handle();
    let segment = run.synthesis.segment("goodbye");
    let frame = run.synthesis.frame(segment);
    run.feed.send(frame);
    let completed = run.synthesis.complete(segment);
    run.feed.send(completed);
    run.feed.send(Payload::Other);
    run.feed.send(Payload::Control(ControlKind::Stop));
    assert!(eventually(|| handle.pushed() == 1));
    assert_eq!(
        settled(&run.receipts),
        0,
        "the end of input waits for the playback of what the sink holds"
    );
    assert!(!handle.ended());
    assert!(run.host.output_open());

    assert!(played_until(&run, 2), "the receipt, then the end");
    let mut sent = run.receipts.taken();
    let end = sent.pop().expect("two envelopes");
    assert_eq!(end.kind, ControlKind::Stop.as_str());
    let (named, receipt) = receipt(sent.remove(0));
    assert_eq!((named, receipt.frames()), (segment, 1));

    let node: &mut dyn NodeImplementation<Payload> = &mut run.sink;
    block_on(node.drain()).expect("the sink drains");
    block_on(node.stop());
    assert!(handle.ended());
    assert!(!run.host.output_open(), "the speaker is closed");
    assert_eq!(handle.failure(), None);
}

#[test]
fn a_closed_input_ends_the_sink_as_the_end_of_input_does() {
    let mut run = started();
    let segment = run.synthesis.segment("closing");
    let completed = run.synthesis.complete(segment);
    run.feed.send(completed);
    run.feed.edge.close();
    assert!(eventually(|| run.receipts.len() == 2));
    let handle = run.sink.handle();
    assert!(eventually(|| handle.ended()));
    assert!(!run.host.output_open());
}

#[test]
fn cancelled_the_sink_acknowledges_nothing_more_and_closes_the_speaker() {
    let mut run = started();
    let handle = run.sink.handle();
    let segment = run.synthesis.segment("cut short");
    let frame = run.synthesis.frame(segment);
    run.feed.send(frame);
    let completed = run.synthesis.complete(segment);
    run.feed.send(completed);
    assert!(eventually(|| handle.pushed() == 1));
    let node: &mut dyn NodeImplementation<Payload> = &mut run.sink;
    block_on(node.cancel());
    let _ = run.host.pull(2 * DEVICE_FRAME);
    block_on(node.stop());
    assert_eq!(run.receipts.len(), 0, "a cancelled sink sends nothing more");
    assert!(handle.receipts().is_empty());
    assert!(!run.host.output_open());
    assert_eq!(
        run.synthesis.state(segment),
        Some(SegmentState::Synthesized)
    );
}

/// A started sink given a turns port, and the edge into that port.
fn started_with_turns() -> (Started, Edge) {
    let nodes = declared();
    let host = FakeHost::laptop();
    let speaker = Speaker::open(&host, &DeviceSelector::Default, PlayoutConfig::default())
        .expect("the default output opens");
    let mut sink = AcknowledgingSink::new(RunId::generate(), speaker).with_turns(nodes.turns);
    let (speech, receipts, turns) = (Edge::default(), Edge::default(), Edge::default());
    let node: &mut dyn NodeImplementation<Payload> = &mut sink;
    block_on(node.describe(&nodes.sink));
    block_on(node.prepare()).expect("the sink prepares");
    block_on(node.start(NodeEndpoints {
        inputs: vec![
            NodeInput {
                port_id: nodes.turns,
                endpoint: Box::new(turns.clone()),
            },
            NodeInput {
                port_id: nodes.speech,
                endpoint: Box::new(speech.clone()),
            },
        ],
        outputs: vec![NodeOutput {
            port_id: nodes.played,
            endpoint: Box::new(receipts.clone()),
        }],
    }))
    .expect("the sink starts");
    let synthesis = Synthesis::new();
    let run = Started {
        host,
        sink,
        feed: Feed {
            edge: speech,
            stream_id: StreamId::generate(),
            sequence: 0,
            generation_id: synthesis.generation_id(),
        },
        receipts,
        synthesis,
    };
    (run, turns)
}

/// Sends the opening of `turn_id` on `turns`.
fn open_turn(turns: &Edge, turn_id: TurnId) {
    turns.put(EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id: RunId::generate(),
        session_id: None,
        stream_id: StreamId::generate(),
        sequence: 1,
        producer: NodeId::generate(),
        port: "turns".to_owned(),
        kind: "inference.stream.TurnOpened".to_owned(),
        generation_id: None,
        timestamp: 0,
        timebase: Timebase::WallTime,
        causation_id: None,
        payload: Payload::TurnOpened(turn_id),
    });
}

#[test]
fn a_turn_opened_while_audio_waits_stops_the_speaker_and_drops_the_older_generation() {
    let (mut run, turns) = started_with_turns();
    let handle = run.sink.handle();
    let segment = run.synthesis.segment("one two three four");
    for _ in 0..3 {
        let frame = run.synthesis.frame(segment);
        run.feed.send(frame);
    }
    assert!(eventually(|| handle.pushed() == 3));
    let _ = run.host.pull(DEVICE_FRAME);

    let turn_id = TurnId::generate();
    open_turn(&turns, turn_id);
    assert!(
        eventually(|| handle.interruptions().len() == 1),
        "the speaker is stopped at once"
    );
    let interruption = handle.interruptions()[0];
    assert_eq!(interruption.turn_id, turn_id);
    assert!(interruption.cleared > 0, "unplayed samples were cleared");
    assert!(
        interruption.position.is_some(),
        "the played position was read"
    );
    assert_eq!(
        interruption.played_after, None,
        "nothing is queued after the stop"
    );

    let late = run.synthesis.frame(segment);
    run.feed.send(late);
    let completed = run.synthesis.complete(segment);
    run.feed.send(completed);
    assert!(eventually(|| handle.stale_frames().len() == 1));
    let stale = handle.stale_frames()[0];
    assert_eq!(stale.segment_id, segment);
    assert_eq!(stale.generation_id, Some(run.synthesis.generation_id()));
    assert_eq!(
        stale.played, interruption.played_after,
        "the played position is unchanged"
    );
    assert_eq!(
        handle.pushed(),
        3,
        "the stale frame never reached the speaker"
    );
    let _ = run.host.pull(8 * DEVICE_FRAME);
    assert_eq!(
        settled(&run.receipts),
        0,
        "no receipt is made of the older generation"
    );

    let mut next = Synthesis::new();
    run.feed.generation_id = next.generation_id();
    let answer = next.segment("hello");
    let frame = next.frame(answer);
    run.feed.send(frame);
    let completed = next.complete(answer);
    run.feed.send(completed);
    assert!(played_until(&run, 1), "the next generation plays");
    let (named, receipt) = receipt(run.receipts.taken().remove(0));
    assert_eq!((named, receipt.frames()), (answer, 1));
    let node: &mut dyn NodeImplementation<Payload> = &mut run.sink;
    block_on(node.cancel());
    block_on(node.stop());
}

#[test]
fn a_turn_opened_with_nothing_to_play_stops_nothing() {
    let (mut run, turns) = started_with_turns();
    let handle = run.sink.handle();
    open_turn(&turns, TurnId::generate());
    let segment = run.synthesis.segment("first answer");
    let frame = run.synthesis.frame(segment);
    run.feed.send(frame);
    let completed = run.synthesis.complete(segment);
    run.feed.send(completed);
    assert!(
        played_until(&run, 1),
        "a generation first seen after the turn opened plays"
    );
    assert_eq!(handle.interruptions(), []);
    assert_eq!(handle.stale_frames(), []);
    let node: &mut dyn NodeImplementation<Payload> = &mut run.sink;
    block_on(node.cancel());
    block_on(node.stop());
}

#[test]
fn a_superseded_generation_is_stopped_and_its_later_frames_dropped() {
    let (mut run, _turns) = started_with_turns();
    let handle = run.sink.handle();
    let segment = run.synthesis.segment("cut off here");
    for _ in 0..2 {
        let frame = run.synthesis.frame(segment);
        run.feed.send(frame);
    }
    assert!(eventually(|| handle.pushed() == 2));
    run.feed
        .send(Payload::Superseded(run.synthesis.generation_id()));
    let late = run.synthesis.frame(segment);
    run.feed.send(late);
    assert!(eventually(|| handle.stale_frames().len() == 1));
    assert_eq!(handle.pushed(), 2);
    let _ = run.host.pull(8 * DEVICE_FRAME);
    assert_eq!(settled(&run.receipts), 0);
    assert_eq!(
        handle.interruptions(),
        [],
        "an interruption is a turn's opening"
    );
    let node: &mut dyn NodeImplementation<Payload> = &mut run.sink;
    block_on(node.cancel());
    block_on(node.stop());
}

#[test]
fn a_generation_started_before_a_turn_opened_has_its_first_frames_dropped_and_its_supersede_records_the_interruption()
 {
    let (mut run, turns) = started_with_turns();
    let handle = run.sink.handle();
    let old = run.synthesis.generation_id();
    open_turn(&turns, TurnId::generate());
    run.feed.send(Payload::Started(old, 1));

    // The user speaks again before any frame of the first turn's answer reached the sink.
    let second = TurnId::generate();
    open_turn(&turns, second);
    let segment = run.synthesis.segment("rendered too late");
    for _ in 0..2 {
        let frame = run.synthesis.frame(segment);
        run.feed.send(frame);
    }
    let completed = run.synthesis.complete(segment);
    run.feed.send(completed);
    assert!(
        eventually(|| handle.stale_frames().len() == 2),
        "every frame of the generation that began before the turn is dropped: {:?}",
        handle.stale_frames()
    );
    assert_eq!(handle.pushed(), 0, "no frame of it reached the speaker");
    assert_eq!(
        handle.interruptions(),
        [],
        "the interruption waits for the generation's supersede"
    );

    run.feed.send(Payload::Superseded(old));
    assert!(eventually(|| handle.interruptions().len() == 1));
    let interruption = handle.interruptions()[0];
    assert_eq!(interruption.turn_id, second);
    assert_eq!(interruption.cleared, 0, "nothing of it was queued");
    let _ = run.host.pull(8 * DEVICE_FRAME);
    assert_eq!(settled(&run.receipts), 0, "no receipt is made of it");

    // The second turn's answer, epoch 2, plays.
    let mut next = Synthesis::new();
    run.feed.generation_id = next.generation_id();
    run.feed.send(Payload::Started(next.generation_id(), 2));
    let answer = next.segment("hello again");
    let frame = next.frame(answer);
    run.feed.send(frame);
    let completed = next.complete(answer);
    run.feed.send(completed);
    assert!(played_until(&run, 1), "the second turn's answer plays");
    let (named, receipt) = receipt(run.receipts.taken().remove(0));
    assert_eq!((named, receipt.frames()), (answer, 1));
    assert_eq!(handle.interruptions().len(), 1);
    let node: &mut dyn NodeImplementation<Payload> = &mut run.sink;
    block_on(node.cancel());
    block_on(node.stop());
}

#[test]
fn a_generation_of_the_turn_before_that_is_never_superseded_makes_no_interruption() {
    let (mut run, turns) = started_with_turns();
    let handle = run.sink.handle();
    open_turn(&turns, TurnId::generate());
    run.feed
        .send(Payload::Started(run.synthesis.generation_id(), 1));
    let segment = run.synthesis.segment("all of it played");
    let frame = run.synthesis.frame(segment);
    run.feed.send(frame);
    let completed = run.synthesis.complete(segment);
    run.feed.send(completed);
    assert!(played_until(&run, 1), "the first answer plays to the end");

    open_turn(&turns, TurnId::generate());
    let mut next = Synthesis::new();
    next.next_sample = run.synthesis.next_sample;
    run.feed.generation_id = next.generation_id();
    run.feed.send(Payload::Started(next.generation_id(), 2));
    let answer = next.segment("the next answer");
    let frame = next.frame(answer);
    run.feed.send(frame);
    let completed = next.complete(answer);
    run.feed.send(completed);
    assert!(played_until(&run, 2), "the second answer plays");
    assert_eq!(handle.interruptions(), []);
    assert_eq!(handle.stale_frames(), []);
    let node: &mut dyn NodeImplementation<Payload> = &mut run.sink;
    block_on(node.cancel());
    block_on(node.stop());
}
