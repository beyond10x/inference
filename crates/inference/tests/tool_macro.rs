//! `#[inference::tool]` and `#[inference::main]` (`docs/design/sdk.md` §3.2, §5): a macro tool is
//! the `Tool` a hand-written declaration gives, its executor runs the function on a started call,
//! its effect reaches the provisional-input barrier and its `confirm` the tool authority; a macro
//! program's `run` is the seam a test drives with a fake host and a stack of mocks. The `trybuild`
//! cases under `tests/ui/` show what compiles and what is refused.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use inference::core::envelope::{ControlKind, EventEnvelope};
use inference::core::generation::{Generation, StartGeneration};
use inference::core::id::{GenerationId, NodeId, RunId, StreamId};
use inference::core::model::{Adapter, AdapterId};
use inference::core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference::pipeline::nodes::{
    AuthorityOutcome, ToolAuthority, ToolCallEvent, ToolCallUpdate, ToolExecutorNode, ToolPayload,
};
use inference::pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeImplementation,
    NodeRole, OverloadPolicy, PipelineSpec, PlannedPipeline, PortTraits, modality,
};
use inference::runtime::RunController;
use inference::runtime::run::RunState;
use inference::text::{OpenTurn, Span, Turn};
use inference::tools::{
    AuthorizeToolCall, CancellationSignal, ConfirmationRequired, DeclareTool, Json,
    ProposeSpeculativeError, ProvisionalInputBarrier, SideEffectClass, Tool, ToolCall, ToolCallId,
    ToolCallResult, ToolExecutor, ToolFailureKind,
};
use inference_testkit::graph::{TextSink, TextSource};
use serde_json::json;

/// How long a case waits for a run before it fails rather than hangs, and the executor's
/// deadline.
const PATIENCE: Duration = Duration::from_secs(10);

/// The local time, fixed for the test.
#[inference::tool]
async fn local_time() -> String {
    "10:30".to_owned()
}

/// How often [`add`]'s body ran.
static ADDED: AtomicUsize = AtomicUsize::new(0);

/// Adds two whole numbers.
#[inference::tool]
async fn add(a: i64, b: i64) -> i64 {
    ADDED.fetch_add(1, Ordering::SeqCst);
    a + b
}

/// The failure of [`weather`] for a city that does not exist.
#[derive(Debug)]
struct NoSuchCity(String);

impl fmt::Display for NoSuchCity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "there is no city called {}", self.0)
    }
}

/// The weather in a city.
///
/// Fails for a city that does not exist.
#[inference::tool]
async fn weather(city: String) -> Result<String, NoSuchCity> {
    if city == "Atlantis" {
        Err(NoSuchCity(city))
    } else {
        Ok(format!("sunny in {city}"))
    }
}

/// Books a table on a day.
#[inference::tool(effect = "non_idempotent_write")]
async fn book(day: String) -> String {
    format!("booked for {day}")
}

/// Looks up the tables free on a day.
#[inference::tool]
async fn look_up(day: String) -> String {
    format!("free on {day}")
}

/// Saves a note under a name.
#[inference::tool(effect = "idempotent_write")]
async fn save(name: String) -> String {
    format!("saved {name}")
}

/// How often [`transfer`]'s body ran.
static TRANSFERS: AtomicUsize = AtomicUsize::new(0);

/// Sends money to someone.
#[inference::tool(effect = "non_idempotent_write", confirm)]
async fn transfer(to: String) -> String {
    TRANSFERS.fetch_add(1, Ordering::SeqCst);
    format!("sent to {to}")
}

/// What a declared tool is, but its identity.
fn declared(tool: &Tool) -> (&str, &str, &Json, &str, SideEffectClass, bool) {
    (
        tool.name(),
        tool.description(),
        tool.schema(),
        tool.schema_revision(),
        tool.side_effect(),
        tool.requires_confirmation(),
    )
}

fn by_hand(name: &str, description: &str, schema: Json, side_effect: SideEffectClass) -> Tool {
    Tool::declare(DeclareTool {
        name: name.to_owned(),
        description: description.to_owned(),
        schema,
        schema_revision: "1".to_owned(),
        side_effect,
        requires_confirmation: false,
    })
    .0
}

