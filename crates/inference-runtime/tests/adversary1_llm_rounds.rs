//! Adversary pass 1 on `story:llm-over-b10x-llm`: the tool loop of the testkit's model node over
//! the wrapper's turn sequencing, in the loop pipeline of `story:tool-feedback-loop` (source to
//! the mock LLM node to sink, `tool_call` through the authority and the executor back to
//! `tool_result`).
//!
//! The node's documentation: "a turn may end with several calls, the calls of one round
//! ([`MockGenerator::proposing_round`]): each is sent, in order, and the generation awaits every
//! one of them". Two red cases:
//!
//! * a round of two answered streams two deltas (one per result) in the next turn, which ends
//!   with the next round. The node emits only the first segment before it asks for proposals, and
//!   `take_proposal` is None while a delta is undelivered, so the second round is never sent and
//!   the generation completes with two calls unanswered;
//! * a reply that is empty gives a turn with no text and a call; the handle completes the
//!   generation in the poll that ends that turn, so the call is never sent.
//!
//! At the base each outcome was a model request of one segment of its own, and the proposal
//! after it was sent.

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant};

use inference_core::envelope::ControlKind;
use inference_core::generation::GenerationState;
use inference_core::id::{GenerationId, RunId};
use inference_core::model::{Adapter, AdapterId, Capability, ModelId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::nodes::{
    AuthorityHandle, ToolAuthority, ToolCallUpdate, ToolExecutorNode, ToolPayload,
};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeImplementation,
    NodeRole, OverloadPolicy, PipelineSpec, PlannedPipeline, PortId, PortTraits, modality,
};
use inference_runtime::run::RunState;
use inference_runtime::{NodeId, RunController, RunError, RunRequests};
use inference_testkit::graph::{RunLog, TextSink, TextSource};
use inference_testkit::llm::{
    Carried, MockConfiguration, MockGenerator, ModelHandle, ModelNode, ModelPayload, ToolProposal,
};
use inference_testkit::stream::StreamEvent;
use inference_testkit::tools::{MockToolExecutor, Script};
use inference_text::{SpanId, TranscriptCommitted};
use inference_tools::{DeclareTool, Json, SideEffectClass, Tool};
use serde_json::json;

const PATIENCE: Duration = Duration::from_secs(10);

const INPUT: &str = "the weather in Berlin and Paris";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Payload {
    Stream(StreamEvent),
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
            other @ (Self::Stream(_) | Self::Control(_)) => Err(other),
        }
    }

    fn from_tool_call(update: ToolCallUpdate) -> Self {
        Self::Tool(update)
    }

    fn control(kind: ControlKind) -> Self {
        Self::Control(kind)
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
        }
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
    adapter: Adapter,
}

impl Catalog for Fixture {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.profile.profile_id() == profile_id).then_some(&self.profile)
    }

    fn adapter(&self, adapter_id: AdapterId) -> Option<&Adapter> {
        (self.adapter.adapter_id() == adapter_id).then_some(&self.adapter)
    }
}

#[derive(Debug, Clone, Copy)]
struct Ids {
    source: NodeId,
    model: NodeId,
    authority: NodeId,
    executor: NodeId,
    sink: NodeId,
    tool_call: PortId,
    tool_result: PortId,
    rejected: PortId,
}

