//! Recording a run and replaying its control decisions; best-effort telemetry
//! (`story:control-event-replay`; `docs/design/inference.md` 320, 688, 995-1001, 1130, 1161,
//! 1217).
//!
//! The voice cascade with a read-only tool, on the testkit mocks: topology B, its audio entering
//! by an ingress, the tool loop through the authority and the executor, the TTS node playing into
//! a sink. The host speaks a question, the LLM node proposes the read-only call, the authority
//! authorizes it, the executor answers and the LLM node commits its reply and the result. The user
//! then speaks again over the answer: the host interrupts, superseding the generation (a
//! barge-in), and ends the input.
//!
//! The [`Recorder`] wraps every node and the ingress, so it records every envelope on the run's
//! event path with the merge order it assigns, and the host's barge-in among them. A [`Replay`]
//! feeds the recorded inputs and the barge-in into a new run of the same planned pipeline, on mocks
//! scripted as the recorded ones were, and compares the control decisions of the two runs.
//!
//! | case | acceptance |
//! |---|---|
//! | [`replaying_a_recorded_barge_in_and_tool_call_reproduces_its_control_decisions_in_order`] | the replay under the same policy version gets the same decisions in the same order |
//! | [`replaying_under_another_policy_version_reports_the_first_diverging_decision`] | the replay under another version reports the first decision that diverged |
//! | [`a_telemetry_consumer_that_never_reads_delays_no_node_and_its_drops_are_counted`] | the run completes within its deadline and the drop counter is above zero |

use std::collections::HashMap;
use std::future::Future;
use std::num::{NonZeroU16, NonZeroU32, NonZeroUsize};
use std::pin::pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant};

use inference_core::endpoint::OutputEndpoint;
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId, Capability, ModelId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::nodes::turn_policy::{
    CompleteTurnRefused, TurnCommands, TurnEvent, TurnPayload, TurnPolicy,
};
use inference_pipeline::nodes::vad::{
    AudioPayload, SpeechActivity, VadConfig, VoiceActivityDetector,
};
use inference_pipeline::nodes::{ToolAuthority, ToolCallUpdate, ToolExecutorNode, ToolPayload};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeImplementation,
    NodeRole, OverloadPolicy, PipelineSpec, PlannedPipeline, PortId, PortTraits, modality,
};
use inference_runtime::record::{ControlDecision, DecisionKind, Entry, Recorder, Recording};
use inference_runtime::replay::{Replay, ReplayReport};
use inference_runtime::run::RunState;
use inference_runtime::telemetry::{self, TelemetrySink};
use inference_runtime::{ExposedEnds, NodeId, RunController, RunError, RunRequests};
use inference_testkit::graph::{RunLog, TextSink};
use inference_testkit::llm::{
    Carried, MockConfiguration as LlmConfiguration, MockGenerator, ModelHandle, ModelNode,
    ModelPayload,
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
use inference_text::{OpenTurn, TurnCompleted, TurnId, TurnOpened};
use inference_tools::{DeclareTool, Json, SideEffectClass, Tool};
use serde_json::json;

/// How long a run, or a wait of the host or of a replay gate, may take before the case fails
/// rather than hangs; the executor's deadline too.
const PATIENCE: Duration = Duration::from_secs(10);

/// The name of the ingress the microphone's frames enter by.
const AUDIO: &str = "audio";

/// The host action the recorder notes when the user speaks over the answer.
const BARGE_IN: &str = "barge-in";

/// What the user asks: the recogniser's scripted span.
const QUESTION: &str = "what is the code word of the day";

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

/// The three model adapters of one run, scripted alike for every run.
struct Bindings {
    recogniser: MockRecogniser,
    generator: MockGenerator,
    synthesiser: MockSynthesiser,
}

/// The cascade every run of a case is a run of: one declaration, planned once.
struct Cascade {
    declaration: Declaration,
    planned: PlannedPipeline,
    ids: Ids,
    models: [ModelId; 3],
    lookup: Tool,
    remind: Tool,
}

/// The version of the tool authority's policy a run is driven under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Policy {
    /// The authority admits both declared tools and authorizes the read-only call.
    AdmitsTools,
    /// The authority admits no tool and rejects every call `denied`.
    AdmitsNoTool,
}

