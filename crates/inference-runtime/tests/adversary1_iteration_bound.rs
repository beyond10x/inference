//! Adversary pass 1 on the iteration rule of the tool authority (`story:tool-feedback-loop`):
//! `ToolAuthority::with_iteration_bound` driven with proposals of several generations, which the
//! loop's own cases (one generation per run) never send.
//!
//! * The bound counts per generation: a second generation has a budget of its own, and
//!   interleaving the two changes nothing.
//! * Every proposal it decides counts, a `denied` one too; a proposal of a call it already decided
//!   (`wrong-state`) does not.
//! * The largest bound rejects nothing.
//!
//! Source to authority to executor to sink, as `tests/adversary1_tool_nodes.rs` declares it.

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use inference_core::envelope::ControlKind;
use inference_core::generation::{Generation, StartGeneration};
use inference_core::id::{GenerationId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::nodes::{
    AuthorityOutcome, ToolAuthority, ToolCallEvent, ToolCallUpdate, ToolExecutorNode, ToolPayload,
};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeImplementation,
    NodeRole, OverloadPolicy, PipelineSpec, PlannedPipeline, PortTraits, modality,
};
use inference_runtime::run::RunState;
use inference_runtime::{NodeId, RunController};
use inference_testkit::graph::{TextSink, TextSource};
use inference_testkit::tools::{MockToolExecutor, Script};
use inference_text::{OpenTurn, Span, Turn};
use inference_tools::{
    DeclareTool, Json, RejectionReason, SideEffectClass, Tool, ToolCall, ToolCallProposed, ToolId,
};
use serde_json::json;

const PATIENCE: Duration = Duration::from_secs(10);

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
    authority: NodeId,
    executor: NodeId,
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
        name: "tool calls".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let sixteen = NonZeroUsize::new(16).expect("16");
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
            pipeline.connect(&proposed, &proposals, sixteen, OverloadPolicy::Fail);
            pipeline.connect(&authorized, &calls, sixteen, OverloadPolicy::Fail);
            pipeline.connect(&outcomes, &collected, sixteen, OverloadPolicy::Fail);
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

