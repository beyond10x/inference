//! `inference.model`: the three commands and three views of `ess/domains/model.yaml`, answered
//! by [`ModelDomain`].
//!
//! Each request field is read on its own ([`Input`]); the outcome, the error and the event come
//! from what the store returned. A struct or a list of structs, the audio formats, the evidence
//! and the resource profile of a capability record, is read through the `inference-core` type
//! that declares it, with an integral wire number read as the integer and every field it does
//! not declare refused; a value that type cannot hold, such as a sample rate of zero, is an
//! error. A command's consistency token is the target's
//! ([`Stores::token`]), and a read demanding a token the stores have not reached is an error
//! ([`Stores::require`]). An observed event's position is its place in the target's one
//! publication order ([`Stores::model_position`]).

use std::collections::BTreeMap;

use ess_conformance::scenario::{CommandRef, ErrorRef, EventRef, OutcomeRef};
use ess_conformance::target::{
    DeclaredErrorValue, EventObservationRequest, ObservedEvent, SemanticCommandRequest,
    SemanticCommandResult, SemanticViewRequest, SemanticViewResult, TargetError, ViewRow,
};
use ess_primitives::node::Node;
use inference_core::model::{
    AdapterDeclared, CapabilityRecordDeclared, DeclareAdapter, DeclareCapabilityRecord,
    DeclareModel, ModelDeclared,
};
use inference_testkit::model::{
    AdaptersRow, CapabilityRecordsRow, DeclareAdapterError, DeclareCapabilityRecordError,
    ModelDomain, ModelEvent, ModelsRow,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::{Input, Stores, integer_node, named_list_node, named_node, text_node, unsupported};

/// The prefix of every name this module answers.
pub const DOMAIN: &str = "inference.model";

const DECLARE_MODEL: &str = "inference.model.DeclareModel";
const DECLARE_ADAPTER: &str = "inference.model.DeclareAdapter";
const DECLARE_CAPABILITY_RECORD: &str = "inference.model.DeclareCapabilityRecord";
const MODEL_DECLARED: &str = "inference.model.ModelDeclared";
const ADAPTER_DECLARED: &str = "inference.model.AdapterDeclared";
const CAPABILITY_RECORD_DECLARED: &str = "inference.model.CapabilityRecordDeclared";
const NON_POSITIVE_CONCURRENCY: &str = "inference.model.NonPositiveConcurrency";
const MODEL_NOT_FOUND: &str = "inference.model.ModelNotFound";
const ADAPTER_NOT_FOUND: &str = "inference.model.AdapterNotFound";
const MODELS: &str = "inference.model.Models";
const ADAPTERS: &str = "inference.model.Adapters";
const CAPABILITY_RECORDS: &str = "inference.model.CapabilityRecords";

/// The `inference.model` store of one scenario.
pub type State = ModelDomain;

pub fn execute_command(
    stores: &mut Stores,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    stores.publish();
    let domain = &mut stores.model;
    let name = request.command.to_string();
    let result = match name.as_str() {
        DECLARE_MODEL => declare_model(domain, request)?,
        DECLARE_ADAPTER => declare_adapter(domain, request)?,
        DECLARE_CAPABILITY_RECORD => declare_capability_record(domain, request)?,
        _ => return Err(unsupported(&name)),
    };
    stores.publish();
    Ok(result.with_consistency(stores.token()?))
}

fn declare_model(
    domain: &mut ModelDomain,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let declare = DeclareModel {
        family: input.text("family")?,
        checkpoint: input.text("checkpoint")?,
        code_license: input.text("code_license")?,
        weight_license: input.text("weight_license")?,
        checksum: input.text("checksum")?,
        input_modalities: input.named_list("input_modalities")?,
        output_modalities: input.named_list("output_modalities")?,
        provenance: input.text("provenance")?,
        source_revision: input.text("source_revision")?,
    };
    input.finish()?;
    let event = domain.declare_model(declare);
    Ok(
        SemanticCommandResult::took(outcome(DECLARE_MODEL, "declared"))
            .emitting(model_declared(&event)?.in_activity(request.correlation.clone())),
    )
}

fn declare_adapter(
    domain: &mut ModelDomain,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let declare = DeclareAdapter {
        model_id: input.named("model_id")?,
        backend: input.text("backend")?,
        contract_revision: input.text("contract_revision")?,
        execution_form: input.named("execution_form")?,
        locality: input.named("locality")?,
        capabilities: input.named_list("capabilities")?,
        max_concurrency: input.integer("max_concurrency")?,
    };
    input.finish()?;
    Ok(match domain.declare_adapter(declare) {
        Ok(event) => SemanticCommandResult::took(outcome(DECLARE_ADAPTER, "declared"))
            .emitting(adapter_declared(&event)?.in_activity(request.correlation.clone())),
        Err(DeclareAdapterError::UnknownModel(error)) => {
            SemanticCommandResult::took(outcome(DECLARE_ADAPTER, "unknown-model")).with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(MODEL_NOT_FOUND))
                    .with("model_id", named_node(&error.model_id)?),
            )
        }
        Err(DeclareAdapterError::Refused(error)) => {
            SemanticCommandResult::took(outcome(DECLARE_ADAPTER, "refused")).with_error(
                DeclaredErrorValue::new(parse::<ErrorRef>(NON_POSITIVE_CONCURRENCY))
                    .with("submitted", integer_node(error.submitted)),
            )
        }
    })
}

