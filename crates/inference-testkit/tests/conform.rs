//! Runs the synthesized suite of `ess/` against the testkit's conformance target and asks
//! `ess verify conform report` for the verdict.
//!
//! The suite is synthesized at test time into a fresh directory under `CARGO_TARGET_TMPDIR`, run
//! in process by `ess_conformance::Runner`, and its per-scenario statuses written as an
//! `ess-conformance-results/1` document beside it. `ess verify conform report` then admits the
//! suite, checks the results against it and writes the `ess-conformance-report/2` the assertions
//! read. The directory is left in place, red or green, and its path printed: `suite.json` and
//! `report.json` in it are the evidence of the run. `ess` must be on `PATH`, as for `task check`.

mod target;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use ess_conformance::{AdmittedSuite, Runner, Status};
use serde_json::{Value, json};

/// The four `inference.model` scenarios this target answers.
const MODEL_SCENARIOS: [&str; 4] = [
    "inference.model.DeclareModel/outcome/declared",
    "inference.model.DeclareAdapter/outcome/declared",
    "inference.model.DeclareAdapter/outcome/refused",
    "inference.model.DeclareAdapter/outcome/unknown-model",
];

/// The eight `inference.stream` scenarios this target answers without a run.
const STREAM_SCENARIOS: [&str; 8] = [
    "inference.stream.AcknowledgePlayback/outcome/wrong-state",
    "inference.stream.CancelGeneration/outcome/wrong-state",
    "inference.stream.CommitSegment/outcome/wrong-state",
    "inference.stream.CompleteGeneration/outcome/wrong-state",
    "inference.stream.CompleteSynthesis/outcome/wrong-state",
    "inference.stream.DiscardSegment/outcome/wrong-state",
    "inference.stream.QueueForSynthesis/outcome/wrong-state",
    "inference.stream.SupersedeGeneration/outcome/wrong-state",
];

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root resolves")
}

/// A directory no earlier run wrote, so that every output of `ess` is a new file.
fn fresh_directory() -> PathBuf {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_millis();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("conform-{millis}-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("the run directory is created");
    dir
}

fn ess(root: &Path, args: &[&str]) {
    let output = Command::new("ess")
        .args(args)
        .current_dir(root)
        .output()
        .expect("`ess` runs; it must be on PATH");
    assert!(
        output.status.success(),
        "`ess {}` exited {}\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn path_arg(path: &Path) -> &str {
    path.to_str().expect("the run directory is valid UTF-8")
}

/// The scenario ids `ess/SKIPPED.md` names, each under the `## <domain>` section it belongs to.
fn skipped_ids(root: &Path) -> BTreeSet<String> {
    let text = fs::read_to_string(root.join("ess/SKIPPED.md")).expect("ess/SKIPPED.md is readable");
    let mut section: Option<&str> = None;
    let mut ids = BTreeSet::new();
    for line in text.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            section = Some(heading.trim());
            continue;
        }
        let Some(entry) = line.strip_prefix("- ") else {
            continue;
        };
        let (id, reason) = entry
            .split_once(": ")
            .unwrap_or_else(|| panic!("`{line}` is `- <scenario id>: <reason>`"));
        assert!(
            !reason.trim().is_empty(),
            "`{id}` in ess/SKIPPED.md names no story or epic"
        );
        let domain = section.unwrap_or_else(|| panic!("`{id}` stands under no `## <domain>`"));
        assert!(
            id.starts_with(&format!("{domain}.")) || id.starts_with(&format!("{domain}/")),
            "`{id}` stands under `## {domain}`, the section of another domain"
        );
        assert!(
            ids.insert(id.to_owned()),
            "`{id}` is named twice in ess/SKIPPED.md"
        );
    }
    ids
}

fn ids(report: &Value, status: &str) -> BTreeSet<String> {
    report["outcomes"][status]
        .as_array()
        .unwrap_or_else(|| panic!("report.json carries `outcomes.{status}` as a list"))
        .iter()
        .map(|id| id.as_str().expect("a scenario id is a string").to_owned())
        .collect()
}

