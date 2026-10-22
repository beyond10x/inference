//! `inference.usage`: the two commands of `ess/domains/usage.yaml`, answered by [`UsageDomain`]:
//! `RecordUsage` over the `inference.model` store, which decides whether the adapter and the
//! model are declared, and `PriceUsage` over the domain's own usage records, which decide whether
//! the usage record priced exists. A cost line and a cost total are read field by field, as a
//! quantity is, and an `Optional` field left out is none, as `ess` 0.66 leaves them out of the
//! minimal input of a refusal. The view `inference.usage.UsageRecords` is
//! [`UsageDomain::usage_records_view`], each row carrying the fields its record's
//! `UsageRecorded` carries, its quantities written as the event writes them.
//!
//! Each request field is read on its own ([`Input`]); an `Optional` field left out or sent as
//! null is none. A quantity is read field by field, `kind`, `source` and `value`, every other
//! field refused, and built by [`Quantity::new`]: a value and a source that disagree are an error,
//! never a pass, since the specification declares no outcome for them; an unknown adapter or model
//! is answered first, as the store refuses on either before it reads a quantity. The outcome, the error
//! and the event come from what the store returned. A command's consistency token is the
//! target's ([`Stores::token`]), and an observed event's position is its place in the target's
//! one publication order ([`Stores::usage_position`]).

use std::collections::BTreeMap;

use ess_conformance::scenario::{CommandRef, ErrorRef, EventRef, OutcomeRef};
use ess_conformance::target::{
    DeclaredErrorValue, EventObservationRequest, ObservedEvent, SemanticCommandRequest,
    SemanticCommandResult, SemanticViewRequest, SemanticViewResult, TargetError, ViewRow,
};
use ess_primitives::node::Node;
use inference_core::usage::{
    CostLine, CostRecorded, CostTotal, PriceUsage, Quantity, RecordUsage, UsageRecorded,
};
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
const PRICE_USAGE: &str = "inference.usage.PriceUsage";
const USAGE_RECORDED: &str = "inference.usage.UsageRecorded";
const USAGE_RECORDS: &str = "inference.usage.UsageRecords";
const COST_RECORDED: &str = "inference.usage.CostRecorded";
const ADAPTER_NOT_FOUND: &str = "inference.model.AdapterNotFound";
const MODEL_NOT_FOUND: &str = "inference.model.ModelNotFound";
const USAGE_RECORD_NOT_FOUND: &str = "inference.usage.UsageRecordNotFound";

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
        PRICE_USAGE => price_usage(stores, request)?,
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
    let mut record = RecordUsage {
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
        quantities: Vec::new(),
    };
    let built = quantities(input.node("quantities")?)?;
    input.finish()?;
    // The declared outcomes turn on the adapter and the model alone, so an unknown one is answered
    // before a quantity whose value and source disagree is refused: the store refuses the command
    // on either before it reads a quantity.
    let declared = stores.model.adapters().get(record.adapter_id).is_some()
        && stores.model.models().get(record.model_id).is_some();
    match built.into_iter().collect::<Result<Vec<_>, _>>() {
        Ok(built) => record.quantities = built,
        Err(error) if declared => return Err(error),
        Err(_) => {}
    }
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

/// `PriceUsage`: each field read on its own, a cost line and the total field by field, every
/// undeclared field refused and an `Optional` one left out or null read as none. The lines and
/// totals are recorded as sent: they are the pricing crate's, and the specification declares no
/// outcome on them.
fn price_usage(
    stores: &mut Stores,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let price = PriceUsage {
        usage_record_id: input.named("usage_record_id")?,
        currency: input.text("currency")?,
        price_revision: input.text("price_revision")?,
        price_digest: input.text("price_digest")?,
        lines: cost_lines(input.node("lines")?)?,
        total: cost_total(input.node("total")?)?,
    };
    input.finish()?;
    Ok(match stores.usage.price_usage(price) {
        Ok(event) => SemanticCommandResult::took(outcome(PRICE_USAGE, "priced"))
            .emitting(cost_recorded(&event)?.in_activity(request.correlation.clone())),
        Err(error) => SemanticCommandResult::took(outcome(PRICE_USAGE, "unknown-usage"))
            .with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(USAGE_RECORD_NOT_FOUND))
                    .with("usage_record_id", named_node(&error.usage_record_id)?),
            ),
    })
}

