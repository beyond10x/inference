//! `inference.model`: the two commands and two views of `ess/domains/model.yaml`, answered by
//! [`ModelDomain`].
//!
//! Each request field is read on its own ([`Input`]); the outcome, the error and the event come
//! from what the store returned. A command's consistency token is the target's
//! ([`Stores::token`]), and a read demanding a token the stores have not reached is an error
//! ([`Stores::require`]).

use std::collections::BTreeMap;

use ess_conformance::scenario::{CommandRef, ErrorRef, EventRef, OutcomeRef};
use ess_conformance::target::{
    DeclaredErrorValue, EventObservationRequest, ObservedEvent, SemanticCommandRequest,
    SemanticCommandResult, SemanticViewRequest, SemanticViewResult, TargetError, ViewRow,
};
use inference_core::model::{AdapterDeclared, DeclareAdapter, DeclareModel, ModelDeclared};
use inference_testkit::model::{
    AdaptersRow, DeclareAdapterError, ModelDomain, ModelEvent, ModelsRow,
};

use super::{Input, Stores, integer_node, named_list_node, named_node, text_node, unsupported};

/// The prefix of every name this module answers.
pub const DOMAIN: &str = "inference.model";

const DECLARE_MODEL: &str = "inference.model.DeclareModel";
const DECLARE_ADAPTER: &str = "inference.model.DeclareAdapter";
const MODEL_DECLARED: &str = "inference.model.ModelDeclared";
const ADAPTER_DECLARED: &str = "inference.model.AdapterDeclared";
const NON_POSITIVE_CONCURRENCY: &str = "inference.model.NonPositiveConcurrency";
const MODEL_NOT_FOUND: &str = "inference.model.ModelNotFound";
const MODELS: &str = "inference.model.Models";
const ADAPTERS: &str = "inference.model.Adapters";

/// The `inference.model` store of one scenario.
pub type State = ModelDomain;

pub fn execute_command(
    stores: &mut Stores,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let domain = &mut stores.model;
    let name = request.command.to_string();
    let result = match name.as_str() {
        DECLARE_MODEL => declare_model(domain, request)?,
        DECLARE_ADAPTER => declare_adapter(domain, request)?,
        _ => return Err(unsupported(&name)),
    };
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
        _ => return Err(unsupported(&name)),
    };
    Ok(SemanticViewResult::of(rows))
}

pub fn observe_events(
    stores: &mut Stores,
    request: &EventObservationRequest,
) -> Result<Vec<ObservedEvent>, TargetError> {
    let wanted = request.event.to_string();
    if wanted != MODEL_DECLARED && wanted != ADAPTER_DECLARED {
        return Err(unsupported(&wanted));
    }
    let mut observed = Vec::new();
    for (position, event) in stores.model.events().iter().enumerate() {
        let event = match event {
            ModelEvent::ModelDeclared(event) => model_declared(event)?,
            ModelEvent::AdapterDeclared(event) => adapter_declared(event)?,
        };
        if event.event == request.event {
            let sequence = u64::try_from(position + 1).unwrap_or(u64::MAX);
            observed.push(event.in_activity(request.correlation.clone()).at(sequence));
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
