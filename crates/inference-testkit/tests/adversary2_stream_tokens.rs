//! Adversary pass 1 against the consistency tokens of the `inference.stream` target module.
//!
//! The runner reads a `read_your_writes` view at `AtLeast(token)`, the token being the one the
//! scenario's last command returned, whatever domain that command belongs to (ess 0.60.0
//! `runner.rs`, `query_view`). A token is the target's, not a module's: a stream view read right
//! after a command of another domain must still be answered, since that command wrote nothing the
//! stream store has to wait for.

mod target;

use std::collections::BTreeMap;

use ess_conformance::scenario::{CommandRef, ViewRef};
use ess_conformance::target::{
    ConformanceTarget, Deadline, ScenarioContext, SemanticCommandRequest, SemanticViewRequest,
};
use ess_primitives::consistency::QueryConsistency;
use ess_primitives::ids::CorrelationId;
use ess_primitives::node::Node;
use ess_primitives::time::Timestamp;

use target::Target;

fn correlation() -> CorrelationId {
    CorrelationId::new("adversary2-tokens").expect("a correlation id")
}

fn text(value: &str) -> Node {
    Node::Text(value.to_owned())
}

fn declare_model() -> SemanticCommandRequest {
    let list = |values: &[&str]| Node::Seq(values.iter().map(|value| text(value)).collect());
    SemanticCommandRequest {
        command: "inference.model.DeclareModel"
            .parse::<CommandRef>()
            .expect("a command name"),
        actor: None,
        caller: None,
        input: [
            ("family", text("family")),
            ("checkpoint", text("checkpoint")),
            ("code_license", text("code_license")),
            ("weight_license", text("weight_license")),
            ("checksum", text("checksum")),
            ("input_modalities", list(&["Text"])),
            ("output_modalities", list(&["Audio"])),
            ("provenance", text("provenance")),
            ("source_revision", text("source_revision")),
        ]
        .into_iter()
        .map(|(field, value)| (field.to_owned(), value))
        .collect(),
        correlation: correlation(),
    }
}

#[test]
fn a_stream_view_read_after_a_command_of_another_domain_is_answered() {
    let target = Target::default();
    target
        .begin_scenario(&ScenarioContext::new(
            "inference.stream.CommitSegment/outcome/wrong-state"
                .parse()
                .expect("a scenario id"),
            correlation(),
        ))
        .expect("the scenario begins");
    let declared = target
        .execute_command(declare_model())
        .expect("DeclareModel answers");
    assert!(declared.error.is_none(), "{declared:?}");
    let token = declared.consistency.expect("DeclareModel returns a token");

    for view in ["inference.stream.Generations", "inference.stream.Segments"] {
        let answer = target.query_view(SemanticViewRequest {
            view: view.parse::<ViewRef>().expect("a view name"),
            params: BTreeMap::new(),
            consistency: QueryConsistency::at_least(token.clone()),
            correlation: correlation(),
            deadline: Deadline::at(Timestamp::from_epoch_millis(u64::MAX)),
        });
        let rows = answer.unwrap_or_else(|error| {
            panic!("`{view}` at the token of inference.model.DeclareModel ({token:?}): {error}")
        });
        assert!(rows.rows.is_empty(), "{view}: {:?}", rows.rows);
    }
}
