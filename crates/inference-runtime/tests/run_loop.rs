//! The run loop, [`RunController::drive`], issues no command the run's state machine would
//! refuse: it drives nothing before the run is Active, takes a cancel before a drain, completes a
//! run it drained, and leaves a terminal run as it is (`story:text-pipeline-end-to-end`, Step 3).

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

use inference_core::id::GenerationId;
use inference_core::model::{Adapter, AdapterId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, InteractionMode, Node, NodeImplementation, NodeRole, OverloadPolicy,
    PipelineSpec, PortTraits, modality,
};
use inference_runtime::run::{FailureReason, NodeInstanceState, RunState, RunStateConflict};
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

/// A Created run of a source feeding a sink, both recording mocks, and their records. A
/// recording source drops its endpoints when it starts, so it never sends an end of input.
fn created() -> (RunController<String>, HashMap<NodeId, NodeCalls>) {
    created_stalling(None)
}

/// [`created`], each node stalling at `stall`, if given.
fn created_stalling(
    stall: Option<LifecycleStep>,
) -> (RunController<String>, HashMap<NodeId, NodeCalls>) {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    let catalog = Profiles(profile);
    let spec = PipelineSpec {
        name: "two nodes".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let mut declaration = Declaration::new();
    let (pipeline_id, ()) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let source = pipeline.node("source", NodeRole::Source, Vec::new());
            let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
            let out = pipeline.output::<modality::Text>(&source, "out", PortTraits::default());
            let into = pipeline.input::<modality::Text>(&sink, "in", PortTraits::default());
            pipeline.connect(&out, &into, NonZeroUsize::MIN, OverloadPolicy::Pause);
        })
        .expect("the profile is declared");
    let validated = declaration
        .validate(pipeline_id)
        .expect("a source feeding a sink is valid");
    let planned = declaration
        .plan(validated, &catalog)
        .expect("nothing is required of either node");
    let mut calls = HashMap::new();
    let controller = RunController::create(
        &planned,
        &declaration,
        HashMap::<GenerationId, i64>::new(),
        |node: &Node| -> Box<dyn NodeImplementation<String>> {
            let mock = match stall {
                Some(step) => RecordingNode::new().stalling_at(step),
                None => RecordingNode::new(),
            };
            calls.insert(node.node_id(), mock.calls());
            Box::new(mock)
        },
    )
    .expect("the edge pauses");
    (controller, calls)
}

fn active() -> (RunController<String>, HashMap<NodeId, NodeCalls>) {
    active_stalling(None)
}

/// [`active`], each node stalling at `stall`, if given.
fn active_stalling(
    stall: Option<LifecycleStep>,
) -> (RunController<String>, HashMap<NodeId, NodeCalls>) {
    let (mut controller, calls) = created_stalling(stall);
    controller.validate().expect("a Created run validates");
    block_on(controller.prepare()).expect("both nodes prepare");
    block_on(controller.activate()).expect("both nodes start");
    (controller, calls)
}

fn stopped(controller: &RunController<String>) -> bool {
    controller
        .instances()
        .all(|instance| instance.state() == NodeInstanceState::Stopped)
}

#[test]
fn the_loop_drives_nothing_before_the_run_is_active() {
    let (mut controller, _) = created();
    assert_eq!(
        block_on(controller.drive()),
        Err(RunError::Conflict(RunStateConflict {
            current: RunState::Created,
        }))
    );
    assert_eq!(controller.run().state(), RunState::Created);

    controller.validate().expect("a Created run validates");
    assert_eq!(
        block_on(controller.drive()),
        Err(RunError::Conflict(RunStateConflict {
            current: RunState::Validating,
        }))
    );
    assert_eq!(controller.run().state(), RunState::Validating);
}

#[test]
fn a_drain_request_drains_then_completes_the_run() {
    let (mut controller, calls) = active();
    controller.requests().drain();
    block_on(controller.drive()).expect("the loop ends the run");

    assert_eq!(controller.run().state(), RunState::Completed);
    assert!(stopped(&controller));
    for record in calls.values() {
        assert_eq!(
            record.calls()[3..],
            [NodeCall::Drain, NodeCall::Stop],
            "each node is drained, then stopped"
        );
    }
}

