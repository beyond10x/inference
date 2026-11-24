//! `inference.tools`: the tool and tool-call commands, views and events of
//! `ess/domains/tools.yaml`, answered by [`ToolsDomain`].
//!
//! Each request field is read on its own ([`Input`]); the outcome, the error and the event come
//! from what the store returned. `ProposeToolCall` and `ProposeSpeculativeToolCall` read the
//! generations and spans of the `inference.stream` store through
//! [`ToolsDomain::propose_tool_call`] and [`ToolsDomain::propose_speculative_tool_call`]. A move
//! naming a call no record carries takes its `wrong-state` outcome, whose error then carries no
//! `current`, since no record holds a state to report; `AuthorizeToolCall`'s `unconfirmed` reads
//! the input alone and answers before the call is looked up. A `Json` field is carried as the
//! value the request sent, read and written through `serde_json`; `failure` is read field by
//! field, and any field it does not declare is an error.
//!
//! [`ToolsDomain::propose_tool_call`]: inference_testkit::tools::ToolsDomain::propose_tool_call
//! [`ToolsDomain::propose_speculative_tool_call`]: inference_testkit::tools::ToolsDomain::propose_speculative_tool_call
//!
//! A command's consistency token is the target's ([`Stores::token`]), and a read demanding a token
//! the stores have not reached is an error ([`Stores::require`]). An observed event's position is
//! its place in the target's one publication order ([`Stores::tools_position`]).

use std::collections::BTreeMap;

use ess_conformance::scenario::{CommandRef, ErrorRef, EventRef, OutcomeRef};
use ess_conformance::target::{
    DeclaredErrorValue, EventObservationRequest, ObservedEvent, SemanticCommandRequest,
    SemanticCommandResult, SemanticViewRequest, SemanticViewResult, TargetError, ViewRow,
};
use ess_primitives::node::Node;
use inference_core::generation::GenerationStateConflict;
use inference_testkit::tools::{
    AuthorizeToolCallError, ProposeSpeculativeToolCallError, ProposeToolCallError,
    ToolCallMoveError, ToolCallsRow, ToolsDomain, ToolsEvent, ToolsRow,
};
use inference_tools::{
    AuthorizeToolCall, CompleteToolCall, DeclareTool, FailToolCall, Json,
    ProposeSpeculativeToolCall, ProposeToolCall, RejectToolCall, ToolCallId, ToolFailure,
    ToolFailureKind,
};

use super::{Input, Stores, boolean_node, named_node, text_node, unreadable, unsupported};

/// The prefix of every name this module answers.
pub const DOMAIN: &str = "inference.tools";

const DECLARE_TOOL: &str = "inference.tools.DeclareTool";
const PROPOSE_TOOL_CALL: &str = "inference.tools.ProposeToolCall";
const PROPOSE_SPECULATIVE_TOOL_CALL: &str = "inference.tools.ProposeSpeculativeToolCall";
const AUTHORIZE_TOOL_CALL: &str = "inference.tools.AuthorizeToolCall";
const REJECT_TOOL_CALL: &str = "inference.tools.RejectToolCall";
const START_TOOL_CALL: &str = "inference.tools.StartToolCall";
const COMPLETE_TOOL_CALL: &str = "inference.tools.CompleteToolCall";
const FAIL_TOOL_CALL: &str = "inference.tools.FailToolCall";

const TOOL_DECLARED: &str = "inference.tools.ToolDeclared";
const TOOL_CALL_PROPOSED: &str = "inference.tools.ToolCallProposed";
const TOOL_CALL_AUTHORIZED: &str = "inference.tools.ToolCallAuthorized";
const TOOL_CALL_REJECTED: &str = "inference.tools.ToolCallRejected";
const TOOL_CALL_STARTED: &str = "inference.tools.ToolCallStarted";
const TOOL_CALL_RESULT: &str = "inference.tools.ToolCallResult";
const TOOL_CALL_FAILED: &str = "inference.tools.ToolCallFailed";

/// Every event this module observes.
const EVENTS: [&str; 7] = [
    TOOL_DECLARED,
    TOOL_CALL_PROPOSED,
    TOOL_CALL_AUTHORIZED,
    TOOL_CALL_REJECTED,
    TOOL_CALL_STARTED,
    TOOL_CALL_RESULT,
    TOOL_CALL_FAILED,
];

