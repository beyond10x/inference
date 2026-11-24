//! `inference.graph`: the seven declaration commands, `DeclareNodeDescriptor`, `ValidatePipeline`,
//! `PlanPipeline`, the eight Run commands, `DescribeNodeInstance` and the six node instance
//! commands, the ten views and the twenty-five events of `ess/domains/graph.yaml`, answered by
//! [`GraphDomain`].
//!
//! Each request field is read on its own ([`Input`]); the outcome, the error and the event come
//! from what the store returned. `DeclarePipeline`, `BindNode` and `PlanPipeline` read the
//! declared profiles and adapters through [`GraphCatalog`] over the `inference.policy` and
//! `inference.model` stores. A
//! command's consistency token is the target's ([`Stores::token`]), and a read demanding a token
//! the stores have not reached is an error ([`Stores::require`]). An observed event's position is
//! its place in the target's one publication order ([`Stores::graph_position`]).
//!
//! `ValidatePipeline` naming a pipeline no record carries takes its `wrong-state` outcome, whose
//! error then carries no `current`, since no record holds a state to report. Its `rejected`
//! outcome is declared `external`: the store takes it when a check finds something, and a scenario
//! may force it ([`configure_external_outcome`]) on the next invocation, which then answers with
//! the findings the checks make over that pipeline, none for a valid or unknown one, and moves
//! nothing. A forced outcome answers in every state and for an unknown pipeline, as the
//! specification orders it: neither command declares an `unknown_instance` outcome, and an
//! external refusal moves nothing, so `wrong_state` never answers before it (the ESS precedence
//! order, step 4 against step 6). The synthesized `ValidatePipeline/outcome/rejected` and the three
//! forced `PlanPipeline` scenarios send an identity no pipeline carries and expect the forced
//! outcome.
//!
//! `PlanPipeline` naming a pipeline no record carries takes `wrong-state` the same way. Its
//! outcomes `unbound`, `mismatch` and `locality-denied` are declared `external`: the store takes
//! each when planning finds it, and a scenario may force any of the three on the next
//! invocation, which then moves nothing and answers the forced error with every field its
//! declaration does not make `Optional`: what the planner finds for that same refusal over that
//! pipeline, and otherwise a stand-in read from the declaration ([`forced_plan`]).
//!
//! The eight Run commands and `DescribeNodeInstance` are the store's too ([`GraphDomain`]), as are
//! the views `Runs` and `NodeInstances` and their events. `CreateRun` builds the run from the
//! Planned form the store kept when it planned the pipeline. A moving run command naming a run no
//! record carries takes its `wrong-state` outcome, whose `RunStateConflict` then carries no
//! `current`, as `ValidatePipeline` does for a pipeline. `CancelRun`'s `cancelled` outcome also
//! cancels the run's Active generations and discards its unplayed segments in the
//! `inference.stream` store ([`StreamDomain::cancel_run`]) and publishes `RunCancelled` only.
//! `FailRun` reads its `failure` field by field: `detail` and an optional `node_id`, absent or
//! null for none; a `FailureReason` is written with `node_id` null when it names no node.
//!
//! The six node instance commands, `PrepareNodeInstance` through `FailNodeInstance`, move the
//! instance they name in the store and emit their own event only; one naming an instance no record
//! carries takes its `wrong-state` outcome with no `current`, as a run command does. A Run command
//! moves no instance here: which run transition drives which instance is the run controller's in
//! `inference-runtime`. The view `NodeInstances` carries each instance's `state`.
//!
//! `PrepareRun`'s `refused` outcome is declared `external`: the store never decides it (whether
//! the runtime can admit a run's resources is not observed here), and a scenario may force it on
//! the next invocation, which then moves nothing and answers `AdmissionDenied` with a stand-in
//! reason, the first of [`AdmissionRefusal::ALL`], in every state and for an unknown run, as for
//! the other forced outcomes.
//!
//! [`StreamDomain::cancel_run`]: inference_testkit::stream::StreamDomain::cancel_run

use std::collections::BTreeMap;

use ess_conformance::scenario::{CommandRef, ErrorRef, EventRef, OutcomeRef};
use ess_conformance::target::{
    DeclaredErrorValue, EventObservationRequest, ExternalOutcomeControl, ObservedEvent,
    SemanticCommandRequest, SemanticCommandResult, SemanticViewRequest, SemanticViewResult,
    TargetError, ViewRow,
};
use ess_primitives::node::Node;
use inference_core::model::{AdapterId, Capability, Locality};
use inference_core::policy::ProfileId;
use inference_pipeline::{
    AddNode, AddNodeError, AddPort, AddPortError, AddSubgraphNode, AddSubgraphNodeError, BindNode,
    BindNodeError, Binding, CapabilityMismatch, Catalog, ConnectPorts, ConnectPortsError,
    Declaration, DeclareFeedbackLoop, DeclareFeedbackLoopError, DeclareNodeDescriptor,
    DeclarePipeline, Edge, ExposePort, ExposePortError, Exposure, FeedbackLoop,
    FeedbackLoopDeclared, GraphFinding, LocalityDenied, NodeAdded, NodeBound, NodeDescriptor,
    NodeDescriptorDeclared, NodeId, NodeUnbound, PipelineDeclared, PipelineId, PipelineNotFound,
    PipelineNotReady, PipelinePlanned, PipelineValidated, PlanPipeline, PlanPipelineError, Port,
    PortAdded, PortDescriptor, PortExposed, PortsConnected, SubgraphInterfaceMismatch,
    SubgraphNodeAdded, ValidatePipeline, ValidatePipelineError,
};
use inference_runtime::RunId;
use inference_runtime::run::{
    AdmissionRefusal, FailureReason, NodeInstance, NodeInstanceDescribed, NodeInstanceId, Run,
    RunCreated, RunFailed, RunNested, RunStateConflict,
};
use inference_testkit::graph::{
    CreateRunError, DescribeNodeInstanceError, GraphCatalog, GraphDomain, GraphEvent, NestRunError,
    NodeInstanceMoveError, RunMoveError,
};
use inference_testkit::stream::StreamDomain;
use uuid::Uuid;

use super::{
    Input, Stores, boolean_node, integer_node, named_list_node, named_node, text_node, unreadable,
    unsupported,
};

/// The prefix of every name this module answers.
pub const DOMAIN: &str = "inference.graph";

const DECLARE_PIPELINE: &str = "inference.graph.DeclarePipeline";
const ADD_NODE: &str = "inference.graph.AddNode";
const ADD_SUBGRAPH_NODE: &str = "inference.graph.AddSubgraphNode";
const ADD_PORT: &str = "inference.graph.AddPort";
const CONNECT_PORTS: &str = "inference.graph.ConnectPorts";
const EXPOSE_PORT: &str = "inference.graph.ExposePort";
const DECLARE_NODE_DESCRIPTOR: &str = "inference.graph.DeclareNodeDescriptor";
const DECLARE_FEEDBACK_LOOP: &str = "inference.graph.DeclareFeedbackLoop";
const BIND_NODE: &str = "inference.graph.BindNode";
const VALIDATE_PIPELINE: &str = "inference.graph.ValidatePipeline";
const PLAN_PIPELINE: &str = "inference.graph.PlanPipeline";
const CREATE_RUN: &str = "inference.graph.CreateRun";
const NEST_RUN: &str = "inference.graph.NestRun";
const VALIDATE_RUN: &str = "inference.graph.ValidateRun";
const PREPARE_RUN: &str = "inference.graph.PrepareRun";
const ACTIVATE_RUN: &str = "inference.graph.ActivateRun";
const DRAIN_RUN: &str = "inference.graph.DrainRun";
const COMPLETE_RUN: &str = "inference.graph.CompleteRun";
const FAIL_RUN: &str = "inference.graph.FailRun";
const CANCEL_RUN: &str = "inference.graph.CancelRun";
const DESCRIBE_NODE_INSTANCE: &str = "inference.graph.DescribeNodeInstance";
const PREPARE_NODE_INSTANCE: &str = "inference.graph.PrepareNodeInstance";
const START_NODE_INSTANCE: &str = "inference.graph.StartNodeInstance";
const DRAIN_NODE_INSTANCE: &str = "inference.graph.DrainNodeInstance";
const CANCEL_NODE_INSTANCE: &str = "inference.graph.CancelNodeInstance";
const STOP_NODE_INSTANCE: &str = "inference.graph.StopNodeInstance";
const FAIL_NODE_INSTANCE: &str = "inference.graph.FailNodeInstance";

/// The outcome of `ValidatePipeline` that `ess/domains/graph.yaml` declares external.
const REJECTED: &str = "rejected";
/// The outcome of `PrepareRun` that `ess/domains/graph.yaml` declares external.
const REFUSED: &str = "refused";
/// The outcomes of `PlanPipeline` that `ess/domains/graph.yaml` declares external.
const UNBOUND: &str = "unbound";
const MISMATCH: &str = "mismatch";
const INTERFACE_MISMATCH: &str = "interface-mismatch";
const LOCALITY_DENIED: &str = "locality-denied";

