//! What the workflows under `.github/workflows/` pin and decide, held to the files they read.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

mod ignores;
mod quality;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn check_workflow() -> String {
    fs::read_to_string(root().join(".github/workflows/check.yml")).unwrap()
}

/// The value of the first `KEY: "value"` line of `workflow`.
fn workflow_value(workflow: &str, key: &str) -> Option<String> {
    let line = workflow
        .lines()
        .find(|line| line.trim_start().starts_with(&format!("{key}:")))?;
    Some(line.split_once(':')?.1.trim().trim_matches('"').to_owned())
}

/// The `run: |` block of the step whose `id:` is `id`, its indentation removed.
fn step_script(workflow: &str, id: &str) -> String {
    let lines: Vec<&str> = workflow.lines().collect();
    let at = lines
        .iter()
        .position(|line| line.trim() == format!("id: {id}"))
        .unwrap_or_else(|| panic!("check.yml has no step with id: {id}"));
    let run = (at..lines.len())
        .find(|index| lines[*index].trim() == "run: |")
        .unwrap_or_else(|| panic!("the step {id} has no run: | block"));
    let key = lines[run].len() - lines[run].trim_start().len();
    let body: Vec<&str> = lines[run + 1..]
        .iter()
        .take_while(|line| line.trim().is_empty() || line.len() - line.trim_start().len() > key)
        .copied()
        .collect();
    let indent = body
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);
    body.iter()
        .map(|line| line.get(indent..).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// What the `scope` step of `check.yml` writes as `gate` for a run of `event` whose pull
/// request changed `changed`, with `git` answering the changed paths.
fn gate(event: &str, changed: &[&str]) -> String {
    let script = step_script(&check_workflow(), "scope");
    let case = tempfile_dir();
    let bin = case.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let runner = case.join("runner");
    fs::create_dir_all(&runner).unwrap();
    let list = case.join("paths-git-answers");
    fs::write(
        &list,
        changed
            .iter()
            .map(|path| format!("{path}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let git = bin.join("git");
    fs::write(
        &git,
        format!(
            "#!/bin/sh\n[ \"$*\" = \"diff --name-only --no-renames HEAD^1 HEAD\" ] || exit 2\ncat '{}'\n",
            list.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).unwrap();
    let output = case.join("output");
    fs::write(&output, "").unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let run = Command::new("sh")
        .arg("-c")
        .arg(&script)
        .env("PATH", path)
        .env("EVENT", event)
        .env("RUNNER_TEMP", &runner)
        .env("GITHUB_OUTPUT", &output)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "the scope step failed for {changed:?}: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    if event == "pull_request" {
        assert_eq!(
            fs::read_to_string(runner.join("changed-paths")).ok(),
            fs::read_to_string(&list).ok(),
            "the scope step decides over the paths git answered"
        );
    }
    let written = fs::read_to_string(&output).unwrap();
    written
        .lines()
        .find_map(|line| line.strip_prefix("gate="))
        .unwrap_or_else(|| panic!("the scope step wrote no gate: {written:?}"))
        .to_owned()
}

/// One step of the job of `check.yml`: its lines, from its `- ` to the next step.
struct Step {
    lines: Vec<String>,
}

impl Step {
    /// The value of the step's own `key:` line, at the indentation of its first key.
    fn value(&self, key: &str) -> Option<String> {
        let first = self.lines.first()?;
        let indent = first.len() - first.trim_start().len() + 2;
        self.lines.iter().find_map(|line| {
            let at = line.len() - line.trim_start().len();
            let body = if let Some(body) = line.trim_start().strip_prefix("- ") {
                body
            } else if at == indent {
                line.trim_start()
            } else {
                return None;
            };
            Some(body.strip_prefix(&format!("{key}:"))?.trim().to_owned())
        })
    }

    /// The value of `key:` under the step's `with:`.
    fn with(&self, key: &str) -> Option<String> {
        let at = self.lines.iter().position(|line| line.trim() == "with:")?;
        self.lines[at + 1..].iter().find_map(|line| {
            Some(
                line.trim()
                    .strip_prefix(&format!("{key}:"))?
                    .trim()
                    .to_owned(),
            )
        })
    }

    /// Its name, or what it uses when it has none.
    fn label(&self) -> String {
        self.value("name")
            .or_else(|| self.value("uses"))
            .unwrap_or_default()
    }
}

/// The steps of the job of `check.yml`, in their order.
fn steps(workflow: &str) -> Vec<Step> {
    let lines: Vec<&str> = workflow.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.trim() == "steps:")
        .expect("check.yml has steps");
    let indent = lines[start + 1].len() - lines[start + 1].trim_start().len();
    let mut steps: Vec<Step> = Vec::new();
    for line in &lines[start + 1..] {
        let at = line.len() - line.trim_start().len();
        if !line.trim().is_empty() && at < indent {
            break;
        }
        if at == indent && line.trim_start().starts_with("- ") {
            steps.push(Step { lines: Vec::new() });
        }
        if let Some(step) = steps.last_mut() {
            step.lines.push((*line).to_owned());
        }
    }
    steps
}

/// Whether a step with the condition `condition` runs, for a run of `event` on `reference` whose
/// scope step wrote `gate`. A condition is terms `<context> == '<value>'` joined by `&&`; any
/// other form fails the case, so that a condition this reads wrong cannot pass.
fn runs(condition: Option<&str>, event: &str, reference: &str, gate: &str) -> bool {
    let Some(condition) = condition else {
        return true;
    };
    condition.split("&&").all(|term| {
        let (context, value) = term
            .split_once("==")
            .unwrap_or_else(|| panic!("the condition {condition:?} has a term without =="));
        let value = value.trim();
        let value = value
            .strip_prefix('\'')
            .and_then(|value| value.strip_suffix('\''))
            .unwrap_or_else(|| panic!("the condition {condition:?} compares with {value}"));
        let actual = match context.trim() {
            "steps.scope.outputs.gate" => gate,
            "github.event_name" => event,
            "github.ref" => reference,
            other => {
                panic!("the condition {condition:?} reads {other}, which the case does not set")
            }
        };
        actual == value
    })
}

/// The labels of the steps of `check.yml` that run for a run of `event` on `reference` whose
/// pull request changed `changed`, the gate decided by the scope step itself.
fn steps_that_run(event: &str, reference: &str, changed: &[&str]) -> Vec<String> {
    let gate = gate(event, changed);
    steps(&check_workflow())
        .iter()
        .filter(|step| runs(step.value("if").as_deref(), event, reference, &gate))
        .map(Step::label)
        .collect()
}

/// The label of the one step of `check.yml` that uploads an artifact, held to upload the quality
/// records under the name of the commit, by an action pinned to its full commit id.
fn upload_step() -> String {
    let uploads: Vec<Step> = steps(&check_workflow())
        .into_iter()
        .filter(|step| {
            step.value("uses")
                .is_some_and(|uses| uses.starts_with("actions/upload-artifact@"))
        })
        .collect();
    assert_eq!(uploads.len(), 1, "check.yml has one step that uploads");
    let upload = &uploads[0];
    let uses = upload.value("uses").unwrap();
    let pin = uses.split_once('@').unwrap().1;
    assert!(
        pin.len() == 40 && pin.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "upload-artifact is pinned by its full commit id, not {pin:?}"
    );
    assert_eq!(
        upload.with("path").as_deref(),
        Some("target/quality-records/")
    );
    let name = upload.with("name").expect("the artifact has a name");
    assert!(
        name.contains("${{ github.sha }}"),
        "the artifact is named with the commit: {name}"
    );
    assert_eq!(upload.with("if-no-files-found").as_deref(), Some("error"));
    upload.label()
}

/// Whether `ran` holds the step `task quality-records` and the upload, each held to come after
/// the step it reads from.
fn records_steps(ran: &[String]) -> (bool, bool) {
    let upload = upload_step();
    let at = |label: &str| ran.iter().position(|step| step == label);
    let records = at("task quality-records");
    let uploaded = at(&upload);
    if let Some(records) = records {
        assert!(
            at("task test-run").is_some_and(|test_run| test_run < records),
            "task quality-records runs after task test-run: {ran:?}"
        );
    }
    if let Some(uploaded) = uploaded {
        assert!(
            records.is_some_and(|records| records < uploaded),
            "the upload runs after task quality-records: {ran:?}"
        );
    }
    (records.is_some(), uploaded.is_some())
}

#[test]
fn a_push_to_main_writes_the_quality_records_and_uploads_them() {
    let ran = steps_that_run("push", "refs/heads/main", &[]);
    assert_eq!(records_steps(&ran), (true, true), "{ran:?}");
    let step = steps(&check_workflow())
        .into_iter()
        .find(|step| step.label() == "task quality-records")
        .expect("check.yml has the step task quality-records");
    assert_eq!(step.value("run").as_deref(), Some("task quality-records"));
}

/// A refused record fails the forge's required check as it fails `task check`: neither the step
/// nor its job lets it fail. `check.yml` is read as YAML, so a key is found wherever it stands
/// in its mapping.
#[test]
fn the_records_step_and_its_job_do_not_continue_on_error() {
    let workflow: serde_json::Value =
        serde_norway::from_str(&check_workflow()).expect("check.yml parses");
    let job = workflow
        .pointer("/jobs/check")
        .expect("check.yml has the job check");
    assert_eq!(
        job.get("continue-on-error"),
        None,
        "the job check continues on error"
    );
    let records: Vec<&serde_json::Value> = job
        .get("steps")
        .and_then(serde_json::Value::as_array)
        .expect("the job has steps")
        .iter()
        .filter(|step| {
            step.get("run").and_then(serde_json::Value::as_str) == Some("task quality-records")
        })
        .collect();
    assert_eq!(records.len(), 1, "one step runs task quality-records");
    assert_eq!(
        records[0].get("continue-on-error"),
        None,
        "the step task quality-records continues on error"
    );
}

#[test]
fn a_pull_request_writes_the_quality_records_and_does_not_upload_them() {
    let ran = steps_that_run(
        "pull_request",
        "refs/pull/1/merge",
        &["crates/inference/src/lib.rs"],
    );
    assert_eq!(records_steps(&ran), (true, false), "{ran:?}");
}

#[test]
fn the_planning_gate_neither_writes_nor_uploads_the_quality_records() {
    let ran = steps_that_run(
        "pull_request",
        "refs/pull/1/merge",
        &[".engineering/planning/story/a.md"],
    );
    assert!(
        ran.iter().any(|step| step == "Planning store validation"),
        "{ran:?}"
    );
    assert_eq!(records_steps(&ran), (false, false), "{ran:?}");
}

/// A fresh directory of its own under the target's temporary directory.
fn tempfile_dir() -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "workflows-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_pull_request_of_planning_and_prose_files_only_skips_the_build() {
    for changed in [
        &[".engineering/planning/story/a.md"][..],
        &[
            ".engineering/planning/story/a.md",
            ".engineering/evidence/story/a/20261123T000000Z-000-0.json",
        ],
        &["AGENTS.md", "README.md", "docs/design/inference.md"],
        &[
            "docs/handoff/2026-11-23.md",
            ".engineering/planning/epic/b.md",
        ],
        &["adapters/inference-openai/README.md"],
        &["docs/quality/README.md"],
    ] {
        assert_eq!(gate("pull_request", changed), "planning", "{changed:?}");
    }
}

#[test]
fn any_other_changed_path_runs_the_full_gate() {
    for changed in [
        &["crates/inference/src/lib.rs"][..],
        &[".engineering/planning/story/a.md", "Cargo.toml"],
        &["AGENTS.md", "Taskfile.yml"],
        &["website/docs/index.md"],
        &["website/README.md"],
        &["CHANGELOG.md"],
        &["ess/SKIPPED.md"],
        &["ess/domains/session.yaml"],
        &["quality/README.md"],
        &[
            "quality/not-measured.md",
            ".engineering/planning/story/a.md",
        ],
        &["rust-toolchain.toml"],
        &[".config/nextest.toml"],
        &[".github/workflows/check.yml"],
        &["docs/brand/banner.svg"],
        &[],
    ] {
        assert_eq!(gate("pull_request", changed), "full", "{changed:?}");
    }
}

#[test]
fn a_push_runs_the_full_gate_whatever_it_changed() {
    assert_eq!(gate("push", &[".engineering/planning/story/a.md"]), "full");
}

#[test]
fn ci_installs_the_rustc_rust_toolchain_toml_pins() {
    let pin = fs::read_to_string(root().join("rust-toolchain.toml")).unwrap();
    let channel = pin
        .lines()
        .find_map(|line| line.trim().strip_prefix("channel = "))
        .map(|value| value.trim_matches('"').to_owned())
        .expect("rust-toolchain.toml names a channel");
    assert!(
        channel.split('.').count() == 3
            && channel.split('.').all(|part| part.parse::<u32>().is_ok()),
        "rust-toolchain.toml pins one rustc release, not {channel:?}"
    );
    for workflow in ["check.yml", "pages.yml", "slow.yml"] {
        let text = fs::read_to_string(root().join(".github/workflows").join(workflow)).unwrap();
        assert!(
            !text.contains("toolchain: stable"),
            "{workflow} installs stable instead of the rustc rust-toolchain.toml pins"
        );
        assert!(
            text.contains("rust-toolchain.toml"),
            "{workflow} does not read rust-toolchain.toml"
        );
    }
}

#[test]
fn ci_installs_the_nextest_version_the_nextest_config_requires() {
    let config = fs::read_to_string(root().join(".config/nextest.toml")).unwrap();
    let required = config
        .lines()
        .find_map(|line| line.strip_prefix("nextest-version = { required = \""))
        .and_then(|rest| rest.split('"').next())
        .expect(".config/nextest.toml requires a nextest version");
    for workflow in ["check.yml", "slow.yml"] {
        let text = fs::read_to_string(root().join(".github/workflows").join(workflow)).unwrap();
        assert_eq!(
            workflow_value(&text, "NEXTEST_VERSION").as_deref(),
            Some(required),
            "{workflow}"
        );
    }
    assert!(
        config.contains("slow-timeout = { period = \"60s\", terminate-after = 1 }"),
        "the default profile kills a test at 60 s"
    );
}

fn ci_ess_version() -> String {
    let workflow = fs::read_to_string(root().join(".github/workflows/check.yml")).unwrap();
    let line = workflow
        .lines()
        .find(|line| line.trim_start().starts_with("ESS_VERSION:"))
        .expect("check.yml sets ESS_VERSION");
    line.split(':')
        .nth(1)
        .unwrap()
        .trim()
        .trim_matches('"')
        .to_owned()
}

#[test]
fn the_run_the_checks_guide_names_the_ess_version_ci_installs() {
    let version = ci_ess_version();
    let guide = fs::read_to_string(root().join("website/docs/guides/run-the-checks.md")).unwrap();
    let claim = format!("With ess {version}, the release CI installs");
    assert!(
        guide.contains(&claim),
        "website/docs/guides/run-the-checks.md does not say {claim:?}; CI installs ess {version}"
    );
    assert!(
        !guide.contains("which is newer"),
        "website/docs/guides/run-the-checks.md shows a 'newer' warning that ess {version} no longer prints for ess/ess-inputs.yaml"
    );
}

/// The workflow that appends the records of each check run on `main` to the series, and the ref
/// it alone writes.
const SERIES_WORKFLOW: &str = "quality-series.yml";
const SERIES_REF: &str = "refs/ratchet/records";
/// The identity GitHub Actions commits as.
const ACTIONS_NAME: &str = "github-actions[bot]";
const ACTIONS_EMAIL: &str = "41898282+github-actions[bot]@users.noreply.github.com";

/// Every workflow under `.github/workflows/`, by its file name, with its text.
fn workflow_files() -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = fs::read_dir(root().join(".github/workflows"))
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| {
            let name = entry.file_name().into_string().unwrap();
            name.ends_with(".yml") || name.ends_with(".yaml")
        })
        .map(|entry| {
            (
                entry.file_name().into_string().unwrap(),
                fs::read_to_string(entry.path()).unwrap(),
            )
        })
        .collect();
    found.sort();
    found
}

