//! What the workflows under `.github/workflows/` pin and decide, held to the files they read.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

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