impl Policy {
    const fn version(self) -> &'static str {
        match self {
            Self::AdmitsTools => "tool authority 1: admits code_word and set_reminder",
            Self::AdmitsNoTool => "tool authority 2: admits no tool",
        }
    }
}

/// The two tools of the run: the read-only lookup of the code word, and an idempotent write the
/// model is offered and never calls.
fn declared_tools() -> (Tool, Tool) {
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
    (tool(lookup), tool(remind))
}

/// The bindings of one run: the recogniser scripted to emit the question and commit it, the LLM
/// scripted to propose the read-only call once, the synthesiser streaming. Each run takes fresh
/// mocks of the same models and scripts, so the adapters answer the replay as they answered the
/// recorded run.
fn mocks(models: [ModelId; 3], lookup: &Tool) -> Bindings {
    let format = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).expect("16000"),
        channels: NonZeroU16::new(1).expect("1"),
    };
    Bindings {
        recogniser: MockRecogniser::new(
            SttConfiguration::Incremental,
            models[0],
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
            models[1],
            Vec::<String>::new(),
        )
        .proposing(lookup.tool_id(), json!({"day": "today"}), 1),
        synthesiser: MockSynthesiser::new(
            TtsConfiguration::IncrementalStreaming,
            models[2],
            format,
        ),
    }
}

/// The cascade, declared with the typed builder, validated and planned: topology B with its tool
/// loop, the microphone's frames entering by the ingress [`AUDIO`] in front of the fan-out.
fn cascade() -> Cascade {
    let (lookup, remind) = declared_tools();
    let models = [
        ModelId::generate(),
        ModelId::generate(),
        ModelId::generate(),
    ];
    let bindings = mocks(models, &lookup);
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: vec![inference_core::model::Locality::SameProcess],
    })
    .expect("a row of the table");
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
        name: "voice cascade recorded and replayed".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let four = NonZeroUsize::new(4).expect("4");
    let wide = NonZeroUsize::new(16).expect("16");
    let mut declaration = Declaration::new();
    let (pipeline_id, ids) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
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

            pipeline.expose(&split_in, AUDIO, wide, OverloadPolicy::Pause);
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
    Cascade {
        declaration,
        planned,
        ids,
        models,
        lookup,
        remind,
    }
}

/// The testkit's in-memory store of `inference.stream`, answering the turn policy's commands.
#[derive(Debug, Clone, Default)]
struct Store(Arc<Mutex<StreamDomain>>);

impl TurnCommands for Store {
    fn open_turn(&mut self, input: OpenTurn) -> Result<TurnOpened, String> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .open_turn(input))
    }

    fn complete_turn(&mut self, turn_id: TurnId) -> Result<TurnCompleted, CompleteTurnRefused> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .complete_turn(turn_id)
            .map_err(|refused| match refused {
                TurnMoveError::Unknown(error) => CompleteTurnRefused::Unknown(error),
                TurnMoveError::Conflict(error) => CompleteTurnRefused::Conflict(error),
            })
    }
}

/// What the read-only tool answers.
fn code_word() -> Json {
    json!({"code_word": "tangerine"})
}

/// The microphone's audio, in three phases: the question, 300 ms of tone and 800 ms of silence;
/// the user speaking over the answer, 100 ms of tone; then 800 ms of silence.
fn audio() -> [Vec<AudioFrame>; 3] {
    let mut frames = synthesize(&[
        Stretch::Tone {
            frames: 30,
            hertz: 440,
            amplitude: 0.5,
        },
        Stretch::Silence { frames: 80 },
        Stretch::Tone {
            frames: 10,
            hertz: 440,
            amplitude: 0.5,
        },
        Stretch::Silence { frames: 80 },
    ]);
    let silence = frames.split_off(120);
    let barge_in = frames.split_off(110);
    [frames, barge_in, silence]
}

