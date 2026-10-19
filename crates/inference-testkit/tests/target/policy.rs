//! `inference.policy`: answers nothing yet. Every command, view and event of the domain is
//! [`TargetError::Unsupported`] until the deployment-profile stores land; `ess/SKIPPED.md` names the story or epic that
//! fills this module.

use ess_conformance::target::{
    EventObservationRequest, ObservedEvent, SemanticCommandRequest, SemanticCommandResult,
    SemanticViewRequest, SemanticViewResult, TargetError,
};

use super::{Stores, unsupported};

/// The prefix of every name this module answers.
pub const DOMAIN: &str = "inference.policy";

/// The stores of one scenario this module reads; none yet.
#[derive(Default)]
pub struct State;

pub fn execute_command(
    _stores: &mut Stores,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    Err(unsupported(&request.command.to_string()))
}

pub fn query_view(
    _stores: &mut Stores,
    request: &SemanticViewRequest,
) -> Result<SemanticViewResult, TargetError> {
    Err(unsupported(&request.view.to_string()))
}

pub fn observe_events(
    _stores: &mut Stores,
    request: &EventObservationRequest,
) -> Result<Vec<ObservedEvent>, TargetError> {
    Err(unsupported(&request.event.to_string()))
}
