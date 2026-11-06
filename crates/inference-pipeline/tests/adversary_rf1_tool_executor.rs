//! Adversary pass 1 on `story:pipeline-tool-and-node-cancel-findings`: the tool executor node
//! driven through its `NodeImplementation`, over endpoints of the test's own, in a variant the
//! unit's own cases do not build.
//!
//! N17 with the output edge full: the acceptance says a cancel racing a delivered call leaves no
//! Started call without `ToolCallStarted`. The unit's case delivers the call after the cancel;
//! here the cancel comes while the `ToolCallStarted` send waits for room.

use std::collections::VecDeque;
use std::future::{self, Future};
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use inference_core::endpoint::{EndpointFuture, InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::generation::{Generation, StartGeneration};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::nodes::{
    ToolAuthority, ToolCallEvent, ToolCallUpdate, ToolExecutorNode, ToolPayload,
};
use inference_pipeline::{
    Catalog, Declaration, InteractionMode, NodeEndpoints, NodeImplementation, NodeInput,
    NodeOutput, NodeRole, PipelineSpec, PortId,
};
use inference_text::{OpenTurn, Span, Turn};
use inference_tools::{
    AuthorizeToolCall, CancellationSignal, DeclareTool, ExecutionFuture, SideEffectClass, Tool,
    ToolCall, ToolCallAuthorized, ToolCallState, ToolExecutor,
};
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Payload {
    Tool(ToolCallUpdate),
    Control(ControlKind),
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

fn declared() -> (Declaration, NodeId) {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    let catalog = Fixture { profile };
    let spec = PipelineSpec {
        name: "adversary tool executor".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let mut declaration = Declaration::new();
    let (_, node_id) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            pipeline.node("node", NodeRole::Tool, Vec::new()).id()
        })
        .expect("the profile is declared");
    (declaration, node_id)
}

fn tool() -> Tool {
    Tool::declare(DeclareTool {
        name: "lookup".to_owned(),
        description: String::new(),
        schema: json!({"type": "object"}),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0
}

fn generation() -> Generation {
    Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    })
    .0
}

fn authorized(tool: &Tool, generation: &Generation) -> ToolCall {
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
    let proposed = ToolCall::propose(tool, generation, &span, json!({}))
        .expect("an Active generation proposes on a committed span")
        .0;
    proposed
        .authorize(&AuthorizeToolCall {
            call_id: proposed.call_id(),
            confirmation_required: false,
            confirmed: false,
        })
        .expect("authorized")
        .0
}

fn envelope(
    kind: &str,
    generation_id: Option<GenerationId>,
    payload: Payload,
) -> EventEnvelope<Payload> {
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id: RunId::generate(),
        session_id: None,
        stream_id: StreamId::generate(),
        sequence: 1,
        producer: NodeId::generate(),
        port: "out".to_owned(),
        kind: kind.to_owned(),
        generation_id,
        timestamp: 0,
        timebase: Timebase::WallTime,
        causation_id: None,
        payload,
    }
}

fn delivery(call: &ToolCall) -> EventEnvelope<Payload> {
    let update = ToolCallUpdate {
        call: call.clone(),
        event: ToolCallEvent::Authorized(ToolCallAuthorized {
            call_id: call.call_id(),
        }),
    };
    envelope(
        update.event.name(),
        Some(call.generation_id()),
        Payload::Tool(update),
    )
}

fn endpoints(
    input: impl InputEndpoint<Payload> + 'static,
    output: impl OutputEndpoint<Payload> + 'static,
) -> NodeEndpoints<Payload> {
    NodeEndpoints {
        inputs: vec![NodeInput {
            port_id: PortId::generate(),
            endpoint: Box::new(input),
        }],
        outputs: vec![NodeOutput {
            port_id: PortId::generate(),
            endpoint: Box::new(output),
        }],
    }
}

fn ends_within<T: Send + 'static>(
    within: Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (sent, received) = mpsc::channel();
    thread::spawn(move || {
        let _ = sent.send(work());
    });
    received.recv_timeout(within).ok()
}

