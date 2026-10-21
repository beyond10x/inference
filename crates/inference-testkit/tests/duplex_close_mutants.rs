//! Do the authored scenarios of the delivery rule at a session's end
//! (`close-delivers-received-usage`, `fail-delivers-received-usage`) go red when the end breaks
//! the rule?
//!
//! Each mutant is a wrapper of the testkit's conformance target: it answers every request through
//! the real target and rewrites only what `inference.duplex.ReceivedSessionEvents` reports, as an
//! end that broke the rule would leave it.
//!
//! - `FirstOnly`: the end delivers only the first event it still held; every later one stays
//!   Received.
//! - `Restamp`: the end stamps every event of the session `delivered_before` it, the one the
//!   caller already had as well.
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
    SemanticCommandResult, SemanticViewRequest, SemanticViewResult, TargetError, ViewRow,
};
use ess_conformance::{AdmittedSuite, Runner, Status};
use ess_primitives::node::Node;

const RECEIVED: &str = "inference.duplex.ReceivedSessionEvents";
const AUTHORED: [&str; 2] = [
    "inference.duplex/authored/close-delivers-received-usage",
    "inference.duplex/authored/fail-delivers-received-usage",
];

/// How the end breaks the delivery rule, if it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mutation {
    None,
    FirstOnly,
    Restamp,
}

/// The testkit's target, with `ReceivedSessionEvents` rewritten by `mutation`.
struct Mutant {
    inner: target::Target,
    mutation: Mutation,
}

/// The session and the end a row carries, if it was delivered before an end.
fn ended(row: &ViewRow) -> Option<(Node, Node)> {
    let end = row.get("delivered_before")?;
    if *end == Node::Null {
        return None;
    }
    Some((row.get("session_id")?.clone(), end.clone()))
}

impl Mutant {
    fn new(mutation: Mutation) -> Self {
        Self {
            inner: target::Target::default(),
            mutation,
        }
    }

    fn rewrite(&self, mut result: SemanticViewResult) -> SemanticViewResult {
        match self.mutation {
            Mutation::None => {}
            Mutation::FirstOnly => {
                let mut seen: Vec<Node> = Vec::new();
                for row in &mut result.rows {
                    let Some((session, _)) = ended(row) else {
                        continue;
                    };
                    if seen.contains(&session) {
                        row.insert("delivered_before".to_owned(), Node::Null);
                        row.insert("state".to_owned(), Node::Text("Received".to_owned()));
                    } else {
                        seen.push(session);
                    }
                }
            }
            Mutation::Restamp => {
                let ends: Vec<(Node, Node)> = result.rows.iter().filter_map(ended).collect();
                for row in &mut result.rows {
                    let Some(session) = row.get("session_id").cloned() else {
                        continue;
                    };
                    if let Some((_, end)) = ends.iter().find(|(id, _)| *id == session) {
                        row.insert("delivered_before".to_owned(), end.clone());
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
        let mutate = request.view.to_string() == RECEIVED;
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
        "duplex-close-mutants-{millis}-{}",
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

/// The status of each authored scenario of the rule against the target `mutation` gives.
fn statuses(admitted: &AdmittedSuite, mutation: Mutation) -> Vec<(String, Status)> {
    let target = Mutant::new(mutation);
    let run = Runner::for_suite(admitted.suite()).run_admitted(admitted, &target);
    AUTHORED
        .iter()
        .map(|id| {
            let found = run
                .scenarios
                .iter()
                .find(|scenario| scenario.scenario.to_string() == *id)
                .unwrap_or_else(|| panic!("`{id}` is in the suite"));
            ((*id).to_owned(), found.status)
        })
        .collect()
}

#[test]
fn both_authored_scenarios_reject_every_mutant_of_the_end() {
    let admitted = synthesized();
    let control = statuses(&admitted, Mutation::None);
    assert!(
        control.iter().all(|(_, status)| *status == Status::Passed),
        "the unmutated target passes both: {control:?}"
    );
    for mutation in [Mutation::FirstOnly, Mutation::Restamp] {
        let mutated = statuses(&admitted, mutation);
        assert!(
            mutated.iter().all(|(_, status)| *status != Status::Passed),
            "{mutation:?} survives an authored scenario: {mutated:?}"
        );
    }
}
