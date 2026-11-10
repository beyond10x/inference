//! Adversary cases on the schema keywords the tool authority validates: the forms JSON Schema
//! 2020-12 gives `prefixItems` and the length keywords, and the `Option` of a `char` and of a
//! fixed array, whose schemas carry a length keyword beside the type `null`.

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use inference::core::envelope::ControlKind;
use inference::core::generation::{Generation, StartGeneration};
use inference::core::id::{GenerationId, NodeId, RunId, StreamId};
use inference::core::model::{Adapter, AdapterId};
use inference::core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference::pipeline::nodes::{
    AuthorityOutcome, ToolAuthority, ToolCallEvent, ToolCallUpdate, ToolPayload,
};
use inference::pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeImplementation,
    NodeRole, OverloadPolicy, PipelineSpec, PlannedPipeline, PortTraits, modality,
};
use inference::runtime::RunController;
use inference::text::{OpenTurn, Span, Turn};
use inference::tools::{
    DeclareTool, Json, RejectionReason, SideEffectClass, Tool, ToolCall, ToolExecutor,
};
use inference_testkit::graph::{TextSink, TextSource};
use serde_json::json;

const PATIENCE: Duration = Duration::from_secs(10);

#[inference::tool]
async fn maybe_letter(c: Option<char>) -> String {
    c.map_or_else(String::new, |c| c.to_string())
}

#[inference::tool]
async fn maybe_triple(values: Option<[u8; 3]>) -> u8 {
    values.map_or(0, |values| values[0])
}

/// The tool `probe`, declared by hand with the argument schema `schema`.
fn declared(schema: Json) -> Tool {
    Tool::declare(DeclareTool {
        name: "probe".to_owned(),
        description: String::new(),
        schema,
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0
}

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

fn tool_of(tool: impl FnOnce() -> (Tool, Box<dyn ToolExecutor>)) -> Tool {
    tool().0
}

fn prepared(tool: &Tool) -> Result<(), String> {
    let mut authority = ToolAuthority::new(RunId::generate(), [tool.clone()]);
    block_on(NodeImplementation::<Payload>::prepare(&mut authority)).map_err(|e| e.to_string())
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

struct Nodes {
    source: NodeId,
    authority: NodeId,
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
    let structured = PortTraits {
        essentiality: Essentiality::Essential,
        delivery: Delivery::Reliable,
        incremental: false,
        end_marker_required: true,
        provisional_allowed: false,
    };
    let spec = PipelineSpec {
        name: "schema keywords".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let capacity = NonZeroUsize::new(16).expect("16");
    let mut declaration = Declaration::new();
    let (pipeline_id, nodes) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let source = pipeline.node("proposer", NodeRole::Source, Vec::new());
            let authority = pipeline.node("authority", NodeRole::Policy, Vec::new());
            let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
            let proposed = pipeline.output::<modality::Structured>(&source, "proposed", structured);
            let proposals =
                pipeline.input::<modality::Structured>(&authority, "proposed", structured);
            let authorized =
                pipeline.output::<modality::Structured>(&authority, "authorized", structured);
            let collected = pipeline.input::<modality::Structured>(&sink, "authorized", structured);
            pipeline.connect(&proposed, &proposals, capacity, OverloadPolicy::Fail);
            pipeline.connect(&authorized, &collected, capacity, OverloadPolicy::Fail);
            Nodes {
                source: source.id(),
                authority: authority.id(),
                sink: sink.id(),
            }
        })
        .expect("the profile is declared");
    let validated = declaration
        .validate(pipeline_id)
        .expect("source to authority to sink is valid");
    let planned = declaration
        .plan(validated, &catalog)
        .expect("no node requires a capability");
    (declaration, planned, nodes)
}

fn generation() -> Generation {
    Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    })
    .0
}

fn committed() -> Span {
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
    .commit()
    .expect("a Hypothesis commits")
    .0
}