#[test]
fn a_cancel_request_is_taken_before_a_drain_request() {
    let (mut controller, calls) = active();
    let requests = controller.requests();
    requests.drain();
    requests.cancel();
    block_on(controller.drive()).expect("the loop ends the run");

    assert_eq!(controller.run().state(), RunState::Cancelled);
    assert!(stopped(&controller));
    for record in calls.values() {
        assert_eq!(record.calls()[3..], [NodeCall::Cancel, NodeCall::Stop]);
    }
}

#[test]
fn a_request_from_another_thread_wakes_the_waiting_loop() {
    let (mut controller, _) = active();
    let requests = controller.requests();
    let drive = thread::spawn(move || {
        let result = block_on(controller.drive());
        (controller, result)
    });
    thread::sleep(std::time::Duration::from_millis(20));
    requests.cancel();
    let (controller, result) = drive.join().expect("the loop does not panic");
    result.expect("the loop ends the run");
    assert_eq!(controller.run().state(), RunState::Cancelled);
}

#[test]
fn a_terminal_run_is_left_as_it_is_and_a_late_request_is_dropped() {
    let (mut controller, calls) = active();
    controller.requests().cancel();
    block_on(controller.drive()).expect("the loop ends the run");
    let before: Vec<Vec<NodeCall>> = calls.values().map(NodeCalls::calls).collect();

    controller.requests().drain();
    controller.requests().cancel();
    block_on(controller.drive()).expect("a terminal run is left as it is");
    assert_eq!(controller.run().state(), RunState::Cancelled);
    let after: Vec<Vec<NodeCall>> = calls.values().map(NodeCalls::calls).collect();
    assert_eq!(before, after, "no node is called again");
}

/// A failure the caller reports, naming no node.
fn reported(detail: &str) -> FailureReason {
    FailureReason {
        node_id: None,
        detail: detail.to_owned(),
    }
}

#[test]
fn a_fail_request_is_taken_before_a_cancel_and_a_drain_and_fails_the_run_with_its_reason() {
    let (mut controller, calls) = active();
    let requests = controller.requests();
    requests.drain();
    requests.cancel();
    requests.fail(reported("first"));
    requests.fail(reported("second"));
    block_on(controller.drive()).expect("the loop ends the run");

    assert_eq!(controller.run().state(), RunState::Failed);
    assert_eq!(
        controller.run().failure(),
        Some(&reported("first")),
        "the first failure asked for is kept"
    );
    assert!(stopped(&controller));
    for record in calls.values() {
        assert_eq!(record.calls()[3..], [NodeCall::Cancel, NodeCall::Stop]);
    }
}

#[test]
fn a_fail_request_abandons_a_drain_that_never_returns() {
    let (mut controller, calls) = active_stalling(Some(LifecycleStep::Drain));
    let requests = controller.requests();
    requests.drain();
    {
        let mut drive = pin!(controller.drive());
        let mut context = Context::from_waker(Waker::noop());
        assert!(
            drive.as_mut().poll(&mut context).is_pending(),
            "the nodes drain"
        );
        requests.fail(reported("seen while draining"));
        assert_eq!(drive.as_mut().poll(&mut context), Poll::Ready(Ok(())));
    }

    assert_eq!(controller.run().state(), RunState::Failed);
    assert_eq!(
        controller.run().failure(),
        Some(&reported("seen while draining"))
    );
    assert!(stopped(&controller));
    let mut drained = 0;
    for record in calls.values() {
        let after_start = record.calls()[3..].to_vec();
        if after_start.first() == Some(&NodeCall::Drain) {
            drained += 1;
        }
        assert!(
            after_start.ends_with(&[NodeCall::Cancel, NodeCall::Stop]),
            "every node is cancelled, then stopped: {after_start:?}"
        );
    }
    assert_eq!(
        drained, 1,
        "the source drains first and never returns, so the sink is never asked to drain"
    );
}

#[test]
fn a_fail_request_on_a_terminal_run_is_dropped() {
    let (mut controller, _) = active();
    controller.requests().drain();
    block_on(controller.drive()).expect("the loop ends the run");

    controller.requests().fail(reported("late"));
    block_on(controller.drive()).expect("a terminal run is left as it is");
    assert_eq!(controller.run().state(), RunState::Completed);
    assert_eq!(controller.run().failure(), None);
}
