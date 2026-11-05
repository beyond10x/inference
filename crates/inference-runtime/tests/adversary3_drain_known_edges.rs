//! Adversary cases on `story:drain-treats-a-pending-receive-as-an-empty-edge`, second pass: the
//! ingress bridge holds an edge whose endpoint answers `InputEndpoint::known_contents` as the
//! endpoint itself, makes one receive of it per poll and drops that receive while pending, inside
//! and outside a drain.
//!
//! Driven here: a receive that spins (wakes its own task and answers `Pending`) while it holds a
//! text, with and without an endpoint that says the edge holds it; a known edge beside an
//! unknowing self-waking one at the drain; several known edges kept busy, for order, turn-taking
//! and the receives made per envelope; and known edges left idle, which no receive is to spin.
//!
//! Every case runs on a thread of its own and bounds the node's drain with a deadline it
//! enforces itself; a drain past its deadline is dropped and the node cancelled, so no case can
//! hang the suite.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use inference_core::endpoint::{EndpointFuture, InputEndpoint, KnownContents};
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

/// How long into the drain a [`Kind::Spins`] edge is released: shorter than [`DRAIN_DEADLINE`].
const SPIN: Duration = Duration::from_millis(300);

/// How long a case lets the node run before it drains, counting the receives' polls meanwhile.
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
    /// Holds a text and, until the edge is released, wakes its own task and answers `Pending`
    /// on every poll, as a receive does that waits for a contended lock by yielding; then hands
    /// it over. A `stop` it hands over at once.
    Spins,
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
    recvs: u64,
    work: Work,
    released: bool,
}

/// A parent edge into the subgraph node's input port, the test holding its sending side.
struct Edge {
    kind: Kind,
    /// Whether its endpoint says what the edge holds; it answers `Unknown` otherwise.
    knows: bool,
    state: Mutex<EdgeState>,
    changed: Condvar,
}

