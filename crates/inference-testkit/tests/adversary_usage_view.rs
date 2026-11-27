//! Adversary probe, story `usage-quantity-view`: does the conformance suite go red when the view
//! `inference.usage.UsageRecords` publishes a quantity that breaks the rule the story says it now
//! obliges a scenario for?
//!
//! `ess/domains/usage.yaml` states two invariants of `inference.usage.Quantity`: a known source
//! has a value, and an unknown source has none ("kept unknown and never read as zero"). The
//! mutants are expressed here, not in the module under attack: a wrapper of the testkit's target
//! answers every request through the real target and rewrites only the rows of `UsageRecords`.
//!
//! - `zero_unknown`: every quantity whose source is `unknown` is published with value 0, the
//!   reading the specification forbids. Expected: some scenario fails.
//! - `drop_quantities`: every row's `quantities` is published empty. A control: it shows the
//!   harness reaches the view, and is expected to be caught.
//!
//! The suite is the one `ess` synthesizes from `ess/`, read through `suite/mod.rs` as `conform.rs`
//! reads it, cut to the scenarios that name `UsageRecords`; `ess` must be on PATH. A mutant
//! rewrites only the rows of that view, so a scenario that does not name it runs alike against
//! the target and against every mutant, and the cut leaves out no scenario that could catch one.
//! Admitting the whole suite takes seconds; that the target passes all of it is `conform.rs`'s
//! case.

#![allow(clippy::duplicate_mod)]

mod suite;
mod target;

use ess_conformance::target::{
    ConformanceTarget, EventObservationRequest, ExternalOutcomeControl, ImplementationIdentity,
    ObservedEvent, RedeliveryRequest, ScenarioContext, SemanticCommandRequest,
    SemanticCommandResult, SemanticViewRequest, SemanticViewResult, TargetError,
};
use ess_conformance::{AdmittedSuite, Runner, Status};
use ess_primitives::node::Node;

const USAGE_RECORDS: &str = "inference.usage.UsageRecords";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mutation {
    None,
    ZeroUnknown,
    DropQuantities,
}

struct Mutant {
    inner: target::Target,
    mutation: Mutation,
}

impl Mutant {
    fn new(mutation: Mutation) -> Self {
        Self {
            inner: target::Target::default(),
            mutation,
        }
    }

    fn rewrite(&self, mut result: SemanticViewResult) -> SemanticViewResult {
        for row in &mut result.rows {
            match self.mutation {
                Mutation::None => {}
                Mutation::DropQuantities => {
                    row.insert("quantities".to_owned(), Node::Seq(Vec::new()));
                }
                Mutation::ZeroUnknown => {
                    if let Some(Node::Seq(quantities)) = row.get_mut("quantities") {
                        for quantity in quantities {
                            if let Node::Map(fields) = quantity {
                                let unknown = matches!(
                                    fields.get("source"),
                                    Some(Node::Text(source)) if source == "unknown"
                                );
                                if unknown {
                                    fields.insert("value".to_owned(), target::integer_node(0));
                                }
                            }
                        }
                    }
                }
            }
        }
        result
    }
}

impl ConformanceTarget for Mutant {
    fn identity(&self) -> Result<ImplementationIdentity, TargetError> {
        self.inner.identity()
    }

    fn begin_scenario(&self, scenario: &ScenarioContext) -> Result<(), TargetError> {
        self.inner.begin_scenario(scenario)
    }

    fn execute_command(
        &self,
        request: SemanticCommandRequest,
    ) -> Result<SemanticCommandResult, TargetError> {
        self.inner.execute_command(request)
    }

    fn query_view(&self, request: SemanticViewRequest) -> Result<SemanticViewResult, TargetError> {
        let mutate = request.view.to_string() == USAGE_RECORDS;
        let result = self.inner.query_view(request)?;
        Ok(if mutate { self.rewrite(result) } else { result })
    }

    fn observe_events(
        &self,
        request: EventObservationRequest,
    ) -> Result<Vec<ObservedEvent>, TargetError> {
        self.inner.observe_events(request)
    }

    fn configure_external_outcome(
        &self,
        request: ExternalOutcomeControl,
    ) -> Result<(), TargetError> {
        self.inner.configure_external_outcome(request)
    }

    fn redeliver_event(&self, request: RedeliveryRequest) -> Result<(), TargetError> {
        self.inner.redeliver_event(request)
    }

    fn end_scenario(&self, scenario: &ScenarioContext) -> Result<(), TargetError> {
        self.inner.end_scenario(scenario)
    }
}

/// The scenarios of the synthesized suite that name [`USAGE_RECORDS`].
fn synthesized() -> AdmittedSuite {
    let cut = suite::cut_by(
        &suite::synthesized(suite::Product::Authored),
        |_, scenario| scenario.to_string().contains(USAGE_RECORDS),
    );
    AdmittedSuite::from_json(&cut).expect("the cut suite is admitted")
}

/// Every scenario of the suite that does not pass against `target`.
fn failing(admitted: &AdmittedSuite, target: &Mutant) -> Vec<(String, Status)> {
    let run = Runner::for_suite(admitted.suite()).run_admitted(admitted, target);
    run.scenarios
        .iter()
        .filter(|scenario| scenario.status != Status::Passed)
        .map(|scenario| (scenario.scenario.to_string(), scenario.status))
        .collect()
}

#[test]
fn the_suite_rejects_a_view_that_reads_an_unknown_quantity_as_zero() {
    let admitted = synthesized();
    let control = failing(&admitted, &Mutant::new(Mutation::None));
    assert!(
        control.is_empty(),
        "the unmutated target passes every scenario that names the view: {control:?}"
    );
    let dropped = failing(&admitted, &Mutant::new(Mutation::DropQuantities));
    assert!(
        !dropped.is_empty(),
        "a view that publishes no quantities survives the suite"
    );
    let zeroed = failing(&admitted, &Mutant::new(Mutation::ZeroUnknown));
    assert!(
        !zeroed.is_empty(),
        "a view that publishes an unknown quantity with value 0 survives every scenario that \
         names the view: no \
         scenario records a quantity of source `unknown`, so the invariant \
         `source != unknown or not defined(value)` is checked only where it holds trivially"
    );
}
