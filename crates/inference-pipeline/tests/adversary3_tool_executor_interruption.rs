//! Final adversary pass on the tool executor node's interruptions, as an executor node that does
//! not answer to a tool authority meets them (`ToolExecutorNode::new` without `answering_to`, the
//! default): the module documentation says that "a call of an interrupted generation the node
//! starts later is started with its signal fired". Every runtime case of the interruption story
//! wires the executor to the authority, which rejects such a call before it starts, so none of
//! them reaches that line.
//!
//! The node is driven through its `NodeImplementation`, on an input and an output of the test's
//! own.

use std::collections::VecDeque;
use std::future::{self, Future};
use std::pin::pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use inference_core::endpoint::{EndpointFuture, InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::generation::{Generation, StartGeneration};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::nodes::{ToolCallEvent, ToolCallUpdate, ToolExecutorNode, ToolPayload};
use inference_pipeline::{
    Catalog, Declaration, InteractionMode, NodeEndpoints, NodeImplementation, NodeInput,
    NodeOutput, NodeRole, PipelineSpec, PortId,
};
use inference_text::{OpenTurn, Span, Turn};
use inference_tools::{
    AuthorizeToolCall, CancellationSignal, DeclareTool, ExecutionFuture, SideEffectClass, Tool,
    ToolCall, ToolCallAuthorized, ToolExecutor, ToolFailure, ToolFailureKind,
};
use serde_json::json;

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
        name: "executor interruption".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let mut declaration = Declaration::new();
    let (_, node_id) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            pipeline.node("executor", NodeRole::Tool, Vec::new()).id()
        })
        .expect("the profile is declared");
    (declaration, node_id)
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

/// An executor recording, for each call, whether its signal was fired when it was first polled,
/// and answering at once: `cancelled` if it was, a result if not.
#[derive(Default)]
struct Observing {
    seen: Mutex<Vec<bool>>,
}

impl ToolExecutor for Observing {
    fn execute<'a>(
        &'a self,
        _: &'a ToolCall,
        _: Instant,
        cancellation: &'a CancellationSignal,
    ) -> ExecutionFuture<'a> {
        let fired = cancellation.is_cancelled();
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(fired);
        Box::pin(future::ready(if fired {
            Err(ToolFailure {
                kind: ToolFailureKind::Cancelled,
                detail: "the call was cancelled".to_owned(),
            })
        } else {
            Ok(json!({"ok": true}))
        }))
    }
}

/// The input edge: the envelopes it was given, then nothing, never closed.
struct Inbox(VecDeque<EventEnvelope<Payload>>);

impl InputEndpoint<Payload> for Inbox {
    fn recv(&mut self) -> EndpointFuture<'_, Option<EventEnvelope<Payload>>> {
        let next = self.0.pop_front();
        Box::pin(future::poll_fn(move |_| {
            next.clone()
                .map_or(Poll::Pending, |envelope| Poll::Ready(Some(envelope)))
        }))
    }
}

/// The output edge: records the kind of every envelope it takes.
struct Recorder(Arc<Mutex<Vec<String>>>);

impl OutputEndpoint<Payload> for Recorder {
    fn send(
        &self,
        envelope: EventEnvelope<Payload>,
    ) -> EndpointFuture<'_, Result<(), SendError<Payload>>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(envelope.kind.clone());
        Box::pin(future::ready(Ok(())))
    }
}

fn envelope(kind: &str, generation_id: GenerationId, payload: Payload) -> EventEnvelope<Payload> {
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id: RunId::generate(),
        session_id: None,
        stream_id: StreamId::generate(),
        sequence: 1,
        producer: NodeId::generate(),
        port: "authorized".to_owned(),
        kind: kind.to_owned(),
        generation_id: Some(generation_id),
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
        call.generation_id(),
        Payload::Tool(update),
    )
}

/// Runs the node on `inbox` until it sent `expected` envelopes: whether each call's signal was
/// fired when its executor first polled it, and the kinds the node sent.
fn run(
    inbox: Vec<EventEnvelope<Payload>>,
    lookup: &Tool,
    expected: usize,
) -> (Vec<bool>, Vec<String>) {
    let (declaration, node_id) = declared();
    let observing = Arc::new(Observing::default());
    let mut node = ToolExecutorNode::new(
        RunId::generate(),
        Arc::clone(&observing),
        [lookup.tool_id()],
        Duration::from_secs(10),
    );
    let sent = Arc::new(Mutex::new(Vec::new()));
    let endpoints = NodeEndpoints {
        inputs: vec![NodeInput {
            port_id: PortId::generate(),
            endpoint: Box::new(Inbox(VecDeque::from(inbox))),
        }],
        outputs: vec![NodeOutput {
            port_id: PortId::generate(),
            endpoint: Box::new(Recorder(Arc::clone(&sent))),
        }],
    };
    let described = declaration.node(node_id).expect("declared");
    block_on(NodeImplementation::<Payload>::describe(
        &mut node, described,
    ));
    block_on(NodeImplementation::<Payload>::prepare(&mut node)).expect("prepares");
    block_on(NodeImplementation::<Payload>::start(&mut node, endpoints)).expect("starts");
    let until = Instant::now() + Duration::from_secs(10);
    while sent.lock().unwrap_or_else(PoisonError::into_inner).len() < expected
        && Instant::now() < until
    {
        thread::sleep(Duration::from_millis(5));
    }
    block_on(NodeImplementation::<Payload>::cancel(&mut node));
    block_on(NodeImplementation::<Payload>::stop(&mut node));
    let seen = observing
        .seen
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let sent = sent.lock().unwrap_or_else(PoisonError::into_inner).clone();
    (seen, sent)
}

fn lookup() -> Tool {
    Tool::declare(DeclareTool {
        name: "lookup".to_owned(),
        schema: json!({"type": "object"}),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0
}

/// A call of a generation an interruption named before the call arrived is started with its
/// signal fired, for either kind of interruption; a call of another generation is not.
#[test]
fn adversary3_a_call_of_an_interrupted_generation_starts_with_its_signal_fired() {
    let lookup = lookup();
    for kind in [
        "inference.stream.GenerationSuperseded",
        "inference.stream.GenerationCancelled",
    ] {
        let (interrupted, other) = (authorized(&lookup), authorized(&lookup));
        let inbox = vec![
            envelope(
                kind,
                interrupted.generation_id(),
                Payload::Control(ControlKind::Cancel),
            ),
            delivery(&other),
            delivery(&interrupted),
        ];
        let (seen, sent) = run(inbox, &lookup, 4);
        assert_eq!(
            seen,
            [false, true],
            "after {kind}: the other generation's call runs unsignalled, the interrupted one's \
             signalled"
        );
        assert_eq!(
            sent,
            [
                "inference.tools.ToolCallStarted",
                "inference.tools.ToolCallResult",
                "inference.tools.ToolCallStarted",
                "inference.tools.ToolCallFailed",
            ],
            "after {kind}"
        );
    }
}
