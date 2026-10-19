//! The devices as pipeline nodes, against the fake device and in-test edges: the microphone
//! source sends its frames in envelopes and the end of input on a drain, drops frames a full edge
//! refuses, and the speaker sink plays what it receives, stops on a `cancel` control event and
//! fails on content that is no audio (`story:audio-device-adapter`, Planned scope: pipeline
//! nodes).

use std::collections::VecDeque;
use std::future::{self, Future};
use std::num::{NonZeroU16, NonZeroU32, NonZeroUsize};
use std::pin::pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_audio_device::convert::encode;
use inference_audio_device::fake::FakeHost;
use inference_audio_device::node::{AUDIO_FRAME, DevicePayload, MicrophoneSource, SpeakerSink};
use inference_audio_device::{CaptureConfig, DeviceSelector, Microphone, PlayoutConfig, Speaker};
use inference_core::endpoint::{EndpointFuture, InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, InteractionMode, Node, NodeEndpoints, NodeImplementation, NodeInput,
    NodeOutput, NodeRole, OverloadPolicy, PipelineSpec, PortId, PortTraits, modality,
};

const PATIENCE: Duration = Duration::from_secs(5);

/// What the edges of the test runs carry.
#[derive(Debug, Clone, PartialEq)]
enum Payload {
    Audio(AudioFrame),
    Control(ControlKind),
    Text(String),
}

impl DevicePayload for Payload {
    fn from_audio(frame: AudioFrame) -> Self {
        Self::Audio(frame)
    }

