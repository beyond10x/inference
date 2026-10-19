//! `inference.graph`: the five declaration commands, `ValidatePipeline` and `PlanPipeline`, the
//! five views and the seven events of `ess/domains/graph.yaml`, answered by [`GraphDomain`].
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
//! the first two when planning finds them, and a scenario may force any of the three on the next
//! invocation, which then moves nothing and answers the forced error with every field its
//! declaration does not make `Optional`: what the planner finds for that same refusal over that
//! pipeline, and otherwise a stand-in read from the declaration ([`forced_plan`]). The store never
//! decides `locality-denied` (check 8 is not planned yet), so a forced one always carries
//! stand-ins.
//!
//! Runs (the run commands, the views `Runs` and `NodeInstances` and their events) are
//! [`TargetError::Unsupported`]: no store decides them yet, and they are not faked.
//! `ess/SKIPPED.md` names the story for each.

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
    AddNode, AddNodeError, AddPort, AddPortError, BindNode, BindNodeError, Binding,
    CapabilityMismatch, Catalog, ConnectPorts, ConnectPortsError, Declaration, DeclarePipeline,
    Edge, GraphFinding, NodeAdded, NodeBound, NodeId, NodeUnbound, PipelineDeclared, PipelineId,
    PipelineNotFound, PipelineNotReady, PipelinePlanned, PipelineValidated, PlanPipeline,
    PlanPipelineError, Port, PortAdded, PortsConnected, ValidatePipeline, ValidatePipelineError,
};
use inference_testkit::graph::{GraphCatalog, GraphDomain, GraphEvent};
use uuid::Uuid;

use super::{
    Input, Stores, boolean_node, integer_node, named_list_node, named_node, text_node, unreadable,
    unsupported,
};

/// The prefix of every name this module answers.
pub const DOMAIN: &str = "inference.graph";

const DECLARE_PIPELINE: &str = "inference.graph.DeclarePipeline";
const ADD_NODE: &str = "inference.graph.AddNode";
const ADD_PORT: &str = "inference.graph.AddPort";
const CONNECT_PORTS: &str = "inference.graph.ConnectPorts";
const BIND_NODE: &str = "inference.graph.BindNode";
const VALIDATE_PIPELINE: &str = "inference.graph.ValidatePipeline";
const PLAN_PIPELINE: &str = "inference.graph.PlanPipeline";

/// The outcome of `ValidatePipeline` that `ess/domains/graph.yaml` declares external.
const REJECTED: &str = "rejected";
/// The outcomes of `PlanPipeline` that `ess/domains/graph.yaml` declares external.
const UNBOUND: &str = "unbound";
const MISMATCH: &str = "mismatch";
const LOCALITY_DENIED: &str = "locality-denied";

const PIPELINE_DECLARED: &str = "inference.graph.PipelineDeclared";
const NODE_ADDED: &str = "inference.graph.NodeAdded";
const PORT_ADDED: &str = "inference.graph.PortAdded";
const PORTS_CONNECTED: &str = "inference.graph.PortsConnected";
const NODE_BOUND: &str = "inference.graph.NodeBound";
const PIPELINE_VALIDATED: &str = "inference.graph.PipelineValidated";
const PIPELINE_PLANNED: &str = "inference.graph.PipelinePlanned";

const PROFILE_NOT_FOUND: &str = "inference.policy.ProfileNotFound";
const PIPELINE_NOT_FOUND: &str = "inference.graph.PipelineNotFound";
const PIPELINE_NOT_READY: &str = "inference.graph.PipelineNotReady";
const NODE_NOT_FOUND: &str = "inference.graph.NodeNotFound";
const PORT_NOT_FOUND: &str = "inference.graph.PortNotFound";
const UNBOUNDED_EDGE: &str = "inference.graph.UnboundedEdge";
const INCOMPATIBLE_EDGE: &str = "inference.graph.IncompatibleEdge";
const ADAPTER_NOT_FOUND: &str = "inference.graph.AdapterNotFound";
const INCOMPATIBLE_GRAPH: &str = "inference.graph.IncompatibleGraph";
const NODE_UNBOUND: &str = "inference.graph.NodeUnbound";
const CAPABILITY_MISMATCH: &str = "inference.graph.CapabilityMismatch";
const LOCALITY_DENIED_ERROR: &str = "inference.graph.LocalityDenied";
const PIPELINE_STATE_CONFLICT: &str = "inference.graph.PipelineStateConflict";

const PIPELINES: &str = "inference.graph.Pipelines";
const NODES: &str = "inference.graph.Nodes";
const PORTS: &str = "inference.graph.Ports";
const EDGES: &str = "inference.graph.Edges";
const BINDINGS: &str = "inference.graph.Bindings";

/// The declaration half of the `inference.graph` store of one scenario.
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
        ADD_PORT => add_port(domain, request)?,
        CONNECT_PORTS => connect_ports(domain, request)?,
        BIND_NODE => bind_node(domain, &catalog, request)?,
        VALIDATE_PIPELINE => validate_pipeline(domain, forced.is_some(), request)?,
        PLAN_PIPELINE => plan_pipeline(domain, &catalog, forced.as_ref(), request)?,
        _ => return Err(unsupported(&name)),
    };
    stores.publish();
    Ok(result.with_consistency(stores.token()?))
}