/// What the host sends on the ingress: frames of its own stream, numbered from 1, on the audio
/// frame clock, then the end of input.
struct Microphone {
    run_id: RunId,
    producer: NodeId,
    stream_id: StreamId,
    sequence: i64,
    last: i64,
}

impl Microphone {
    fn envelope(&mut self, kind: &str, payload: Payload) -> EventEnvelope<Payload> {
        self.sequence += 1;
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: self.run_id,
            session_id: None,
            stream_id: self.stream_id,
            sequence: self.sequence,
            producer: self.producer,
            port: AUDIO.to_owned(),
            kind: kind.to_owned(),
            generation_id: None,
            timestamp: self.last,
            timebase: Timebase::AudioFrameTime,
            causation_id: None,
            payload,
        }
    }

    fn speak(&mut self, ingress: &dyn OutputEndpoint<Payload>, frames: Vec<AudioFrame>) {
        for frame in frames {
            self.last = i64::try_from(frame.position.get()).expect("within i64");
            let envelope = self.envelope("inference.audio.AudioFrame", Payload::Audio(frame));
            block_on(ingress.send(envelope)).expect("the ingress takes every frame");
        }
    }

    fn stop(&mut self, ingress: &dyn OutputEndpoint<Payload>) {
        let stop = self.envelope(
            ControlKind::Stop.as_str(),
            Payload::Control(ControlKind::Stop),
        );
        block_on(ingress.send(stop)).expect("the ingress takes the end of input");
    }
}

/// One run of the cascade under way: the caller's ends, the LLM node's handle the host
/// interrupts through, and the drive loop on a thread of its own.
struct Running {
    run_id: RunId,
    ends: ExposedEnds<Payload>,
    llm: ModelHandle,
    requests: RunRequests,
    drive: JoinHandle<(RunController<Payload>, Result<(), RunError>)>,
}

/// Starts a run of `cascade` under `policy`, every node wrapped by `recorder`, and drives it.
fn start(cascade: &Cascade, policy: Policy, recorder: &Recorder<Payload>) -> Running {
    let ids = cascade.ids;
    let (lookup, remind) = (cascade.lookup.clone(), cascade.remind.clone());
    let bindings = mocks(cascade.models, &lookup);
    let run_id = RunId::generate();
    let log = RunLog::new();
    let llm = ModelNode::carrying(run_id, bindings.generator, reply, log.clone())
        .with_tools(
            [lookup.clone(), remind.clone()],
            ids.tool_call,
            ids.tool_result,
        )
        .with_turns(ids.llm_turns);
    let admitted = match policy {
        Policy::AdmitsTools => vec![lookup.clone(), remind.clone()],
        Policy::AdmitsNoTool => Vec::new(),
    };
    let authority = ToolAuthority::new(run_id, admitted)
        .bounded_by(&cascade.declaration, &cascade.planned, ids.authority)
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
    let llm_handle = llm.handle();
    let mut implementations: HashMap<NodeId, Box<dyn NodeImplementation<Payload>>> =
        HashMap::from([
            (
                ids.split,
                Box::new(FanOut::new()) as Box<dyn NodeImplementation<Payload>>,
            ),
            (ids.vad, Box::new(VoiceActivityDetector::new(run_id, VAD))),
            (
                ids.policy,
                Box::new(TurnPolicy::new(run_id, Store::default())),
            ),
            (
                ids.stt,
                Box::new(SttNode::new(
                    run_id,
                    bindings.recogniser,
                    ids.stt_ports,
                    log.clone(),
                )),
            ),
            (ids.llm, Box::new(llm)),
            (ids.authority, Box::new(authority)),
            (ids.executor, Box::new(executor)),
            (
                ids.tts,
                Box::new(TtsNode::new(run_id, bindings.synthesiser, log).playing()),
            ),
            (ids.speaker, Box::new(TextSink::<Payload>::default())),
        ]);
    let mut controller = RunController::create_as(
        run_id,
        &cascade.planned,
        &cascade.declaration,
        HashMap::<GenerationId, i64>::new(),
        |node: &Node| {
            let implementation = implementations
                .remove(&node.node_id())
                .expect("every node of the cascade has an implementation");
            recorder.wrap(node, implementation)
        },
    )
    .expect("the cascade is planned and no edge cancels or supersedes");
    controller.validate().expect("a Created run validates");
    block_on(controller.prepare()).expect("every node prepares");
    let ends = controller.exposed();
    block_on(controller.activate()).expect("every node starts");
    let requests = controller.requests();
    let drive = thread::spawn(move || {
        let result = block_on(controller.drive());
        (controller, result)
    });
    Running {
        run_id,
        ends,
        llm: llm_handle,
        requests,
        drive,
    }
}