fn series_workflow() -> String {
    fs::read_to_string(root().join(".github/workflows").join(SERIES_WORKFLOW))
        .unwrap_or_else(|error| panic!("{SERIES_WORKFLOW} cannot be read: {error}"))
}

/// The workflows the way `series_problems` reads them: the series workflow and every other one.
fn split_workflows() -> (String, Vec<(String, String)>) {
    let mut series = None;
    let mut others = Vec::new();
    for (name, text) in workflow_files() {
        if name == SERIES_WORKFLOW {
            series = Some(text);
        } else {
            others.push((name, text));
        }
    }
    (
        series.unwrap_or_else(|| panic!("{SERIES_WORKFLOW} does not exist")),
        others,
    )
}

/// Whether a job with the condition `condition` runs for a run of `check` that concluded
/// `conclusion`, of the event `event` on the branch `branch`. `None` when the condition has a
/// term other than `github.event.workflow_run.<field> == '<value>'` joined by `&&`.
fn series_job_runs(condition: &str, conclusion: &str, event: &str, branch: &str) -> Option<bool> {
    let mut runs = true;
    for term in condition.split("&&") {
        let (context, value) = term.split_once("==")?;
        let value = value.trim().strip_prefix('\'')?.strip_suffix('\'')?;
        if value.contains('\'') {
            return None;
        }
        let actual = match context.trim() {
            "github.event.workflow_run.conclusion" => conclusion,
            "github.event.workflow_run.event" => event,
            "github.event.workflow_run.head_branch" => branch,
            _ => return None,
        };
        runs &= actual == value;
    }
    Some(runs)
}

