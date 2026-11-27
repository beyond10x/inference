//! Adversary cases for the quality documents (`ratchet.toml`, `quality/`).
//!
//! Three kinds. A document rule of the specification the documents are written against, held on
//! the files as they are. The scope step of `.github/workflows/check.yml`, asked whether a pull
//! request that changes only the page the check reads runs the check. And defects planted in
//! memory, one each, where the check of `mod.rs` is asked whether it reports a problem: each is
//! one that `quality/README.md` says the check fails on, or one where the page and the documents
//! disagree.

use std::collections::BTreeMap;

use serde_json::Value;

use super::{Documents, entries, load, problems, specification, text};

const GATE: &str = "quality/gate.ratchet.yaml";
const LINT_RECORD: &str = "quality/records/lint.evidence.json";

/// `source` with its one occurrence of `from` replaced by `to`.
fn replaced(source: &str, from: &str, to: &str) -> String {
    assert_eq!(
        source.matches(from).count(),
        1,
        "the text holds {from:?} exactly once"
    );
    source.replacen(from, to, 1)
}

fn change(files: &mut BTreeMap<String, String>, path: &str, from: &str, to: &str) {
    let source = files
        .get_mut(path)
        .unwrap_or_else(|| panic!("the documents hold {path}"));
    *source = replaced(source, from, to);
}

/// The problems the check reports for `documents` and not for the documents of the tree.
fn reported(documents: &Documents) -> Vec<String> {
    let clean = problems(&load());
    problems(documents)
        .into_iter()
        .filter(|problem| !clean.contains(problem))
        .collect()
}

#[track_caller]
fn assert_reported(defect: &str, documents: &Documents) {
    let found = reported(documents);
    assert!(
        !found.is_empty(),
        "the check reports no problem for this planted defect: {defect}"
    );
}

// ---------------------------------------------------------------------------------------------
// The documents as they are, against the specification at the pinned commit.
// ---------------------------------------------------------------------------------------------

