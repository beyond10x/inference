//! Adversary pass 1 on `story:runtime-subgraph-and-recorder-bounds`: the variants the
//! implementor's cases do not cover.
//!
//! - a subgraph node dropped while its child holds a subgraph node of its own ends both
//!   children within the bound;
//! - a subgraph node dropped while a drain of it is in flight, the drain abandoned, ends its
//!   child within the bound;
//! - a subgraph node dropped after a stop of it was abandoned during the usage grace gives way at
//!   once, as its drop is documented to;
//! - a cancel followed by a stop loses no usage when the consumer reads within the grace, on a
//!   `fail` edge and on a `pause` edge;
//! - a recorder of bound 0 and one filled exactly to its bound;
//! - the replay of a recording its bound cut, against a run that made the same decisions.

use std::collections::HashMap;
use std::future::{self, Future};
use std::num::NonZeroUsize;
use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use inference_core::endpoint::{EndpointFuture, OutputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_core::usage::UsageRecorded;
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeEndpoints, NodeError,
    NodeFuture, NodeImplementation, NodeInstanceId, NodeOutput, NodeRole, OverloadPolicy,
    PipelineSpec, PlannedPipeline, PortId, PortTraits, USAGE_PORT, modality,
};
use inference_runtime::SubgraphNode;
use inference_runtime::record::{Recorded, Recorder};
use inference_runtime::replay::Replay;
use inference_runtime::subgraph::{CHILD_DROP_BOUND, USAGE_STOP_GRACE};
use inference_testkit::graph::PipelinePayload;
use inference_testkit::stream::StreamEvent;
use inference_text::{SpanId, TranscriptCommitted};

const MARGIN: Duration = Duration::from_secs(3);

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

/// Polls `future` for at most `window`: its output if it completed by then.
fn poll_for<F: Future>(mut future: Pin<&mut F>, window: Duration) -> Option<F::Output> {
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let until = Instant::now() + window;
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

fn catalog() -> Profiles {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
    })
    .expect("a row of the table");
    Profiles(profile)
}

const TEXT: PortTraits = PortTraits {
    essentiality: Essentiality::Essential,
    delivery: Delivery::Reliable,
    incremental: true,
    end_marker_required: true,
    provisional_allowed: false,
};

const RECORDS: PortTraits = PortTraits {
    incremental: false,
    ..TEXT
};

fn spec(name: &str) -> PipelineSpec {
    PipelineSpec {
        name: name.to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    }
}

fn one() -> NonZeroUsize {
    NonZeroUsize::new(1).expect("1")
}

fn envelope(
    run_id: RunId,
    sequence: i64,
    kind: &str,
    payload: PipelinePayload,
) -> EventEnvelope<PipelinePayload> {
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id,
        session_id: None,
        stream_id: StreamId::generate(),
        sequence,
        producer: NodeId::generate(),
        port: "out".to_owned(),
        kind: kind.to_owned(),
        generation_id: None,
        timestamp: 0,
        timebase: Timebase::WallTime,
        causation_id: None,
        payload,
    }
}

fn text(run_id: RunId, sequence: i64) -> EventEnvelope<PipelinePayload> {
    let event = StreamEvent::TranscriptCommitted(TranscriptCommitted {
        span_id: SpanId::generate(),
        text: sequence.to_string(),
    });
    let kind = event.name().to_owned();
    envelope(run_id, sequence, &kind, PipelinePayload::Stream(event))
}

#[derive(Clone, Copy)]
enum Act {
    Linger,
    OneUsage,
}

struct ChildNode {
    run_id: RunId,
    act: Act,
    held: Option<NodeEndpoints<PipelinePayload>>,
    _alive: mpsc::Sender<()>,
}

impl NodeImplementation<PipelinePayload> for ChildNode {
    fn describe(&mut self, _: &Node) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn start(
        &mut self,
        mut endpoints: NodeEndpoints<PipelinePayload>,
    ) -> NodeFuture<'_, Result<(), NodeError>> {
        let run_id = self.run_id;
        match self.act {
            Act::Linger => self.held = Some(endpoints),
            Act::OneUsage => {
                let output = endpoints.outputs.remove(0).endpoint;
                thread::spawn(move || {
                    let _sent = block_on(output.send(envelope(
                        run_id,
                        1,
                        UsageRecorded::KIND,
                        PipelinePayload::Control(ControlKind::Commit),
                    )));
                });
            }
        }
        Box::pin(future::ready(Ok(())))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        self.held = None;
        Box::pin(future::ready(()))
    }
}

