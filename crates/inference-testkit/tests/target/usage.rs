//! `inference.usage`: the one command of `ess/domains/usage.yaml`, answered by [`UsageDomain`]
//! over the `inference.model` store, which decides whether the adapter and the model are
//! declared.
//!
//! Each request field is read on its own ([`Input`]); an `Optional` field left out or sent as
//! null is none. A quantity is read field by field, `kind`, `source` and `value`, every other
//! field refused, and built by [`Quantity::new`]: a value and a source that disagree are an error,
//! never a pass, since the specification declares no outcome for them. The outcome, the error
//! and the event come from what the store returned. A command's consistency token is the
//! target's ([`Stores::token`]), and an observed event's position is its place in the target's
//! one publication order ([`Stores::usage_position`]).

use std::collections::BTreeMap;

use ess_conformance::scenario::{CommandRef, ErrorRef, EventRef, OutcomeRef};
use ess_conformance::target::{
    DeclaredErrorValue, EventObservationRequest, ObservedEvent, SemanticCommandRequest,
    SemanticCommandResult, SemanticViewRequest, SemanticViewResult, TargetError,
};
use ess_primitives::node::Node;
use inference_core::usage::{Quantity, RecordUsage, UsageRecorded};
use inference_testkit::usage::{RecordUsageError, UsageDomain, UsageEvent};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{
    Input, Stores, boolean_node, integer, integer_node, named_node, text_node, unreadable,
    unsupported,
};

/// The prefix of every name this module answers.
pub const DOMAIN: &str = "inference.usage";

const RECORD_USAGE: &str = "inference.usage.RecordUsage";
const USAGE_RECORDED: &str = "inference.usage.UsageRecorded";
const ADAPTER_NOT_FOUND: &str = "inference.model.AdapterNotFound";
const MODEL_NOT_FOUND: &str = "inference.model.ModelNotFound";

/// The `inference.usage` store of one scenario.
pub type State = UsageDomain;

pub fn execute_command(
    stores: &mut Stores,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    stores.publish();
    let name = request.command.to_string();
    let result = match name.as_str() {
        RECORD_USAGE => record_usage(stores, request)?,
        _ => return Err(unsupported(&name)),
    };
    stores.publish();
    Ok(result.with_consistency(stores.token()?))
}

fn record_usage(
    stores: &mut Stores,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let record = RecordUsage {
        run_id: optional_named(&mut input, "run_id")?,
        node_id: optional_named(&mut input, "node_id")?,
        session_id: optional_named(&mut input, "session_id")?,
        speech_session_id: optional_named(&mut input, "speech_session_id")?,
        response_id: optional_named(&mut input, "response_id")?,
        adapter_id: input.named("adapter_id")?,
        model_id: input.named("model_id")?,
        upstream_model: optional_text(&mut input, "upstream_model")?,
        started_at_ms: input.integer("started_at_ms")?,
        ended_at_ms: input.integer("ended_at_ms")?,
        r#final: input.boolean("final")?,
        quantities: quantities(input.node("quantities")?)?,
    };
    input.finish()?;
    Ok(match stores.usage.record_usage(&stores.model, record) {
        Ok(event) => SemanticCommandResult::took(outcome(RECORD_USAGE, "recorded"))
            .emitting(usage_recorded(&event)?.in_activity(request.correlation.clone())),
        Err(RecordUsageError::UnknownAdapter(error)) => {
            SemanticCommandResult::took(outcome(RECORD_USAGE, "unknown-adapter")).with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(ADAPTER_NOT_FOUND))
                    .with("adapter_id", named_node(&error.adapter_id)?),
            )
        }
        Err(RecordUsageError::UnknownModel(error)) => {
            SemanticCommandResult::took(outcome(RECORD_USAGE, "unknown-model")).with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(MODEL_NOT_FOUND))
                    .with("model_id", named_node(&error.model_id)?),
            )
        }
    })
}

