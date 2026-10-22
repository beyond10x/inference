//! Adversary probe: do the two authored scenarios of story `duplex-late-usage-counted`
//! (`close-delivers-received-usage`, `fail-delivers-received-usage`) go red when the close or the
//! failure breaks the delivery rule in a way other than the one they were written against?
//!
//! The mutant is expressed here, not in the module under attack: a wrapper of the testkit's
//! conformance target answers every request through the real target and rewrites only what
//! `inference.duplex.ReceivedSessionEvents` reports. Mutant `restamp`: when a session ends, every
//! row of it already delivered is stamped `delivered_before` the end as well, as a
//! `deliver_held` that does not check the row was still held would stamp it. The rule says an
//! event the caller already had stays delivered as it was; a scenario of the rule must see this.
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

const RECEIVED: &str = "inference.duplex.ReceivedSessionEvents";
const AUTHORED: [&str; 2] = [
    "inference.duplex/authored/close-delivers-received-usage",
    "inference.duplex/authored/fail-delivers-received-usage",
];

/// The testkit's target, with `ReceivedSessionEvents` rewritten when `restamp` is set.
struct Mutant {
    inner: target::Target,
    restamp: bool,
}

impl Mutant {
    fn rewrite(&self, mut result: SemanticViewResult) -> SemanticViewResult {
        if !self.restamp {
            return result;
        }
        let ends: Vec<(Node, Node)> = result
            .rows
            .iter()
            .filter_map(|row| {
                let end = row.get("delivered_before")?;
                (*end != Node::Null).then(|| (row.get("session_id").cloned(), end.clone()))
            })
            .filter_map(|(session, end)| Some((session?, end)))
            .collect();
        for row in &mut result.rows {
            let Some(session) = row.get("session_id").cloned() else {
                continue;
            };
            if let Some((_, end)) = ends.iter().find(|(id, _)| *id == session) {
                row.insert("delivered_before".to_owned(), end.clone());
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
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("adversary11-{millis}-{}", std::process::id()));
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

/// The status of each authored scenario of the story against `target`.
fn statuses(admitted: &AdmittedSuite, target: &Mutant) -> Vec<(String, Status)> {
    let run = Runner::for_suite(admitted.suite()).run_admitted(admitted, target);
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
fn the_authored_scenarios_reject_a_close_that_restamps_delivered_events() {
    let admitted = synthesized();
    let control = statuses(
        &admitted,
        &Mutant {
            inner: target::Target::default(),
            restamp: false,
        },
    );
    assert!(
        control.iter().all(|(_, status)| *status == Status::Passed),
        "the unmutated target passes both: {control:?}"
    );
    let mutated = statuses(
        &admitted,
        &Mutant {
            inner: target::Target::default(),
            restamp: true,
        },
    );
    assert!(
        mutated.iter().any(|(_, status)| *status != Status::Passed),
        "an end that stamps the already-delivered transcript `delivered_before` the close \
         survives both authored scenarios: {mutated:?}"
    );
}
