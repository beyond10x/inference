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
//! testkit target, as `duplex_close_mutants.rs` builds its mutants. A witness scenario, written
//! here and synthesized with the authored ones, shows each mutant is a real departure from the
//! rule: the unmutated target passes it and each mutant fails it. The case under test is
//! whether the authored scenarios of the rule reject them too.
//!
//! The suite is synthesized with `ess` from `ess/`; `ess` must be on PATH.

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
const WITNESS: &str = "inference.duplex/authored/adversary-close-after-out-of-order-delivery";

/// Session A receives three events, the caller has the third before the close while the first
/// two are still held; session B, open beside it, holds one. A's close delivers its first two
/// before it, leaves the third as it was, and leaves B's untouched.
const WITNESS_YAML: &str = r"type: ess-scenario/1
domain: inference.duplex
scenario: adversary-close-after-out-of-order-delivery
summary: An event the caller had out of order before the close stays as it was, and the close of one session leaves what another holds.
arrange:
  - instance: model
    entity: inference.model.Model
  - instance: adapter
    entity: inference.model.Adapter
  - instance: a
    entity: inference.duplex.SpeechSession
  - instance: b
    entity: inference.duplex.SpeechSession
  - instance: a1
    entity: inference.duplex.ReceivedSessionEvent
  - instance: a2
    entity: inference.duplex.ReceivedSessionEvent
  - instance: a3
    entity: inference.duplex.ReceivedSessionEvent
  - instance: b1
    entity: inference.duplex.ReceivedSessionEvent
timeline:
  - at: 2026-01-05T09:00:00Z
    command: inference.model.DeclareModel
    input:
      family: speech
      checkpoint: hosted
      code_license: none
      weight_license: none
      checksum: none
      input_modalities: [Audio]
      output_modalities: [Audio, Text]
      provenance: hosted endpoint
      source_revision: r1
    outcome: declared
    capture: {instance: model, event: inference.model.ModelDeclared, field: model_id}
  - at: 2026-01-05T09:00:01Z
    command: inference.model.DeclareAdapter
    input:
      model_id: {$instance: model}
      backend: realtime
      contract_revision: r1
      execution_form: hosted_service
      locality: external_provider
      capabilities: [output_streaming, incremental_input, concurrent_input_output, interruptible]
      max_concurrency: 2
    outcome: declared
    capture: {instance: adapter, event: inference.model.AdapterDeclared, field: adapter_id}
  - at: 2026-01-05T09:00:02Z
    command: inference.duplex.OpenSpeechSession
    input:
      adapter_id: {$instance: adapter}
      voice: calm
      instructions: answer briefly
      input_format: {encoding: pcm_s16le, sample_rate: 16000, channels: 1}
      output_format: {encoding: pcm_s16le, sample_rate: 16000, channels: 1}
      turn_detection: server_vad
      auto_response: true
      auto_interrupt: false
      tools: []
      idle_limit_ms: 15000
    outcome: opened
    capture: {instance: a, event: inference.duplex.SpeechSessionOpened, field: session_id}
  - at: 2026-01-05T09:00:03Z
    command: inference.duplex.OpenSpeechSession
    input:
      adapter_id: {$instance: adapter}
      voice: calm
      instructions: answer briefly
      input_format: {encoding: pcm_s16le, sample_rate: 16000, channels: 1}
      output_format: {encoding: pcm_s16le, sample_rate: 16000, channels: 1}
      turn_detection: server_vad
      auto_response: true
      auto_interrupt: false
      tools: []
      idle_limit_ms: 15000
    outcome: opened
    capture: {instance: b, event: inference.duplex.SpeechSessionOpened, field: session_id}
  - at: 2026-01-05T09:00:04Z
    command: inference.duplex.ReceiveSessionEvent
    input: {session_id: {$instance: a}, kind: output_transcript, sequence: 1}
    outcome: received
    capture: {instance: a1, event: inference.duplex.SessionEventReceived, field: received_id}
  - at: 2026-01-05T09:00:05Z
    command: inference.duplex.ReceiveSessionEvent
    input: {session_id: {$instance: a}, kind: usage, sequence: 2}
    outcome: received
    capture: {instance: a2, event: inference.duplex.SessionEventReceived, field: received_id}
  - at: 2026-01-05T09:00:06Z
    command: inference.duplex.ReceiveSessionEvent
    input: {session_id: {$instance: a}, kind: response_end, sequence: 3}
    outcome: received
    capture: {instance: a3, event: inference.duplex.SessionEventReceived, field: received_id}
  - at: 2026-01-05T09:00:07Z
    command: inference.duplex.ReceiveSessionEvent
    input: {session_id: {$instance: b}, kind: usage, sequence: 1}
    outcome: received
    capture: {instance: b1, event: inference.duplex.SessionEventReceived, field: received_id}
  - at: 2026-01-05T09:00:08Z
    command: inference.duplex.DeliverSessionEvent
    input: {received_id: {$instance: a3}}
    outcome: delivered
  - at: 2026-01-05T09:00:09Z
    command: inference.duplex.CloseSpeechSession
    input: {session_id: {$instance: a}}
    outcome: closed
assert:
  - view: inference.duplex.ReceivedSessionEvents
    contains: {received_id: {$instance: a1}, session_id: {$instance: a}, kind: output_transcript, sequence: 1, delivered_before: closed, state: Delivered}
  - view: inference.duplex.ReceivedSessionEvents
    contains: {received_id: {$instance: a2}, session_id: {$instance: a}, kind: usage, sequence: 2, delivered_before: closed, state: Delivered}
  - view: inference.duplex.ReceivedSessionEvents
    contains: {received_id: {$instance: a3}, session_id: {$instance: a}, kind: response_end, sequence: 3, delivered_before: null, state: Delivered}
  - view: inference.duplex.ReceivedSessionEvents
    contains: {received_id: {$instance: b1}, session_id: {$instance: b}, kind: usage, sequence: 1, delivered_before: null, state: Received}
";

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

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root resolves")
}

/// The authored scenarios of `ess/scenarios` and the witness, synthesized together.
fn synthesized() -> AdmittedSuite {
    let root = repository_root();
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_millis();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "adversary12-close-witness-{millis}-{}",
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
        scenarios.join("adversary-close-after-out-of-order-delivery.yaml"),
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
    let text = fs::read_to_string(&suite).expect("suite.json is readable");
    AdmittedSuite::from_json(&text).expect("the synthesized suite is admitted")
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