/// Accepts a force of `ValidatePipeline/rejected` or of `PlanPipeline/unbound`, `mismatch` or
/// `locality-denied`, the external outcomes of the commands this module answers; refuses a force
/// of another outcome of those two commands as an error, as no specification-declared external
/// outcome, and any other command's as unsupported.
pub fn configure_external_outcome(
    stores: &mut Stores,
    request: &ExternalOutcomeControl,
) -> Result<(), TargetError> {
    let command = request.force.command.to_string();
    let external: &[&str] = match command.as_str() {
        VALIDATE_PIPELINE => &[REJECTED],
        PLAN_PIPELINE => &[UNBOUND, MISMATCH, LOCALITY_DENIED],
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
        Err(PlanPipelineError::Unbound(error)) => {
            SemanticCommandResult::took(outcome(PLAN_PIPELINE, UNBOUND))
                .with_error(node_unbound(error)?)
        }
        Err(PlanPipelineError::Mismatch(error)) => {
            SemanticCommandResult::took(outcome(PLAN_PIPELINE, MISMATCH))
                .with_error(capability_mismatch(error)?)
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
    let error = if *forced == outcome(PLAN_PIPELINE, UNBOUND) {
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
    } else {
        let binding = declaration
            .bindings()
            .find(|binding| binding.pipeline_id() == pipeline_id);
        let adapter_id = binding.map_or_else(
            || AdapterId::from(Uuid::nil()),
            |binding| binding.adapter_id(),
        );
        DeclaredErrorValue::new(parse::<ErrorRef>(LOCALITY_DENIED_ERROR))
            .with(
                "node_id",
                named_node(&binding.map_or(first_node, |binding| binding.node_id()))?,
            )
            .with("adapter_id", named_node(&adapter_id)?)
            .with(
                "locality",
                named_node(
                    &catalog
                        .adapter(adapter_id)
                        .map_or(Locality::ALL[0], |adapter| adapter.locality()),
                )?,
            )
            .with(
                "profile_id",
                named_node(&pipeline.map_or_else(
                    || ProfileId::from(Uuid::nil()),
                    |pipeline| pipeline.profile_id(),
                ))?,
            )
    };
    Ok(SemanticCommandResult::took(forced.clone()).with_error(error))
}

/// `NodeUnbound`, naming the node.
fn node_unbound(error: NodeUnbound) -> Result<DeclaredErrorValue, TargetError> {
    Ok(DeclaredErrorValue::new(parse::<ErrorRef>(NODE_UNBOUND))
        .with("node_id", named_node(&error.node_id)?))
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
        BINDINGS => declaration
            .bindings()
            .map(bindings_row)
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
    if ![
        PIPELINE_DECLARED,
        NODE_ADDED,
        PORT_ADDED,
        PORTS_CONNECTED,
        NODE_BOUND,
        PIPELINE_VALIDATED,
        PIPELINE_PLANNED,
    ]
    .contains(&wanted.as_str())
    {
        return Err(unsupported(&wanted));
    }
    stores.publish();
    let mut observed = Vec::new();
    for (index, event) in stores.graph.events().iter().enumerate() {
        let event = match event {
            GraphEvent::PipelineDeclared(event) => pipeline_declared(event)?,
            GraphEvent::NodeAdded(event) => node_added(event)?,
            GraphEvent::PortAdded(event) => port_added(event)?,
            GraphEvent::PortsConnected(event) => ports_connected(event)?,
            GraphEvent::NodeBound(event) => node_bound(event)?,
            GraphEvent::PipelineValidated(event) => pipeline_validated(event)?,
            GraphEvent::PipelinePlanned(event) => pipeline_planned(event)?,
        };
        if event.event == request.event {
            let position = stores.graph_position(index)?;
            observed.push(event.in_activity(request.correlation.clone()).at(position));
        }
    }
    Ok(observed)
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ess_conformance::scenario::{CommandRef, EventRef, OutcomeRef, ViewRef};
    use ess_conformance::target::{
        ConformanceTarget, Deadline, EventObservationRequest, ExternalOutcomeControl,
        ObservedEvent, SemanticCommandRequest, SemanticCommandResult, SemanticViewRequest,
        TargetError,
    };
    use ess_primitives::consistency::QueryConsistency;
    use ess_primitives::facts::Number;
    use ess_primitives::ids::CorrelationId;
    use ess_primitives::node::Node;
    use ess_primitives::time::Timestamp;

    use super::super::{Stores, Target, model, policy};
    use super::{execute_command, observe_events, query_view};

    const UNKNOWN: &str = "00000000-0000-4000-8000-6eced0bc8ba0";

    fn correlation() -> CorrelationId {
        CorrelationId::new("graph").expect("a correlation id")
    }

    fn text(value: &str) -> Node {
        Node::Text(value.to_owned())
    }

    fn number(value: f64) -> Node {
        Node::Number(Number::new(value).expect("finite"))
    }

    fn list(values: &[&str]) -> Node {
        Node::Seq(values.iter().map(|value| text(value)).collect())
    }

    fn command(name: &str, input: &[(&str, Node)]) -> SemanticCommandRequest {
        SemanticCommandRequest {
            command: name.parse::<CommandRef>().expect("a command name"),
            actor: None,
            caller: None,
            input: input
                .iter()
                .map(|(field, value)| ((*field).to_owned(), value.clone()))
                .collect(),
            correlation: correlation(),
        }
    }

    /// Runs `name` through the module of its domain.
    fn run(stores: &mut Stores, name: &str, input: &[(&str, Node)]) -> SemanticCommandResult {
        let request = command(name, input);
        let result = if name.starts_with("inference.policy.") {
            policy::execute_command(stores, &request)
        } else if name.starts_with("inference.model.") {
            model::execute_command(stores, &request)
        } else {
            execute_command(stores, &request)
        };
        result.unwrap_or_else(|error| panic!("`{name}` answers: {error}"))
    }

    fn branch(result: &SemanticCommandResult) -> String {
        result
            .outcome
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default()
    }

    /// The field `field` of the one event `result` emitted.
    fn emitted(result: &SemanticCommandResult, field: &str) -> Node {
        assert_eq!(result.direct_events.len(), 1, "{result:?}");
        result.direct_events[0].payload[field].clone()
    }

    fn rows(stores: &mut Stores, name: &str) -> Vec<BTreeMap<String, Node>> {
        query_view(
            stores,
            &SemanticViewRequest {
                view: name.parse::<ViewRef>().expect("a view name"),
                params: BTreeMap::new(),
                consistency: QueryConsistency::Current,
                correlation: correlation(),
                deadline: Deadline::at(Timestamp::from_epoch_millis(u64::MAX)),
            },
        )
        .unwrap_or_else(|error| panic!("`{name}` reads: {error}"))
        .rows
    }

    fn observed(stores: &mut Stores, name: &str) -> Vec<ObservedEvent> {
        observe_events(
            stores,
            &EventObservationRequest {
                event: name.parse::<EventRef>().expect("an event name"),
                correlation: correlation(),
                deadline: Deadline::at(Timestamp::from_epoch_millis(u64::MAX)),
            },
        )
        .unwrap_or_else(|error| panic!("`{name}` is observed: {error}"))
    }

    fn profile(stores: &mut Stores) -> Node {
        let result = run(
            stores,
            "inference.policy.DeclareProfile",
            &[
                ("kind", text("offline_strict")),
                ("egress", text("denied")),
                ("admitted", list(&["same_process"])),
            ],
        );
        emitted(&result, "profile_id")
    }

    fn pipeline(stores: &mut Stores, profile_id: Node) -> Node {
        let result = run(
            stores,
            "inference.graph.DeclarePipeline",
            &[
                ("name", text("voice")),
                ("schema_revision", text("1")),
                ("mode", text("turn_based")),
                ("profile_id", profile_id),
                ("required_capabilities", list(&["output_streaming"])),
            ],
        );
        emitted(&result, "pipeline_id")
    }

    fn node(stores: &mut Stores, pipeline_id: Node) -> Node {
        let result = run(
            stores,
            "inference.graph.AddNode",
            &[
                ("pipeline_id", pipeline_id),
                ("name", text("llm")),
                ("role", text("model")),
                ("requires", list(&[])),
            ],
        );
        emitted(&result, "node_id")
    }

    fn port_input(
        pipeline_id: Node,
        node_id: Node,
        direction: &str,
        incremental: Node,
    ) -> Vec<(&'static str, Node)> {
        vec![
            ("pipeline_id", pipeline_id),
            ("node_id", node_id),
            ("name", text(direction)),
            ("direction", text(direction)),
            ("modality", text("Text")),
            ("essentiality", text("essential")),
            ("delivery", text("reliable")),
            ("incremental", incremental),
            ("end_marker_required", Node::Bool(false)),
            ("provisional_allowed", Node::Bool(true)),
        ]
    }

    fn port(stores: &mut Stores, pipeline_id: Node, node_id: Node, direction: &str) -> Node {
        let result = run(
            stores,
            "inference.graph.AddPort",
            &port_input(pipeline_id, node_id, direction, Node::Bool(true)),
        );
        emitted(&result, "port_id")
    }

    fn adapter(stores: &mut Stores) -> Node {
        let model_id = emitted(
            &run(
                stores,
                "inference.model.DeclareModel",
                &[
                    ("family", text("f")),
                    ("checkpoint", text("c")),
                    ("code_license", text("l")),
                    ("weight_license", text("w")),
                    ("checksum", text("s")),
                    ("input_modalities", list(&["Text"])),
                    ("output_modalities", list(&["Text"])),
                    ("provenance", text("p")),
                    ("source_revision", text("r")),
                ],
            ),
            "model_id",
        );
        emitted(
            &run(
                stores,
                "inference.model.DeclareAdapter",
                &[
                    ("model_id", model_id),
                    ("backend", text("b")),
                    ("contract_revision", text("c")),
                    ("execution_form", text("in_process")),
                    ("locality", text("same_process")),
                    ("capabilities", list(&["output_streaming"])),
                    ("max_concurrency", number(1.0)),
                ],
            ),
            "adapter_id",
        )
    }

    #[test]
    fn a_declaration_through_the_target_is_in_every_view_and_observed_in_order() {
        let mut stores = Stores::default();
        let profile_id = profile(&mut stores);
        let pipeline_id = pipeline(&mut stores, profile_id.clone());
        let node_id = node(&mut stores, pipeline_id.clone());
        let output = port(&mut stores, pipeline_id.clone(), node_id.clone(), "output");
        let input = port(&mut stores, pipeline_id.clone(), node_id.clone(), "input");
        let connected = run(
            &mut stores,
            "inference.graph.ConnectPorts",
            &[
                ("pipeline_id", pipeline_id.clone()),
                ("from_port", output.clone()),
                ("to_port", input.clone()),
                ("capacity", number(4.0)),
                ("overload", text("drop_stale_epochs")),
            ],
        );
        assert_eq!(branch(&connected), "inference.graph.ConnectPorts/connected");
        let adapter_id = adapter(&mut stores);
        let bound = run(
            &mut stores,
            "inference.graph.BindNode",
            &[
                ("pipeline_id", pipeline_id.clone()),
                ("node_id", node_id.clone()),
                ("adapter_id", adapter_id.clone()),
            ],
        );
        assert_eq!(branch(&bound), "inference.graph.BindNode/bound");
        assert_eq!(
            bound.consistency.as_ref().map(|token| token.as_str()),
            Some("seq:9"),
            "every store's events count"
        );

        let pipelines = rows(&mut stores, "inference.graph.Pipelines");
        assert_eq!(pipelines.len(), 1);
        assert_eq!(pipelines[0]["pipeline_id"], pipeline_id);
        assert_eq!(pipelines[0]["profile_id"], profile_id);
        assert_eq!(pipelines[0]["mode"], text("turn_based"));
        assert_eq!(pipelines[0]["effective_capabilities"], Node::Null);
        assert_eq!(pipelines[0]["downgrades"], Node::Null);
        assert_eq!(pipelines[0]["state"], text("Declared"));
        assert_eq!(pipelines[0].len(), 9);

        let ports = rows(&mut stores, "inference.graph.Ports");
        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0]["port_id"], output);
        assert_eq!(ports[0]["incremental"], Node::Bool(true));
        assert_eq!(ports[0]["end_marker_required"], Node::Bool(false));
        assert_eq!(ports[0]["provisional_allowed"], Node::Bool(true));
        assert_eq!(ports[0]["pipeline_id"], pipeline_id);
        assert_eq!(ports[0].len(), 11);

        let edges = rows(&mut stores, "inference.graph.Edges");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0]["capacity"], Node::Number(Number::from(4_i64)));
        assert_eq!(edges[0]["overload"], text("drop_stale_epochs"));
        assert_eq!(edges[0]["to_port"], input);

        let bindings = rows(&mut stores, "inference.graph.Bindings");
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0]["node_id"], node_id);
        assert_eq!(bindings[0]["adapter_id"], adapter_id);
        assert_eq!(rows(&mut stores, "inference.graph.Nodes").len(), 1);

        let at = |stores: &mut Stores, name: &str| -> Vec<Option<u64>> {
            observed(stores, name)
                .iter()
                .map(|event| event.sequence)
                .collect()
        };
        assert_eq!(
            at(&mut stores, "inference.graph.PipelineDeclared"),
            [Some(2)]
        );
        assert_eq!(at(&mut stores, "inference.graph.NodeAdded"), [Some(3)]);
        assert_eq!(
            at(&mut stores, "inference.graph.PortAdded"),
            [Some(4), Some(5)]
        );
        assert_eq!(at(&mut stores, "inference.graph.PortsConnected"), [Some(6)]);
        assert_eq!(at(&mut stores, "inference.graph.NodeBound"), [Some(9)]);
        let profiles = policy::observe_events(
            &mut stores,
            &EventObservationRequest {
                event: "inference.policy.ProfileDeclared"
                    .parse::<EventRef>()
                    .expect("an event name"),
                correlation: correlation(),
                deadline: Deadline::at(Timestamp::from_epoch_millis(u64::MAX)),
            },
        )
        .expect("the policy module observes its event");
        assert_eq!(
            profiles
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            [Some(1)]
        );
        assert_eq!(profiles[0].payload["profile_id"], profile_id);
        assert_eq!(profiles[0].payload["admitted"], list(&["same_process"]));
    }

    #[test]
    fn a_refused_command_emits_nothing_and_advances_no_token() {
        let mut stores = Stores::default();
        let profile_id = profile(&mut stores);
        let pipeline_id = pipeline(&mut stores, profile_id);
        let node_id = node(&mut stores, pipeline_id.clone());
        let input = port(&mut stores, pipeline_id.clone(), node_id, "input");

        let refused = run(
            &mut stores,
            "inference.graph.ConnectPorts",
            &[
                ("pipeline_id", pipeline_id),
                ("from_port", input.clone()),
                ("to_port", text(UNKNOWN)),
                ("capacity", number(1.0)),
                ("overload", text("pause")),
            ],
        );
        assert_eq!(
            branch(&refused),
            "inference.graph.ConnectPorts/source-not-output"
        );
        let error = refused.error.as_ref().expect("an error");
        assert_eq!(error.fields["from_port"], input);
        assert_eq!(error.fields["to_port"], text(UNKNOWN));
        assert_eq!(error.fields["defect"], text("source_not_output"));
        assert!(refused.direct_events.is_empty());
        assert_eq!(
            refused.consistency.as_ref().map(|token| token.as_str()),
            Some("seq:4")
        );
        assert!(rows(&mut stores, "inference.graph.Edges").is_empty());
    }

    #[test]
    fn an_undeclared_field_or_a_flag_that_is_not_a_boolean_is_an_error_and_adds_nothing() {
        let mut stores = Stores::default();
        let profile_id = profile(&mut stores);
        let pipeline_id = pipeline(&mut stores, profile_id);
        let node_id = node(&mut stores, pipeline_id.clone());

        let textual = execute_command(
            &mut stores,
            &command(
                "inference.graph.AddPort",
                &port_input(pipeline_id.clone(), node_id.clone(), "input", text("true")),
            ),
        );
        assert!(
            matches!(&textual, Err(TargetError::Unavailable { .. })),
            "{textual:?}"
        );
        let mut extra = port_input(pipeline_id.clone(), node_id, "input", Node::Bool(true));
        extra.push(("profile_id", pipeline_id.clone()));
        let extra = execute_command(&mut stores, &command("inference.graph.AddPort", &extra));
        assert!(
            matches!(&extra, Err(TargetError::Unavailable { .. })),
            "{extra:?}"
        );
        let fractional = execute_command(
            &mut stores,
            &command(
                "inference.graph.ConnectPorts",
                &[
                    ("pipeline_id", pipeline_id),
                    ("from_port", text(UNKNOWN)),
                    ("to_port", text(UNKNOWN)),
                    ("capacity", number(1.5)),
                    ("overload", text("pause")),
                ],
            ),
        );
        assert!(
            matches!(&fractional, Err(TargetError::Unavailable { .. })),
            "{fractional:?}"
        );
        assert!(rows(&mut stores, "inference.graph.Ports").is_empty());
        assert_eq!(stores.graph.events().len(), 2);
    }

    fn validate(stores: &mut Stores, pipeline_id: Node) -> SemanticCommandResult {
        run(
            stores,
            "inference.graph.ValidatePipeline",
            &[("pipeline_id", pipeline_id)],
        )
    }

    #[test]
    fn a_valid_pipeline_is_validated_once_and_its_event_is_placed_in_order() {
        let mut stores = Stores::default();
        let profile_id = profile(&mut stores);
        let pipeline_id = pipeline(&mut stores, profile_id);

        let validated = validate(&mut stores, pipeline_id.clone());
        assert_eq!(
            branch(&validated),
            "inference.graph.ValidatePipeline/validated"
        );
        assert_eq!(emitted(&validated, "pipeline_id"), pipeline_id);
        assert_eq!(validated.direct_events[0].payload.len(), 1);
        assert_eq!(
            validated.consistency.as_ref().map(|token| token.as_str()),
            Some("seq:3")
        );
        assert_eq!(
            rows(&mut stores, "inference.graph.Pipelines")[0]["state"],
            text("Validated")
        );
        let placed: Vec<_> = observed(&mut stores, "inference.graph.PipelineValidated")
            .iter()
            .map(|event| (event.sequence, event.payload["pipeline_id"].clone()))
            .collect();
        assert_eq!(placed, [(Some(3), pipeline_id.clone())]);

        let again = validate(&mut stores, pipeline_id);
        assert_eq!(
            branch(&again),
            "inference.graph.ValidatePipeline/wrong-state"
        );
        let error = again.error.as_ref().expect("a declared error");
        assert_eq!(
            error.error.to_string(),
            "inference.graph.PipelineStateConflict"
        );
        assert_eq!(
            error.fields,
            BTreeMap::from([("current".to_owned(), text("Validated"))])
        );
        assert!(again.direct_events.is_empty());
        assert_eq!(
            again.consistency.as_ref().map(|token| token.as_str()),
            Some("seq:3")
        );
    }

    #[test]
    fn an_unknown_pipeline_is_wrong_state_with_no_current() {
        let mut stores = Stores::default();
        let refused = validate(&mut stores, text(UNKNOWN));
        assert_eq!(
            branch(&refused),
            "inference.graph.ValidatePipeline/wrong-state"
        );
        let error = refused.error.as_ref().expect("a declared error");
        assert_eq!(
            error.error.to_string(),
            "inference.graph.PipelineStateConflict"
        );
        assert!(error.fields.is_empty(), "{error:?}");
        assert!(refused.direct_events.is_empty());
    }

    #[test]
    fn an_invalid_pipeline_is_rejected_with_its_findings_and_stays_declared() {
        let mut stores = Stores::default();
        let profile_id = profile(&mut stores);
        let pipeline_id = pipeline(&mut stores, profile_id);
        let node_id = node(&mut stores, pipeline_id.clone());
        let output = port(&mut stores, pipeline_id.clone(), node_id.clone(), "output");

        let rejected = validate(&mut stores, pipeline_id);
        assert_eq!(
            branch(&rejected),
            "inference.graph.ValidatePipeline/rejected"
        );
        let error = rejected.error.as_ref().expect("a declared error");
        assert_eq!(error.error.to_string(), "inference.graph.IncompatibleGraph");
        assert_eq!(
            error.fields,
            BTreeMap::from([(
                "findings".to_owned(),
                Node::Seq(vec![Node::Map(BTreeMap::from([
                    ("defect".to_owned(), text("unreachable_mandatory_output")),
                    ("node_id".to_owned(), node_id),
                    ("port_id".to_owned(), output),
                ]))])
            )])
        );
        assert!(rejected.direct_events.is_empty());
        assert_eq!(
            rows(&mut stores, "inference.graph.Pipelines")[0]["state"],
            text("Declared")
        );
        assert!(observed(&mut stores, "inference.graph.PipelineValidated").is_empty());
    }

    #[test]
    fn a_port_is_added_only_under_a_known_pipeline_that_is_still_declared() {
        let mut stores = Stores::default();
        let profile_id = profile(&mut stores);
        let pipeline_id = pipeline(&mut stores, profile_id);
        let node_id = node(&mut stores, pipeline_id.clone());

        let unknown = run(
            &mut stores,
            "inference.graph.AddPort",
            &port_input(text(UNKNOWN), node_id.clone(), "input", Node::Bool(true)),
        );
        assert_eq!(branch(&unknown), "inference.graph.AddPort/unknown-pipeline");
        let error = unknown.error.as_ref().expect("a declared error");
        assert_eq!(error.error.to_string(), "inference.graph.PipelineNotFound");
        assert_eq!(
            error.fields,
            BTreeMap::from([("pipeline_id".to_owned(), text(UNKNOWN))])
        );

        let validated = validate(&mut stores, pipeline_id.clone());
        assert_eq!(
            branch(&validated),
            "inference.graph.ValidatePipeline/validated"
        );
        let events = stores.graph.events().len();
        let refused = run(
            &mut stores,
            "inference.graph.AddPort",
            &port_input(pipeline_id.clone(), node_id, "input", Node::Bool(true)),
        );
        assert_eq!(branch(&refused), "inference.graph.AddPort/not-declared");
        let error = refused.error.as_ref().expect("a declared error");
        assert_eq!(error.error.to_string(), "inference.graph.PipelineNotReady");
        assert_eq!(
            error.fields,
            BTreeMap::from([
                ("pipeline_id".to_owned(), pipeline_id),
                ("required".to_owned(), text("Declared")),
            ])
        );
        assert!(refused.direct_events.is_empty());
        assert_eq!(stores.graph.events().len(), events);
        assert!(rows(&mut stores, "inference.graph.Ports").is_empty());
        assert_eq!(state_of(&mut stores), text("Validated"));
    }

    #[test]
    fn a_forced_rejection_of_a_declared_pipeline_carries_the_stores_findings() {
        let mut stores = Stores::default();
        let profile_id = profile(&mut stores);
        let pipeline_id = pipeline(&mut stores, profile_id);
        let node_id = node(&mut stores, pipeline_id.clone());
        let output = port(&mut stores, pipeline_id.clone(), node_id.clone(), "output");

        force(&mut stores, "inference.graph.ValidatePipeline/rejected")
            .expect("rejected is external");
        let forced = validate(&mut stores, pipeline_id);
        assert_eq!(branch(&forced), "inference.graph.ValidatePipeline/rejected");
        assert_eq!(
            forced.error.as_ref().expect("a declared error").fields["findings"],
            Node::Seq(vec![Node::Map(BTreeMap::from([
                ("defect".to_owned(), text("unreachable_mandatory_output")),
                ("node_id".to_owned(), node_id),
                ("port_id".to_owned(), output),
            ]))])
        );
        assert_eq!(state_of(&mut stores), text("Declared"));
    }

    #[test]
    fn a_forced_rejection_answers_in_every_state_as_the_specification_orders_it() {
        let mut stores = Stores::default();
        let profile_id = profile(&mut stores);
        let pipeline_id = pipeline(&mut stores, profile_id);
        let validated = validate(&mut stores, pipeline_id.clone());
        assert_eq!(
            branch(&validated),
            "inference.graph.ValidatePipeline/validated"
        );

        force(&mut stores, "inference.graph.ValidatePipeline/rejected")
            .expect("rejected is external");
        let forced = validate(&mut stores, pipeline_id);
        assert_eq!(
            branch(&forced),
            "inference.graph.ValidatePipeline/rejected",
            "`rejected` moves nothing, so `wrong_state` never answers before it (ESS precedence, \
             step 4 against step 6), and the interpreter answers it on a Validated pipeline"
        );
        assert_eq!(
            forced.error.as_ref().expect("a declared error").fields["findings"],
            Node::Seq(Vec::new())
        );
        assert_eq!(state_of(&mut stores), text("Validated"));
    }

    #[test]
    fn a_forced_rejection_answers_the_next_validation_only() {
        let target = Target::default();
        let pipeline_id = {
            let stores = &mut *target.stores.borrow_mut();
            let profile_id = profile(stores);
            pipeline(stores, profile_id)
        };
        let force = |outcome: &str| {
            target.configure_external_outcome(ExternalOutcomeControl {
                force: OutcomeRef::new(
                    outcome
                        .split_once('/')
                        .and_then(|(command, _)| command.parse::<CommandRef>().ok())
                        .expect("a command name"),
                    outcome
                        .split_once('/')
                        .and_then(|(_, branch)| branch.parse().ok())
                        .expect("an outcome name"),
                ),
                correlation: correlation(),
            })
        };
        let validate = || {
            target
                .execute_command(command(
                    "inference.graph.ValidatePipeline",
                    &[("pipeline_id", pipeline_id.clone())],
                ))
                .expect("ValidatePipeline answers")
        };

        force("inference.graph.ValidatePipeline/rejected").expect("rejected is external");
        let rejected = validate();
        assert_eq!(
            branch(&rejected),
            "inference.graph.ValidatePipeline/rejected"
        );
        let error = rejected.error.as_ref().expect("a declared error");
        assert_eq!(error.error.to_string(), "inference.graph.IncompatibleGraph");
        assert_eq!(error.fields["findings"], Node::Seq(Vec::new()));
        assert!(rejected.direct_events.is_empty());

        let validated = validate();
        assert_eq!(
            branch(&validated),
            "inference.graph.ValidatePipeline/validated",
            "the force lapses after one invocation"
        );

        let declared = force("inference.graph.ValidatePipeline/validated");
        assert!(
            matches!(&declared, Err(TargetError::Unavailable { .. })),
            "{declared:?}"
        );
        let planned = force("inference.graph.PlanPipeline/planned");
        assert!(
            matches!(&planned, Err(TargetError::Unavailable { .. })),
            "{planned:?}"
        );
        let run = force("inference.graph.CreateRun/created");
        assert!(
            matches!(&run, Err(TargetError::Unsupported { .. })),
            "{run:?}"
        );
    }

    #[test]
    fn runs_are_unsupported_not_faked() {
        let mut stores = Stores::default();
        let answer = execute_command(&mut stores, &command("inference.graph.CreateRun", &[]));
        assert!(
            matches!(&answer, Err(TargetError::Unsupported { .. })),
            "{answer:?}"
        );
        let event = observe_events(
            &mut stores,
            &EventObservationRequest {
                event: "inference.graph.RunCreated"
                    .parse::<EventRef>()
                    .expect("an event name"),
                correlation: correlation(),
                deadline: Deadline::at(Timestamp::from_epoch_millis(u64::MAX)),
            },
        );
        assert!(
            matches!(&event, Err(TargetError::Unsupported { .. })),
            "{event:?}"
        );
    }

    fn plan(stores: &mut Stores, pipeline_id: Node) -> SemanticCommandResult {
        run(
            stores,
            "inference.graph.PlanPipeline",
            &[("pipeline_id", pipeline_id)],
        )
    }

    /// Forces `outcome`, written `<command>/<outcome>`, through the module.
    fn force(stores: &mut Stores, outcome: &str) -> Result<(), TargetError> {
        let (command, branch) = outcome.split_once('/').expect("a command and an outcome");
        super::configure_external_outcome(
            stores,
            &ExternalOutcomeControl {
                force: OutcomeRef::new(
                    command.parse::<CommandRef>().expect("a command name"),
                    branch.parse().expect("an outcome name"),
                ),
                correlation: correlation(),
            },
        )
    }

    fn state_of(stores: &mut Stores) -> Node {
        rows(stores, "inference.graph.Pipelines")[0]["state"].clone()
    }

    /// A Validated pipeline holding one model node that requires `requires`, bound to the
    /// adapter of [`adapter`] (which implements `output_streaming` only) when `bound`.
    fn one_model(stores: &mut Stores, requires: &[&str], bound: bool) -> (Node, Node) {
        let profile_id = profile(stores);
        let pipeline_id = pipeline(stores, profile_id);
        let node_id = emitted(
            &run(
                stores,
                "inference.graph.AddNode",
                &[
                    ("pipeline_id", pipeline_id.clone()),
                    ("name", text("llm")),
                    ("role", text("model")),
                    ("requires", list(requires)),
                ],
            ),
            "node_id",
        );
        if bound {
            let adapter_id = adapter(stores);
            let bound = run(
                stores,
                "inference.graph.BindNode",
                &[
                    ("pipeline_id", pipeline_id.clone()),
                    ("node_id", node_id.clone()),
                    ("adapter_id", adapter_id),
                ],
            );
            assert_eq!(branch(&bound), "inference.graph.BindNode/bound");
        }
        let validated = validate(stores, pipeline_id.clone());
        assert_eq!(
            branch(&validated),
            "inference.graph.ValidatePipeline/validated"
        );
        (pipeline_id, node_id)
    }

    #[test]
    fn a_validated_pipeline_is_planned_once_and_its_event_is_placed_in_order() {
        let mut stores = Stores::default();
        let profile_id = profile(&mut stores);
        let pipeline_id = pipeline(&mut stores, profile_id);
        validate(&mut stores, pipeline_id.clone());

        let planned = plan(&mut stores, pipeline_id.clone());
        assert_eq!(branch(&planned), "inference.graph.PlanPipeline/planned");
        assert_eq!(emitted(&planned, "pipeline_id"), pipeline_id);
        assert_eq!(planned.direct_events[0].payload.len(), 1);
        assert_eq!(
            planned.consistency.as_ref().map(|token| token.as_str()),
            Some("seq:4")
        );
        let row = &rows(&mut stores, "inference.graph.Pipelines")[0];
        assert_eq!(row["state"], text("Planned"));
        assert_eq!(
            row["effective_capabilities"],
            list(&[]),
            "a pipeline with no node has no effective capability"
        );
        assert_eq!(row["downgrades"], list(&[]));
        let placed: Vec<_> = observed(&mut stores, "inference.graph.PipelinePlanned")
            .iter()
            .map(|event| (event.sequence, event.payload["pipeline_id"].clone()))
            .collect();
        assert_eq!(placed, [(Some(4), pipeline_id.clone())]);

        for refused in [
            plan(&mut stores, pipeline_id.clone()),
            validate(&mut stores, pipeline_id),
        ] {
            assert!(
                branch(&refused).ends_with("/wrong-state"),
                "{}",
                branch(&refused)
            );
            let error = refused.error.as_ref().expect("a declared error");
            assert_eq!(
                error.error.to_string(),
                "inference.graph.PipelineStateConflict"
            );
            assert_eq!(
                error.fields,
                BTreeMap::from([("current".to_owned(), text("Planned"))])
            );
            assert!(refused.direct_events.is_empty());
        }
    }

    #[test]
    fn planning_a_declared_or_unknown_pipeline_is_wrong_state() {
        let mut stores = Stores::default();
        let profile_id = profile(&mut stores);
        let pipeline_id = pipeline(&mut stores, profile_id);

        let declared = plan(&mut stores, pipeline_id);
        assert_eq!(
            branch(&declared),
            "inference.graph.PlanPipeline/wrong-state"
        );
        assert_eq!(
            declared.error.as_ref().expect("a declared error").fields,
            BTreeMap::from([("current".to_owned(), text("Declared"))])
        );
        assert_eq!(state_of(&mut stores), text("Declared"));

        let unknown = plan(&mut stores, text(UNKNOWN));
        assert_eq!(branch(&unknown), "inference.graph.PlanPipeline/wrong-state");
        let error = unknown.error.as_ref().expect("a declared error");
        assert_eq!(
            error.error.to_string(),
            "inference.graph.PipelineStateConflict"
        );
        assert!(error.fields.is_empty(), "{error:?}");
    }

    #[test]
    fn a_node_short_of_tool_calling_is_refused_naming_the_node_and_the_capability() {
        let mut stores = Stores::default();
        let (pipeline_id, node_id) = one_model(&mut stores, &["tool_calling"], true);

        let refused = plan(&mut stores, pipeline_id.clone());
        assert_eq!(branch(&refused), "inference.graph.PlanPipeline/mismatch");
        let error = refused.error.as_ref().expect("a declared error");
        assert_eq!(
            error.error.to_string(),
            "inference.graph.CapabilityMismatch"
        );
        let fields = BTreeMap::from([
            ("node_id".to_owned(), node_id),
            ("capability".to_owned(), text("tool_calling")),
        ]);
        assert_eq!(error.fields, fields);
        assert!(refused.direct_events.is_empty());
        assert_eq!(state_of(&mut stores), text("Validated"));

        force(&mut stores, "inference.graph.PlanPipeline/mismatch").expect("mismatch is external");
        let forced = plan(&mut stores, pipeline_id);
        assert_eq!(branch(&forced), "inference.graph.PlanPipeline/mismatch");
        assert_eq!(
            forced.error.as_ref().expect("a declared error").fields,
            fields,
            "a forced mismatch names what the planner finds"
        );
    }

    #[test]
    fn a_node_requiring_tool_calling_with_no_binding_is_refused_as_unbound() {
        let mut stores = Stores::default();
        let (pipeline_id, node_id) = one_model(&mut stores, &["tool_calling"], false);

        let refused = plan(&mut stores, pipeline_id);
        assert_eq!(branch(&refused), "inference.graph.PlanPipeline/unbound");
        let error = refused.error.as_ref().expect("a declared error");
        assert_eq!(error.error.to_string(), "inference.graph.NodeUnbound");
        assert_eq!(
            error.fields,
            BTreeMap::from([("node_id".to_owned(), node_id)])
        );
        assert!(refused.direct_events.is_empty());
        assert_eq!(state_of(&mut stores), text("Validated"));
    }

    #[test]
    fn a_forced_planning_refusal_answers_the_next_plan_only_and_moves_nothing() {
        let mut stores = Stores::default();
        let (pipeline_id, node_id) = one_model(&mut stores, &[], true);
        let events = stores.graph.events().len();
        let adapter_id = rows(&mut stores, "inference.graph.Bindings")[0]["adapter_id"].clone();
        let profile_id = rows(&mut stores, "inference.graph.Pipelines")[0]["profile_id"].clone();
        let fields = |pairs: &[(&str, Node)]| -> BTreeMap<String, Node> {
            pairs
                .iter()
                .map(|(field, value)| ((*field).to_owned(), value.clone()))
                .collect()
        };

        for (outcome, error, carried) in [
            (
                "unbound",
                "inference.graph.NodeUnbound",
                fields(&[("node_id", node_id.clone())]),
            ),
            (
                "mismatch",
                "inference.graph.CapabilityMismatch",
                fields(&[
                    ("node_id", Node::Null),
                    ("capability", text("output_streaming")),
                ]),
            ),
            (
                "locality-denied",
                "inference.graph.LocalityDenied",
                fields(&[
                    ("node_id", node_id.clone()),
                    ("adapter_id", adapter_id.clone()),
                    ("locality", text("same_process")),
                    ("profile_id", profile_id.clone()),
                ]),
            ),
        ] {
            let name = format!("inference.graph.PlanPipeline/{outcome}");
            force(&mut stores, &name).expect("the outcome is external");
            let forced = plan(&mut stores, pipeline_id.clone());
            assert_eq!(branch(&forced), name);
            let answered = forced.error.as_ref().expect("a declared error");
            assert_eq!(answered.error.to_string(), error);
            assert_eq!(
                answered.fields, carried,
                "the planner finds no such refusal, so the first declared values are named"
            );
            assert!(forced.direct_events.is_empty());
            assert_eq!(state_of(&mut stores), text("Validated"));
            assert_eq!(stores.graph.events().len(), events);
        }

        let nil = text("00000000-0000-0000-0000-000000000000");
        for (outcome, carried) in [
            ("unbound", fields(&[("node_id", nil.clone())])),
            (
                "mismatch",
                fields(&[
                    ("node_id", Node::Null),
                    ("capability", text("output_streaming")),
                ]),
            ),
            (
                "locality-denied",
                fields(&[
                    ("node_id", nil.clone()),
                    ("adapter_id", nil.clone()),
                    ("locality", text("same_process")),
                    ("profile_id", nil.clone()),
                ]),
            ),
        ] {
            let name = format!("inference.graph.PlanPipeline/{outcome}");
            force(&mut stores, &name).expect("the outcome is external");
            let forced = plan(&mut stores, text(UNKNOWN));
            assert_eq!(branch(&forced), name);
            assert_eq!(
                forced.error.as_ref().expect("a declared error").fields,
                carried,
                "no record names a node, an adapter or a profile, so the nil identity stands in"
            );
        }

        let planned = plan(&mut stores, pipeline_id);
        assert_eq!(
            branch(&planned),
            "inference.graph.PlanPipeline/planned",
            "the force lapses after one invocation"
        );
        assert!(matches!(
            force(&mut stores, "inference.graph.PlanPipeline/wrong-state"),
            Err(TargetError::Unavailable { .. })
        ));
    }
}