fn declare_capability_record(
    domain: &mut ModelDomain,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let declare = DeclareCapabilityRecord {
        adapter_id: input.named("adapter_id")?,
        model_id: input.named("model_id")?,
        hardware: input.text("hardware")?,
        languages: typed(DECLARE_CAPABILITY_RECORD, "languages", &mut input)?,
        audio_formats: typed(DECLARE_CAPABILITY_RECORD, "audio_formats", &mut input)?,
        evidence: typed(DECLARE_CAPABILITY_RECORD, "evidence", &mut input)?,
        resource_profile: typed(DECLARE_CAPABILITY_RECORD, "resource_profile", &mut input)?,
        adapter_revision: input.text("adapter_revision")?,
    };
    input.finish()?;
    Ok(match domain.declare_capability_record(declare) {
        Ok(event) => SemanticCommandResult::took(outcome(DECLARE_CAPABILITY_RECORD, "declared"))
            .emitting(capability_record_declared(&event)?.in_activity(request.correlation.clone())),
        Err(DeclareCapabilityRecordError::UnknownAdapter(error)) => {
            SemanticCommandResult::took(outcome(DECLARE_CAPABILITY_RECORD, "unknown-adapter"))
                .with_error(
                    DeclaredErrorValue::new(parse::<ErrorRef>(ADAPTER_NOT_FOUND))
                        .with("adapter_id", named_node(&error.adapter_id)?),
                )
        }
        Err(DeclareCapabilityRecordError::UnknownModel(error)) => {
            SemanticCommandResult::took(outcome(DECLARE_CAPABILITY_RECORD, "unknown-model"))
                .with_error(
                    DeclaredErrorValue::new(parse::<ErrorRef>(MODEL_NOT_FOUND))
                        .with("model_id", named_node(&error.model_id)?),
                )
        }
    })
}

/// The field `field` of the input, read as the `inference-core` type `T` reads its JSON form:
/// an integral number as the integer, a field `T` does not declare refused.
fn typed<T: DeserializeOwned>(
    command: &str,
    field: &str,
    input: &mut Input<'_>,
) -> Result<T, TargetError> {
    let node = input.node(field)?;
    serde_json::from_value(wire_json(node)).map_err(|error| {
        TargetError::unavailable(
            format!("reading `{command}`"),
            format!("`{field}`: {error}: {node:?}"),
        )
    })
}

/// The JSON form of a wire node, with every number that is an ESS `Integer` written as one, as
/// `target::integer` reads it: `1.0` is `1`, while `1.5` and anything beyond `i64` stay numbers
/// no integer field reads. `target/model.rs` is also loaded by a test binary of its own, which
/// re-exports a fixed set of `target` helpers, so this module reads the number itself.
fn wire_json(node: &Node) -> Value {
    match node {
        Node::Null => Value::Null,
        Node::Bool(value) => Value::Bool(*value),
        Node::Number(number) => number.as_i64().map_or_else(
            || serde_json::to_value(node).unwrap_or(Value::Null),
            Value::from,
        ),
        Node::Text(text) => Value::String(text.clone()),
        Node::Seq(items) => Value::Array(items.iter().map(wire_json).collect()),
        Node::Map(fields) => Value::Object(
            fields
                .iter()
                .map(|(name, value)| (name.clone(), wire_json(value)))
                .collect(),
        ),
    }
}

/// The node of a value through its JSON form: a list of strings, formats or evidence, or a
/// resource profile.
fn json_node<T: Serialize>(value: &T) -> Result<Node, TargetError> {
    let value = serde_json::to_value(value)
        .map_err(|error| TargetError::unavailable("writing a value", error.to_string()))?;
    serde_json::from_value(value.clone())
        .map_err(|error| TargetError::unavailable("writing a value", format!("{error}: {value}")))
}

