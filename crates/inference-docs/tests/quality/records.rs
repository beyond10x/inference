//! The records a check run writes: `task quality-records` takes the assessments it writes a
//! record for from `inference-docs junit-assessments`, which reads them from the plan files, and
//! `task check` runs it after `task test-run`, whose JUnit report it reads. The list is held
//! against the plan files of the tree and against plans of its own; the commands of the task run
//! with `sh` and stand-ins of `cargo` and `git`.

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

use super::{entries, load, problems, record_problems, report, text, yaml};
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

/// What `task check` runs, and what of the task the run of its commands below does not hold:
/// that `check` runs it after `test-run`, and that it names no assessment itself.
#[test]
fn check_runs_the_records_task_after_test_run_and_the_task_names_no_assessment() {
    let check = task_commands("check");
    let at = |task: &str| check.iter().position(|command| command == task);
    assert!(
        at("test-run").is_some_and(|test_run| at("quality-records") > Some(test_run)),
        "task check runs quality-records after test-run: {check:?}"
    );
    let script = task_commands("quality-records").join("\n");
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

/// The commands of `task` in `Taskfile.yml`: a line, or the name of a task it calls.
fn task_commands(task: &str) -> Vec<String> {
    let documents = load();
    let mut problems = Vec::new();
    let taskfile = yaml("Taskfile.yml", &documents.taskfile, &mut problems).expect("it parses");
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
}

/// A directory of its own under the target's temporary directory, empty.
fn scratch(name: &str) -> PathBuf {
    let directory =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).expect("the scratch directory is made");
    directory
}

/// A stand-in for `cargo`: it logs its arguments to `calls.log`; for `junit-assessments` it
/// prints `list.txt`; for `assess` it exits 1, 2 or 3 for the assessments named so, and
/// otherwise writes `--out`.
const STUB_CARGO: &str = "#!/bin/sh\nprintf '%s\\n' \"$*\" >> calls.log\ncase \" $* \" in *' junit-assessments '*) cat list.txt; exit 0;; esac\nprev=''; assessment=''; out=''\nfor argument do\n  case \"$prev\" in --assessment) assessment=$argument;; --out) out=$argument;; esac\n  prev=$argument\ndone\ncase \"$assessment\" in refused-one) exit 1;; wrong-call) exit 2;; unreadable) exit 3;; esac\nprintf '{}' > \"$out\"\n";
/// A stand-in for `git`: it logs its arguments to `git.log` and answers the commit of the run.
const STUB_GIT: &str =
    "#!/bin/sh\nprintf '%s\\n' \"$*\" >> git.log\necho 0123456789abcdef0123456789abcdef01234567\n";

/// What one run of the commands of `task quality-records` left: the exit status, stderr and the
/// calls of `cargo` and of `git`.
struct TaskRun {
    status: Option<i32>,
    stderr: String,
    calls: Vec<String>,
    git: Vec<String>,
}

impl TaskRun {
    /// The call of `assess` for the assessment `id`.
    #[track_caller]
    fn call(&self, id: &str) -> &str {
        self.calls
            .iter()
            .find(|call| call.contains(&format!("--assessment {id} ")))
            .unwrap_or_else(|| panic!("no call for {id}: {:?}", self.calls))
    }
}

/// Runs the commands of `task quality-records` in `directory`, each with `sh` in order and the
/// run stopped at the first that fails, as go-task runs them, with the stubs of `cargo` and
/// `git` first on the path, `list` as what `junit-assessments` prints, and the variables of a
/// forge run when `forge`. go-task runs a command in its own shell interpreter; `sh` stands in
/// for it here.
fn run_the_records_task(directory: &Path, list: &str, forge: bool) -> TaskRun {
    let bin = directory.join("bin");
    fs::create_dir_all(&bin).expect("bin");
    for (name, text) in [("cargo", STUB_CARGO), ("git", STUB_GIT)] {
        let file = bin.join(name);
        fs::write(&file, text).expect("the stub is written");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755))
            .expect("the stub is executable");
    }
    fs::write(directory.join("list.txt"), list).expect("the list is written");
    let _ = fs::remove_file(directory.join("calls.log"));
    let _ = fs::remove_file(directory.join("git.log"));
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut status = Some(0);
    let mut stderr = String::new();
    for command in task_commands("quality-records") {
        let mut shell = Command::new("sh");
        shell
            .arg("-c")
            .arg(&command)
            .current_dir(directory)
            .env("PATH", &path)
            .stdin(Stdio::null());
        for name in ["GITHUB_SERVER_URL", "GITHUB_REPOSITORY", "GITHUB_RUN_ID"] {
            shell.env_remove(name);
        }
        if forge {
            shell
                .env("GITHUB_SERVER_URL", "https://github.com")
                .env("GITHUB_REPOSITORY", "beyond10x/inference")
                .env("GITHUB_RUN_ID", "7");
        }
        let output = shell.output().expect("sh runs");
        stderr.push_str(&String::from_utf8_lossy(&output.stderr));
        status = output.status.code();
        if status != Some(0) {
            break;
        }
    }
    let log = |name: &str| -> Vec<String> {
        fs::read_to_string(directory.join(name))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    };
    TaskRun {
        status,
        stderr,
        calls: log("calls.log"),
        git: log("git.log"),
    }
}

