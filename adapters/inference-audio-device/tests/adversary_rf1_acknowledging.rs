//! Adversary pass 1 against `story:acknowledging-sink-drain-ends`: a speaker that stops while a
//! frame waits for room in a full queue, the end of input queued behind it; the receipt of a
//! segment heard in full, kept at a turn's opening, holding back the receipts of the next answer
//! when its completion never comes; and a heard segment on the old clock, kept, taken for a clock
//! regression by the first frame of the next answer on a new clock.

#![cfg(feature = "pipeline")]

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
    ACKNOWLEDGE_PLAYBACK, AcknowledgingSink, Playable, PlaybackFault, PlaybackPayload,
};
use inference_audio_device::convert::encode;
use inference_audio_device::fake::FakeHost;
use inference_audio_device::{AudioHost, DeviceSelector, PlayoutConfig, Speaker};
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
use inference_tts::inference_text::{OutputSegment, SegmentId, TurnId};
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
    Started(GenerationId, i64),
    Control(ControlKind),
}

impl PlaybackPayload for Payload {
    fn playable(self) -> Playable {
        match self {
            Self::Audio(audio) => Playable::Audio(audio),
            Self::Completed(segment_id) => Playable::Completed(segment_id),
            Self::TurnOpened(turn_id) => Playable::TurnOpened(turn_id),
            Self::Started(generation_id, epoch) => Playable::Started {
                generation_id,
                epoch,
            },
            Self::Receipt(..) | Self::Control(_) => Playable::Other,
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

/// Polls `future` for at most `limit`: its output, or `None` when it was still pending.
fn block_on_within<F: Future>(future: F, limit: Duration) -> Option<F::Output> {
    let deadline = Instant::now() + limit;
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
            return Some(output);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        thread::park_timeout(left.min(Duration::from_millis(10)));
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
            Payload::Started(..) => "inference.stream.GenerationStarted".to_owned(),
            _ => "inference.audio.AudioChunk".to_owned(),
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

/// A started sink waiting `patience` for a stalled speaker, its speaker opened on `opener`,
/// whose output stream `host` drives.
fn started_on(opener: &dyn AudioHost, host: FakeHost, patience: Duration) -> Started {
    let nodes = declared();
    let speaker = Speaker::open(opener, &DeviceSelector::Default, PlayoutConfig::default())
        .expect("the default output opens");
    let mut sink =
        AcknowledgingSink::new(RunId::generate(), speaker).with_playback_patience(patience);
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

/// The default playout queue of two seconds, in frames of the test: the frame after them waits
/// for room.
const QUEUE_FRAMES: u64 = 100;

#[test]
fn a_speaker_that_stops_while_a_frame_waits_for_room_fails_the_drain_within_the_patience() {
    let patience = Duration::from_millis(300);
    let host = FakeHost::laptop();
    let mut run = started_on(&host.clone(), host, patience);
    let handle = run.sink.handle();
    let segment = run
        .synthesis
        .segment("an answer longer than the playout queue");
    for _ in 0..=QUEUE_FRAMES {
        let frame = run.synthesis.frame(segment);
        run.feed.send(frame);
    }
    let completed = run.synthesis.complete(segment);
    run.feed.send(completed);
    run.feed.send(Payload::Control(ControlKind::Stop));
    assert!(
        eventually(|| handle.pushed() == QUEUE_FRAMES),
        "the queue fills: {} pushed",
        handle.pushed()
    );

    // The device calls back no more: the frame after the full queue never finds room, and the
    // end of input waits behind it on the edge.
    let node: &mut dyn NodeImplementation<Payload> = &mut run.sink;
    let drained = block_on_within(node.drain(), PATIENCE);
    let fault = handle.fault();
    block_on(node.cancel());
    block_on(node.stop());
    let failure = drained
        .expect(
            "story:acknowledging-sink-drain-ends acceptance: a speaker whose callback stops makes \
             drain() fail within the stated patience; with a frame waiting for room the sink \
             never reads the end of input, so the watch never starts and the drain never ends",
        )
        .expect_err("a drain the device never finished fails");
    assert!(
        matches!(fault, Some(PlaybackFault::Stalled { .. })),
        "the fault is a stall: {fault:?}, {failure:?}"
    );
}

#[test]
fn a_heard_segment_whose_completion_never_comes_does_not_hold_back_the_next_answer() {
    let (mut run, turns) = started_with_turns();
    let handle = run.sink.handle();
    let heard = run.synthesis.segment("heard in full, never completed");
    for _ in 0..2 {
        let frame = run.synthesis.frame(heard);
        run.feed.send(frame);
    }
    assert!(eventually(|| handle.pushed() == 2));
    let _ = run.host.pull(4 * DEVICE_FRAME);
    open_turn(&turns, TurnId::generate());
    assert!(eventually(|| turns.len() == 0), "the sink took the turn");

    // The next answer, on its own clock; the cut generation is neither completed nor superseded.
    let mut next = Synthesis::new();
    run.feed.generation_id = next.generation_id();
    run.feed.send(Payload::Started(next.generation_id(), 1));
    let answer = next.segment("the next answer");
    let frame = next.frame(answer);
    run.feed.send(frame);
    let completed = next.complete(answer);
    run.feed.send(completed);
    assert!(
        eventually(|| handle.pushed() == 3),
        "the next answer is pushed"
    );
    let acknowledged = played_until(&run, 1);
    let fault = handle.fault();
    let node: &mut dyn NodeImplementation<Payload> = &mut run.sink;
    block_on(node.cancel());
    block_on(node.stop());
    assert_eq!(fault, None);
    assert!(
        acknowledged,
        "story:acknowledging-sink-drain-ends N30: the next answer, played in full, is acknowledged; \
         it waits behind the kept segment of the cut generation, whose completion never comes"
    );
    let (named, _) = receipt(run.receipts.taken().remove(0));
    assert_eq!(named, answer);
}

#[test]
fn a_heard_segment_kept_at_a_turn_is_no_clock_regression_for_the_next_answer_on_a_new_clock() {
    let (mut run, turns) = started_with_turns();
    let handle = run.sink.handle();
    let heard = run.synthesis.segment("heard in full before the turn");
    for _ in 0..2 {
        let frame = run.synthesis.frame(heard);
        run.feed.send(frame);
    }
    assert!(eventually(|| handle.pushed() == 2));
    let _ = run.host.pull(4 * DEVICE_FRAME);
    open_turn(&turns, TurnId::generate());
    assert!(eventually(|| turns.len() == 0), "the sink took the turn");

    let mut next = Synthesis::new();
    let old = run.feed.generation_id;
    run.feed.generation_id = next.generation_id();
    run.feed.send(Payload::Started(next.generation_id(), 1));
    let answer = next.segment("the next answer, from sample 0");
    let frame = next.frame(answer);
    run.feed.send(frame);
    assert!(
        eventually(|| handle.pushed() == 3 || handle.fault().is_some()),
        "the frame is pushed or refused"
    );
    assert_eq!(
        handle.fault(),
        None,
        "the heard segment ends at sample {} of a clock the speaker left; the next answer starts \
         its own clock at 0",
        2 * FRAME
    );
    run.feed.generation_id = old;
    let completed = run.synthesis.complete(heard);
    run.feed.send(completed);
    run.feed.generation_id = next.generation_id();
    let completed = next.complete(answer);
    run.feed.send(completed);
    assert!(played_until(&run, 2), "both are acknowledged");
    let named = run
        .receipts
        .taken()
        .into_iter()
        .map(|envelope| receipt(envelope).0)
        .collect::<Vec<_>>();
    assert_eq!(named, [heard, answer]);
    let node: &mut dyn NodeImplementation<Payload> = &mut run.sink;
    block_on(node.cancel());
    block_on(node.stop());
}