fn yaml(text: &str) -> Result<serde_json::Value, String> {
    serde_norway::from_str(text).map_err(|error| error.to_string())
}

/// The keys of a mapping, in order.
fn keys(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(serde_json::Value::as_object)
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default()
}

/// Every place of `workflow` that grants a write permission, by where it stands: a `permissions`
/// of the workflow or of a job that is `write-all` or names a scope with `write`.
fn writes(workflow: &serde_json::Value) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut check = |at: String, permissions: Option<&serde_json::Value>| match permissions {
        Some(serde_json::Value::String(all)) if all != "read-all" => {
            found.push((at, all.clone()));
        }
        Some(serde_json::Value::Object(scopes)) => {
            for (scope, level) in scopes {
                let level = level
                    .as_str()
                    .map_or_else(|| level.to_string(), str::to_owned);
                if level != "read" && level != "none" {
                    found.push((at.clone(), format!("{scope}: {level}")));
                }
            }
        }
        _ => {}
    };
    check("the workflow".to_owned(), workflow.get("permissions"));
    for (job, body) in workflow
        .get("jobs")
        .and_then(serde_json::Value::as_object)
        .into_iter()
        .flatten()
    {
        check(format!("the job {job}"), body.get("permissions"));
    }
    found
}

/// Every way the series workflow `series` and the other workflows `others` break the rules of
/// the series' one writer, one line each; empty when they hold. `check` is the text of
/// `check.yml`, whose name the trigger names and whose pins the actions take.
fn series_problems(series: &str, check: &str, others: &[(String, String)]) -> Vec<String> {
    let mut problems = Vec::new();
    let workflow = match yaml(series) {
        Ok(workflow) => workflow,
        Err(error) => return vec![format!("{SERIES_WORKFLOW} does not parse: {error}")],
    };
    let check_name = yaml(check)
        .ok()
        .and_then(|check| {
            check
                .get("name")
                .and_then(|name| name.as_str().map(str::to_owned))
        })
        .unwrap_or_default();

    let on = workflow.get("on");
    if keys(on) != ["workflow_run"] {
        problems.push(format!(
            "the trigger is {:?}; the series is appended on workflow_run alone",
            keys(on)
        ));
    }
    let run = on.and_then(|on| on.get("workflow_run"));
    for key in keys(run) {
        if !["workflows", "types", "branches"].contains(&key.as_str()) {
            problems.push(format!("workflow_run filters by {key}"));
        }
    }
    let list = |key: &str| run.and_then(|run| run.get(key)).cloned();
    if list("workflows") != Some(serde_json::json!([check_name])) {
        problems.push(format!(
            "workflow_run names the workflows {:?}, not the check workflow {check_name:?} alone",
            list("workflows")
        ));
    }
    if list("types") != Some(serde_json::json!(["completed"])) {
        problems.push(format!(
            "workflow_run has the types {:?}, not completed alone",
            list("types")
        ));
    }
    if list("branches").is_some_and(|branches| branches != serde_json::json!(["main"])) {
        problems.push(format!(
            "workflow_run filters the branches {:?}",
            list("branches")
        ));
    }

    if workflow.get("permissions") != Some(&serde_json::json!({"contents": "read"})) {
        problems.push(format!(
            "the workflow's permissions are {:?}, not contents: read",
            workflow.get("permissions")
        ));
    }
    let concurrency = |at: &str, value: Option<&serde_json::Value>, required: bool| {
        let mut found = Vec::new();
        match value {
            None if required => found.push(format!("{at} has no concurrency group")),
            None => {}
            Some(value) => {
                if value
                    .get("group")
                    .and_then(serde_json::Value::as_str)
                    .is_none_or(str::is_empty)
                {
                    found.push(format!("the concurrency of {at} names no group"));
                }
                if value.get("cancel-in-progress") != Some(&serde_json::Value::Bool(false)) {
                    found.push(format!(
                        "the concurrency of {at} has cancel-in-progress {:?}; a run waits, it never cancels one",
                        value.get("cancel-in-progress")
                    ));
                }
            }
        }
        found
    };
    problems.extend(concurrency(
        "the workflow",
        workflow.get("concurrency"),
        true,
    ));

    let jobs = workflow
        .get("jobs")
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    if jobs.len() != 1 {
        problems.push(format!(
            "{SERIES_WORKFLOW} has {} jobs; the writer is its only job",
            jobs.len()
        ));
    }
    for (index, (name, job)) in jobs.iter().enumerate() {
        let at = format!("the job {name}");
        if index > 0 {
            for (_, grant) in writes(&serde_json::json!({"jobs": {name: job}})) {
                problems.push(format!("{at} of {SERIES_WORKFLOW} grants {grant}"));
            }
            continue;
        }
        if job.get("permissions")
            != Some(&serde_json::json!({"contents": "write", "actions": "read"}))
        {
            problems.push(format!(
                "{at} has the permissions {:?}, not contents: write and actions: read",
                job.get("permissions")
            ));
        }
        problems.extend(concurrency(&at, job.get("concurrency"), false));
        if job.get("continue-on-error").is_some() {
            problems.push(format!("{at} continues on error"));
        }
        match job.get("if").and_then(serde_json::Value::as_str) {
            None => problems.push(format!("{at} has no condition")),
            Some(condition) => {
                for conclusion in ["success", "failure", "cancelled"] {
                    for event in ["push", "pull_request", "workflow_dispatch"] {
                        for branch in ["main", "feature"] {
                            let wanted =
                                conclusion == "success" && event == "push" && branch == "main";
                            match series_job_runs(condition, conclusion, event, branch) {
                                None => {
                                    problems.push(format!(
                                        "the condition of {at} cannot be read: {condition:?}"
                                    ));
                                }
                                Some(runs) if runs != wanted => problems.push(format!(
                                    "{at} runs {runs} for a check run that concluded {conclusion}, of a {event} on {branch}"
                                )),
                                Some(_) => continue,
                            }
                            break;
                        }
                    }
                }
            }
        }
        for step in job
            .get("steps")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            if step.get("continue-on-error").is_some() {
                problems.push(format!("a step of {at} continues on error"));
            }
            let Some(uses) = step.get("uses").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let pin = uses.split_once('@').map_or("", |(_, pin)| pin);
            if pin.len() != 40 || !pin.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                problems.push(format!("{uses} is not pinned by a full commit id"));
            }
            if !check
                .lines()
                .any(|line| line.trim().trim_start_matches("- ") == format!("uses: {uses}"))
            {
                problems.push(format!("{uses} is no action check.yml pins"));
            }
        }
    }

    if workflow.pointer("/env/SERIES_REF") != Some(&serde_json::json!(SERIES_REF)) {
        problems.push(format!(
            "the workflow names the ref as {:?} in its env SERIES_REF, not {SERIES_REF}",
            workflow.pointer("/env/SERIES_REF")
        ));
    }
    let named = series.matches(SERIES_REF).count();
    if named != 1 {
        problems.push(format!(
            "{SERIES_WORKFLOW} names {SERIES_REF} {named} times; it is named once, in its env"
        ));
    }

    for (name, text) in others {
        let other = match yaml(text) {
            Ok(other) => other,
            Err(error) => {
                problems.push(format!("{name} does not parse: {error}"));
                continue;
            }
        };
        if other.get("permissions").is_none() {
            problems.push(format!(
                "{name} sets no permissions, so its jobs get the repository's default"
            ));
        }
        for (at, grant) in writes(&other) {
            if grant.starts_with("contents:") || !grant.contains(':') {
                problems.push(format!("{at} of {name} grants {grant}"));
            }
        }
        if text.contains("refs/ratchet") {
            problems.push(format!("{name} names refs/ratchet"));
        }
    }
    problems.dedup();
    problems
}