/// What the task does: it calls the program for the list and once per listed assessment from
/// the report of `task test-run`; a refused assessment of any exit status is named and fails the
/// task; a written one is written under its plan; `--revision` goes only to a target of the kind
/// revision; the class and source are the forge run's on the forge and the report's elsewhere; a
/// record of an earlier run does not stay; an empty list fails.
#[test]
fn the_records_task_writes_each_listed_record_and_fails_naming_every_refused_one() {
    let directory = scratch("records-task");
    let stale = directory.join("target/quality-records/gate/stale.evidence.json");
    fs::create_dir_all(stale.parent().unwrap()).unwrap();
    fs::write(&stale, "{}").unwrap();
    let list = "gate ok-one revision\ngate refused-one revision\nadapters doc-one document\ngate wrong-call revision\nadapters unreadable document\n";
    let run = run_the_records_task(&directory, list, false);
    assert_ne!(run.status, Some(0), "stderr: {}", run.stderr);
    for refused in ["refused-one", "wrong-call", "unreadable"] {
        assert!(
            run.stderr.contains(refused),
            "{refused} is not named: {}",
            run.stderr
        );
    }
    assert!(!stale.exists(), "the record of an earlier run stays");
    for written in [
        "target/quality-records/gate/ok-one.evidence.json",
        "target/quality-records/adapters/doc-one.evidence.json",
    ] {
        assert!(
            directory.join(written).is_file(),
            "{written} is not written"
        );
    }
    assert_eq!(
        run.calls.first().map(String::as_str),
        Some("run -q -p inference-docs -- junit-assessments"),
        "the list comes from the program"
    );
    assert_eq!(
        run.calls.len(),
        6,
        "one call per listed assessment: {:?}",
        run.calls
    );
    for call in &run.calls[1..] {
        assert!(
            call.starts_with("run -q -p inference-docs -- assess --project inference "),
            "{call}"
        );
        assert!(call.contains(" --root . "), "{call}");
    }
    let ok = run.call("ok-one");
    for part in [
        "--class local",
        "--source target/nextest/default/junit.xml",
        "--from target/nextest/default/junit.xml",
        "--out target/quality-records/gate/ok-one.evidence.json",
        "--revision 0123456789abcdef0123456789abcdef01234567",
    ] {
        assert!(ok.contains(part), "{ok}");
    }
    assert_eq!(
        run.git,
        ["rev-parse HEAD"],
        "the revision is the commit of HEAD, asked of git once"
    );
    assert!(
        !run.call("doc-one").contains("--revision"),
        "{}",
        run.call("doc-one")
    );

    let forge = run_the_records_task(&directory, "gate ok-one revision\n", true);
    assert_eq!(forge.status, Some(0), "stderr: {}", forge.stderr);
    let ok = forge.call("ok-one");
    assert!(ok.contains("--class ci"), "{ok}");
    assert!(
        ok.contains("--source https://github.com/beyond10x/inference/actions/runs/7"),
        "{ok}"
    );

    let empty = run_the_records_task(&directory, "", false);
    assert_ne!(empty.status, Some(0), "an empty list passes");
    assert!(
        empty.stderr.contains("listed no assessment"),
        "{}",
        empty.stderr
    );
}

