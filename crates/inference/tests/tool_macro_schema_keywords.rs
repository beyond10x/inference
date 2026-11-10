//! The schema schemars writes for each argument type of a macro tool is one the `ToolAuthority`
//! prepares with and decides calls by: an argument the schema admits is authorized, one it
//! refuses is rejected as `SchemaInvalid`.

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
use inference::tools::{Json, RejectionReason, Tool, ToolCall, ToolExecutor};
use inference_testkit::graph::{TextSink, TextSource};
use serde_json::json;

const PATIENCE: Duration = Duration::from_secs(10);

#[inference::tool]
async fn letter(c: char) -> String {
    c.to_string()
}

#[inference::tool]
async fn couple(pair: (u8, String)) -> String {
    pair.1
}

#[inference::tool]
async fn triple(values: [u8; 3]) -> u8 {
    values[0]
}

#[derive(inference::__private::serde::Deserialize, inference::__private::schemars::JsonSchema)]
#[serde(crate = "inference::__private::serde")]
#[schemars(crate = "inference::__private::schemars")]
enum Shape {
    Circle { radius: f64 },
    Square(f64),
    Empty,
}

#[derive(inference::__private::serde::Deserialize, inference::__private::schemars::JsonSchema)]
#[serde(crate = "inference::__private::serde", tag = "kind")]
#[schemars(crate = "inference::__private::schemars")]
enum Command {
    Move { steps: u8 },
    Stop,
}

#[derive(inference::__private::serde::Deserialize, inference::__private::schemars::JsonSchema)]
#[serde(crate = "inference::__private::serde")]
#[schemars(crate = "inference::__private::schemars")]
struct Place {
    city: String,
}

#[inference::tool]
async fn draw(shape: Shape) -> String {
    match shape {
        Shape::Circle { radius } => format!("circle of radius {radius}"),
        Shape::Square(side) => format!("square of side {side}"),
        Shape::Empty => "empty".to_owned(),
    }
}

#[inference::tool]
async fn drive(command: Command) -> String {
    match command {
        Command::Move { steps } => steps.to_string(),
        Command::Stop => "stop".to_owned(),
    }
}

#[inference::tool]
async fn maybe_draw(shape: Option<Shape>) -> String {
    shape.map_or_else(|| "nothing".to_owned(), |_| "something".to_owned())
}

#[inference::tool]
async fn maybe_visit(place: Option<Place>) -> String {
    place.map_or_else(|| "nowhere".to_owned(), |place| place.city)
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

#[test]
fn a_char_is_one_code_point() {
    holds(
        &tool_of(letter),
        &[
            json!({"c": "a"}),
            json!({"c": "\u{e9}"}),
            json!({"c": "\u{1f600}"}),
        ],
        &[json!({"c": ""}), json!({"c": "ab"})],
    );
}

#[test]
fn a_tuple_has_its_arity_and_its_positional_types() {
    holds(
        &tool_of(couple),
        &[json!({"pair": [1, "a"]})],
        &[
            json!({"pair": [1]}),
            json!({"pair": [1, "a", "b"]}),
            json!({"pair": ["a", 1]}),
        ],
    );
}

#[test]
fn a_fixed_array_has_its_length() {
    holds(
        &tool_of(triple),
        &[json!({"values": [1, 2, 3]})],
        &[
            json!({"values": [1, 2]}),
            json!({"values": [1, 2, 3, 4]}),
            json!({"values": [1, 2, 300]}),
        ],
    );
}

#[test]
fn an_externally_tagged_enum_with_data_matches_exactly_one_variant() {
    holds(
        &tool_of(draw),
        &[
            json!({"shape": {"Circle": {"radius": 1.5}}}),
            json!({"shape": {"Square": 2.0}}),
            json!({"shape": "Empty"}),
        ],
        &[
            json!({"shape": {"Triangle": 1}}),
            json!({"shape": {"Circle": {"radius": "big"}}}),
            json!({"shape": "Circle"}),
        ],
    );
}

#[test]
fn an_internally_tagged_enum_is_told_apart_by_its_constant() {
    holds(
        &tool_of(drive),
        &[
            json!({"command": {"kind": "Move", "steps": 3}}),
            json!({"command": {"kind": "Stop"}}),
        ],
        &[
            json!({"command": {"kind": "Fly"}}),
            json!({"command": {"kind": "Move"}}),
            json!({"command": {"kind": "Move", "steps": "far"}}),
        ],
    );
}

#[test]
fn an_option_of_a_data_enum_is_the_enum_or_null() {
    holds(
        &tool_of(maybe_draw),
        &[
            json!({"shape": null}),
            json!({"shape": {"Square": 2.0}}),
            json!({"shape": "Empty"}),
        ],
        &[json!({"shape": {"Triangle": 1}}), json!({"shape": 3})],
    );
}

#[test]
fn an_option_of_a_struct_is_the_struct_or_null() {
    holds(
        &tool_of(maybe_visit),
        &[json!({"place": null}), json!({"place": {"city": "Oslo"}})],
        &[json!({"place": {"town": "Oslo"}}), json!({"place": 3})],
    );
}
