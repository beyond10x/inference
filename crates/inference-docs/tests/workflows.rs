//! What the workflows under `.github/workflows/` pin and decide, held to the files they read.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

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