const PIPELINE_DECLARED: &str = "inference.graph.PipelineDeclared";
const NODE_ADDED: &str = "inference.graph.NodeAdded";
const SUBGRAPH_NODE_ADDED: &str = "inference.graph.SubgraphNodeAdded";
const PORT_ADDED: &str = "inference.graph.PortAdded";
const PORTS_CONNECTED: &str = "inference.graph.PortsConnected";
const PORT_EXPOSED: &str = "inference.graph.PortExposed";
const NODE_DESCRIPTOR_DECLARED: &str = "inference.graph.NodeDescriptorDeclared";
const FEEDBACK_LOOP_DECLARED: &str = "inference.graph.FeedbackLoopDeclared";
const NODE_BOUND: &str = "inference.graph.NodeBound";
const PIPELINE_VALIDATED: &str = "inference.graph.PipelineValidated";
const PIPELINE_PLANNED: &str = "inference.graph.PipelinePlanned";
const RUN_CREATED: &str = "inference.graph.RunCreated";
const RUN_NESTED: &str = "inference.graph.RunNested";
const RUN_VALIDATING: &str = "inference.graph.RunValidating";
const RUN_PREPARING: &str = "inference.graph.RunPreparing";
const RUN_ACTIVATED: &str = "inference.graph.RunActivated";
const RUN_DRAINING: &str = "inference.graph.RunDraining";
const RUN_COMPLETED: &str = "inference.graph.RunCompleted";
const RUN_FAILED: &str = "inference.graph.RunFailed";
const RUN_CANCELLED: &str = "inference.graph.RunCancelled";
const NODE_INSTANCE_DESCRIBED: &str = "inference.graph.NodeInstanceDescribed";
const NODE_INSTANCE_PREPARED: &str = "inference.graph.NodeInstancePrepared";
const NODE_INSTANCE_STARTED: &str = "inference.graph.NodeInstanceStarted";
const NODE_INSTANCE_DRAINING: &str = "inference.graph.NodeInstanceDraining";
const NODE_INSTANCE_CANCELLING: &str = "inference.graph.NodeInstanceCancelling";
const NODE_INSTANCE_STOPPED: &str = "inference.graph.NodeInstanceStopped";
const NODE_INSTANCE_FAILED: &str = "inference.graph.NodeInstanceFailed";

/// Every event this module observes.
const EVENTS: [&str; 27] = [
    PIPELINE_DECLARED,
    NODE_ADDED,
    SUBGRAPH_NODE_ADDED,
    PORT_ADDED,
    PORTS_CONNECTED,
    PORT_EXPOSED,
    NODE_DESCRIPTOR_DECLARED,
    FEEDBACK_LOOP_DECLARED,
    NODE_BOUND,
    PIPELINE_VALIDATED,
    PIPELINE_PLANNED,
    RUN_CREATED,
    RUN_NESTED,
    RUN_VALIDATING,
    RUN_PREPARING,
    RUN_ACTIVATED,
    RUN_DRAINING,
    RUN_COMPLETED,
    RUN_FAILED,
    RUN_CANCELLED,
    NODE_INSTANCE_DESCRIBED,
    NODE_INSTANCE_PREPARED,
    NODE_INSTANCE_STARTED,
    NODE_INSTANCE_DRAINING,
    NODE_INSTANCE_CANCELLING,
    NODE_INSTANCE_STOPPED,
    NODE_INSTANCE_FAILED,
];

const PROFILE_NOT_FOUND: &str = "inference.policy.ProfileNotFound";
const PIPELINE_NOT_FOUND: &str = "inference.graph.PipelineNotFound";
const PIPELINE_NOT_READY: &str = "inference.graph.PipelineNotReady";
const NODE_NOT_FOUND: &str = "inference.graph.NodeNotFound";
const PORT_NOT_FOUND: &str = "inference.graph.PortNotFound";
const UNBOUNDED_EDGE: &str = "inference.graph.UnboundedEdge";
const EDGE_NOT_FOUND: &str = "inference.graph.EdgeNotFound";
const UNBOUNDED_FEEDBACK: &str = "inference.graph.UnboundedFeedback";
const INCOMPATIBLE_EDGE: &str = "inference.graph.IncompatibleEdge";
const ADAPTER_NOT_FOUND: &str = "inference.graph.AdapterNotFound";
const NODE_DESCRIPTOR_EXISTS: &str = "inference.graph.NodeDescriptorExists";
const INCOMPATIBLE_GRAPH: &str = "inference.graph.IncompatibleGraph";
const NODE_UNBOUND: &str = "inference.graph.NodeUnbound";
const CAPABILITY_MISMATCH: &str = "inference.graph.CapabilityMismatch";
const SUBGRAPH_INTERFACE_MISMATCH: &str = "inference.graph.SubgraphInterfaceMismatch";
const LOCALITY_DENIED_ERROR: &str = "inference.graph.LocalityDenied";
const PIPELINE_STATE_CONFLICT: &str = "inference.graph.PipelineStateConflict";
const RUN_STATE_CONFLICT: &str = "inference.graph.RunStateConflict";
const NODE_INSTANCE_STATE_CONFLICT: &str = "inference.graph.NodeInstanceStateConflict";
const RUN_NOT_FOUND: &str = "inference.graph.RunNotFound";
const NODE_INSTANCE_NOT_FOUND: &str = "inference.graph.NodeInstanceNotFound";
const RUN_NOT_RUNNING: &str = "inference.graph.RunNotRunning";
const ADMISSION_DENIED: &str = "inference.graph.AdmissionDenied";

const PIPELINES: &str = "inference.graph.Pipelines";
const NODES: &str = "inference.graph.Nodes";
const PORTS: &str = "inference.graph.Ports";
const EDGES: &str = "inference.graph.Edges";
const EXPOSURES: &str = "inference.graph.Exposures";
const NODE_DESCRIPTORS: &str = "inference.graph.NodeDescriptors";
const FEEDBACK_LOOPS: &str = "inference.graph.FeedbackLoops";
const BINDINGS: &str = "inference.graph.Bindings";
const RUNS: &str = "inference.graph.Runs";
const NODE_INSTANCES: &str = "inference.graph.NodeInstances";

/// The `inference.graph` store of one scenario.
pub type State = GraphDomain;

pub fn execute_command(
    stores: &mut Stores,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    stores.publish();
    let name = request.command.to_string();
    let forced = stores.forced(&request.command);
    let catalog = GraphCatalog {
        policy: &stores.policy,
        model: &stores.model,
    };
    let domain = &mut stores.graph;
    let result = match name.as_str() {
        DECLARE_PIPELINE => declare_pipeline(domain, &catalog, request)?,
        ADD_NODE => add_node(domain, request)?,
        ADD_SUBGRAPH_NODE => add_subgraph_node(domain, request)?,
        ADD_PORT => add_port(domain, request)?,
        CONNECT_PORTS => connect_ports(domain, request)?,
        EXPOSE_PORT => expose_port(domain, request)?,
        DECLARE_NODE_DESCRIPTOR => declare_node_descriptor(domain, request)?,
        DECLARE_FEEDBACK_LOOP => declare_feedback_loop(domain, request)?,
        BIND_NODE => bind_node(domain, &catalog, request)?,
        VALIDATE_PIPELINE => validate_pipeline(domain, forced.is_some(), request)?,
        PLAN_PIPELINE => plan_pipeline(domain, &catalog, forced.as_ref(), request)?,
        CREATE_RUN => create_run(domain, request)?,
        NEST_RUN => nest_run(domain, request)?,
        VALIDATE_RUN => {
            let answer = domain.validate_run(run_id(request)?);
            run_moved(
                VALIDATE_RUN,
                "validating",
                answer.map(GraphEvent::RunValidating),
                request,
            )?
        }
        PREPARE_RUN => {
            let run_id = run_id(request)?;
            if forced.is_some() {
                refused()?
            } else {
                let answer = domain.prepare_run(run_id);
                run_moved(
                    PREPARE_RUN,
                    "preparing",
                    answer.map(GraphEvent::RunPreparing),
                    request,
                )?
            }
        }
        ACTIVATE_RUN => {
            let answer = domain.activate_run(run_id(request)?);
            run_moved(
                ACTIVATE_RUN,
                "active",
                answer.map(GraphEvent::RunActivated),
                request,
            )?
        }
        DRAIN_RUN => {
            let answer = domain.drain_run(run_id(request)?);
            run_moved(
                DRAIN_RUN,
                "draining",
                answer.map(GraphEvent::RunDraining),
                request,
            )?
        }
        COMPLETE_RUN => {
            let answer = domain.complete_run(run_id(request)?);
            run_moved(
                COMPLETE_RUN,
                "completed",
                answer.map(GraphEvent::RunCompleted),
                request,
            )?
        }
        FAIL_RUN => fail_run(domain, request)?,
        CANCEL_RUN => cancel_run(domain, &mut stores.stream, request)?,
        DESCRIBE_NODE_INSTANCE => describe_node_instance(domain, request)?,
        PREPARE_NODE_INSTANCE => {
            let answer = domain.prepare_node_instance(instance_id(request)?);
            instance_moved(
                PREPARE_NODE_INSTANCE,
                "prepared",
                answer.map(GraphEvent::NodeInstancePrepared),
                request,
            )?
        }
        START_NODE_INSTANCE => {
            let answer = domain.start_node_instance(instance_id(request)?);
            instance_moved(
                START_NODE_INSTANCE,
                "started",
                answer.map(GraphEvent::NodeInstanceStarted),
                request,
            )?
        }
        DRAIN_NODE_INSTANCE => {
            let answer = domain.drain_node_instance(instance_id(request)?);
            instance_moved(
                DRAIN_NODE_INSTANCE,
                "draining",
                answer.map(GraphEvent::NodeInstanceDraining),
                request,
            )?
        }
        CANCEL_NODE_INSTANCE => {
            let answer = domain.cancel_node_instance(instance_id(request)?);
            instance_moved(
                CANCEL_NODE_INSTANCE,
                "cancelling",
                answer.map(GraphEvent::NodeInstanceCancelling),
                request,
            )?
        }
        STOP_NODE_INSTANCE => {
            let answer = domain.stop_node_instance(instance_id(request)?);
            instance_moved(
                STOP_NODE_INSTANCE,
                "stopped",
                answer.map(GraphEvent::NodeInstanceStopped),
                request,
            )?
        }
        FAIL_NODE_INSTANCE => fail_node_instance(domain, request)?,
        _ => return Err(unsupported(&name)),
    };
    stores.publish();
    Ok(result.with_consistency(stores.token()?))
}

