//! Adversary probe, pass 3, story `duplex-late-usage-counted` at 4f5a175: two more mutants of the
//! delivery rule at a session's end, beyond the `FirstOnly` and `Restamp` that
//! `duplex_close_mutants.rs` shows the authored scenarios reject.
//!
//! - `Watermark`: the end stamps every event of the session from the first one it still held
//!   onward, by sequence, as an end that keeps a delivery cursor instead of each event's state
//!   would. An event the caller already had out of order, after one still held, is restamped.
//! - `CrossSession`: the end also delivers what another open session still holds, as an end that
//!   does not filter the received events by session would.
//!
//! Both are rewrites of what `inference.duplex.ReceivedSessionEvents` reports, through the real
//! testkit target, as `duplex_close_mutants.rs` builds its mutants. A witness scenario, a file
//! of `suite/witness` synthesized with the authored ones, shows each mutant is a real departure
//! from the rule: the unmutated target passes it and each mutant fails it. The case under test is
//! whether the authored scenarios of the rule reject them too.
//!
//! The suite is the one `ess` synthesizes from `ess/` and the witnesses of `suite/witness`, read
//! through `suite/mod.rs`; `ess` must be on PATH.

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
/// The witness, `suite/witness/adversary-close-after-out-of-order-delivery.yaml`. Session A
/// receives three events, the caller has the third before the close while the first two are
/// still held; session B, open beside it, holds one. A's close delivers its first two before
/// it, leaves the third as it was, and leaves B's untouched.
const WITNESS: &str = "inference.duplex/authored/adversary-close-after-out-of-order-delivery";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mutation {
    None,
    Watermark,
    CrossSession,
}

struct Mutant {
    inner: target::Target,
    mutation: Mutation,
}

fn text(row: &ViewRow, field: &str) -> Option<Node> {
    row.get(field).cloned()
}

fn end_of(row: &ViewRow) -> Option<Node> {
    let end = row.get("delivered_before")?;
    (*end != Node::Null).then(|| end.clone())
}

fn sequence(row: &ViewRow) -> Option<i64> {
    target::integer(row.get("sequence")?)
}

impl Mutant {
    fn rewrite(&self, mut result: SemanticViewResult) -> SemanticViewResult {
        // Each session that ended, with its end and the lowest sequence stamped with it.
        let mut ends: Vec<(Node, Node, i64)> = Vec::new();
        for row in &result.rows {
            let (Some(session), Some(end), Some(at)) =
                (text(row, "session_id"), end_of(row), sequence(row))
            else {
                continue;
            };
            match ends.iter_mut().find(|(id, _, _)| *id == session) {
                Some((_, _, low)) => *low = (*low).min(at),
                None => ends.push((session, end, at)),
            }
        }
        for row in &mut result.rows {
            let Some(session) = text(row, "session_id") else {
                continue;
            };
            match self.mutation {
                Mutation::None => {}
                Mutation::Watermark => {
                    if let (Some((_, end, low)), Some(at)) =
                        (ends.iter().find(|(id, _, _)| *id == session), sequence(row))
                        && at > *low
                    {
                        row.insert("delivered_before".to_owned(), end.clone());
                        row.insert("state".to_owned(), Node::Text("Delivered".to_owned()));
                    }
                }
                Mutation::CrossSession => {
                    let held = row.get("state") == Some(&Node::Text("Received".to_owned()));
                    if let Some((_, end, _)) = ends.iter().find(|(id, _, _)| *id != session)
                        && held
                    {
                        row.insert("delivered_before".to_owned(), end.clone());
                        row.insert("state".to_owned(), Node::Text("Delivered".to_owned()));
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

/// The authored scenarios of the rule and the witness, synthesized with every scenario of
/// `ess/scenarios` and cut out of that suite: the test reads no other scenario's status.
fn synthesized() -> AdmittedSuite {
    let text = suite::synthesized(suite::Product::Witnessed);
    let cut = suite::cut(&text, |id| AUTHORED.contains(&id) || id == WITNESS);
    AdmittedSuite::from_json(&cut).expect("the cut suite is admitted")
}

fn statuses(admitted: &AdmittedSuite, mutation: Mutation, ids: &[&str]) -> Vec<(String, Status)> {
    let target = Mutant {
        inner: target::Target::default(),
        mutation,
    };
    let run = Runner::for_suite(admitted.suite()).run_admitted(admitted, &target);
    ids.iter()
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
fn the_authored_scenarios_reject_the_watermark_and_cross_session_mutants() {
    let admitted = synthesized();
    let all = [AUTHORED[0], AUTHORED[1], WITNESS];
    let control = statuses(&admitted, Mutation::None, &all);
    assert!(
        control.iter().all(|(_, status)| *status == Status::Passed),
        "the unmutated target passes the authored scenarios and the witness: {control:?}"
    );
    let mut survivors = Vec::new();
    for mutation in [Mutation::Watermark, Mutation::CrossSession] {
        let witness = statuses(&admitted, mutation, &[WITNESS]);
        assert!(
            witness.iter().all(|(_, status)| *status != Status::Passed),
            "{mutation:?} is no departure from the rule: the witness passes it"
        );
        let authored = statuses(&admitted, mutation, &AUTHORED);
        if authored.iter().all(|(_, status)| *status == Status::Passed) {
            survivors.push((mutation, authored));
        }
    }
    assert!(
        survivors.is_empty(),
        "mutants the witness rejects and every authored scenario passes: {survivors:?}"
    );
}
