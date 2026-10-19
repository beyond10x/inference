//! The endpoint controller: the voice activity detector and the turn policy of
//! `inference-pipeline` open a turn at each speech onset and complete it after the hold time
//! (`story:endpoint-controller`; `docs/design/inference.md` 466, 606, 699).
//!
//! Each case declares source, detector, turn policy and sink with the typed builder, the audio
//! edge Audio and the others Control, every edge bounded and pausing on overload, plans it and
//! drives one run of it with [`RunController::drive`]. The source is the testkit's mock source
//! holding the synthetic fixture, tone, silence, tone, silence; the sink its collecting sink; the
//! turn commands are answered by the testkit's in-memory store of `inference.stream`, and the
//! turn events the sink received are read back through its event log. The cases witness
//! `inference.stream.OpenTurn/outcome/opened`, `inference.stream.CompleteTurn/outcome/completed`,
//! `inference.stream.CompleteTurn/outcome/wrong-state`,
//! `inference.stream.Turn/state/Completed/refuses/inference.stream.CompleteTurn` and
//! `inference.stream.Turn/transition/complete/by/inference.stream.CompleteTurn/completed`.

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant};

use inference_core::envelope::{ControlKind, EventEnvelope, Timebase};
use inference_core::id::{GenerationId, RunId};
use inference_core::model::{Adapter, AdapterId, ModelId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::nodes::turn_policy::{
    CompleteTurnRefused, TurnCommands, TurnEvent, TurnOutcome, TurnPayload, TurnPolicy,
    TurnPolicyHandle,
};
use inference_pipeline::nodes::vad::{
    AudioPayload, SpeechActivity, VadConfig, VoiceActivityDetector,
};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeImplementation,
    NodeRole, OverloadPolicy, PipelineSpec, PlannedPipeline, PortTraits, modality,
};
use inference_runtime::run::{NodeInstanceState, RunState};
use inference_runtime::{NodeId, RunController, RunError};
use inference_testkit::graph::{SinkHandle, SourceHandle, TextSink, TextSource};
use inference_testkit::stream::{EventLog, StreamDomain, StreamEvent, TurnMoveError};
use inference_testkit::stt::inference_stt::inference_audio::AudioFrame;
use inference_testkit::stt::inference_stt::{Recogniser, RecognitionHandle, RecognitionRequest};
use inference_testkit::stt::synthetic_audio::tone_silence_tone_silence;
use inference_testkit::stt::{Cue, MockConfiguration, MockRecogniser, ScriptStep};
use inference_text::{
    EmitHypothesis, OpenTurn, SpanState, TurnCompleted, TurnId, TurnOpened, TurnState,
    TurnStateConflict,
};

/// How long a case waits for the exchange tasks before it fails rather than hangs.
const PATIENCE: Duration = Duration::from_secs(10);

/// The detector of every case: a frame is loud from a tenth of half scale, speech starts after
/// 30 ms of loud frames and ends after 200 ms of quiet ones.
const CONFIG: VadConfig = VadConfig {
    threshold: 0.05,
    attack: Duration::from_millis(30),
    hold: Duration::from_millis(200),
};

/// The frames of the first tone of the fixture.
const FIRST_TONE: usize = 30;

/// Where the turn events of the fixture are decided on its sample clock, 16 samples a
/// millisecond: each onset 30 ms into a tone, which starts at 0 ms and at 800 ms, and each
/// completion 200 ms into a pause, which starts at 300 ms and at 1100 ms.
const OPENED_AT: [i64; 2] = [30 * 16, 830 * 16];
const COMPLETED_AT: [i64; 2] = [500 * 16, 1300 * 16];

/// What the edges of the test pipeline carry.
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

/// Polls `future` on this thread until it completes: the runtime commits to no executor.
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

/// Whether `ready` holds within [`PATIENCE`], asked every millisecond.
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

/// The profile the pipeline is declared under; no node is bound to an adapter.
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

/// The four nodes of the pipeline.
#[derive(Debug, Clone, Copy)]
struct Nodes {
    source: NodeId,
    detector: NodeId,
    policy: NodeId,
    sink: NodeId,
}

