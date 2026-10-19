//! Adversary pass 1 against the forced `PlanPipeline` refusals of the testkit target.
//!
//! `ess/domains/graph.yaml` declares `inference.graph.NodeUnbound.node_id` as a `NodeId` and
//! `inference.graph.CapabilityMismatch.capability` as a `Capability`, neither `Optional`, and the
//! `unbound` and `mismatch` outcomes give both a generated value. A forced refusal over a pipeline
//! the planner would plan is answered here without them.

mod target;

use ess_conformance::scenario::{CommandRef, OutcomeRef};
use ess_conformance::target::{
    ConformanceTarget, ExternalOutcomeControl, ScenarioContext, SemanticCommandRequest,
    SemanticCommandResult,
};
use ess_primitives::ids::CorrelationId;
use ess_primitives::node::Node;

use target::Target;

fn correlation() -> CorrelationId {
    CorrelationId::new("adversary1-planning").expect("a correlation id")
}

fn text(value: &str) -> Node {
    Node::Text(value.to_owned())
}

fn list(values: &[&str]) -> Node {
    Node::Seq(values.iter().map(|value| text(value)).collect())
}

fn run(target: &Target, name: &str, input: Vec<(&str, Node)>) -> SemanticCommandResult {
    target
        .execute_command(SemanticCommandRequest {
            command: name.parse::<CommandRef>().expect("a command name"),
            actor: None,
            caller: None,
            input: input
                .into_iter()
                .map(|(field, value)| (field.to_owned(), value))
                .collect(),
            correlation: correlation(),
        })
        .unwrap_or_else(|error| panic!("`{name}` answers: {error}"))
}

fn emitted(result: &SemanticCommandResult, field: &str) -> Node {
    assert_eq!(result.direct_events.len(), 1, "{result:?}");
    result.direct_events[0].payload[field].clone()
}

/// A target holding one Validated, empty pipeline, which the planner would plan.
fn validated() -> (Target, Node) {
    let target = Target::default();
    target
        .begin_scenario(&ScenarioContext::new(
            "inference.graph.PlanPipeline/outcome/unbound"
                .parse()
                .expect("a scenario id"),
            correlation(),
        ))
        .expect("the scenario begins");
    let profile_id = emitted(
        &run(
            &target,
            "inference.policy.DeclareProfile",
            vec![
                ("kind", text("offline_strict")),
                ("egress", text("denied")),
                ("admitted", list(&[])),
            ],
        ),
        "profile_id",
    );
    let pipeline_id = emitted(
        &run(
            &target,
            "inference.graph.DeclarePipeline",
            vec![
                ("name", text("voice")),
                ("schema_revision", text("1")),
                ("mode", text("batch")),
                ("profile_id", profile_id),
                ("required_capabilities", list(&[])),
            ],
        ),
        "pipeline_id",
    );
    run(
        &target,
        "inference.graph.ValidatePipeline",
        vec![("pipeline_id", pipeline_id.clone())],
    );
    (target, pipeline_id)
}

fn forced(outcome: &str) -> SemanticCommandResult {
    let (target, pipeline_id) = validated();
    target
        .configure_external_outcome(ExternalOutcomeControl {
            force: OutcomeRef::new(
                "inference.graph.PlanPipeline"
                    .parse::<CommandRef>()
                    .expect("a command name"),
                outcome.parse().expect("an outcome name"),
            ),
            correlation: correlation(),
        })
        .expect("the outcome is external");
    run(
        &target,
        "inference.graph.PlanPipeline",
        vec![("pipeline_id", pipeline_id)],
    )
}

#[test]
fn a_forced_unbound_carries_the_node_its_declared_error_requires() {
    let answer = forced("unbound");
    let error = answer.error.as_ref().expect("a declared error");
    assert_eq!(error.error.to_string(), "inference.graph.NodeUnbound");
    assert!(
        error.fields.contains_key("node_id"),
        "NodeUnbound.node_id is not Optional in ess/domains/graph.yaml: {error:?}"
    );
}

#[test]
fn a_forced_mismatch_carries_the_capability_its_declared_error_requires() {
    let answer = forced("mismatch");
    let error = answer.error.as_ref().expect("a declared error");
    assert_eq!(
        error.error.to_string(),
        "inference.graph.CapabilityMismatch"
    );
    assert!(
        error.fields.contains_key("capability"),
        "CapabilityMismatch.capability is not Optional in ess/domains/graph.yaml: {error:?}"
    );
}
