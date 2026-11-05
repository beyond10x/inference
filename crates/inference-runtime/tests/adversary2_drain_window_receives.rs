//! Adversary cases on `story:drain-treats-a-pending-receive-as-an-empty-edge`: while a subgraph
//! node drains, its ingress bridge counts an edge as empty only when the edge's receive answered
//! `Pending` without waking the bridge's task during that poll.
//!
//! Two receives that `InputEndpoint::recv` does not forbid are driven through the drain:
//!
//! - a receive that, on an empty edge still open, wakes its own task on every poll (a spin wait):
//!   does the node's drain still end, and what does the bridge do meanwhile;
//! - a receive that holds an envelope and answers `Pending` until a worker thread of its own has
//!   finished with it and wakes it: does that envelope cross into the child.
//!
//! Every case runs on a thread of its own and bounds the node's drain with a deadline it
//! enforces itself; a drain past its deadline is dropped and the node cancelled, so no case can
//! hang the suite.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

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

/// How long a case may take, all of it, before it counts as hung.
const PATIENCE: Duration = Duration::from_secs(30);

/// How long a case waits for the node's drain before it gives up on it and cancels the node.
const DRAIN_DEADLINE: Duration = Duration::from_secs(4);

/// How long a worker of a [`Kind::Worked`] edge holds its envelope unless the test releases it
/// first: shorter than [`DRAIN_DEADLINE`], so a drain that waits for the receive sees it.
const WORK: Duration = Duration::from_secs(1);

/// How long a case lets the node run before it drains, counting the receives' polls meanwhile:
/// the bridge outside a drain, for comparison with the polls while it drains.
const IDLE: Duration = Duration::from_millis(250);

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

/// Polls `future` until it completes or `deadline` passes; none then.
fn block_on_until<F: Future>(future: F, deadline: Duration) -> Option<F::Output> {
    let waker = waker();
    let mut future = pin!(future);
    let until = Instant::now() + deadline;
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
            return Some(output);
        }
        let now = Instant::now();
        if now >= until {
            return None;
        }
        thread::park_timeout(until - now);
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

fn text(sequence: i64, text: &str) -> EventEnvelope<PipelinePayload> {
    envelope(
        sequence,
        PipelinePayload::Stream(StreamEvent::TranscriptCommitted(TranscriptCommitted {
            span_id: SpanId::generate(),
            text: text.to_owned(),
        })),
    )
}

fn stop(sequence: i64) -> EventEnvelope<PipelinePayload> {
    envelope(sequence, PipelinePayload::Control(ControlKind::Stop))
}

/// How an edge's receive behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Hands over what it holds, and waits for a put while empty.
    Plain,
    /// Hands over what it holds; while empty, wakes its own task and answers `Pending` on every
    /// poll: it waits by being polled again.
    SelfWaking,
    /// Hands a text over only once a worker thread of its own has finished with it: the first
    /// poll that finds a text starts the worker and answers `Pending`; the worker wakes the
    /// receive's latest waker when done. A `stop` it hands over at once.
    Worked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Work {
    Idle,
    Running,
    Done,
}

struct EdgeState {
    queued: VecDeque<EventEnvelope<PipelinePayload>>,
    reader: Option<Waker>,
    polls: u64,
    work: Work,
    released: bool,
}

/// A parent edge into the subgraph node's input port, the test holding its sending side.
struct Edge {
    kind: Kind,
    state: Mutex<EdgeState>,
    changed: Condvar,
}

impl Edge {
    fn new(kind: Kind) -> Arc<Self> {
        Arc::new(Self {
            kind,
            state: Mutex::new(EdgeState {
                queued: VecDeque::new(),
                reader: None,
                polls: 0,
                work: Work::Idle,
                released: false,
            }),
            changed: Condvar::new(),
        })
    }

    fn state(&self) -> MutexGuard<'_, EdgeState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

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

    /// Lets a running worker finish now.
    fn release(&self) {
        self.state().released = true;
        self.changed.notify_all();
    }

    fn left(&self) -> usize {
        self.state().queued.len()
    }