/// The loop pipeline, its feedback edge bounded by eight rounds, validated and planned.
fn planned(generator: &MockGenerator) -> (Declaration, PlannedPipeline, Ids) {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: vec![inference_core::model::Locality::SameProcess],
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    let catalog = Fixture {
        profile,
        adapter: generator.adapter().clone(),
    };
    let structured = PortTraits {
        essentiality: Essentiality::Essential,
        delivery: Delivery::Reliable,
        incremental: false,
        end_marker_required: true,
        provisional_allowed: false,
    };
    let text = PortTraits {
        incremental: true,
        ..structured
    };
    let spec = PipelineSpec {
        name: "text with tool rounds".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: vec![Capability::OutputStreaming],
    };
    let four = NonZeroUsize::new(4).expect("4");
    let eight = NonZeroUsize::new(8).expect("8");
    let adapter_id = generator.adapter().adapter_id();
    let mut declaration = Declaration::new();
    let (pipeline_id, ids) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let source = pipeline.node("source", NodeRole::Source, Vec::new());
            let model = pipeline.node(
                "model",
                NodeRole::Model,
                vec![Capability::OutputStreaming, Capability::ToolCalling],
            );
            let authority = pipeline.node("authority", NodeRole::Policy, Vec::new());
            let executor = pipeline.node("executor", NodeRole::Tool, Vec::new());
            let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
            let emitted = pipeline.output::<modality::Text>(&source, "text", text);
            let prompt = pipeline.input::<modality::Text>(&model, "prompt", text);
            let answer = pipeline.output::<modality::Text>(&model, "answer", text);
            let tool_call =
                pipeline.output::<modality::Structured>(&model, "tool_call", structured);
            let tool_result =
                pipeline.input::<modality::Structured>(&model, "tool_result", structured);
            let proposed =
                pipeline.input::<modality::Structured>(&authority, "proposed", structured);
            let authorized =
                pipeline.output::<modality::Structured>(&authority, "authorized", structured);
            let rejected =
                pipeline.output::<modality::Structured>(&authority, "rejected", structured);
            let calls = pipeline.input::<modality::Structured>(&executor, "authorized", structured);
            let outcomes =
                pipeline.output::<modality::Structured>(&executor, "outcomes", structured);
            let collected = pipeline.input::<modality::Text>(&sink, "text", text);
            pipeline.connect(&emitted, &prompt, four, OverloadPolicy::Pause);
            pipeline.connect(
                &answer,
                &collected,
                NonZeroUsize::new(16).expect("16"),
                OverloadPolicy::Fail,
            );
            pipeline.connect_feedback(&tool_call, &proposed, four, OverloadPolicy::Fail, eight);
            pipeline.connect(&authorized, &calls, four, OverloadPolicy::Fail);
            pipeline.connect(&outcomes, &tool_result, four, OverloadPolicy::Fail);
            pipeline.connect(&rejected, &tool_result, four, OverloadPolicy::Fail);
            pipeline
                .bind(&model, adapter_id)
                .expect("the catalog holds the mock LLM's adapter");
            (
                pipeline.pipeline_id(),
                Ids {
                    source: source.id(),
                    model: model.id(),
                    authority: authority.id(),
                    executor: executor.id(),
                    sink: sink.id(),
                    tool_call: tool_call.id(),
                    tool_result: tool_result.id(),
                    rejected: rejected.id(),
                },
            )
        })
        .expect("the profile is declared")
        .1;
    let validated = declaration
        .validate(pipeline_id)
        .expect("a cycle through a bounded feedback edge is valid");
    let planned = declaration
        .plan(validated, &catalog)
        .expect("the LLM node is bound to an adapter that streams and calls tools");
    (declaration, planned, ids)
}