    fn into_audio(self) -> Result<AudioFrame, Self> {
        match self {
            Self::Audio(frame) => Ok(frame),
            other => Err(other),
        }
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

/// An edge of the test: a queue the test reads and writes, refusing a send beyond `capacity`
/// with [`SendError::Full`] as the overload policy `fail` does.
#[derive(Debug, Default)]
struct Queue {
    envelopes: VecDeque<EventEnvelope<Payload>>,
    closed: bool,
    waker: Option<Waker>,
}

#[derive(Debug, Clone)]
struct Edge {
    queue: Arc<Mutex<Queue>>,
    capacity: usize,
}

impl Edge {
    fn new(capacity: usize) -> Self {
        Self {
            queue: Arc::default(),
            capacity,
        }
    }

    fn with<T>(&self, change: impl FnOnce(&mut Queue) -> T) -> T {
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
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
        let sent = if self.len() >= self.capacity {
            Err(SendError::Full(envelope))
        } else {
            self.put(envelope);
            Ok(())
        };
        Box::pin(future::ready(sent))
    }
}

impl InputEndpoint<Payload> for Edge {
    fn recv(&mut self) -> EndpointFuture<'_, Option<EventEnvelope<Payload>>> {
        let queue = Arc::clone(&self.queue);
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

/// The declared microphone and speaker nodes of a pipeline connecting them, and their ports.
struct Declared {
    microphone: Node,
    speaker: Node,
    out: PortId,
    into: PortId,
}

fn declared() -> Declared {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    let catalog = Profiles(profile);
    let spec = PipelineSpec {
        name: "mic to speaker".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::Duplex,
        required_capabilities: Vec::new(),
    };
    let mut declaration = Declaration::new();
    let (_, (microphone, speaker, out, into)) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let microphone = pipeline.node("microphone", NodeRole::Source, Vec::new());
            let speaker = pipeline.node("speaker", NodeRole::Sink, Vec::new());
            let out =
                pipeline.output::<modality::Audio>(&microphone, "audio", PortTraits::default());
            let into = pipeline.input::<modality::Audio>(&speaker, "audio", PortTraits::default());
            pipeline.connect(&out, &into, NonZeroUsize::MIN, OverloadPolicy::Fail);
            (microphone.id(), speaker.id(), out.id(), into.id())
        })
        .expect("the profile is declared");
    let node = |node_id: NodeId| declaration.node(node_id).expect("a declared node").clone();
    Declared {
        microphone: node(microphone),
        speaker: node(speaker),
        out,
        into,
    }
}

fn output(port_id: PortId, edge: &Edge) -> NodeEndpoints<Payload> {
    NodeEndpoints {
        inputs: Vec::new(),
        outputs: vec![NodeOutput {
            port_id,
            endpoint: Box::new(edge.clone()),
        }],
    }
}

fn input(port_id: PortId, edge: &Edge) -> NodeEndpoints<Payload> {
    NodeEndpoints {
        inputs: vec![NodeInput {
            port_id,
            endpoint: Box::new(edge.clone()),
        }],
        outputs: Vec::new(),
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

fn stereo(frames: usize, level: f32) -> Vec<f32> {
    vec![level; frames * 2]
}

fn audio_frame(at: u64, level: f32) -> AudioFrame {
    AudioFrame {
        format: AudioFormat {
            encoding: AudioEncoding::PcmS16le,
            sample_rate: NonZeroU32::new(16_000).expect("a rate"),
            channels: NonZeroU16::MIN,
        },
        position: SampleClockPosition::new(at).expect("a position"),
        sample_count: 320,
        data: encode(&[level; 320], AudioEncoding::PcmS16le),
    }
}

/// An envelope of the test feeding the sink: `kind` and `payload`, numbered `sequence`.
fn envelope(
    producer: NodeId,
    sequence: i64,
    kind: &str,
    payload: Payload,
) -> EventEnvelope<Payload> {
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id: RunId::generate(),
        session_id: None,
        stream_id: StreamId::generate(),
        sequence,
        producer,
        port: "audio".to_owned(),
        kind: kind.to_owned(),
        generation_id: None,
        timestamp: 0,
        timebase: Timebase::AudioFrameTime,
        causation_id: None,
        payload,
    }
}

fn started_source(host: &FakeHost, edge: &Edge) -> (MicrophoneSource, Declared, RunId) {
    let nodes = declared();
    let run_id = RunId::generate();
    let microphone = Microphone::open(host, &DeviceSelector::Default, CaptureConfig::default())
        .expect("the default input opens");
    let mut source = MicrophoneSource::new(run_id, microphone);
    let node: &mut dyn NodeImplementation<Payload> = &mut source;
    block_on(node.describe(&nodes.microphone));
    block_on(node.prepare()).expect("the source prepares");
    block_on(node.start(output(nodes.out, edge))).expect("the source starts");
    (source, nodes, run_id)
}

fn started_sink(speaker: &Arc<Speaker>, edge: &Edge) -> (SpeakerSink, Declared) {
    let nodes = declared();
    let mut sink = SpeakerSink::new(Arc::clone(speaker));
    let node: &mut dyn NodeImplementation<Payload> = &mut sink;
    block_on(node.describe(&nodes.speaker));
    block_on(node.prepare()).expect("the sink prepares");
    block_on(node.start(input(nodes.into, edge))).expect("the sink starts");
    (sink, nodes)
}

#[test]
fn the_source_sends_each_frame_in_an_envelope_on_its_sample_clock_and_stop_on_a_drain() {
    let host = FakeHost::laptop();
    let edge = Edge::new(usize::MAX);
    let (mut source, nodes, run_id) = started_source(&host, &edge);
    let handle = source.handle();
    assert!(host.feed(&stereo(4800, 0.5)));
    assert!(eventually(|| edge.len() == 5), "five 20 ms frames arrive");
    let node: &mut dyn NodeImplementation<Payload> = &mut source;
    block_on(node.drain()).expect("the source drains");
    block_on(node.stop());
    let envelopes = edge.taken();
    assert_eq!(envelopes.len(), 6, "five frames and the end of input");
    let stream = envelopes[0].stream_id;
    for (index, envelope) in envelopes.iter().enumerate() {
        assert_eq!(envelope.run_id, run_id);
        assert_eq!(envelope.producer, nodes.microphone.node_id());
        assert_eq!(envelope.port, nodes.out.to_string());
        assert_eq!(envelope.stream_id, stream);
        assert_eq!(envelope.sequence, index as i64 + 1);
        assert_eq!(envelope.timebase, Timebase::AudioFrameTime);
    }
    for (index, envelope) in envelopes[..5].iter().enumerate() {
        assert_eq!(envelope.kind, AUDIO_FRAME);
        assert_eq!(envelope.timestamp, index as i64 * 320);
        let Payload::Audio(frame) = &envelope.payload else {
            panic!("an audio frame, not {:?}", envelope.payload);
        };
        assert_eq!(frame.position.get(), index as u64 * 320);
        assert_eq!(frame.sample_count, 320);
    }
    assert_eq!(envelopes[5].kind, ControlKind::Stop.as_str());
    assert_eq!(envelopes[5].payload, Payload::Control(ControlKind::Stop));
    assert_eq!(
        envelopes[5].timestamp, 1600,
        "the end of input is just past the last frame"
    );
    assert_eq!(handle.sent(), 5);
    assert_eq!(handle.dropped(), 0);
}

#[test]
fn the_source_drops_and_counts_frames_a_full_edge_refuses_and_sends_nothing_once_cancelled() {
    let host = FakeHost::laptop();
    let edge = Edge::new(2);
    let (mut source, _, _) = started_source(&host, &edge);
    let handle = source.handle();
    assert!(host.feed(&stereo(4800, 0.5)));
    assert!(
        eventually(|| handle.sent() + handle.dropped() == 5),
        "every frame was sent or dropped"
    );
    assert_eq!((handle.sent(), handle.dropped()), (2, 3));
    let node: &mut dyn NodeImplementation<Payload> = &mut source;
    block_on(node.cancel());
    block_on(node.stop());
    assert!(!host.input_open(), "a stopped source closes its device");
    let envelopes = edge.taken();
    assert_eq!(envelopes.len(), 2, "no end of input once cancelled");
    assert!(
        envelopes
            .iter()
            .all(|envelope| envelope.kind == AUDIO_FRAME)
    );
}

#[test]
fn a_source_handed_no_output_edge_does_not_start() {
    let host = FakeHost::laptop();
    let nodes = declared();
    let microphone = Microphone::open(&host, &DeviceSelector::Default, CaptureConfig::default())
        .expect("the default input opens");
    let mut source = MicrophoneSource::new(RunId::generate(), microphone);
    let node: &mut dyn NodeImplementation<Payload> = &mut source;
    block_on(node.describe(&nodes.microphone));
    assert!(block_on(node.start(NodeEndpoints::default())).is_err());
}

#[test]
fn the_sink_plays_the_frames_it_receives_and_a_cancel_event_stops_the_speaker_at_once() {
    let host = FakeHost::laptop();
    let speaker = Arc::new(
        Speaker::open(&host, &DeviceSelector::Default, PlayoutConfig::default())
            .expect("the default output opens"),
    );
    let edge = Edge::new(usize::MAX);
    let (mut sink, nodes) = started_sink(&speaker, &edge);
    let handle = sink.handle();
    let producer = nodes.microphone.node_id();
    for index in 0..3 {
        edge.put(envelope(
            producer,
            index + 1,
            AUDIO_FRAME,
            Payload::Audio(audio_frame(index as u64 * 320, 0.5)),
        ));
    }
    assert!(eventually(|| handle.pushed() == 3), "three frames pushed");
    let played = host.pull(480).expect("the output runs");
    assert!(played.iter().all(|&sample| sample > 0.49));
    assert_eq!(speaker.played(), SampleClockPosition::new(160));
    edge.put(envelope(
        producer,
        4,
        ControlKind::Cancel.as_str(),
        Payload::Control(ControlKind::Cancel),
    ));
    assert!(
        eventually(|| handle.stopped_at().is_some()),
        "the cancel arrives"
    );
    assert_eq!(handle.stopped_at(), SampleClockPosition::new(160));
    assert_eq!(speaker.queued(), 0);
    assert!(
        host.pull(480)
            .expect("the output runs")
            .iter()
            .all(|&sample| sample == 0.0)
    );
    edge.close();
    let node: &mut dyn NodeImplementation<Payload> = &mut sink;
    block_on(node.drain()).expect("the sink drains once its edge is closed");
    block_on(node.stop());
}

#[test]
fn the_end_of_input_finishes_the_audio_so_running_dry_after_it_is_no_underrun() {
    let host = FakeHost::laptop();
    let speaker = Arc::new(
        Speaker::open(&host, &DeviceSelector::Default, PlayoutConfig::default())
            .expect("the default output opens"),
    );
    let edge = Edge::new(usize::MAX);
    let (mut sink, nodes) = started_sink(&speaker, &edge);
    let handle = sink.handle();
    let producer = nodes.microphone.node_id();
    edge.put(envelope(
        producer,
        1,
        AUDIO_FRAME,
        Payload::Audio(audio_frame(0, 0.5)),
    ));
    edge.put(envelope(
        producer,
        2,
        ControlKind::Stop.as_str(),
        Payload::Control(ControlKind::Stop),
    ));
    assert!(eventually(|| handle.pushed() == 1));
    assert!(eventually(|| {
        let _ = host.pull(960);
        speaker.queued() == 0
    }));
    let _ = host.pull(960);
    assert_eq!(speaker.stats().underruns, 0);
    assert_eq!(speaker.played(), SampleClockPosition::new(320));
    edge.close();
    let node: &mut dyn NodeImplementation<Payload> = &mut sink;
    block_on(node.drain()).expect("the sink drains");
    block_on(node.stop());
}

#[test]
fn the_sink_fails_on_content_that_is_no_audio_frame() {
    let host = FakeHost::laptop();
    let speaker = Arc::new(
        Speaker::open(&host, &DeviceSelector::Default, PlayoutConfig::default())
            .expect("the default output opens"),
    );
    let edge = Edge::new(usize::MAX);
    let (mut sink, nodes) = started_sink(&speaker, &edge);
    edge.put(envelope(
        nodes.microphone.node_id(),
        1,
        "inference.stream.TextDelta",
        Payload::Text("hello".to_owned()),
    ));
    let node: &mut dyn NodeImplementation<Payload> = &mut sink;
    let drained = block_on(node.drain());
    let error = drained.expect_err("text is no audio");
    assert!(
        error.detail().contains("inference.stream.TextDelta"),
        "{}",
        error.detail()
    );
    block_on(node.stop());
}

#[test]
fn cancelling_the_sink_stops_the_speaker_and_ends_its_exchange() {
    let host = FakeHost::laptop();
    let speaker = Arc::new(
        Speaker::open(&host, &DeviceSelector::Default, PlayoutConfig::default())
            .expect("the default output opens"),
    );
    let edge = Edge::new(usize::MAX);
    let (mut sink, nodes) = started_sink(&speaker, &edge);
    let handle = sink.handle();
    edge.put(envelope(
        nodes.microphone.node_id(),
        1,
        AUDIO_FRAME,
        Payload::Audio(audio_frame(0, 0.5)),
    ));
    assert!(eventually(|| handle.pushed() == 1));
    let node: &mut dyn NodeImplementation<Payload> = &mut sink;
    block_on(node.cancel());
    block_on(node.stop());
    assert_eq!(
        speaker.queued(),
        0,
        "a cancelled sink leaves nothing to play"
    );
}