#[test]
fn the_series_is_written_by_one_job_after_a_successful_check_of_a_push_to_main() {
    let (series, others) = split_workflows();
    let problems = series_problems(&series, &check_workflow(), &others);
    assert!(problems.is_empty(), "{problems:#?}");
}

/// The problems after `plant` changed the workflows, held to name `expected`.
#[track_caller]
fn assert_series_problem(
    expected: &str,
    plant: impl FnOnce(&mut String, &mut Vec<(String, String)>),
) {
    let (mut series, mut others) = split_workflows();
    plant(&mut series, &mut others);
    let problems = series_problems(&series, &check_workflow(), &others);
    assert!(
        problems.iter().any(|problem| problem.contains(expected)),
        "no problem names {expected:?}: {problems:#?}"
    );
}

#[track_caller]
fn replaced(text: &str, from: &str, to: &str) -> String {
    assert!(text.contains(from), "the text holds {from:?}");
    text.replacen(from, to, 1)
}

fn other<'a>(others: &'a mut [(String, String)], name: &str) -> &'a mut String {
    &mut others
        .iter_mut()
        .find(|(file, _)| file == name)
        .unwrap_or_else(|| panic!("no workflow {name}"))
        .1
}

#[test]
fn a_write_permission_on_any_other_job_is_refused() {
    assert_series_problem(
        "the job extra of quality-series.yml grants contents: write",
        |series, _| {
            series.push_str(
            "\n  extra:\n    runs-on: ubuntu-latest\n    permissions:\n      contents: write\n    steps:\n      - run: \"true\"\n",
        );
        },
    );
    assert_series_problem("has 2 jobs", |series, _| {
        series.push_str(
            "\n  extra:\n    runs-on: ubuntu-latest\n    steps:\n      - run: \"true\"\n",
        );
    });
    assert_series_problem(
        "the job check of check.yml grants contents: write",
        |_, others| {
            let check = other(others, "check.yml");
            *check = replaced(
                check,
                "    permissions:\n      contents: read",
                "    permissions:\n      contents: write",
            );
        },
    );
    assert_series_problem("the workflow of pages.yml grants write-all", |_, others| {
        let pages = other(others, "pages.yml");
        *pages = replaced(
            pages,
            "permissions:\n  contents: read",
            "permissions: write-all",
        );
    });
    assert_series_problem("slow.yml sets no permissions", |_, others| {
        let slow = other(others, "slow.yml");
        *slow = replaced(slow, "permissions:\n  contents: read\n", "");
    });
    assert_series_problem("docs-ux.yml names refs/ratchet", |_, others| {
        let docs = other(others, "docs-ux.yml");
        docs.push_str("# refs/ratchet/records\n");
    });
    assert_series_problem("not contents: write and actions: read", |series, _| {
        *series = replaced(series, "      actions: read\n", "      actions: write\n");
    });
    assert_series_problem("not contents: read", |series, _| {
        *series = replaced(
            series,
            "permissions:\n  contents: read",
            "permissions:\n  contents: write",
        );
    });
}