/// The one field of a moving run command but `FailRun`.
fn run_id(request: &SemanticCommandRequest) -> Result<RunId, TargetError> {
    let mut input = Input::of(request);
    let run_id = input.named("run_id")?;
    input.finish()?;
    Ok(run_id)
}

/// The answer of a moving run command: `branch` and its event, or `wrong-state`, whose
/// `RunStateConflict` carries no `current` for a run no record carries.
fn run_moved(
    command: &str,
    branch: &str,
    answer: Result<GraphEvent, RunMoveError>,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    Ok(match answer {
        Ok(event) => SemanticCommandResult::took(outcome(command, branch))
            .emitting(graph_event(&event)?.in_activity(request.correlation.clone())),
        Err(RunMoveError::Unknown(_)) => {
            SemanticCommandResult::took(outcome(command, "wrong-state")).with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(RUN_STATE_CONFLICT)),
            )
        }
        Err(RunMoveError::Conflict(conflict)) => {
            SemanticCommandResult::took(outcome(command, "wrong-state"))
                .with_error(run_conflict(conflict)?)
        }
    })
}

/// `RunStateConflict`, carrying the state the run holds.
fn run_conflict(conflict: RunStateConflict) -> Result<DeclaredErrorValue, TargetError> {
    Ok(
        DeclaredErrorValue::new(parse::<ErrorRef>(RUN_STATE_CONFLICT))
            .with("current", named_node(&conflict.current)?),
    )
}

/// The forced `refused` answer of `PrepareRun`: `AdmissionDenied` with the stand-in reason.
fn refused() -> Result<SemanticCommandResult, TargetError> {
    Ok(
        SemanticCommandResult::took(outcome(PREPARE_RUN, REFUSED)).with_error(
            DeclaredErrorValue::new(parse::<ErrorRef>(ADMISSION_DENIED))
                .with("reason", named_node(&AdmissionRefusal::ALL[0])?),
        ),
    )
}

fn create_run(
    domain: &mut GraphDomain,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let pipeline_id = input.named("pipeline_id")?;
    input.finish()?;
    Ok(match domain.create_run(pipeline_id) {
        Ok(event) => SemanticCommandResult::took(outcome(CREATE_RUN, "created"))
            .emitting(run_created(&event)?.in_activity(request.correlation.clone())),
        Err(CreateRunError::UnknownPipeline(error)) => {
            SemanticCommandResult::took(outcome(CREATE_RUN, "unknown-pipeline"))
                .with_error(pipeline_not_found(error)?)
        }
        Err(CreateRunError::NotPlanned(error)) => {
            SemanticCommandResult::took(outcome(CREATE_RUN, "not-planned"))
                .with_error(pipeline_not_ready(error)?)
        }
    })
}

/// `NestRun` naming a run no record carries has no declared outcome, so it is an error here.
fn nest_run(
    domain: &mut GraphDomain,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let run_id = input.named("run_id")?;
    let parent = input.named("parent")?;
    input.finish()?;
    let took = |branch| SemanticCommandResult::took(outcome(NEST_RUN, branch));
    Ok(match domain.nest_run(run_id, parent) {
        Ok(event) => {
            took("nested").emitting(run_nested(&event)?.in_activity(request.correlation.clone()))
        }
        Err(NestRunError::UnknownParent(error)) => took("unknown-parent").with_error(
            DeclaredErrorValue::new(parse::<ErrorRef>(NODE_INSTANCE_NOT_FOUND))
                .with("instance_id", named_node(&error.instance_id)?),
        ),
        Err(NestRunError::UnknownRun(error)) => {
            return Err(unreadable(
                format!("answering `{NEST_RUN}`"),
                format!("{error}, and the specification declares no outcome for it"),
            ));
        }
    })
}

fn run_nested(event: &RunNested) -> Result<ObservedEvent, TargetError> {
    Ok(ObservedEvent::new(parse::<EventRef>(RUN_NESTED))
        .with("run_id", named_node(&event.run_id)?)
        .with("parent", named_node(&event.parent)?))
}

fn fail_run(
    domain: &mut GraphDomain,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let run_id = input.named("run_id")?;
    let failure = failure_reason(input.node("failure")?)?;
    input.finish()?;
    let answer = domain.fail_run(run_id, failure);
    run_moved(
        FAIL_RUN,
        "failed",
        answer.map(GraphEvent::RunFailed),
        request,
    )
}

/// `CancelRun`: the run moves to Cancelled in the graph store, and then its Active generations
/// and unplayed segments in the stream store; `RunCancelled` is the one event.
fn cancel_run(
    domain: &mut GraphDomain,
    stream: &mut StreamDomain,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let run_id = run_id(request)?;
    let answer = domain.cancel_run(run_id);
    if answer.is_ok() {
        stream.cancel_run(run_id);
    }
    run_moved(
        CANCEL_RUN,
        "cancelled",
        answer.map(GraphEvent::RunCancelled),
        request,
    )
}

fn describe_node_instance(
    domain: &mut GraphDomain,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let run_id = input.named("run_id")?;
    let node_id = input.named("node_id")?;
    input.finish()?;
    Ok(match domain.describe_node_instance(run_id, node_id) {
        Ok(event) => SemanticCommandResult::took(outcome(DESCRIBE_NODE_INSTANCE, "described"))
            .emitting(node_instance_described(&event)?.in_activity(request.correlation.clone())),
        Err(DescribeNodeInstanceError::UnknownRun(error)) => {
            SemanticCommandResult::took(outcome(DESCRIBE_NODE_INSTANCE, "unknown-run")).with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(RUN_NOT_FOUND))
                    .with("run_id", named_node(&error.run_id)?),
            )
        }
        Err(DescribeNodeInstanceError::UnknownNode(error)) => {
            SemanticCommandResult::took(outcome(DESCRIBE_NODE_INSTANCE, "unknown-node")).with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(NODE_NOT_FOUND))
                    .with("node_id", named_node(&error.node_id)?),
            )
        }
        Err(DescribeNodeInstanceError::RunNotRunning(error)) => {
            SemanticCommandResult::took(outcome(DESCRIBE_NODE_INSTANCE, "run-not-running"))
                .with_error(
                    DeclaredErrorValue::new(parse::<ErrorRef>(RUN_NOT_RUNNING))
                        .with("run_id", named_node(&error.run_id)?),
                )
        }
    })
}

/// The one field of a node instance command but `FailNodeInstance`.
fn instance_id(request: &SemanticCommandRequest) -> Result<NodeInstanceId, TargetError> {
    let mut input = Input::of(request);
    let instance_id = input.named("instance_id")?;
    input.finish()?;
    Ok(instance_id)
}

/// The answer of a node instance command: `branch` and its event, or `wrong-state`, whose
/// `NodeInstanceStateConflict` carries no `current` for an instance no record carries.
fn instance_moved(
    command: &str,
    branch: &str,
    answer: Result<GraphEvent, NodeInstanceMoveError>,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    Ok(match answer {
        Ok(event) => SemanticCommandResult::took(outcome(command, branch))
            .emitting(graph_event(&event)?.in_activity(request.correlation.clone())),
        Err(NodeInstanceMoveError::Unknown(_)) => {
            SemanticCommandResult::took(outcome(command, "wrong-state")).with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(NODE_INSTANCE_STATE_CONFLICT)),
            )
        }
        Err(NodeInstanceMoveError::Conflict(conflict)) => {
            SemanticCommandResult::took(outcome(command, "wrong-state")).with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(NODE_INSTANCE_STATE_CONFLICT))
                    .with("current", named_node(&conflict.current)?),
            )
        }
    })
}