fn tool(name: &str) -> Tool {
    Tool::declare(DeclareTool {
        name: name.to_owned(),
        schema: json!({"type": "object"}),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0
}

/// A generation that proposes calls, each resting on one committed span.
struct Proposer {
    generation: Generation,
    span: Span,
}

impl Proposer {
    fn new() -> Self {
        let (generation, _) = Generation::start(StartGeneration {
            run_id: RunId::generate(),
            node_id: NodeId::generate(),
            epoch: 1,
        });
        let (turn, _) = Turn::open(OpenTurn {
            run_id: RunId::generate(),
        });
        let (span, _) = Span::emit(
            &turn,
            turn.run_id(),
            StreamId::generate(),
            1,
            "a request".to_owned(),
        )
        .expect("an Open turn admits a span");
        let (span, _) = span.commit().expect("a Hypothesis commits");
        Self { generation, span }
    }

    fn propose(&self, tool: &Tool) -> ToolCall {
        ToolCall::propose(tool, &self.generation, &self.span, json!({}))
            .expect("an Active generation proposes on a committed span")
            .0
    }
}

/// What the authority decided, per proposal in the order sent: `Ok(())` for Authorized,
/// `Err(reason)` for Rejected, and `None` for a wrong-state answer.
type Verdict = Option<Result<(), RejectionReason>>;

/// Runs the pipeline once: the source sends `proposals` in order to an authority knowing `known`
/// and enforcing `bound`, and ends. Answers each decision.
fn decide(known: &[Tool], bound: NonZeroUsize, proposals: &[ToolCall]) -> Vec<Verdict> {
    let (declaration, planned, nodes) = planned();
    let run_id = RunId::generate();
    let held: Vec<_> = proposals
        .iter()
        .map(|call| {
            let update = ToolCallUpdate {
                call: call.clone(),
                event: ToolCallEvent::Proposed(ToolCallProposed {
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
    let source = TextSource::scripted(run_id, held);
    let authority = ToolAuthority::new(run_id, known.iter().cloned()).with_iteration_bound(bound);
    let tool_ids: Vec<ToolId> = known.iter().map(Tool::tool_id).collect();
    let executor = ToolExecutorNode::new(
        run_id,
        Arc::new(MockToolExecutor::new(
            tool_ids
                .iter()
                .map(|tool_id| (*tool_id, Script::Result(Json::Null))),
        )),
        tool_ids.clone(),
        PATIENCE,
    );
    let sink = TextSink::<Payload>::default();
    let (source_handle, authority_handle) = (source.handle(), authority.handle());
    let mut implementations: HashMap<NodeId, Box<dyn NodeImplementation<Payload>>> =
        HashMap::from([
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
    let requests = controller.requests();
    let drive = thread::spawn(move || {
        let result = block_on(controller.drive());
        (controller, result)
    });
    while source_handle.release() {}
    assert!(source_handle.release_end(), "the source admits its end");
    let until = Instant::now() + PATIENCE;
    while !drive.is_finished() && Instant::now() < until {
        thread::sleep(Duration::from_millis(2));
    }
    let finished = drive.is_finished();
    if !finished {
        requests.cancel();
    }
    let (controller, _) = drive.join().expect("the drive loop does not panic");
    assert!(finished, "the run ended within {PATIENCE:?}");
    assert_eq!(controller.run().state(), RunState::Completed);
    let decisions = authority_handle.decisions();
    assert_eq!(
        decisions.len(),
        proposals.len(),
        "every proposal was decided"
    );
    decisions
        .iter()
        .map(|decision| match &decision.outcome {
            AuthorityOutcome::Authorized(_) => Some(Ok(())),
            AuthorityOutcome::Rejected(rejected) => Some(Err(rejected.reason)),
            AuthorityOutcome::WrongState(_) => None,
            AuthorityOutcome::Unconfirmed(_) => panic!("no tool requires confirmation"),
        })
        .collect()
}

const AUTHORIZED: Verdict = Some(Ok(()));
const EXHAUSTED: Verdict = Some(Err(RejectionReason::Exhausted));

#[test]
fn the_bound_counts_per_generation_and_a_second_generation_has_a_budget_of_its_own() {
    let lookup = tool("lookup");
    let (first, second) = (Proposer::new(), Proposer::new());
    let proposals = [
        first.propose(&lookup),
        second.propose(&lookup),
        first.propose(&lookup),
        second.propose(&lookup),
        first.propose(&lookup),
        second.propose(&lookup),
    ];
    assert_eq!(
        decide(&[lookup], NonZeroUsize::new(2).expect("2"), &proposals),
        vec![
            AUTHORIZED, AUTHORIZED, AUTHORIZED, AUTHORIZED, EXHAUSTED, EXHAUSTED
        ]
    );
}

#[test]
fn a_denied_proposal_counts_toward_the_bound_and_a_repeated_one_does_not() {
    let (lookup, unknown) = (tool("lookup"), tool("unknown"));
    let denied = Proposer::new();
    let repeated = Proposer::new();
    let once = repeated.propose(&lookup);
    let proposals = [
        denied.propose(&unknown),
        denied.propose(&lookup),
        denied.propose(&lookup),
        once.clone(),
        once,
        repeated.propose(&lookup),
    ];
    let verdicts = decide(&[lookup], NonZeroUsize::new(2).expect("2"), &proposals);
    assert_eq!(
        verdicts,
        vec![
            Some(Err(RejectionReason::Denied)),
            AUTHORIZED,
            EXHAUSTED,
            AUTHORIZED,
            None,
            AUTHORIZED
        ]
    );
}

#[test]
fn the_largest_bound_rejects_nothing() {
    let lookup = tool("lookup");
    let proposer = Proposer::new();
    let proposals: Vec<ToolCall> = (0..5).map(|_| proposer.propose(&lookup)).collect();
    assert_eq!(
        decide(&[lookup], NonZeroUsize::MAX, &proposals),
        vec![AUTHORIZED; 5]
    );
}
