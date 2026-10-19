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

/// The eleven `inference.stream` scenarios this target answers without a run.
const STREAM_SCENARIOS: [&str; 11] = [
    "inference.stream.AcknowledgePlayback/outcome/wrong-state",
    "inference.stream.CancelGeneration/outcome/wrong-state",
    "inference.stream.CommitSegment/outcome/wrong-state",
    "inference.stream.CommitSpan/outcome/wrong-state",
    "inference.stream.CompleteGeneration/outcome/wrong-state",
    "inference.stream.CompleteSynthesis/outcome/wrong-state",
    "inference.stream.CompleteTurn/outcome/wrong-state",
    "inference.stream.DiscardSegment/outcome/wrong-state",
    "inference.stream.QueueForSynthesis/outcome/wrong-state",
    "inference.stream.ReviseSpan/outcome/no-such-span",
    "inference.stream.SupersedeGeneration/outcome/wrong-state",
];

/// The four `inference.policy` scenarios this target answers.
const POLICY_SCENARIOS: [&str; 4] = [
    "inference.policy.DeclareProfile/outcome/declared",
    "inference.policy.DeclareProfile/outcome/offline-admits-cluster",
    "inference.policy.DeclareProfile/outcome/remote-not-opted-in",
    "inference.policy.DeclareProfile/outcome/wrong-egress",
];

/// The eleven `inference.graph` declaration scenarios this target answers without validating a
/// pipeline.
const GRAPH_SCENARIOS: [&str; 11] = [
    "inference.graph.AddNode/outcome/unknown-pipeline",
    "inference.graph.AddPort/outcome/added",
    "inference.graph.AddPort/outcome/unknown-node",
    "inference.graph.AddPort/outcome/unknown-pipeline",
    "inference.graph.BindNode/outcome/unknown-pipeline",
    "inference.graph.ConnectPorts/outcome/source-not-output",
    "inference.graph.ConnectPorts/outcome/unbounded",
    "inference.graph.ConnectPorts/outcome/unknown-pipeline",
    "inference.graph.ConnectPorts/outcome/unknown-source",
    "inference.graph.DeclarePipeline/outcome/declared",
    "inference.graph.DeclarePipeline/outcome/unknown-profile",
];

/// The fifteen `inference.graph` scenarios this target answers by validating a pipeline: the
/// five of `ValidatePipeline` and the ten declaration scenarios whose arrangement validates one.
const GRAPH_VALIDATION_SCENARIOS: [&str; 15] = [
    "inference.graph.Pipeline/state/Validated/refuses/inference.graph.ValidatePipeline",
    "inference.graph.Pipeline/transition/validate/by/inference.graph.ValidatePipeline/validated",
    "inference.graph.ValidatePipeline/outcome/rejected",
    "inference.graph.ValidatePipeline/outcome/validated",
    "inference.graph.ValidatePipeline/outcome/wrong-state",
    "inference.graph.AddNode/outcome/added",
    "inference.graph.AddNode/outcome/not-declared",
    "inference.graph.AddPort/outcome/not-declared",
    "inference.graph.BindNode/outcome/bound",
    "inference.graph.BindNode/outcome/not-declared",
    "inference.graph.BindNode/outcome/unknown-adapter",
    "inference.graph.BindNode/outcome/unknown-node",
    "inference.graph.ConnectPorts/outcome/connected",
    "inference.graph.ConnectPorts/outcome/not-declared",
    "inference.graph.Edge/invariant/after/inference.graph.ConnectPorts/connected",
];

/// The nine `inference.graph` scenarios this target answers by planning a pipeline: the five of
/// `PlanPipeline`, its transition and the three state refusals that reach Planned or start from
/// Declared.
const GRAPH_PLANNING_SCENARIOS: [&str; 9] = [
    "inference.graph.PlanPipeline/outcome/locality-denied",
    "inference.graph.PlanPipeline/outcome/mismatch",
    "inference.graph.PlanPipeline/outcome/planned",
    "inference.graph.PlanPipeline/outcome/unbound",
    "inference.graph.PlanPipeline/outcome/wrong-state",
    "inference.graph.Pipeline/transition/plan/by/inference.graph.PlanPipeline/planned",
    "inference.graph.Pipeline/state/Declared/refuses/inference.graph.PlanPipeline",
    "inference.graph.Pipeline/state/Planned/refuses/inference.graph.PlanPipeline",
    "inference.graph.Pipeline/state/Planned/refuses/inference.graph.ValidatePipeline",
];

