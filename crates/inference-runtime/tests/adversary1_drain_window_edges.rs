//! Adversary cases on the drain window of `story:subgraph-nested-drain-residual-loss`, built on
//! the forced window of `subgraph_drain_window.rs`: two parent edges into one ingress bridge,
//! held on either edge, and an edge whose receive is ready only on a later poll.
//!
//! The window `story:subgraph-nested-drain-residual-loss` found, forced: an ingress bridge of a
//! subgraph node polls its parent edge and finds it empty, and before it reads whether the node
//! drains, the parent puts a text and its `stop` on that edge and drains the node. The parent
//! drains a node only once everything it admitted before is on the node's edges, so the text was
//! in flight when the drain came (`inference.graph.DrainRun`: in-flight events complete) and must
//! cross into the child.
//!
//! No sleep opens the window. The test drives the subgraph node itself and hands it, as the edge
//! into its input port, an [`Edge`] of its own: the first time the bridge's thread polls it and
//! finds it empty, the edge holds that thread inside the poll until the test lets it go. Meanwhile
//! the test puts the text and `stop` on the edge and polls the node's drain once, which tells the
//! bridges the node drains; then it lets the thread go, so the bridge reads the drain after the
//! poll that found its edge empty, as a thread preempted between the two would.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use inference_core::endpoint::{EndpointFuture, InputEndpoint};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeEndpoints, NodeError,
    NodeFuture, NodeImplementation, NodeInput, NodeInstanceId, NodeRole, OverloadPolicy,
    PipelineSpec, PortTraits, modality,
};
use inference_runtime::SubgraphNode;
use inference_runtime::run::RunState;
use inference_testkit::graph::PipelinePayload;
use inference_testkit::stream::StreamEvent;
use inference_text::{SpanId, TranscriptCommitted};

/// How long the case may take before it counts as hung.
const PATIENCE: Duration = Duration::from_secs(10);

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

fn waker() -> Waker {
    Waker::from(Arc::new(Unpark(thread::current())))
}

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = waker();
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
            return output;
        }
        thread::park();
    }
}

struct Profiles(DeploymentProfile);

impl Catalog for Profiles {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.0.profile_id() == profile_id).then_some(&self.0)
    }

    fn adapter(&self, _: AdapterId) -> Option<&Adapter> {
        None
    }
}

const TEXT: PortTraits = PortTraits {
    essentiality: Essentiality::Essential,
    delivery: Delivery::Reliable,
    incremental: true,
    end_marker_required: true,
    provisional_allowed: false,
};

fn spec(name: &str) -> PipelineSpec {
    PipelineSpec {
        name: name.to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    }
}

fn capacity() -> NonZeroUsize {
    NonZeroUsize::new(4).expect("4")
}

fn envelope(sequence: i64, payload: PipelinePayload) -> EventEnvelope<PipelinePayload> {
    let kind = match &payload {
        PipelinePayload::Stream(event) => event.name().to_owned(),
        PipelinePayload::Control(kind) => kind.as_str().to_owned(),
        PipelinePayload::Usage(_) => unreachable!("the parent sends no usage"),
    };
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id: RunId::generate(),
        session_id: None,
        stream_id: StreamId::generate(),
        sequence,
        producer: NodeId::generate(),
        port: "in".to_owned(),
        kind,
        generation_id: None,
        timestamp: 0,
        timebase: Timebase::WallTime,
        causation_id: None,
        payload,
    }
}

/// Whether the edge holds the next reader that finds it empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gate {
    /// The next poll that finds the edge empty is held.
    Armed,
    /// A poll that found the edge empty is held inside the edge until the test opens it.
    Held,
    /// No poll is held any more.
    Open,
}

struct EdgeState {
    queued: VecDeque<EventEnvelope<PipelinePayload>>,
    reader: Option<Waker>,
    gate: Gate,
    yield_once: bool,
    yielded: bool,
}

