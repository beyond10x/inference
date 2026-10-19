//! The run drives each node instance through describe, prepare, start, drain or cancel, stop
//! (`story:node-instance-lifecycle`, Checks): a run of two recording mock nodes, connected by one
//! edge, driven to Completed, to Cancelled from Active, and to Failed by a node that fails on
//! start; and a step its caller abandons mid-way leaves no later step to panic or to answer with
//! the run ahead of its instances.

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

use inference_core::id::GenerationId;
use inference_core::model::{Adapter, AdapterId, Capability};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, InteractionMode, Node, NodeError, NodeImplementation, NodeRole,
    OverloadPolicy, PipelineSpec, PlannedPipeline, PortTraits, modality,
};
use inference_runtime::run::{FailureReason, NodeInstanceState, RunState};
use inference_runtime::{NodeId, RunController};
use inference_testkit::graph::{FallibleStep, LifecycleStep, NodeCall, NodeCalls, RecordingNode};

/// The one profile the case declares, and no adapter: what `Declaration::build` reads.
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

/// Polls `future` on this thread until it completes: the runtime commits to no executor.
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

/// A source feeding a sink along one text edge, declared, validated and planned: the declaration,
/// the Planned form and the two nodes, source first.
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

/// A controller of a run of `planned` whose nodes are `mocks`, by node, and the record of each
/// mock's calls.
fn controller(
    declaration: &Declaration,
    planned: &PlannedPipeline,
    mut mocks: HashMap<NodeId, RecordingNode>,
) -> (RunController<String>, HashMap<NodeId, NodeCalls>) {
    let calls = mocks
        .iter()
        .map(|(node_id, mock)| (*node_id, mock.calls()))
        .collect();
    let controller = RunController::create(
        planned,
        declaration,
        HashMap::<GenerationId, i64>::new(),
        |node: &Node| -> Box<dyn NodeImplementation<String>> {
            Box::new(
                mocks
                    .remove(&node.node_id())
                    .expect("every node of the pipeline has a mock"),
            )
        },
    )
    .expect("the pipeline is planned and its edge pauses");
    (controller, calls)
}

fn answering(nodes: [NodeId; 2]) -> HashMap<NodeId, RecordingNode> {
    nodes
        .into_iter()
        .map(|node_id| (node_id, RecordingNode::new()))
        .collect()
}

/// The start each node of `nodes` sees: the source one output endpoint, the sink one input.
fn starts(nodes: [NodeId; 2]) -> HashMap<NodeId, NodeCall> {
    HashMap::from([
        (
            nodes[0],
            NodeCall::Start {
                inputs: 0,
                outputs: 1,
            },
        ),
        (
            nodes[1],
            NodeCall::Start {
                inputs: 1,
                outputs: 0,
            },
        ),
    ])
}

#[test]
fn a_completed_run_described_prepared_started_drained_and_stopped_each_node() {
    let (declaration, planned, nodes) = planned();
    let (mut controller, calls) = controller(&declaration, &planned, answering(nodes));

    controller.validate().expect("a Created run validates");
    block_on(controller.prepare()).expect("both nodes prepare");
    block_on(controller.activate()).expect("both nodes start");
    block_on(controller.drain()).expect("both nodes drain");
    block_on(controller.complete()).expect("a Draining run completes");

    assert_eq!(controller.run().state(), RunState::Completed);
    let starts = starts(nodes);
    for node_id in nodes {
        assert_eq!(
            calls[&node_id].calls(),
            vec![
                NodeCall::Describe,
                NodeCall::Prepare,
                starts[&node_id],
                NodeCall::Drain,
                NodeCall::Stop,
            ],
            "the calls on node {node_id}"
        );
        assert_eq!(
            controller.instance(node_id).map(|i| i.state()),
            Some(NodeInstanceState::Stopped),
            "the instance of node {node_id}"
        );
    }
}

