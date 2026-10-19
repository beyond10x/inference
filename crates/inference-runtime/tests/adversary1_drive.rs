//! Adversary pass 1 on the run loop, [`RunController::drive`]: the end of input of sources fed by
//! more than one edge, of more than one source and of no source; a stop that is not a source's;
//! a cancel while the run drains; a request made before the run is Active; and the queues of a
//! run that ended before it started.
//!
//! Most cases poll the loop once with a waker that does nothing and drop it, so what the loop
//! did on its first poll is read from the run without a thread or a timeout.

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeEndpoints, NodeError,
    NodeFuture, NodeImplementation, NodeRole, OverloadPolicy, PipelineBuilder, PipelineSpec,
    PlannedPipeline, PortTraits, modality,
};
use inference_runtime::run::RunState;
use inference_runtime::{NodeId, RunController, RunError};
use inference_testkit::graph::{LifecycleStep, NodeCall, NodeCalls, RecordingNode};

struct Profiles(DeploymentProfile);

impl Catalog for Profiles {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.0.profile_id() == profile_id).then_some(&self.0)
    }

    fn adapter(&self, _: AdapterId) -> Option<&Adapter> {
        None
    }
}

/// Declares, validates and plans the pipeline `define` builds.
fn plan<R>(
    define: impl for<'b> FnOnce(&mut PipelineBuilder<'_, 'b>) -> R,
) -> (Declaration, PlannedPipeline, R) {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    let catalog = Profiles(profile);
    let spec = PipelineSpec {
        name: "adversary".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let mut declaration = Declaration::new();
    let (pipeline_id, handles) = declaration
        .build(&catalog, profile_id, spec, define)
        .expect("the profile is declared");
    let validated = declaration
        .validate(pipeline_id)
        .expect("the pipeline is valid");
    let planned = declaration
        .plan(validated, &catalog)
        .expect("nothing is required of any node");
    (declaration, planned, handles)
}

/// What a [`Scripted`] node sends when it starts: on the output edge at `.0`, an envelope of
/// kind `.1`.
type Sends = Vec<(usize, &'static str)>;

/// A node that, when it starts, sends the envelopes of its script and then drops its endpoints,
/// and whose drain, when it is gated, completes only once the gate is open (polled again by the
/// test, never woken).
struct Scripted {
    sends: Sends,
    drain_gate: Option<Arc<AtomicBool>>,
    node_id: Option<NodeId>,
}

impl Scripted {
    fn sending(sends: Sends) -> Self {
        Self {
            sends,
            drain_gate: None,
            node_id: None,
        }
    }

    fn gated(gate: &Arc<AtomicBool>) -> Self {
        Self {
            sends: Vec::new(),
            drain_gate: Some(Arc::clone(gate)),
            node_id: None,
        }
    }
}

fn envelope(
    producer: NodeId,
    stream_id: StreamId,
    sequence: i64,
    kind: &str,
) -> EventEnvelope<String> {
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id: RunId::generate(),
        session_id: None,
        stream_id,
        sequence,
        producer,
        port: "out".to_owned(),
        kind: kind.to_owned(),
        generation_id: None,
        timestamp: 0,
        timebase: Timebase::WallTime,
        causation_id: None,
        payload: kind.to_owned(),
    }
}

impl NodeImplementation<String> for Scripted {
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.node_id = Some(node.node_id());
        Box::pin(std::future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn start(&mut self, endpoints: NodeEndpoints<String>) -> NodeFuture<'_, Result<(), NodeError>> {
        let producer = self.node_id.expect("described before started");
        let sends = self.sends.clone();
        Box::pin(async move {
            let stream_id = StreamId::generate();
            for (sequence, (index, kind)) in (1..).zip(sends) {
                endpoints.outputs[index]
                    .endpoint
                    .send(envelope(producer, stream_id, sequence, kind))
                    .await
                    .map_err(|refused| NodeError::new(refused.to_string()))?;
            }
            drop(endpoints);
            Ok(())
        })
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        match self.drain_gate.clone() {
            None => Box::pin(std::future::ready(Ok(()))),
            Some(gate) => Box::pin(std::future::poll_fn(move |_| {
                if gate.load(Ordering::SeqCst) {
                    Poll::Ready(Ok(()))
                } else {
                    Poll::Pending
                }
            })),
        }
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(std::future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(std::future::ready(()))
    }
}

/// A Created run of `planned` whose node implementations `implement` returns, keyed by the
/// node's name.
fn create(
    declaration: &Declaration,
    planned: &PlannedPipeline,
    mut implementations: HashMap<NodeId, Box<dyn NodeImplementation<String>>>,
) -> RunController<String> {
    RunController::create(
        planned,
        declaration,
        HashMap::<GenerationId, i64>::new(),
        |node: &Node| {
            implementations
                .remove(&node.node_id())
                .unwrap_or_else(|| Box::new(RecordingNode::new()))
        },
    )
    .expect("every edge pauses")
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

fn activate(controller: &mut RunController<String>) {
    controller.validate().expect("a Created run validates");
    block_on(controller.prepare()).expect("every node prepares");
    block_on(controller.activate()).expect("every node starts");
    assert_eq!(controller.run().state(), RunState::Active);
}

/// Polls the run loop once and drops it: what it did before it first waited.
fn poll_once(controller: &mut RunController<String>) -> Poll<Result<(), RunError>> {
    let mut drive = pin!(controller.drive());
    drive.as_mut().poll(&mut Context::from_waker(Waker::noop()))
}

const OUT: PortTraits = PortTraits {
    essentiality: Essentiality::Essential,
    delivery: Delivery::Reliable,
    incremental: false,
    end_marker_required: false,
    provisional_allowed: false,
};

fn capacity() -> NonZeroUsize {
    NonZeroUsize::new(4).expect("4")
}

#[test]
fn a_source_feeding_two_edges_has_not_ended_its_input_until_both_took_its_stop() {
    let (declaration, planned, (source, _)) = plan(|pipeline| {
        let source = pipeline.node("source", NodeRole::Source, Vec::new());
        let left = pipeline.node("left", NodeRole::Sink, Vec::new());
        let right = pipeline.node("right", NodeRole::Sink, Vec::new());
        let a = pipeline.output::<modality::Text>(&source, "a", OUT);
        let b = pipeline.output::<modality::Text>(&source, "b", OUT);
        let into_left = pipeline.input::<modality::Text>(&left, "in", OUT);
        let into_right = pipeline.input::<modality::Text>(&right, "in", OUT);
        pipeline.connect(&a, &into_left, capacity(), OverloadPolicy::Pause);
        pipeline.connect(&b, &into_right, capacity(), OverloadPolicy::Pause);
        (source.id(), ())
    });
    let stop = ControlKind::Stop.as_str();
    let mut controller = create(
        &declaration,
        &planned,
        HashMap::from([(
            source,
            Box::new(Scripted::sending(vec![(0, stop), (0, stop)]))
                as Box<dyn NodeImplementation<String>>,
        )]),
    );
    activate(&mut controller);

    assert!(poll_once(&mut controller).is_pending(), "the loop waits");
    assert_eq!(
        controller.run().state(),
        RunState::Active,
        "two stops on one of the source's two edges do not end its input"
    );
}

#[test]
fn one_of_two_sources_ending_its_input_does_not_end_the_runs_input() {
    let (declaration, planned, (first, _)) = plan(|pipeline| {
        let first = pipeline.node("first", NodeRole::Source, Vec::new());
        let second = pipeline.node("second", NodeRole::Source, Vec::new());
        let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
        let from_first = pipeline.output::<modality::Text>(&first, "out", OUT);
        let from_second = pipeline.output::<modality::Text>(&second, "out", OUT);
        let a = pipeline.input::<modality::Text>(&sink, "a", OUT);
        let b = pipeline.input::<modality::Text>(&sink, "b", OUT);
        pipeline.connect(&from_first, &a, capacity(), OverloadPolicy::Pause);
        pipeline.connect(&from_second, &b, capacity(), OverloadPolicy::Pause);
        (first.id(), ())
    });
    let mut controller = create(
        &declaration,
        &planned,
        HashMap::from([(
            first,
            Box::new(Scripted::sending(vec![(0, ControlKind::Stop.as_str())]))
                as Box<dyn NodeImplementation<String>>,
        )]),
    );
    activate(&mut controller);

    assert!(poll_once(&mut controller).is_pending(), "the loop waits");
    assert_eq!(controller.run().state(), RunState::Active);
}

#[test]
fn content_from_a_source_is_not_its_end_of_input() {
    let (declaration, planned, source) = plan(|pipeline| {
        let source = pipeline.node("source", NodeRole::Source, Vec::new());
        let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
        let out = pipeline.output::<modality::Text>(&source, "out", OUT);
        let into = pipeline.input::<modality::Text>(&sink, "in", OUT);
        pipeline.connect(&out, &into, capacity(), OverloadPolicy::Pause);
        source.id()
    });
    let mut controller = create(
        &declaration,
        &planned,
        HashMap::from([(
            source,
            Box::new(Scripted::sending(vec![
                (0, "inference.stream.TranscriptCommitted"),
                (0, "Stop"),
                (0, "end"),
            ])) as Box<dyn NodeImplementation<String>>,
        )]),
    );
    activate(&mut controller);

    assert!(poll_once(&mut controller).is_pending(), "the loop waits");
    assert_eq!(controller.run().state(), RunState::Active);
}

#[test]
fn a_run_without_a_source_does_not_end_its_input_on_its_own() {
    let (declaration, planned, ()) = plan(|pipeline| {
        let _ = pipeline.node("transform", NodeRole::Transform, Vec::new());
    });
    let mut controller = create(&declaration, &planned, HashMap::new());
    activate(&mut controller);

    assert!(poll_once(&mut controller).is_pending(), "the loop waits");
    assert_eq!(controller.run().state(), RunState::Active);
}

#[test]
fn only_a_source_ends_the_input_and_a_node_that_is_not_one_is_not_waited_for() {
    let (declaration, planned, source) = plan(|pipeline| {
        let source = pipeline.node("source", NodeRole::Source, Vec::new());
        let transform = pipeline.node("transform", NodeRole::Transform, Vec::new());
        let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
        let out = pipeline.output::<modality::Text>(&source, "out", OUT);
        let into = pipeline.input::<modality::Text>(&transform, "in", OUT);
        let onward = pipeline.output::<modality::Text>(&transform, "out", OUT);
        let collected = pipeline.input::<modality::Text>(&sink, "in", OUT);
        pipeline.connect(&out, &into, capacity(), OverloadPolicy::Pause);
        pipeline.connect(&onward, &collected, capacity(), OverloadPolicy::Pause);
        source.id()
    });
    let mut controller = create(
        &declaration,
        &planned,
        HashMap::from([(
            source,
            Box::new(Scripted::sending(vec![(0, ControlKind::Stop.as_str())]))
                as Box<dyn NodeImplementation<String>>,
        )]),
    );
    activate(&mut controller);

    assert!(
        poll_once(&mut controller).is_ready(),
        "the source's stop ends the input; the transform, which never sends one, is no source"
    );
    assert_eq!(controller.run().state(), RunState::Completed);
}

/// A source feeding a sink, with `source` and `sink` as their implementations.
fn two_nodes(
    source: Box<dyn NodeImplementation<String>>,
    sink: Box<dyn NodeImplementation<String>>,
) -> RunController<String> {
    let (declaration, planned, (source_id, sink_id)) = plan(|pipeline| {
        let source = pipeline.node("source", NodeRole::Source, Vec::new());
        let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
        let out = pipeline.output::<modality::Text>(&source, "out", OUT);
        let into = pipeline.input::<modality::Text>(&sink, "in", OUT);
        pipeline.connect(&out, &into, capacity(), OverloadPolicy::Pause);
        (source.id(), sink.id())
    });
    create(
        &declaration,
        &planned,
        HashMap::from([(source_id, source), (sink_id, sink)]),
    )
}

#[test]
fn a_cancel_asked_for_while_the_run_drains_is_taken_once_the_drain_returns() {
    let gate = Arc::new(AtomicBool::new(false));
    let mut controller = two_nodes(
        Box::new(Scripted::sending(Vec::new())),
        Box::new(Scripted::gated(&gate)),
    );
    activate(&mut controller);
    let requests = controller.requests();
    requests.drain();
    {
        let mut drive = pin!(controller.drive());
        let mut context = Context::from_waker(Waker::noop());
        assert!(
            drive.as_mut().poll(&mut context).is_pending(),
            "the sink drains"
        );
        requests.cancel();
        gate.store(true, Ordering::SeqCst);
        assert_eq!(drive.as_mut().poll(&mut context), Poll::Ready(Ok(())));
    }
    assert_eq!(
        controller.run().state(),
        RunState::Cancelled,
        "a cancel asked for while the run was Draining is taken"
    );
}

/// Waits at most `patience` for `calls` to hold `call`.
fn wait_for(calls: &NodeCalls, call: &NodeCall, patience: Duration) -> bool {
    let deadline = Instant::now() + patience;
    while Instant::now() < deadline {
        if calls.calls().contains(call) {
            return true;
        }
        thread::sleep(Duration::from_millis(5));
    }
    false
}

#[test]
fn a_cancel_ends_a_run_whose_node_never_finishes_draining() {
    let sink = RecordingNode::new().stalling_at(LifecycleStep::Drain);
    let sink_calls = sink.calls();
    let mut controller = two_nodes(Box::new(Scripted::sending(Vec::new())), Box::new(sink));
    activate(&mut controller);
    let requests = controller.requests();
    let (ended, outcome) = mpsc::channel();
    thread::spawn(move || {
        let result = block_on(controller.drive());
        let _ = ended.send((controller.run().state(), result));
    });
    requests.drain();
    assert!(
        wait_for(&sink_calls, &NodeCall::Drain, Duration::from_secs(5)),
        "the loop drains the sink"
    );
    requests.cancel();

    let outcome = outcome.recv_timeout(Duration::from_secs(5));
    assert_eq!(
        outcome,
        Ok((RunState::Cancelled, Ok(()))),
        "CancelRun is admitted from Draining and abandons in-flight events, so the loop takes the \
         cancel while a node is still draining"
    );
}

#[test]
fn a_drain_asked_for_before_the_run_is_active_is_dropped() {
    let mut controller = two_nodes(
        Box::new(Scripted::sending(Vec::new())),
        Box::new(RecordingNode::new()),
    );
    controller.requests().drain();
    assert_eq!(
        block_on(controller.drive()),
        Err(RunError::Conflict(inference_runtime::RunStateConflict {
            current: RunState::Created,
        })),
        "the loop drives nothing before the run is Active"
    );
    activate(&mut controller);

    assert!(
        poll_once(&mut controller).is_pending(),
        "DrainRun is not admitted from Created, so the request made there was dropped \
         (RunRequests: a request is dropped where the state machine does not admit it)"
    );
    assert_eq!(controller.run().state(), RunState::Active);
}

#[test]
fn the_queues_of_a_run_cancelled_before_it_started_are_dropped() {
    let mut controller = two_nodes(
        Box::new(Scripted::sending(Vec::new())),
        Box::new(RecordingNode::new()),
    );
    let queues = controller.queues();
    assert_eq!(queues.len(), 1);
    assert!(!queues[0].is_dropped(), "a Created run holds its queue");
    controller.validate().expect("a Created run validates");
    block_on(controller.cancel()).expect("a Validating run cancels");
    assert_eq!(controller.run().state(), RunState::Cancelled);

    assert!(
        queues[0].is_dropped(),
        "a run that ended releases its edge queues (§23.3 test 7), started or not"
    );
}