/// `FailNodeInstance`: `instance_id` and `reason`, a string.
fn fail_node_instance(
    domain: &mut GraphDomain,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let instance_id = input.named("instance_id")?;
    let reason = input.text("reason")?;
    input.finish()?;
    let answer = domain.fail_node_instance(instance_id, reason);
    instance_moved(
        FAIL_NODE_INSTANCE,
        "failed",
        answer.map(GraphEvent::NodeInstanceFailed),
        request,
    )
}

/// An `inference.graph.FailureReason` read field by field: `detail`, a string, and `node_id`, a
/// node identity, absent or null for none. Any other field, or a field of another type, is an
/// error.
fn failure_reason(node: &Node) -> Result<FailureReason, TargetError> {
    let refuse = |detail: String| unreadable(format!("reading `{FAIL_RUN}`"), detail);
    let Node::Map(fields) = node else {
        return Err(refuse(format!("`failure` is not a record: {node:?}")));
    };
    let mut detail = None;
    let mut node_id = None;
    for (field, value) in fields {
        match (field.as_str(), value) {
            ("detail", Node::Text(text)) => detail = Some(text.clone()),
            ("node_id", Node::Null) => node_id = None,
            ("node_id", Node::Text(text)) => {
                node_id = Some(
                    serde_json::from_value::<NodeId>(serde_json::Value::String(text.clone()))
                        .map_err(|_| {
                            refuse(format!("`failure.node_id` is not a node id: {text}"))
                        })?,
                );
            }
            _ => {
                return Err(refuse(format!(
                    "`failure.{field}` is undeclared or not of its declared type: {value:?}"
                )));
            }
        }
    }
    let detail = detail.ok_or_else(|| refuse("`failure` carries no `detail`".to_owned()))?;
    Ok(FailureReason { node_id, detail })
}

/// The node of an `inference.graph.FailureReason`: `node_id` null when it names no node.
fn failure_node(failure: &FailureReason) -> Result<Node, TargetError> {
    Ok(Node::Map(BTreeMap::from([
        (
            "node_id".to_owned(),
            optional_node(failure.node_id.as_ref())?,
        ),
        ("detail".to_owned(), text_node(&failure.detail)),
    ])))
}

/// Accepts a force of `ValidatePipeline/rejected`, of `PlanPipeline/unbound`, `mismatch` or
/// `locality-denied`, or of `PrepareRun/refused`, the external outcomes of the commands this
/// module answers; refuses a force of another outcome of those three commands as an error, as no
/// specification-declared external outcome, and any other command's as unsupported.
pub fn configure_external_outcome(
    stores: &mut Stores,
    request: &ExternalOutcomeControl,
) -> Result<(), TargetError> {
    let command = request.force.command.to_string();
    let external: &[&str] = match command.as_str() {
        VALIDATE_PIPELINE => &[REJECTED],
        PLAN_PIPELINE => &[INTERFACE_MISMATCH, UNBOUND, MISMATCH, LOCALITY_DENIED],
        PREPARE_RUN => &[REFUSED],
        _ => return Err(unsupported(&request.force.to_string())),
    };
    if !external
        .iter()
        .any(|branch| request.force == outcome(&command, branch))
    {
        return Err(unreadable(
            format!("forcing `{}`", request.force),
            "it is not an outcome `ess/domains/graph.yaml` declares external",
        ));
    }
    stores.force(request.force.clone());
    Ok(())
}

fn declare_pipeline(
    domain: &mut GraphDomain,
    catalog: &GraphCatalog<'_>,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let declare = DeclarePipeline {
        name: input.text("name")?,
        schema_revision: input.text("schema_revision")?,
        mode: input.named("mode")?,
        profile_id: input.named("profile_id")?,
        required_capabilities: input.named_list("required_capabilities")?,
    };
    input.finish()?;
    Ok(match domain.declare_pipeline(declare, catalog) {
        Ok(event) => SemanticCommandResult::took(outcome(DECLARE_PIPELINE, "declared"))
            .emitting(pipeline_declared(&event)?.in_activity(request.correlation.clone())),
        Err(error) => SemanticCommandResult::took(outcome(DECLARE_PIPELINE, "unknown-profile"))
            .with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(PROFILE_NOT_FOUND))
                    .with("profile_id", named_node(&error.profile_id)?),
            ),
    })
}

fn add_node(
    domain: &mut GraphDomain,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let add = AddNode {
        pipeline_id: input.named("pipeline_id")?,
        name: input.text("name")?,
        role: input.named("role")?,
        requires: input.named_list("requires")?,
    };
    input.finish()?;
    Ok(match domain.add_node(add) {
        Ok(event) => SemanticCommandResult::took(outcome(ADD_NODE, "added"))
            .emitting(node_added(&event)?.in_activity(request.correlation.clone())),
        Err(AddNodeError::UnknownPipeline(error)) => {
            SemanticCommandResult::took(outcome(ADD_NODE, "unknown-pipeline"))
                .with_error(pipeline_not_found(error)?)
        }
        Err(AddNodeError::NotDeclared(error)) => {
            SemanticCommandResult::took(outcome(ADD_NODE, "not-declared"))
                .with_error(pipeline_not_ready(error)?)
        }
    })
}

fn add_subgraph_node(
    domain: &mut GraphDomain,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let add = AddSubgraphNode {
        pipeline_id: input.named("pipeline_id")?,
        name: input.text("name")?,
        role: input.named("role")?,
        subgraph_id: input.named("subgraph_id")?,
    };
    input.finish()?;
    let took = |branch| SemanticCommandResult::took(outcome(ADD_SUBGRAPH_NODE, branch));
    Ok(match domain.add_subgraph_node(add) {
        Ok(event) => took("added")
            .emitting(subgraph_node_added(&event)?.in_activity(request.correlation.clone())),
        Err(AddSubgraphNodeError::UnknownPipeline(error)) => {
            took("unknown-pipeline").with_error(pipeline_not_found(error)?)
        }
        Err(AddSubgraphNodeError::UnknownSubgraph(error)) => {
            took("unknown-subgraph").with_error(pipeline_not_found(error)?)
        }
        Err(AddSubgraphNodeError::NotDeclared(error)) => {
            took("not-declared").with_error(pipeline_not_ready(error)?)
        }
        Err(AddSubgraphNodeError::SubgraphNotPlanned(error)) => {
            took("subgraph-not-planned").with_error(pipeline_not_ready(error)?)
        }
    })
}

fn subgraph_node_added(event: &SubgraphNodeAdded) -> Result<ObservedEvent, TargetError> {
    Ok(ObservedEvent::new(parse::<EventRef>(SUBGRAPH_NODE_ADDED))
        .with("node_id", named_node(&event.node_id)?)
        .with("pipeline_id", named_node(&event.pipeline_id)?)
        .with("subgraph_id", named_node(&event.subgraph_id)?)
        .with("name", text_node(&event.name)))
}

fn add_port(
    domain: &mut GraphDomain,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let add = AddPort {
        pipeline_id: input.named("pipeline_id")?,
        node_id: input.named("node_id")?,
        name: input.text("name")?,
        direction: input.named("direction")?,
        modality: input.named("modality")?,
        essentiality: input.named("essentiality")?,
        delivery: input.named("delivery")?,
        incremental: input.boolean("incremental")?,
        end_marker_required: input.boolean("end_marker_required")?,
        provisional_allowed: input.boolean("provisional_allowed")?,
    };
    input.finish()?;
    Ok(match domain.add_port(add) {
        Ok(event) => SemanticCommandResult::took(outcome(ADD_PORT, "added"))
            .emitting(port_added(&event)?.in_activity(request.correlation.clone())),
        Err(AddPortError::UnknownPipeline(error)) => {
            SemanticCommandResult::took(outcome(ADD_PORT, "unknown-pipeline"))
                .with_error(pipeline_not_found(error)?)
        }
        Err(AddPortError::UnknownNode(error)) => {
            SemanticCommandResult::took(outcome(ADD_PORT, "unknown-node")).with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(NODE_NOT_FOUND))
                    .with("node_id", named_node(&error.node_id)?),
            )
        }
        Err(AddPortError::NotDeclared(error)) => {
            SemanticCommandResult::took(outcome(ADD_PORT, "not-declared"))
                .with_error(pipeline_not_ready(error)?)
        }
    })
}