/// The input edge: every envelope it was given ready at once, then nothing, never closed.
struct Supply(VecDeque<EventEnvelope<Payload>>);

impl InputEndpoint<Payload> for Supply {
    fn recv(&mut self) -> EndpointFuture<'_, Option<EventEnvelope<Payload>>> {
        let next = self.0.pop_front();
        Box::pin(future::poll_fn(move |_| {
            next.clone()
                .map_or(Poll::Pending, |envelope| Poll::Ready(Some(envelope)))
        }))
    }
}

/// The output edge: full for good. It tells the test the kind of each envelope whose send first
/// waits on it, and takes nothing.
struct Full {
    waiting: Sender<String>,
}

impl OutputEndpoint<Payload> for Full {
    fn send(
        &self,
        envelope: EventEnvelope<Payload>,
    ) -> EndpointFuture<'_, Result<(), SendError<Payload>>> {
        let waiting = self.waiting.clone();
        let kind = envelope.kind;
        let mut told = false;
        Box::pin(future::poll_fn(move |_| {
            if !told {
                told = true;
                let _ = waiting.send(kind.clone());
            }
            Poll::Pending
        }))
    }
}

/// An executor whose execution never finishes on its own; it counts its invocations.
#[derive(Default)]
struct Watching {
    invoked: AtomicUsize,
}

impl ToolExecutor for Watching {
    fn execute<'a>(
        &'a self,
        _: &'a ToolCall,
        _: Instant,
        _: &'a CancellationSignal,
    ) -> ExecutionFuture<'a> {
        self.invoked.fetch_add(1, Ordering::SeqCst);
        Box::pin(future::pending())
    }
}

/// N17, the residual the implementor named: the call is delivered before the cancel, its
/// `ToolCallStarted` send waits on a full output edge, and the cancel comes then. The acceptance
/// of the story is that no call is left Started without its `ToolCallStarted`.
#[test]
fn a_cancel_while_tool_call_started_waits_on_a_full_edge_leaves_no_started_call_without_it() {
    let lookup = tool();
    let call = authorized(&lookup, &generation());
    let (declaration, node_id) = declared();
    let authority = ToolAuthority::new(RunId::generate(), [lookup.clone()]).handle();
    let executor = Arc::new(Watching::default());
    let mut node = ToolExecutorNode::new(
        RunId::generate(),
        Arc::clone(&executor),
        [lookup.tool_id()],
        Duration::from_secs(60),
    )
    .answering_to(&authority);
    let (waiting, waits) = mpsc::channel();
    let output = Full { waiting };
    block_on(NodeImplementation::<Payload>::describe(
        &mut node,
        declaration.node(node_id).expect("declared"),
    ));
    block_on(NodeImplementation::<Payload>::prepare(&mut node)).expect("prepares");
    block_on(NodeImplementation::<Payload>::start(
        &mut node,
        endpoints(Supply(VecDeque::from([delivery(&call)])), output),
    ))
    .expect("starts");
    let kind = waits
        .recv_timeout(Duration::from_secs(10))
        .expect("a send waits on the full edge");
    assert_eq!(kind, "inference.tools.ToolCallStarted");
    block_on(NodeImplementation::<Payload>::cancel(&mut node));
    let stopped = ends_within(Duration::from_secs(5), move || {
        block_on(NodeImplementation::<Payload>::stop(&mut node));
    });
    assert!(stopped.is_some(), "the cancelled node stops");

    assert_eq!(
        executor.invoked.load(Ordering::SeqCst),
        0,
        "the executor is not invoked"
    );
    let started: Vec<_> = authority
        .calls()
        .into_iter()
        .filter(|call| call.state() == ToolCallState::Started)
        .collect();
    assert!(
        started.is_empty(),
        "story:pipeline-tool-and-node-cancel-findings acceptance: no Started call without its \
         ToolCallStarted; the full edge took nothing and the record holds {started:?}"
    );
}
