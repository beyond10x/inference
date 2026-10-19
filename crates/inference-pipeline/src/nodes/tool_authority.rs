//! The tool authority: the one node of a pipeline that authorizes and rejects tool calls
//! (`docs/design/inference.md` §15.1, 699), a node of role `policy` with one input port for
//! proposed calls and one output port for authorized calls.
//!
//! The authority decides each call once. A proposal of a call it already holds or decided is
//! refused as `AuthorizeToolCall/outcome/wrong-state`, naming the state it holds the call in, and
//! is neither held again nor sent on. For any other proposal its input edge delivers, it rejects
//! a call for a tool it was not constructed with (`denied`), then validates the arguments against
//! the tool's schema and rejects a call that fails it (`schema_invalid`, 848), then runs the
//! default policy: it authorizes the call, passing the tool's `requires_confirmation` as
//! `confirmation_required` and `confirmed` false. A tool that requires confirmation answers
//! `unconfirmed`: the call is held Proposed until the host answers it through the
//! [`AuthorityHandle`], the confirmation hook (853, 1054). A confirmed call is authorized with
//! `confirmed` true; a denied one is rejected `denied`. An authorized call is sent on the output
//! edge; a rejected one is not, and every decision is recorded on the handle.
//!
//! The schema is read as a subset of JSON Schema, the language `ess/domains/tools.yaml` leaves
//! unmapped: `type`, `properties`, `required`, `additionalProperties`, `items` and `enum`, the
//! annotations `title`, `description`, `$schema`, `$id`, `$comment`, `default` and `examples`,
//! and the schemas `true` and `false`. `enum` compares numbers by value, as `integer` does. A
//! tool whose schema uses another keyword, or one of these in a form other than JSON Schema's,
//! fails the node's preparation rather than being validated in part.
//!
//! On the end of input, a control event of kind `stop`, or once its input edge is closed, the
//! node waits for the host to answer every call it holds, then passes the end of input on and
//! ends. Cancelled, it abandons the calls it holds and sends nothing more.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::future;
use std::sync::Arc;

use inference_core::endpoint::{InputEndpoint, OutputEndpoint};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId};
use inference_core::id::{NodeId, RunId};
use inference_tools::{
    AuthorizeError, AuthorizeToolCall, ConfirmationRequired, Json, RejectionReason, Tool, ToolCall,
    ToolCallAuthorized, ToolCallId, ToolCallRejected, ToolCallState, ToolCallStateConflict, ToolId,
};

use super::{
    Emitter, Next, Task, ToolCallEvent, ToolCallUpdate, ToolPayload, Watched, block_on, next,
    one_in_one_out,
};
use crate::{Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation};

/// What the authority decided for one call: the outcome of `inference.tools.AuthorizeToolCall`
/// or `inference.tools.RejectToolCall`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorityOutcome {
    /// `AuthorizeToolCall/outcome/authorized`: the call is Authorized and was sent on.
    Authorized(ToolCallAuthorized),
    /// `AuthorizeToolCall/outcome/unconfirmed`: the tool requires confirmation and none was
    /// given; the call is held Proposed for the confirmation hook.
    Unconfirmed(ConfirmationRequired),
    /// `RejectToolCall/outcome/rejected`: the call is Rejected and was not sent on.
    Rejected(ToolCallRejected),
    /// `AuthorizeToolCall/outcome/wrong-state`: the call was proposed again after the authority
    /// held or decided it; `current` is the state the authority holds it in. Nothing moved.
    WrongState(ToolCallStateConflict),
}

/// One decision of the authority: the call as the decision left it, and the outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub call: ToolCall,
    pub outcome: AuthorityOutcome,
}

/// A call held for confirmation, with the envelope that proposed it.
#[derive(Debug, Clone)]
struct Held {
    call: ToolCall,
    cause: EventId,
}

