//! A tool whose executor panics: the tool executor node ends that call as a failed call, the
//! form it gives a tool that returns an error, and serves the next call. The node is driven
//! through its `NodeImplementation`, as the run controller drives it, over endpoints of the
//! test's own.

use std::collections::VecDeque;
use std::future::{self, Future};
use std::pin::pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use inference_core::endpoint::{EndpointFuture, InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::generation::{Generation, StartGeneration};
use inference_core::id::{NodeId, RunId, StreamId};
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
    ToolCall, ToolCallAuthorized, ToolCallId, ToolExecutor, ToolFailureKind,
};
use serde_json::json;

/// How long a case waits for the node before it fails rather than hangs.
const PATIENCE: Duration = Duration::from_secs(5);

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
        name: "tool executor panic".to_owned(),
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

fn authorized(tool: &Tool) -> ToolCall {
    let generation = Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    })
    .0;
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
    let proposed = ToolCall::propose(tool, &generation, &span, json!({}))
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

fn envelope(kind: &str, payload: Payload) -> EventEnvelope<Payload> {
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
        generation_id: None,
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
    envelope(update.event.name(), Payload::Tool(update))
}

fn end_of_input() -> EventEnvelope<Payload> {
    envelope(
        ControlKind::Stop.as_str(),
        Payload::Control(ControlKind::Stop),
    )
}

/// The input edge: every envelope it was given, ready at once, then nothing, never closed.
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

/// The output edge: it keeps the payload of every envelope sent on it.
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<Payload>>>);

impl Recorder {
    fn sent(&self) -> Vec<Payload> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl OutputEndpoint<Payload> for Recorder {
    fn send(
        &self,
        envelope: EventEnvelope<Payload>,
    ) -> EndpointFuture<'_, Result<(), SendError<Payload>>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(envelope.payload);
        Box::pin(future::ready(Ok(())))
    }
}

/// Where the executor of a tool with a defect panics.
#[derive(Clone, Copy)]
enum Defect {
    /// As the tool is called, before it returns its future.
    Called,
    /// As its future is polled.
    Polled,
}

/// An executor whose tool has a defect: its first call panics; every later call answers.
struct Broken {
    defect: Defect,
    invoked: AtomicUsize,
}

impl ToolExecutor for Broken {
    fn execute<'a>(
        &'a self,
        _: &'a ToolCall,
        _: Instant,
        _: &'a CancellationSignal,
    ) -> ExecutionFuture<'a> {
        let first = self.invoked.fetch_add(1, Ordering::SeqCst) == 0;
        match (first, self.defect) {
            (true, Defect::Called) => panic!("the tool has a defect"),
            (true, Defect::Polled) => {
                Box::pin(future::poll_fn(|_| panic!("the tool has a defect")))
            }
            (false, _) => Box::pin(future::ready(Ok(json!({"answer": 42})))),
        }
    }
}

/// What the node sent about `call_id`, as the names of the steps, with the failure of a failed
/// call.
fn steps(sent: &[Payload], call_id: ToolCallId) -> Vec<String> {
    sent.iter()
        .filter_map(|payload| match payload {
            Payload::Tool(update) if update.call.call_id() == call_id => Some(update),
            _ => None,
        })
        .map(|update| match &update.event {
            ToolCallEvent::Started(_) => "started".to_owned(),
            ToolCallEvent::Result(result) => format!("result {}", result.result),
            ToolCallEvent::Failed(failed) => {
                assert_eq!(failed.failure.kind, ToolFailureKind::ExecutionError);
                format!("failed: {}", failed.failure.detail)
            }
            other => format!("{other:?}"),
        })
        .collect()
}

/// Two calls and the end of input over an executor whose first call panics by `defect`: the
/// first call ends failed with an `execution_error` naming the panic, the second is executed
/// and answered, the end of input is passed on, and the node's drain succeeds.
fn the_panicking_call_fails_and_the_next_is_served(defect: Defect) {
    let lookup = tool();
    let (first, second) = (authorized(&lookup), authorized(&lookup));
    let (declaration, node_id) = declared();
    let executor = Arc::new(Broken {
        defect,
        invoked: AtomicUsize::new(0),
    });
    let mut node = ToolExecutorNode::new(
        RunId::generate(),
        Arc::clone(&executor),
        [lookup.tool_id()],
        Duration::from_secs(30),
    );
    let handle = node.handle();
    let output = Recorder::default();
    block_on(NodeImplementation::<Payload>::describe(
        &mut node,
        declaration.node(node_id).expect("declared"),
    ));
    block_on(NodeImplementation::<Payload>::prepare(&mut node)).expect("prepares");
    let supply = Supply(VecDeque::from([
        delivery(&first),
        delivery(&second),
        end_of_input(),
    ]));
    block_on(NodeImplementation::<Payload>::start(
        &mut node,
        NodeEndpoints {
            inputs: vec![NodeInput {
                port_id: PortId::generate(),
                endpoint: Box::new(supply),
            }],
            outputs: vec![NodeOutput {
                port_id: PortId::generate(),
                endpoint: Box::new(output.clone()),
            }],
        },
    ))
    .expect("starts");

    let (told, drained) = mpsc::channel();
    thread::spawn(move || {
        let drained = block_on(NodeImplementation::<Payload>::drain(&mut node));
        let _ = told.send(drained.map_err(|error| error.to_string()));
    });
    let drained = drained
        .recv_timeout(PATIENCE)
        .expect("the node ends at the end of its input");

    let sent = output.sent();
    assert_eq!(drained, Ok(()), "the node sent {sent:#?}");
    let failed = steps(&sent, first.call_id());
    assert_eq!(failed.len(), 2, "{sent:#?}");
    assert_eq!(failed[0], "started");
    assert!(
        failed[1].starts_with("failed: ") && failed[1].contains("the tool has a defect"),
        "the failure names the panic: {failed:?}"
    );
    assert_eq!(
        steps(&sent, second.call_id()),
        ["started", r#"result {"answer":42}"#],
        "{sent:#?}"
    );
    assert_eq!(
        sent.last(),
        Some(&Payload::Control(ControlKind::Stop)),
        "the end of input is passed on"
    );
    assert_eq!(executor.invoked.load(Ordering::SeqCst), 2);
    assert_eq!(handle.failure(), None, "the node's exchange did not fail");
    let failures: Vec<_> = handle
        .measures()
        .iter()
        .map(|measure| measure.failure)
        .collect();
    assert_eq!(failures, [Some(ToolFailureKind::ExecutionError), None]);
}

#[test]
fn a_tool_that_panics_as_it_is_called_fails_its_call_and_the_node_serves_the_next() {
    the_panicking_call_fails_and_the_next_is_served(Defect::Called);
}

#[test]
fn a_tool_whose_future_panics_fails_its_call_and_the_node_serves_the_next() {
    the_panicking_call_fails_and_the_next_is_served(Defect::Polled);
}