/// The 72 scenarios this target answers by running a pipeline: the 70 synthesized `inference.graph`
/// scenarios of the eight Run commands and `DescribeNodeInstance`, the authored Draining-to-Failed
/// one, and the authored `CancelRun` cascade over a run's generation and segments.
const RUN_SCENARIOS: [&str; 72] = [
    "inference.graph.ActivateRun/outcome/active",
    "inference.graph.ActivateRun/outcome/wrong-state",
    "inference.graph/authored/draining-run-fails",
    "inference.graph.CancelRun/outcome/wrong-state",
    "inference.graph.CompleteRun/outcome/completed",
    "inference.graph.CompleteRun/outcome/wrong-state",
    "inference.graph.CreateRun/outcome/created",
    "inference.graph.CreateRun/outcome/not-planned",
    "inference.graph.CreateRun/outcome/unknown-pipeline",
    "inference.graph.DescribeNodeInstance/outcome/described",
    "inference.graph.DescribeNodeInstance/outcome/run-not-running",
    "inference.graph.DescribeNodeInstance/outcome/unknown-node",
    "inference.graph.DescribeNodeInstance/outcome/unknown-run",
    "inference.graph.DrainRun/outcome/draining",
    "inference.graph.DrainRun/outcome/wrong-state",
    "inference.graph.FailRun/outcome/failed",
    "inference.graph.FailRun/outcome/wrong-state",
    "inference.graph.PrepareRun/outcome/preparing",
    "inference.graph.PrepareRun/outcome/refused",
    "inference.graph.PrepareRun/outcome/wrong-state",
    "inference.graph.Run/state/Active/refuses/inference.graph.ActivateRun",
    "inference.graph.Run/state/Active/refuses/inference.graph.CompleteRun",
    "inference.graph.Run/state/Active/refuses/inference.graph.PrepareRun",
    "inference.graph.Run/state/Active/refuses/inference.graph.ValidateRun",
    "inference.graph.Run/state/Cancelled/refuses/inference.graph.ActivateRun",
    "inference.graph.Run/state/Cancelled/refuses/inference.graph.CancelRun",
    "inference.graph.Run/state/Cancelled/refuses/inference.graph.CompleteRun",
    "inference.graph.Run/state/Cancelled/refuses/inference.graph.DrainRun",
    "inference.graph.Run/state/Cancelled/refuses/inference.graph.FailRun",
    "inference.graph.Run/state/Cancelled/refuses/inference.graph.PrepareRun",
    "inference.graph.Run/state/Cancelled/refuses/inference.graph.ValidateRun",
    "inference.graph.Run/state/Completed/refuses/inference.graph.ActivateRun",
    "inference.graph.Run/state/Completed/refuses/inference.graph.CancelRun",
    "inference.graph.Run/state/Completed/refuses/inference.graph.CompleteRun",
    "inference.graph.Run/state/Completed/refuses/inference.graph.DrainRun",
    "inference.graph.Run/state/Completed/refuses/inference.graph.FailRun",
    "inference.graph.Run/state/Completed/refuses/inference.graph.PrepareRun",
    "inference.graph.Run/state/Completed/refuses/inference.graph.ValidateRun",
    "inference.graph.Run/state/Created/refuses/inference.graph.ActivateRun",
    "inference.graph.Run/state/Created/refuses/inference.graph.CompleteRun",
    "inference.graph.Run/state/Created/refuses/inference.graph.DrainRun",
    "inference.graph.Run/state/Created/refuses/inference.graph.FailRun",
    "inference.graph.Run/state/Created/refuses/inference.graph.PrepareRun",
    "inference.graph.Run/state/Draining/refuses/inference.graph.ActivateRun",
    "inference.graph.Run/state/Draining/refuses/inference.graph.DrainRun",
    "inference.graph.Run/state/Draining/refuses/inference.graph.PrepareRun",
    "inference.graph.Run/state/Draining/refuses/inference.graph.ValidateRun",
    "inference.graph.Run/state/Failed/refuses/inference.graph.ActivateRun",
    "inference.graph.Run/state/Failed/refuses/inference.graph.CancelRun",
    "inference.graph.Run/state/Failed/refuses/inference.graph.CompleteRun",
    "inference.graph.Run/state/Failed/refuses/inference.graph.DrainRun",
    "inference.graph.Run/state/Failed/refuses/inference.graph.FailRun",
    "inference.graph.Run/state/Failed/refuses/inference.graph.PrepareRun",
    "inference.graph.Run/state/Failed/refuses/inference.graph.ValidateRun",
    "inference.graph.Run/state/Preparing/refuses/inference.graph.CompleteRun",
    "inference.graph.Run/state/Preparing/refuses/inference.graph.DrainRun",
    "inference.graph.Run/state/Preparing/refuses/inference.graph.PrepareRun",
    "inference.graph.Run/state/Preparing/refuses/inference.graph.ValidateRun",
    "inference.graph.Run/state/Validating/refuses/inference.graph.ActivateRun",
    "inference.graph.Run/state/Validating/refuses/inference.graph.CompleteRun",
    "inference.graph.Run/state/Validating/refuses/inference.graph.DrainRun",
    "inference.graph.Run/state/Validating/refuses/inference.graph.ValidateRun",
    "inference.graph.Run/transition/activate/by/inference.graph.ActivateRun/active",
    "inference.graph.Run/transition/cancel/by/inference.graph.CancelRun/cancelled",
    "inference.graph.Run/transition/complete/by/inference.graph.CompleteRun/completed",
    "inference.graph.Run/transition/drain/by/inference.graph.DrainRun/draining",
    "inference.graph.Run/transition/fail/by/inference.graph.FailRun/failed",
    "inference.graph.Run/transition/prepare/by/inference.graph.PrepareRun/preparing",
    "inference.graph.Run/transition/validate/by/inference.graph.ValidateRun/validating",
    "inference.graph.ValidateRun/outcome/validating",
    "inference.graph.ValidateRun/outcome/wrong-state",
    "inference.stream/authored/cancel-run-discards-unplayed-segments",
];

