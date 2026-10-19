//! `inference.graph`: the five declaration commands and `ValidatePipeline`, the five views and the
//! six events of `ess/domains/graph.yaml`, answered by [`GraphDomain`].
//!
//! Each request field is read on its own ([`Input`]); the outcome, the error and the event come
//! from what the store returned. `DeclarePipeline` and `BindNode` read the declared profiles and
//! adapters through [`GraphCatalog`] over the `inference.policy` and `inference.model` stores. A
//! command's consistency token is the target's ([`Stores::token`]), and a read demanding a token
//! the stores have not reached is an error ([`Stores::require`]). An observed event's position is
//! its place in the target's one publication order ([`Stores::graph_position`]).
//!
//! `ValidatePipeline` naming a pipeline no record carries takes its `wrong-state` outcome, whose
//! error then carries no `current`, since no record holds a state to report. Its `rejected`
//! outcome is declared `external`: the store takes it when a check finds something, and a scenario
//! may force it ([`configure_external_outcome`]) on the next invocation, which then answers with
//! the findings the checks make over that pipeline, none for a valid or unknown one, and moves
//! nothing.
//!
//! Planning and runs (`PlanPipeline`, the run commands, the views `Runs` and `NodeInstances` and
//! their events) are [`TargetError::Unsupported`]: no store decides them yet, and they are not
//! faked. `ess/SKIPPED.md` names the story for each.

use std::collections::BTreeMap;

use ess_conformance::scenario::{CommandRef, ErrorRef, EventRef, OutcomeRef};
use ess_conformance::target::{
    DeclaredErrorValue, EventObservationRequest, ExternalOutcomeControl, ObservedEvent,
    SemanticCommandRequest, SemanticCommandResult, SemanticViewRequest, SemanticViewResult,
    TargetError, ViewRow,
};
use ess_primitives::node::Node;
use inference_core::model::Capability;
use inference_pipeline::{
    AddNode, AddNodeError, AddPort, BindNode, BindNodeError, Binding, ConnectPorts,
    ConnectPortsError, DeclarePipeline, Edge, GraphFinding, NodeAdded, NodeBound, PipelineDeclared,
    PipelineNotFound, PipelineNotReady, PipelineValidated, Port, PortAdded, PortsConnected,
    ValidatePipeline, ValidatePipelineError,
};
use inference_testkit::graph::{GraphCatalog, GraphDomain, GraphEvent};

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

/// The one outcome of an answered command that `ess/domains/graph.yaml` declares external.
const REJECTED: &str = "rejected";

const PIPELINE_DECLARED: &str = "inference.graph.PipelineDeclared";
const NODE_ADDED: &str = "inference.graph.NodeAdded";
const PORT_ADDED: &str = "inference.graph.PortAdded";
const PORTS_CONNECTED: &str = "inference.graph.PortsConnected";
const NODE_BOUND: &str = "inference.graph.NodeBound";
const PIPELINE_VALIDATED: &str = "inference.graph.PipelineValidated";

const PROFILE_NOT_FOUND: &str = "inference.policy.ProfileNotFound";
const PIPELINE_NOT_FOUND: &str = "inference.graph.PipelineNotFound";
const PIPELINE_NOT_READY: &str = "inference.graph.PipelineNotReady";
const NODE_NOT_FOUND: &str = "inference.graph.NodeNotFound";
const PORT_NOT_FOUND: &str = "inference.graph.PortNotFound";
const UNBOUNDED_EDGE: &str = "inference.graph.UnboundedEdge";
const INCOMPATIBLE_EDGE: &str = "inference.graph.IncompatibleEdge";
const ADAPTER_NOT_FOUND: &str = "inference.graph.AdapterNotFound";
const INCOMPATIBLE_GRAPH: &str = "inference.graph.IncompatibleGraph";
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
        _ => return Err(unsupported(&name)),
    };
    stores.publish();
    Ok(result.with_consistency(stores.token()?))
}

/// Accepts a force of `ValidatePipeline/rejected`, the one external outcome of a command this
/// module answers; refuses a force of another `ValidatePipeline` outcome as an error, as no
/// specification-declared external outcome, and any other command's as unsupported.
pub fn configure_external_outcome(
    stores: &mut Stores,
    request: &ExternalOutcomeControl,
) -> Result<(), TargetError> {
    let command = request.force.command.to_string();
    if command != VALIDATE_PIPELINE {
        return Err(unsupported(&request.force.to_string()));
    }
    if request.force != outcome(VALIDATE_PIPELINE, REJECTED) {
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
        Err(error) => SemanticCommandResult::took(outcome(ADD_PORT, "unknown-node")).with_error(
            DeclaredErrorValue::new(parse::<ErrorRef>(NODE_NOT_FOUND))
                .with("node_id", named_node(&error.node_id)?),
        ),
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

fn ports_row(port: &Port) -> Result<ViewRow, TargetError> {
    Ok(BTreeMap::from([
        ("port_id".to_owned(), named_node(&port.port_id())?),
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

    fn port_input(node_id: Node, direction: &str, incremental: Node) -> Vec<(&'static str, Node)> {
        vec![
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

    fn port(stores: &mut Stores, node_id: Node, direction: &str) -> Node {
        let result = run(
            stores,
            "inference.graph.AddPort",
            &port_input(node_id, direction, Node::Bool(true)),
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
        let output = port(&mut stores, node_id.clone(), "output");
        let input = port(&mut stores, node_id.clone(), "input");
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
        assert_eq!(ports[0].len(), 10);

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
        let input = port(&mut stores, node_id, "input");

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
                &port_input(node_id.clone(), "input", text("true")),
            ),
        );
        assert!(
            matches!(&textual, Err(TargetError::Unavailable { .. })),
            "{textual:?}"
        );
        let mut extra = port_input(node_id, "input", Node::Bool(true));
        extra.push(("pipeline_id", pipeline_id.clone()));
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
        let output = port(&mut stores, node_id.clone(), "output");

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
        let planning = force("inference.graph.PlanPipeline/mismatch");
        assert!(
            matches!(&planning, Err(TargetError::Unsupported { .. })),
            "{planning:?}"
        );
    }

    #[test]
    fn planning_and_runs_are_unsupported_not_faked() {
        let mut stores = Stores::default();
        for name in ["inference.graph.PlanPipeline", "inference.graph.CreateRun"] {
            let answer = execute_command(&mut stores, &command(name, &[]));
            assert!(
                matches!(&answer, Err(TargetError::Unsupported { .. })),
                "{name}: {answer:?}"
            );
        }
        let event = observe_events(
            &mut stores,
            &EventObservationRequest {
                event: "inference.graph.PipelinePlanned"
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
}