/// What the authority holds and has done.
#[derive(Debug, Default)]
struct AuthorityState {
    held: Vec<Held>,
    /// The hook's answers not yet acted on: the call, and whether it was confirmed.
    answers: VecDeque<(ToolCallId, bool)>,
    decisions: Vec<Decision>,
    cancelled: bool,
    ended: bool,
    failure: Option<String>,
}

impl fmt::Debug for Watched<AuthorityState> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.read(|state| state.fmt(f))
    }
}

/// What the exchange acts on besides its input.
enum Signal {
    Cancel,
    Answer(Held, bool),
    Settled,
}

impl AuthorityState {
    /// A cancel first, then an answered call, then, once the input ended, that nothing is held.
    fn signal(&mut self, input_ended: bool) -> Option<Signal> {
        if self.cancelled {
            return Some(Signal::Cancel);
        }
        if let Some((call_id, confirmed)) = self.answers.pop_front()
            && let Some(index) = self
                .held
                .iter()
                .position(|held| held.call.call_id() == call_id)
        {
            return Some(Signal::Answer(self.held.remove(index), confirmed));
        }
        (input_ended && self.held.is_empty()).then_some(Signal::Settled)
    }
}

/// The tool authority node; see the module documentation.
#[derive(Debug)]
pub struct ToolAuthority {
    run_id: RunId,
    tools: Arc<HashMap<ToolId, Tool>>,
    node_id: Option<NodeId>,
    state: Arc<Watched<AuthorityState>>,
    task: Task,
}

impl ToolAuthority {
    /// An authority of the run `run_id` deciding calls for `tools`, the declared tools it knows.
    #[must_use]
    pub fn new(run_id: RunId, tools: impl IntoIterator<Item = Tool>) -> Self {
        Self {
            run_id,
            tools: Arc::new(
                tools
                    .into_iter()
                    .map(|tool| (tool.tool_id(), tool))
                    .collect(),
            ),
            node_id: None,
            state: Arc::new(Watched::new(AuthorityState::default())),
            task: Task::new(),
        }
    }

    /// The host's side of the authority: the confirmation hook and the record of its decisions.
    #[must_use]
    pub fn handle(&self) -> AuthorityHandle {
        AuthorityHandle {
            state: Arc::clone(&self.state),
        }
    }

    fn failure(&self) -> Result<(), NodeError> {
        self.state
            .read(|state| state.failure.clone())
            .map_or(Ok(()), |failure| Err(NodeError::new(failure)))
    }
}

/// The host's side of a [`ToolAuthority`]: the confirmation hook, which confirms or denies a call
/// held for confirmation, and the record of every decision. Clones share one authority.
#[derive(Debug, Clone)]
pub struct AuthorityHandle {
    state: Arc<Watched<AuthorityState>>,
}

impl AuthorityHandle {
    /// Every call held Proposed for confirmation, in the order it was held.
    #[must_use]
    pub fn held(&self) -> Vec<ToolCall> {
        self.state
            .read(|state| state.held.iter().map(|held| held.call.clone()).collect())
    }

    /// Confirms the held call `call_id`: the authority authorizes it with `confirmed` true. False,
    /// confirming nothing, when no call of that identity is held, when it was answered already,
    /// or once the authority was cancelled or ended.
    #[must_use]
    pub fn confirm(&self, call_id: ToolCallId) -> bool {
        self.answer(call_id, true)
    }

    /// Denies the held call `call_id`: the authority rejects it `denied`. False, denying nothing,
    /// as [`AuthorityHandle::confirm`] answers false.
    #[must_use]
    pub fn deny(&self, call_id: ToolCallId) -> bool {
        self.answer(call_id, false)
    }

    fn answer(&self, call_id: ToolCallId, confirmed: bool) -> bool {
        self.state.update(|state| {
            let held = state.held.iter().any(|held| held.call.call_id() == call_id);
            let answered = state
                .answers
                .iter()
                .any(|(answered, _)| *answered == call_id);
            if state.cancelled || state.ended || !held || answered {
                return false;
            }
            state.answers.push_back((call_id, confirmed));
            true
        })
    }

