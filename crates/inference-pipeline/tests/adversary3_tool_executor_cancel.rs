//! Final adversary pass on the tool executor node's cancel: the module documentation promises
//! that, cancelled, the node "abandons [the call] and sends nothing more, the answer its executor
//! gives after the cancel among it", because the check and each poll of a send are made under
//! the lock the cancel is taken under.
//!
//! `ToolExecutorNode::cancel` fires the node's cancellation signal first and only then takes that
//! lock to record the cancel. Between the two, an executor that observes the signal answers, the
//! exchange fails the call and sends its `ToolCallFailed`: the lock is free and says "not
//! cancelled". The case widens that window the only way a test can: the output endpoint holds
//! the node's state lock while it takes the `ToolCallStarted` (a send poll is made under that
//! lock), so the cancel fires the signal and then waits for the lock. The endpoint and the input
//! are the test's own; the node is driven through its `NodeImplementation`, as the run controller
//! drives it.

use std::collections::VecDeque;
use std::future::{self, Future};
use std::pin::pin;
use std::sync::mpsc::{self, Receiver, Sender};
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

/// A declared executor node to describe the node under test with.
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
        name: "executor cancel".to_owned(),
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

/// An executor answering `execution_error` as soon as it sees its call cancelled, as a tool
/// reporting its abort does, and polling itself until then.
struct AnswersOnCancel;

impl ToolExecutor for AnswersOnCancel {
    fn execute<'a>(
        &'a self,
        _: &'a ToolCall,
        _: Instant,
        cancellation: &'a CancellationSignal,
    ) -> ExecutionFuture<'a> {
        Box::pin(future::poll_fn(move |cx| {
            if cancellation.is_cancelled() {
                Poll::Ready(Err(ToolFailure {
                    kind: ToolFailureKind::ExecutionError,
                    detail: "aborted on cancel".to_owned(),
                }))
            } else {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
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

/// The output edge: records the kind of every envelope it takes; the first one it takes only
/// once the test opens its gate, telling the test when it started taking it.
struct Gated {
    taken: Arc<Mutex<Vec<String>>>,
    entered: Mutex<Option<Sender<()>>>,
    gate: Mutex<Option<Receiver<()>>>,
}

impl OutputEndpoint<Payload> for Gated {
    fn send(
        &self,
        envelope: EventEnvelope<Payload>,
    ) -> EndpointFuture<'_, Result<(), SendError<Payload>>> {
        Box::pin(future::poll_fn(move |_| {
            let entered = self
                .entered
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            if let Some(entered) = entered {
                let _ = entered.send(());
                let gate = self
                    .gate
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
                if let Some(gate) = gate {
                    let _ = gate.recv_timeout(Duration::from_secs(10));
                }
            }
            self.taken
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(envelope.kind.clone());
            Poll::Ready(Ok(()))
        }))
    }
}

fn delivery(call: &ToolCall) -> EventEnvelope<Payload> {
    let update = ToolCallUpdate {
        call: call.clone(),
        event: ToolCallEvent::Authorized(ToolCallAuthorized {
            call_id: call.call_id(),
        }),
    };
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id: RunId::generate(),
        session_id: None,
        stream_id: StreamId::generate(),
        sequence: 1,
        producer: NodeId::generate(),
        port: "authorized".to_owned(),
        kind: update.event.name().to_owned(),
        generation_id: Some(call.generation_id()),
        timestamp: 0,
        timebase: Timebase::WallTime,
        causation_id: None,
        payload: Payload::Tool(update),
    }
}

/// One cancel landing while the node takes the `ToolCallStarted`: what the output took.
fn one_cancel(declaration: &Declaration, node_id: NodeId, lookup: &Tool) -> Vec<String> {
    let run_id = RunId::generate();
    let mut node = ToolExecutorNode::new(
        run_id,
        Arc::new(AnswersOnCancel),
        [lookup.tool_id()],
        Duration::from_secs(10),
    );
    let call = authorized(lookup);
    let (entered_tx, entered_rx) = mpsc::channel();
    let (gate_tx, gate_rx) = mpsc::channel();
    let taken = Arc::new(Mutex::new(Vec::new()));
    let endpoints = NodeEndpoints {
        inputs: vec![NodeInput {
            port_id: PortId::generate(),
            endpoint: Box::new(Inbox(VecDeque::from([delivery(&call)]))),
        }],
        outputs: vec![NodeOutput {
            port_id: PortId::generate(),
            endpoint: Box::new(Gated {
                taken: Arc::clone(&taken),
                entered: Mutex::new(Some(entered_tx)),
                gate: Mutex::new(Some(gate_rx)),
            }),
        }],
    };
    let described = declaration.node(node_id).expect("declared");
    block_on(NodeImplementation::<Payload>::describe(
        &mut node, described,
    ));
    block_on(NodeImplementation::<Payload>::prepare(&mut node)).expect("prepares");
    block_on(NodeImplementation::<Payload>::start(&mut node, endpoints)).expect("starts");
    entered_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the node sends the ToolCallStarted");
    let canceller = thread::spawn(move || {
        block_on(NodeImplementation::<Payload>::cancel(&mut node));
        node
    });
    thread::sleep(Duration::from_millis(30));
    gate_tx.send(()).expect("the endpoint waits at the gate");
    let mut node = canceller.join().expect("the cancel does not panic");
    block_on(NodeImplementation::<Payload>::stop(&mut node));
    taken.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Cancelled while it takes the `ToolCallStarted`, the node sends nothing more: not the failure
/// its executor answers to the cancel.
#[test]
fn adversary3_the_answer_to_the_cancel_is_not_sent_after_the_cancel() {
    let lookup = Tool::declare(DeclareTool {
        name: "lookup".to_owned(),
        schema: json!({"type": "object"}),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0;
    let (declaration, node_id) = declared();
    let trials = 100;
    let leaked: Vec<Vec<String>> = (0..trials)
        .map(|_| one_cancel(&declaration, node_id, &lookup))
        .filter(|taken| taken.len() > 1)
        .collect();
    assert!(
        leaked.is_empty(),
        "{} of {trials} cancels let the node send after the cancel, e.g. {:?}",
        leaked.len(),
        leaked.first()
    );
}