/// Waits for the drive loop to end within `deadline`, cancelling the run when it does not so
/// that the case fails instead of hanging, and answers the state the run ended in.
fn finish(running: Running, deadline: Instant) -> RunState {
    let Running {
        ends,
        requests,
        drive,
        ..
    } = running;
    drop(ends);
    while Instant::now() < deadline && !drive.is_finished() {
        thread::sleep(Duration::from_millis(5));
    }
    let ended = drive.is_finished();
    if !ended {
        requests.cancel();
    }
    let (controller, result) = drive.join().expect("the drive loop does not panic");
    assert!(ended, "the run ended within its deadline");
    result.expect("the drive loop ends the run");
    controller.run().state()
}

/// The decisions of `decisions` the node `node_id` made of `kind`.
fn count(decisions: &[ControlDecision], node_id: NodeId, kind: DecisionKind) -> usize {
    decisions
        .iter()
        .filter(|decision| decision.node_id == node_id && decision.kind == kind)
        .count()
}

/// Records one run of `cascade` under `policy`: the host asks the question and waits for the LLM
/// node to commit its reply and the tool's result, speaks over the answer and interrupts it,
/// noting the barge-in, waits for the supersede, then lets the silence and the end of input
/// follow. `telemetry` is offered every envelope the recorder takes.
fn record(
    cascade: &Cascade,
    policy: Policy,
    telemetry: Option<Arc<dyn TelemetrySink>>,
) -> (Recording<Payload>, RunState) {
    let deadline = Instant::now() + PATIENCE;
    let mut recorder = Recorder::new(policy.version());
    if let Some(sink) = telemetry {
        recorder = recorder.with_telemetry(sink);
    }
    let mut running = start(cascade, policy, &recorder);
    let ingress = recorder.ingress(
        AUDIO,
        running.ends.ingress(AUDIO).expect("the audio ingress"),
    );
    let mut microphone = Microphone {
        run_id: running.run_id,
        producer: NodeId::generate(),
        stream_id: StreamId::generate(),
        sequence: 0,
        last: 0,
    };
    let llm = cascade.ids.llm;
    let [question, barge_in, silence] = audio();
    microphone.speak(ingress.as_ref(), question);
    assert!(
        recorder.wait_until(PATIENCE, |decisions| {
            count(decisions, llm, DecisionKind::Commit) >= 2
        }),
        "the LLM node committed its reply and the tool's result: {:?}",
        recorder.decisions()
    );
    microphone.speak(ingress.as_ref(), barge_in);
    recorder.host(BARGE_IN);
    assert!(
        running.llm.supersede(),
        "the host interrupts the generation"
    );
    assert!(
        recorder.wait_until(PATIENCE, |decisions| {
            count(decisions, llm, DecisionKind::Supersede) >= 1
        }),
        "the LLM node superseded its generation: {:?}",
        recorder.decisions()
    );
    microphone.speak(ingress.as_ref(), silence);
    microphone.stop(ingress.as_ref());
    drop(ingress);
    let state = finish(running, deadline);
    (recorder.recording(), state)
}