/// The parent edge into the subgraph node's input port, the test holding its sending side: it
/// queues what the test puts on it, and holds the first poll that finds it empty (see the module
/// documentation).
struct Edge {
    state: Mutex<EdgeState>,
    changed: Condvar,
}

impl Edge {
    fn new(gate: Gate, yield_once: bool) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(EdgeState {
                queued: VecDeque::new(),
                reader: None,
                gate,
                yield_once,
                yielded: false,
            }),
            changed: Condvar::new(),
        })
    }

    fn state(&self) -> MutexGuard<'_, EdgeState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Returns once a poll that found the edge empty is held inside it.
    fn wait_held(&self) {
        let (state, waited) = self
            .changed
            .wait_timeout_while(self.state(), PATIENCE, |state| state.gate != Gate::Held)
            .unwrap_or_else(PoisonError::into_inner);
        drop(state);
        assert!(
            !waited.timed_out(),
            "the ingress bridge did not poll its edge within {PATIENCE:?}"
        );
    }

    /// Puts `envelope` on the edge and wakes its reader.
    fn put(&self, envelope: EventEnvelope<PipelinePayload>) {
        let reader = {
            let mut state = self.state();
            state.queued.push_back(envelope);
            state.reader.take()
        };
        if let Some(reader) = reader {
            reader.wake();
        }
    }

    /// Lets the held poll go on.
    fn open(&self) {
        self.state().gate = Gate::Open;
        self.changed.notify_all();
    }

    /// How many envelopes the edge still holds.
    fn left(&self) -> usize {
        self.state().queued.len()
    }

    fn poll_recv(&self, cx: &Context<'_>) -> Poll<Option<EventEnvelope<PipelinePayload>>> {
        let mut state = self.state();
        if !state.queued.is_empty() && state.yield_once && !state.yielded {
            state.yielded = true;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        if let Some(envelope) = state.queued.pop_front() {
            state.yielded = false;
            return Poll::Ready(Some(envelope));
        }
        state.reader = Some(cx.waker().clone());
        if state.gate == Gate::Armed {
            state.gate = Gate::Held;
            self.changed.notify_all();
            drop(
                self.changed
                    .wait_while(state, |state| state.gate == Gate::Held)
                    .unwrap_or_else(PoisonError::into_inner),
            );
        }
        Poll::Pending
    }
}

/// The subgraph node's receiving end of an [`Edge`].
struct Receiver(Arc<Edge>);

impl InputEndpoint<PipelinePayload> for Receiver {
    fn recv(&mut self) -> EndpointFuture<'_, Option<EventEnvelope<PipelinePayload>>> {
        Box::pin(std::future::poll_fn(|cx| self.0.poll_recv(cx)))
    }
}

type Received = Arc<Mutex<Vec<EventEnvelope<PipelinePayload>>>>;

/// Set once the child node's thread has ended; a task polling it is woken then.
#[derive(Clone, Default)]
struct Done(Arc<Mutex<(bool, Option<Waker>)>>);