    /// Every decision of the authority, in the order it made them.
    #[must_use]
    pub fn decisions(&self) -> Vec<Decision> {
        self.state.read(|state| state.decisions.clone())
    }
}

/// One started exchange of a [`ToolAuthority`].
struct Exchanging<P> {
    tools: Arc<HashMap<ToolId, Tool>>,
    state: Arc<Watched<AuthorityState>>,
    /// Every call the authority held or decided, as it last left it.
    known: HashMap<ToolCallId, ToolCall>,
    input: Box<dyn InputEndpoint<P>>,
    output: Box<dyn OutputEndpoint<P>>,
    emitter: Emitter,
}

impl<P: ToolPayload> Exchanging<P> {
    /// Sends `envelope` on, unless the authority was cancelled: it then sends nothing more.
    async fn send(&self, envelope: EventEnvelope<P>) -> Result<(), String> {
        if self.state.read(|state| state.cancelled) {
            return Ok(());
        }
        self.output
            .send(envelope)
            .await
            .map_err(|refused| refused.to_string())
    }

    /// Records `decision` and sends the authorized call on, answering `cause`.
    async fn decided(&mut self, decision: Decision, cause: EventId) -> Result<(), String> {
        let sent = match &decision.outcome {
            AuthorityOutcome::Authorized(event) => Some(ToolCallUpdate {
                call: decision.call.clone(),
                event: ToolCallEvent::Authorized(*event),
            }),
            AuthorityOutcome::Unconfirmed(_) => {
                let held = Held {
                    call: decision.call.clone(),
                    cause,
                };
                self.state.update(|state| state.held.push(held));
                None
            }
            AuthorityOutcome::Rejected(_) | AuthorityOutcome::WrongState(_) => None,
        };
        if !matches!(decision.outcome, AuthorityOutcome::WrongState(_)) {
            self.known
                .insert(decision.call.call_id(), decision.call.clone());
        }
        self.state.update(|state| state.decisions.push(decision));
        match sent {
            Some(update) => {
                let envelope = self.emitter.update(update, cause);
                self.send(envelope).await
            }
            None => Ok(()),
        }
    }

    /// Rejects `call` with `reason`.
    fn reject(call: &ToolCall, reason: RejectionReason) -> Result<Decision, String> {
        let (call, event) = call.reject(reason).map_err(|refused| refused.to_string())?;
        Ok(Decision {
            call,
            outcome: AuthorityOutcome::Rejected(event),
        })
    }

    /// Authorizes `call` for `tool`, with the hook's answer `confirmed`.
    fn authorize(call: &ToolCall, tool: &Tool, confirmed: bool) -> Result<Decision, String> {
        let input = AuthorizeToolCall {
            call_id: call.call_id(),
            confirmation_required: tool.requires_confirmation(),
            confirmed,
        };
        match call.authorize(&input) {
            Ok((call, event)) => Ok(Decision {
                call,
                outcome: AuthorityOutcome::Authorized(event),
            }),
            Err(AuthorizeError::Unconfirmed(refusal)) => Ok(Decision {
                call: call.clone(),
                outcome: AuthorityOutcome::Unconfirmed(refusal),
            }),
            Err(AuthorizeError::Conflict(conflict)) => Err(conflict.to_string()),
        }
    }

    /// The decision for the proposed `call`: a call held or decided already, then the tool, then
    /// the schema, then the policy.
    fn decide(&self, call: &ToolCall) -> Result<Decision, String> {
        if let Some(known) = self.known.get(&call.call_id()) {
            return Ok(Decision {
                call: known.clone(),
                outcome: AuthorityOutcome::WrongState(ToolCallStateConflict {
                    current: known.state(),
                }),
            });
        }
        let Some(tool) = self.tools.get(&call.tool_id()) else {
            return Self::reject(call, RejectionReason::Denied);
        };
        if !admits(tool.schema(), call.arguments()) {
            return Self::reject(call, RejectionReason::SchemaInvalid);
        }
        Self::authorize(call, tool, false)
    }

