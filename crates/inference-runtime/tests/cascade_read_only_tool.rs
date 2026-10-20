//! The voice cascade executes a read-only tool and speaks the result
//! (`story:cascade-read-only-tool`; topology B of `docs/design/inference.md` 759-778, the exit
//! condition of P3 at 1161, demonstrator step 5 at 1175).
//!
//! One cascade declaration, under the `offline_strict` profile with egress denied, bound to the
//! testkit mocks: the microphone holds a synthetic utterance, one tone and a pause; a fan-out
//! hands its frames to the voice activity detector and to the STT node; the turn policy opens the
//! turn at the tone's onset and completes it after the hold time; the STT node recognises the turn
//! on the mock recogniser, scripted to emit and commit the question's span, and passes the turn's
//! completion on after its committed spans; the LLM node, given a turn input, starts its
//! generation at that `TurnCompleted`, offers its adapter the two declared tools and answers the
//! question on the mock LLM, scripted to propose the read-only call; the loop through the tool
//! authority and the executor, the mock executor serving both tools, returns the result, which the
//! LLM node reinjects into the same generation; the TTS node synthesises every committed segment
//! on the mock synthesiser and acknowledges its playback through the sink double.
//!
//! The case witnesses `inference.tools.ProposeToolCall/outcome/proposed`,
//! `inference.tools.CompleteToolCall/outcome/completed`,
//! `inference.stream.Turn/transition/complete/by/inference.stream.CompleteTurn/completed` and
//! `inference.stream.OutputSegment/transition/play/by/inference.stream.AcknowledgePlayback/played`,
//! and check 1: the mock LLM records the tool set the LLM node offered it.

use std::collections::HashMap;
use std::future::Future;
use std::num::{NonZeroU16, NonZeroU32, NonZeroUsize};
use std::pin::pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant};

use inference_core::envelope::ControlKind;
use inference_core::generation::GenerationState;
use inference_core::id::{GenerationId, RunId};
use inference_core::model::{Adapter, AdapterId, Capability, ModelId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_llm::tools::tool_id;
use inference_pipeline::nodes::turn_policy::{
    CompleteTurnRefused, TurnCommands, TurnEvent, TurnOutcome, TurnPayload, TurnPolicy,
};
use inference_pipeline::nodes::vad::{
    AudioPayload, SpeechActivity, VadConfig, VoiceActivityDetector,
};
use inference_pipeline::nodes::{
    AuthorityOutcome, ToolAuthority, ToolCallUpdate, ToolExecutorNode, ToolPayload,
};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeImplementation,
    NodeRole, OverloadPolicy, PipelineSpec, PlannedPipeline, PortId, PortTraits, modality,
};
use inference_runtime::run::{NodeInstanceState, RunState};
use inference_runtime::{NodeId, RunController, RunError, RunRequests};
use inference_testkit::graph::{RunLog, TextSink, TextSource};
use inference_testkit::llm::{
    Carried, MockConfiguration as LlmConfiguration, MockGenerator, ModelNode, ModelPayload,
};
use inference_testkit::stream::{StreamDomain, StreamEvent, TurnMoveError};
use inference_testkit::stt::inference_stt::Recogniser;
use inference_testkit::stt::inference_stt::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame,
};
use inference_testkit::stt::synthetic_audio::{Stretch, synthesize};
use inference_testkit::stt::{
    Cue, FanOut, Heard, MockConfiguration as SttConfiguration, MockRecogniser, ScriptStep, SttNode,
    SttPayload, SttPorts,
};
use inference_testkit::tools::{MockToolExecutor, Script, ToolsDomain};
use inference_testkit::tts::inference_tts::Synthesiser;
use inference_testkit::tts::{
    MockConfiguration as TtsConfiguration, MockSynthesiser, Spoken, TtsNode, TtsPayload,
};
use inference_text::{
    OpenTurn, SegmentState, SpanState, TurnCompleted, TurnId, TurnOpened, TurnState,
};
use inference_tools::{DeclareTool, Json, SideEffectClass, Tool, ToolCallState};
use serde_json::json;