/// An `Optional` identifier: none when left out or null, otherwise read as [`Input::named`] reads
/// one.
fn optional_named<T: DeserializeOwned>(
    input: &mut Input<'_>,
    field: &str,
) -> Result<Option<T>, TargetError> {
    input
        .optional(field)
        .map(|node| match node {
            Node::Text(text) => serde_json::from_value(serde_json::Value::String(text.clone()))
                .map_err(|error| refused(field, &format!("{error}: {node:?}"))),
            other => Err(refused(field, &format!("not an identifier: {other:?}"))),
        })
        .transpose()
}

/// An `Optional<String>`: none when left out or null.
fn optional_text(input: &mut Input<'_>, field: &str) -> Result<Option<String>, TargetError> {
    input
        .optional(field)
        .map(|node| match node {
            Node::Text(text) => Ok(text.clone()),
            other => Err(refused(field, &format!("not a string: {other:?}"))),
        })
        .transpose()
}

/// A `List<inference.usage.Quantity>`, each item read field by field and built by
/// [`Quantity::new`].
fn quantities(node: &Node) -> Result<Vec<Quantity>, TargetError> {
    let Node::Seq(items) = node else {
        return Err(refused("quantities", &format!("not a list: {node:?}")));
    };
    items.iter().map(quantity).collect()
}

fn quantity(node: &Node) -> Result<Quantity, TargetError> {
    let Node::Map(fields) = node else {
        return Err(refused("quantities", &format!("not a struct: {node:?}")));
    };
    if let Some(extra) = fields
        .keys()
        .find(|name| !["kind", "source", "value"].contains(&name.as_str()))
    {
        return Err(refused(
            "quantities",
            &format!("an item carries the undeclared field `{extra}`"),
        ));
    }
    let value = match fields.get("value") {
        None | Some(Node::Null) => None,
        Some(node) => Some(integer(node).ok_or_else(|| {
            refused(
                "quantities",
                &format!("`value` is not an integer within i64: {node:?}"),
            )
        })?),
    };
    Quantity::new(variant(fields, "kind")?, variant(fields, "source")?, value)
        .map_err(|error| refused("quantities", &error.to_string()))
}

/// The enum variant a quantity carries in its field `name`.
fn variant<T: DeserializeOwned>(
    fields: &BTreeMap<String, Node>,
    name: &str,
) -> Result<T, TargetError> {
    match fields.get(name) {
        Some(Node::Text(text)) => serde_json::from_value(serde_json::Value::String(text.clone()))
            .map_err(|error| refused("quantities", &format!("`{name}`: {error}"))),
        other => Err(refused(
            "quantities",
            &format!("`{name}` is not a declared value: {other:?}"),
        )),
    }
}

fn refused(field: &str, detail: &str) -> TargetError {
    unreadable(
        format!("reading `{RECORD_USAGE}`"),
        format!("`{field}`: {detail}"),
    )
}

pub fn query_view(
    stores: &mut Stores,
    request: &SemanticViewRequest,
) -> Result<SemanticViewResult, TargetError> {
    let name = request.view.to_string();
    stores.require(&name, &request.consistency)?;
    Err(unsupported(&name))
}

pub fn observe_events(
    stores: &mut Stores,
    request: &EventObservationRequest,
) -> Result<Vec<ObservedEvent>, TargetError> {
    let wanted = request.event.to_string();
    if wanted != USAGE_RECORDED {
        return Err(unsupported(&wanted));
    }
    stores.publish();
    let mut observed = Vec::new();
    for (index, event) in stores.usage.events().iter().enumerate() {
        let UsageEvent::UsageRecorded(event) = event;
        let position = stores.usage_position(index)?;
        observed.push(
            usage_recorded(event)?
                .in_activity(request.correlation.clone())
                .at(position),
        );
    }
    Ok(observed)
}