    /// The decision for the content of an envelope: a proposed call, or a failure naming what
    /// arrived instead.
    async fn proposal(&mut self, payload: P, kind: &str, cause: EventId) -> Result<(), String> {
        let call = match payload.into_tool_call() {
            Ok(update) if update.call.state() == ToolCallState::Proposed => update.call,
            Ok(update) => {
                return Err(format!(
                    "the tool authority takes proposed calls; call {} arrived {:?}",
                    update.call.call_id(),
                    update.call.state()
                ));
            }
            Err(_) => {
                return Err(format!(
                    "the tool authority takes proposed calls; an event of kind {kind} arrived"
                ));
            }
        };
        let decision = self.decide(&call)?;
        self.decided(decision, cause).await
    }

    /// The decision for the held call `held`, once the hook answered `confirmed`.
    fn answered(&self, held: &Held, confirmed: bool) -> Result<Decision, String> {
        if !confirmed {
            return Self::reject(&held.call, RejectionReason::Denied);
        }
        let tool = self
            .tools
            .get(&held.call.tool_id())
            .ok_or_else(|| format!("the tool of the held call {} is gone", held.call.call_id()))?;
        Self::authorize(&held.call, tool, true)
    }

    async fn exchange(&mut self) -> Result<(), String> {
        let mut input_ended = false;
        let mut end_of_input = false;
        loop {
            let next = if input_ended {
                let state = Arc::clone(&self.state);
                Next::Signal(state.until(|state| state.signal(true)).await)
            } else {
                let state = Arc::clone(&self.state);
                next(self.input.as_mut(), &state, |state| state.signal(false)).await
            };
            match next {
                Next::Signal(Signal::Cancel) => return Ok(()),
                Next::Signal(Signal::Answer(held, confirmed)) => {
                    let decision = self.answered(&held, confirmed)?;
                    self.decided(decision, held.cause).await?;
                }
                Next::Signal(Signal::Settled) => {
                    if end_of_input {
                        let end = self.emitter.control(ControlKind::Stop);
                        self.send(end).await?;
                    }
                    return Ok(());
                }
                Next::Envelope(envelope) => match ControlKind::of(&envelope.kind) {
                    Some(ControlKind::Stop) => {
                        input_ended = true;
                        end_of_input = true;
                    }
                    Some(_) => {}
                    None => {
                        self.proposal(envelope.payload, &envelope.kind, envelope.event_id)
                            .await?;
                    }
                },
                Next::Closed => input_ended = true,
            }
        }
    }

    fn run(mut self) {
        let ended = block_on(self.exchange());
        self.state.update(|state| {
            state.ended = true;
            state.failure = ended.err();
        });
    }
}

impl<P: ToolPayload> NodeImplementation<P> for ToolAuthority {
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.node_id = Some(node.node_id());
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        let supported = self.tools.values().try_for_each(|tool| {
            supported(tool.schema()).map_err(|unsupported| {
                NodeError::new(format!(
                    "the schema of tool {} ({}) uses {unsupported}, which the tool authority does \
                     not validate",
                    tool.name(),
                    tool.tool_id()
                ))
            })
        });
        Box::pin(future::ready(supported))
    }

    fn start(&mut self, endpoints: NodeEndpoints<P>) -> NodeFuture<'_, Result<(), NodeError>> {
        let started = one_in_one_out(self.node_id, endpoints).map(|(node_id, input, output)| {
            let exchanging = Exchanging {
                tools: Arc::clone(&self.tools),
                state: Arc::clone(&self.state),
                known: HashMap::new(),
                input: input.endpoint,
                emitter: Emitter::new(self.run_id, node_id, output.port_id.to_string()),
                output: output.endpoint,
            };
            self.task.spawn(move || exchanging.run());
        });
        Box::pin(future::ready(started))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(async move {
            self.task.finished().await.map_err(NodeError::new)?;
            self.failure()
        })
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        self.state.update(|state| state.cancelled = true);
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(async move {
            let _already_reported_by_drain = self.task.finished().await;
        })
    }
}