#[test]
fn a_trigger_other_than_a_successful_check_on_main_is_refused() {
    assert_series_problem("the trigger is", |series, _| {
        *series = replaced(series, "on:\n", "on:\n  push:\n    branches: [main]\n");
    });
    assert_series_problem("not the check workflow", |series, _| {
        *series = replaced(series, "workflows: [check]", "workflows: [slow]");
    });
    assert_series_problem("not completed alone", |series, _| {
        *series = replaced(
            series,
            "types: [completed]",
            "types: [requested, completed]",
        );
    });
    assert_series_problem("filters the branches", |series, _| {
        *series = replaced(series, "branches: [main]", "branches: ['**']");
    });
    assert_series_problem("for a check run that concluded failure", |series, _| {
        *series = replaced(
            series,
            "github.event.workflow_run.conclusion == 'success'\n      && ",
            "",
        );
    });
    assert_series_problem("of a pull_request on main", |series, _| {
        *series = replaced(
            series,
            "\n      && github.event.workflow_run.event == 'push'",
            "",
        );
    });
    assert_series_problem("of a push on feature", |series, _| {
        *series = replaced(
            series,
            "\n      && github.event.workflow_run.head_branch == 'main'",
            "",
        );
    });
    assert_series_problem("cannot be read", |series, _| {
        *series = replaced(
            series,
            "github.event.workflow_run.conclusion == 'success'",
            "(github.event.workflow_run.conclusion == 'success' || true)",
        );
    });
}