/// Replays `recording` into a new run of `cascade` under `policy`, the barge-in redone through
/// the LLM node's handle.
fn replay(cascade: &Cascade, recording: &Recording<Payload>, policy: Policy) -> ReplayReport {
    let deadline = Instant::now() + PATIENCE * 2;
    let recorder = Recorder::new(policy.version());
    let mut running = start(cascade, policy, &recorder);
    let replay = Replay::new(recording, &recorder);
    let llm = running.llm.clone();
    let fed = replay.feed(
        running.run_id,
        &mut running.ends,
        |action| {
            assert_eq!(action, BARGE_IN, "the one host action recorded");
            let _ = llm.supersede();
        },
        PATIENCE,
    );
    assert_eq!(
        (fed.inputs, fed.refused, fed.missing, fed.gates_timed_out),
        (recording.inputs().count(), 0, 0, 0),
        "every recorded input was fed and taken, and no wait before one timed out"
    );
    let state = finish(running, deadline);
    assert_eq!(state, RunState::Completed, "the replayed run completes");
    replay.report()
}

/// The key a decision is compared by: who made it, on which port, and what it was.
fn keys(decisions: &[ControlDecision]) -> Vec<(NodeId, PortId, DecisionKind, String)> {
    decisions
        .iter()
        .map(|decision| {
            (
                decision.node_id,
                decision.port_id,
                decision.kind,
                decision.event.clone(),
            )
        })
        .collect()
}

/// Each node's decisions of `decisions`, in the order it made them.
fn by_node(
    decisions: &[ControlDecision],
) -> HashMap<NodeId, Vec<(NodeId, PortId, DecisionKind, String)>> {
    let mut nodes: HashMap<NodeId, Vec<_>> = HashMap::new();
    for key in keys(decisions) {
        nodes.entry(key.0).or_default().push(key);
    }
    nodes
}