fn connect_ports(
    domain: &mut GraphDomain,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let connect = ConnectPorts {
        pipeline_id: input.named("pipeline_id")?,
        from_port: input.named("from_port")?,
        to_port: input.named("to_port")?,
        capacity: input.integer("capacity")?,
        overload: input.named("overload")?,
    };
    input.finish()?;
    Ok(match domain.connect_ports(connect) {
        Ok(event) => SemanticCommandResult::took(outcome(CONNECT_PORTS, "connected"))
            .emitting(ports_connected(&event)?.in_activity(request.correlation.clone())),
        Err(ConnectPortsError::UnknownPipeline(error)) => {
            SemanticCommandResult::took(outcome(CONNECT_PORTS, "unknown-pipeline"))
                .with_error(pipeline_not_found(error)?)
        }
        Err(ConnectPortsError::UnknownSource(error)) => {
            SemanticCommandResult::took(outcome(CONNECT_PORTS, "unknown-source")).with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(PORT_NOT_FOUND))
                    .with("port_id", named_node(&error.port_id)?),
            )
        }
        Err(ConnectPortsError::Unbounded(error)) => {
            SemanticCommandResult::took(outcome(CONNECT_PORTS, "unbounded")).with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(UNBOUNDED_EDGE))
                    .with("submitted", integer_node(error.submitted)),
            )
        }
        Err(ConnectPortsError::NotDeclared(error)) => {
            SemanticCommandResult::took(outcome(CONNECT_PORTS, "not-declared"))
                .with_error(pipeline_not_ready(error)?)
        }
        Err(ConnectPortsError::SourceNotOutput(error)) => {
            SemanticCommandResult::took(outcome(CONNECT_PORTS, "source-not-output")).with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(INCOMPATIBLE_EDGE))
                    .with("from_port", named_node(&error.from_port)?)
                    .with("to_port", named_node(&error.to_port)?)
                    .with("defect", named_node(&error.defect)?),
            )
        }
    })
}

fn expose_port(
    domain: &mut GraphDomain,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let expose = ExposePort {
        pipeline_id: input.named("pipeline_id")?,
        port_id: input.named("port_id")?,
        name: input.text("name")?,
        capacity: input.integer("capacity")?,
        overload: input.named("overload")?,
    };
    input.finish()?;
    let took = |branch| SemanticCommandResult::took(outcome(EXPOSE_PORT, branch));
    Ok(match domain.expose_port(expose) {
        Ok(event) => {
            took("exposed").emitting(port_exposed(&event)?.in_activity(request.correlation.clone()))
        }
        Err(ExposePortError::UnknownPipeline(error)) => {
            took("unknown-pipeline").with_error(pipeline_not_found(error)?)
        }
        Err(ExposePortError::UnknownPort(error)) => took("unknown-port").with_error(
            DeclaredErrorValue::new(parse::<ErrorRef>(PORT_NOT_FOUND))
                .with("port_id", named_node(&error.port_id)?),
        ),
        Err(ExposePortError::Unbounded(error)) => took("unbounded").with_error(
            DeclaredErrorValue::new(parse::<ErrorRef>(UNBOUNDED_EDGE))
                .with("submitted", integer_node(error.submitted)),
        ),
        Err(ExposePortError::NotDeclared(error)) => {
            took("not-declared").with_error(pipeline_not_ready(error)?)
        }
    })
}

fn declare_node_descriptor(
    domain: &mut GraphDomain,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let kind = input.named("kind")?;
    let role = input.named("role")?;
    let requires = input.named_list("requires")?;
    let ports = port_descriptors(input.take("ports")?)?;
    input.finish()?;
    let declare = DeclareNodeDescriptor {
        kind,
        role,
        requires,
        ports,
    };
    let took = |branch| SemanticCommandResult::took(outcome(DECLARE_NODE_DESCRIPTOR, branch));
    Ok(match domain.declare_node_descriptor(declare) {
        Ok(event) => took("declared")
            .emitting(node_descriptor_declared(&event)?.in_activity(request.correlation.clone())),
        Err(error) => took("already-declared").with_error(
            DeclaredErrorValue::new(parse::<ErrorRef>(NODE_DESCRIPTOR_EXISTS))
                .with("kind", named_node(&error.kind)?),
        ),
    })
}

/// The eight fields of an `inference.graph.PortDescriptor`.
const PORT_DESCRIPTOR_FIELDS: [&str; 8] = [
    "name",
    "direction",
    "modality",
    "essentiality",
    "delivery",
    "incremental",
    "end_marker_required",
    "provisional_allowed",
];

/// An ESS `List<inference.graph.PortDescriptor>`: each record carries the eight declared fields
/// and no other, strings and booleans as declared.
fn port_descriptors(node: &Node) -> Result<Vec<PortDescriptor>, TargetError> {
    let refuse =
        |detail: String| unreadable(format!("reading `{DECLARE_NODE_DESCRIPTOR}`"), detail);
    let Node::Seq(items) = node else {
        return Err(refuse(format!("`ports` is not a list: {node:?}")));
    };
    items
        .iter()
        .map(|item| {
            let Node::Map(fields) = item else {
                return Err(refuse(format!(
                    "a port descriptor is not a record: {item:?}"
                )));
            };
            let mut record = serde_json::Map::new();
            for (field, value) in fields {
                if !PORT_DESCRIPTOR_FIELDS.contains(&field.as_str()) {
                    return Err(refuse(format!("`ports.{field}` is undeclared")));
                }
                let value = match value {
                    Node::Text(text) => serde_json::Value::String(text.clone()),
                    Node::Bool(flag) => serde_json::Value::Bool(*flag),
                    other => {
                        return Err(refuse(format!(
                            "`ports.{field}` is not of its declared type: {other:?}"
                        )));
                    }
                };
                record.insert(field.clone(), value);
            }
            serde_json::from_value(serde_json::Value::Object(record))
                .map_err(|error| refuse(format!("a port descriptor is not read: {error}")))
        })
        .collect()
}

/// The node of an ESS `List<inference.graph.PortDescriptor>`.
fn port_descriptors_node(ports: &[PortDescriptor]) -> Result<Node, TargetError> {
    ports
        .iter()
        .map(|port| {
            Ok(Node::Map(BTreeMap::from([
                ("name".to_owned(), text_node(&port.name)),
                ("direction".to_owned(), named_node(&port.direction)?),
                ("modality".to_owned(), named_node(&port.modality)?),
                ("essentiality".to_owned(), named_node(&port.essentiality)?),
                ("delivery".to_owned(), named_node(&port.delivery)?),
                ("incremental".to_owned(), boolean_node(port.incremental)),
                (
                    "end_marker_required".to_owned(),
                    boolean_node(port.end_marker_required),
                ),
                (
                    "provisional_allowed".to_owned(),
                    boolean_node(port.provisional_allowed),
                ),
            ])))
        })
        .collect::<Result<Vec<_>, TargetError>>()
        .map(Node::Seq)
}

fn declare_feedback_loop(
    domain: &mut GraphDomain,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let declare = DeclareFeedbackLoop {
        pipeline_id: input.named("pipeline_id")?,
        edge_id: input.named("edge_id")?,
        max_iterations: input.integer("max_iterations")?,
    };
    input.finish()?;
    let took = |branch| SemanticCommandResult::took(outcome(DECLARE_FEEDBACK_LOOP, branch));
    Ok(match domain.declare_feedback_loop(declare) {
        Ok(event) => took("declared")
            .emitting(feedback_loop_declared(&event)?.in_activity(request.correlation.clone())),
        Err(DeclareFeedbackLoopError::UnknownPipeline(error)) => {
            took("unknown-pipeline").with_error(pipeline_not_found(error)?)
        }
        Err(DeclareFeedbackLoopError::UnknownEdge(error)) => took("unknown-edge").with_error(
            DeclaredErrorValue::new(parse::<ErrorRef>(EDGE_NOT_FOUND))
                .with("edge_id", named_node(&error.edge_id)?),
        ),
        Err(DeclareFeedbackLoopError::Unbounded(error)) => took("unbounded").with_error(
            DeclaredErrorValue::new(parse::<ErrorRef>(UNBOUNDED_FEEDBACK))
                .with("submitted", integer_node(error.submitted)),
        ),
        Err(DeclareFeedbackLoopError::NotDeclared(error)) => {
            took("not-declared").with_error(pipeline_not_ready(error)?)
        }
    })
}

fn bind_node(
    domain: &mut GraphDomain,
    catalog: &GraphCatalog<'_>,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let bind = BindNode {
        pipeline_id: input.named("pipeline_id")?,
        node_id: input.named("node_id")?,
        adapter_id: input.named("adapter_id")?,
    };
    input.finish()?;
    Ok(match domain.bind_node(bind, catalog) {
        Ok(event) => SemanticCommandResult::took(outcome(BIND_NODE, "bound"))
            .emitting(node_bound(&event)?.in_activity(request.correlation.clone())),
        Err(BindNodeError::UnknownPipeline(error)) => {
            SemanticCommandResult::took(outcome(BIND_NODE, "unknown-pipeline"))
                .with_error(pipeline_not_found(error)?)
        }
        Err(BindNodeError::UnknownNode(error)) => {
            SemanticCommandResult::took(outcome(BIND_NODE, "unknown-node")).with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(NODE_NOT_FOUND))
                    .with("node_id", named_node(&error.node_id)?),
            )
        }
        Err(BindNodeError::UnknownAdapter(error)) => {
            SemanticCommandResult::took(outcome(BIND_NODE, "unknown-adapter")).with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(ADAPTER_NOT_FOUND))
                    .with("adapter_id", named_node(&error.adapter_id)?),
            )
        }
        Err(BindNodeError::NotDeclared(error)) => {
            SemanticCommandResult::took(outcome(BIND_NODE, "not-declared"))
                .with_error(pipeline_not_ready(error)?)
        }
    })
}