#[test]
fn a_run_cancelled_from_active_cancelled_and_stopped_each_node() {
    let (declaration, planned, nodes) = planned();
    let (mut controller, calls) = controller(&declaration, &planned, answering(nodes));

    controller.validate().expect("a Created run validates");
    block_on(controller.prepare()).expect("both nodes prepare");
    block_on(controller.activate()).expect("both nodes start");
    block_on(controller.cancel()).expect("an Active run cancels");

    assert_eq!(controller.run().state(), RunState::Cancelled);
    let starts = starts(nodes);
    for node_id in nodes {
        assert_eq!(
            calls[&node_id].calls(),
            vec![
                NodeCall::Describe,
                NodeCall::Prepare,
                starts[&node_id],
                NodeCall::Cancel,
                NodeCall::Stop,
            ],
            "the calls on node {node_id}"
        );
        assert_eq!(
            controller.instance(node_id).map(|i| i.state()),
            Some(NodeInstanceState::Stopped),
            "the instance of node {node_id}"
        );
    }
}

#[test]
fn a_node_failing_on_start_fails_the_run_naming_it_and_stops_every_other() {
    let (declaration, planned, nodes) = planned();
    let [source, sink] = nodes;
    let error = NodeError::new("the sink's device is gone");
    let mocks = HashMap::from([
        (source, RecordingNode::new()),
        (
            sink,
            RecordingNode::failing_at(FallibleStep::Start, error.clone()),
        ),
    ]);
    let (mut controller, calls) = controller(&declaration, &planned, mocks);

    controller.validate().expect("a Created run validates");
    block_on(controller.prepare()).expect("both nodes prepare");
    let failure = FailureReason {
        node_id: Some(sink),
        detail: error.detail().to_owned(),
    };
    assert_eq!(
        block_on(controller.activate()),
        Err(inference_runtime::RunError::NodeFailed(failure.clone())),
        "the activation answers the typed failure"
    );

    assert_eq!(controller.run().state(), RunState::Failed);
    assert_eq!(controller.run().failure(), Some(&failure));
    assert_eq!(
        controller.instance(sink).map(|i| i.state()),
        Some(NodeInstanceState::Failed)
    );
    assert_eq!(
        controller.instance(source).map(|i| i.state()),
        Some(NodeInstanceState::Stopped)
    );
    assert_eq!(
        calls[&sink].calls(),
        vec![
            NodeCall::Describe,
            NodeCall::Prepare,
            NodeCall::Start {
                inputs: 1,
                outputs: 0
            },
        ],
        "a failed instance is called no further"
    );
    assert_eq!(
        calls[&source].calls(),
        vec![
            NodeCall::Describe,
            NodeCall::Prepare,
            NodeCall::Start {
                inputs: 0,
                outputs: 1
            },
            NodeCall::Cancel,
            NodeCall::Stop,
        ]
    );
}

#[test]
fn a_node_failing_on_prepare_leaves_a_described_node_to_be_cancelled_and_stopped() {
    let (declaration, planned, nodes) = planned();
    let [source, sink] = nodes;
    let error = NodeError::new("no memory for the source");
    let mocks = HashMap::from([
        (
            source,
            RecordingNode::failing_at(FallibleStep::Prepare, error.clone()),
        ),
        (sink, RecordingNode::new()),
    ]);
    let (mut controller, calls) = controller(&declaration, &planned, mocks);

    controller.validate().expect("a Created run validates");
    let failure = FailureReason {
        node_id: Some(source),
        detail: error.detail().to_owned(),
    };
    assert_eq!(
        block_on(controller.prepare()),
        Err(inference_runtime::RunError::NodeFailed(failure.clone()))
    );

    assert_eq!(controller.run().failure(), Some(&failure));
    assert_eq!(
        controller.instance(source).map(|i| i.state()),
        Some(NodeInstanceState::Failed)
    );
    assert_eq!(
        controller.instance(sink).map(|i| i.state()),
        Some(NodeInstanceState::Stopped)
    );
    assert_eq!(
        calls[&source].calls(),
        vec![NodeCall::Describe, NodeCall::Prepare]
    );
    assert_eq!(
        calls[&sink].calls(),
        vec![NodeCall::Describe, NodeCall::Cancel, NodeCall::Stop],
        "the sink was described and never prepared"
    );
    assert!(
        block_on(controller.activate()).is_err(),
        "a Failed run does not activate"
    );
    assert_eq!(calls[&sink].calls().len(), 3, "and calls no node");
}