#[test]
fn replaying_a_recorded_barge_in_and_tool_call_reproduces_its_control_decisions_in_order() {
    let cascade = cascade();
    let ids = cascade.ids;
    let (recording, state) = record(&cascade, Policy::AdmitsTools, None);
    assert_eq!(state, RunState::Completed);
    assert_eq!(recording.policy_version, Policy::AdmitsTools.version());

    // The recording: every envelope in merge order, the inputs and the barge-in among them.
    let orders: Vec<u64> = recording
        .entries
        .iter()
        .map(|entry| entry.merge_order)
        .collect();
    assert!(
        orders.windows(2).all(|pair| pair[0] < pair[1]),
        "the merge order the recorder assigned increases"
    );
    let [question, barge_in, silence] = audio();
    assert_eq!(
        recording.inputs().count(),
        question.len() + barge_in.len() + silence.len() + 2,
        "every frame, the end of input and the barge-in"
    );
    let host: Vec<&str> = recording
        .entries
        .iter()
        .filter_map(|entry| match &entry.entry {
            Entry::Host { action } => Some(action.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(host, [BARGE_IN]);
    for node_id in [ids.split, ids.vad, ids.policy, ids.stt, ids.llm, ids.tts] {
        assert!(
            !recording.emitted_by(node_id).is_empty(),
            "the recorder took what node {node_id} emitted"
        );
    }

    // The decisions: the question and the turns committed, the call authorized, the reply and
    // the result committed, the generation superseded at the barge-in.
    let recorded = recording.decisions();
    assert_eq!(
        count(&recorded, ids.authority, DecisionKind::ToolAuthorized),
        1
    );
    assert_eq!(
        count(&recorded, ids.authority, DecisionKind::ToolRejected),
        0
    );
    assert_eq!(count(&recorded, ids.llm, DecisionKind::Commit), 2);
    assert!(count(&recorded, ids.llm, DecisionKind::Supersede) >= 1);
    assert!(count(&recorded, ids.stt, DecisionKind::Commit) >= 1);
    assert!(count(&recorded, ids.policy, DecisionKind::Commit) >= 2);
    let position = |node_id, kind| {
        recorded
            .iter()
            .position(|decision| decision.node_id == node_id && decision.kind == kind)
            .expect("the decision was recorded")
    };
    assert!(
        position(ids.authority, DecisionKind::ToolAuthorized)
            < position(ids.llm, DecisionKind::Supersede),
        "the call was authorized before the barge-in superseded the generation"
    );

    // The replay, under the same policy version: the same decisions, each node's in the same
    // order, and no divergence.
    let report = replay(&cascade, &recording, Policy::AdmitsTools);
    assert_eq!(report.divergence, None, "{report:#?}");
    assert_eq!(report.recorded_policy, report.replayed_policy);
    assert_eq!(keys(&report.recorded), keys(&recorded));
    assert_eq!(report.replayed.len(), report.recorded.len());
    assert_eq!(by_node(&report.replayed), by_node(&report.recorded));
}

#[test]
fn replaying_under_another_policy_version_reports_the_first_diverging_decision() {
    let cascade = cascade();
    let ids = cascade.ids;
    let (recording, state) = record(&cascade, Policy::AdmitsTools, None);
    assert_eq!(state, RunState::Completed);

    let report = replay(&cascade, &recording, Policy::AdmitsNoTool);
    assert_eq!(report.recorded_policy, Policy::AdmitsTools.version());
    assert_eq!(report.replayed_policy, Policy::AdmitsNoTool.version());
    let divergence = report
        .divergence
        .clone()
        .expect("the replay under another policy version diverges");
    assert_eq!(divergence.node_id, ids.authority, "{divergence:#?}");
    assert_eq!(divergence.index, 0, "the authority's first decision");
    assert_eq!(
        divergence.recorded.as_ref().map(|decision| decision.kind),
        Some(DecisionKind::ToolAuthorized)
    );
    assert_eq!(
        divergence.replayed.as_ref().map(|decision| decision.kind),
        Some(DecisionKind::ToolRejected)
    );

    // Every decision recorded before it, in merge order, the replay reproduced.
    let first = divergence
        .recorded
        .as_ref()
        .expect("a recorded decision")
        .merge_order;
    let before: Vec<_> = report
        .recorded
        .iter()
        .filter(|decision| decision.merge_order < first)
        .cloned()
        .collect();
    assert!(!before.is_empty(), "the question was committed first");
    let replayed = by_node(&report.replayed);
    for (node_id, decisions) in by_node(&before) {
        assert_eq!(
            replayed.get(&node_id).map(|all| &all[..decisions.len()]),
            Some(&decisions[..]),
            "node {node_id} made the same decisions up to the divergence"
        );
    }
}

#[test]
fn a_telemetry_consumer_that_never_reads_delays_no_node_and_its_drops_are_counted() {
    let cascade = cascade();
    let capacity = NonZeroUsize::new(8).expect("8");
    let (sender, receiver) = telemetry::channel(capacity);
    let started = Instant::now();
    let (recording, state) = record(
        &cascade,
        Policy::AdmitsTools,
        Some(Arc::new(sender.clone()) as Arc<dyn TelemetrySink>),
    );
    let took = started.elapsed();
    assert_eq!(state, RunState::Completed, "the run completed");
    assert!(
        took < PATIENCE,
        "the run took {took:?}, within {PATIENCE:?}"
    );

    let offered = recording
        .entries
        .iter()
        .filter(|entry| !matches!(entry.entry, Entry::Host { .. }))
        .count();
    assert!(sender.dropped() > 0, "the drop counter is above zero");
    assert_eq!(
        sender.accepted(),
        capacity.get() as u64,
        "the channel holds what it has room for"
    );
    assert_eq!(
        sender.accepted() + sender.dropped(),
        offered as u64,
        "every envelope on the event path was offered once"
    );
    assert_eq!(receiver.dropped(), sender.dropped());
    drop(receiver);
}