/// The 71 `inference.stream` scenarios this target answers once a run is answered: their
/// arrangement creates and activates a Run, or their command reads the run and node stores.
const RUN_ROUTED_SCENARIOS: [&str; 71] = [
    "inference.stream.AcknowledgePlayback/outcome/played",
    "inference.stream/authored/cancel-generation-discards-unplayed-segments",
    "inference.stream/authored/supersede-discards-unplayed-segments",
    "inference.stream.CommitSegment/outcome/committed",
    "inference.stream.CommitSpan/outcome/committed",
    "inference.stream.CompleteGeneration/outcome/completed",
    "inference.stream.CompleteSynthesis/outcome/synthesized",
    "inference.stream.CompleteTurn/outcome/completed",
    "inference.stream.DiscardSegment/outcome/discarded",
    "inference.stream.EmitHypothesis/outcome/emitted",
    "inference.stream.EmitHypothesis/outcome/run-not-active",
    "inference.stream.EmitHypothesis/outcome/turn-not-open",
    "inference.stream.EmitHypothesis/outcome/unknown-run",
    "inference.stream.EmitHypothesis/outcome/unknown-turn",
    "inference.stream.EmitTextDelta/outcome/emitted",
    "inference.stream.EmitTextDelta/outcome/stale-generation",
    "inference.stream.EmitTextDelta/outcome/unknown-generation",
    "inference.stream.Generation/state/Cancelled/refuses/inference.stream.CancelGeneration",
    "inference.stream.Generation/state/Cancelled/refuses/inference.stream.CompleteGeneration",
    "inference.stream.Generation/state/Cancelled/refuses/inference.stream.SupersedeGeneration",
    "inference.stream.Generation/state/Completed/refuses/inference.stream.CancelGeneration",
    "inference.stream.Generation/state/Completed/refuses/inference.stream.CompleteGeneration",
    "inference.stream.Generation/state/Completed/refuses/inference.stream.SupersedeGeneration",
    "inference.stream.Generation/state/Superseded/refuses/inference.stream.CancelGeneration",
    "inference.stream.Generation/state/Superseded/refuses/inference.stream.CompleteGeneration",
    "inference.stream.Generation/state/Superseded/refuses/inference.stream.SupersedeGeneration",
    "inference.stream.Generation/transition/cancel/by/inference.stream.CancelGeneration/cancelled",
    "inference.stream.Generation/transition/complete/by/inference.stream.CompleteGeneration/completed",
    "inference.stream.Generation/transition/supersede/by/inference.stream.SupersedeGeneration/superseded",
    "inference.stream.OpenTurn/outcome/opened",
    "inference.stream.OpenTurn/outcome/run-not-active",
    "inference.stream.OpenTurn/outcome/unknown-run",
    "inference.stream.OutputSegment/state/Committed/refuses/inference.stream.AcknowledgePlayback",
    "inference.stream.OutputSegment/state/Committed/refuses/inference.stream.CommitSegment",
    "inference.stream.OutputSegment/state/Committed/refuses/inference.stream.CompleteSynthesis",
    "inference.stream.OutputSegment/state/Discarded/refuses/inference.stream.AcknowledgePlayback",
    "inference.stream.OutputSegment/state/Discarded/refuses/inference.stream.CommitSegment",
    "inference.stream.OutputSegment/state/Discarded/refuses/inference.stream.CompleteSynthesis",
    "inference.stream.OutputSegment/state/Discarded/refuses/inference.stream.DiscardSegment",
    "inference.stream.OutputSegment/state/Discarded/refuses/inference.stream.QueueForSynthesis",
    "inference.stream.OutputSegment/state/Played/refuses/inference.stream.AcknowledgePlayback",
    "inference.stream.OutputSegment/state/Played/refuses/inference.stream.CommitSegment",
    "inference.stream.OutputSegment/state/Played/refuses/inference.stream.CompleteSynthesis",
    "inference.stream.OutputSegment/state/Played/refuses/inference.stream.DiscardSegment",
    "inference.stream.OutputSegment/state/Played/refuses/inference.stream.QueueForSynthesis",
    "inference.stream.OutputSegment/state/Queued/refuses/inference.stream.AcknowledgePlayback",
    "inference.stream.OutputSegment/state/Queued/refuses/inference.stream.CommitSegment",
    "inference.stream.OutputSegment/state/Queued/refuses/inference.stream.QueueForSynthesis",
    "inference.stream.OutputSegment/state/Synthesized/refuses/inference.stream.CommitSegment",
    "inference.stream.OutputSegment/state/Synthesized/refuses/inference.stream.CompleteSynthesis",
    "inference.stream.OutputSegment/state/Synthesized/refuses/inference.stream.QueueForSynthesis",
    "inference.stream.OutputSegment/state/Tentative/refuses/inference.stream.AcknowledgePlayback",
    "inference.stream.OutputSegment/state/Tentative/refuses/inference.stream.CompleteSynthesis",
    "inference.stream.OutputSegment/state/Tentative/refuses/inference.stream.QueueForSynthesis",
    "inference.stream.OutputSegment/transition/commit/by/inference.stream.CommitSegment/committed",
    "inference.stream.OutputSegment/transition/discard/by/inference.stream.DiscardSegment/discarded",
    "inference.stream.OutputSegment/transition/play/by/inference.stream.AcknowledgePlayback/played",
    "inference.stream.OutputSegment/transition/queue/by/inference.stream.QueueForSynthesis/queued",
    "inference.stream.OutputSegment/transition/synthesize/by/inference.stream.CompleteSynthesis/synthesized",
    "inference.stream.QueueForSynthesis/outcome/queued",
    "inference.stream.ReviseSpan/outcome/immutable",
    "inference.stream.ReviseSpan/outcome/revised",
    "inference.stream.Span/state/Committed/refuses/inference.stream.CommitSpan",
    "inference.stream.Span/state/Committed/refuses/inference.stream.ReviseSpan",
    "inference.stream.Span/transition/commit/by/inference.stream.CommitSpan/committed",
    "inference.stream.StartGeneration/outcome/run-not-active",
    "inference.stream.StartGeneration/outcome/started",
    "inference.stream.StartGeneration/outcome/unknown-node",
    "inference.stream.StartGeneration/outcome/unknown-run",
    "inference.stream.Turn/state/Completed/refuses/inference.stream.CompleteTurn",
    "inference.stream.Turn/transition/complete/by/inference.stream.CompleteTurn/completed",
];