#[test]
fn an_exchange_failure_reported_for_a_started_node_fails_the_run_naming_it() {
    let (declaration, planned, nodes) = planned();
    let [source, sink] = nodes;
    let (mut controller, calls) = controller(&declaration, &planned, answering(nodes));
    controller.validate().expect("a Created run validates");
    block_on(controller.prepare()).expect("both nodes prepare");
    block_on(controller.activate()).expect("both nodes start");

    let error = NodeError::new("the source lost its input");
    block_on(controller.fail_node(source, &error)).expect("a started node can fail");

    assert_eq!(controller.run().state(), RunState::Failed);
    assert_eq!(
        controller.run().failure(),
        Some(&FailureReason {
            node_id: Some(source),
            detail: error.detail().to_owned(),
        })
    );
    assert_eq!(
        controller.instance(source).map(|i| i.state()),
        Some(NodeInstanceState::Failed)
    );
    assert_eq!(
        controller.instance(sink).map(|i| i.state()),
        Some(NodeInstanceState::Stopped)
    );
    assert_eq!(calls[&source].calls().last(), Some(&starts(nodes)[&source]));
    assert_eq!(
        calls[&sink].calls()[3..],
        [NodeCall::Cancel, NodeCall::Stop]
    );
    assert!(
        block_on(controller.fail_node(sink, &error)).is_err(),
        "a Stopped instance does not fail"
    );
}

#[test]
fn the_controller_futures_are_send() {
    fn send<T: Send>(_: &T) {}
    let (declaration, planned, nodes) = planned();
    let (mut controller, _) = controller(&declaration, &planned, answering(nodes));
    send(&controller.prepare());
    send(&controller.activate());
    send(&controller.drain());
    send(&controller.complete());
    send(&controller.cancel());
}

/// A step of the run the caller drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Prepare,
    Activate,
    Drain,
    Complete,
    Cancel,
}

/// The state every instance is in once `step` answered `Ok`.
fn reached(step: Step) -> NodeInstanceState {
    match step {
        Step::Prepare => NodeInstanceState::Prepared,
        Step::Activate => NodeInstanceState::Started,
        Step::Drain => NodeInstanceState::Draining,
        Step::Complete | Step::Cancel => NodeInstanceState::Stopped,
    }
}

/// Runs `step` to its end, its refusal written out.
fn drive(controller: &mut RunController<String>, step: Step) -> Result<(), String> {
    match step {
        Step::Prepare => block_on(controller.prepare()),
        Step::Activate => block_on(controller.activate()),
        Step::Drain => block_on(controller.drain()),
        Step::Complete => block_on(controller.complete()),
        Step::Cancel => block_on(controller.cancel()),
    }
    .map_err(|error| format!("{error:?}"))
}

/// Polls `step` once and drops it, as a caller's timeout does; whether it finished.
fn abandon(controller: &mut RunController<String>, step: Step) -> bool {
    fn once<F: Future>(future: F) -> bool {
        let mut future = pin!(future);
        future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_ready()
    }
    match step {
        Step::Prepare => once(controller.prepare()),
        Step::Activate => once(controller.activate()),
        Step::Drain => once(controller.drain()),
        Step::Complete => once(controller.complete()),
        Step::Cancel => once(controller.cancel()),
    }
}

/// The run is never ahead of its instances: a terminal run holds only terminal instances, and a
/// Completed one only Stopped ones.
fn agree(controller: &RunController<String>, nodes: [NodeId; 2], context: &str) {
    let run = controller.run().state();
    let states: Vec<_> = nodes
        .iter()
        .map(|node_id| controller.instance(*node_id).map(|i| i.state()))
        .collect();
    if run.is_terminal() {
        assert!(
            states
                .iter()
                .all(|s| s.is_none_or(NodeInstanceState::is_terminal)),
            "{context}: the run is {run:?} and its instances {states:?}"
        );
    }
    if run == RunState::Completed {
        assert!(
            states
                .iter()
                .all(|s| *s == Some(NodeInstanceState::Stopped)),
            "{context}: the run is Completed and its instances {states:?}"
        );
    }
}