pub fn query_view(
    stores: &mut Stores,
    request: &SemanticViewRequest,
) -> Result<SemanticViewResult, TargetError> {
    let domain = &stores.model;
    let name = request.view.to_string();
    stores.require(&name, &request.consistency)?;
    let rows = match name.as_str() {
        MODELS => domain
            .models_view()
            .iter()
            .map(models_row)
            .collect::<Result<Vec<_>, _>>()?,
        ADAPTERS => domain
            .adapters_view()
            .iter()
            .map(adapters_row)
            .collect::<Result<Vec<_>, _>>()?,
        CAPABILITY_RECORDS => domain
            .capability_records_view()
            .iter()
            .map(capability_records_row)
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
    if ![MODEL_DECLARED, ADAPTER_DECLARED, CAPABILITY_RECORD_DECLARED].contains(&wanted.as_str()) {
        return Err(unsupported(&wanted));
    }
    stores.publish();
    let mut observed = Vec::new();
    for (index, event) in stores.model.events().iter().enumerate() {
        let event = match event {
            ModelEvent::ModelDeclared(event) => model_declared(event)?,
            ModelEvent::AdapterDeclared(event) => adapter_declared(event)?,
            ModelEvent::CapabilityRecordDeclared(event) => capability_record_declared(event)?,
        };
        if event.event == request.event {
            let position = stores.model_position(index)?;
            observed.push(event.in_activity(request.correlation.clone()).at(position));
        }
    }
    Ok(observed)
}

fn model_declared(event: &ModelDeclared) -> Result<ObservedEvent, TargetError> {
    Ok(ObservedEvent::new(parse::<EventRef>(MODEL_DECLARED))
        .with("model_id", named_node(&event.model_id)?)
        .with("family", text_node(&event.family))
        .with("checkpoint", text_node(&event.checkpoint)))
}

fn adapter_declared(event: &AdapterDeclared) -> Result<ObservedEvent, TargetError> {
    Ok(ObservedEvent::new(parse::<EventRef>(ADAPTER_DECLARED))
        .with("adapter_id", named_node(&event.adapter_id)?)
        .with("model_id", named_node(&event.model_id)?)
        .with("capabilities", named_list_node(&event.capabilities)?))
}

fn capability_record_declared(
    event: &CapabilityRecordDeclared,
) -> Result<ObservedEvent, TargetError> {
    Ok(
        ObservedEvent::new(parse::<EventRef>(CAPABILITY_RECORD_DECLARED))
            .with(
                "capability_record_id",
                named_node(&event.capability_record_id)?,
            )
            .with("adapter_id", named_node(&event.adapter_id)?)
            .with("model_id", named_node(&event.model_id)?)
            .with("hardware", text_node(&event.hardware)),
    )
}

fn capability_records_row(row: &CapabilityRecordsRow) -> Result<ViewRow, TargetError> {
    Ok(BTreeMap::from([
        (
            "capability_record_id".to_owned(),
            named_node(&row.capability_record_id)?,
        ),
        ("adapter_id".to_owned(), named_node(&row.adapter_id)?),
        ("model_id".to_owned(), named_node(&row.model_id)?),
        ("hardware".to_owned(), text_node(&row.hardware)),
        ("languages".to_owned(), json_node(&row.languages)?),
        ("audio_formats".to_owned(), json_node(&row.audio_formats)?),
        ("evidence".to_owned(), json_node(&row.evidence)?),
        (
            "resource_profile".to_owned(),
            json_node(&row.resource_profile)?,
        ),
        (
            "adapter_revision".to_owned(),
            text_node(&row.adapter_revision),
        ),
    ]))
}

fn models_row(row: &ModelsRow) -> Result<ViewRow, TargetError> {
    Ok(BTreeMap::from([
        ("model_id".to_owned(), named_node(&row.model_id)?),
        ("family".to_owned(), text_node(&row.family)),
        ("checkpoint".to_owned(), text_node(&row.checkpoint)),
        ("code_license".to_owned(), text_node(&row.code_license)),
        ("weight_license".to_owned(), text_node(&row.weight_license)),
        ("checksum".to_owned(), text_node(&row.checksum)),
        (
            "input_modalities".to_owned(),
            named_list_node(&row.input_modalities)?,
        ),
        (
            "output_modalities".to_owned(),
            named_list_node(&row.output_modalities)?,
        ),
        ("provenance".to_owned(), text_node(&row.provenance)),
        (
            "source_revision".to_owned(),
            text_node(&row.source_revision),
        ),
    ]))
}

fn adapters_row(row: &AdaptersRow) -> Result<ViewRow, TargetError> {
    Ok(BTreeMap::from([
        ("adapter_id".to_owned(), named_node(&row.adapter_id)?),
        ("model_id".to_owned(), named_node(&row.model_id)?),
        ("backend".to_owned(), text_node(&row.backend)),
        (
            "contract_revision".to_owned(),
            text_node(&row.contract_revision),
        ),
        (
            "execution_form".to_owned(),
            named_node(&row.execution_form)?,
        ),
        ("locality".to_owned(), named_node(&row.locality)?),
        (
            "capabilities".to_owned(),
            named_list_node(&row.capabilities)?,
        ),
        (
            "max_concurrency".to_owned(),
            integer_node(row.max_concurrency),
        ),
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