/// Exposes `port` of `node` under its name, text or, for `usage`, structured.
fn expose_out<'b>(
    pipeline: &mut inference_pipeline::PipelineBuilder<'_, 'b>,
    node: &inference_pipeline::NodeHandle<'b>,
    port: &str,
) -> PortId {
    if port == USAGE_PORT {
        let out = pipeline.output::<modality::Structured>(node, port, RECORDS);
        pipeline.expose(&out, port, one(), OverloadPolicy::Pause);
        out.id()
    } else {
        let out = pipeline.output::<modality::Text>(node, port, TEXT);
        pipeline.expose(&out, port, one(), OverloadPolicy::Pause);
        out.id()
    }
}

/// A source pipeline, and `depth` pipelines each of one subgraph node over the one before, every
/// one exposing `port`. Answers the declaration, the planned pipelines from the source up, and the
/// topmost subgraph node with its output port.
fn declared(port: &str, depth: usize) -> (Declaration, Vec<PlannedPipeline>, Node, PortId) {
    let catalog = catalog();
    let mut declaration = Declaration::new();
    let (source_id, ()) = declaration
        .build(
            &catalog,
            catalog.0.profile_id(),
            spec("source"),
            |pipeline| {
                let source = pipeline.node("source", NodeRole::Source, Vec::new());
                let _ = expose_out(pipeline, &source, port);
            },
        )
        .expect("the profile is declared");
    let validated = declaration.validate(source_id).expect("valid");
    let mut planned = vec![declaration.plan(validated, &catalog).expect("plans")];
    let mut below = source_id;
    let mut top = None;
    for level in 0..depth {
        let (id, (subgraph, out)) = declaration
            .build(
                &catalog,
                catalog.0.profile_id(),
                spec(&format!("level{level}")),
                |pipeline| {
                    let subgraph = pipeline
                        .subgraph("sub", NodeRole::Transform, below)
                        .expect("the child is Planned");
                    let out = expose_out(pipeline, &subgraph, port);
                    (subgraph.id(), out)
                },
            )
            .expect("the profile is declared");
        let validated = declaration.validate(id).expect("valid");
        planned.push(declaration.plan(validated, &catalog).expect("plans"));
        below = id;
        top = Some((subgraph, out));
    }
    let (subgraph, out) = top.expect("depth is at least 1");
    let node = declaration.node(subgraph).expect("declared").clone();
    (declaration, planned, node, out)
}

/// The implementations of a run of `planned[level]`: the source node at level 0, else a
/// subgraph node over the level below.
fn implement(
    declaration: Declaration,
    planned: Vec<PlannedPipeline>,
    level: usize,
    act: Act,
    alive: mpsc::Sender<()>,
) -> inference_runtime::ChildImplementations<PipelinePayload> {
    Box::new(move |run_id, _: &Node| {
        if level == 0 {
            Box::new(ChildNode {
                run_id,
                act,
                held: None,
                _alive: alive.clone(),
            }) as Box<dyn NodeImplementation<PipelinePayload>>
        } else {
            Box::new(SubgraphNode::new(
                run_id,
                &declaration,
                &planned[level - 1],
                HashMap::<GenerationId, i64>::new(),
                implement(
                    declaration.clone(),
                    planned.clone(),
                    level - 1,
                    act,
                    alive.clone(),
                ),
            ))
        }
    })
}

/// The topmost subgraph node of a chain of `depth`, started with `output` on its port.
fn started(
    port: &str,
    depth: usize,
    act: Act,
    output: Box<dyn OutputEndpoint<PipelinePayload>>,
) -> (SubgraphNode<PipelinePayload>, mpsc::Receiver<()>) {
    let (declaration, planned, top, out) = declared(port, depth);
    let (alive, gone) = mpsc::channel();
    let mut node = SubgraphNode::new(
        RunId::generate(),
        &declaration,
        &planned[depth - 1],
        HashMap::<GenerationId, i64>::new(),
        implement(declaration.clone(), planned.clone(), depth - 1, act, alive),
    );
    block_on(node.describe(&top));
    node.described(NodeInstanceId::generate());
    block_on(node.prepare()).expect("the child prepares");
    block_on(node.start(NodeEndpoints {
        inputs: Vec::new(),
        outputs: vec![NodeOutput {
            port_id: out,
            endpoint: output,
        }],
    }))
    .expect("the child starts");
    (node, gone)
}

/// A parent edge whose consumer never reads: every send waits forever, and is counted.
struct NeverRead(Arc<AtomicUsize>);

impl OutputEndpoint<PipelinePayload> for NeverRead {
    fn send(
        &self,
        _: EventEnvelope<PipelinePayload>,
    ) -> EndpointFuture<'_, Result<(), SendError<PipelinePayload>>> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Box::pin(future::pending())
    }
}