#[test]
fn a_step_abandoned_at_any_node_call_leaves_no_later_step_to_panic_or_run_ahead() {
    let cases = [
        (LifecycleStep::Describe, Step::Prepare, RunState::Preparing),
        (LifecycleStep::Prepare, Step::Prepare, RunState::Preparing),
        (LifecycleStep::Start, Step::Activate, RunState::Preparing),
        (LifecycleStep::Drain, Step::Drain, RunState::Draining),
        (LifecycleStep::Stop, Step::Complete, RunState::Draining),
        (LifecycleStep::Cancel, Step::Cancel, RunState::Active),
        (LifecycleStep::Stop, Step::Cancel, RunState::Active),
    ];
    let path = [Step::Prepare, Step::Activate, Step::Drain, Step::Complete];
    for (stall, abandoned, left) in cases {
        let (declaration, planned, nodes) = planned();
        let mocks = HashMap::from([
            (nodes[0], RecordingNode::new().stalling_at(stall)),
            (nodes[1], RecordingNode::new()),
        ]);
        let (mut controller, _) = controller(&declaration, &planned, mocks);
        controller.validate().expect("a Created run validates");
        let before = if abandoned == Step::Cancel {
            &path[..2]
        } else {
            &path[..path
                .iter()
                .position(|s| *s == abandoned)
                .expect("on the path")]
        };
        for step in before {
            drive(&mut controller, *step).expect("the steps before the abandoned one answer");
        }
        let context = format!("{abandoned:?} abandoned at {stall:?}");
        assert!(
            !abandon(&mut controller, abandoned),
            "{context}: the stalled call holds the step"
        );
        assert_eq!(
            controller.run().state(),
            left,
            "{context}: where the abandoned step leaves the run, as `RunController` documents"
        );
        agree(&controller, nodes, &context);

        for step in path {
            let answer = catch_unwind(AssertUnwindSafe(|| drive(&mut controller, step)))
                .unwrap_or_else(|_| panic!("{context}: {step:?} panicked"));
            let context = format!("{context}, then {step:?} answered {answer:?}");
            agree(&controller, nodes, &context);
            if answer.is_ok() {
                for node_id in nodes {
                    assert_eq!(
                        controller.instance(node_id).map(|i| i.state()),
                        Some(reached(step)),
                        "{context}: an answered step leaves no instance behind"
                    );
                }
            }
        }

        let cancelled = drive(&mut controller, Step::Cancel);
        let context = format!("{context}, then Cancel answered {cancelled:?}");
        assert!(
            controller.run().state().is_terminal(),
            "{context}: the run ends"
        );
        for node_id in nodes {
            assert!(
                controller
                    .instance(node_id)
                    .is_none_or(|i| i.state().is_terminal()),
                "{context}: every instance ends"
            );
        }
    }
}

#[test]
fn an_activation_abandoned_mid_start_is_refused_again_and_calls_no_node() {
    let (declaration, planned, nodes) = planned();
    let [source, sink] = nodes;
    let mocks = HashMap::from([
        (
            source,
            RecordingNode::new().stalling_at(LifecycleStep::Start),
        ),
        (sink, RecordingNode::new()),
    ]);
    let (mut first, calls) = controller(&declaration, &planned, mocks);
    first.validate().expect("a Created run validates");
    block_on(first.prepare()).expect("both nodes prepare");
    assert!(!abandon(&mut first, Step::Activate));
    assert_eq!(
        block_on(first.activate()),
        Err(inference_runtime::RunError::EndpointsTaken(source)),
        "the source's endpoints went with the abandoned start"
    );
    assert_eq!(first.run().state(), RunState::Preparing);
    assert_eq!(
        calls[&sink].calls(),
        vec![NodeCall::Describe, NodeCall::Prepare],
        "the refused activation called no node"
    );
}

#[test]
fn a_completion_abandoned_mid_stop_resumes_to_completed() {
    let (declaration, planned, nodes) = planned();
    let [source, sink] = nodes;
    let mocks = HashMap::from([
        (
            source,
            RecordingNode::new().stalling_at(LifecycleStep::Stop),
        ),
        (sink, RecordingNode::new()),
    ]);
    let (mut second, calls) = controller(&declaration, &planned, mocks);
    second.validate().expect("a Created run validates");
    block_on(second.prepare()).expect("both nodes prepare");
    block_on(second.activate()).expect("both nodes start");
    block_on(second.drain()).expect("both nodes drain");
    assert!(!abandon(&mut second, Step::Complete));
    assert_eq!(second.run().state(), RunState::Draining);
    block_on(second.complete()).expect("a resumed completion finishes");
    assert_eq!(second.run().state(), RunState::Completed);
    for node_id in nodes {
        assert_eq!(
            second.instance(node_id).map(|i| i.state()),
            Some(NodeInstanceState::Stopped)
        );
    }
    assert_eq!(calls[&sink].calls().last(), Some(&NodeCall::Stop));
}