/// The JSON Schema type names [`typed`] reads.
const TYPES: [&str; 7] = [
    "object", "array", "string", "boolean", "null", "number", "integer",
];

/// The annotations a schema may carry, which validate nothing.
const ANNOTATIONS: [&str; 7] = [
    "title",
    "description",
    "$schema",
    "$id",
    "$comment",
    "default",
    "examples",
];

/// Nothing, when `schema` and every schema inside it uses only the keywords [`admits`]
/// validates, each in JSON Schema's form, and the [`ANNOTATIONS`]; otherwise what it uses
/// instead.
fn supported(schema: &Json) -> Result<(), String> {
    let keywords = match schema {
        Json::Bool(_) => return Ok(()),
        Json::Object(keywords) => keywords,
        other => return Err(format!("{other} as a schema")),
    };
    for (keyword, value) in keywords {
        let well_formed = match keyword.as_str() {
            "type" => match value {
                Json::String(name) => TYPES.contains(&name.as_str()),
                Json::Array(names) => names
                    .iter()
                    .all(|name| name.as_str().is_some_and(|name| TYPES.contains(&name))),
                _ => false,
            },
            "properties" => match value {
                Json::Object(properties) => {
                    properties.values().try_for_each(supported)?;
                    true
                }
                _ => false,
            },
            "required" => value
                .as_array()
                .is_some_and(|names| names.iter().all(Json::is_string)),
            "additionalProperties" | "items" => {
                supported(value)?;
                true
            }
            "enum" => value.is_array(),
            annotation if ANNOTATIONS.contains(&annotation) => true,
            other => return Err(format!("the keyword {other:?}")),
        };
        if !well_formed {
            return Err(format!("{keyword:?} in the form {value}"));
        }
    }
    Ok(())
}

/// Whether `value` is of the JSON Schema type `name`.
fn typed(name: &str, value: &Json) -> bool {
    match name {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        "integer" => {
            value.is_i64() || value.is_u64() || value.as_f64().is_some_and(|n| n.fract() == 0.0)
        }
        _ => false,
    }
}

/// Whether `a` and `b` are equal as JSON Schema compares instances: numbers by value, arrays
/// element by element, objects member by member.
fn equal(a: &Json, b: &Json) -> bool {
    match (a, b) {
        (Json::Number(x), Json::Number(y)) => {
            if let (Some(x), Some(y)) = (x.as_i64(), y.as_i64()) {
                x == y
            } else if let (Some(x), Some(y)) = (x.as_u64(), y.as_u64()) {
                x == y
            } else {
                x.as_f64() == y.as_f64()
            }
        }
        (Json::Array(x), Json::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| equal(a, b))
        }
        (Json::Object(x), Json::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(name, a)| y.get(name).is_some_and(|b| equal(a, b)))
        }
        _ => a == b,
    }
}