#[test]
fn the_suite_runs_against_the_testkit_target() {
    let root = repository_root();
    let dir = fresh_directory();
    println!("conformance run directory: {}", dir.display());
    let suite_path = dir.join("suite.json");
    let results_path = dir.join("results.json");
    let report_path = dir.join("report.json");

    ess(
        &root,
        &[
            "verify",
            "conform",
            "synthesize",
            "--path",
            "ess",
            "--scenarios",
            "ess/scenarios",
            "--out",
            path_arg(&suite_path),
        ],
    );

    let original = fs::read_to_string(&suite_path).expect("suite.json is readable");
    let admitted = AdmittedSuite::from_json(&original)
        .unwrap_or_else(|error| panic!("the synthesized suite is admitted: {error:?}"));
    let run =
        Runner::for_suite(admitted.suite()).run_admitted(&admitted, &target::Target::default());

    let results: Vec<Value> = run
        .scenarios
        .iter()
        .map(|scenario| {
            let mut entry = json!({
                "scenario_id": scenario.scenario.to_string(),
                "status": scenario.status.as_str(),
            });
            if scenario.status != Status::Passed
                && let Some(diagnostic) = scenario.diagnostics().next()
            {
                entry["message"] = Value::String(diagnostic.to_string());
            }
            entry
        })
        .collect();
    let document = json!({
        "format": "ess-conformance-results/1",
        "completed_at": run.completed_at.epoch_millis(),
        "suite_digest": run.suite_digest(),
        "results": results,
    });
    fs::write(
        &results_path,
        serde_json::to_string_pretty(&document).expect("the results serialize"),
    )
    .expect("results.json is written");

    ess(
        &root,
        &[
            "verify",
            "conform",
            "report",
            "--suite",
            path_arg(&suite_path),
            "--results",
            path_arg(&results_path),
            "--report-out",
            path_arg(&report_path),
            "--implementation",
            "inference-testkit",
        ],
    );

    let report: Value =
        serde_json::from_str(&fs::read_to_string(&report_path).expect("report.json is readable"))
            .expect("report.json is JSON");
    assert_eq!(report["format"], "ess-conformance-report/2");
    println!("counts: {}", report["counts"]);
    println!("conformance run directory: {}", dir.display());

    let messages =
        |wanted: &BTreeSet<String>| -> Vec<String> { results_messages(&document, wanted) };
    let failed = ids(&report, "failed");
    assert!(failed.is_empty(), "failed: {:#?}", messages(&failed));
    let errored = ids(&report, "error");
    assert!(errored.is_empty(), "error: {:#?}", messages(&errored));

    let passed = ids(&report, "passed");
    for id in MODEL_SCENARIOS.into_iter().chain(STREAM_SCENARIOS) {
        assert!(
            passed.contains(id),
            "`{id}` did not pass: {:#?}",
            messages(&BTreeSet::from([id.to_owned()]))
        );
    }

    let unsupported = ids(&report, "unsupported");
    let skipped = skipped_ids(&root);
    let unnamed: Vec<_> = unsupported.difference(&skipped).collect();
    let stale: Vec<_> = skipped.difference(&unsupported).collect();
    assert!(
        unnamed.is_empty() && stale.is_empty(),
        "the unsupported ids differ from ess/SKIPPED.md\nunsupported and not named there: {unnamed:#?}\nnamed there and not unsupported: {stale:#?}"
    );
}

fn results_messages(document: &Value, wanted: &BTreeSet<String>) -> Vec<String> {
    document["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| {
            entry["scenario_id"]
                .as_str()
                .is_some_and(|id| wanted.contains(id))
        })
        .map(|entry| {
            format!(
                "{}: {} {}",
                entry["scenario_id"], entry["status"], entry["message"]
            )
        })
        .collect()
}