impl Done {
    fn set(&self) {
        let waker = {
            let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            state.0 = true;
            state.1.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    fn poll(&self, cx: &Context<'_>) -> Poll<()> {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if state.0 {
            Poll::Ready(())
        } else {
            state.1 = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

/// The child's one node: keeps every envelope its input takes, until `stop` or the input's end;
/// its drain waits for that end.
struct Recorder {
    received: Received,
    done: Done,
}

impl NodeImplementation<PipelinePayload> for Recorder {
    fn describe(&mut self, _: &Node) -> NodeFuture<'_, ()> {
        Box::pin(std::future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn start(
        &mut self,
        mut endpoints: NodeEndpoints<PipelinePayload>,
    ) -> NodeFuture<'_, Result<(), NodeError>> {
        let mut input = endpoints.inputs.remove(0).endpoint;
        let (received, done) = (Arc::clone(&self.received), self.done.clone());
        thread::spawn(move || {
            while let Some(envelope) = block_on(input.recv()) {
                let end = envelope.kind == ControlKind::Stop.as_str();
                received
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(envelope);
                if end {
                    break;
                }
            }
            done.set();
        });
        Box::pin(std::future::ready(Ok(())))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        let done = self.done.clone();
        Box::pin(async move {
            std::future::poll_fn(|cx| done.poll(cx)).await;
            Ok(())
        })
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(std::future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(std::future::ready(()))
    }
}

/// What the case came to: the node's drain, the child run's end state, the texts the child's node
/// took and how many envelopes the parent edge still held.
#[derive(Debug)]
struct Ended {
    drained: Result<(), NodeError>,
    child: Option<RunState>,
    texts: Vec<String>,
    left_on_the_edge: usize,
}

/// Drives one subgraph node over a child of one [`Recorder`] through the window; see the module
/// documentation.
fn through_the_window(shape: Shape) -> Ended {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
    })
    .expect("a row of the table");
    let catalog = Profiles(profile);
    let mut declaration = Declaration::new();
    let (child_id, ()) = declaration
        .build(
            &catalog,
            catalog.0.profile_id(),
            spec("child"),
            |pipeline| {
                let recorder = pipeline.node("recorder", NodeRole::Sink, Vec::new());
                let into = pipeline.input::<modality::Text>(&recorder, "in", TEXT);
                pipeline.expose(&into, "in", capacity(), OverloadPolicy::Pause);
            },
        )
        .expect("the profile is declared");
    let validated = declaration.validate(child_id).expect("the child is valid");
    let child = declaration
        .plan(validated, &catalog)
        .expect("the child plans");
    let (parent_id, (frontend, into)) = declaration
        .build(
            &catalog,
            catalog.0.profile_id(),
            spec("parent"),
            |pipeline| {
                let frontend = pipeline
                    .subgraph("frontend", NodeRole::Sink, child_id)
                    .expect("the child is Planned");
                let into = pipeline.input::<modality::Text>(&frontend, "in", TEXT);
                pipeline.expose(&into, "in", capacity(), OverloadPolicy::Pause);
                (frontend.id(), into.id())
            },
        )
        .expect("the profile is declared");
    let validated = declaration
        .validate(parent_id)
        .expect("the parent is valid");
    let _parent = declaration
        .plan(validated, &catalog)
        .expect("the subgraph node matches the child's exposures");
    let declared = declaration
        .node(frontend)
        .expect("the subgraph node is declared")
        .clone();

    let received = Received::default();
    let child_received = Arc::clone(&received);
    let mut node = SubgraphNode::new(
        RunId::generate(),
        &declaration,
        &child,
        HashMap::<GenerationId, i64>::new(),
        Box::new(move |_, _: &Node| {
            Box::new(Recorder {
                received: Arc::clone(&child_received),
                done: Done::default(),
            }) as Box<dyn NodeImplementation<PipelinePayload>>
        }),
    );
    let handle = node.handle();
    block_on(node.describe(&declared));
    node.described(NodeInstanceId::generate());
    block_on(node.prepare()).expect("the child prepares");
    let edges: Vec<Arc<Edge>> = match shape {
        Shape::TextOnFirstOfTwo => {
            vec![Edge::new(Gate::Open, false), Edge::new(Gate::Armed, false)]
        }
        Shape::TextOnSecondOfTwo => {
            vec![Edge::new(Gate::Armed, false), Edge::new(Gate::Open, false)]
        }
        Shape::RecvReadyOnALaterPoll => vec![Edge::new(Gate::Armed, true)],
    };
    let (held, carrier) = match shape {
        Shape::TextOnFirstOfTwo => (1, 0),
        Shape::TextOnSecondOfTwo => (0, 1),
        Shape::RecvReadyOnALaterPoll => (0, 0),
    };
    block_on(
        node.start(NodeEndpoints {
            inputs: edges
                .iter()
                .map(|edge| NodeInput {
                    port_id: into,
                    endpoint: Box::new(Receiver(Arc::clone(edge)))
                        as Box<dyn InputEndpoint<PipelinePayload>>,
                })
                .collect(),
            outputs: Vec::new(),
        }),
    )
    .expect("the child starts");

    edges[held].wait_held();
    let text = PipelinePayload::Stream(StreamEvent::TranscriptCommitted(TranscriptCommitted {
        span_id: SpanId::generate(),
        text: "1".to_owned(),
    }));
    edges[carrier].put(envelope(1, text));
    edges[carrier].put(envelope(2, PipelinePayload::Control(ControlKind::Stop)));
    for (index, edge) in edges.iter().enumerate() {
        if index != carrier {
            edge.put(envelope(1, PipelinePayload::Control(ControlKind::Stop)));
        }
    }
    let drained = {
        let mut drain = node.drain();
        let first = drain.as_mut().poll(&mut Context::from_waker(&waker()));
        assert!(
            first.is_pending(),
            "the drain waits for the ingress bridge, whose thread the edge holds"
        );
        edges[held].open();
        block_on(drain)
    };
    block_on(node.stop());

    let texts = received
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter_map(|envelope| match &envelope.payload {
            PipelinePayload::Stream(StreamEvent::TranscriptCommitted(committed)) => {
                Some(committed.text.clone())
            }
            _ => None,
        })
        .collect();
    Ended {
        drained,
        child: handle.at_stop().map(|child| child.state),
        texts,
        left_on_the_edge: edges.iter().map(|edge| edge.left()).sum(),
    }
}

/// Where the text is when the drain comes.
#[derive(Debug, Clone, Copy)]
enum Shape {
    /// Two parent edges into one port; the bridge is held in its poll of the second while the
    /// text lands on the first, which it polled empty before.
    TextOnFirstOfTwo,
    /// Two parent edges into one port; the bridge is held in its poll of the first while the
    /// text lands on the second, which it has not polled yet.
    TextOnSecondOfTwo,
    /// One parent edge whose receive answers `Pending` once, waking itself, before it hands over
    /// an envelope it holds: a future ready only on a later poll, which `InputEndpoint::recv`
    /// does not forbid.
    RecvReadyOnALaterPoll,
}

fn run(shape: Shape) -> Ended {
    let (done, finished) = mpsc::channel();
    let runner = thread::spawn(move || {
        let _ = done.send(through_the_window(shape));
    });
    match finished.recv_timeout(PATIENCE) {
        Ok(ended) => ended,
        Err(mpsc::RecvTimeoutError::Disconnected) => match runner.join() {
            Err(panic) => std::panic::resume_unwind(panic),
            Ok(()) => unreachable!("the runner sends what the case came to"),
        },
        Err(mpsc::RecvTimeoutError::Timeout) => {
            panic!("the case {shape:?} did not end within {PATIENCE:?}")
        }
    }
}

fn crosses(shape: Shape) {
    let ended = run(shape);
    assert!(
        ended.drained.is_ok()
            && ended.child == Some(RunState::Completed)
            && ended.texts == ["1"]
            && ended.left_on_the_edge == 0,
        "{shape:?}: the text the parent put on an edge before the drain did not cross into the \
         child: {ended:?}"
    );
}

#[test]
fn text_on_the_first_of_two_edges_crosses_while_the_bridge_is_held_on_the_second() {
    crosses(Shape::TextOnFirstOfTwo);
}

#[test]
fn text_on_the_second_of_two_edges_crosses_while_the_bridge_is_held_on_the_first() {
    crosses(Shape::TextOnSecondOfTwo);
}

#[test]
fn text_on_an_edge_whose_receive_is_ready_only_on_a_later_poll_crosses() {
    crosses(Shape::RecvReadyOnALaterPoll);
}
