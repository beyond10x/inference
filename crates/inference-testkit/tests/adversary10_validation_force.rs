//! Adversary pass 1 against the forced `ValidatePipeline/rejected` of the testkit target.
//!
//! The target documents that a forced outcome is held until the next invocation of its command,
//! and that an invocation of another command leaves it in place. The unit's own case forces and
//! validates at once; this one puts a graph command of another name between the two.

mod target;

use std::collections::BTreeMap;

use ess_conformance::scenario::{CommandRef, OutcomeRef};
use ess_conformance::target::{
    ConformanceTarget, ExternalOutcomeControl, ScenarioContext, SemanticCommandRequest,
    SemanticCommandResult,
};
use ess_primitives::ids::CorrelationId;
use ess_primitives::node::Node;

use target::Target;

fn correlation() -> CorrelationId {
    CorrelationId::new("adversary10").expect("a correlation id")
}

fn text(value: &str) -> Node {
    Node::Text(value.to_owned())
}

fn list(values: &[&str]) -> Node {
    Node::Seq(values.iter().map(|value| text(value)).collect())
}

fn begun() -> Target {
    let target = Target::default();
    let context = ScenarioContext::new(
        "inference.graph.ValidatePipeline/outcome/rejected"
            .parse()
            .expect("a scenario id"),
        correlation(),
    );
    target
        .begin_scenario(&context)
        .expect("the scenario begins");
    target
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

fn branch(result: &SemanticCommandResult) -> String {
    result
        .outcome
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_default()
}

fn declare_pipeline(target: &Target) -> Node {
    let profile_id = emitted(
        &run(
            target,
            "inference.policy.DeclareProfile",
            vec![
                ("kind", text("offline_strict")),
                ("egress", text("denied")),
                ("admitted", list(&[])),
            ],
        ),
        "profile_id",
    );
    emitted(
        &run(
            target,
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
    )
}

#[test]
fn a_forced_rejection_outlives_another_graph_command() {
    let target = begun();
    let pipeline_id = declare_pipeline(&target);

    target
        .configure_external_outcome(ExternalOutcomeControl {
            force: OutcomeRef::new(
                "inference.graph.ValidatePipeline"
                    .parse::<CommandRef>()
                    .expect("a command name"),
                "rejected".parse().expect("an outcome name"),
            ),
            correlation: correlation(),
        })
        .expect("rejected is external");

    let other = declare_pipeline(&target);
    assert_ne!(other, pipeline_id);

    let answer = run(
        &target,
        "inference.graph.ValidatePipeline",
        vec![("pipeline_id", pipeline_id.clone())],
    );
    assert_eq!(branch(&answer), "inference.graph.ValidatePipeline/rejected");
    let error = answer.error.as_ref().expect("a declared error");
    assert_eq!(error.error.to_string(), "inference.graph.IncompatibleGraph");
    assert_eq!(
        error.fields,
        BTreeMap::from([("findings".to_owned(), Node::Seq(Vec::new()))])
    );
    assert!(answer.direct_events.is_empty());

    let next = run(
        &target,
        "inference.graph.ValidatePipeline",
        vec![("pipeline_id", pipeline_id)],
    );
    assert_eq!(branch(&next), "inference.graph.ValidatePipeline/validated");
}