#[test]
fn a_concurrency_that_cancels_is_refused() {
    assert_series_problem("cancel-in-progress Some(Bool(true))", |series, _| {
        *series = replaced(
            series,
            "cancel-in-progress: false",
            "cancel-in-progress: true",
        );
    });
    assert_series_problem("the workflow has no concurrency group", |series, _| {
        *series = replaced(
            series,
            "concurrency:\n  group: quality-series\n  cancel-in-progress: false\n",
            "",
        );
    });
    assert_series_problem(
        "the concurrency of the job append has cancel-in-progress",
        |series, _| {
            *series = replaced(
                series,
                "    runs-on: ubuntu-latest\n",
                "    runs-on: ubuntu-latest\n    concurrency:\n      group: append\n      cancel-in-progress: true\n",
            );
        },
    );
}

#[test]
fn an_action_check_does_not_pin_and_a_ref_named_twice_are_refused() {
    assert_series_problem("is not pinned by a full commit id", |series, _| {
        *series = replaced(
            series,
            "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
            "actions/checkout@v6",
        );
    });
    assert_series_problem("is no action check.yml pins", |series, _| {
        *series = replaced(
            series,
            "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
            "actions/checkout@0000000000000000000000000000000000000000",
        );
    });
    assert_series_problem("names refs/ratchet/records 2 times", |series, _| {
        series.push_str("# refs/ratchet/records\n");
    });
}

/// The expression each `env` value of a step of the series workflow stands for in a case.
fn step_env(step: &serde_json::Value, values: &[(&str, String)]) -> Vec<(String, String)> {
    step.get("env")
        .and_then(serde_json::Value::as_object)
        .into_iter()
        .flatten()
        .map(|(key, value)| {
            let value = value.as_str().unwrap_or_default();
            let given = values
                .iter()
                .find(|(expression, _)| *expression == value)
                .unwrap_or_else(|| panic!("the case gives no value for {key}: {value}"));
            (key.clone(), given.1.clone())
        })
        .collect()
}

/// The first `git` on the path.
fn real_git() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|directory| directory.join("git"))
        .find(|git| git.is_file())
        .expect("git is on the path")
}

/// Runs `git` with `arguments` in `directory`, with no configuration of the host.
fn git_in(directory: &Path, arguments: &[&str]) -> std::process::Output {
    Command::new(real_git())
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap()
}

#[track_caller]
fn git_text(directory: &Path, arguments: &[&str]) -> String {
    let output = git_in(directory, arguments);
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .trim_end()
        .to_owned()
}

/// A forge in a directory: the bare repository the series workflow pushes to, the artifact of a
/// check run, and stand-ins of `gh` and `git` on the path. The stand-in of `gh` copies the
/// artifact where `gh run download` is told to put it, for the run and the commit of the case
/// alone. The stand-in of `git` counts the pushes and, for the first `moves` of them, first
/// pushes a commit of another writer to the ref, as a run of the workflow beside this one would.
struct Forge {
    directory: PathBuf,
}

impl Forge {
    fn new(hook: Option<&str>) -> Self {
        let forge = Self {
            directory: tempfile_dir(),
        };
        fs::create_dir_all(forge.remote()).unwrap();
        git_text(&forge.remote(), &["init", "-q", "--bare"]);
        if let Some(hook) = hook {
            let path = forge.remote().join("hooks/pre-receive");
            fs::write(&path, hook).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let fixture = root().join("crates/inference-docs/tests/fixtures/adopt/artifact");
        copy_tree(&fixture, &forge.artifact());
        fs::create_dir_all(forge.other()).unwrap();
        git_text(&forge.other(), &["init", "-q"]);
        let bin = forge.directory.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let gh = bin.join("gh");
        fs::write(
            &gh,
            "#!/bin/sh\n\
             [ \"$1 $2 $3\" = \"run download $CASE_RUN\" ] || { echo \"gh: unexpected call: $*\" >&2; exit 2; }\n\
             shift 3\n\
             repo= name= dir=\n\
             while [ $# -gt 0 ]; do\n\
               case \"$1\" in\n\
                 --repo) repo=$2 ;;\n\
                 --name) name=$2 ;;\n\
                 --dir) dir=$2 ;;\n\
                 *) echo \"gh: unexpected argument $1\" >&2; exit 2 ;;\n\
               esac\n\
               shift 2\n\
             done\n\
             [ \"$repo\" = \"$GITHUB_REPOSITORY\" ] || { echo \"gh: repo $repo\" >&2; exit 2; }\n\
             [ \"$name\" = \"quality-records-$CASE_COMMIT\" ] || { echo \"gh: no artifact $name\" >&2; exit 1; }\n\
             [ -n \"$GH_TOKEN\" ] || { echo \"gh: no token\" >&2; exit 4; }\n\
             mkdir -p \"$dir\" && cp -R \"$CASE_ARTIFACT/.\" \"$dir\"\n",
        )
        .unwrap();
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
        let git = bin.join("git");
        fs::write(
            &git,
            format!(
                "#!/bin/sh\n\
                 for argument in \"$@\"; do\n\
                   if [ \"$argument\" = push ]; then\n\
                     echo push >> \"$CASE_PUSHES\"\n\
                     count=$(wc -l < \"$CASE_PUSHES\")\n\
                     if [ \"$count\" -le \"$CASE_MOVES\" ]; then\n\
                       \"$CASE_GIT\" -C \"$CASE_OTHER\" fetch -q \"$CASE_REMOTE\" {SERIES_REF} || exit 90\n\
                       \"$CASE_GIT\" -C \"$CASE_OTHER\" checkout -q --detach FETCH_HEAD || exit 91\n\
                       echo \"$count $$\" > \"$CASE_OTHER/concurrent-$count\"\n\
                       \"$CASE_GIT\" -C \"$CASE_OTHER\" add -A || exit 92\n\
                       \"$CASE_GIT\" -C \"$CASE_OTHER\" -c user.name=other -c user.email=other@example.invalid commit -q -m \"concurrent $count\" || exit 93\n\
                       \"$CASE_GIT\" -C \"$CASE_OTHER\" push -q \"$CASE_REMOTE\" HEAD:{SERIES_REF} || exit 94\n\
                     fi\n\
                     break\n\
                   fi\n\
                 done\n\
                 exec \"$CASE_GIT\" \"$@\"\n"
            ),
        )
        .unwrap();
        fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).unwrap();
        forge
    }