impl Edge {
    fn new(kind: Kind, knows: bool) -> Arc<Self> {
        Arc::new(Self {
            kind,
            knows,
            state: Mutex::new(EdgeState {
                queued: VecDeque::new(),
                reader: None,
                polls: 0,
                recvs: 0,
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

    /// Lets a running worker finish, or a spinning receive hand over, now.
    fn release(&self) {
        let reader = {
            let mut state = self.state();
            state.released = true;
            state.reader.take()
        };
        self.changed.notify_all();
        if let Some(reader) = reader {
            reader.wake();
        }
    }

    fn left(&self) -> usize {
        self.state().queued.len()
    }

    fn polls(&self) -> u64 {
        self.state().polls
    }

    fn recvs(&self) -> u64 {
        self.state().recvs
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
        if self.kind == Kind::Spins && holds_text && !state.released {
            state.reader = Some(cx.waker().clone());
            drop(state);
            cx.waker().wake_by_ref();
            return Poll::Pending;
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

/// The subgraph node's receiving end of an [`Edge`]. Its state is the edge's, so a receive
/// dropped before it completed loses nothing.
struct Receiver(Arc<Edge>);

impl InputEndpoint<PipelinePayload> for Receiver {
    fn recv(&mut self) -> EndpointFuture<'_, Option<EventEnvelope<PipelinePayload>>> {
        self.0.state().recvs += 1;
        Box::pin(std::future::poll_fn(|cx| self.0.poll_recv(cx)))
    }

    fn known_contents(&self) -> KnownContents {
        if !self.0.knows {
            KnownContents::Unknown
        } else if self.0.left() == 0 {
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
    /// The receives' polls over the whole case, over every edge.
    polls: u64,
    /// The receives made over the whole case, over every edge: each is one boxed future.
    recvs: u64,
    child: Option<RunState>,
    texts: Vec<String>,
    left_on_the_edges: usize,
}

/// Drives one subgraph node over a child of one [`Recorder`] with `edges` as the parent edges
/// into its input port: `put` fills them after the node started, then the node drains, bounded
/// by [`DRAIN_DEADLINE`], and stops.
fn drive(kinds: &[(Kind, bool)], put: fn(&[Arc<Edge>])) -> Ended {
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
    let edges: Vec<Arc<Edge>> = kinds
        .iter()
        .map(|&(kind, knows)| Edge::new(kind, knows))
        .collect();
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
    let spinning: Vec<Arc<Edge>> = edges
        .iter()
        .filter(|edge| edge.kind == Kind::Spins)
        .cloned()
        .collect();
    thread::spawn(move || {
        thread::sleep(SPIN);
        for edge in &spinning {
            edge.release();
        }
    });
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
        polls: polls(&edges),
        recvs: edges.iter().map(|edge| edge.recvs()).sum(),
        child: handle.at_stop().map(|child| child.state),
        texts,
        left_on_the_edges: edges.iter().map(|edge| edge.left()).sum(),
    }
}

fn run(name: &'static str, kinds: &'static [(Kind, bool)], put: fn(&[Arc<Edge>])) -> Ended {
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

/// The texts `count` puts on each of `edges` edges, named `e<edge>-<n>`, in the order each
/// edge is to hand them over.
fn labelled(edge: usize, n: usize) -> String {
    format!("e{edge}-{n}")
}

/// Whether every edge's texts crossed, all of them, in the order the edge held them.
fn each_edge_in_order(texts: &[String], edges: usize, count: usize) -> bool {
    (0..edges).all(|edge| {
        let prefix = format!("e{edge}-");
        let crossed: Vec<&String> = texts
            .iter()
            .filter(|text| text.starts_with(&prefix))
            .collect();
        let expected: Vec<String> = (0..count).map(|n| labelled(edge, n)).collect();
        crossed.len() == count && crossed.iter().zip(&expected).all(|(a, b)| *a == b)
    })
}

/// The longest run of consecutive texts from one edge within `texts[from..to]`.
fn longest_run(texts: &[String], from: usize, to: usize) -> usize {
    let edge_of = |text: &String| text.split('-').next().map(str::to_owned);
    let window = &texts[from.min(texts.len())..to.min(texts.len())];
    let mut longest = 0;
    let mut run = 0;
    let mut last = None;
    for text in window {
        let edge = edge_of(text);
        run = if edge == last { run + 1 } else { 1 };
        last = edge;
        longest = longest.max(run);
    }
    longest
}

/// A receive that spins while it holds a text, its endpoint saying the edge holds it, until the
/// edge is released [`SPIN`] into the drain: the bridge waits for it and the text crosses.
#[test]
fn a_text_a_spinning_receive_holds_crosses_at_the_drain_when_its_endpoint_says_so() {
    let ended = run(
        "spinning holder, endpoint knows",
        &[(Kind::Spins, true)],
        |edges: &[Arc<Edge>]| {
            edges[0].put(text(1, "1"));
            edges[0].put(stop(2));
        },
    );
    assert!(
        drained_with(&ended, &["1"]),
        "a text a spinning receive held at the drain, its endpoint answering `NotEmpty`, did not \
         cross: {ended:?}"
    );
}

/// The same receive, its endpoint not saying what the edge holds: the bridge polls it again at
/// most `DRAIN_REPOLLS` more times on the drain's read, then ends for the drain with the text on
/// the edge. This pins the documented bound, not the story's acceptance.
#[test]
fn a_text_a_spinning_receive_holds_stays_on_the_edge_past_the_bound_when_its_endpoint_does_not_know()
 {
    let ended = run(
        "spinning holder, endpoint does not know",
        &[(Kind::Spins, false)],
        |edges: &[Arc<Edge>]| {
            edges[0].put(text(1, "1"));
            edges[0].put(stop(2));
        },
    );
    assert!(
        matches!(ended.drained, Some(Ok(())))
            && ended.texts.is_empty()
            && ended.left_on_the_edges == 2
            && ended.drain_ran < SPIN,
        "story:drain-treats-a-pending-receive-as-an-empty-edge: an endpoint that answers \
         `Unknown` and whose receive keeps waking itself while it holds a text is documented to \
         get a bounded number of polls on the drain's read, then the text stays on the edge; \
         the drain ran {:?} with {} polls, against {} polls in {IDLE:?} before it: {ended:?}",
        ended.drain_ran,
        ended.polls_while_draining,
        ended.polls_while_idle
    );
}

/// A worked edge whose endpoint says it holds a text, beside an empty, open edge whose
/// endpoint does not know and whose receive wakes itself: the bridge waits for the text, which
/// crosses, and the drain then ends within the bound.
#[test]
fn a_known_holding_edge_beside_an_unknowing_self_waking_edge_crosses_and_the_drain_ends() {
    let ended = run(
        "worked known edge beside a self-waking unknowing edge",
        &[(Kind::Worked, true), (Kind::SelfWaking, false)],
        |edges: &[Arc<Edge>]| {
            edges[0].put(text(1, "1"));
            edges[0].put(stop(2));
        },
    );
    assert!(
        drained_with(&ended, &["1"]),
        "beside a self-waking edge that does not know, the known edge's held text did not cross \
         or the drain did not end: {ended:?}"
    );
}

const EDGES: usize = 3;
const BUSY: usize = 40;

/// Fills every edge with [`BUSY`] texts and a `stop` while their receives hold back, then lets
/// them hand over.
fn busy(edges: &[Arc<Edge>]) {
    for (index, edge) in edges.iter().enumerate() {
        for n in 0..BUSY {
            edge.put(text(i64::try_from(n).expect("small"), &labelled(index, n)));
        }
        edge.put(stop(i64::try_from(BUSY).expect("small")));
    }
    for edge in edges {
        edge.release();
    }
}

/// Three busy edges, outside a drain: each edge's texts cross complete and in order, the edges
/// take turns, and the receives the bridge makes per envelope stay few, whether the endpoints
/// say what their edges hold or not.
#[test]
fn busy_edges_take_turns_and_keep_their_order_known_or_not() {
    for (name, kinds) in [
        (
            "busy known edges",
            &[
                (Kind::Spins, true),
                (Kind::Spins, true),
                (Kind::Spins, true),
            ],
        ),
        (
            "busy unknowing edges",
            &[
                (Kind::Spins, false),
                (Kind::Spins, false),
                (Kind::Spins, false),
            ],
        ),
    ] {
        let ended = run(name, kinds, busy);
        let envelopes = u64::try_from(EDGES * (BUSY + 1)).expect("small");
        eprintln!(
            "{name}: {} receives and {} polls for {envelopes} envelopes; longest run {}",
            ended.recvs,
            ended.polls,
            longest_run(&ended.texts, 10, 100)
        );
        assert!(
            matches!(ended.drained, Some(Ok(())))
                && ended.left_on_the_edges == 0
                && each_edge_in_order(&ended.texts, EDGES, BUSY),
            "{name}: an edge's texts did not all cross in order: {ended:?}"
        );
        assert!(
            longest_run(&ended.texts, 10, 100) <= 4,
            "{name}: one edge crossed more than 4 texts in a row while the others held texts: \
             {:?}",
            ended.texts
        );
    }
}

static IDLE_KNOWN_POLLS: AtomicU64 = AtomicU64::new(u64::MAX);
const SPARSE: usize = 10;

/// Leaves the edges empty for a while, counting the receives' polls meanwhile, then puts
/// [`SPARSE`] texts on each, one at a time and a millisecond apart, and a `stop` on each.
fn idle_then_sparse(edges: &[Arc<Edge>]) {
    let polls = || edges.iter().map(|edge| edge.polls()).sum::<u64>();
    thread::sleep(Duration::from_millis(50));
    let settled = polls();
    thread::sleep(Duration::from_millis(200));
    IDLE_KNOWN_POLLS.store(polls() - settled, Ordering::SeqCst);
    for n in 0..SPARSE {
        for (index, edge) in edges.iter().enumerate() {
            edge.put(text(i64::try_from(n).expect("small"), &labelled(index, n)));
            thread::sleep(Duration::from_millis(1));
        }
    }
    for edge in edges {
        edge.put(stop(i64::try_from(SPARSE).expect("small")));
    }
}

/// Three known edges left empty, then fed sparsely: no receive is polled while they are idle,
/// every text crosses in its edge's order, and the bridge makes a bounded number of receives,
/// each one boxed future, per envelope.
#[test]
fn idle_known_edges_are_not_polled_and_sparse_texts_cross_in_order() {
    let ended = run(
        "idle then sparse known edges",
        &[
            (Kind::Plain, true),
            (Kind::Plain, true),
            (Kind::Plain, true),
        ],
        idle_then_sparse,
    );
    let idle = IDLE_KNOWN_POLLS.load(Ordering::SeqCst);
    let envelopes = u64::try_from(EDGES * (SPARSE + 1)).expect("small");
    eprintln!(
        "idle then sparse known edges: {idle} polls while idle; {} receives and {} polls for \
         {envelopes} envelopes",
        ended.recvs, ended.polls
    );
    assert!(
        matches!(ended.drained, Some(Ok(())))
            && ended.left_on_the_edges == 0
            && each_edge_in_order(&ended.texts, EDGES, SPARSE),
        "a sparse text was lost or reordered: {ended:?}"
    );
    assert_eq!(idle, 0, "known edges left empty were polled while idle");
    assert!(
        ended.recvs <= envelopes * 3 * u64::try_from(EDGES).expect("small"),
        "the bridge made {} receives for {envelopes} envelopes over {EDGES} edges",
        ended.recvs
    );
}