/// Drops `value` on a thread of its own: how long it took, or none past `bound`.
fn dropped_within<T: Send + 'static>(value: T, bound: Duration) -> Option<Duration> {
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let started = Instant::now();
        drop(value);
        let _ = done.send(started.elapsed());
    });
    finished.recv_timeout(bound).ok()
}

/// Dropping a subgraph node whose child runs a subgraph node of its own ends both children's
/// threads within the bound: the source node two levels down is dropped.
#[test]
fn dropping_a_subgraph_node_over_a_nested_subgraph_ends_both_children() {
    let (node, gone) = started("out", 2, Act::Linger, Box::new(NeverRead(Arc::default())));
    assert_eq!(
        gone.recv_timeout(Duration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout),
        "the nested child runs"
    );
    let took = dropped_within(node, CHILD_DROP_BOUND + MARGIN);
    assert!(
        took.is_some(),
        "dropping the outer subgraph node did not return within {:?}",
        CHILD_DROP_BOUND + MARGIN
    );
    assert_eq!(
        gone.recv_timeout(MARGIN),
        Err(mpsc::RecvTimeoutError::Disconnected),
        "the nested child's node outlived the outer node's drop ({took:?})"
    );
}

/// Dropping a subgraph node while a drain of it waits for the child, the drain abandoned first,
/// ends the child within the bound.
#[test]
fn dropping_a_subgraph_node_during_an_abandoned_drain_ends_the_child() {
    let (mut node, gone) = started("out", 1, Act::Linger, Box::new(NeverRead(Arc::default())));
    {
        let drain = pin!(node.drain());
        let _drained_or_not = poll_for(drain, Duration::from_millis(100));
    }
    let took = dropped_within(node, CHILD_DROP_BOUND + MARGIN);
    assert!(
        took.is_some(),
        "dropping the node after an abandoned drain did not return within {:?}",
        CHILD_DROP_BOUND + MARGIN
    );
    assert_eq!(
        gone.recv_timeout(MARGIN),
        Err(mpsc::RecvTimeoutError::Disconnected),
        "the child's node outlived the drop ({took:?})"
    );
}

/// How soon a drop counts as "at once" for a usage send already waiting: well inside the grace.
const AT_ONCE: Duration = Duration::from_millis(400);

/// The drop of a subgraph node has every bridge send give way at once, usage sends included
/// (its documentation). Here a stop of the node was started, so the usage send waits for the
/// grace, and abandoned, as by a caller timing the stop out; the node is then dropped.
#[test]
fn a_drop_after_an_abandoned_stop_gives_the_usage_send_way_at_once() {
    let sends = Arc::new(AtomicUsize::new(0));
    let (mut node, _gone) = started(
        USAGE_PORT,
        1,
        Act::OneUsage,
        Box::new(NeverRead(Arc::clone(&sends))),
    );
    let handle = node.handle();
    let first = Instant::now() + Duration::from_secs(10);
    while sends.load(Ordering::Relaxed) == 0 && Instant::now() < first {
        thread::yield_now();
    }
    assert_eq!(sends.load(Ordering::Relaxed), 1, "the usage send waits");
    block_on(node.cancel());
    {
        let stop = pin!(node.stop());
        assert!(
            poll_for(stop, Duration::from_millis(50)).is_none(),
            "the stop waits for the grace"
        );
    }
    let took = dropped_within(node, CHILD_DROP_BOUND + MARGIN);
    let took = took.expect("the drop returns within its bound");
    assert!(
        took < AT_ONCE,
        "a drop after an abandoned stop took {took:?}: the waiting usage send gave way only at \
         the stop's grace ({USAGE_STOP_GRACE:?}), not at once as the drop is documented to"
    );
    assert_eq!(handle.dropped_usage(), 1, "the record given up is counted");
}

/// A `fail` edge that is full until `open`, then takes every send.
struct FullUntil {
    open: Arc<AtomicBool>,
    taken: Arc<AtomicUsize>,
}

impl OutputEndpoint<PipelinePayload> for FullUntil {
    fn send(
        &self,
        envelope: EventEnvelope<PipelinePayload>,
    ) -> EndpointFuture<'_, Result<(), SendError<PipelinePayload>>> {
        if self.open.load(Ordering::SeqCst) {
            self.taken.fetch_add(1, Ordering::SeqCst);
            Box::pin(future::ready(Ok(())))
        } else {
            Box::pin(future::ready(Err(SendError::Full(envelope))))
        }
    }
}