/// What the authority over `tool` decides for a call with each of `arguments`, in order.
fn decided(tool: &Tool, arguments: &[Json]) -> Vec<AuthorityOutcome> {
    let calls: Vec<ToolCall> = arguments
        .iter()
        .map(|arguments| {
            ToolCall::propose(tool, &generation(), &committed(), arguments.clone())
                .expect("an Active generation proposes on a committed span")
                .0
        })
        .collect();
    let (declaration, planned, nodes) = planned();
    let run_id = RunId::generate();
    let proposals: Vec<_> = calls
        .iter()
        .map(|call| {
            let update = ToolCallUpdate {
                call: call.clone(),
                event: ToolCallEvent::Proposed(inference::tools::ToolCallProposed {
                    call_id: call.call_id(),
                    tool_id: call.tool_id(),
                    generation_id: call.generation_id(),
                }),
            };
            (
                update.event.name().to_owned(),
                Some(call.generation_id()),
                Payload::Tool(update),
            )
        })
        .collect();
    let source = TextSource::scripted(run_id, proposals);
    let authority = ToolAuthority::new(run_id, [tool.clone()]);
    let sink = TextSink::<Payload>::default();
    let (source_handle, authority_handle) = (source.handle(), authority.handle());
    let mut implementations: HashMap<_, Box<dyn NodeImplementation<Payload>>> = HashMap::from([
        (
            nodes.source,
            Box::new(source) as Box<dyn NodeImplementation<Payload>>,
        ),
        (nodes.authority, Box::new(authority)),
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
    while source_handle.release() {}
    let until = Instant::now() + PATIENCE;
    while authority_handle.decisions().len() < calls.len() {
        assert!(Instant::now() < until, "the authority decides every call");
        thread::sleep(Duration::from_millis(1));
    }
    assert!(source_handle.release_end(), "the source admits its end");
    let (_, result) = drive.join().expect("the drive loop does not panic");
    result.expect("the drive loop ends the run");
    let decisions = authority_handle.decisions();
    calls
        .iter()
        .map(|call| {
            decisions
                .iter()
                .find(|decision| decision.call.call_id() == call.call_id())
                .expect("the authority decided the call")
                .outcome
                .clone()
        })
        .collect()
}

fn is_admitted(outcome: &AuthorityOutcome) -> bool {
    matches!(outcome, AuthorityOutcome::Authorized(_))
}

fn is_refused(outcome: &AuthorityOutcome) -> bool {
    matches!(
        outcome,
        AuthorityOutcome::Rejected(rejected) if rejected.reason == RejectionReason::SchemaInvalid
    )
}

/// `tool` prepares under the authority, admits every one of `admitted` and refuses every one of
/// `refused`.
fn holds(tool: &Tool, admitted: &[Json], refused: &[Json]) {
    assert_eq!(prepared(tool), Ok(()), "{}", tool.schema());
    let arguments: Vec<Json> = admitted.iter().chain(refused).cloned().collect();
    let outcomes = decided(tool, &arguments);
    for (arguments, outcome) in arguments.iter().zip(&outcomes).take(admitted.len()) {
        assert!(
            is_admitted(outcome),
            "{arguments} is admitted by {}, got {outcome:?}",
            tool.schema()
        );
    }
    for (arguments, outcome) in arguments.iter().zip(&outcomes).skip(admitted.len()) {
        assert!(
            is_refused(outcome),
            "{arguments} is refused by {}, got {outcome:?}",
            tool.schema()
        );
    }
}

/// JSON Schema 2020-12 (validation, `prefixItems`): "The value of `prefixItems` MUST be a
/// non-empty array of valid JSON Schemas." The authority refuses the empty `oneOf` and `anyOf`
/// as malformed and has to refuse the empty `prefixItems` the same way.
#[test]
fn an_empty_prefix_items_is_not_in_json_schemas_form() {
    let tool = declared(json!({
        "type": "object",
        "properties": {"pair": {"type": "array", "prefixItems": []}},
        "required": ["pair"]
    }));
    assert!(
        prepared(&tool).is_err(),
        "the authority prepares with the malformed schema {}",
        tool.schema()
    );
}

/// JSON Schema 2020-12 makes a length keyword's value a non-negative integer, and an integer
/// is any number with a zero fractional part (core, "Instance Data Model"); the official test
/// suite's `minLength`/`maxLength`/`minItems`/`maxItems` "validation with a decimal" cases
/// write `2.0`. The authority already reads `1.0` as an integer for `type`.
#[test]
fn a_length_written_with_a_zero_fraction_is_an_integer() {
    let tool = declared(json!({
        "type": "object",
        "properties": {
            "c": {"type": "string", "minLength": 1.0, "maxLength": 1.0},
            "values": {"type": "array", "minItems": 2.0, "maxItems": 2.0}
        },
        "required": ["c", "values"]
    }));
    holds(
        &tool,
        &[json!({"c": "a", "values": [1, 2]})],
        &[
            json!({"c": "ab", "values": [1, 2]}),
            json!({"c": "a", "values": [1]}),
        ],
    );
}

/// `Option<char>` is `{"type": ["string", "null"], "minLength": 1, "maxLength": 1}`: the length
/// keywords leave a value that is not a string alone, so `null` is admitted.
#[test]
fn an_option_of_a_char_is_one_code_point_or_null() {
    holds(
        &tool_of(maybe_letter),
        &[json!({"c": null}), json!({"c": "\u{1f600}"})],
        &[json!({"c": ""}), json!({"c": "ab"}), json!({"c": 1})],
    );
}

/// `Option<[u8; 3]>` is `{"type": ["array", "null"], "items": ..., "minItems": 3, "maxItems": 3}`:
/// the item counts leave a value that is not an array alone, so `null` is admitted.
#[test]
fn an_option_of_a_fixed_array_is_its_length_or_null() {
    holds(
        &tool_of(maybe_triple),
        &[json!({"values": null}), json!({"values": [1, 2, 3]})],
        &[json!({"values": [1, 2]}), json!({"values": [1, 2, 3, 4]})],
    );
}