/// A `List<inference.usage.CostLine>`.
fn cost_lines(node: &Node) -> Result<Vec<CostLine>, TargetError> {
    let Node::Seq(items) = node else {
        return Err(cost_refused("lines", &format!("not a list: {node:?}")));
    };
    items.iter().map(cost_line).collect()
}

fn cost_line(node: &Node) -> Result<CostLine, TargetError> {
    let fields = declared_fields(node, "lines", &["kind", "quantity", "amount", "unknown"])?;
    let quantity = match present(fields, "quantity") {
        None => None,
        Some(node) => Some(integer(node).ok_or_else(|| {
            cost_refused(
                "lines",
                &format!("`quantity` is not an integer within i64: {node:?}"),
            )
        })?),
    };
    Ok(CostLine {
        kind: cost_variant(fields, "lines", "kind")?,
        quantity,
        amount: optional_string(fields, "lines", "amount")?,
        unknown: present(fields, "unknown")
            .map(|_| cost_variant(fields, "lines", "unknown"))
            .transpose()?,
    })
}

/// An `inference.usage.CostTotal`.
fn cost_total(node: &Node) -> Result<CostTotal, TargetError> {
    let fields = declared_fields(
        node,
        "total",
        &["known_subtotal", "unknown_lines", "complete_total"],
    )?;
    let unknown_lines = present(fields, "unknown_lines")
        .and_then(integer)
        .and_then(|value| u64::try_from(value).ok())
        .ok_or_else(|| {
            cost_refused(
                "total",
                &format!(
                    "`unknown_lines` is not a count: {:?}",
                    fields.get("unknown_lines")
                ),
            )
        })?;
    Ok(CostTotal {
        known_subtotal: optional_string(fields, "total", "known_subtotal")?
            .ok_or_else(|| cost_refused("total", "`known_subtotal` is missing"))?,
        unknown_lines,
        complete_total: optional_string(fields, "total", "complete_total")?,
    })
}

/// The fields of the struct `node`, refusing a field outside `declared`.
fn declared_fields<'a>(
    node: &'a Node,
    field: &str,
    declared: &[&str],
) -> Result<&'a BTreeMap<String, Node>, TargetError> {
    let Node::Map(fields) = node else {
        return Err(cost_refused(field, &format!("not a struct: {node:?}")));
    };
    if let Some(extra) = fields
        .keys()
        .find(|name| !declared.contains(&name.as_str()))
    {
        return Err(cost_refused(
            field,
            &format!("carries the undeclared field `{extra}`"),
        ));
    }
    Ok(fields)
}

/// The field `name` of a struct, none when left out or null.
fn present<'a>(fields: &'a BTreeMap<String, Node>, name: &str) -> Option<&'a Node> {
    fields.get(name).filter(|node| !matches!(node, Node::Null))
}

/// A `String` or `Optional<String>` field of a struct: none when left out or null.
fn optional_string(
    fields: &BTreeMap<String, Node>,
    field: &str,
    name: &str,
) -> Result<Option<String>, TargetError> {
    match present(fields, name) {
        None => Ok(None),
        Some(Node::Text(text)) => Ok(Some(text.clone())),
        Some(other) => Err(cost_refused(
            field,
            &format!("`{name}` is not a string: {other:?}"),
        )),
    }
}

/// The enum variant a struct carries in its field `name`.
fn cost_variant<T: DeserializeOwned>(
    fields: &BTreeMap<String, Node>,
    field: &str,
    name: &str,
) -> Result<T, TargetError> {
    match present(fields, name) {
        Some(Node::Text(text)) => serde_json::from_value(serde_json::Value::String(text.clone()))
            .map_err(|error| cost_refused(field, &format!("`{name}`: {error}"))),
        other => Err(cost_refused(
            field,
            &format!("`{name}` is not a declared value: {other:?}"),
        )),
    }
}