fn validate_pipeline(
    domain: &mut GraphDomain,
    forced: bool,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let validate = ValidatePipeline {
        pipeline_id: input.named("pipeline_id")?,
    };
    input.finish()?;
    if forced {
        return rejected(&domain.declaration().findings(validate.pipeline_id));
    }
    Ok(match domain.validate_pipeline(validate) {
        Ok(event) => SemanticCommandResult::took(outcome(VALIDATE_PIPELINE, "validated"))
            .emitting(pipeline_validated(&event)?.in_activity(request.correlation.clone())),
        Err(ValidatePipelineError::UnknownPipeline(_)) => {
            SemanticCommandResult::took(outcome(VALIDATE_PIPELINE, "wrong-state")).with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(PIPELINE_STATE_CONFLICT)),
            )
        }
        Err(ValidatePipelineError::WrongState(conflict)) => {
            SemanticCommandResult::took(outcome(VALIDATE_PIPELINE, "wrong-state")).with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(PIPELINE_STATE_CONFLICT))
                    .with("current", named_node(&conflict.current)?),
            )
        }
        Err(ValidatePipelineError::Rejected(error)) => rejected(&error.findings)?,
    })
}

/// The `rejected` answer of `ValidatePipeline`: `IncompatibleGraph` carrying each finding.
fn rejected(findings: &[GraphFinding]) -> Result<SemanticCommandResult, TargetError> {
    let findings = findings
        .iter()
        .map(|finding| {
            Ok(Node::Map(BTreeMap::from([
                ("defect".to_owned(), named_node(&finding.defect)?),
                (
                    "node_id".to_owned(),
                    optional_node(finding.node_id.as_ref())?,
                ),
                (
                    "port_id".to_owned(),
                    optional_node(finding.port_id.as_ref())?,
                ),
            ])))
        })
        .collect::<Result<Vec<_>, TargetError>>()?;
    Ok(
        SemanticCommandResult::took(outcome(VALIDATE_PIPELINE, REJECTED)).with_error(
            DeclaredErrorValue::new(parse::<ErrorRef>(INCOMPATIBLE_GRAPH))
                .with("findings", Node::Seq(findings)),
        ),
    )
}

fn plan_pipeline(
    domain: &mut GraphDomain,
    catalog: &GraphCatalog<'_>,
    forced: Option<&OutcomeRef>,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let plan = PlanPipeline {
        pipeline_id: input.named("pipeline_id")?,
    };
    input.finish()?;
    if let Some(forced) = forced {
        return forced_plan(forced, domain.declaration(), catalog, plan.pipeline_id);
    }
    Ok(match domain.plan_pipeline(plan, catalog) {
        Ok(event) => SemanticCommandResult::took(outcome(PLAN_PIPELINE, "planned"))
            .emitting(pipeline_planned(&event)?.in_activity(request.correlation.clone())),
        Err(PlanPipelineError::UnknownPipeline(_)) => {
            SemanticCommandResult::took(outcome(PLAN_PIPELINE, "wrong-state")).with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(PIPELINE_STATE_CONFLICT)),
            )
        }
        Err(PlanPipelineError::WrongState(conflict)) => {
            SemanticCommandResult::took(outcome(PLAN_PIPELINE, "wrong-state")).with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(PIPELINE_STATE_CONFLICT))
                    .with("current", named_node(&conflict.current)?),
            )
        }
        Err(PlanPipelineError::InterfaceMismatch(error)) => {
            SemanticCommandResult::took(outcome(PLAN_PIPELINE, INTERFACE_MISMATCH))
                .with_error(interface_mismatch(error)?)
        }
        Err(PlanPipelineError::Unbound(error)) => {
            SemanticCommandResult::took(outcome(PLAN_PIPELINE, UNBOUND))
                .with_error(node_unbound(error)?)
        }
        Err(PlanPipelineError::Mismatch(error)) => {
            SemanticCommandResult::took(outcome(PLAN_PIPELINE, MISMATCH))
                .with_error(capability_mismatch(error)?)
        }
        Err(PlanPipelineError::LocalityDenied(error)) => {
            SemanticCommandResult::took(outcome(PLAN_PIPELINE, LOCALITY_DENIED))
                .with_error(locality_denied(error)?)
        }
    })
}

/// The answer of a forced `PlanPipeline` outcome: its error, carrying every field its declaration
/// does not make `Optional`. Each is what the planner decides over that pipeline when it decides
/// that same refusal; otherwise a stand-in read from the declaration, so that two runs answer
/// alike: the pipeline's first declared node, the first capability it requires (else the first of
/// [`Capability::ALL`]), and for `locality-denied` its first binding's node, adapter and that
/// adapter's locality (else the first of [`Locality::ALL`]) with the pipeline's profile. Where no
/// record names a node, an adapter or a profile, the nil identity stands in.
fn forced_plan(
    forced: &OutcomeRef,
    declaration: &Declaration,
    catalog: &GraphCatalog<'_>,
    pipeline_id: PipelineId,
) -> Result<SemanticCommandResult, TargetError> {
    let decided = declaration.plan_of(pipeline_id, catalog);
    let pipeline = declaration.pipeline(pipeline_id);
    let first_node = declaration
        .nodes()
        .find(|node| node.pipeline_id() == pipeline_id)
        .map_or_else(|| NodeId::from(Uuid::nil()), |node| node.node_id());
    let error = if *forced == outcome(PLAN_PIPELINE, INTERFACE_MISMATCH) {
        interface_mismatch(match decided {
            Err(PlanPipelineError::InterfaceMismatch(error)) => error,
            _ => SubgraphInterfaceMismatch {
                node_id: declaration
                    .nodes()
                    .find(|node| node.pipeline_id() == pipeline_id && node.subgraph_id().is_some())
                    .map_or(first_node, |node| node.node_id()),
                port_id: None,
                exposure_id: None,
            },
        })?
    } else if *forced == outcome(PLAN_PIPELINE, UNBOUND) {
        node_unbound(match decided {
            Err(PlanPipelineError::Unbound(error)) => error,
            _ => NodeUnbound {
                node_id: first_node,
            },
        })?
    } else if *forced == outcome(PLAN_PIPELINE, MISMATCH) {
        capability_mismatch(match decided {
            Err(PlanPipelineError::Mismatch(error)) => error,
            _ => CapabilityMismatch {
                node_id: None,
                capability: pipeline
                    .and_then(|pipeline| pipeline.required_capabilities().first().copied())
                    .unwrap_or(Capability::ALL[0]),
            },
        })?
    } else if let Err(PlanPipelineError::LocalityDenied(error)) = decided {
        locality_denied(error)?
    } else {
        let binding = declaration
            .bindings()
            .find(|binding| binding.pipeline_id() == pipeline_id);
        let adapter_id = binding.map_or_else(
            || AdapterId::from(Uuid::nil()),
            |binding| binding.adapter_id(),
        );
        locality_denied(LocalityDenied {
            node_id: binding.map_or(first_node, |binding| binding.node_id()),
            adapter_id,
            locality: catalog
                .adapter(adapter_id)
                .map_or(Locality::ALL[0], |adapter| adapter.locality()),
            profile_id: pipeline.map_or_else(
                || ProfileId::from(Uuid::nil()),
                |pipeline| pipeline.profile_id(),
            ),
        })?
    };
    Ok(SemanticCommandResult::took(forced.clone()).with_error(error))
}

/// `SubgraphInterfaceMismatch`, naming the node and the port or exposure, null where none.
fn interface_mismatch(error: SubgraphInterfaceMismatch) -> Result<DeclaredErrorValue, TargetError> {
    Ok(
        DeclaredErrorValue::new(parse::<ErrorRef>(SUBGRAPH_INTERFACE_MISMATCH))
            .with("node_id", named_node(&error.node_id)?)
            .with("port_id", optional_node(error.port_id.as_ref())?)
            .with("exposure_id", optional_node(error.exposure_id.as_ref())?),
    )
}

/// `NodeUnbound`, naming the node.
fn node_unbound(error: NodeUnbound) -> Result<DeclaredErrorValue, TargetError> {
    Ok(DeclaredErrorValue::new(parse::<ErrorRef>(NODE_UNBOUND))
        .with("node_id", named_node(&error.node_id)?))
}

/// `LocalityDenied`, naming the node, its adapter, the adapter's locality and the profile.
fn locality_denied(error: LocalityDenied) -> Result<DeclaredErrorValue, TargetError> {
    Ok(
        DeclaredErrorValue::new(parse::<ErrorRef>(LOCALITY_DENIED_ERROR))
            .with("node_id", named_node(&error.node_id)?)
            .with("adapter_id", named_node(&error.adapter_id)?)
            .with("locality", named_node(&error.locality)?)
            .with("profile_id", named_node(&error.profile_id)?),
    )
}