/// A `pause` edge whose send waits until `open`, woken then.
#[derive(Clone, Default)]
struct Gate {
    open: Arc<AtomicBool>,
    waiting: Arc<Mutex<Vec<Waker>>>,
    taken: Arc<AtomicUsize>,
}

impl Gate {
    fn release(&self) {
        self.open.store(true, Ordering::SeqCst);
        let wakers =
            std::mem::take(&mut *self.waiting.lock().unwrap_or_else(PoisonError::into_inner));
        wakers.into_iter().for_each(Waker::wake);
    }
}

impl OutputEndpoint<PipelinePayload> for Gate {
    fn send(
        &self,
        _: EventEnvelope<PipelinePayload>,
    ) -> EndpointFuture<'_, Result<(), SendError<PipelinePayload>>> {
        Box::pin(future::poll_fn(|cx| {
            if self.open.load(Ordering::SeqCst) {
                self.taken.fetch_add(1, Ordering::SeqCst);
                return Poll::Ready(Ok(()));
            }
            self.waiting
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(cx.waker().clone());
            if self.open.load(Ordering::SeqCst) {
                self.taken.fetch_add(1, Ordering::SeqCst);
                return Poll::Ready(Ok(()));
            }
            Poll::Pending
        }))
    }
}

/// When the consumer starts reading after the stop began.
const READ_AFTER: Duration = Duration::from_millis(300);

fn cancel_then_stop_with_a_read_within_the_grace(
    output: Box<dyn OutputEndpoint<PipelinePayload>>,
    taken: &Arc<AtomicUsize>,
    release: impl FnOnce() + Send + 'static,
    attempted: impl Fn() -> bool,
) {
    let (mut node, _gone) = started(USAGE_PORT, 1, Act::OneUsage, output);
    let handle = node.handle();
    let first = Instant::now() + Duration::from_secs(10);
    while !attempted() && Instant::now() < first {
        thread::yield_now();
    }
    assert!(attempted(), "the usage send waits for room");
    let (stopped, stopping) = mpsc::channel();
    thread::spawn(move || {
        block_on(node.cancel());
        let started = Instant::now();
        block_on(node.stop());
        let _ = stopped.send(started.elapsed());
    });
    thread::sleep(READ_AFTER);
    release();
    let took = stopping.recv_timeout(USAGE_STOP_GRACE + MARGIN);
    assert!(took.is_ok(), "the stop returns");
    assert_eq!(
        (taken.load(Ordering::SeqCst), handle.dropped_usage()),
        (1, 0),
        "a cancel followed by a stop whose consumer read within the grace lost the usage record"
    );
}

#[test]
fn a_cancel_then_stop_loses_no_usage_read_within_the_grace_on_a_fail_edge() {
    let open = Arc::new(AtomicBool::new(false));
    let taken = Arc::new(AtomicUsize::new(0));
    let attempts = Arc::new(AtomicUsize::new(0));
    struct Counting(FullUntil, Arc<AtomicUsize>);
    impl OutputEndpoint<PipelinePayload> for Counting {
        fn send(
            &self,
            envelope: EventEnvelope<PipelinePayload>,
        ) -> EndpointFuture<'_, Result<(), SendError<PipelinePayload>>> {
            self.1.fetch_add(1, Ordering::SeqCst);
            self.0.send(envelope)
        }
    }
    let output = Counting(
        FullUntil {
            open: Arc::clone(&open),
            taken: Arc::clone(&taken),
        },
        Arc::clone(&attempts),
    );
    cancel_then_stop_with_a_read_within_the_grace(
        Box::new(output),
        &taken,
        move || open.store(true, Ordering::SeqCst),
        || attempts.load(Ordering::SeqCst) > 0,
    );
}

#[test]
fn a_cancel_then_stop_loses_no_usage_read_within_the_grace_on_a_pause_edge() {
    let gate = Gate::default();
    let taken = Arc::clone(&gate.taken);
    let waiting = Arc::clone(&gate.waiting);
    let releaser = gate.clone();
    cancel_then_stop_with_a_read_within_the_grace(
        Box::new(gate),
        &taken,
        move || releaser.release(),
        move || {
            !waiting
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_empty()
        },
    );
}

/// An ingress that takes everything at once.
struct Taking;

impl OutputEndpoint<()> for Taking {
    fn send(&self, _: EventEnvelope<()>) -> EndpointFuture<'_, Result<(), SendError<()>>> {
        Box::pin(future::ready(Ok(())))
    }
}

fn once<F: Future>(future: F) -> F::Output {
    match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("the send completes at once"),
    }
}