fn pair(tool: impl Into<(Tool, Box<dyn ToolExecutor>)>) -> (Tool, Box<dyn ToolExecutor>) {
    tool.into()
}

#[test]
fn a_tool_without_arguments_is_the_tool_a_hand_written_declaration_gives() {
    let (tool, _) = pair(local_time);
    let expected = by_hand(
        "local_time",
        "The local time, fixed for the test.",
        json!({"type": "object", "properties": {}}),
        SideEffectClass::ReadOnly,
    );
    assert_eq!(declared(&tool), declared(&expected));
}

#[test]
fn a_tool_with_two_typed_arguments_is_the_tool_a_hand_written_declaration_gives() {
    let (tool, _) = pair(add);
    let expected = by_hand(
        "add",
        "Adds two whole numbers.",
        json!({
            "type": "object",
            "properties": {
                "a": {"type": "integer", "format": "int64"},
                "b": {"type": "integer", "format": "int64"}
            },
            "required": ["a", "b"]
        }),
        SideEffectClass::ReadOnly,
    );
    assert_eq!(declared(&tool), declared(&expected));
}

#[test]
fn a_tool_returning_result_is_the_tool_a_hand_written_declaration_gives() {
    let (tool, _) = pair(weather);
    let expected = by_hand(
        "weather",
        "The weather in a city.\n\nFails for a city that does not exist.",
        json!({
            "type": "object",
            "properties": {"city": {"type": "string"}},
            "required": ["city"]
        }),
        SideEffectClass::ReadOnly,
    );
    assert_eq!(declared(&tool), declared(&expected));
}

#[test]
fn effect_sets_the_side_effect_class_and_confirm_requires_confirmation() {
    assert_eq!(
        pair(book).0.side_effect(),
        SideEffectClass::NonIdempotentWrite
    );
    assert_eq!(pair(save).0.side_effect(), SideEffectClass::IdempotentWrite);
    assert_eq!(pair(look_up).0.side_effect(), SideEffectClass::ReadOnly);
    let (confirmed, _) = pair(transfer);
    assert!(confirmed.requires_confirmation());
    assert_eq!(confirmed.side_effect(), SideEffectClass::NonIdempotentWrite);
    assert!(!pair(book).0.requires_confirmation());
}

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Polls `future` on this thread until it completes: the executor commits to no runtime.
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

fn generation() -> Generation {
    Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    })
    .0
}

fn hypothesis() -> Span {
    let (turn, _) = Turn::open(OpenTurn {
        run_id: RunId::generate(),
    });
    Span::emit(
        &turn,
        turn.run_id(),
        StreamId::generate(),
        1,
        "a request".to_owned(),
    )
    .expect("an Open turn admits a span")
    .0
}

fn committed() -> Span {
    hypothesis().commit().expect("a Hypothesis commits").0
}

/// A call of `tool` with `arguments`, proposed on a committed span.
fn proposed(tool: &Tool, arguments: Json) -> ToolCall {
    ToolCall::propose(tool, &generation(), &committed(), arguments)
        .expect("an Active generation proposes on a committed span")
        .0
}

/// A call of `tool` with `arguments`, proposed on a committed span, authorized and started.
fn started(tool: &Tool, arguments: Json) -> ToolCall {
    let call = proposed(tool, arguments);
    let call = call
        .authorize(&AuthorizeToolCall {
            call_id: call.call_id(),
            confirmation_required: false,
            confirmed: false,
        })
        .expect("Proposed authorizes")
        .0;
    call.start().expect("Authorized starts").0
}

fn execute(
    executor: &dyn ToolExecutor,
    call: &ToolCall,
) -> Result<Json, inference::tools::ToolFailure> {
    block_on(executor.execute(call, Instant::now() + PATIENCE, &CancellationSignal::new()))
}

#[test]
fn the_executor_runs_the_function_and_refuses_malformed_arguments_without_running_it() {
    let (tool, executor) = pair(add);
    let call = started(&tool, json!({"a": 2, "b": 3}));
    assert_eq!(execute(executor.as_ref(), &call), Ok(json!(5)));
    assert_eq!(ADDED.load(Ordering::SeqCst), 1, "the body ran once");

    let malformed = started(&tool, json!({"a": "two", "b": 3}));
    let failure = execute(executor.as_ref(), &malformed).expect_err("a string is not an i64");
    assert_eq!(failure.kind, ToolFailureKind::ExecutionError);
    assert!(!failure.detail.is_empty(), "the failure says why");
    assert_eq!(
        ADDED.load(Ordering::SeqCst),
        1,
        "the body did not run on malformed arguments"
    );
}

