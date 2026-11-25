//! Adversary cases for the measures per adapter crate (`quality/adapters.md`,
//! `quality/adapters.ratchet.yaml`, the records of the adapters).
//!
//! Two kinds. A defect planted in memory, one each, where the check of `mod.rs` and
//! `adapters.rs` is asked whether it reports a problem: a latency number that stands in an
//! entry without a baseline, a named test that is no function of its file, and a boundary no
//! assessor declares. And one record read as it is, held against its own results.

use serde_json::Value;

use super::{Documents, change, changed, load, problems};

const ADAPTERS: &str = "quality/adapters.ratchet.yaml";
const CANCEL: &str = "adapters/inference-cartesia/tests/cancel.rs";
const NAMED: &str = "fn a_cancel_does_not_wait_for_the_server(";
const ADAPTER_TESTS: &str = "quality/records/adapter-tests.evidence.json";

/// The last line of the entry `cartesia-live/first-audio` and the heading after it.
const END_OF_ENTRY: &str =
    "- Baseline: none. No run of this command is kept.\n\n## `inference-deepgram`";

fn on_page(documents: &mut Documents, from: &str, to: &str) {
    documents.adapters_page = changed(&documents.adapters_page, from, to);
}

fn in_plans(documents: &mut Documents, from: &str, to: &str) {
    change(&mut documents.specifications, ADAPTERS, from, to);
}

/// The test file of the Cartesia cancel tests with `from` replaced by `to` and `tail` appended.
fn in_cancel(documents: &mut Documents, from: &str, to: &str, tail: &str) {
    let code = documents
        .file(CANCEL)
        .expect("the test file is in the tree");
    let code = format!("{}{tail}", changed(&code, from, to));
    documents.replaced.insert(CANCEL.to_owned(), Some(code));
}

#[track_caller]
fn assert_reported(defect: &str, plant: impl FnOnce(&mut Documents)) {
    let mut documents = load();
    plant(&mut documents);
    let found = problems(&documents);
    assert!(
        !found.is_empty(),
        "the check reports no problem for this planted defect: {defect}"
    );
}

// ---------------------------------------------------------------------------------------------
// A latency number in an entry that says `Baseline: none.`
// ---------------------------------------------------------------------------------------------

/// The acceptance: no latency or connect-time number stands without a named run or file. The
/// check reads the bullets `- <name>: <text>` of an entry. A paragraph under the entry is none.
#[test]
fn a_number_in_a_paragraph_of_an_entry_without_a_baseline_is_a_problem() {
    assert_reported(
        "`cartesia-live/first-audio` says `Baseline: none.` and a paragraph under it says 310 ms",
        |documents| {
            on_page(
                documents,
                END_OF_ENTRY,
                "- Baseline: none. No run of this command is kept.\n\nA run on a wired line gave 310 ms.\n\n## `inference-deepgram`",
            );
        },
    );
}

/// A bullet that does not start with `<name>: ` is no line of the entry for the check.
#[test]
fn a_number_in_a_bullet_without_a_name_of_an_entry_without_a_baseline_is_a_problem() {
    assert_reported(
        "`cartesia-live/first-audio` says `Baseline: none.` and a bullet under it says 310 ms",
        |documents| {
            on_page(
                documents,
                END_OF_ENTRY,
                "- Baseline: none. No run of this command is kept.\n- A run on a wired line gave 310 ms.\n\n## `inference-deepgram`",
            );
        },
    );
}

/// A table of an entry without a baseline is not read.
#[test]
fn a_number_in_a_table_of_an_entry_without_a_baseline_is_a_problem() {
    assert_reported(
        "`cartesia-live/first-audio` says `Baseline: none.` and a table under it says 310 ms",
        |documents| {
            on_page(
                documents,
                END_OF_ENTRY,
                "- Baseline: none. No run of this command is kept.\n\n| Line | First audio |\n|---|---|\n| wired | 310 ms |\n\n## `inference-deepgram`",
            );
        },
    );
}

// ---------------------------------------------------------------------------------------------
// A named test that is no function of its file.
// ---------------------------------------------------------------------------------------------

/// The acceptance: the check goes red when a named test is renamed. Here the test is renamed
/// and a comment keeps the old name.
#[test]
fn a_test_named_only_in_a_comment_of_its_file_is_a_problem() {
    assert_reported(
        "the test of `cartesia-time-bounds/held` was renamed and a comment keeps its old name",
        |documents| {
            in_cancel(
                documents,
                NAMED,
                "fn a_cancel_returns_at_once(",
                "\n// Before the rename: fn a_cancel_does_not_wait_for_the_server().\n",
            );
        },
    );
}

/// The same with the old name in a string of the file.
#[test]
fn a_test_named_only_in_a_string_of_its_file_is_a_problem() {
    assert_reported(
        "the test of `cartesia-time-bounds/held` was renamed and a string keeps its old name",
        |documents| {
            in_cancel(
                documents,
                NAMED,
                "fn a_cancel_returns_at_once(",
                "\nconst _WAS: &str = \"fn a_cancel_does_not_wait_for_the_server()\";\n",
            );
        },
    );
}

// ---------------------------------------------------------------------------------------------
// The clock of a latency measure in the plans.
// ---------------------------------------------------------------------------------------------

/// The 25 latency measures have one output, `duration`, and differ in the plans only by their
/// `boundary`, the part of the time each is about. `printed-duration@1` declares the twenty
/// boundaries (`spec/domains/assessor.yaml:104-108` of the pinned commit: the assessor declares
/// the parts it reports). A measurement that names another is not reported.
#[test]
fn a_boundary_the_assessor_does_not_declare_is_a_problem() {
    assert_reported(
        "`cartesia-live/first-audio` names a boundary that `printed-duration@1` does not declare",
        |documents| {
            in_plans(
                documents,
                "boundary: end_of_utterance_to_first_audio",
                "boundary: somewhere_to_somewhere",
            );
        },
    );
}

// ---------------------------------------------------------------------------------------------
// The record of `adapter-tests`, as it is.
// ---------------------------------------------------------------------------------------------

/// `spec/domains/evidence.yaml:220-229` of the pinned commit: the coverage is "how much of the
/// plan a run executed", in units of the assessor's coverage unit. The plan names eleven
/// packages as its input, and the record has one `tests_run` result for each.
#[test]
fn the_coverage_of_the_adapter_tests_record_counts_the_tests_of_its_packages() {
    let documents = load();
    let record: Value =
        serde_json::from_str(&documents.records[ADAPTER_TESTS]).expect("the record is JSON");
    let run: u64 = record
        .pointer("/evidence/results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|result| result.get("output").and_then(Value::as_str) == Some("tests_run"))
        .filter_map(|result| result.get("value")?.as_str()?.parse::<u64>().ok())
        .sum();
    let executed = record
        .pointer("/evidence/coverage/executed")
        .and_then(Value::as_u64);
    assert_eq!(
        executed,
        Some(run),
        "the coverage counts {executed:?} executed tests, and the `tests_run` results of the \
         packages the plan names sum to {run}"
    );
}