fn unit(sequence: i64) -> EventEnvelope<()> {
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id: RunId::generate(),
        session_id: None,
        stream_id: StreamId::generate(),
        sequence,
        producer: NodeId::generate(),
        port: "audio".to_owned(),
        kind: "inference.audio.AudioChunk".to_owned(),
        generation_id: None,
        timestamp: 0,
        timebase: Timebase::WallTime,
        causation_id: None,
        payload: (),
    }
}

#[test]
fn a_recorder_of_bound_zero_keeps_nothing_and_writes_everything() {
    let written = Arc::new(AtomicUsize::new(0));
    let sink = Arc::clone(&written);
    let recorder = Recorder::new("1")
        .with_sink(move |_: &Recorded<()>| {
            sink.fetch_add(1, Ordering::SeqCst);
        })
        .with_bound(0);
    let ingress = recorder.ingress("audio", Box::new(Taking));
    for sequence in 1..=3 {
        once(ingress.send(unit(sequence))).expect("taken");
    }
    assert!(recorder.recording().entries.is_empty());
    assert!(recorder.decisions().is_empty());
    assert_eq!(recorder.dropped(), 3);
    assert_eq!(written.load(Ordering::SeqCst), 3);
}

#[test]
fn a_recorder_filled_exactly_to_its_bound_drops_nothing_and_the_next_is_dropped() {
    let recorder = Recorder::new("1").with_bound(5);
    let ingress = recorder.ingress("audio", Box::new(Taking));
    for sequence in 1..=5 {
        once(ingress.send(unit(sequence))).expect("taken");
    }
    assert_eq!(
        (recorder.recording().entries.len(), recorder.dropped()),
        (5, 0)
    );
    once(ingress.send(unit(6))).expect("taken");
    assert_eq!(
        (recorder.recording().entries.len(), recorder.dropped()),
        (5, 1)
    );
}

/// A node that, once started, emits `commits` transcript commits on its first output, each taken
/// at once.
struct Committer {
    commits: i64,
}

impl NodeImplementation<PipelinePayload> for Committer {
    fn describe(&mut self, _: &Node) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn start(
        &mut self,
        mut endpoints: NodeEndpoints<PipelinePayload>,
    ) -> NodeFuture<'_, Result<(), NodeError>> {
        let output = endpoints.outputs.remove(0).endpoint;
        let run_id = RunId::generate();
        for sequence in 1..=self.commits {
            block_on(output.send(text(run_id, sequence))).expect("taken");
        }
        Box::pin(future::ready(Ok(())))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }
}

struct TakingText;

impl OutputEndpoint<PipelinePayload> for TakingText {
    fn send(
        &self,
        _: EventEnvelope<PipelinePayload>,
    ) -> EndpointFuture<'_, Result<(), SendError<PipelinePayload>>> {
        Box::pin(future::ready(Ok(())))
    }
}

/// Runs a `Committer` of `commits`, wrapped by `recorder`, as the node `node` on port `port`.
fn commit(recorder: &Recorder<PipelinePayload>, node: &Node, port: PortId, commits: i64) {
    let mut wrapped = recorder.wrap(node, Box::new(Committer { commits }));
    block_on(wrapped.start(NodeEndpoints {
        inputs: Vec::new(),
        outputs: vec![NodeOutput {
            port_id: port,
            endpoint: Box::new(TakingText),
        }],
    }))
    .expect("started");
}

/// The module documentation says what a bounded recorder keeps is a prefix of the run "which a
/// replay can feed". A replay that makes exactly the recorded run's decisions is compared with
/// the recording its bound cut: the report names a divergence at the cut, and neither the
/// recording nor the report says the recording was cut, so the caller cannot tell it from a
/// real one.
#[test]
fn the_replay_of_a_recording_its_bound_cut_reports_no_divergence_the_run_did_not_make() {
    let (_, _, node, port) = declared("out", 1);
    let bound = 3;
    let commits = 5;
    let recorded = Recorder::new("1").with_bound(bound);
    commit(&recorded, &node, port, commits);
    assert_eq!(recorded.dropped(), 2, "the bound cut the recording");
    let recording = recorded.recording();

    let replayed = Recorder::new("1");
    let replay = Replay::new(&recording, &replayed);
    commit(&replayed, &node, port, commits);
    let report = replay.report();
    assert!(
        report.divergence.is_none(),
        "a replay that made the recorded run's {commits} decisions, compared with the recording \
         its bound of {bound} cut, reports the divergence {:?}; the recording carries no mark of \
         the cut, so the report cannot be told from a real divergence",
        report.divergence
    );
}