#[test]
fn a_tool_returning_result_maps_ok_to_its_value_and_err_to_an_execution_error() {
    let (tool, executor) = pair(weather);
    let call = started(&tool, json!({"city": "Oslo"}));
    assert_eq!(
        execute(executor.as_ref(), &call),
        Ok(json!("sunny in Oslo"))
    );

    let call = started(&tool, json!({"city": "Atlantis"}));
    let failure = execute(executor.as_ref(), &call).expect_err("Atlantis does not exist");
    assert_eq!(failure.kind, ToolFailureKind::ExecutionError);
    assert!(
        failure.detail.contains("there is no city called Atlantis"),
        "{}",
        failure.detail
    );
}

#[test]
fn a_tool_without_arguments_runs_on_an_empty_object() {
    let (tool, executor) = pair(local_time);
    let call = started(&tool, json!({}));
    assert_eq!(execute(executor.as_ref(), &call), Ok(json!("10:30")));
}

#[test]
fn a_non_idempotent_write_is_refused_on_provisional_input_and_a_read_only_tool_is_proposed() {
    let (writing, _) = pair(book);
    let refused = ToolCall::propose_speculative_as(
        ToolCallId::generate(),
        &writing,
        &generation(),
        &hypothesis(),
        json!({"day": "monday"}),
    );
    assert!(
        matches!(
            refused,
            Err(ProposeSpeculativeError::SideEffecting(
                ProvisionalInputBarrier {
                    side_effect: SideEffectClass::NonIdempotentWrite
                }
            ))
        ),
        "{refused:?}"
    );

    let (reading, _) = pair(look_up);
    let call_id = ToolCallId::generate();
    let (call, _) = ToolCall::propose_speculative_as(
        call_id,
        &reading,
        &generation(),
        &hypothesis(),
        json!({"day": "monday"}),
    )
    .expect("a read-only tool may act on a hypothesis");
    assert_eq!(call.call_id(), call_id);
}

/// What the edges of the confirmation pipeline carry: a step of a tool call or a control event.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Payload {
    Tool(ToolCallUpdate),
    Control(ControlKind),
}

