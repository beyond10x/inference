//! Adversary pass 1 against the `inference.tools` module of the conformance target, through the
//! runner: the synthesized suite witnesses the provisional-input barrier, the default-path
//! proposal and the declaration with one value each (a read-only tool, `requires_confirmation:
//! true`) and checks no field of `ProvisionalInputBarrier`. These cases run the same scenarios with
//! the other values, so a target that drops or fixes one of them fails here.
#![cfg(feature = "run")]

mod target;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use ess_conformance::{AdmittedSuite, Runner, Status};
use serde_json::{Value, json};

use target::Target;

/// The synthesized suite of `ess/`, synthesized into a fresh directory of this test.
fn synthesized_suite(case: &str) -> Value {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root resolves");
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_millis();
    let dir: PathBuf = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "adversary1-tools-{case}-{millis}-{}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("the directory is created");
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
        .expect("`ess` runs");
    assert!(output.status.success(), "ess synthesize: {output:?}");
    serde_json::from_str(&fs::read_to_string(&suite).expect("suite.json")).expect("JSON")
}

/// Runs the scenario `id` of `suite` with its steps edited by `edit`, and returns the status and
/// the messages the runner recorded.
fn run_edited(suite: &Value, id: &str, edit: impl Fn(&mut Vec<Value>)) -> (Status, String) {
    let mut one = suite.clone();
    let mut kept = one["scenarios"][id].clone();
    assert!(kept.is_object(), "the suite carries `{id}`");
    edit(kept["steps"].as_array_mut().expect("steps"));
    one["scenarios"] = json!({ id: kept });
    let admitted = AdmittedSuite::from_json(&one.to_string())
        .unwrap_or_else(|error| panic!("the edited suite is admitted: {error:?}"));
    let run = Runner::for_suite(admitted.suite()).run_admitted(&admitted, &Target::default());
    assert_eq!(run.scenarios.len(), 1);
    let scenario = &run.scenarios[0];
    (scenario.status, format!("{scenario:?}"))
}

/// The index of the first `execute_command` step of `command` whose input field `field` holds
/// the literal `value`.
fn command_step(steps: &[Value], command: &str, field: &str, value: &Value) -> usize {
    steps
        .iter()
        .position(|step| {
            step["step"] == "execute_command"
                && step["command"] == command
                && step["input"][field]["value"] == *value
        })
        .unwrap_or_else(|| panic!("a `{command}` step with {field} = {value}"))
}

/// The instance a `capture_instance` step after `after` binds from `event`.
fn captured_after(steps: &[Value], after: usize, event: &str) -> String {
    steps[after..]
        .iter()
        .find(|step| step["step"] == "capture_instance" && step["event"] == event)
        .and_then(|step| step["instance"].as_str())
        .unwrap_or_else(|| panic!("a capture of `{event}` after step {after}"))
        .to_owned()
}

/// The barred proposal of the suite rests on a Hypothesis span with a read-only tool, and its
/// `expect_error` names no field. With the tool declared an idempotent or non-idempotent write,
/// §23.1 test 5 through the target: the proposal is barred and the barrier carries that class.
#[test]
fn a_side_effecting_proposal_on_a_hypothesis_is_barred_with_its_class_through_the_target() {
    let suite = synthesized_suite("barred");
    let id = "inference.tools.ProposeToolCall/outcome/barred";
    let (control, messages) = run_edited(&suite, id, |_| {});
    assert_eq!(control, Status::Passed, "the unedited scenario: {messages}");

    for class in ["idempotent_write", "non_idempotent_write", "read_only"] {
        let (status, messages) = run_edited(&suite, id, |steps| {
            let declare = steps
                .iter()
                .position(|step| {
                    step["step"] == "execute_command"
                        && step["command"] == "inference.tools.DeclareTool"
                })
                .expect("the scenario declares a tool");
            steps[declare]["input"]["side_effect"] = json!({"kind": "literal", "value": class});
            let error = steps
                .iter()
                .position(|step| {
                    step["step"] == "expect_error"
                        && step["error"] == "inference.tools.ProvisionalInputBarrier"
                })
                .expect("the scenario expects the barrier");
            steps[error]["fields"] = json!({ "side_effect": class });
        });
        assert_eq!(status, Status::Passed, "{class}: {messages}");
    }
}

/// The suite's default-path proposal on a Committed span is of a read-only tool. With the
/// idempotent write the same scenario declares, the proposal is admitted and the call carries the
/// tool's class.
#[test]
fn a_side_effecting_proposal_on_a_committed_span_is_proposed_with_its_class_through_the_target() {
    let suite = synthesized_suite("proposed");
    let id = "inference.tools.ProposeToolCall/outcome/proposed";
    let (status, messages) = run_edited(&suite, id, |steps| {
        let write = command_step(
            steps,
            "inference.tools.DeclareTool",
            "side_effect",
            &json!("idempotent_write"),
        );
        let tool = captured_after(steps, write, "inference.tools.ToolDeclared");
        let propose = steps
            .iter()
            .position(|step| {
                step["step"] == "execute_command"
                    && step["command"] == "inference.tools.ProposeToolCall"
            })
            .expect("the scenario proposes");
        steps[propose]["input"]["tool_id"] = json!({"kind": "instance", "instance": tool});
        for step in steps.iter_mut() {
            if step["step"] == "expect_event_values"
                && step["event"] == "inference.tools.ToolCallProposed"
            {
                step["payload"]["tool_id"] = json!({"kind": "instance", "instance": tool});
            }
            if step["step"] == "expect_view" && step["view"] == "inference.tools.ToolCalls" {
                let fields = &mut step["expectation"]["fields"];
                fields["tool_id"] = json!({"kind": "instance", "instance": tool});
                fields["side_effect"] = json!({"kind": "literal", "value": "idempotent_write"});
            }
        }
    });
    assert_eq!(status, Status::Passed, "{messages}");
}

/// The suite declares every tool it reads back with `requires_confirmation: true`; declared
/// `false`, the `inference.tools.Tools` row carries `false`.
#[test]
fn a_tool_declared_without_confirmation_reads_back_false_through_the_target() {
    let suite = synthesized_suite("declared");
    let id = "inference.tools.DeclareTool/outcome/declared";
    let (status, messages) = run_edited(&suite, id, |steps| {
        for step in steps.iter_mut() {
            if step["step"] == "execute_command" && step["command"] == "inference.tools.DeclareTool"
            {
                step["input"]["requires_confirmation"] = json!({"kind": "literal", "value": false});
            }
            if step["step"] == "expect_view" && step["view"] == "inference.tools.Tools" {
                step["expectation"]["fields"]["requires_confirmation"] =
                    json!({"kind": "literal", "value": false});
            }
        }
    });
    assert_eq!(status, Status::Passed, "{messages}");
}
