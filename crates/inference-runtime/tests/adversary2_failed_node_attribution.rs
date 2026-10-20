//! Final adversary pass on the failed-node path of the run loop: which node a run that failed
//! names.
//!
//! The loop wakes from Active when an edge refused an envelope as closed, drains the node the
//! edge arrives at and, when that drain answers an error, fails that node
//! (`RunController::drive`, `drive.rs`). From Draining it does not: `Signals::consumer_gone`
//! keeps a refusal only while the run is Active, and the run's drain goes upstream first, so the
//! producer whose send was refused, failed by that refusal, answers its drain first and the run
//! fails naming it. The node whose exchange failed, and its failure, are not what the run reports.
//!
//! Each case drives source to `first` executor to `second` executor to sink. The second executor
//! fails on the `ToolCallStarted` the first sends it (it takes authorized calls only), while the
//! first holds the call on a gated executor; the gate then opens and the first sends the result
//! into the closed edge. The cases differ only in whether the source ended its input before the
//! gate opened: the run is Active in one, Draining in the other.

use std::collections::HashMap;
use std::future::{self, Future};
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use inference_core::envelope::ControlKind;
use inference_core::generation::{Generation, StartGeneration};
use inference_core::id::{GenerationId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::nodes::{ToolCallEvent, ToolCallUpdate, ToolExecutorNode, ToolPayload};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeImplementation,
    NodeRole, OverloadPolicy, PipelineSpec, PortTraits, modality,
};
use inference_runtime::{NodeId, RunController, RunError};
use inference_testkit::graph::{TextSink, TextSource};
use inference_text::{OpenTurn, Span, Turn};
use inference_tools::{
    AuthorizeToolCall, CancellationSignal, DeclareTool, ExecutionFuture, SideEffectClass, Tool,
    ToolCall, ToolCallAuthorized, ToolExecutor,
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

/// An executor answering a result once its gate opens.
#[derive(Default)]
struct Gate {
    open: AtomicBool,
    waiting: Mutex<Option<Waker>>,
}

impl Gate {
    fn open(&self) {
        self.open.store(true, Ordering::Release);
        let waker = self
            .waiting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

impl ToolExecutor for Gate {
    fn execute<'a>(
        &'a self,
        _: &'a ToolCall,
        _: Instant,
        _: &'a CancellationSignal,
    ) -> ExecutionFuture<'a> {
        Box::pin(future::poll_fn(move |cx| {
            *self.waiting.lock().unwrap_or_else(PoisonError::into_inner) = Some(cx.waker().clone());
            if self.open.load(Ordering::Acquire) {
                Poll::Ready(Ok(json!({"ok": true})))
            } else {
                Poll::Pending
            }
        }))
    }
}

fn authorized(tool: &Tool) -> ToolCall {
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
    let (proposed, _) = ToolCall::propose(tool, &generation, &span, json!({}))
        .expect("an Active generation proposes on a committed span");
    proposed
        .authorize(&AuthorizeToolCall {
            call_id: proposed.call_id(),
            confirmation_required: false,
            confirmed: false,
        })
        .expect("authorized")
        .0
}

/// Runs the chain; `end_first` releases the source's end of input before the gate opens. The
/// error the drive loop returned, and the second executor's node.
fn failed_run(end_first: bool) -> (Result<(), RunError>, NodeId, NodeId, Option<String>) {
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
        name: "failed node attribution".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let capacity = NonZeroUsize::new(8).expect("8");
    let mut declaration = Declaration::new();
    let (pipeline_id, [source, first, second, sink]) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let source = pipeline.node("source", NodeRole::Source, Vec::new());
            let first = pipeline.node("first", NodeRole::Tool, Vec::new());
            let second = pipeline.node("second", NodeRole::Tool, Vec::new());
            let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
            let out = pipeline.output::<modality::Structured>(&source, "out", structured);
            let first_in = pipeline.input::<modality::Structured>(&first, "in", structured);
            let first_out = pipeline.output::<modality::Structured>(&first, "out", structured);
            let second_in = pipeline.input::<modality::Structured>(&second, "in", structured);
            let second_out = pipeline.output::<modality::Structured>(&second, "out", structured);
            let sink_in = pipeline.input::<modality::Structured>(&sink, "in", structured);
            pipeline.connect(&out, &first_in, capacity, OverloadPolicy::Fail);
            pipeline.connect(&first_out, &second_in, capacity, OverloadPolicy::Fail);
            pipeline.connect(&second_out, &sink_in, capacity, OverloadPolicy::Fail);
            [source.id(), first.id(), second.id(), sink.id()]
        })
        .expect("the profile is declared");
    let validated = declaration.validate(pipeline_id).expect("valid");
    let planned = declaration.plan(validated, &catalog).expect("planned");

    let lookup = Tool::declare(DeclareTool {
        name: "lookup".to_owned(),
        schema: json!({"type": "object"}),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0;
    let call = authorized(&lookup);
    let run_id = RunId::generate();
    let gate = Arc::new(Gate::default());
    let update = ToolCallUpdate {
        call: call.clone(),
        event: ToolCallEvent::Authorized(ToolCallAuthorized {
            call_id: call.call_id(),
        }),
    };
    let text_source = TextSource::scripted(
        run_id,
        [(
            update.event.name().to_owned(),
            Some(call.generation_id()),
            Payload::Tool(update),
        )],
    );
    let source_handle = text_source.handle();
    let first_node = ToolExecutorNode::new(run_id, Arc::clone(&gate), [lookup.tool_id()], PATIENCE);
    let second_node =
        ToolExecutorNode::new(run_id, Arc::clone(&gate), [lookup.tool_id()], PATIENCE);
    let second_handle = second_node.handle();
    let mut implementations: HashMap<NodeId, Box<dyn NodeImplementation<Payload>>> =
        HashMap::from([
            (
                source,
                Box::new(text_source) as Box<dyn NodeImplementation<Payload>>,
            ),
            (first, Box::new(first_node)),
            (second, Box::new(second_node)),
            (sink, Box::new(TextSink::<Payload>::default())),
        ]);
    let mut controller = RunController::create_as(
        run_id,
        &planned,
        &declaration,
        HashMap::<GenerationId, i64>::new(),
        |node: &Node| {
            implementations
                .remove(&node.node_id())
                .expect("an implementation for every node")
        },
    )
    .expect("planned");
    controller.validate().expect("validates");
    block_on(controller.prepare()).expect("prepares");
    block_on(controller.activate()).expect("starts");
    let drive = thread::spawn(move || block_on(controller.drive()));

    assert!(source_handle.release(), "the source sends the call");
    let until = Instant::now() + PATIENCE;
    while second_handle.failure().is_none() && Instant::now() < until {
        thread::sleep(Duration::from_millis(5));
    }
    let second_failure = second_handle.failure();
    if end_first {
        assert!(source_handle.release_end(), "the source ends its input");
        thread::sleep(Duration::from_millis(300));
    }
    gate.open();
    if !end_first {
        thread::sleep(Duration::from_millis(300));
        let _ = source_handle.release_end();
    }
    let result = drive.join().expect("the drive loop does not panic");
    (result, first, second, second_failure)
}

fn failed_node(result: &Result<(), RunError>) -> Option<NodeId> {
    match result {
        Err(RunError::NodeFailed(failure)) => failure.node_id,
        _ => None,
    }
}

/// Active: the run fails naming the second executor, whose exchange failed.
#[test]
fn adversary2_control_active_the_run_names_the_node_whose_exchange_failed() {
    let (result, _first, second, second_failure) = failed_run(false);
    assert!(second_failure.is_some(), "the second executor failed");
    assert_eq!(failed_node(&result), Some(second), "{result:?}");
}

/// Draining: the run fails naming the second executor too, the node whose exchange failed, and
/// not the first, whose send that failure refused.
#[test]
fn adversary2_draining_the_run_names_the_node_whose_exchange_failed() {
    let (result, first, second, second_failure) = failed_run(true);
    assert!(second_failure.is_some(), "the second executor failed");
    assert_eq!(
        failed_node(&result),
        Some(second),
        "the run names node {:?} (the first executor is {first}): {result:?}; the second \
         executor's own failure: {second_failure:?}",
        failed_node(&result)
    );
}