/// `CapabilityMismatch`, naming the node (null for the pipeline as a whole) and the capability.
fn capability_mismatch(error: CapabilityMismatch) -> Result<DeclaredErrorValue, TargetError> {
    Ok(
        DeclaredErrorValue::new(parse::<ErrorRef>(CAPABILITY_MISMATCH))
            .with("node_id", optional_node(error.node_id.as_ref())?)
            .with("capability", named_node(&error.capability)?),
    )
}

/// The node of an ESS `Optional` of a value [`named_node`] writes: null when absent.
fn optional_node<T: serde::Serialize>(value: Option<&T>) -> Result<Node, TargetError> {
    value.map_or(Ok(Node::Null), named_node)
}

fn pipeline_not_found(error: PipelineNotFound) -> Result<DeclaredErrorValue, TargetError> {
    Ok(
        DeclaredErrorValue::new(parse::<ErrorRef>(PIPELINE_NOT_FOUND))
            .with("pipeline_id", named_node(&error.pipeline_id)?),
    )
}

fn pipeline_not_ready(error: PipelineNotReady) -> Result<DeclaredErrorValue, TargetError> {
    Ok(
        DeclaredErrorValue::new(parse::<ErrorRef>(PIPELINE_NOT_READY))
            .with("pipeline_id", named_node(&error.pipeline_id)?)
            .with("required", named_node(&error.required)?),
    )
}

pub fn query_view(
    stores: &mut Stores,
    request: &SemanticViewRequest,
) -> Result<SemanticViewResult, TargetError> {
    let name = request.view.to_string();
    stores.require(&name, &request.consistency)?;
    let declaration = stores.graph.declaration();
    let rows = match name.as_str() {
        PIPELINES => declaration
            .pipelines()
            .map(|pipeline| {
                Ok(BTreeMap::from([
                    (
                        "pipeline_id".to_owned(),
                        named_node(&pipeline.pipeline_id())?,
                    ),
                    ("name".to_owned(), text_node(pipeline.name())),
                    (
                        "schema_revision".to_owned(),
                        text_node(pipeline.schema_revision()),
                    ),
                    ("mode".to_owned(), named_node(&pipeline.mode())?),
                    ("profile_id".to_owned(), named_node(&pipeline.profile_id())?),
                    (
                        "required_capabilities".to_owned(),
                        named_list_node(pipeline.required_capabilities())?,
                    ),
                    (
                        "effective_capabilities".to_owned(),
                        optional_list(pipeline.effective_capabilities())?,
                    ),
                    (
                        "downgrades".to_owned(),
                        optional_list(pipeline.downgrades())?,
                    ),
                    ("state".to_owned(), named_node(&pipeline.state())?),
                ]))
            })
            .collect::<Result<Vec<ViewRow>, TargetError>>()?,
        NODES => declaration
            .nodes()
            .map(|node| {
                Ok(BTreeMap::from([
                    ("node_id".to_owned(), named_node(&node.node_id())?),
                    ("pipeline_id".to_owned(), named_node(&node.pipeline_id())?),
                    ("name".to_owned(), text_node(node.name())),
                    ("role".to_owned(), named_node(&node.role())?),
                    ("requires".to_owned(), named_list_node(node.requires())?),
                ]))
            })
            .collect::<Result<Vec<ViewRow>, TargetError>>()?,
        PORTS => declaration
            .ports()
            .map(ports_row)
            .collect::<Result<Vec<_>, _>>()?,
        EDGES => declaration
            .edges()
            .map(edges_row)
            .collect::<Result<Vec<_>, _>>()?,
        EXPOSURES => declaration
            .exposures()
            .map(exposures_row)
            .collect::<Result<Vec<_>, _>>()?,
        NODE_DESCRIPTORS => stores
            .graph
            .descriptors()
            .iter()
            .map(node_descriptors_row)
            .collect::<Result<Vec<_>, _>>()?,
        FEEDBACK_LOOPS => declaration
            .feedback_loops()
            .map(feedback_loops_row)
            .collect::<Result<Vec<_>, _>>()?,
        BINDINGS => declaration
            .bindings()
            .map(bindings_row)
            .collect::<Result<Vec<_>, _>>()?,
        RUNS => stores
            .graph
            .runs()
            .map(runs_row)
            .collect::<Result<Vec<_>, _>>()?,
        NODE_INSTANCES => stores
            .graph
            .node_instances()
            .map(node_instances_row)
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err(unsupported(&name)),
    };
    Ok(SemanticViewResult::of(rows))
}

pub fn observe_events(
    stores: &mut Stores,
    request: &EventObservationRequest,
) -> Result<Vec<ObservedEvent>, TargetError> {
    let wanted = request.event.to_string();
    if !EVENTS.contains(&wanted.as_str()) {
        return Err(unsupported(&wanted));
    }
    stores.publish();
    let mut observed = Vec::new();
    for (index, event) in stores.graph.events().iter().enumerate() {
        let event = graph_event(event)?;
        if event.event == request.event {
            let position = stores.graph_position(index)?;
            observed.push(event.in_activity(request.correlation.clone()).at(position));
        }
    }
    Ok(observed)
}

/// An event of the store as the suite observes it, every declared field carried.
fn graph_event(event: &GraphEvent) -> Result<ObservedEvent, TargetError> {
    let run = |name: &str, run_id: RunId| {
        Ok::<_, TargetError>(
            ObservedEvent::new(parse::<EventRef>(name)).with("run_id", named_node(&run_id)?),
        )
    };
    let instance = |name: &str, instance_id: NodeInstanceId| {
        Ok::<_, TargetError>(
            ObservedEvent::new(parse::<EventRef>(name))
                .with("instance_id", named_node(&instance_id)?),
        )
    };
    match event {
        GraphEvent::PipelineDeclared(event) => pipeline_declared(event),
        GraphEvent::NodeAdded(event) => node_added(event),
        GraphEvent::PortAdded(event) => port_added(event),
        GraphEvent::PortsConnected(event) => ports_connected(event),
        GraphEvent::PortExposed(event) => port_exposed(event),
        GraphEvent::NodeDescriptorDeclared(event) => node_descriptor_declared(event),
        GraphEvent::NodeBound(event) => node_bound(event),
        GraphEvent::RunNested(event) => run_nested(event),
        GraphEvent::SubgraphNodeAdded(event) => subgraph_node_added(event),
        GraphEvent::FeedbackLoopDeclared(event) => feedback_loop_declared(event),
        GraphEvent::PipelineValidated(event) => pipeline_validated(event),
        GraphEvent::PipelinePlanned(event) => pipeline_planned(event),
        GraphEvent::RunCreated(event) => run_created(event),
        GraphEvent::RunValidating(event) => run(RUN_VALIDATING, event.run_id),
        GraphEvent::RunPreparing(event) => run(RUN_PREPARING, event.run_id),
        GraphEvent::RunActivated(event) => run(RUN_ACTIVATED, event.run_id),
        GraphEvent::RunDraining(event) => run(RUN_DRAINING, event.run_id),
        GraphEvent::RunCompleted(event) => run(RUN_COMPLETED, event.run_id),
        GraphEvent::RunFailed(event) => run_failed(event),
        GraphEvent::RunCancelled(event) => run(RUN_CANCELLED, event.run_id),
        GraphEvent::NodeInstanceDescribed(event) => node_instance_described(event),
        GraphEvent::NodeInstancePrepared(event) => {
            instance(NODE_INSTANCE_PREPARED, event.instance_id)
        }
        GraphEvent::NodeInstanceStarted(event) => {
            instance(NODE_INSTANCE_STARTED, event.instance_id)
        }
        GraphEvent::NodeInstanceDraining(event) => {
            instance(NODE_INSTANCE_DRAINING, event.instance_id)
        }
        GraphEvent::NodeInstanceCancelling(event) => {
            instance(NODE_INSTANCE_CANCELLING, event.instance_id)
        }
        GraphEvent::NodeInstanceStopped(event) => {
            instance(NODE_INSTANCE_STOPPED, event.instance_id)
        }
        GraphEvent::NodeInstanceFailed(event) => {
            Ok(instance(NODE_INSTANCE_FAILED, event.instance_id)?
                .with("reason", text_node(&event.reason)))
        }
    }
}

fn run_created(event: &RunCreated) -> Result<ObservedEvent, TargetError> {
    Ok(ObservedEvent::new(parse::<EventRef>(RUN_CREATED))
        .with("run_id", named_node(&event.run_id)?)
        .with("pipeline_id", named_node(&event.pipeline_id)?))
}

fn run_failed(event: &RunFailed) -> Result<ObservedEvent, TargetError> {
    Ok(ObservedEvent::new(parse::<EventRef>(RUN_FAILED))
        .with("run_id", named_node(&event.run_id)?)
        .with("failure", failure_node(&event.failure)?))
}