fn usage_recorded(event: &UsageRecorded) -> Result<ObservedEvent, TargetError> {
    Ok(ObservedEvent::new(parse::<EventRef>(USAGE_RECORDED))
        .with("usage_record_id", named_node(&event.usage_record_id)?)
        .with("run_id", optional_node(event.run_id.as_ref())?)
        .with("node_id", optional_node(event.node_id.as_ref())?)
        .with("session_id", optional_node(event.session_id.as_ref())?)
        .with(
            "speech_session_id",
            optional_node(event.speech_session_id.as_ref())?,
        )
        .with("response_id", optional_node(event.response_id.as_ref())?)
        .with("adapter_id", named_node(&event.adapter_id)?)
        .with("model_id", named_node(&event.model_id)?)
        .with(
            "upstream_model",
            event
                .upstream_model
                .as_deref()
                .map_or(Node::Null, text_node),
        )
        .with("started_at_ms", integer_node(event.started_at_ms))
        .with("ended_at_ms", integer_node(event.ended_at_ms))
        .with("final", boolean_node(event.r#final))
        .with("quantities", quantities_node(&event.quantities)?))
}

fn optional_node<T: Serialize>(value: Option<&T>) -> Result<Node, TargetError> {
    value.map_or(Ok(Node::Null), named_node)
}

fn quantities_node(quantities: &[Quantity]) -> Result<Node, TargetError> {
    quantities
        .iter()
        .map(|quantity| {
            Ok(Node::Map(BTreeMap::from([
                ("kind".to_owned(), named_node(&quantity.kind())?),
                ("source".to_owned(), named_node(&quantity.source())?),
                (
                    "value".to_owned(),
                    quantity.value().map_or(Node::Null, integer_node),
                ),
            ])))
        })
        .collect::<Result<Vec<_>, TargetError>>()
        .map(Node::Seq)
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

    use ess_conformance::scenario::CommandRef;
    use ess_conformance::target::SemanticCommandRequest;
    use ess_primitives::facts::Number;
    use ess_primitives::ids::CorrelationId;
    use ess_primitives::node::Node;

    use super::{RECORD_USAGE, Stores, execute_command};

    fn request(quantity: BTreeMap<String, Node>) -> SemanticCommandRequest {
        SemanticCommandRequest {
            command: RECORD_USAGE.parse::<CommandRef>().expect("a command name"),
            actor: None,
            caller: None,
            input: [
                (
                    "adapter_id",
                    Node::Text("00000001-0000-4000-8000-000000000002".to_owned()),
                ),
                (
                    "model_id",
                    Node::Text("00000001-0000-4000-8000-000000000001".to_owned()),
                ),
                ("started_at_ms", Node::Number(Number::from(1_i64))),
                ("ended_at_ms", Node::Number(Number::from(2_i64))),
                ("final", Node::Bool(true)),
                ("quantities", Node::Seq(vec![Node::Map(quantity)])),
            ]
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect(),
            correlation: CorrelationId::new("usage").expect("a correlation id"),
        }
    }

    fn quantity(fields: &[(&str, Node)]) -> BTreeMap<String, Node> {
        fields
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone()))
            .collect()
    }

    #[test]
    fn a_quantity_whose_value_and_source_disagree_is_an_error_not_an_outcome() {
        let mut stores = Stores::default();
        let answer = execute_command(
            &mut stores,
            &request(quantity(&[
                ("kind", Node::Text("requests".to_owned())),
                ("source", Node::Text("unknown".to_owned())),
                ("value", Node::Number(Number::from(1_i64))),
            ])),
        );
        assert!(answer.is_err(), "{answer:?}");
        assert!(stores.usage.events().is_empty());
    }

    #[test]
    fn a_quantity_with_an_undeclared_field_is_an_error() {
        let mut stores = Stores::default();
        let answer = execute_command(
            &mut stores,
            &request(quantity(&[
                ("kind", Node::Text("requests".to_owned())),
                ("source", Node::Text("unknown".to_owned())),
                ("unit", Node::Text("x".to_owned())),
            ])),
        );
        assert!(answer.is_err(), "{answer:?}");
    }

    #[test]
    fn an_unknown_quantity_without_a_value_is_read_and_its_adapter_decided() {
        let mut stores = Stores::default();
        let answer = execute_command(
            &mut stores,
            &request(quantity(&[
                ("kind", Node::Text("requests".to_owned())),
                ("source", Node::Text("unknown".to_owned())),
            ])),
        )
        .expect("the request is read");
        assert_eq!(
            answer.outcome.map(|outcome| outcome.to_string()),
            Some("inference.usage.RecordUsage/unknown-adapter".to_owned())
        );
    }
}