    fn remote(&self) -> PathBuf {
        self.directory.join("forge/owner/inference.git")
    }

    fn artifact(&self) -> PathBuf {
        self.directory.join("artifact")
    }

    fn other(&self) -> PathBuf {
        self.directory.join("other")
    }

    fn pushes(&self) -> PathBuf {
        self.directory.join("pushes")
    }

    /// The commit the ref points at on the forge, if it exists.
    fn head(&self) -> Option<String> {
        let output = git_in(&self.remote(), &["rev-parse", "--verify", "-q", SERIES_REF]);
        output
            .status
            .success()
            .then(|| String::from_utf8(output.stdout).unwrap().trim().to_owned())
    }

    fn count_pushes(&self) -> usize {
        fs::read_to_string(self.pushes())
            .map(|text| text.lines().count())
            .unwrap_or(0)
    }

    /// Runs the steps `download` and `append` of the series workflow for the check run `run` of
    /// the commit `commit`, with the other writer moving the ref before the first `moves` pushes;
    /// the exit status of the step that ended the run and what both printed.
    fn run(&self, run: &str, commit: &str, moves: usize) -> (bool, String) {
        let _ = fs::remove_file(self.pushes());
        let text = series_workflow();
        let workflow = yaml(&text).unwrap();
        let steps = workflow
            .pointer("/jobs/append/steps")
            .and_then(serde_json::Value::as_array)
            .expect("the job append has steps")
            .clone();
        let runner = tempfile_dir();
        let path = format!(
            "{}:{}",
            self.directory.join("bin").display(),
            std::env::var("PATH").unwrap()
        );
        let values = [
            ("${{ github.token }}", "a-token".to_owned()),
            ("${{ github.event.workflow_run.id }}", run.to_owned()),
            (
                "${{ github.event.workflow_run.head_sha }}",
                commit.to_owned(),
            ),
            (
                "target/debug/inference-docs",
                env!("CARGO_BIN_EXE_inference-docs").to_owned(),
            ),
        ];
        let mut printed = String::new();
        for id in ["download", "append"] {
            let step = steps
                .iter()
                .find(|step| step.get("id").and_then(serde_json::Value::as_str) == Some(id))
                .unwrap_or_else(|| panic!("the job append has no step {id}"));
            assert_eq!(
                step.get("shell").and_then(serde_json::Value::as_str),
                Some("sh"),
                "the step {id} runs in sh"
            );
            let script = step_script(&text, id);
            let output = Command::new("sh")
                .arg("-c")
                .arg(&script)
                .current_dir(root())
                .envs(step_env(step, &values))
                .env("PATH", &path)
                .env("SERIES_REF", SERIES_REF)
                .env("RUNNER_TEMP", &runner)
                .env("GITHUB_WORKSPACE", root())
                .env(
                    "GITHUB_SERVER_URL",
                    format!("file://{}", self.directory.join("forge").display()),
                )
                .env("GITHUB_REPOSITORY", "owner/inference")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("CASE_RUN", run)
                .env("CASE_COMMIT", commit)
                .env("CASE_ARTIFACT", self.artifact())
                .env("CASE_GIT", real_git())
                .env("CASE_PUSHES", self.pushes())
                .env("CASE_MOVES", moves.to_string())
                .env("CASE_OTHER", self.other())
                .env("CASE_REMOTE", format!("file://{}", self.remote().display()))
                .output()
                .unwrap();
            printed.push_str(&String::from_utf8_lossy(&output.stdout));
            printed.push_str(&String::from_utf8_lossy(&output.stderr));
            if !output.status.success() {
                return (false, printed);
            }
        }
        (true, printed)
    }

    /// The paths the series holds at `commit` on the forge.
    fn tree(&self, commit: &str) -> Vec<String> {
        git_text(&self.remote(), &["ls-tree", "-r", "--name-only", commit])
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// The parents of `commit` on the forge.
    fn parents(&self, commit: &str) -> Vec<String> {
        git_text(
            &self.remote(),
            &["rev-list", "--parents", "-n", "1", commit],
        )
        .split(' ')
        .skip(1)
        .map(str::to_owned)
        .collect()
    }

    /// The paths the series holds for the artifact as it is now.
    fn records_in_series(&self) -> Vec<String> {
        let mut paths: Vec<String> = files(&self.artifact())
            .into_iter()
            .map(|(path, bytes)| {
                let digest: String = <sha2::Sha256 as sha2::Digest>::digest(&bytes)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect();
                format!(
                    "{}/{digest}.evidence.json",
                    path.strip_suffix(".evidence.json").unwrap()
                )
            })
            .collect();
        paths.sort();
        paths
    }

    /// Changes the value of the slowest test in the record of `workspace-tests`.
    fn change_a_record(&self, value: &str) {
        let path = self.artifact().join("gate/workspace-tests.evidence.json");
        let text = fs::read_to_string(&path).unwrap();
        let at = text.find("\"value\": \"").unwrap() + "\"value\": \"".len();
        let end = at + text[at..].find('"').unwrap();
        let changed = format!("{}{value}{}", &text[..at], &text[end..]);
        assert_ne!(changed, text);
        fs::write(path, changed).unwrap();
    }
}

/// The files under `directory`, by their path from it, with their bytes.
fn files(directory: &Path) -> Vec<(String, Vec<u8>)> {
    let mut found = Vec::new();
    let mut open = vec![(directory.to_path_buf(), String::new())];
    while let Some((at, prefix)) = open.pop() {
        for entry in fs::read_dir(&at).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().into_string().unwrap();
            if entry.file_type().unwrap().is_dir() {
                open.push((entry.path(), format!("{prefix}{name}/")));
            } else {
                found.push((format!("{prefix}{name}"), fs::read(entry.path()).unwrap()));
            }
        }
    }
    found.sort();
    found
}

