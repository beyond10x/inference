//! Adversary pass on `story:long-talk-over-does-not-resume-the-cut-answer`, the cascade path:
//! the order in which the acknowledging sink takes a talk-over's two signals.
//!
//! In the cascade the turn policy sends the talk-over's `TurnOpened` to the sink on the sink's
//! turns edge, and to the model node, which supersedes the cut generation at the same opening;
//! that `GenerationSuperseded` reaches the sink on its audio edge, through the gate and the TTS
//! node. The two edges are not ordered with each other. `talk_over.rs` waits for one
//! interruption of the sink at the talk-over's opening. This case gives the sink the supersede
//! first, as the audio edge may deliver it, then the opening, on the sink alone, with the
//! in-test edges of `adapters/inference-audio-device/tests/acknowledging.rs`.

use std::collections::VecDeque;
use std::future::{self, Future};
use std::num::{NonZeroU16, NonZeroU32, NonZeroUsize};
use std::pin::pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_audio_device::acknowledging::{AcknowledgingSink, Playable, PlaybackPayload};
use inference_audio_device::convert::encode;
use inference_audio_device::fake::FakeHost;
use inference_audio_device::{DeviceSelector, PlayoutConfig, Speaker};
use inference_core::endpoint::{EndpointFuture, InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::generation::{Generation, StartGeneration};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{
    Adapter, AdapterDescriptor, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality,
    ModelId,
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

#[derive(Debug)]
enum Payload {
    Audio(inference_tts::SegmentAudio),
    Receipt,
    TurnOpened(TurnId),
    Superseded(GenerationId),
    Control,
}

impl PlaybackPayload for Payload {
    fn playable(self) -> Playable {
        match self {
            Self::Audio(audio) => Playable::Audio(audio),
            Self::TurnOpened(turn_id) => Playable::TurnOpened(turn_id),
            Self::Superseded(generation_id) => Playable::Superseded(generation_id),
            Self::Receipt | Self::Control => Playable::Other,
        }
    }

    fn receipt(segment_id: SegmentId, receipt: PlaybackReceipt) -> Self {
        let _ = (segment_id, receipt);
        Self::Receipt
    }

    fn control(_kind: ControlKind) -> Self {
        Self::Control
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
    waker: Option<Waker>,
}

/// An edge of the test: a queue the test writes and the sink reads, unbounded.
#[derive(Debug, Clone, Default)]
struct Edge(Arc<Mutex<Queue>>);

impl Edge {
    fn put(&self, envelope: EventEnvelope<Payload>) {
        let mut queue = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        queue.envelopes.push_back(envelope);
        if let Some(waker) = queue.waker.take() {
            waker.wake();
        }
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

/// A synthesis claiming incremental input and output streaming, to make the frames of the cut
/// answer.
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
            backend: "sink order test".to_owned(),
            contract_revision: CONTRACT_REVISION.to_owned(),
            execution_form: ExecutionForm::InProcess,
            locality: Locality::SameProcess,
            capabilities: vec![Capability::IncrementalInput, Capability::OutputStreaming],
            max_concurrency: 1,
            descriptor: AdapterDescriptor::default(),
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
}

/// The TTS node's stream feeding the sink's audio edge.
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
            Payload::Superseded(_) => "inference.stream.GenerationSuperseded".to_owned(),
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

/// The cut answer is playing, the rest of it queued; the supersede of its generation reaches
/// the sink before the talk-over's `TurnOpened`. The sink stops the speaker at the supersede,
/// and at the opening finds nothing left to play, so it parks the turn's interruption for a
/// supersede that already came: the talk-over is never recorded as an interruption, and
/// `talk_over.rs`, which waits for one, waits out its 60 s.
#[test]
fn a_supersede_taken_before_the_talk_overs_opening_still_records_its_interruption() {
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
            endpoint: Box::new(receipts),
        }],
    }))
    .expect("the sink starts");
    let handle = sink.handle();
    let mut synthesis = Synthesis::new();
    let mut feed = Feed {
        edge: speech,
        stream_id: StreamId::generate(),
        sequence: 0,
        generation_id: synthesis.generation_id(),
    };

    let segment = synthesis.segment("the answer the caller talks over");
    for _ in 0..3 {
        let frame = synthesis.frame(segment);
        feed.send(frame);
    }
    assert!(eventually(|| handle.pushed() == 3), "the answer is queued");
    let _ = host.pull(DEVICE_FRAME);

    feed.send(Payload::Superseded(synthesis.generation_id()));
    let late = synthesis.frame(segment);
    feed.send(late);
    assert!(
        eventually(|| handle.stale_frames().len() == 1),
        "the sink took the supersede: {:?}",
        handle.stale_frames()
    );

    let turn_id = TurnId::generate();
    open_turn(&turns, turn_id);
    let recorded = eventually(|| handle.interruptions().len() == 1);
    let node: &mut dyn NodeImplementation<Payload> = &mut sink;
    block_on(node.cancel());
    block_on(node.stop());
    assert!(
        recorded,
        "the talk-over's opening, taken after the supersede of the generation it cut, records \
         no interruption: {:?}",
        handle.interruptions()
    );
    assert_eq!(handle.interruptions()[0].turn_id, turn_id);
}
