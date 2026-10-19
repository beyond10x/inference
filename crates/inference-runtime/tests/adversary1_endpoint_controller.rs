//! Adversary pass 1 of `story:endpoint-controller`: the voice activity detector and the turn
//! policy driven through the run loop on inputs the unit's own cases do not build.
//!
//! The harness mirrors `tests/endpoint_controller.rs`: source, detector, turn policy and sink,
//! the turn commands answered by the testkit's in-memory store of `inference.stream`.

use std::collections::HashMap;
use std::future::Future;
use std::num::{NonZeroU16, NonZeroU32, NonZeroUsize};
use std::pin::pin;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use inference_core::envelope::{ControlKind, EventEnvelope, Timebase};
use inference_core::id::{GenerationId, RunId};
use inference_core::model::{Adapter, AdapterId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::nodes::turn_policy::{
    CompleteTurnRefused, TurnCommands, TurnEvent, TurnPayload, TurnPolicy,
};
use inference_pipeline::nodes::vad::{
    AudioPayload, SpeechActivity, VadConfig, VoiceActivityDetector,
};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeImplementation,
    NodeRole, OverloadPolicy, PipelineSpec, PlannedPipeline, PortTraits, modality,
};
use inference_runtime::run::RunState;
use inference_runtime::{NodeId, RunController, RunError, RunRequests};
use inference_testkit::graph::{SinkHandle, SourceHandle, TextSink, TextSource};
use inference_testkit::stream::{StreamDomain, TurnMoveError};
use inference_testkit::stt::FRAME_SAMPLES;
use inference_testkit::stt::inference_stt::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_testkit::stt::synthetic_audio::{Stretch, synthesize, tone_silence_tone_silence};
use inference_text::{OpenTurn, TurnCompleted, TurnId, TurnOpened, TurnState};

const PATIENCE: Duration = Duration::from_secs(10);

/// The configuration of `tests/endpoint_controller.rs`.
const CONFIG: VadConfig = VadConfig {
    threshold: 0.05,
    attack: Duration::from_millis(30),
    hold: Duration::from_millis(200),
};

#[derive(Debug, Clone, PartialEq)]
enum Payload {
    Audio(AudioFrame),
    Activity(SpeechActivity),
    Turn(TurnEvent),
    Control(ControlKind),
}

