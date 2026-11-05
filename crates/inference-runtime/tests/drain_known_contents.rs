//! `story:drain-treats-a-pending-receive-as-an-empty-edge`: a draining subgraph node's ingress
//! bridge trusts an endpoint's `InputEndpoint::known_contents` answer, and the runtime's queue
//! receiver answers it from its own state.
//!
//! The harness is that of `adversary2_drain_window_receives.rs`, whose receive does not
//! implement the method; here it does, answering from what the edge holds. The scenario of its
//! worker-woken case is repeated with that answer: a receive that holds a text and answers
//! `Pending` until a worker thread of its own wakes it, which the bridge cannot tell from an
//! empty edge without the answer. A self-waking receive that knows its edge is empty ends the
//! drain at once.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use inference_core::endpoint::{EndpointFuture, InputEndpoint, KnownContents, OutputEndpoint};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Edge as DeclaredEdge, Essentiality, InteractionMode, Node,
    NodeEndpoints, NodeError, NodeFuture, NodeImplementation, NodeInput, NodeInstanceId, NodeRole,
    OverloadPolicy, PipelineSpec, PortTraits, modality,
};
use inference_runtime::run::RunState;
use inference_runtime::{EdgeQueue, SubgraphNode};
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

/// The subgraph node's receiving end of an [`Edge`], which knows whether the edge holds
/// anything: what it holds stays on the edge until a poll hands it over, so a receive dropped
/// before that loses nothing.
struct Receiver(Arc<Edge>);

impl InputEndpoint<PipelinePayload> for Receiver {
    fn recv(&mut self) -> EndpointFuture<'_, Option<EventEnvelope<PipelinePayload>>> {
        Box::pin(std::future::poll_fn(|cx| self.0.poll_recv(cx)))
    }

    fn known_contents(&self) -> KnownContents {
        if self.0.left() == 0 {
            KnownContents::Empty
        } else {
            KnownContents::NotEmpty
        }
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

/// A receive that holds a text while a worker thread of its own finishes with it and is woken
/// by that thread, answering that its edge is not empty meanwhile: the text was on the edge when
/// the node drained, so it crosses into the child and the drain ends.
#[test]
fn a_text_a_receive_holds_until_its_worker_wakes_it_crosses_when_it_says_the_edge_holds_it() {
    let ended = run("worked edge", &[Kind::Worked], |edges: &[Arc<Edge>]| {
        edges[0].put(text(1, "1"));
        edges[0].put(stop(2));
    });
    assert!(
        drained_with(&ended, &["1"]),
        "the text the edge's receive held while its worker ran, answering that the edge is not \
         empty, did not cross into the child: {ended:?}"
    );
}

/// The same beside a plain edge carrying a text and its `stop`: both texts cross.
#[test]
fn a_worked_text_and_a_plain_text_both_cross_at_the_drain() {
    let ended = run(
        "worked edge beside a plain edge",
        &[Kind::Worked, Kind::Plain],
        |edges: &[Arc<Edge>]| {
            edges[0].put(text(1, "worked"));
            edges[0].put(stop(2));
            edges[1].put(text(1, "plain"));
            edges[1].put(stop(2));
        },
    );
    let mut texts = ended.texts.clone();
    texts.sort();
    assert!(
        matches!(ended.drained, Some(Ok(())))
            && ended.child == Some(RunState::Completed)
            && texts == ["plain", "worked"]
            && ended.left_on_the_edges == 0,
        "a text a worked edge held or a plain edge's text did not cross: {ended:?}"
    );
}

/// An empty, open edge whose receive wakes itself on every poll but answers that its edge is
/// empty: the drain ends at once, without polling it again.
#[test]
fn a_self_waking_receive_that_knows_its_edge_is_empty_ends_the_drain() {
    let ended = run(
        "self-waking empty edge that knows it",
        &[Kind::SelfWaking],
        |_: &[Arc<Edge>]| {},
    );
    assert!(
        drained_with(&ended, &[]),
        "an empty edge that says so held the drain: {ended:?}"
    );
    assert!(
        ended.drain_ran < WORK,
        "the drain took {:?} on an edge known to be empty, its receive polled {} times while \
         draining against {} times in {IDLE:?} before",
        ended.drain_ran,
        ended.polls_while_draining,
        ended.polls_while_idle
    );
}

/// One declared edge of a two-node text pipeline.
fn declared_edge() -> DeclaredEdge {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
    })
    .expect("a row of the table");
    let catalog = Profiles(profile);
    let mut declaration = Declaration::new();
    declaration
        .build(&catalog, catalog.0.profile_id(), spec("edge"), |pipeline| {
            let source = pipeline.node("source", NodeRole::Source, Vec::new());
            let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
            let out = pipeline.output::<modality::Text>(&source, "out", TEXT);
            let into = pipeline.input::<modality::Text>(&sink, "in", TEXT);
            pipeline.connect(&out, &into, capacity(), OverloadPolicy::Pause);
        })
        .expect("the profile is declared");
    declaration.edges().next().expect("one edge").clone()
}

/// Polls one receive of `receiver` once and drops it: whether it handed an envelope over, or
/// pending.
fn polled_once(receiver: &mut dyn InputEndpoint<PipelinePayload>) -> Poll<bool> {
    receiver
        .recv()
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .map(|received| received.is_some())
}

/// The runtime's queue receiver answers from what the queue holds, open or closed, and a
/// pending receive dropped before it completed took nothing.
#[test]
fn the_queue_receiver_knows_whether_its_edge_holds_anything_open_or_closed() {
    let (sender, mut receiver) = EdgeQueue::new(&declared_edge(), HashMap::new())
        .expect("a supported policy")
        .split();
    assert_eq!(
        receiver.known_contents(),
        KnownContents::Empty,
        "open, empty"
    );
    assert_eq!(polled_once(&mut receiver), Poll::Pending);
    for envelope in [text(1, "1"), stop(2)] {
        assert!(matches!(
            pin!(sender.send(envelope))
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Ok(()))
        ));
    }
    assert_eq!(
        receiver.known_contents(),
        KnownContents::NotEmpty,
        "open, holding, after a dropped pending receive"
    );
    drop(sender);
    assert_eq!(
        receiver.known_contents(),
        KnownContents::NotEmpty,
        "closed, holding"
    );
    assert_eq!(polled_once(&mut receiver), Poll::Ready(true));
    assert_eq!(
        receiver.known_contents(),
        KnownContents::NotEmpty,
        "closed, one left"
    );
    assert_eq!(polled_once(&mut receiver), Poll::Ready(true));
    assert_eq!(
        receiver.known_contents(),
        KnownContents::Empty,
        "closed, empty"
    );
    assert_eq!(polled_once(&mut receiver), Poll::Ready(false));
}
