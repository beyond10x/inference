//! Adversary pass 1 on `story:node-instance-lifecycle`: the run controller drives its instances
//! from every run state `CancelRun` and `FailRun` leave, through a drain error, and through a
//! lifecycle future the caller abandons (a timeout, a `select!`), after which the run and its
//! instances must still agree.

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

use inference_core::id::GenerationId;
use inference_core::model::{Adapter, AdapterId, Capability};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, InteractionMode, Node, NodeEndpoints, NodeError, NodeFuture,
    NodeImplementation, NodeRole, OverloadPolicy, PipelineSpec, PlannedPipeline, PortTraits,
    modality,
};
use inference_runtime::run::{FailureReason, NodeInstanceState, RunState};
use inference_runtime::{FailNodeError, NodeId, RunController, RunError};
use inference_testkit::graph::{FallibleStep, NodeCall, NodeCalls, RecordingNode};

struct Profiles(DeploymentProfile);

impl Catalog for Profiles {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.0.profile_id() == profile_id).then_some(&self.0)
    }

    fn adapter(&self, _: AdapterId) -> Option<&Adapter> {
        None
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

/// Polls `future` once and drops it, as a caller's timeout does; whether it finished.
fn poll_once_and_abandon<F: Future>(future: F) -> bool {
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut future = pin!(future);
    future
        .as_mut()
        .poll(&mut Context::from_waker(&waker))
        .is_ready()
}

fn planned() -> (Declaration, PlannedPipeline, [NodeId; 2]) {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    let catalog = Profiles(profile);
    let mut declaration = Declaration::new();
    let spec = PipelineSpec {
        name: "two nodes".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: vec![Capability::OutputStreaming],
    };
    let (pipeline_id, nodes) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let source = pipeline.node("source", NodeRole::Source, Vec::new());
            let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
            let out = pipeline.output::<modality::Text>(&source, "out", PortTraits::default());
            let into = pipeline.input::<modality::Text>(&sink, "in", PortTraits::default());
            pipeline.connect(&out, &into, NonZeroUsize::MIN, OverloadPolicy::Pause);
            [source.id(), sink.id()]
        })
        .expect("the profile is declared");
    let validated = declaration
        .validate(pipeline_id)
        .expect("a source feeding a sink is valid");
    let planned = declaration
        .plan(validated, &catalog)
        .expect("nothing is required of either node");
    (declaration, planned, nodes)
}

/// A controller of two recording nodes, the sink failing at `failing` if given, and their calls,
/// source first. The node identities are not known before the pipeline is declared, so the
/// implementations are handed out in declaration order.
fn recording(
    failing: Option<(FallibleStep, NodeError)>,
) -> (RunController<String>, [NodeId; 2], [NodeCalls; 2]) {
    let source = RecordingNode::new();
    let sink = match failing {
        Some((step, error)) => RecordingNode::failing_at(step, error),
        None => RecordingNode::new(),
    };
    let calls = [source.calls(), sink.calls()];
    let mut queue: Vec<Box<dyn NodeImplementation<String>>> =
        vec![Box::new(sink), Box::new(source)];
    let (declaration, planned, nodes) = planned();
    let controller = RunController::create(
        &planned,
        &declaration,
        HashMap::<GenerationId, i64>::new(),
        |_: &Node| queue.pop().expect("two nodes"),
    )
    .expect("the pipeline is planned and its edge pauses");
    (controller, nodes, calls)
}

fn states(
    controller: &RunController<String>,
    nodes: [NodeId; 2],
) -> Vec<Option<NodeInstanceState>> {
    nodes
        .iter()
        .map(|node_id| controller.instance(*node_id).map(|i| i.state()))
        .collect()
}

const SOURCE_START: NodeCall = NodeCall::Start {
    inputs: 0,
    outputs: 1,
};
const SINK_START: NodeCall = NodeCall::Start {
    inputs: 1,
    outputs: 0,
};