impl From<ControlKind> for Payload {
    fn from(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

impl AudioPayload for Payload {
    fn into_audio(self) -> Result<AudioFrame, Self> {
        match self {
            Self::Audio(frame) => Ok(frame),
            other => Err(other),
        }
    }

    fn from_activity(activity: SpeechActivity) -> Self {
        Self::Activity(activity)
    }

    fn control(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

impl TurnPayload for Payload {
    fn into_activity(self) -> Result<SpeechActivity, Self> {
        match self {
            Self::Activity(activity) => Ok(activity),
            other => Err(other),
        }
    }

    fn from_turn(event: TurnEvent) -> Self {
        Self::Turn(event)
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
        thread::park();
    }
}

fn eventually(ready: impl Fn() -> bool) -> bool {
    let until = Instant::now() + PATIENCE;
    while Instant::now() < until {
        if ready() {
            return true;
        }
        thread::sleep(Duration::from_millis(1));
    }
    ready()
}

struct Fixture {
    profile: DeploymentProfile,
}

impl Catalog for Fixture {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.profile.profile_id() == profile_id).then_some(&self.profile)
    }

    fn adapter(&self, _: AdapterId) -> Option<&Adapter> {
        None
    }
}

#[derive(Debug, Clone, Copy)]
struct Nodes {
    source: NodeId,
    detector: NodeId,
    policy: NodeId,
    sink: NodeId,
}

fn planned() -> (Declaration, PlannedPipeline, Nodes) {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    let catalog = Fixture { profile };
    let audio = PortTraits {
        essentiality: Essentiality::Essential,
        delivery: Delivery::Reliable,
        incremental: true,
        end_marker_required: true,
        provisional_allowed: false,
    };
    let control = PortTraits {
        incremental: false,
        ..audio
    };
    let spec = PipelineSpec {
        name: "endpoint controller".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let capacity = NonZeroUsize::new(8).expect("8");
    let mut declaration = Declaration::new();
    let (pipeline_id, nodes) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let source = pipeline.node("microphone", NodeRole::Source, Vec::new());
            let detector = pipeline.node("vad", NodeRole::Policy, Vec::new());
            let policy = pipeline.node("turn policy", NodeRole::Policy, Vec::new());
            let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
            let captured = pipeline.output::<modality::Audio>(&source, "audio", audio);
            let heard = pipeline.input::<modality::Audio>(&detector, "audio", audio);
            let detected = pipeline.output::<modality::Control>(&detector, "activity", control);
            let activity = pipeline.input::<modality::Control>(&policy, "activity", control);
            let turns = pipeline.output::<modality::Control>(&policy, "turns", control);
            let collected = pipeline.input::<modality::Control>(&sink, "turns", control);
            pipeline.connect(&captured, &heard, capacity, OverloadPolicy::Pause);
            pipeline.connect(&detected, &activity, capacity, OverloadPolicy::Pause);
            pipeline.connect(&turns, &collected, capacity, OverloadPolicy::Pause);
            Nodes {
                source: source.id(),
                detector: detector.id(),
                policy: policy.id(),
                sink: sink.id(),
            }
        })
        .expect("the profile is declared");
    let validated = declaration.validate(pipeline_id).expect("valid");
    let planned = declaration.plan(validated, &catalog).expect("planned");
    (declaration, planned, nodes)
}

#[derive(Debug, Clone, Default)]
struct Store(Arc<Mutex<StreamDomain>>);

impl Store {
    fn with<T>(&self, read: impl FnOnce(&mut StreamDomain) -> T) -> T {
        read(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn states(&self) -> Vec<TurnState> {
        self.with(|stream| stream.turns_view().iter().map(|row| row.state).collect())
    }
}

impl TurnCommands for Store {
    fn open_turn(&mut self, input: OpenTurn) -> Result<TurnOpened, String> {
        Ok(self.with(|stream| stream.open_turn(input)))
    }

    fn complete_turn(&mut self, turn_id: TurnId) -> Result<TurnCompleted, CompleteTurnRefused> {
        self.with(|stream| stream.complete_turn(turn_id))
            .map_err(|refused| match refused {
                TurnMoveError::Unknown(error) => CompleteTurnRefused::Unknown(error),
                TurnMoveError::Conflict(error) => CompleteTurnRefused::Conflict(error),
            })
    }
}

struct Running {
    source: SourceHandle<Payload>,
    sink: SinkHandle<Payload>,
    requests: RunRequests,
    ended: mpsc::Receiver<(RunState, Result<(), RunError>)>,
}

fn start(store: Store, config: VadConfig, frames: Vec<AudioFrame>) -> Running {
    let (declaration, planned, nodes) = planned();
    let run_id = RunId::generate();
    let held = frames.into_iter().map(|frame| {
        (
            "inference.audio.AudioFrame".to_owned(),
            None,
            Payload::Audio(frame),
        )
    });
    let source = TextSource::scripted(run_id, held.collect::<Vec<_>>());
    let detector = VoiceActivityDetector::new(run_id, config);
    let policy = TurnPolicy::new(run_id, store);
    let sink = TextSink::<Payload>::default();
    let (source_handle, sink_handle) = (source.handle(), sink.handle());
    let mut implementations: HashMap<NodeId, Box<dyn NodeImplementation<Payload>>> =
        HashMap::from([
            (
                nodes.source,
                Box::new(source) as Box<dyn NodeImplementation<Payload>>,
            ),
            (nodes.detector, Box::new(detector)),
            (nodes.policy, Box::new(policy)),
            (nodes.sink, Box::new(sink)),
        ]);
    let mut controller = RunController::create_as(
        run_id,
        &planned,
        &declaration,
        HashMap::<GenerationId, i64>::new(),
        |node: &Node| implementations.remove(&node.node_id()).expect("a node"),
    )
    .expect("created");
    controller.validate().expect("validates");
    block_on(controller.prepare()).expect("prepares");
    block_on(controller.activate()).expect("starts");
    let requests = controller.requests();
    let (sender, ended) = mpsc::channel();
    thread::spawn(move || {
        let result = block_on(controller.drive());
        let _ = sender.send((controller.run().state(), result));
    });
    Running {
        source: source_handle,
        sink: sink_handle,
        requests,
        ended,
    }
}

impl Running {
    fn wait(&self) -> (RunState, Result<(), RunError>) {
        match self.ended.recv_timeout(PATIENCE) {
            Ok(ended) => ended,
            Err(_) => {
                self.requests.cancel();
                panic!("the run did not end within {PATIENCE:?}");
            }
        }
    }

    fn finish(&self) -> Vec<EventEnvelope<Payload>> {
        while self.source.release() {}
        assert!(self.source.release_end());
        assert_eq!(self.wait(), (RunState::Completed, Ok(())));
        self.sink.received()
    }
}

/// The kind and the audio frame time stamp of every envelope the sink received.
fn stamped(received: &[EventEnvelope<Payload>]) -> Vec<(String, Timebase, i64)> {
    received
        .iter()
        .filter(|envelope| envelope.kind != ControlKind::Stop.as_str())
        .map(|envelope| (envelope.kind.clone(), envelope.timebase, envelope.timestamp))
        .collect()
}

/// `count` frames of 16 kHz mono `pcm_s16le`, every sample `value`, from sample 0.
fn constant(count: u64, value: i16) -> Vec<AudioFrame> {
    let format = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).expect("16000"),
        channels: NonZeroU16::new(1).expect("1"),
    };
    (0..count)
        .map(|index| AudioFrame {
            format,
            position: SampleClockPosition::new(index * u64::from(FRAME_SAMPLES)).expect("small"),
            sample_count: FRAME_SAMPLES,
            data: value.to_le_bytes().repeat(FRAME_SAMPLES as usize),
        })
        .collect()
}

/// The fixture never ends during speech. Input that does, 300 ms of tone and its end of input,
/// must still complete the turn it opened: the detector ends speech just past the last frame
/// (`vad.rs` module doc) and the turn policy completes on that.
#[test]
fn the_end_of_input_during_speech_completes_the_open_turn() {
    let store = Store::default();
    let tone = Stretch::Tone {
        frames: 30,
        hertz: 440,
        amplitude: 0.5,
    };
    let running = start(store.clone(), CONFIG, synthesize(&[tone]));
    let received = running.finish();
    assert_eq!(
        stamped(&received),
        [
            (
                "inference.stream.TurnOpened".to_owned(),
                Timebase::AudioFrameTime,
                480
            ),
            (
                "inference.stream.TurnCompleted".to_owned(),
                Timebase::AudioFrameTime,
                4800
            ),
        ]
    );
    assert_eq!(store.states(), [TurnState::Completed]);
}

/// A drain asked for mid-utterance (`RunRequests::drain`, `inference.graph.DrainRun`) ends the
/// run's input as surely as a `stop` does. The turn policy is the single authority for turns
/// (`docs/design/inference.md` 699); once the run is Completed nothing completes a turn it left
/// Open, so the drained run must not leave one.
#[test]
fn a_drained_run_leaves_no_turn_open() {
    let store = Store::default();
    let running = start(store.clone(), CONFIG, tone_silence_tone_silence());
    for _ in 0..30 {
        assert!(running.source.release());
    }
    assert!(
        eventually(|| store.states() == [TurnState::Open]),
        "the first tone opens a turn"
    );
    running.requests.drain();
    assert_eq!(running.wait(), (RunState::Completed, Ok(())));
    assert_eq!(
        store.states(),
        [TurnState::Completed],
        "the drained run left its turn Open; the sink received {:?}",
        stamped(&running.sink.received())
    );
}

/// The detector calls a frame loud when its level "reaches" the threshold (`vad.rs` module doc):
/// a frame at exactly the threshold, constant 16384 of 32768 against 0.5, opens a turn.
#[test]
fn a_level_exactly_at_the_threshold_is_loud() {
    let store = Store::default();
    let config = VadConfig {
        threshold: 0.5,
        ..CONFIG
    };
    let running = start(store.clone(), config, constant(10, 16_384));
    let received = running.finish();
    assert_eq!(
        stamped(&received),
        [
            (
                "inference.stream.TurnOpened".to_owned(),
                Timebase::AudioFrameTime,
                480
            ),
            (
                "inference.stream.TurnCompleted".to_owned(),
                Timebase::AudioFrameTime,
                1600
            ),
        ]
    );
}

/// The attack time is counted in samples "rounded up" (`samples_in`): 30.031 ms at 16 kHz is
/// 480.5 samples, so 3 frames of 160 do not reach it and speech starts on the fourth.
#[test]
fn an_attack_time_between_samples_rounds_up() {
    let store = Store::default();
    let config = VadConfig {
        attack: Duration::from_micros(30_031),
        ..CONFIG
    };
    let running = start(store.clone(), config, constant(10, 16_384));
    let received = running.finish();
    assert_eq!(
        stamped(&received).first(),
        Some(&(
            "inference.stream.TurnOpened".to_owned(),
            Timebase::AudioFrameTime,
            640
        ))
    );
}

/// Each turn event names the speech activity that caused it (`turn_policy.rs` module doc), and
/// no two name the same one.
#[test]
fn every_turn_event_names_its_cause() {
    let store = Store::default();
    let running = start(store, CONFIG, tone_silence_tone_silence());
    let received = running.finish();
    let causes: Vec<_> = received
        .iter()
        .filter(|envelope| envelope.kind != ControlKind::Stop.as_str())
        .map(|envelope| envelope.causation_id)
        .collect();
    assert_eq!(causes.len(), 4);
    assert!(causes.iter().all(Option::is_some), "{causes:?}");
    let mut distinct = causes.clone();
    distinct.sort_by_key(|cause| cause.map(|id| id.to_string()));
    distinct.dedup();
    assert_eq!(distinct.len(), 4, "{causes:?}");
}