const TOOL_NOT_FOUND: &str = "inference.tools.ToolNotFound";
const PROVISIONAL_INPUT_BARRIER: &str = "inference.tools.ProvisionalInputBarrier";
const TOOL_CALL_STATE_CONFLICT: &str = "inference.tools.ToolCallStateConflict";
const CONFIRMATION_REQUIRED: &str = "inference.tools.ConfirmationRequired";
const GENERATION_NOT_FOUND: &str = "inference.stream.GenerationNotFound";
const GENERATION_STATE_CONFLICT: &str = "inference.stream.GenerationStateConflict";
const SPAN_NOT_FOUND: &str = "inference.stream.SpanNotFound";

const TOOLS: &str = "inference.tools.Tools";
const TOOL_CALLS: &str = "inference.tools.ToolCalls";

/// The tool and tool-call stores of one scenario.
pub type State = ToolsDomain;

pub fn execute_command(
    stores: &mut Stores,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    stores.publish();
    let stream = &stores.stream;
    let domain = &mut stores.tools;
    let name = request.command.to_string();
    let result = match name.as_str() {
        DECLARE_TOOL => declare_tool(domain, request)?,
        PROPOSE_TOOL_CALL => {
            let mut input = Input::of(request);
            let propose = ProposeToolCall {
                tool_id: input.named("tool_id")?,
                generation_id: input.named("generation_id")?,
                basis_span_id: input.named("basis_span_id")?,
                arguments: json(input.node("arguments")?)?,
            };
            input.finish()?;
            match domain.propose_tool_call(stream, propose) {
                Ok(event) => accepted(
                    PROPOSE_TOOL_CALL,
                    "proposed",
                    &ToolsEvent::ToolCallProposed(event),
                    request,
                )?,
                Err(ProposeToolCallError::UnknownTool(error)) => refused(
                    PROPOSE_TOOL_CALL,
                    "unknown-tool",
                    DeclaredErrorValue::new(parse::<ErrorRef>(TOOL_NOT_FOUND))
                        .with("tool_id", named_node(&error.tool_id)?),
                ),
                Err(ProposeToolCallError::UnknownGeneration(error)) => refused(
                    PROPOSE_TOOL_CALL,
                    "unknown-generation",
                    DeclaredErrorValue::new(parse::<ErrorRef>(GENERATION_NOT_FOUND))
                        .with("generation_id", named_node(&error.generation_id)?),
                ),
                Err(ProposeToolCallError::UnknownSpan(error)) => refused(
                    PROPOSE_TOOL_CALL,
                    "unknown-span",
                    DeclaredErrorValue::new(parse::<ErrorRef>(SPAN_NOT_FOUND))
                        .with("span_id", named_node(&error.span_id)?),
                ),
                Err(ProposeToolCallError::StaleGeneration(conflict)) => refused(
                    PROPOSE_TOOL_CALL,
                    "stale-generation",
                    generation_conflict(conflict)?,
                ),
                Err(ProposeToolCallError::Barred(barrier)) => refused(
                    PROPOSE_TOOL_CALL,
                    "barred",
                    DeclaredErrorValue::new(parse::<ErrorRef>(PROVISIONAL_INPUT_BARRIER))
                        .with("side_effect", named_node(&barrier.side_effect)?),
                ),
            }
        }
        PROPOSE_SPECULATIVE_TOOL_CALL => {
            let mut input = Input::of(request);
            let propose = ProposeSpeculativeToolCall {
                tool_id: input.named("tool_id")?,
                generation_id: input.named("generation_id")?,
                basis_span_id: input.named("basis_span_id")?,
                arguments: json(input.node("arguments")?)?,
            };
            input.finish()?;
            let command = PROPOSE_SPECULATIVE_TOOL_CALL;
            match domain.propose_speculative_tool_call(stream, propose) {
                Ok(event) => accepted(
                    command,
                    "proposed",
                    &ToolsEvent::ToolCallProposed(event),
                    request,
                )?,
                Err(ProposeSpeculativeToolCallError::UnknownTool(error)) => refused(
                    command,
                    "unknown-tool",
                    DeclaredErrorValue::new(parse::<ErrorRef>(TOOL_NOT_FOUND))
                        .with("tool_id", named_node(&error.tool_id)?),
                ),
                Err(ProposeSpeculativeToolCallError::UnknownGeneration(error)) => refused(
                    command,
                    "unknown-generation",
                    DeclaredErrorValue::new(parse::<ErrorRef>(GENERATION_NOT_FOUND))
                        .with("generation_id", named_node(&error.generation_id)?),
                ),
                Err(ProposeSpeculativeToolCallError::UnknownSpan(error)) => refused(
                    command,
                    "unknown-span",
                    DeclaredErrorValue::new(parse::<ErrorRef>(SPAN_NOT_FOUND))
                        .with("span_id", named_node(&error.span_id)?),
                ),
                Err(ProposeSpeculativeToolCallError::StaleGeneration(conflict)) => {
                    refused(command, "stale-generation", generation_conflict(conflict)?)
                }
                Err(ProposeSpeculativeToolCallError::SideEffecting(barrier)) => refused(
                    command,
                    "side-effecting",
                    DeclaredErrorValue::new(parse::<ErrorRef>(PROVISIONAL_INPUT_BARRIER))
                        .with("side_effect", named_node(&barrier.side_effect)?),
                ),
            }
        }
        AUTHORIZE_TOOL_CALL => {
            let mut input = Input::of(request);
            let authorize = AuthorizeToolCall {
                call_id: input.named("call_id")?,
                confirmation_required: input.boolean("confirmation_required")?,
                confirmed: input.boolean("confirmed")?,
            };
            input.finish()?;
            match domain.authorize_tool_call(authorize) {
                Ok(event) => accepted(
                    AUTHORIZE_TOOL_CALL,
                    "authorized",
                    &ToolsEvent::ToolCallAuthorized(event),
                    request,
                )?,
                Err(AuthorizeToolCallError::Unconfirmed(error)) => refused(
                    AUTHORIZE_TOOL_CALL,
                    "unconfirmed",
                    DeclaredErrorValue::new(parse::<ErrorRef>(CONFIRMATION_REQUIRED))
                        .with("call_id", named_node(&error.call_id)?),
                ),
                Err(AuthorizeToolCallError::Unknown { .. }) => {
                    wrong_state(AUTHORIZE_TOOL_CALL, None)?
                }
                Err(AuthorizeToolCallError::Conflict(conflict)) => {
                    wrong_state(AUTHORIZE_TOOL_CALL, Some(conflict.current))?
                }
            }
        }
        REJECT_TOOL_CALL => {
            let mut input = Input::of(request);
            let reject = RejectToolCall {
                call_id: input.named("call_id")?,
                reason: input.named("reason")?,
            };
            input.finish()?;
            let answer = domain.reject_tool_call(reject);
            moved(
                REJECT_TOOL_CALL,
                "rejected",
                answer.map(ToolsEvent::ToolCallRejected),
                request,
            )?
        }
        START_TOOL_CALL => {
            let mut input = Input::of(request);
            let call_id: ToolCallId = input.named("call_id")?;
            input.finish()?;
            let answer = domain.start_tool_call(call_id);
            moved(
                START_TOOL_CALL,
                "started",
                answer.map(ToolsEvent::ToolCallStarted),
                request,
            )?
        }
        COMPLETE_TOOL_CALL => {
            let mut input = Input::of(request);
            let complete = CompleteToolCall {
                call_id: input.named("call_id")?,
                result: json(input.node("result")?)?,
            };
            input.finish()?;
            let answer = domain.complete_tool_call(complete);
            moved(
                COMPLETE_TOOL_CALL,
                "completed",
                answer.map(ToolsEvent::ToolCallResult),
                request,
            )?
        }
        FAIL_TOOL_CALL => {
            let mut input = Input::of(request);
            let call_id = input.named("call_id")?;
            let failure = tool_failure(input.node("failure")?)?;
            input.finish()?;
            let answer = domain.fail_tool_call(FailToolCall { call_id, failure });
            moved(
                FAIL_TOOL_CALL,
                "failed",
                answer.map(ToolsEvent::ToolCallFailed),
                request,
            )?
        }
        _ => return Err(unsupported(&name)),
    };
    stores.publish();
    Ok(result.with_consistency(stores.token()?))
}

