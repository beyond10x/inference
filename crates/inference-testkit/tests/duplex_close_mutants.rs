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
//! - `Watermark`: the end stamps every event of the session from the first one it still held
//!   onward, by sequence, as an end that keeps a delivery cursor instead of each event's state
//!   would: one the caller had out of order, after one still held, is restamped.
//! - `CrossSession`: the end also delivers what another open session still holds, as an end that
//!   does not filter the received events by session would.
//!
//! The suite is the one `ess` synthesizes from `ess/`, read through `suite/mod.rs` as `conform.rs`
//! reads it; `ess` must be on PATH.

#![allow(clippy::duplicate_mod)]

mod suite;
mod target;

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
    Watermark,
    CrossSession,
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

fn sequence(row: &ViewRow) -> Option<i64> {
    target::integer(row.get("sequence")?)
}

fn deliver_before(row: &mut ViewRow, end: &Node) {
    row.insert("delivered_before".to_owned(), end.clone());
    row.insert("state".to_owned(), Node::Text("Delivered".to_owned()));
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
            Mutation::Watermark => {
                let mut lowest: Vec<(Node, Node, i64)> = Vec::new();
                for row in &result.rows {
                    let (Some((session, end)), Some(at)) = (ended(row), sequence(row)) else {
                        continue;
                    };
                    match lowest.iter_mut().find(|(id, _, _)| *id == session) {
                        Some((_, _, low)) => *low = (*low).min(at),
                        None => lowest.push((session, end, at)),
                    }
                }
                for row in &mut result.rows {
                    let (Some(session), Some(at)) = (row.get("session_id").cloned(), sequence(row))
                    else {
                        continue;
                    };
                    if let Some((_, end, low)) = lowest.iter().find(|(id, _, _)| *id == session)
                        && at > *low
                    {
                        deliver_before(row, end);
                    }
                }
            }
            Mutation::CrossSession => {
                let ends: Vec<(Node, Node)> = result.rows.iter().filter_map(ended).collect();
                for row in &mut result.rows {
                    let held = row.get("state") == Some(&Node::Text("Received".to_owned()));
                    let Some(session) = row.get("session_id").cloned() else {
                        continue;
                    };
                    if let Some((_, end)) = ends.iter().find(|(id, _)| *id != session)
                        && held
                    {
                        deliver_before(row, end);
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

/// The authored scenarios of the rule, cut out of the synthesized suite: the tests read no
/// other scenario's status.
fn synthesized() -> AdmittedSuite {
    let cut = suite::cut(&suite::synthesized(suite::Product::Authored), |id| {
        AUTHORED.contains(&id)
    });
    AdmittedSuite::from_json(&cut).expect("the cut suite is admitted")
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

/// The close's scenario holds two sessions and an event the caller had out of order before the
/// close, so it rejects an end that keeps a delivery cursor and one that crosses sessions.
#[test]
fn the_close_scenario_rejects_the_watermark_and_cross_session_mutants() {
    let admitted = synthesized();
    for mutation in [Mutation::Watermark, Mutation::CrossSession] {
        let mutated = statuses(&admitted, mutation);
        let close = mutated
            .iter()
            .find(|(id, _)| id == AUTHORED[0])
            .expect("the close's scenario ran");
        assert_ne!(
            close.1,
            Status::Passed,
            "{mutation:?} survives the close's scenario: {mutated:?}"
        );
    }
}