fn node_instance_described(event: &NodeInstanceDescribed) -> Result<ObservedEvent, TargetError> {
    Ok(
        ObservedEvent::new(parse::<EventRef>(NODE_INSTANCE_DESCRIBED))
            .with("instance_id", named_node(&event.instance_id)?)
            .with("run_id", named_node(&event.run_id)?)
            .with("node_id", named_node(&event.node_id)?),
    )
}

fn runs_row(run: &Run) -> Result<ViewRow, TargetError> {
    Ok(BTreeMap::from([
        ("run_id".to_owned(), named_node(&run.run_id())?),
        ("pipeline_id".to_owned(), named_node(&run.pipeline_id())?),
        (
            "failure".to_owned(),
            run.failure().map_or(Ok(Node::Null), failure_node)?,
        ),
        ("parent".to_owned(), optional_node(run.parent().as_ref())?),
        ("state".to_owned(), named_node(&run.state())?),
    ]))
}

fn node_instances_row(instance: &NodeInstance) -> Result<ViewRow, TargetError> {
    Ok(BTreeMap::from([
        (
            "instance_id".to_owned(),
            named_node(&instance.instance_id())?,
        ),
        ("run_id".to_owned(), named_node(&instance.run_id())?),
        ("node_id".to_owned(), named_node(&instance.node_id())?),
        ("state".to_owned(), named_node(&instance.state())?),
    ]))
}

fn pipeline_declared(event: &PipelineDeclared) -> Result<ObservedEvent, TargetError> {
    Ok(ObservedEvent::new(parse::<EventRef>(PIPELINE_DECLARED))
        .with("pipeline_id", named_node(&event.pipeline_id)?)
        .with("name", text_node(&event.name)))
}

fn node_added(event: &NodeAdded) -> Result<ObservedEvent, TargetError> {
    Ok(ObservedEvent::new(parse::<EventRef>(NODE_ADDED))
        .with("node_id", named_node(&event.node_id)?)
        .with("pipeline_id", named_node(&event.pipeline_id)?)
        .with("name", text_node(&event.name)))
}

fn port_added(event: &PortAdded) -> Result<ObservedEvent, TargetError> {
    Ok(ObservedEvent::new(parse::<EventRef>(PORT_ADDED))
        .with("port_id", named_node(&event.port_id)?)
        .with("node_id", named_node(&event.node_id)?)
        .with("name", text_node(&event.name)))
}

fn ports_connected(event: &PortsConnected) -> Result<ObservedEvent, TargetError> {
    Ok(ObservedEvent::new(parse::<EventRef>(PORTS_CONNECTED))
        .with("edge_id", named_node(&event.edge_id)?)
        .with("from_port", named_node(&event.from_port)?)
        .with("to_port", named_node(&event.to_port)?))
}

fn node_descriptor_declared(event: &NodeDescriptorDeclared) -> Result<ObservedEvent, TargetError> {
    Ok(
        ObservedEvent::new(parse::<EventRef>(NODE_DESCRIPTOR_DECLARED))
            .with("kind", named_node(&event.kind)?)
            .with("role", named_node(&event.role)?),
    )
}

fn node_descriptors_row(descriptor: &NodeDescriptor) -> Result<ViewRow, TargetError> {
    Ok(BTreeMap::from([
        ("kind".to_owned(), named_node(descriptor.kind())?),
        ("role".to_owned(), named_node(&descriptor.role())?),
        (
            "requires".to_owned(),
            named_list_node(descriptor.requires())?,
        ),
        (
            "ports".to_owned(),
            port_descriptors_node(descriptor.ports())?,
        ),
    ]))
}

fn port_exposed(event: &PortExposed) -> Result<ObservedEvent, TargetError> {
    Ok(ObservedEvent::new(parse::<EventRef>(PORT_EXPOSED))
        .with("exposure_id", named_node(&event.exposure_id)?)
        .with("pipeline_id", named_node(&event.pipeline_id)?)
        .with("port_id", named_node(&event.port_id)?)
        .with("name", text_node(&event.name)))
}

fn feedback_loop_declared(event: &FeedbackLoopDeclared) -> Result<ObservedEvent, TargetError> {
    Ok(
        ObservedEvent::new(parse::<EventRef>(FEEDBACK_LOOP_DECLARED))
            .with("feedback_id", named_node(&event.feedback_id)?)
            .with("edge_id", named_node(&event.edge_id)?)
            .with("max_iterations", integer_node(event.max_iterations)),
    )
}

fn node_bound(event: &NodeBound) -> Result<ObservedEvent, TargetError> {
    Ok(ObservedEvent::new(parse::<EventRef>(NODE_BOUND))
        .with("binding_id", named_node(&event.binding_id)?)
        .with("node_id", named_node(&event.node_id)?)
        .with("adapter_id", named_node(&event.adapter_id)?))
}

fn pipeline_validated(event: &PipelineValidated) -> Result<ObservedEvent, TargetError> {
    Ok(ObservedEvent::new(parse::<EventRef>(PIPELINE_VALIDATED))
        .with("pipeline_id", named_node(&event.pipeline_id)?))
}

fn pipeline_planned(event: &PipelinePlanned) -> Result<ObservedEvent, TargetError> {
    Ok(ObservedEvent::new(parse::<EventRef>(PIPELINE_PLANNED))
        .with("pipeline_id", named_node(&event.pipeline_id)?))
}

fn ports_row(port: &Port) -> Result<ViewRow, TargetError> {
    Ok(BTreeMap::from([
        ("port_id".to_owned(), named_node(&port.port_id())?),
        ("pipeline_id".to_owned(), named_node(&port.pipeline_id())?),
        ("node_id".to_owned(), named_node(&port.node_id())?),
        ("name".to_owned(), text_node(port.name())),
        ("direction".to_owned(), named_node(&port.direction())?),
        ("modality".to_owned(), named_node(&port.modality())?),
        (
            "provisional_allowed".to_owned(),
            boolean_node(port.provisional_allowed()),
        ),
        ("essentiality".to_owned(), named_node(&port.essentiality())?),
        ("delivery".to_owned(), named_node(&port.delivery())?),
        ("incremental".to_owned(), boolean_node(port.incremental())),
        (
            "end_marker_required".to_owned(),
            boolean_node(port.end_marker_required()),
        ),
    ]))
}

fn edges_row(edge: &Edge) -> Result<ViewRow, TargetError> {
    Ok(BTreeMap::from([
        ("edge_id".to_owned(), named_node(&edge.edge_id())?),
        ("from_port".to_owned(), named_node(&edge.from_port())?),
        ("to_port".to_owned(), named_node(&edge.to_port())?),
        (
            "capacity".to_owned(),
            integer_node(i64::try_from(edge.capacity().get()).unwrap_or(i64::MAX)),
        ),
        ("overload".to_owned(), named_node(&edge.overload())?),
    ]))
}

fn exposures_row(exposure: &Exposure) -> Result<ViewRow, TargetError> {
    Ok(BTreeMap::from([
        (
            "exposure_id".to_owned(),
            named_node(&exposure.exposure_id())?,
        ),
        (
            "pipeline_id".to_owned(),
            named_node(&exposure.pipeline_id())?,
        ),
        ("port_id".to_owned(), named_node(&exposure.port_id())?),
        ("name".to_owned(), text_node(exposure.name())),
        (
            "capacity".to_owned(),
            integer_node(i64::try_from(exposure.capacity().get()).unwrap_or(i64::MAX)),
        ),
        ("overload".to_owned(), named_node(&exposure.overload())?),
    ]))
}

fn feedback_loops_row(feedback: &FeedbackLoop) -> Result<ViewRow, TargetError> {
    Ok(BTreeMap::from([
        (
            "feedback_id".to_owned(),
            named_node(&feedback.feedback_id())?,
        ),
        (
            "pipeline_id".to_owned(),
            named_node(&feedback.pipeline_id())?,
        ),
        ("edge_id".to_owned(), named_node(&feedback.edge_id())?),
        (
            "max_iterations".to_owned(),
            integer_node(i64::try_from(feedback.max_iterations().get()).unwrap_or(i64::MAX)),
        ),
    ]))
}

fn bindings_row(binding: &Binding) -> Result<ViewRow, TargetError> {
    Ok(BTreeMap::from([
        ("binding_id".to_owned(), named_node(&binding.binding_id())?),
        ("node_id".to_owned(), named_node(&binding.node_id())?),
        ("adapter_id".to_owned(), named_node(&binding.adapter_id())?),
    ]))
}

/// The node of an ESS `Optional<List<Capability>>`: null when unset.
fn optional_list(values: Option<&[Capability]>) -> Result<Node, TargetError> {
    values.map_or(Ok(Node::Null), named_list_node)
}

fn outcome(command: &str, branch: &str) -> OutcomeRef {
    OutcomeRef::new(
        parse::<CommandRef>(command),
        branch
            .parse()
            .unwrap_or_else(|error| panic!("`{branch}` is an outcome name: {error}")),
    )
}

/// Parses a name this module writes as a literal; a malformed one is a defect here.
fn parse<T>(name: &str) -> T
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    name.parse()
        .unwrap_or_else(|error| panic!("`{name}` is a well-formed name: {error}"))
}