#[test]
fn a_run_cancelled_while_preparing_cancels_and_stops_each_prepared_node() {
    let (mut controller, nodes, calls) = recording(None);
    controller.validate().expect("Created validates");
    block_on(controller.prepare()).expect("both prepare");
    block_on(controller.cancel()).expect("a Preparing run cancels");

    assert_eq!(controller.run().state(), RunState::Cancelled);
    assert_eq!(
        states(&controller, nodes),
        vec![Some(NodeInstanceState::Stopped); 2]
    );
    for record in &calls {
        assert_eq!(
            record.calls(),
            vec![
                NodeCall::Describe,
                NodeCall::Prepare,
                NodeCall::Cancel,
                NodeCall::Stop
            ],
            "CancelRun cancels then stops each instance that is not terminal"
        );
    }
}

#[test]
fn a_run_cancelled_while_draining_cancels_and_stops_each_draining_node() {
    let (mut controller, nodes, calls) = recording(None);
    controller.validate().expect("Created validates");
    block_on(controller.prepare()).expect("both prepare");
    block_on(controller.activate()).expect("both start");
    block_on(controller.drain()).expect("both drain");
    block_on(controller.cancel()).expect("a Draining run cancels");

    assert_eq!(controller.run().state(), RunState::Cancelled);
    assert_eq!(
        states(&controller, nodes),
        vec![Some(NodeInstanceState::Stopped); 2]
    );
    for (record, start) in calls.iter().zip([SOURCE_START, SINK_START]) {
        assert_eq!(
            record.calls(),
            vec![
                NodeCall::Describe,
                NodeCall::Prepare,
                start,
                NodeCall::Drain,
                NodeCall::Cancel,
                NodeCall::Stop
            ]
        );
    }
}

#[test]
fn a_node_failing_on_drain_fails_the_run_naming_it_and_winds_the_other_down() {
    let error = NodeError::new("an event in flight failed");
    let (mut controller, [source, sink], [source_calls, sink_calls]) =
        recording(Some((FallibleStep::Drain, error.clone())));
    controller.validate().expect("Created validates");
    block_on(controller.prepare()).expect("both prepare");
    block_on(controller.activate()).expect("both start");
    let failure = FailureReason {
        node_id: Some(sink),
        detail: error.detail().to_owned(),
    };
    assert_eq!(
        block_on(controller.drain()),
        Err(RunError::NodeFailed(failure.clone()))
    );

    assert_eq!(controller.run().state(), RunState::Failed);
    assert_eq!(controller.run().failure(), Some(&failure));
    assert_eq!(
        states(&controller, [source, sink]),
        vec![
            Some(NodeInstanceState::Stopped),
            Some(NodeInstanceState::Failed)
        ]
    );
    assert_eq!(
        sink_calls.calls(),
        vec![
            NodeCall::Describe,
            NodeCall::Prepare,
            SINK_START,
            NodeCall::Drain
        ],
        "a failed instance is called no further"
    );
    assert_eq!(
        source_calls.calls(),
        vec![
            NodeCall::Describe,
            NodeCall::Prepare,
            SOURCE_START,
            NodeCall::Drain,
            NodeCall::Cancel,
            NodeCall::Stop
        ]
    );
}

#[test]
fn an_exchange_failure_while_draining_fails_the_draining_instance() {
    let (mut controller, [source, sink], [source_calls, sink_calls]) = recording(None);
    controller.validate().expect("Created validates");
    block_on(controller.prepare()).expect("both prepare");
    block_on(controller.activate()).expect("both start");
    block_on(controller.drain()).expect("both drain");

    let error = NodeError::new("the sink lost its output");
    block_on(controller.fail_node(sink, &error)).expect("a Draining instance fails");

    assert_eq!(controller.run().state(), RunState::Failed);
    assert_eq!(
        states(&controller, [source, sink]),
        vec![
            Some(NodeInstanceState::Stopped),
            Some(NodeInstanceState::Failed)
        ]
    );
    assert_eq!(sink_calls.calls().last(), Some(&NodeCall::Drain));
    assert_eq!(
        source_calls.calls()[4..],
        [NodeCall::Cancel, NodeCall::Stop]
    );
}

#[test]
fn a_run_failed_by_its_caller_while_active_cancels_and_stops_each_node() {
    let (mut controller, nodes, calls) = recording(None);
    controller.validate().expect("Created validates");
    block_on(controller.prepare()).expect("both prepare");
    block_on(controller.activate()).expect("both start");
    let failure = FailureReason {
        node_id: None,
        detail: "the deadline passed".to_owned(),
    };
    block_on(controller.fail(failure.clone())).expect("an Active run fails");

    assert_eq!(controller.run().failure(), Some(&failure));
    assert_eq!(
        states(&controller, nodes),
        vec![Some(NodeInstanceState::Stopped); 2]
    );
    for record in &calls {
        assert_eq!(record.calls()[3..], [NodeCall::Cancel, NodeCall::Stop]);
    }
}