fn copy_tree(from: &Path, to: &Path) {
    for (path, bytes) in files(from) {
        let at = to.join(path);
        fs::create_dir_all(at.parent().unwrap()).unwrap();
        fs::write(at, bytes).unwrap();
    }
}

const FIRST_RUN: &str = "45000000101";
const FIRST_COMMIT: &str = "1111111111111111111111111111111111111111";
const SECOND_RUN: &str = "45000000202";
const SECOND_COMMIT: &str = "2222222222222222222222222222222222222222";

#[test]
fn the_first_run_starts_the_series_as_a_commit_with_no_parent() {
    let forge = Forge::new(None);
    assert_eq!(forge.head(), None);
    let (passed, printed) = forge.run(FIRST_RUN, FIRST_COMMIT, 0);
    assert!(passed, "{printed}");
    let head = forge.head().expect("the run created the ref");
    assert_eq!(forge.parents(&head), Vec::<String>::new());
    assert_eq!(forge.tree(&head), forge.records_in_series());
    assert_eq!(
        git_text(
            &forge.remote(),
            &["log", "-1", "--format=%an <%ae>|%cn <%ce>", &head]
        ),
        format!("{ACTIONS_NAME} <{ACTIONS_EMAIL}>|{ACTIONS_NAME} <{ACTIONS_EMAIL}>")
    );
    let message = git_text(&forge.remote(), &["log", "-1", "--format=%B", &head]);
    assert!(
        message.lines().next().unwrap().contains(FIRST_RUN),
        "the subject names the run: {message}"
    );
    assert!(
        message.contains(FIRST_COMMIT),
        "the message names the commit: {message}"
    );
    assert!(
        message.contains(&format!("/actions/runs/{FIRST_RUN}")),
        "the message links the run: {message}"
    );
    assert_eq!(forge.count_pushes(), 1);
}

#[test]
fn a_run_of_the_same_records_pushes_nothing_and_a_changed_record_adds_one_commit_with_one_file() {
    let forge = Forge::new(None);
    assert!(forge.run(FIRST_RUN, FIRST_COMMIT, 0).0);
    let first = forge.head().unwrap();
    let before = forge.tree(&first);
    let (passed, printed) = forge.run(FIRST_RUN, FIRST_COMMIT, 0);
    assert!(passed, "{printed}");
    assert_eq!(forge.head().as_deref(), Some(first.as_str()), "{printed}");
    assert_eq!(forge.count_pushes(), 0, "{printed}");

    forge.change_a_record("7.250");
    let (passed, printed) = forge.run(SECOND_RUN, SECOND_COMMIT, 0);
    assert!(passed, "{printed}");
    let second = forge.head().unwrap();
    assert_eq!(forge.parents(&second), vec![first.clone()]);
    let added = git_text(
        &forge.remote(),
        &[
            "diff-tree",
            "-r",
            "--no-commit-id",
            "--name-status",
            &first,
            &second,
        ],
    );
    let after = forge.tree(&second);
    let new: Vec<&String> = after.iter().filter(|path| !before.contains(path)).collect();
    assert_eq!(new.len(), 1, "{after:?}");
    assert_eq!(
        added,
        format!("A\t{}", new[0]),
        "one file is added, none changed"
    );
    assert!(new[0].starts_with("gate/workspace-tests/"), "{}", new[0]);
}

#[test]
fn a_push_the_forge_refuses_fails_the_job_and_names_the_refusal() {
    let forge = Forge::new(Some(
        "#!/bin/sh\necho 'refused by a rule of the forge' >&2\nexit 1\n",
    ));
    let (passed, printed) = forge.run(FIRST_RUN, FIRST_COMMIT, 0);
    assert!(!passed, "{printed}");
    assert!(
        printed.contains("refused by a rule of the forge"),
        "{printed}"
    );
    assert!(
        printed.contains(SERIES_REF),
        "the failure names the ref: {printed}"
    );
    assert_eq!(forge.head(), None);
    assert_eq!(forge.count_pushes(), 1, "a refused push is not tried again");
}

#[test]
fn a_push_behind_another_append_is_appended_again_once_and_then_fails() {
    let forge = Forge::new(None);
    assert!(forge.run(FIRST_RUN, FIRST_COMMIT, 0).0);
    forge.change_a_record("7.250");
    let (passed, printed) = forge.run(SECOND_RUN, SECOND_COMMIT, 1);
    assert!(passed, "{printed}");
    assert_eq!(forge.count_pushes(), 2, "{printed}");
    let head = forge.head().unwrap();
    let parent = forge.parents(&head);
    assert_eq!(
        git_text(&forge.remote(), &["log", "-1", "--format=%s", &parent[0]]),
        "concurrent 1"
    );
    let tree = forge.tree(&head);
    assert!(tree.contains(&"concurrent-1".to_owned()), "{tree:?}");
    for path in forge.records_in_series() {
        assert!(tree.contains(&path), "{path}: {tree:?}");
    }

    forge.change_a_record("6.500");
    let (passed, printed) = forge.run("45000000303", "3333333333333333333333333333333333333333", 2);
    assert!(!passed, "{printed}");
    assert_eq!(
        forge.count_pushes(),
        2,
        "one append again, no more: {printed}"
    );
    let head = forge.head().unwrap();
    assert_eq!(
        git_text(&forge.remote(), &["log", "-1", "--format=%s", &head]),
        "concurrent 2"
    );
}