/// `spec/domains/specification.yaml:479-485` and `docs/design.md:94` of the pinned commit:
/// "`ratchet validate` reports a sampled measurement whose plan declares none" (no conditions).
#[test]
fn q1_a_plan_with_a_sampled_measurement_declares_its_conditions() {
    let documents = load();
    let (maps, _) = specification(&documents, &mut Vec::new());
    let without: Vec<&String> = maps
        .get("assessments")
        .into_iter()
        .flatten()
        .filter(|(_, plan)| {
            entries(plan, "measurements")
                .any(|(_, measurement)| text(measurement, "kind") == Some("sampled"))
        })
        .filter(|(_, plan)| {
            !plan
                .pointer("/observation/conditions")
                .and_then(Value::as_object)
                .is_some_and(|conditions| !conditions.is_empty())
        })
        .map(|(id, _)| id)
        .collect();
    assert!(
        without.is_empty(),
        "these plans hold a sampled measurement and declare no observation conditions: {without:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// The pull request that changes only the page the check reads.
// ---------------------------------------------------------------------------------------------

/// The check reads `quality/README.md` (`mod.rs`, `load`). A pull request that changes only that
/// file has to run the step the check is part of, or the check first runs on `main`.
#[test]
fn q1_a_pull_request_that_changes_only_the_quality_page_runs_the_check() {
    assert_eq!(
        crate::gate("pull_request", &["quality/README.md"]),
        "full",
        "the scope step of check.yml skips the build and the tests for quality/README.md, \
         which the quality check reads"
    );
}

// ---------------------------------------------------------------------------------------------
// Defects planted in memory, one each. The check is asked whether it reports a problem.
// ---------------------------------------------------------------------------------------------

/// The page: the check fails "when a unit differs from the assessor's".
#[test]
fn q1_planted_a_condition_unit_differs_from_the_declaration() {
    let mut documents = load();
    change(
        &mut documents.specifications,
        GATE,
        "condition: {operator: lt, value: 60, unit: s}",
        "condition: {operator: lt, value: 60, unit: ms}",
    );
    assert_reported(
        "the condition of test-budget is in ms, the assessor's output and the measurement in s",
        &documents,
    );
}

/// The page: the check fails "when a unit differs from the assessor's".
#[test]
fn q1_planted_a_record_unit_differs_from_the_declaration() {
    let mut documents = load();
    change(
        &mut documents.records,
        "quality/records/test-run-step.evidence.json",
        "\"unit\": \"s\"",
        "\"unit\": \"ms\"",
    );
    assert_reported(
        "the record of test-run-step states 101 ms, the assessor's output is in s",
        &documents,
    );
}

/// The page: the check fails when "a record names something that is not declared".
#[test]
fn q1_planted_a_record_names_an_assessor_without_a_declaration() {
    let mut documents = load();
    change(
        &mut documents.records,
        LINT_RECORD,
        "\"assessor\": \"command-run@1\"",
        "\"assessor\": \"command-run@2\"",
    );
    assert_reported(
        "the record of lint names command-run@2, which has no declaration",
        &documents,
    );
}

/// The page: the check fails when "a record names something that is not declared".
#[test]
fn q1_planted_a_record_names_an_output_its_assessor_does_not_declare() {
    let mut documents = load();
    change(
        &mut documents.records,
        LINT_RECORD,
        "\"output\": \"exit_zero\"",
        "\"output\": \"exit_code\"",
    );
    assert_reported(
        "the record of lint holds a result of the output exit_code, which command-run@1 does not declare",
        &documents,
    );
}

/// The page: the check fails when "an assessment ... names something that is not declared".
#[test]
fn q1_planted_an_assessment_names_an_undeclared_subject() {
    let mut documents = load();
    change(
        &mut documents.specifications,
        GATE,
        "    subject: native-archive\n",
        "    subject: native-archives\n",
    );
    assert_reported(
        "the assessment native-archive names the subject native-archives, which is not declared",
        &documents,
    );
}

/// A name that points at nothing: the list of a gate.
#[test]
fn q1_planted_a_gate_requires_an_undeclared_requirement() {
    let mut documents = load();
    change(
        &mut documents.specifications,
        GATE,
        "      - formatted\n",
        "      - formatted-renamed\n",
    );
    assert_reported(
        "the gate check requires formatted-renamed, which is no requirement",
        &documents,
    );
}

/// The page and the documents disagree: the value.
#[test]
fn q1_planted_a_record_value_differs_from_the_baseline_of_the_page() {
    let mut documents = load();
    change(
        &mut documents.records,
        "quality/records/workspace-tests.evidence.json",
        "\"value\": \"8.011\"",
        "\"value\": \"9.999\"",
    );
    assert_reported(
        "the record of workspace-tests holds 9.999 s, the page states the baseline 8.011 s",
        &documents,
    );
}

/// The page and the documents disagree: an entry without its baseline and its run.
#[test]
fn q1_planted_an_entry_of_the_page_loses_its_baseline() {
    let mut documents = load();
    documents.readme = replaced(
        &documents.readme,
        "- Baseline: 101 s. Check run, step `task test-run`.\n",
        "",
    );
    assert_reported(
        "the entry test-run-step/run-time of the page has no baseline and names no run",
        &documents,
    );
}

/// The page and the documents disagree: the bound.
#[test]
fn q1_planted_a_bound_of_the_specification_differs_from_the_page_and_the_taskfile() {
    let mut documents = load();
    change(
        &mut documents.specifications,
        GATE,
        "condition: {operator: lt, value: 120, unit: s}",
        "condition: {operator: lt, value: 240, unit: s}",
    );
    assert_reported(
        "test-step-budget is under 240 s, the page says under 120 s and Taskfile.yml ends the step at 120 s",
        &documents,
    );
}