/// How long the case waits for the run to end before it fails rather than hangs, and the
/// executor's deadline.
const PATIENCE: Duration = Duration::from_secs(10);

/// What the user asks: the recogniser's scripted span.
const QUESTION: &str = "what is the code word of the day";

/// The fixed fact the read-only tool answers, which no model knows from its weights.
const CODE_WORD: &str = "tangerine";

/// The detector: a frame is loud from a tenth of half scale, speech starts after 30 ms of loud
/// frames and ends after 200 ms of quiet ones.
const VAD: VadConfig = VadConfig {
    threshold: 0.05,
    attack: Duration::from_millis(30),
    hold: Duration::from_millis(200),
};

/// The mock LLM's answer to the question, before it proposes the call.
fn reply(question: &str) -> String {
    format!("let me look that up: {question}")
}

/// What the edges of the cascade carry.
#[derive(Debug, Clone)]
enum Payload {
    Audio(AudioFrame),
    Activity(SpeechActivity),
    Stream(StreamEvent),
    Tool(ToolCallUpdate),
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
        Self::Stream(match event {
            TurnEvent::Opened(opened) => StreamEvent::TurnOpened(opened),
            TurnEvent::Completed(completed) => StreamEvent::TurnCompleted(completed),
        })
    }

    fn control(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

impl SttPayload for Payload {
    fn stream(event: StreamEvent) -> Self {
        Self::Stream(event)
    }

    fn heard(self) -> Heard {
        match self {
            Self::Audio(frame) => Heard::Audio(frame),
            Self::Stream(event) => Heard::Stream(event),
            Self::Control(kind) => Heard::Control(kind),
            Self::Activity(_) | Self::Tool(_) => Heard::Other,
        }
    }
}

impl ModelPayload for Payload {
    fn stream(event: StreamEvent) -> Self {
        Self::Stream(event)
    }

    fn carried(self) -> Carried {
        match self {
            Self::Stream(event) => Carried::Stream(event),
            Self::Tool(update) => Carried::Tool(update),
            Self::Control(kind) => Carried::Control(kind),
            Self::Audio(_) | Self::Activity(_) => Carried::Control(ControlKind::Error),
        }
    }
}

impl ToolPayload for Payload {
    fn into_tool_call(self) -> Result<ToolCallUpdate, Self> {
        match self {
            Self::Tool(update) => Ok(update),
            other => Err(other),
        }
    }

    fn from_tool_call(update: ToolCallUpdate) -> Self {
        Self::Tool(update)
    }

    fn control(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

impl TtsPayload for Payload {
    fn stream(event: StreamEvent) -> Self {
        Self::Stream(event)
    }

    fn audio(frame: AudioFrame) -> Self {
        Self::Audio(frame)
    }

    fn spoken(self) -> Spoken {
        match self {
            Self::Stream(event) => Spoken::Stream(event),
            Self::Control(kind) => Spoken::Control(kind),
            Self::Audio(_) | Self::Activity(_) | Self::Tool(_) => Spoken::Other,
        }
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

/// The profile the cascade is declared under and the three adapters its model nodes bind to.
struct Fixture {
    profile: DeploymentProfile,
    adapters: Vec<Adapter>,
}

impl Catalog for Fixture {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.profile.profile_id() == profile_id).then_some(&self.profile)
    }

    fn adapter(&self, adapter_id: AdapterId) -> Option<&Adapter> {
        self.adapters
            .iter()
            .find(|adapter| adapter.adapter_id() == adapter_id)
    }
}

/// The nodes and the ports a run of the cascade names.
#[derive(Debug, Clone, Copy)]
struct Ids {
    microphone: NodeId,
    split: NodeId,
    vad: NodeId,
    policy: NodeId,
    stt: NodeId,
    llm: NodeId,
    authority: NodeId,
    executor: NodeId,
    tts: NodeId,
    speaker: NodeId,
    stt_ports: SttPorts,
    llm_turns: PortId,
    tool_call: PortId,
    tool_result: PortId,
    rejected: PortId,
}

impl Ids {
    fn nodes(self) -> [NodeId; 10] {
        [
            self.microphone,
            self.split,
            self.vad,
            self.policy,
            self.stt,
            self.llm,
            self.authority,
            self.executor,
            self.tts,
            self.speaker,
        ]
    }
}

/// The three model adapters the cascade binds: the bindings of the deterministic run.
struct Bindings {
    recogniser: MockRecogniser,
    generator: MockGenerator,
    synthesiser: MockSynthesiser,
}

/// The cascade, declared with the typed builder over `bindings`, validated and planned: the
/// declaration of topology B with its tool loop, the one rounds bounded by a feedback loop of 2.
fn planned(bindings: &Bindings) -> (Declaration, PlannedPipeline, Ids) {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: vec![inference_core::model::Locality::SameProcess],
    })
    .expect("a row of the table");
    assert_eq!(profile.egress(), Egress::Denied, "the whole run is offline");
    let profile_id = profile.profile_id();
    let catalog = Fixture {
        profile,
        adapters: vec![
            bindings.recogniser.adapter().clone(),
            bindings.generator.adapter().clone(),
            bindings.synthesiser.adapter().clone(),
        ],
    };
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
    let text = audio;
    let structured = control;
    let spec = PipelineSpec {
        name: "voice cascade with a read-only tool".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let four = NonZeroUsize::new(4).expect("4");
    let wide = NonZeroUsize::new(16).expect("16");
    let mut declaration = Declaration::new();
    let (pipeline_id, ids) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let microphone = pipeline.node("microphone", NodeRole::Source, Vec::new());
            let split = pipeline.node("split", NodeRole::FanOut, Vec::new());
            let vad = pipeline.node("vad", NodeRole::Policy, Vec::new());
            let policy = pipeline.node("turn policy", NodeRole::Policy, Vec::new());
            let stt = pipeline.node(
                "stt",
                NodeRole::Model,
                vec![Capability::IncrementalInput, Capability::OutputStreaming],
            );
            let llm = pipeline.node(
                "agent",
                NodeRole::Model,
                vec![Capability::OutputStreaming, Capability::ToolCalling],
            );
            let authority = pipeline.node("authority", NodeRole::Policy, Vec::new());
            let executor = pipeline.node("tools", NodeRole::Tool, Vec::new());
            let tts = pipeline.node(
                "tts",
                NodeRole::Model,
                vec![Capability::IncrementalInput, Capability::OutputStreaming],
            );
            let speaker = pipeline.node("output", NodeRole::Sink, Vec::new());

            let captured = pipeline.output::<modality::Audio>(&microphone, "audio", audio);
            let split_in = pipeline.input::<modality::Audio>(&split, "audio", audio);
            let to_vad = pipeline.output::<modality::Audio>(&split, "vad", audio);
            let to_stt = pipeline.output::<modality::Audio>(&split, "stt", audio);
            let heard = pipeline.input::<modality::Audio>(&vad, "audio", audio);
            let detected = pipeline.output::<modality::Control>(&vad, "activity", control);
            let activity = pipeline.input::<modality::Control>(&policy, "activity", control);
            let turns = pipeline.output::<modality::Control>(&policy, "turns", control);
            let stt_audio = pipeline.input::<modality::Audio>(&stt, "audio", audio);
            let stt_turns = pipeline.input::<modality::Control>(&stt, "turns", control);
            let committed = pipeline.output::<modality::Text>(&stt, "committed_text", text);
            let stt_turns_out = pipeline.output::<modality::Control>(&stt, "turns_out", control);
            let user_input = pipeline.input::<modality::Text>(&llm, "user_input", text);
            let llm_turns = pipeline.input::<modality::Control>(&llm, "turns", control);
            let answer = pipeline.output::<modality::Text>(&llm, "committed_text", text);
            let tool_call = pipeline.output::<modality::Structured>(&llm, "tool_call", structured);
            let tool_result =
                pipeline.input::<modality::Structured>(&llm, "tool_result", structured);
            let proposed =
                pipeline.input::<modality::Structured>(&authority, "proposed", structured);
            let authorized =
                pipeline.output::<modality::Structured>(&authority, "authorized", structured);
            let rejected =
                pipeline.output::<modality::Structured>(&authority, "rejected", structured);
            let calls = pipeline.input::<modality::Structured>(&executor, "call", structured);
            let results = pipeline.output::<modality::Structured>(&executor, "result", structured);
            let speak = pipeline.input::<modality::Text>(&tts, "text", text);
            let spoken = pipeline.output::<modality::Audio>(&tts, "audio", audio);
            let played = pipeline.input::<modality::Audio>(&speaker, "audio", audio);

            pipeline.connect(&captured, &split_in, wide, OverloadPolicy::Pause);
            pipeline.connect(&to_vad, &heard, wide, OverloadPolicy::Pause);
            pipeline.connect(&to_stt, &stt_audio, wide, OverloadPolicy::Pause);
            pipeline.connect(&detected, &activity, wide, OverloadPolicy::Pause);
            pipeline.connect(&turns, &stt_turns, wide, OverloadPolicy::Pause);
            pipeline.connect(&committed, &user_input, wide, OverloadPolicy::Pause);
            pipeline.connect(&stt_turns_out, &llm_turns, wide, OverloadPolicy::Pause);
            pipeline.connect(&answer, &speak, wide, OverloadPolicy::Pause);
            pipeline.connect(&spoken, &played, wide, OverloadPolicy::Pause);
            pipeline.connect_feedback(
                &tool_call,
                &proposed,
                four,
                OverloadPolicy::Fail,
                NonZeroUsize::new(2).expect("2"),
            );
            pipeline.connect(&authorized, &calls, four, OverloadPolicy::Fail);
            pipeline.connect(&results, &tool_result, four, OverloadPolicy::Fail);
            pipeline.connect(&rejected, &tool_result, four, OverloadPolicy::Fail);
            pipeline
                .bind(&stt, bindings.recogniser.adapter().adapter_id())
                .expect("the catalog holds the mock recogniser's adapter");
            pipeline
                .bind(&llm, bindings.generator.adapter().adapter_id())
                .expect("the catalog holds the mock LLM's adapter");
            pipeline
                .bind(&tts, bindings.synthesiser.adapter().adapter_id())
                .expect("the catalog holds the mock synthesiser's adapter");
            Ids {
                microphone: microphone.id(),
                split: split.id(),
                vad: vad.id(),
                policy: policy.id(),
                stt: stt.id(),
                llm: llm.id(),
                authority: authority.id(),
                executor: executor.id(),
                tts: tts.id(),
                speaker: speaker.id(),
                stt_ports: SttPorts {
                    audio: stt_audio.id(),
                    turns: stt_turns.id(),
                    committed_text: committed.id(),
                    turns_out: stt_turns_out.id(),
                    transcript: None,
                },
                llm_turns: llm_turns.id(),
                tool_call: tool_call.id(),
                tool_result: tool_result.id(),
                rejected: rejected.id(),
            }
        })
        .expect("the profile is declared");
    let validated = declaration
        .validate(pipeline_id)
        .expect("the cascade is valid, its one cycle through a bounded feedback edge");
    let planned = declaration
        .plan(validated, &catalog)
        .expect("every model node is bound to an adapter claiming what it requires");
    (declaration, planned, ids)
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

/// The two tools of the run, declared through `inference.tools.DeclareTool`: the read-only
/// lookup of the code word, and an idempotent write the model is offered and never calls.
fn declared_tools() -> (ToolsDomain, Tool, Tool) {
    let mut tools = ToolsDomain::new();
    let mut declare = |name: &str, side_effect| {
        let declared = tools.declare_tool(DeclareTool {
            name: name.to_owned(),
            schema: json!({
                "type": "object",
                "properties": {"day": {"type": "string"}},
                "required": ["day"],
                "additionalProperties": false
            }),
            schema_revision: "1".to_owned(),
            side_effect,
            requires_confirmation: false,
        });
        declared.tool_id
    };
    let lookup = declare("code_word", SideEffectClass::ReadOnly);
    let remind = declare("set_reminder", SideEffectClass::IdempotentWrite);
    let tool = |tool_id| {
        tools
            .tools()
            .get(tool_id)
            .cloned()
            .expect("the domain stores what it declared")
    };
    let (lookup, remind) = (tool(lookup), tool(remind));
    (tools, lookup, remind)
}

/// What the read-only tool answers.
fn code_word() -> Json {
    json!({"code_word": CODE_WORD})
}

/// The synthetic utterance: 300 ms of a 440 Hz tone at half scale, then 800 ms of silence.
fn utterance() -> Vec<AudioFrame> {
    synthesize(&[
        Stretch::Tone {
            frames: 30,
            hertz: 440,
            amplitude: 0.5,
        },
        Stretch::Silence { frames: 80 },
    ])
}

/// The deterministic bindings: the recogniser scripted to emit the question and commit it, the
/// LLM scripted to propose the read-only call once, the synthesiser streaming.
fn mocks(lookup: &Tool) -> Bindings {
    let format = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).expect("16000"),
        channels: NonZeroU16::new(1).expect("1"),
    };
    Bindings {
        recogniser: MockRecogniser::new(
            SttConfiguration::Incremental,
            ModelId::generate(),
            vec![
                Cue {
                    after_frames: 1,
                    step: ScriptStep::Hypothesis(QUESTION.to_owned()),
                },
                Cue {
                    after_frames: 2,
                    step: ScriptStep::Commit { span: 0 },
                },
            ],
        ),
        generator: MockGenerator::new(
            LlmConfiguration::StreamingAndConcurrentInput,
            ModelId::generate(),
            Vec::<String>::new(),
        )
        .proposing(lookup.tool_id(), json!({"day": "today"}), 1),
        synthesiser: MockSynthesiser::new(
            TtsConfiguration::IncrementalStreaming,
            ModelId::generate(),
            format,
        ),
    }
}