fn cost_refused(field: &str, detail: &str) -> TargetError {
    unreadable(
        format!("reading `{PRICE_USAGE}`"),
        format!("`{field}`: {detail}"),
    )
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
/// [`Quantity::new`]. An item the specification's shape does not allow is an error at once; the
/// inner result is what [`Quantity::new`] made of an item of the declared shape.
fn quantities(node: &Node) -> Result<Vec<Result<Quantity, TargetError>>, TargetError> {
    let Node::Seq(items) = node else {
        return Err(refused("quantities", &format!("not a list: {node:?}")));
    };
    items.iter().map(quantity).collect()
}

fn quantity(node: &Node) -> Result<Result<Quantity, TargetError>, TargetError> {
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
    Ok(
        Quantity::new(variant(fields, "kind")?, variant(fields, "source")?, value)
            .map_err(|error| refused("quantities", &error.to_string())),
    )
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
    if name != USAGE_RECORDS {
        return Err(unsupported(&name));
    }
    stores.require(&name, &request.consistency)?;
    let rows = stores
        .usage
        .usage_records_view()
        .iter()
        .map(usage_record_row)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(SemanticViewResult::of(rows))
}

pub fn observe_events(
    stores: &mut Stores,
    request: &EventObservationRequest,
) -> Result<Vec<ObservedEvent>, TargetError> {
    let wanted = request.event.to_string();
    if wanted != USAGE_RECORDED && wanted != COST_RECORDED {
        return Err(unsupported(&wanted));
    }
    stores.publish();
    let mut observed = Vec::new();
    for (index, event) in stores.usage.events().iter().enumerate() {
        let event = match event {
            UsageEvent::UsageRecorded(event) if wanted == USAGE_RECORDED => usage_recorded(event)?,
            UsageEvent::CostRecorded(event) if wanted == COST_RECORDED => cost_recorded(event)?,
            _ => continue,
        };
        let position = stores.usage_position(index)?;
        observed.push(event.in_activity(request.correlation.clone()).at(position));
    }
    Ok(observed)
}

fn cost_recorded(event: &CostRecorded) -> Result<ObservedEvent, TargetError> {
    let lines = event
        .lines
        .iter()
        .map(|line| {
            Ok(Node::Map(BTreeMap::from([
                ("kind".to_owned(), named_node(&line.kind)?),
                (
                    "quantity".to_owned(),
                    line.quantity.map_or(Node::Null, integer_node),
                ),
                (
                    "amount".to_owned(),
                    line.amount.as_deref().map_or(Node::Null, text_node),
                ),
                ("unknown".to_owned(), optional_node(line.unknown.as_ref())?),
            ])))
        })
        .collect::<Result<Vec<_>, TargetError>>()?;
    let unknown_lines = i64::try_from(event.total.unknown_lines).map_err(|_| {
        unreadable(
            format!("writing `{COST_RECORDED}`"),
            "`unknown_lines` beyond i64",
        )
    })?;
    let total = Node::Map(BTreeMap::from([
        (
            "known_subtotal".to_owned(),
            text_node(&event.total.known_subtotal),
        ),
        ("unknown_lines".to_owned(), integer_node(unknown_lines)),
        (
            "complete_total".to_owned(),
            event
                .total
                .complete_total
                .as_deref()
                .map_or(Node::Null, text_node),
        ),
    ]));
    Ok(ObservedEvent::new(parse::<EventRef>(COST_RECORDED))
        .with("cost_record_id", named_node(&event.cost_record_id)?)
        .with("usage_record_id", named_node(&event.usage_record_id)?)
        .with("currency", text_node(&event.currency))
        .with("price_revision", text_node(&event.price_revision))
        .with("price_digest", text_node(&event.price_digest))
        .with("lines", Node::Seq(lines))
        .with("total", total))
}

fn usage_recorded(event: &UsageRecorded) -> Result<ObservedEvent, TargetError> {
    Ok(usage_record_row(event)?.into_iter().fold(
        ObservedEvent::new(parse::<EventRef>(USAGE_RECORDED)),
        |observed, (field, value)| observed.with(field, value),
    ))
}

/// Every field of a usage record, which are the fields of its event and of its row in the view
/// `inference.usage.UsageRecords`.
fn usage_record_row(event: &UsageRecorded) -> Result<ViewRow, TargetError> {
    Ok(BTreeMap::from([
        (
            "usage_record_id".to_owned(),
            named_node(&event.usage_record_id)?,
        ),
        ("run_id".to_owned(), optional_node(event.run_id.as_ref())?),
        ("node_id".to_owned(), optional_node(event.node_id.as_ref())?),
        (
            "session_id".to_owned(),
            optional_node(event.session_id.as_ref())?,
        ),
        (
            "speech_session_id".to_owned(),
            optional_node(event.speech_session_id.as_ref())?,
        ),
        (
            "response_id".to_owned(),
            optional_node(event.response_id.as_ref())?,
        ),
        ("adapter_id".to_owned(), named_node(&event.adapter_id)?),
        ("model_id".to_owned(), named_node(&event.model_id)?),
        (
            "upstream_model".to_owned(),
            event
                .upstream_model
                .as_deref()
                .map_or(Node::Null, text_node),
        ),
        (
            "started_at_ms".to_owned(),
            integer_node(event.started_at_ms),
        ),
        ("ended_at_ms".to_owned(), integer_node(event.ended_at_ms)),
        ("final".to_owned(), boolean_node(event.r#final)),
        ("quantities".to_owned(), quantities_node(&event.quantities)?),
    ]))
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
    use inference_core::model::{DeclareAdapter, DeclareModel, ExecutionForm, Locality, Modality};
    use inference_core::usage::UnknownReason;

    use super::{PRICE_USAGE, RECORD_USAGE, Stores, execute_command};

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

    /// Stores holding the model and the adapter [`request`] names, the first of each the model
    /// store issues.
    fn declared() -> Stores {
        let mut stores = Stores::default();
        let model = stores.model.declare_model(DeclareModel {
            family: "family".to_owned(),
            checkpoint: "checkpoint".to_owned(),
            code_license: "MIT".to_owned(),
            weight_license: "MIT".to_owned(),
            checksum: "checksum".to_owned(),
            input_modalities: vec![Modality::Text],
            output_modalities: vec![Modality::Text],
            provenance: "provenance".to_owned(),
            source_revision: "revision".to_owned(),
        });
        stores
            .model
            .declare_adapter(DeclareAdapter {
                model_id: model.model_id,
                backend: "backend".to_owned(),
                contract_revision: "1".to_owned(),
                execution_form: ExecutionForm::InProcess,
                locality: Locality::SameProcess,
                capabilities: Vec::new(),
                max_concurrency: 1,
            })
            .expect("the model is declared");
        stores
    }

    #[test]
    fn a_quantity_whose_value_and_source_disagree_is_an_error_not_an_outcome() {
        let mut stores = declared();
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
    fn an_unknown_adapter_is_answered_before_a_quantity_whose_value_and_source_disagree() {
        let mut stores = Stores::default();
        let answer = execute_command(
            &mut stores,
            &request(quantity(&[
                ("kind", Node::Text("input_text_tokens".to_owned())),
                ("source", Node::Text("provider_reported".to_owned())),
            ])),
        )
        .expect("the request is read");
        assert_eq!(
            answer.outcome.map(|outcome| outcome.to_string()),
            Some("inference.usage.RecordUsage/unknown-adapter".to_owned())
        );
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

    /// A `PriceUsage` of `usage_record_id` whose line carries `line` and whose total carries
    /// `total`.
    fn price_request(
        usage_record_id: &str,
        line: &[(&str, Node)],
        total: &[(&str, Node)],
    ) -> SemanticCommandRequest {
        SemanticCommandRequest {
            command: PRICE_USAGE.parse::<CommandRef>().expect("a command name"),
            actor: None,
            caller: None,
            input: [
                ("usage_record_id", Node::Text(usage_record_id.to_owned())),
                ("currency", Node::Text("EUR".to_owned())),
                ("price_revision", Node::Text("1".to_owned())),
                ("price_digest", Node::Text("prices-sha256:00".to_owned())),
                ("lines", Node::Seq(vec![Node::Map(quantity(line))])),
                ("total", Node::Map(quantity(total))),
            ]
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect(),
            correlation: CorrelationId::new("usage").expect("a correlation id"),
        }
    }

    fn outcome_of(answer: &ess_conformance::target::SemanticCommandResult) -> Option<String> {
        answer.outcome.as_ref().map(ToString::to_string)
    }

    #[test]
    fn a_minimal_price_input_without_its_optional_fields_is_read_and_its_usage_decided() {
        let mut stores = Stores::default();
        let answer = execute_command(
            &mut stores,
            &price_request(
                "00000000-0000-4000-8000-c33919ac60ea",
                &[("kind", Node::Text("input_text_tokens".to_owned()))],
                &[
                    ("known_subtotal", Node::Text("0".to_owned())),
                    ("unknown_lines", Node::Number(Number::from(1_i64))),
                ],
            ),
        )
        .expect("the request is read");
        assert_eq!(
            outcome_of(&answer),
            Some("inference.usage.PriceUsage/unknown-usage".to_owned())
        );
        assert!(stores.usage.events().is_empty());
    }

    #[test]
    fn a_cost_of_a_recorded_usage_is_priced_with_its_lines_as_sent() {
        let mut stores = declared();
        execute_command(
            &mut stores,
            &request(quantity(&[
                ("kind", Node::Text("requests".to_owned())),
                ("source", Node::Text("measured".to_owned())),
                ("value", Node::Number(Number::from(1_i64))),
            ])),
        )
        .expect("recorded");
        let answer = execute_command(
            &mut stores,
            &price_request(
                "00000007-0000-4000-8000-000000000001",
                &[
                    ("kind", Node::Text("requests".to_owned())),
                    ("quantity", Node::Number(Number::from(1_i64))),
                    ("amount", Node::Null),
                    ("unknown", Node::Text("price_unknown".to_owned())),
                ],
                &[
                    ("known_subtotal", Node::Text("0".to_owned())),
                    ("unknown_lines", Node::Number(Number::from(1_i64))),
                    ("complete_total", Node::Null),
                ],
            ),
        )
        .expect("the request is read");
        assert_eq!(
            outcome_of(&answer),
            Some("inference.usage.PriceUsage/priced".to_owned())
        );
        let cost = stores.usage.costs().iter().next().expect("stored");
        assert_eq!(cost.lines()[0].unknown, Some(UnknownReason::PriceUnknown));
        assert_eq!(cost.lines()[0].amount, None);
        assert_eq!(cost.total().complete_total, None);
    }

    #[test]
    fn a_cost_line_with_an_undeclared_field_or_a_float_amount_is_an_error() {
        for line in [
            vec![
                ("kind", Node::Text("requests".to_owned())),
                ("unit", Node::Text("request".to_owned())),
            ],
            vec![
                ("kind", Node::Text("requests".to_owned())),
                ("amount", Node::Number(Number::from(1_i64))),
            ],
        ] {
            let mut stores = Stores::default();
            let answer = execute_command(
                &mut stores,
                &price_request(
                    "00000000-0000-4000-8000-c33919ac60ea",
                    &line,
                    &[
                        ("known_subtotal", Node::Text("0".to_owned())),
                        ("unknown_lines", Node::Number(Number::from(0_i64))),
                    ],
                ),
            );
            assert!(answer.is_err(), "{line:?}: {answer:?}");
        }
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