/// The filters of `junit-assessments` that no assessment of the tree reaches: an assessment of
/// `task test-run` under another profile, one of `printed-duration@1` and one of `command-run@1`
/// are not listed; one of `nextest-run@1` without a profile is; a listed one without a target
/// kind and a root without `ratchet.toml` exit 3.
#[test]
fn junit_assessments_leaves_out_another_profile_and_the_assessors_of_no_report() {
    let directory = scratch("junit-assessments");
    fs::create_dir_all(directory.join("quality")).unwrap();
    fs::write(
        directory.join("ratchet.toml"),
        "[spec]\npaths = [\"quality/x.ratchet.yaml\"]\n",
    )
    .unwrap();
    let run = |plan: &str| {
        fs::write(directory.join("quality/x.ratchet.yaml"), plan).unwrap();
        Command::new(env!("CARGO_BIN_EXE_inference-docs"))
            .arg("junit-assessments")
            .arg("--root")
            .arg(&directory)
            .output()
            .expect("the program runs")
    };
    let observation = "observation: {command: task test-run}";
    let plan = format!(
        "assessments:\n  slow-profile:\n    assessor: nextest-run@1\n    inputs: {{profile: slow}}\n    target: {{kind: revision}}\n    {observation}\n  stack-profile:\n    assessor: package-tests@1\n    inputs: {{profile: stack}}\n    target: {{kind: revision}}\n    {observation}\n  printed:\n    assessor: printed-duration@1\n    target: {{kind: build}}\n    {observation}\n  step:\n    assessor: command-run@1\n    target: {{kind: revision}}\n    {observation}\n  no-profile:\n    assessor: nextest-run@1\n    target: {{kind: revision}}\n    {observation}\n  named:\n    assessor: named-tests@1\n    target: {{kind: document}}\n    {observation}\n"
    );
    let output = run(&plan);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "x no-profile revision\nx named document\n"
    );
    let output = run(&format!(
        "assessments:\n  kindless:\n    assessor: nextest-run@1\n    {observation}\n"
    ));
    assert_eq!(output.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&output.stderr).contains("kindless has no target kind"));
    fs::remove_file(directory.join("ratchet.toml")).unwrap();
    assert_eq!(run(&plan).status.code(), Some(3));
}

/// A record that holds no result for a measure of its assessment, or for one package of a
/// measure with one value per package: `problems()` refuses it through the entries and the
/// sections of the pages ("has no value for the measure", "no result of .. for <adapter>"), and
/// the record rules refuse it from the plan alone, since the output of each measure is what the
/// plan asks of the record. The committed records hold every rule of a record.
#[test]
fn record_problems_refuses_a_record_without_the_result_of_a_measure() {
    assert!(
        record_problems(&load()).is_empty(),
        "{}",
        report(&record_problems(&load()))
    );
    let mut documents = load();
    let drop_slowest = |result: &Value| result["output"] == "slowest_test";
    let drop_aws_slowest =
        |result: &Value| result["output"] == "slowest_test" && result["item"] == "inference-aws";
    for (name, drop) in [
        ("workspace-tests", &drop_slowest as &dyn Fn(&Value) -> bool),
        ("adapter-tests", &drop_aws_slowest),
    ] {
        let path = format!("quality/records/{name}.evidence.json");
        let mut record: Value = serde_json::from_str(&documents.records[&path]).unwrap();
        record
            .pointer_mut("/evidence/results")
            .and_then(Value::as_array_mut)
            .expect("the record has results")
            .retain(|result| !drop(result));
        documents
            .records
            .insert(path, serde_json::to_string_pretty(&record).unwrap());
    }
    let all = problems(&documents);
    for expected in [
        "the record of workspace-tests has no value for the measure",
        "quality/records/adapter-tests.evidence.json: no result of `adapter-tests/slowest-test` for inference-aws",
    ] {
        assert!(
            all.iter().any(|problem| problem.contains(expected)),
            "problems() does not name {expected:?}: {}",
            report(&all)
        );
    }
    let found = record_problems(&documents);
    for expected in [
        "quality/records/workspace-tests.evidence.json: no result of `workspace-tests/slowest-test`",
        "quality/records/adapter-tests.evidence.json: no result of `adapter-tests/slowest-test` for the package inference-aws",
    ] {
        assert!(
            found.iter().any(|problem| problem.contains(expected)),
            "record_problems() does not name {expected:?}: {}",
            report(&found)
        );
    }
}