#[test]
fn fail_node_for_a_node_of_no_described_instance_moves_nothing() {
    let (mut controller, nodes, calls) = recording(None);
    controller.validate().expect("Created validates");
    let error = NodeError::new("too early");
    assert!(matches!(
        block_on(controller.fail_node(nodes[0], &error)),
        Err(FailNodeError::UnknownNode(_))
    ));
    assert_eq!(controller.run().state(), RunState::Validating);
    assert!(calls.iter().all(|record| record.calls().is_empty()));
}

/// A node whose step `stalls` never finishes, recording each call by name.
struct Stalling {
    stalls: &'static str,
    calls: Arc<Mutex<Vec<&'static str>>>,
}

impl Stalling {
    fn answer<T: Send + 'static>(&self, step: &'static str, value: T) -> NodeFuture<'_, T> {
        self.calls.lock().expect("unpoisoned").push(step);
        if step == self.stalls {
            Box::pin(std::future::pending())
        } else {
            Box::pin(async move { value })
        }
    }
}

impl NodeImplementation<String> for Stalling {
    fn describe(&mut self, _: &Node) -> NodeFuture<'_, ()> {
        self.answer("describe", ())
    }
    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        self.answer("prepare", Ok(()))
    }
    fn start(&mut self, _: NodeEndpoints<String>) -> NodeFuture<'_, Result<(), NodeError>> {
        self.answer("start", Ok(()))
    }
    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        self.answer("drain", Ok(()))
    }
    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        self.answer("cancel", ())
    }
    fn stop(&mut self) -> NodeFuture<'_, ()> {
        self.answer("stop", ())
    }
}

/// A controller whose first node (the source) stalls at `stalls` and whose sink records.
fn stalling_at(stalls: &'static str) -> (RunController<String>, [NodeId; 2]) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut queue: Vec<Box<dyn NodeImplementation<String>>> = vec![
        Box::new(RecordingNode::new()),
        Box::new(Stalling { stalls, calls }),
    ];
    let (declaration, planned, nodes) = planned();
    let controller = RunController::create(
        &planned,
        &declaration,
        HashMap::<GenerationId, i64>::new(),
        |_: &Node| queue.pop().expect("two nodes"),
    )
    .expect("the pipeline is planned and its edge pauses");
    (controller, nodes)
}

#[test]
fn activating_after_an_abandoned_prepare_does_not_panic() {
    let (mut controller, nodes) = stalling_at("prepare");
    controller.validate().expect("Created validates");
    assert!(
        !poll_once_and_abandon(controller.prepare()),
        "the source's prepare stalls"
    );
    assert_eq!(controller.run().state(), RunState::Preparing);
    assert_eq!(
        states(&controller, nodes),
        vec![Some(NodeInstanceState::Described); 2],
        "the abandoned prepare left both instances Described"
    );

    let activated = catch_unwind(AssertUnwindSafe(|| block_on(controller.activate())));
    assert!(
        activated.is_ok(),
        "ActivateRun on a Preparing run whose instances are Described panicked instead of \
         answering"
    );
}

#[test]
fn completing_after_an_abandoned_drain_leaves_no_instance_unstopped() {
    let (mut controller, nodes) = stalling_at("drain");
    controller.validate().expect("Created validates");
    block_on(controller.prepare()).expect("both prepare");
    block_on(controller.activate()).expect("both start");
    assert!(
        !poll_once_and_abandon(controller.drain()),
        "the source's drain stalls"
    );
    assert_eq!(controller.run().state(), RunState::Draining);

    let completed = block_on(controller.complete());
    let states = states(&controller, nodes);
    assert!(
        completed.is_err()
            || states
                .iter()
                .all(|s| s.is_some_and(NodeInstanceState::is_terminal)),
        "CompleteRun answered {completed:?} and the run is {:?} while its instances are {states:?}",
        controller.run().state()
    );
}