/// The 46 `inference.graph` scenarios of the node-instance lifecycle this target answers: the
/// moving and `wrong-state` outcomes of the six instance commands, each transition, and each
/// command a state does not leave by.
const NODE_INSTANCE_SCENARIOS: [&str; 46] = [
    "inference.graph.CancelNodeInstance/outcome/cancelling",
    "inference.graph.CancelNodeInstance/outcome/wrong-state",
    "inference.graph.DrainNodeInstance/outcome/draining",
    "inference.graph.DrainNodeInstance/outcome/wrong-state",
    "inference.graph.FailNodeInstance/outcome/failed",
    "inference.graph.FailNodeInstance/outcome/wrong-state",
    "inference.graph.NodeInstance/state/Cancelling/refuses/inference.graph.CancelNodeInstance",
    "inference.graph.NodeInstance/state/Cancelling/refuses/inference.graph.DrainNodeInstance",
    "inference.graph.NodeInstance/state/Cancelling/refuses/inference.graph.FailNodeInstance",
    "inference.graph.NodeInstance/state/Cancelling/refuses/inference.graph.PrepareNodeInstance",
    "inference.graph.NodeInstance/state/Cancelling/refuses/inference.graph.StartNodeInstance",
    "inference.graph.NodeInstance/state/Described/refuses/inference.graph.DrainNodeInstance",
    "inference.graph.NodeInstance/state/Described/refuses/inference.graph.StartNodeInstance",
    "inference.graph.NodeInstance/state/Described/refuses/inference.graph.StopNodeInstance",
    "inference.graph.NodeInstance/state/Draining/refuses/inference.graph.DrainNodeInstance",
    "inference.graph.NodeInstance/state/Draining/refuses/inference.graph.PrepareNodeInstance",
    "inference.graph.NodeInstance/state/Draining/refuses/inference.graph.StartNodeInstance",
    "inference.graph.NodeInstance/state/Failed/refuses/inference.graph.CancelNodeInstance",
    "inference.graph.NodeInstance/state/Failed/refuses/inference.graph.DrainNodeInstance",
    "inference.graph.NodeInstance/state/Failed/refuses/inference.graph.FailNodeInstance",
    "inference.graph.NodeInstance/state/Failed/refuses/inference.graph.PrepareNodeInstance",
    "inference.graph.NodeInstance/state/Failed/refuses/inference.graph.StartNodeInstance",
    "inference.graph.NodeInstance/state/Failed/refuses/inference.graph.StopNodeInstance",
    "inference.graph.NodeInstance/state/Prepared/refuses/inference.graph.DrainNodeInstance",
    "inference.graph.NodeInstance/state/Prepared/refuses/inference.graph.PrepareNodeInstance",
    "inference.graph.NodeInstance/state/Started/refuses/inference.graph.PrepareNodeInstance",
    "inference.graph.NodeInstance/state/Started/refuses/inference.graph.StartNodeInstance",
    "inference.graph.NodeInstance/state/Started/refuses/inference.graph.StopNodeInstance",
    "inference.graph.NodeInstance/state/Stopped/refuses/inference.graph.CancelNodeInstance",
    "inference.graph.NodeInstance/state/Stopped/refuses/inference.graph.DrainNodeInstance",
    "inference.graph.NodeInstance/state/Stopped/refuses/inference.graph.FailNodeInstance",
    "inference.graph.NodeInstance/state/Stopped/refuses/inference.graph.PrepareNodeInstance",
    "inference.graph.NodeInstance/state/Stopped/refuses/inference.graph.StartNodeInstance",
    "inference.graph.NodeInstance/state/Stopped/refuses/inference.graph.StopNodeInstance",
    "inference.graph.NodeInstance/transition/cancel/by/inference.graph.CancelNodeInstance/cancelling",
    "inference.graph.NodeInstance/transition/drain/by/inference.graph.DrainNodeInstance/draining",
    "inference.graph.NodeInstance/transition/fail/by/inference.graph.FailNodeInstance/failed",
    "inference.graph.NodeInstance/transition/prepare/by/inference.graph.PrepareNodeInstance/prepared",
    "inference.graph.NodeInstance/transition/start/by/inference.graph.StartNodeInstance/started",
    "inference.graph.NodeInstance/transition/stop/by/inference.graph.StopNodeInstance/stopped",
    "inference.graph.PrepareNodeInstance/outcome/prepared",
    "inference.graph.PrepareNodeInstance/outcome/wrong-state",
    "inference.graph.StartNodeInstance/outcome/started",
    "inference.graph.StartNodeInstance/outcome/wrong-state",
    "inference.graph.StopNodeInstance/outcome/stopped",
    "inference.graph.StopNodeInstance/outcome/wrong-state",
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
    for id in MODEL_SCENARIOS
        .into_iter()
        .chain(STREAM_SCENARIOS)
        .chain(POLICY_SCENARIOS)
        .chain(GRAPH_SCENARIOS)
        .chain(GRAPH_VALIDATION_SCENARIOS)
        .chain(GRAPH_PLANNING_SCENARIOS)
        .chain(RUN_SCENARIOS)
        .chain(RUN_ROUTED_SCENARIOS)
        .chain(NODE_INSTANCE_SCENARIOS)
    {
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
