//! Adversary probe, pass 1, story `adapter-descriptor-carrier`: does the conformance suite hold
//! the testkit's `inference.model.Adapters` view to the descriptor each `DeclareAdapter` sent?
//!
//! Three rewrites of the view's `descriptor`, through the real testkit target:
//!
//! - `Default`: every row publishes an empty descriptor, as a view that dropped the field would.
//! - `FirstOnly`: each list of the descriptor keeps its first element only.
//! - `SwapRate`: each rate limit publishes its `limit` as `window_ms` and the reverse.
//!
//! A witness scenario, written here, declares an adapter with two voices, two codecs and two
//! rate limits whose `limit` and `window_ms` differ, and asserts the row whole; it shows each
//! rewrite is a departure from the specification. The question is whether the repository's own
//! scenarios (`ess/scenarios` and what `ess` synthesizes from `ess/`) reject them too.
//!
//! Only the scenarios that read `inference.model.Adapters` are run: a rewrite of that view
//! cannot fail any other. `ess` must be on PATH.

#![allow(clippy::duplicate_mod)]

mod target;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
const WITNESS: &str = "inference.model/authored/adversary-w23-descriptor-whole";
const DEADLINE: Duration = Duration::from_secs(28);

const WITNESS_YAML: &str = r"type: ess-scenario/1
domain: inference.model
scenario: adversary-w23-descriptor-whole
summary: An adapter declared with two voices, two codecs and two rate limits is published by the Adapters view with that descriptor, whole.
arrange:
  - instance: model
    entity: inference.model.Model
  - instance: adapter
    entity: inference.model.Adapter
timeline:
  - at: 2026-01-05T09:00:00Z
    command: inference.model.DeclareModel
    input:
      family: speech
      checkpoint: hosted
      code_license: none
      weight_license: none
      checksum: none
      input_modalities: [Text]
      output_modalities: [Audio]
      provenance: hosted endpoint
      source_revision: r1
    outcome: declared
    capture: {instance: model, event: inference.model.ModelDeclared, field: model_id}
  - at: 2026-01-05T09:00:01Z
    command: inference.model.DeclareAdapter
    input:
      model_id: {$instance: model}
      backend: speech
      contract_revision: r1
      execution_form: hosted_service
      locality: external_provider
      capabilities: [output_streaming]
      max_concurrency: 1
      descriptor:
        voices: [calm, bright]
        codecs: [pcm16, opus]
        context_tokens: 32000
        rate_limits:
          - {unit: requests, limit: 60, window_ms: 60000}
          - {unit: characters, limit: 5000, window_ms: 1000}
    outcome: declared
    capture: {instance: adapter, event: inference.model.AdapterDeclared, field: adapter_id}
assert:
  - view: inference.model.Adapters
    contains:
      adapter_id: {$instance: adapter}
      descriptor:
        voices: [calm, bright]
        codecs: [pcm16, opus]
        context_tokens: 32000
        rate_limits:
          - {unit: requests, limit: 60, window_ms: 60000}
          - {unit: characters, limit: 5000, window_ms: 1000}
";

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

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root resolves")
}

/// The authored scenarios of `ess/scenarios` and the witness, synthesized together, cut to the
/// scenarios that read the Adapters view.
fn synthesized(tag: &str) -> AdmittedSuite {
    let root = repository_root();
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_millis();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "adversary-w23-descriptor-{tag}-{millis}-{}",
        std::process::id()
    ));
    let scenarios = dir.join("scenarios");
    fs::create_dir_all(&scenarios).expect("the run directory is created");
    for entry in fs::read_dir(root.join("ess/scenarios")).expect("ess/scenarios is readable") {
        let path = entry.expect("an entry").path();
        if path.extension().is_some_and(|ext| ext == "yaml") {
            fs::copy(&path, scenarios.join(path.file_name().expect("a name")))
                .expect("the scenario is copied");
        }
    }
    fs::write(
        scenarios.join("adversary-w23-descriptor-whole.yaml"),
        WITNESS_YAML,
    )
    .expect("the witness is written");
    let suite = dir.join("suite.json");
    let output = Command::new("ess")
        .args([
            "verify",
            "conform",
            "synthesize",
            "--path",
            "ess",
            "--scenarios",
        ])
        .arg(&scenarios)
        .arg("--out")
        .arg(&suite)
        .current_dir(&root)
        .output()
        .expect("`ess` runs; it must be on PATH");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut json: Value =
        serde_json::from_str(&fs::read_to_string(&suite).expect("suite.json is readable"))
            .expect("suite.json is JSON");
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
        let admitted = synthesized("witness");
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
        let admitted = synthesized("repository");
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