fn declare_tool(
    domain: &mut ToolsDomain,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let declare = DeclareTool {
        name: input.text("name")?,
        description: input.text("description")?,
        schema: json(input.node("schema")?)?,
        schema_revision: input.text("schema_revision")?,
        side_effect: input.named("side_effect")?,
        requires_confirmation: input.boolean("requires_confirmation")?,
    };
    input.finish()?;
    let event = domain.declare_tool(declare);
    accepted(
        DECLARE_TOOL,
        "declared",
        &ToolsEvent::ToolDeclared(event),
        request,
    )
}

/// The accepting outcome `branch` of `command`, emitting `event`.
fn accepted(
    command: &str,
    branch: &str,
    event: &ToolsEvent,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    Ok(SemanticCommandResult::took(outcome(command, branch))
        .emitting(tools_event(event)?.in_activity(request.correlation.clone())))
}

/// The refusing outcome `branch` of `command`, carrying `error`.
fn refused(command: &str, branch: &str, error: DeclaredErrorValue) -> SemanticCommandResult {
    SemanticCommandResult::took(outcome(command, branch)).with_error(error)
}

/// The `wrong-state` outcome of `command`: `ToolCallStateConflict`, carrying the state the call
/// holds, or nothing when no call carries the identity sent.
fn wrong_state(
    command: &str,
    current: Option<inference_tools::ToolCallState>,
) -> Result<SemanticCommandResult, TargetError> {
    let error = DeclaredErrorValue::new(parse::<ErrorRef>(TOOL_CALL_STATE_CONFLICT));
    let error = match current {
        Some(current) => error.with("current", named_node(&current)?),
        None => error,
    };
    Ok(refused(command, "wrong-state", error))
}

