//! `inference.policy`: the command, view and event of `ess/domains/policy.yaml`, answered by
//! [`PolicyDomain`].
//!
//! Each request field is read on its own ([`Input`]); the outcome, the error and the event come
//! from what the store returned, the refusing outcome from the [`TableRule`] the row breaks. A
//! command's consistency token is the target's ([`Stores::token`]), and a read demanding a token
//! the stores have not reached is an error ([`Stores::require`]). An observed event's position is
//! its place in the target's one publication order ([`Stores::policy_position`]).
//!
//! [`TableRule`]: inference_core::policy::TableRule

use std::collections::BTreeMap;

use ess_conformance::scenario::{CommandRef, ErrorRef, EventRef, OutcomeRef};
use ess_conformance::target::{
    DeclaredErrorValue, EventObservationRequest, ObservedEvent, SemanticCommandRequest,
    SemanticCommandResult, SemanticViewRequest, SemanticViewResult, TargetError, ViewRow,
};
use inference_core::policy::{DeclareProfile, DeploymentProfile, ProfileDeclared};
use inference_testkit::policy::{PolicyDomain, PolicyEvent};

use super::{Input, Stores, named_list_node, named_node, unsupported};

/// The prefix of every name this module answers.
pub const DOMAIN: &str = "inference.policy";

const DECLARE_PROFILE: &str = "inference.policy.DeclareProfile";
const PROFILE_DECLARED: &str = "inference.policy.ProfileDeclared";
const PROFILE_OUTSIDE_TABLE: &str = "inference.policy.ProfileOutsideTable";
const PROFILES: &str = "inference.policy.Profiles";

/// The `inference.policy` store of one scenario.
pub type State = PolicyDomain;

pub fn execute_command(
    stores: &mut Stores,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    stores.publish();
    let name = request.command.to_string();
    let result = match name.as_str() {
        DECLARE_PROFILE => declare_profile(&mut stores.policy, request)?,
        _ => return Err(unsupported(&name)),
    };
    stores.publish();
    Ok(result.with_consistency(stores.token()?))
}

fn declare_profile(
    domain: &mut PolicyDomain,
    request: &SemanticCommandRequest,
) -> Result<SemanticCommandResult, TargetError> {
    let mut input = Input::of(request);
    let declare = DeclareProfile {
        kind: input.named("kind")?,
        egress: input.named("egress")?,
        admitted: input.named_list("admitted")?,
    };
    input.finish()?;
    Ok(match domain.declare_profile(declare) {
        Ok(event) => SemanticCommandResult::took(outcome(DECLARE_PROFILE, "declared"))
            .emitting(profile_declared(&event)?.in_activity(request.correlation.clone())),
        Err(refused) => {
            SemanticCommandResult::took(outcome(DECLARE_PROFILE, refused.rule.outcome()))
                .with_error(
                    DeclaredErrorValue::new(parse::<ErrorRef>(PROFILE_OUTSIDE_TABLE))
                        .with("kind", named_node(&refused.error.kind)?)
                        .with("egress", named_node(&refused.error.egress)?)
                        .with("admitted", named_list_node(&refused.error.admitted)?),
                )
        }
    })
}

pub fn query_view(
    stores: &mut Stores,
    request: &SemanticViewRequest,
) -> Result<SemanticViewResult, TargetError> {
    let name = request.view.to_string();
    stores.require(&name, &request.consistency)?;
    let rows = match name.as_str() {
        PROFILES => stores
            .policy
            .profiles()
            .map(profiles_row)
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
    if wanted != PROFILE_DECLARED {
        return Err(unsupported(&wanted));
    }
    stores.publish();
    let mut observed = Vec::new();
    for (index, event) in stores.policy.events().iter().enumerate() {
        let PolicyEvent::ProfileDeclared(event) = event;
        let position = stores.policy_position(index)?;
        observed.push(
            profile_declared(event)?
                .in_activity(request.correlation.clone())
                .at(position),
        );
    }
    Ok(observed)
}

fn profile_declared(event: &ProfileDeclared) -> Result<ObservedEvent, TargetError> {
    Ok(ObservedEvent::new(parse::<EventRef>(PROFILE_DECLARED))
        .with("profile_id", named_node(&event.profile_id)?)
        .with("kind", named_node(&event.kind)?)
        .with("egress", named_node(&event.egress)?)
        .with("admitted", named_list_node(&event.admitted)?))
}

fn profiles_row(profile: &DeploymentProfile) -> Result<ViewRow, TargetError> {
    Ok(BTreeMap::from([
        ("profile_id".to_owned(), named_node(&profile.profile_id())?),
        ("kind".to_owned(), named_node(&profile.kind())?),
        ("egress".to_owned(), named_node(&profile.egress())?),
        ("admitted".to_owned(), named_list_node(profile.admitted())?),
        ("state".to_owned(), named_node(&profile.state())?),
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