/// Whether `value` satisfies `schema`, read as the subset of JSON Schema [`supported`] admits.
fn admits(schema: &Json, value: &Json) -> bool {
    let Json::Object(keywords) = schema else {
        return schema.as_bool().unwrap_or(false);
    };
    let type_holds = match keywords.get("type") {
        None => true,
        Some(Json::String(name)) => typed(name, value),
        Some(Json::Array(names)) => names
            .iter()
            .any(|name| name.as_str().is_some_and(|name| typed(name, value))),
        Some(_) => false,
    };
    let enum_holds = match keywords.get("enum") {
        None => true,
        Some(Json::Array(allowed)) => allowed.iter().any(|member| equal(member, value)),
        Some(_) => false,
    };
    if !type_holds || !enum_holds {
        return false;
    }
    if let Json::Object(members) = value {
        let properties = keywords.get("properties").and_then(Json::as_object);
        let required = keywords
            .get("required")
            .and_then(Json::as_array)
            .map_or(&[][..], Vec::as_slice);
        if !required
            .iter()
            .all(|name| name.as_str().is_some_and(|name| members.contains_key(name)))
        {
            return false;
        }
        let additional = keywords.get("additionalProperties");
        let members_hold = members.iter().all(|(name, member)| {
            match properties.and_then(|properties| properties.get(name)) {
                Some(schema) => admits(schema, member),
                None => additional.is_none_or(|schema| admits(schema, member)),
            }
        });
        if !members_hold {
            return false;
        }
    }
    if let (Json::Array(elements), Some(items)) = (value, keywords.get("items")) {
        return elements.iter().all(|element| admits(items, element));
    }
    true
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{admits, supported};

    #[test]
    fn the_schema_subset_admits_what_json_schema_admits_and_refuses_what_it_refuses() {
        let schema = json!({
            "type": "object",
            "description": "a city and how many days",
            "properties": {
                "city": {"type": "string"},
                "days": {"type": "integer"},
                "unit": {"enum": ["c", "f"]},
                "level": {"enum": [1, [2.0], {"n": 3}]},
                "tags": {"type": "array", "items": {"type": "string"}}
            },
            "required": ["city"],
            "additionalProperties": false
        });
        assert_eq!(supported(&schema), Ok(()));
        for admitted in [
            json!({"city": "Berlin"}),
            json!({"city": "Berlin", "days": 3, "unit": "c", "tags": ["a"]}),
            json!({"city": "Berlin", "days": 3.0}),
            json!({"city": "Berlin", "level": 1.0}),
            json!({"city": "Berlin", "level": [2]}),
            json!({"city": "Berlin", "level": {"n": 3.0}}),
        ] {
            assert!(admits(&schema, &admitted), "{admitted}");
        }
        for refused in [
            json!({}),
            json!({"city": 7}),
            json!({"city": "Berlin", "days": 1.5}),
            json!({"city": "Berlin", "unit": "k"}),
            json!({"city": "Berlin", "level": 1.5}),
            json!({"city": "Berlin", "level": "1"}),
            json!({"city": "Berlin", "level": [2, 2]}),
            json!({"city": "Berlin", "tags": [1]}),
            json!({"city": "Berlin", "country": "DE"}),
            json!("Berlin"),
        ] {
            assert!(!admits(&schema, &refused), "{refused}");
        }
        assert!(admits(&json!(true), &json!(1)));
        assert!(!admits(&json!(false), &json!(1)));
        assert!(admits(&json!({}), &json!({"anything": [1]})));
    }

    #[test]
    fn a_schema_keyword_outside_the_subset_is_named_rather_than_ignored() {
        assert_eq!(
            supported(&json!({"type": "string", "pattern": "^a"})),
            Err("the keyword \"pattern\"".to_owned())
        );
        assert_eq!(
            supported(&json!({"properties": {"n": {"minimum": 1}}})),
            Err("the keyword \"minimum\"".to_owned())
        );
        assert_eq!(
            supported(&json!({"items": {"oneOf": []}})),
            Err("the keyword \"oneOf\"".to_owned())
        );
        assert_eq!(supported(&json!(3)), Err("3 as a schema".to_owned()));
    }

    #[test]
    fn a_keyword_of_the_subset_in_another_form_than_json_schemas_is_named_rather_than_ignored() {
        for malformed in [
            json!({"required": "city"}),
            json!({"required": [1]}),
            json!({"properties": ["city"]}),
            json!({"properties": {"city": "string"}}),
            json!({"type": "text"}),
            json!({"type": ["string", 1]}),
            json!({"type": 3}),
            json!({"enum": "a"}),
            json!({"items": [{"type": "string"}]}),
            json!({"additionalProperties": 3}),
        ] {
            assert!(supported(&malformed).is_err(), "{malformed}");
        }
        for well_formed in [
            json!({"type": ["string", "null"], "required": [], "properties": {}}),
            json!({"items": true, "additionalProperties": {"type": "integer"}, "enum": []}),
        ] {
            assert_eq!(supported(&well_formed), Ok(()), "{well_formed}");
        }
    }
}