/// Whether the drive loop ended within [`PATIENCE`]; when it did not, the run is cancelled so
/// that the case fails instead of hanging.
fn within(
    drive: &JoinHandle<(RunController<Payload>, Result<(), RunError>)>,
    requests: &RunRequests,
) -> bool {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if drive.is_finished() {
            return true;
        }
        thread::sleep(Duration::from_millis(5));
    }
    requests.cancel();
    false
}

#[test]
fn the_cascade_on_mocks_executes_the_read_only_tool_and_speaks_its_result() {
    let (tools, lookup, remind) = declared_tools();
    let bindings = mocks(&lookup);
    let generator = bindings.generator.clone();
    let (declaration, planned, ids) = planned(&bindings);
    let run_id = RunId::generate();
    let log = RunLog::new();
    let store = Store::default();

    let frames = utterance().into_iter().map(|frame| {
        (
            "inference.audio.AudioFrame".to_owned(),
            None,
            Payload::Audio(frame),
        )
    });
    let microphone = TextSource::scripted(run_id, frames.collect::<Vec<_>>());
    let split = FanOut::new();
    let vad = VoiceActivityDetector::new(run_id, VAD);
    let policy = TurnPolicy::new(run_id, store.clone());
    let stt = SttNode::new(run_id, bindings.recogniser, ids.stt_ports, log.clone());
    let llm = ModelNode::carrying(run_id, bindings.generator, reply, log.clone())
        .with_tools(
            [lookup.clone(), remind.clone()],
            ids.tool_call,
            ids.tool_result,
        )
        .with_turns(ids.llm_turns);
    let authority = ToolAuthority::new(run_id, [lookup.clone(), remind.clone()])
        .bounded_by(&declaration, &planned, ids.authority)
        .expect("the cascade declares a feedback loop into the authority")
        .rejecting_on(ids.rejected);
    let executor = ToolExecutorNode::new(
        run_id,
        Arc::new(MockToolExecutor::new([
            (lookup.tool_id(), Script::Result(code_word())),
            (remind.tool_id(), Script::Result(json!({"set": true}))),
        ])),
        [lookup.tool_id(), remind.tool_id()],
        PATIENCE,
    )
    .answering_to(&authority.handle());
    let tts = TtsNode::new(run_id, bindings.synthesiser, log.clone()).playing();
    let speaker = TextSink::<Payload>::default();
    let (microphone_handle, policy_handle, stt_handle, llm_handle, authority_handle, tts_handle) = (
        microphone.handle(),
        policy.handle(),
        stt.handle(),
        llm.handle(),
        authority.handle(),
        tts.handle(),
    );
    let speaker_handle = speaker.handle();
    let mut implementations: HashMap<NodeId, Box<dyn NodeImplementation<Payload>>> =
        HashMap::from([
            (
                ids.microphone,
                Box::new(microphone) as Box<dyn NodeImplementation<Payload>>,
            ),
            (ids.split, Box::new(split)),
            (ids.vad, Box::new(vad)),
            (ids.policy, Box::new(policy)),
            (ids.stt, Box::new(stt)),
            (ids.llm, Box::new(llm)),
            (ids.authority, Box::new(authority)),
            (ids.executor, Box::new(executor)),
            (ids.tts, Box::new(tts)),
            (ids.speaker, Box::new(speaker)),
        ]);
    let mut controller = RunController::create_as(
        run_id,
        &planned,
        &declaration,
        HashMap::<GenerationId, i64>::new(),
        |node: &Node| {
            implementations
                .remove(&node.node_id())
                .expect("every node of the cascade has an implementation")
        },
    )
    .expect("the cascade is planned and no edge cancels or supersedes");
    controller.validate().expect("a Created run validates");
    block_on(controller.prepare()).expect("every node prepares");
    block_on(controller.activate()).expect("every node starts");
    assert_eq!(controller.run().state(), RunState::Active);
    let requests = controller.requests();
    let drive = thread::spawn(move || {
        let result = block_on(controller.drive());
        (controller, result)
    });
    while microphone_handle.release() {}
    assert!(
        microphone_handle.release_end(),
        "the microphone admits its end"
    );
    let ended = within(&drive, &requests);
    let (controller, result) = drive.join().expect("the drive loop does not panic");
    assert!(ended, "the run ended within {PATIENCE:?}");
    assert_eq!(llm_handle.failure(), None, "the LLM node does not fail");
    assert_eq!(stt_handle.failure(), None, "the STT node does not fail");
    assert_eq!(tts_handle.failure(), None, "the TTS node does not fail");
    result.expect("the drive loop ends the run");
    assert_eq!(controller.run().state(), RunState::Completed);
    for node_id in ids.nodes() {
        assert_eq!(
            controller.instance(node_id).map(|i| i.state()),
            Some(NodeInstanceState::Stopped),
            "the instance of node {node_id}"
        );
    }

    // The turn: opened at the tone and completed after the hold time, by the turn policy.
    let turns = store.with(|stream| stream.turns_view());
    assert_eq!(turns.len(), 1, "one utterance, one turn");
    let turn = turns[0].turn_id;
    assert_eq!(
        (turns[0].run_id, turns[0].state),
        (run_id, TurnState::Completed)
    );
    assert_eq!(
        policy_handle.outcomes(),
        [
            TurnOutcome::Opened(TurnOpened {
                turn_id: turn,
                run_id
            }),
            TurnOutcome::Completed(TurnCompleted { turn_id: turn }),
        ]
    );

    // The question: one span of that turn, Committed.
    let spans = stt_handle.spans();
    assert_eq!(spans.len(), 1, "the recogniser emitted the question's span");
    let question = &spans[0];
    assert_eq!(
        (question.turn_id(), question.text(), question.state()),
        (turn, QUESTION, SpanState::Committed)
    );

    // Check 1: the LLM node offered its adapter both declared tools.
    let offered: Vec<Vec<_>> = generator
        .offered()
        .iter()
        .map(|set| set.iter().filter_map(|spec| tool_id(&spec.name)).collect())
        .collect();
    assert_eq!(
        offered,
        vec![vec![lookup.tool_id(), remind.tool_id()]],
        "one generation, offered the read-only tool and the idempotent write"
    );
    assert_eq!(
        tools
            .tools_view()
            .iter()
            .map(|row| (row.tool_id, row.side_effect))
            .collect::<Vec<_>>(),
        vec![
            (lookup.tool_id(), SideEffectClass::ReadOnly),
            (remind.tool_id(), SideEffectClass::IdempotentWrite),
        ]
    );

    // The generation starts at the turn's completion, after the question was committed.
    let envelopes = log.envelopes();
    let at = |wanted: fn(&StreamEvent) -> bool| {
        envelopes
            .iter()
            .position(|envelope| wanted(&envelope.payload))
    };
    let committed_at = at(|event| matches!(event, StreamEvent::TranscriptCommitted(_)))
        .expect("the STT node committed the question");
    let completed_at = at(|event| matches!(event, StreamEvent::TurnCompleted(_)))
        .expect("the STT node passed the turn's completion on");
    let started_at = at(|event| matches!(event, StreamEvent::GenerationStarted(_)))
        .expect("the LLM node started a generation");
    assert!(
        committed_at < completed_at && completed_at < started_at,
        "committed {committed_at}, turn completed {completed_at}, generation started {started_at}"
    );
    let generation = match &envelopes[started_at].payload {
        StreamEvent::GenerationStarted(started) => started.generation_id,
        other => panic!("not a GenerationStarted: {other:?}"),
    };
    assert_eq!(
        llm_handle.generation_state(),
        Some(GenerationState::Completed)
    );

    // The call: one, proposed, authorized and Completed, resting on the question's Committed
    // span and belonging to the generation; none for the idempotent write.
    let decisions = authority_handle.decisions();
    assert_eq!(decisions.len(), 1, "one proposal reached the authority");
    assert!(
        matches!(decisions[0].outcome, AuthorityOutcome::Authorized(_)),
        "{:?}",
        decisions[0].outcome
    );
    let calls = authority_handle.calls();
    assert_eq!(calls.len(), 1, "the ToolCalls view lists one call");
    let call = &calls[0];
    assert_eq!(
        (
            call.tool_id(),
            call.side_effect(),
            call.basis_span_id(),
            call.generation_id(),
            call.state()
        ),
        (
            lookup.tool_id(),
            SideEffectClass::ReadOnly,
            question.span_id(),
            generation,
            ToolCallState::Completed
        )
    );
    assert!(
        calls.iter().all(|call| call.tool_id() != remind.tool_id()),
        "the idempotent write was never called"
    );

    // The speech: every committed segment of the generation queued, synthesised and played,
    // the result among them.
    let played = tts_handle.played();
    let texts: Vec<&str> = played.iter().map(|segment| segment.text()).collect();
    let result = code_word().to_string();
    assert_eq!(texts, vec![reply(QUESTION).as_str(), result.as_str()]);
    assert!(texts.iter().any(|text| text.contains(CODE_WORD)));
    for segment in &played {
        assert_eq!(
            (segment.generation_id(), segment.state()),
            (generation, SegmentState::Played)
        );
    }
    let audio_played: Vec<_> = envelopes
        .iter()
        .filter_map(|envelope| match envelope.payload {
            StreamEvent::AudioPlayed(event) => Some(event.segment_id),
            _ => None,
        })
        .collect();
    assert_eq!(
        audio_played,
        played
            .iter()
            .map(|segment| segment.segment_id())
            .collect::<Vec<_>>()
    );
    let frames = speaker_handle
        .received()
        .iter()
        .filter(|envelope| matches!(envelope.payload, Payload::Audio(_)))
        .count();
    let words: usize = texts
        .iter()
        .map(|text| text.split_whitespace().count().max(1))
        .sum();
    assert_eq!(frames, words, "the speaker got every frame synthesised");
}