/// The answer of a call move other than `AuthorizeToolCall`: `branch` and its event, or
/// `wrong-state`.
fn moved(
    command: &str,
    branch: &str,
    answer: Result<ToolsEvent, ToolCallMoveError>,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    match answer {
        Ok(event) => accepted(command, branch, &event, request),
        Err(ToolCallMoveError::Unknown { .. }) => wrong_state(command, None),
        Err(ToolCallMoveError::Conflict(conflict)) => wrong_state(command, Some(conflict.current)),
    }
}

/// `inference.stream.GenerationStateConflict`, carrying the state the generation holds.
fn generation_conflict(
    conflict: GenerationStateConflict,
) -> Result<DeclaredErrorValue, TargetError> {
    Ok(
        DeclaredErrorValue::new(parse::<ErrorRef>(GENERATION_STATE_CONFLICT))
            .with("current", named_node(&conflict.current)?),
    )
}

/// An ESS `Json` value read from the node the request carries.
fn json(node: &Node) -> Result<Json, TargetError> {
    serde_json::to_value(node)
        .map_err(|error| unreadable("reading a `Json` value", format!("{error}: {node:?}")))
}

/// The node of an ESS `Json` value.
fn json_node(value: &Json) -> Result<Node, TargetError> {
    serde_json::from_value(value.clone())
        .map_err(|error| unreadable("writing a `Json` value", format!("{error}: {value}")))
}

/// An `inference.tools.ToolFailure` read field by field: `kind`, a declared variant, and `detail`,
/// a string. Any other field, a missing one or a field of another type is an error.
fn tool_failure(node: &Node) -> Result<ToolFailure, TargetError> {
    let refuse = |detail: String| unreadable(format!("reading `{FAIL_TOOL_CALL}`"), detail);
    let Node::Map(fields) = node else {
        return Err(refuse(format!("`failure` is not a record: {node:?}")));
    };
    let mut kind = None;
    let mut detail = None;
    for (field, value) in fields {
        match (field.as_str(), value) {
            ("kind", Node::Text(text)) => {
                kind = Some(
                    serde_json::from_value::<ToolFailureKind>(serde_json::Value::String(
                        text.clone(),
                    ))
                    .map_err(|_| {
                        refuse(format!("`failure.kind` is not a declared kind: {text}"))
                    })?,
                );
            }
            ("detail", Node::Text(text)) => detail = Some(text.clone()),
            _ => {
                return Err(refuse(format!(
                    "`failure.{field}` is undeclared or not of its declared type: {value:?}"
                )));
            }
        }
    }
    Ok(ToolFailure {
        kind: kind.ok_or_else(|| refuse("`failure` carries no `kind`".to_owned()))?,
        detail: detail.ok_or_else(|| refuse("`failure` carries no `detail`".to_owned()))?,
    })
}