impl From<ControlKind> for Payload {
    fn from(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

impl ToolPayload for Payload {
    fn into_tool_call(self) -> Result<ToolCallUpdate, Self> {
        match self {
            Self::Tool(update) => Ok(update),
            other @ Self::Control(_) => Err(other),
        }
    }

    fn from_tool_call(update: ToolCallUpdate) -> Self {
        Self::Tool(update)
    }

    fn control(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
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
    source: inference::core::id::NodeId,
    authority: inference::core::id::NodeId,
    executor: inference::core::id::NodeId,
    sink: inference::core::id::NodeId,
}

/// Source to authority to executor to sink, every port Structured and reliable.
fn planned() -> (Declaration, PlannedPipeline, Nodes) {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    let catalog = Fixture { profile };
    let structured = PortTraits {
        essentiality: Essentiality::Essential,
        delivery: Delivery::Reliable,
        incremental: false,
        end_marker_required: true,
        provisional_allowed: false,
    };
    let spec = PipelineSpec {
        name: "macro tool calls".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let capacity = NonZeroUsize::new(4).expect("4");
    let mut declaration = Declaration::new();
    let (pipeline_id, nodes) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let source = pipeline.node("proposer", NodeRole::Source, Vec::new());
            let authority = pipeline.node("authority", NodeRole::Policy, Vec::new());
            let executor = pipeline.node("executor", NodeRole::Tool, Vec::new());
            let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
            let proposed = pipeline.output::<modality::Structured>(&source, "proposed", structured);
            let proposals =
                pipeline.input::<modality::Structured>(&authority, "proposed", structured);
            let authorized =
                pipeline.output::<modality::Structured>(&authority, "authorized", structured);
            let calls = pipeline.input::<modality::Structured>(&executor, "authorized", structured);
            let outcomes =
                pipeline.output::<modality::Structured>(&executor, "outcomes", structured);
            let collected = pipeline.input::<modality::Structured>(&sink, "outcomes", structured);
            pipeline.connect(&proposed, &proposals, capacity, OverloadPolicy::Fail);
            pipeline.connect(&authorized, &calls, capacity, OverloadPolicy::Fail);
            pipeline.connect(&outcomes, &collected, capacity, OverloadPolicy::Fail);
            Nodes {
                source: source.id(),
                authority: authority.id(),
                executor: executor.id(),
                sink: sink.id(),
            }
        })
        .expect("the profile is declared");
    let validated = declaration
        .validate(pipeline_id)
        .expect("source to authority to executor to sink is valid");
    let planned = declaration
        .plan(validated, &catalog)
        .expect("no node requires a capability");
    (declaration, planned, nodes)
}

/// The steps of the call `call_id` the sink received, in order.
fn steps(received: &[EventEnvelope<Payload>], call_id: ToolCallId) -> Vec<ToolCallEvent> {
    received
        .iter()
        .filter_map(|envelope| match &envelope.payload {
            Payload::Tool(update) if update.call.call_id() == call_id => Some(update.event.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_tool_marked_confirm_is_held_by_the_authority_until_the_handle_confirms_it_then_runs() {
    let (tool, executor) = pair(transfer);
    let call = proposed(&tool, json!({"to": "Ada"}));
    let (declaration, planned, nodes) = planned();
    let run_id = RunId::generate();
    let proposal = ToolCallUpdate {
        call: call.clone(),
        event: ToolCallEvent::Proposed(inference::tools::ToolCallProposed {
            call_id: call.call_id(),
            tool_id: call.tool_id(),
            generation_id: call.generation_id(),
        }),
    };
    let source = TextSource::scripted(
        run_id,
        [(
            proposal.event.name().to_owned(),
            Some(call.generation_id()),
            Payload::Tool(proposal),
        )],
    );
    let authority = ToolAuthority::new(run_id, [tool.clone()]);
    let executor = ToolExecutorNode::new(
        run_id,
        Arc::<dyn ToolExecutor>::from(executor),
        [tool.tool_id()],
        PATIENCE,
    );
    let sink = TextSink::<Payload>::default();
    let (source_handle, authority_handle, sink_handle) =
        (source.handle(), authority.handle(), sink.handle());
    let mut implementations: HashMap<_, Box<dyn NodeImplementation<Payload>>> = HashMap::from([
        (
            nodes.source,
            Box::new(source) as Box<dyn NodeImplementation<Payload>>,
        ),
        (nodes.authority, Box::new(authority)),
        (nodes.executor, Box::new(executor)),
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
    .expect("the pipeline is planned");
    controller.validate().expect("a Created run validates");
    block_on(controller.prepare()).expect("every node prepares");
    block_on(controller.activate()).expect("every node starts");
    let drive = thread::spawn(move || {
        let result = block_on(controller.drive());
        (controller, result)
    });

    assert!(source_handle.release(), "the transfer is proposed");
    let until = Instant::now() + PATIENCE;
    while authority_handle.held().is_empty() {
        assert!(Instant::now() < until, "the authority holds the transfer");
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        authority_handle
            .decisions()
            .iter()
            .map(|decision| decision.outcome.clone())
            .collect::<Vec<_>>(),
        [AuthorityOutcome::Unconfirmed(ConfirmationRequired {
            call_id: call.call_id()
        })],
        "the macro tool's confirm reached the authorization"
    );
    assert_eq!(TRANSFERS.load(Ordering::SeqCst), 0, "not run while held");
    assert_eq!(steps(&sink_handle.received(), call.call_id()), []);

    assert!(
        authority_handle.confirm(call.call_id()),
        "the hook confirms"
    );
    assert!(
        sink_handle.wait_received(2, PATIENCE),
        "the confirmed transfer is started and completed"
    );
    while source_handle.release() {}
    assert!(source_handle.release_end(), "the source admits its end");
    let (controller, result) = drive.join().expect("the drive loop does not panic");
    result.expect("the drive loop ends the run");
    assert_eq!(controller.run().state(), RunState::Completed);

    let steps = steps(&sink_handle.received(), call.call_id());
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert_eq!(
        steps[1],
        ToolCallEvent::Result(ToolCallResult {
            call_id: call.call_id(),
            result: json!("sent to Ada"),
        })
    );
    assert_eq!(
        TRANSFERS.load(Ordering::SeqCst),
        1,
        "run once, after the confirmation"
    );
}

#[test]
fn ui() {
    let cases = trybuild::TestCases::new();
    cases.pass("tests/ui/tool_pass_*.rs");
    cases.compile_fail("tests/ui/tool_argument_not_deserialize.rs");
    #[cfg(feature = "audio-device")]
    cases.compile_fail("tests/ui/main_without_stack.rs");
}

#[cfg(all(feature = "testing", feature = "audio-device"))]
mod program {
    use std::num::{NonZeroU16, NonZeroU32};
    use std::sync::atomic::{AtomicBool, Ordering};

    use inference::audio::{AudioEncoding, AudioFormat};
    use inference::audio_device::fake::{FakeDevice, FakeHost};
    use inference::core::model::ModelId;
    use inference::prelude::*;
    use inference_testkit::duplex::mock::MockSpeechModel;
    use inference_testkit::llm::{self, MockGenerator};
    use inference_testkit::stt::{self, MockRecogniser};
    use inference_testkit::tts::{self, MockSynthesiser};

    /// Whether [`app`]'s body ran to its end.
    static RAN: AtomicBool = AtomicBool::new(false);

    /// The devices the test gives the fake host.
    fn devices() -> Vec<FakeDevice> {
        let rate = NonZeroU32::new(16_000).expect("16 kHz");
        vec![
            FakeDevice::input("test microphone", rate, NonZeroU16::MIN, true),
            FakeDevice::output("test speaker", rate, NonZeroU16::MIN, true),
        ]
    }

    /// A stack binding the testkit mock of every role.
    fn mocks() -> Stack {
        let pcm16 = AudioFormat {
            encoding: AudioEncoding::PcmS16le,
            sample_rate: NonZeroU32::new(24_000).expect("24 kHz"),
            channels: NonZeroU16::MIN,
        };
        Stack::new()
            .bind(Binding::stt(MockRecogniser::new(
                stt::MockConfiguration::Incremental,
                ModelId::generate(),
                stt::script(),
            )))
            .bind(Binding::llm(MockGenerator::new(
                llm::MockConfiguration::StreamingOnly,
                ModelId::generate(),
                ["Hello"],
            )))
            .bind(Binding::tts(MockSynthesiser::new(
                tts::MockConfiguration::IncrementalStreaming,
                ModelId::generate(),
                pcm16,
            )))
            .bind(Binding::speech(MockSpeechModel::native(Vec::new())))
            .resolve_with(&Env::new())
            .expect("the mocks resolve")
    }

    mod app {
        use inference::prelude::*;

        #[inference::main]
        async fn main(host: impl AudioHost, stack: Stack) {
            let llm = stack.adapter(Role::Llm).expect("the llm role is bound");
            assert_eq!(llm.adapter().backend(), "inference-testkit mock generator");
            let listed = host.devices().expect("the fake host lists its devices");
            let given: Vec<_> = super::devices()
                .into_iter()
                .map(|device| device.info)
                .collect();
            assert_eq!(listed, given);
            super::RAN.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    mod failing {
        use inference::prelude::*;

        #[inference::main]
        async fn main(host: impl AudioHost, stack: Stack) {
            let _ = (host, stack);
            Err(Error::Closed {
                message: "the script ended".to_owned(),
            })?
        }
    }

    #[tokio::test]
    async fn run_drives_the_body_with_the_fake_host_and_the_stack_it_is_given() {
        app::run(FakeHost::new(devices()), mocks())
            .await
            .expect("the body's asserts held");
        assert!(RAN.load(Ordering::SeqCst), "the body ran to its end");
    }

    #[tokio::test]
    async fn run_returns_the_error_a_question_mark_in_the_body_raises() {
        let error = failing::run(FakeHost::new(devices()), mocks())
            .await
            .expect_err("the body ends in Err(..)?");
        assert_eq!(error.kind(), ErrorKind::Closed);
        assert!(error.to_string().contains("the script ended"), "{error}");
    }
}
