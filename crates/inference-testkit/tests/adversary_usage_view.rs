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
//! The suite is synthesized with `ess` from `ess/`, as `conform.rs` does; `ess` must be on PATH.

#![allow(clippy::duplicate_mod)]

mod target;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

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

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root resolves")
}

fn synthesized() -> AdmittedSuite {
    let root = repository_root();
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_millis();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "adversary-usage-view-{millis}-{}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("the run directory is created");
    let suite = dir.join("suite.json");
    let output = Command::new("ess")
        .args([
            "verify",
            "conform",
            "synthesize",
            "--path",
            "ess",
            "--scenarios",
            "ess/scenarios",
            "--out",
            suite.to_str().expect("UTF-8"),
        ])
        .current_dir(&root)
        .output()
        .expect("`ess` runs; it must be on PATH");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = fs::read_to_string(&suite).expect("suite.json is readable");
    AdmittedSuite::from_json(&text).expect("the synthesized suite is admitted")
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
        "the unmutated target passes every scenario: {control:?}"
    );
    let dropped = failing(&admitted, &Mutant::new(Mutation::DropQuantities));
    assert!(
        !dropped.is_empty(),
        "a view that publishes no quantities survives the suite"
    );
    let zeroed = failing(&admitted, &Mutant::new(Mutation::ZeroUnknown));
    assert!(
        !zeroed.is_empty(),
        "a view that publishes an unknown quantity with value 0 survives the whole suite: no \
         scenario records a quantity of source `unknown`, so the invariant \
         `source != unknown or not defined(value)` is checked only where it holds trivially"
    );
}