/// The node of an `inference.tools.ToolFailure`.
fn tool_failure_node(failure: &ToolFailure) -> Result<Node, TargetError> {
    Ok(Node::Map(BTreeMap::from([
        ("kind".to_owned(), named_node(&failure.kind)?),
        ("detail".to_owned(), text_node(&failure.detail)),
    ])))
}

pub fn query_view(
    stores: &mut Stores,
    request: &SemanticViewRequest,
) -> Result<SemanticViewResult, TargetError> {
    let name = request.view.to_string();
    if ![TOOLS, TOOL_CALLS].contains(&name.as_str()) {
        return Err(unsupported(&name));
    }
    stores.require(&name, &request.consistency)?;
    let domain = &stores.tools;
    let rows = if name == TOOLS {
        domain
            .tools_view()
            .iter()
            .map(tools_row)
            .collect::<Result<Vec<_>, _>>()?
    } else {
        domain
            .tool_calls_view()
            .iter()
            .map(tool_calls_row)
            .collect::<Result<Vec<_>, _>>()?
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
    for (index, event) in stores.tools.events().iter().enumerate() {
        let event = tools_event(event)?;
        if event.event == request.event {
            let position = stores.tools_position(index)?;
            observed.push(event.in_activity(request.correlation.clone()).at(position));
        }
    }
    Ok(observed)
}

/// An event of the store as the suite observes it, every declared field carried.
fn tools_event(event: &ToolsEvent) -> Result<ObservedEvent, TargetError> {
    let call = |name: &str, call_id: ToolCallId| {
        Ok::<_, TargetError>(
            ObservedEvent::new(parse::<EventRef>(name)).with("call_id", named_node(&call_id)?),
        )
    };
    Ok(match event {
        ToolsEvent::ToolDeclared(event) => ObservedEvent::new(parse::<EventRef>(TOOL_DECLARED))
            .with("tool_id", named_node(&event.tool_id)?)
            .with("name", text_node(&event.name))
            .with("side_effect", named_node(&event.side_effect)?),
        ToolsEvent::ToolCallProposed(event) => call(TOOL_CALL_PROPOSED, event.call_id)?
            .with("tool_id", named_node(&event.tool_id)?)
            .with("generation_id", named_node(&event.generation_id)?),
        ToolsEvent::ToolCallAuthorized(event) => call(TOOL_CALL_AUTHORIZED, event.call_id)?,
        ToolsEvent::ToolCallRejected(event) => {
            call(TOOL_CALL_REJECTED, event.call_id)?.with("reason", named_node(&event.reason)?)
        }
        ToolsEvent::ToolCallStarted(event) => call(TOOL_CALL_STARTED, event.call_id)?,
        ToolsEvent::ToolCallResult(event) => {
            call(TOOL_CALL_RESULT, event.call_id)?.with("result", json_node(&event.result)?)
        }
        ToolsEvent::ToolCallFailed(event) => call(TOOL_CALL_FAILED, event.call_id)?
            .with("failure", tool_failure_node(&event.failure)?),
    })
}

fn tools_row(row: &ToolsRow) -> Result<ViewRow, TargetError> {
    Ok(BTreeMap::from([
        ("tool_id".to_owned(), named_node(&row.tool_id)?),
        ("name".to_owned(), text_node(&row.name)),
        ("description".to_owned(), text_node(&row.description)),
        ("schema".to_owned(), json_node(&row.schema)?),
        (
            "schema_revision".to_owned(),
            text_node(&row.schema_revision),
        ),
        ("side_effect".to_owned(), named_node(&row.side_effect)?),
        (
            "requires_confirmation".to_owned(),
            boolean_node(row.requires_confirmation),
        ),
    ]))
}

fn tool_calls_row(row: &ToolCallsRow) -> Result<ViewRow, TargetError> {
    Ok(BTreeMap::from([
        ("call_id".to_owned(), named_node(&row.call_id)?),
        ("tool_id".to_owned(), named_node(&row.tool_id)?),
        ("generation_id".to_owned(), named_node(&row.generation_id)?),
        ("side_effect".to_owned(), named_node(&row.side_effect)?),
        ("basis_span_id".to_owned(), named_node(&row.basis_span_id)?),
        ("arguments".to_owned(), json_node(&row.arguments)?),
        ("state".to_owned(), named_node(&row.state)?),
    ]))
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
