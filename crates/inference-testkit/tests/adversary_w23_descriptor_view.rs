//! Adversary probe, pass 1, story `adapter-descriptor-carrier`: does the conformance suite hold
//! the testkit's `inference.model.Adapters` view to the descriptor each `DeclareAdapter` sent?
//!
//! Three rewrites of the view's `descriptor`, through the real testkit target:
//!
//! - `Default`: every row publishes an empty descriptor, as a view that dropped the field would.
//! - `FirstOnly`: each list of the descriptor keeps its first element only.
//! - `SwapRate`: each rate limit publishes its `limit` as `window_ms` and the reverse.
//!
//! A witness scenario, a file of `suite/witness`, declares an adapter with two voices, two codecs
//! and two rate limits whose `limit` and `window_ms` differ, and asserts the row whole; it shows
//! each rewrite is a departure from the specification. The question is whether the repository's
//! own scenarios (`ess/scenarios` and what `ess` synthesizes from `ess/`) reject them too.
//!
//! Only the scenarios that read `inference.model.Adapters` are run: a rewrite of that view
//! cannot fail any other. `ess` must be on PATH.

#![allow(clippy::duplicate_mod)]

mod suite;
mod target;

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use ess_conformance::target::{
    ConformanceTarget, EventObservationRequest, ExternalOutcomeControl, ImplementationIdentity,
    ObservedEvent, RedeliveryRequest, ScenarioContext, SemanticCommandRequest,
    SemanticCommandResult, SemanticViewRequest, SemanticViewResult, TargetError,
};
use ess_conformance::{AdmittedSuite, Runner, Status};
use ess_primitives::node::Node;
use serde_json::Value;

const ADAPTERS: &str = "inference.model.Adapters";
const SYNTHESIZED: &str = "inference.model.DeclareAdapter/outcome/declared";
/// The witness, `suite/witness/adversary-w23-descriptor-whole.yaml`.
const WITNESS: &str = "inference.model/authored/adversary-w23-descriptor-whole";
const DEADLINE: Duration = Duration::from_secs(28);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mutation {
    None,
    Default,
    FirstOnly,
    SwapRate,
}

struct Mutant {
    inner: target::Target,
    mutation: Mutation,
}

fn rewrite(mutation: Mutation, descriptor: &mut Node) {
    let Node::Map(fields) = descriptor else {
        return;
    };
    match mutation {
        Mutation::None => {}
        Mutation::Default => {
            fields.clear();
            for list in ["voices", "codecs", "rate_limits"] {
                fields.insert(list.to_owned(), Node::Seq(Vec::new()));
            }
        }
        Mutation::FirstOnly => {
            for list in ["voices", "codecs", "rate_limits"] {
                if let Some(Node::Seq(items)) = fields.get_mut(list) {
                    items.truncate(1);
                }
            }
        }
        Mutation::SwapRate => {
            if let Some(Node::Seq(limits)) = fields.get_mut("rate_limits") {
                for limit in limits {
                    if let Node::Map(limit) = limit {
                        let count = limit.remove("limit");
                        let window = limit.remove("window_ms");
                        if let (Some(count), Some(window)) = (count, window) {
                            limit.insert("limit".to_owned(), window);
                            limit.insert("window_ms".to_owned(), count);
                        }
                    }
                }
            }
        }
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
        let mutate = request.view.to_string() == ADAPTERS;
        let mut result = self.inner.query_view(request)?;
        if mutate {
            for row in &mut result.rows {
                if let Some(descriptor) = row.get_mut("descriptor") {
                    rewrite(self.mutation, descriptor);
                }
            }
        }
        Ok(result)
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

/// Runs `case` on its own thread and fails when it has not returned within [`DEADLINE`].
fn within(name: &str, case: impl FnOnce() + Send + 'static) {
    let (done, finished) = mpsc::channel();
    let runner = thread::spawn(move || {
        case();
        let _ = done.send(());
    });
    match finished.recv_timeout(DEADLINE) {
        Ok(()) => runner.join().expect("the case returned"),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            if let Err(panic) = runner.join() {
                std::panic::resume_unwind(panic);
            }
        }
        Err(mpsc::RecvTimeoutError::Timeout) => panic!("{name} ran past {DEADLINE:?}"),
    }
}

/// The authored scenarios of `ess/scenarios` and the witnesses of `suite/witness`, synthesized
/// together, cut to the scenarios that read the Adapters view.
fn synthesized() -> AdmittedSuite {
    let text = suite::synthesized(suite::Product::Witnessed);
    let mut json: Value = serde_json::from_str(&text).expect("suite.json is JSON");
    let all = json["scenarios"]
        .as_object_mut()
        .expect("the suite's scenarios are a map");
    all.retain(|_, scenario| scenario.to_string().contains(ADAPTERS));
    let ids: Vec<&String> = all.keys().collect();
    assert!(
        ids.iter().any(|id| *id == SYNTHESIZED) && ids.iter().any(|id| *id == WITNESS),
        "the cut keeps the synthesized declaration and the witness: {ids:?}"
    );
    AdmittedSuite::from_json(&json.to_string()).expect("the cut suite is admitted")
}

fn statuses(admitted: &AdmittedSuite, mutation: Mutation) -> Vec<(String, Status)> {
    let target = Mutant {
        inner: target::Target::default(),
        mutation,
    };
    let run = Runner::for_suite(admitted.suite()).run_admitted(admitted, &target);
    run.scenarios
        .iter()
        .map(|scenario| (scenario.scenario.to_string(), scenario.status))
        .collect()
}

fn status_of(statuses: &[(String, Status)], id: &str) -> Status {
    statuses
        .iter()
        .find(|(scenario, _)| scenario == id)
        .unwrap_or_else(|| panic!("`{id}` ran"))
        .1
}

#[test]
fn adversary_w23_the_witness_rejects_every_rewrite_and_the_suite_rejects_a_dropped_descriptor() {
    within("witness", || {
        let admitted = synthesized();
        let control = statuses(&admitted, Mutation::None);
        assert!(
            control.iter().all(|(_, status)| *status == Status::Passed),
            "the unmutated target passes every scenario that reads the Adapters view: {control:?}"
        );
        for mutation in [Mutation::Default, Mutation::FirstOnly, Mutation::SwapRate] {
            let run = statuses(&admitted, mutation);
            assert_ne!(
                status_of(&run, WITNESS),
                Status::Passed,
                "{mutation:?} is no departure: the witness passes it"
            );
        }
        let dropped = statuses(&admitted, Mutation::Default);
        assert_ne!(
            status_of(&dropped, SYNTHESIZED),
            Status::Passed,
            "the synthesized declaration passes a view that drops the descriptor"
        );
    });
}

#[test]
fn adversary_w23_the_repository_scenarios_reject_a_truncated_or_swapped_descriptor() {
    within("repository scenarios", || {
        let admitted = synthesized();
        let mut survivors = Vec::new();
        for mutation in [Mutation::FirstOnly, Mutation::SwapRate] {
            let run = statuses(&admitted, mutation);
            let rejected: Vec<&(String, Status)> = run
                .iter()
                .filter(|(id, status)| id != WITNESS && *status != Status::Passed)
                .collect();
            if rejected.is_empty() {
                survivors.push(mutation);
            }
        }
        assert!(
            survivors.is_empty(),
            "rewrites of the Adapters view's descriptor that the witness rejects and every \
             scenario of the repository passes: {survivors:?}; the synthesized declaration \
             sends one voice, one codec and one rate limit whose limit equals its window_ms, \
             and no authored scenario reads the view"
        );
    });
}