    fn polls(&self) -> u64 {
        self.state().polls
    }

    fn work(self: &Arc<Self>) {
        let edge = Arc::clone(self);
        thread::spawn(move || {
            let (mut state, _) = edge
                .changed
                .wait_timeout_while(edge.state(), WORK, |state| !state.released)
                .unwrap_or_else(PoisonError::into_inner);
            state.work = Work::Done;
            let reader = state.reader.take();
            drop(state);
            if let Some(reader) = reader {
                reader.wake();
            }
        });
    }

    fn poll_recv(
        self: &Arc<Self>,
        cx: &Context<'_>,
    ) -> Poll<Option<EventEnvelope<PipelinePayload>>> {
        let mut state = self.state();
        state.polls += 1;
        let holds_text = state
            .queued
            .front()
            .is_some_and(|front| front.kind != ControlKind::Stop.as_str());
        if self.kind == Kind::Worked && holds_text {
            match state.work {
                Work::Idle => {
                    state.work = Work::Running;
                    state.reader = Some(cx.waker().clone());
                    drop(state);
                    self.work();
                    return Poll::Pending;
                }
                Work::Running => {
                    state.reader = Some(cx.waker().clone());
                    return Poll::Pending;
                }
                Work::Done => state.work = Work::Idle,
            }
        }
        if let Some(envelope) = state.queued.pop_front() {
            return Poll::Ready(Some(envelope));
        }
        state.reader = Some(cx.waker().clone());
        if self.kind == Kind::SelfWaking {
            cx.waker().wake_by_ref();
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

/// What a case came to.
#[derive(Debug)]
struct Ended {
    /// The node's drain, or none when it had not ended by [`DRAIN_DEADLINE`] (the node was then
    /// cancelled).
    drained: Option<Result<(), NodeError>>,
    /// How long the drain ran, until it ended or the deadline.
    drain_ran: Duration,
    /// The receives' polls during [`IDLE`] before the drain, over every edge.
    polls_while_idle: u64,
    /// The receives' polls while the node drained, over every edge.
    polls_while_draining: u64,
    child: Option<RunState>,
    texts: Vec<String>,
    left_on_the_edges: usize,
}

/// Drives one subgraph node over a child of one [`Recorder`] with `edges` as the parent edges
/// into its input port: `put` fills them after the node started, then the node drains, bounded
/// by [`DRAIN_DEADLINE`], and stops.
fn drive(kinds: &[Kind], put: fn(&[Arc<Edge>])) -> Ended {
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
    let edges: Vec<Arc<Edge>> = kinds.iter().map(|kind| Edge::new(*kind)).collect();
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

    put(&edges);
    let polls = |edges: &[Arc<Edge>]| edges.iter().map(|edge| edge.polls()).sum::<u64>();
    let idle = polls(&edges);
    thread::sleep(IDLE);
    let before = polls(&edges);
    let polls_while_idle = before - idle;
    let started = Instant::now();
    let drained = block_on_until(node.drain(), DRAIN_DEADLINE);
    let drain_ran = started.elapsed();
    let polls_while_draining = polls(&edges) - before;
    for edge in &edges {
        edge.release();
    }
    if drained.is_none() {
        block_on(node.cancel());
    }
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
        drain_ran,
        polls_while_idle,
        polls_while_draining,
        child: handle.at_stop().map(|child| child.state),
        texts,
        left_on_the_edges: edges.iter().map(|edge| edge.left()).sum(),
    }
}

fn run(name: &'static str, kinds: &'static [Kind], put: fn(&[Arc<Edge>])) -> Ended {
    let (done, finished) = mpsc::channel();
    let runner = thread::spawn(move || {
        let _ = done.send(drive(kinds, put));
    });
    match finished.recv_timeout(PATIENCE) {
        Ok(ended) => {
            eprintln!("{name}: {ended:?}");
            ended
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => match runner.join() {
            Err(panic) => std::panic::resume_unwind(panic),
            Ok(()) => unreachable!("the runner sends what the case came to"),
        },
        Err(mpsc::RecvTimeoutError::Timeout) => {
            panic!("{name}: the case did not end within {PATIENCE:?}, its cancel included")
        }
    }
}

fn drained_with(ended: &Ended, texts: &[&str]) -> bool {
    matches!(ended.drained, Some(Ok(())))
        && ended.child == Some(RunState::Completed)
        && ended.texts == texts
        && ended.left_on_the_edges == 0
}

/// An empty edge, still open, whose receive waits by waking itself: the parent drains the node
/// once everything it admitted is on the node's edges, so this edge holds nothing more and the
/// drain is to end, as it did before the bridge polled such a receive again.
#[test]
fn the_drain_ends_with_an_empty_open_edge_whose_receive_wakes_itself() {
    let ended = run(
        "self-waking empty edge",
        &[Kind::SelfWaking],
        |_: &[Arc<Edge>]| {},
    );
    assert!(
        drained_with(&ended, &[]),
        "the node's drain did not end within {DRAIN_DEADLINE:?} on an empty, open edge whose \
         receive wakes itself while it waits; the bridge polled it {} times in {:?}, against {} \
         times in {IDLE:?} before the drain: {ended:?}",
        ended.polls_while_draining,
        ended.drain_ran,
        ended.polls_while_idle
    );
}

/// Two edges, one plain carrying a text and its `stop`, one empty and open whose receive wakes
/// itself: the text crosses and the drain ends.
#[test]
fn the_drain_ends_and_the_text_crosses_beside_a_self_waking_empty_edge() {
    let ended = run(
        "plain edge with text beside a self-waking empty edge",
        &[Kind::Plain, Kind::SelfWaking],
        |edges: &[Arc<Edge>]| {
            edges[0].put(text(1, "1"));
            edges[0].put(stop(2));
        },
    );
    assert!(
        drained_with(&ended, &["1"]),
        "beside an empty, open edge whose receive wakes itself, the node's drain did not end \
         within {DRAIN_DEADLINE:?} with the plain edge's text across: {ended:?}"
    );
}

/// A self-waking edge that carries its `stop`, beside a plain edge with a text and its `stop`:
/// the `stop` ends the self-waking edge, so the drain is not held.
#[test]
fn a_stop_on_a_self_waking_edge_lets_the_drain_end() {
    let ended = run(
        "self-waking edge with stop beside a plain edge with text",
        &[Kind::SelfWaking, Kind::Plain],
        |edges: &[Arc<Edge>]| {
            edges[0].put(stop(1));
            edges[1].put(text(1, "1"));
            edges[1].put(stop(2));
        },
    );
    assert!(
        drained_with(&ended, &["1"]),
        "a self-waking edge that carried its stop held the drain or lost the text: {ended:?}"
    );
}

/// A drain held by a self-waking empty edge, given up and followed by a cancel: the cancel ends
/// the bridge, the node stops, and the child run is cancelled, within the case's patience.
#[test]
fn a_cancel_after_a_held_drain_ends_the_node() {
    let ended = run(
        "cancel after a drain held by a self-waking edge",
        &[Kind::SelfWaking],
        |_: &[Arc<Edge>]| {},
    );
    if ended.drained.is_none() {
        assert_eq!(
            ended.child,
            Some(RunState::Cancelled),
            "the cancel after the held drain did not cancel the child run: {ended:?}"
        );
    }
}

/// An edge whose receive holds a text while a worker thread of its own finishes with it, and is
/// woken by that thread: the text was on the edge when the node drained, so it is to cross
/// (the story's acceptance: an envelope an edge's receive holds when the node drains reaches the
/// child).
#[test]
fn a_text_a_receive_holds_until_its_worker_wakes_it_crosses_at_the_drain() {
    let ended = run("worked edge", &[Kind::Worked], |edges: &[Arc<Edge>]| {
        edges[0].put(text(1, "1"));
        edges[0].put(stop(2));
    });
    assert!(
        drained_with(&ended, &["1"]),
        "the text the edge's receive held while its worker ran, woken from the worker's thread \
         after {WORK:?}, did not cross into the child: {ended:?}"
    );
}