fn lookup() -> Tool {
    Tool::declare(DeclareTool {
        name: "lookup".to_owned(),
        schema: json!({
            "type": "object",
            "properties": {"city": {"type": "string"}},
            "required": ["city"],
            "additionalProperties": false
        }),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0
}

fn weather() -> Json {
    json!({"temperature": 21})
}

struct Finished {
    state: RunState,
    model: ModelHandle,
    authority: AuthorityHandle,
}

/// Runs [`INPUT`] through the loop, the mock LLM ending each of its first `proposals` turns after
/// the first with every proposal of `round`, the node replying to the input with `reply`.
fn run(
    tool: &Tool,
    round: Vec<ToolProposal>,
    proposals: usize,
    reply: fn(&str) -> String,
) -> Finished {
    let generator = MockGenerator::new(
        MockConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        Vec::<String>::new(),
    )
    .proposing_round(round, proposals);
    let (declaration, planned, ids) = planned(&generator);
    let run_id = RunId::generate();
    let log = RunLog::new();
    let source = TextSource::scripted(
        run_id,
        [(
            "inference.stream.TranscriptCommitted".to_owned(),
            None,
            Payload::Stream(StreamEvent::TranscriptCommitted(TranscriptCommitted {
                span_id: SpanId::generate(),
                text: INPUT.to_owned(),
            })),
        )],
    );
    let model = ModelNode::carrying(run_id, generator, reply, log).with_tools(
        [tool.clone()],
        ids.tool_call,
        ids.tool_result,
    );
    let authority = ToolAuthority::new(run_id, [tool.clone()])
        .bounded_by(&declaration, &planned, ids.authority)
        .expect("the pipeline declares a feedback loop into the authority")
        .rejecting_on(ids.rejected);
    let executor = ToolExecutorNode::new(
        run_id,
        Arc::new(MockToolExecutor::new([(
            tool.tool_id(),
            Script::Result(weather()),
        )])),
        [tool.tool_id()],
        PATIENCE,
    );
    let sink = TextSink::<Payload>::default();
    let (source_handle, model_handle, authority_handle) =
        (source.handle(), model.handle(), authority.handle());
    let mut implementations: HashMap<NodeId, Box<dyn NodeImplementation<Payload>>> =
        HashMap::from([
            (
                ids.source,
                Box::new(source) as Box<dyn NodeImplementation<Payload>>,
            ),
            (ids.model, Box::new(model)),
            (ids.authority, Box::new(authority)),
            (ids.executor, Box::new(executor)),
            (ids.sink, Box::new(sink)),
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
    let drive: JoinHandle<(RunController<Payload>, Result<(), RunError>)> =
        thread::spawn(move || {
            let result = block_on(controller.drive());
            (controller, result)
        });
    assert!(source_handle.release(), "the source holds its input");
    assert!(source_handle.release_end(), "the source admits its end");
    let ended = within(&drive, &requests);
    let (controller, result) = drive.join().expect("the drive loop does not panic");
    assert!(ended, "the run ended within {PATIENCE:?}");
    result.expect("the drive loop ends the run");
    Finished {
        state: controller.run().state(),
        model: model_handle,
        authority: authority_handle,
    }
}

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

fn reply(input: &str) -> String {
    format!("looking up {input}")
}

fn city(tool: &Tool, city: &str) -> ToolProposal {
    ToolProposal {
        tool_id: tool.tool_id(),
        arguments: json!({"city": city}),
    }
}

#[test]
fn every_call_of_a_second_round_of_two_reaches_the_authority() {
    let tool = lookup();
    let finished = run(
        &tool,
        vec![city(&tool, "Berlin"), city(&tool, "Paris")],
        2,
        reply,
    );
    assert_eq!(finished.state, RunState::Completed);
    assert_eq!(
        finished.model.generation_state(),
        Some(GenerationState::Completed)
    );
    let arguments: Vec<Json> = finished
        .authority
        .decisions()
        .into_iter()
        .map(|decision| decision.call.arguments().clone())
        .collect();
    assert_eq!(
        arguments,
        [
            json!({"city": "Berlin"}),
            json!({"city": "Paris"}),
            json!({"city": "Berlin"}),
            json!({"city": "Paris"}),
        ],
        "two rounds of two calls each; model requests {}",
        finished.model.model_requests()
    );
}

#[test]
fn a_call_ending_a_turn_with_no_text_reaches_the_authority() {
    let tool = lookup();
    let finished = run(&tool, vec![city(&tool, "Berlin")], 1, |_| String::new());
    assert_eq!(finished.state, RunState::Completed);
    assert_eq!(
        finished.authority.decisions().len(),
        1,
        "the call of the turn answering the input; model requests {}",
        finished.model.model_requests()
    );
}