/// Source to detector to turn policy to sink: the audio port Audio, the activity and turn ports
/// Control, every port reliable and every edge bounded and pausing on overload, so that no frame
/// and no event is lost.
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
    let validated = declaration
        .validate(pipeline_id)
        .expect("source to detector to turn policy to sink is valid");
    let planned = declaration
        .plan(validated, &catalog)
        .expect("no node requires a capability");
    (declaration, planned, nodes)
}

/// The testkit's in-memory store of `inference.stream`, answering the turn policy's commands.
/// Clones share one store.
#[derive(Debug, Clone, Default)]
struct Store(Arc<Mutex<StreamDomain>>);

impl Store {
    fn with<T>(&self, read: impl FnOnce(&mut StreamDomain) -> T) -> T {
        read(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
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

/// One run, Active, with its drive loop on a thread of its own.
struct Running {
    nodes: Nodes,
    run_id: RunId,
    source: SourceHandle<Payload>,
    policy: TurnPolicyHandle,
    sink: SinkHandle<Payload>,
    drive: JoinHandle<(RunController<Payload>, Result<(), RunError>)>,
}

/// Starts a run whose source holds the fixture and whose turn policy's commands `commands`
/// answers.
fn start(commands: impl TurnCommands) -> Running {
    let (declaration, planned, nodes) = planned();
    let run_id = RunId::generate();
    let frames = tone_silence_tone_silence().into_iter().map(|frame| {
        (
            "inference.audio.AudioFrame".to_owned(),
            None,
            Payload::Audio(frame),
        )
    });
    let source = TextSource::scripted(run_id, frames.collect::<Vec<_>>());
    let detector = VoiceActivityDetector::new(run_id, CONFIG);
    let policy = TurnPolicy::new(run_id, commands);
    let sink = TextSink::<Payload>::default();
    let (source_handle, policy_handle, sink_handle) =
        (source.handle(), policy.handle(), sink.handle());
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
        |node: &Node| {
            implementations
                .remove(&node.node_id())
                .expect("every node of the pipeline has an implementation")
        },
    )
    .expect("the pipeline is planned and no edge cancels or supersedes");
    controller.validate().expect("a Created run validates");
    block_on(controller.prepare()).expect("every node prepares");
    block_on(controller.activate()).expect("every node starts");
    assert_eq!(controller.run().state(), RunState::Active);
    let drive = thread::spawn(move || {
        let result = block_on(controller.drive());
        (controller, result)
    });
    Running {
        nodes,
        run_id,
        source: source_handle,
        policy: policy_handle,
        sink: sink_handle,
        drive,
    }
}

impl Running {
    /// Releases the next `count` frames the source holds.
    fn release(&self, count: usize) {
        for _ in 0..count {
            assert!(self.source.release(), "the source holds another frame");
        }
    }

    /// Releases every frame the source still holds, then its end of input, and waits for the
    /// drive loop to complete the run with every node instance Stopped; what the sink received.
    fn finish(self) -> Vec<EventEnvelope<Payload>> {
        while self.source.release() {}
        assert!(
            self.source.release_end(),
            "the source admits its end of input"
        );
        let (controller, result) = self.drive.join().expect("the drive loop does not panic");
        result.expect("the drive loop ends the run");
        assert_eq!(controller.run().state(), RunState::Completed);
        for node_id in [
            self.nodes.source,
            self.nodes.detector,
            self.nodes.policy,
            self.nodes.sink,
        ] {
            assert_eq!(
                controller
                    .instance(node_id)
                    .map(|instance| instance.state()),
                Some(NodeInstanceState::Stopped),
                "the instance of node {node_id}"
            );
        }
        self.sink.received()
    }
}

/// The turn events of `received`, each in its envelope with its payload as a [`StreamEvent`],
/// recorded in an event log in the order received; the end of input is the last envelope and
/// carries no turn event.
fn logged(received: Vec<EventEnvelope<Payload>>) -> EventLog {
    let (last, events) = received
        .split_last()
        .expect("the sink received the end of input");
    assert_eq!(last.payload, Payload::Control(ControlKind::Stop));
    let mut log = EventLog::new();
    for envelope in events {
        let event = match envelope.payload {
            Payload::Turn(TurnEvent::Opened(event)) => StreamEvent::TurnOpened(event),
            Payload::Turn(TurnEvent::Completed(event)) => StreamEvent::TurnCompleted(event),
            ref other => panic!("the turn policy sent {other:?}, not a turn event"),
        };
        let envelope = EventEnvelope {
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
            payload: event,
        };
        log.record(envelope)
            .expect("the log takes every turn event");
    }
    log
}

#[test]
fn two_utterances_open_and_complete_two_turns_in_order_and_complete_no_span() {
    let store = Store::default();
    let running = start(store.clone());

    running.release(FIRST_TONE);
    assert!(
        eventually(|| store.with(|stream| stream.turns_view().len()) == 1),
        "the first tone opens a turn"
    );
    let first = store.with(|stream| stream.turns_view()[0].turn_id);

    let mut recogniser = MockRecogniser::new(
        MockConfiguration::Incremental,
        ModelId::generate(),
        vec![Cue {
            after_frames: 1,
            step: ScriptStep::Hypothesis("hello".to_owned()),
        }],
    );
    let mut recognition = recogniser
        .start(RecognitionRequest::new(
            running.run_id,
            first,
            inference_core::id::StreamId::generate(),
            NodeId::generate(),
            "text_out",
        ))
        .expect("the mock admits one recognition");
    let frame = tone_silence_tone_silence().remove(0);
    recognition.offer_frame(frame).expect("input has not ended");
    let hypothesis = recognition.next().expect("the script emits one hypothesis");
    let text = match hypothesis.payload {
        inference_testkit::stt::inference_stt::TranscriptEvent::Hypothesis(event) => event.text,
        other => panic!("the script emits a hypothesis, not {other:?}"),
    };
    assert!(recognition.next().is_none(), "the script commits nothing");
    store
        .with(|stream| {
            stream.emit_hypothesis(EmitHypothesis {
                run_id: running.run_id,
                turn_id: first,
                stream_id: hypothesis.stream_id,
                sequence: hypothesis.sequence,
                text,
            })
        })
        .expect("the first turn is Open while its tone plays");

    let run_id = running.run_id;
    let policy = running.policy.clone();
    let log = logged(running.finish());

    let kinds: Vec<&str> = log
        .envelopes()
        .iter()
        .map(|envelope| envelope.kind.as_str())
        .collect();
    assert_eq!(
        kinds,
        [
            "inference.stream.TurnOpened",
            "inference.stream.TurnCompleted",
            "inference.stream.TurnOpened",
            "inference.stream.TurnCompleted",
        ]
    );
    let stamps: Vec<(Timebase, i64)> = log
        .envelopes()
        .iter()
        .map(|envelope| (envelope.timebase, envelope.timestamp))
        .collect();
    assert_eq!(
        stamps,
        [
            (Timebase::AudioFrameTime, OPENED_AT[0]),
            (Timebase::AudioFrameTime, COMPLETED_AT[0]),
            (Timebase::AudioFrameTime, OPENED_AT[1]),
            (Timebase::AudioFrameTime, COMPLETED_AT[1]),
        ],
        "each turn opens 30 ms into its tone and completes 200 ms into its pause"
    );
    let events: Vec<StreamEvent> = log
        .envelopes()
        .iter()
        .map(|envelope| envelope.payload.clone())
        .collect();
    let turns = store.with(|stream| stream.turns_view());
    assert_eq!(turns.len(), 2, "inference.stream.Turns lists two turns");
    let second = turns[1].turn_id;
    assert_eq!(
        events,
        [
            StreamEvent::TurnOpened(TurnOpened {
                turn_id: first,
                run_id
            }),
            StreamEvent::TurnCompleted(TurnCompleted { turn_id: first }),
            StreamEvent::TurnOpened(TurnOpened {
                turn_id: second,
                run_id
            }),
            StreamEvent::TurnCompleted(TurnCompleted { turn_id: second }),
        ]
    );
    for row in &turns {
        assert_eq!((row.run_id, row.state), (run_id, TurnState::Completed));
    }
    assert_eq!(
        policy.outcomes(),
        [
            TurnOutcome::Opened(TurnOpened {
                turn_id: first,
                run_id
            }),
            TurnOutcome::Completed(TurnCompleted { turn_id: first }),
            TurnOutcome::Opened(TurnOpened {
                turn_id: second,
                run_id
            }),
            TurnOutcome::Completed(TurnCompleted { turn_id: second }),
        ]
    );

    let recorded = store.with(|stream| stream.events().len());
    let mut commands = store.clone();
    assert_eq!(
        commands.complete_turn(first),
        Err(CompleteTurnRefused::Conflict(TurnStateConflict {
            current: TurnState::Completed
        })),
        "a repeated CompleteTurn is refused wrong-state"
    );
    assert_eq!(store.with(|stream| stream.events().len()), recorded);
    assert_eq!(store.with(|stream| stream.turns_view()), turns);

    let spans = store.with(|stream| stream.spans_view());
    assert_eq!(spans.len(), 1);
    assert_eq!(
        (spans[0].turn_id, spans[0].text.as_str(), spans[0].state),
        (first, "hello", SpanState::Hypothesis),
        "completing the turn committed no span"
    );
}

/// Another authority completes each turn just before the turn policy does: the store refuses the
/// policy's completion.
#[derive(Debug, Clone, Default)]
struct Contested(Store);

impl TurnCommands for Contested {
    fn open_turn(&mut self, input: OpenTurn) -> Result<TurnOpened, String> {
        self.0.open_turn(input)
    }

    fn complete_turn(&mut self, turn_id: TurnId) -> Result<TurnCompleted, CompleteTurnRefused> {
        self.0
            .complete_turn(turn_id)
            .expect("the other authority completes the Open turn");
        self.0.complete_turn(turn_id)
    }
}

#[test]
fn a_completion_the_store_refuses_is_recorded_wrong_state_and_sends_nothing() {
    let contested = Contested::default();
    let running = start(contested.clone());
    let run_id = running.run_id;
    let policy = running.policy.clone();
    let log = logged(running.finish());

    let turns = contested.0.with(|stream| stream.turns_view());
    assert_eq!(turns.len(), 2);
    let refused = CompleteTurnRefused::Conflict(TurnStateConflict {
        current: TurnState::Completed,
    });
    assert_eq!(
        policy.outcomes(),
        [
            TurnOutcome::Opened(TurnOpened {
                turn_id: turns[0].turn_id,
                run_id
            }),
            TurnOutcome::WrongState {
                turn_id: turns[0].turn_id,
                refused
            },
            TurnOutcome::Opened(TurnOpened {
                turn_id: turns[1].turn_id,
                run_id
            }),
            TurnOutcome::WrongState {
                turn_id: turns[1].turn_id,
                refused
            },
        ]
    );
    let kinds: Vec<&str> = log
        .envelopes()
        .iter()
        .map(|envelope| envelope.kind.as_str())
        .collect();
    assert_eq!(
        kinds,
        ["inference.stream.TurnOpened", "inference.stream.TurnOpened"],
        "a refused completion is not sent on"
    );
}

/// The refusal a [`Refusing`] store answers every `OpenTurn` with.
const NOT_ACTIVE: &str = "run-not-active: the run is Draining";

/// A store that refuses every `OpenTurn`, as the testkit refuses one for a run that is not Active
/// (`inference.stream.OpenTurn/outcome/run-not-active`).
#[derive(Debug, Clone, Default)]
struct Refusing(Store);

impl TurnCommands for Refusing {
    fn open_turn(&mut self, _: OpenTurn) -> Result<TurnOpened, String> {
        Err(NOT_ACTIVE.to_owned())
    }

    fn complete_turn(&mut self, turn_id: TurnId) -> Result<TurnCompleted, CompleteTurnRefused> {
        self.0.complete_turn(turn_id)
    }
}

#[test]
fn an_open_the_store_refuses_is_recorded_sends_nothing_and_the_run_completes() {
    let refusing = Refusing::default();
    let running = start(refusing.clone());
    let policy = running.policy.clone();
    let log = logged(running.finish());

    let refused = TurnOutcome::NotOpened {
        refused: NOT_ACTIVE.to_owned(),
    };
    assert_eq!(policy.outcomes(), [refused.clone(), refused]);
    assert!(log.is_empty(), "a refused open is not sent on");
    assert!(refusing.0.with(|stream| stream.turns_view().is_empty()));
}
