//! The records a check run writes: `task quality-records` takes the assessments it writes a
//! record for from `inference-docs junit-assessments`, which reads them from the plan files, and
//! `task check` runs it after `task test-run`, whose JUnit report it reads.

use std::collections::BTreeSet;
use std::process::Command;

use serde_json::Value;

use super::{entries, load, text, yaml};
use crate::root;

/// The assessors whose records are read from the JUnit report of cargo-nextest.
const READ_FROM_THE_REPORT: [&str; 4] = [
    "nextest-run@1",
    "package-tests@1",
    "named-tests@1",
    "capability-record@1",
];
/// The command whose JUnit report `task quality-records` reads.
const TEST_RUN: &str = "task test-run";

/// One line the program printed: the plan, the assessment and the kind of its target.
type Listed = (String, String, String);

/// What `inference-docs junit-assessments` prints for the tree, held to exit 0 and print
/// nothing on stderr.
fn listed() -> Vec<Listed> {
    let output = Command::new(env!("CARGO_BIN_EXE_inference-docs"))
        .arg("junit-assessments")
        .arg("--root")
        .arg(root())
        .output()
        .expect("the program runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
    assert!(stderr.is_empty(), "stderr: {stderr}");
    String::from_utf8(output.stdout)
        .expect("the list is UTF-8")
        .lines()
        .map(|line| {
            let fields: Vec<&str> = line.split(' ').collect();
            assert_eq!(
                fields.len(),
                3,
                "a line is plan, assessment, kind: {line:?}"
            );
            (
                fields[0].to_owned(),
                fields[1].to_owned(),
                fields[2].to_owned(),
            )
        })
        .collect()
}

/// Every assessment of the plan files whose assessor reads the JUnit report, with the plan it
/// stands in, the kind of its target and the command of its observation.
fn read_from_the_report() -> Vec<(Listed, String)> {
    let documents = load();
    let mut problems = Vec::new();
    let mut found = Vec::new();
    for (path, source) in &documents.specifications {
        let plan = path
            .strip_prefix("quality/")
            .and_then(|name| name.strip_suffix(".ratchet.yaml"))
            .expect("a plan file is quality/<plan>.ratchet.yaml");
        let Some(specification) = yaml(path, source, &mut problems) else {
            continue;
        };
        for (id, assessment) in entries(&specification, "assessments") {
            let assessor = text(assessment, "assessor").unwrap_or_default();
            if !READ_FROM_THE_REPORT.contains(&assessor) {
                continue;
            }
            let kind = assessment
                .pointer("/target/kind")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let command = assessment
                .pointer("/observation/command")
                .and_then(Value::as_str)
                .unwrap_or_default();
            found.push((
                (plan.to_owned(), id.clone(), kind.to_owned()),
                command.to_owned(),
            ));
        }
    }
    assert!(problems.is_empty(), "{problems:?}");
    found
}

#[test]
fn the_list_of_the_records_task_is_every_assessment_that_reads_the_report_of_task_test_run() {
    let listed = listed();
    let unique: BTreeSet<&Listed> = listed.iter().collect();
    assert_eq!(
        unique.len(),
        listed.len(),
        "an assessment stands once: {listed:?}"
    );
    let all = read_from_the_report();
    let expected: BTreeSet<&Listed> = all
        .iter()
        .filter(|(_, command)| command == TEST_RUN)
        .map(|(assessment, _)| assessment)
        .collect();
    assert_eq!(unique, expected);
    let other: Vec<&str> = all
        .iter()
        .filter(|(_, command)| command != TEST_RUN)
        .map(|((_, id, _), _)| id.as_str())
        .collect();
    assert_eq!(
        other,
        ["slow-lane"],
        "of the assessments read from a JUnit report, only the slow lane reads another run's"
    );
    for (plan, id, kind) in [
        ("gate", "workspace-tests", "revision"),
        ("adapters", "adapter-tests", "revision"),
        ("gate", "release-statuses", "revision"),
        ("adapters", "sherpa-onnx-stt-record", "document"),
    ] {
        let line = (plan.to_owned(), id.to_owned(), kind.to_owned());
        assert!(unique.contains(&line), "{line:?} is not in {listed:?}");
    }
}

#[test]
fn the_records_task_takes_its_list_from_the_program_and_check_runs_it_after_test_run() {
    let documents = load();
    let mut problems = Vec::new();
    let taskfile = yaml("Taskfile.yml", &documents.taskfile, &mut problems).expect("it parses");
    let commands = |task: &str| -> Vec<String> {
        taskfile
            .pointer(&format!("/tasks/{task}/cmds"))
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("Taskfile.yml has the task {task}"))
            .iter()
            .map(|command| match command {
                Value::String(line) => line.clone(),
                other => text(other, "task").unwrap_or_default().to_owned(),
            })
            .collect()
    };
    let check = commands("check");
    let at = |task: &str| check.iter().position(|command| command == task);
    assert!(
        at("test-run").is_some_and(|test_run| at("quality-records") > Some(test_run)),
        "task check runs quality-records after test-run: {check:?}"
    );
    let script = commands("quality-records").join("\n");
    for needed in [
        "cargo run -q -p inference-docs -- junit-assessments",
        "cargo run -q -p inference-docs -- assess",
        "target/nextest/default/junit.xml",
        "target/quality-records/",
    ] {
        assert!(script.contains(needed), "the task does not hold {needed:?}");
    }
    let words: BTreeSet<&str> = script
        .split(|character: char| !(character.is_ascii_alphanumeric() || character == '-'))
        .collect();
    for (_, id, _) in listed() {
        assert!(
            !words.contains(id.as_str()),
            "the task names the assessment {id}; it takes the list from the program"
        );
    }
}
