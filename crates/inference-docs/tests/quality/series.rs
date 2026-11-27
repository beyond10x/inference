//! `inference-docs series-append` and `inference-docs history`, the programs that keep and read the
//! series of quality records: the records of each check run on `main`, appended once each under
//! `<plan>/<assessment>/<SHA-256 of the bytes>.evidence.json` to a checkout of the series ref.
//!
//! Every case runs the programs on a place of its own under the target's scratch directory: the
//! fixture artifact of `tests/fixtures/adopt/` (the records of the assessments
//! `inference-docs junit-assessments` lists, laid out as the artifact `quality-records-<commit>`
//! holds them) and an empty series. The plan files are the tree's. Nothing is downloaded.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use crate::root;

/// The run and the commit of the fixture artifact.
const RUN: &str = "45000000017";
const COMMIT: &str = "abcdef0123456789abcdef0123456789abcdef01";
const WORKSPACE_TESTS: &str = "gate/workspace-tests.evidence.json";

/// The files under `directory`, by their path from it with `/`; a link is listed with no bytes.
fn files_under(directory: &Path) -> BTreeMap<String, (Vec<u8>, Option<SystemTime>)> {
    let mut found = BTreeMap::new();
    let mut open = vec![(directory.to_path_buf(), String::new())];
    while let Some((at, prefix)) = open.pop() {
        for entry in fs::read_dir(&at).expect("the directory is read") {
            let entry = entry.expect("an entry of the directory");
            let name = entry.file_name().into_string().expect("a name in UTF-8");
            let kind = entry.file_type().expect("the kind of the entry");
            if kind.is_dir() {
                open.push((entry.path(), format!("{prefix}{name}/")));
            } else if kind.is_file() {
                let bytes = fs::read(entry.path()).expect("the file is read");
                let modified = entry.metadata().and_then(|meta| meta.modified()).ok();
                found.insert(format!("{prefix}{name}"), (bytes, modified));
            } else {
                found.insert(format!("{prefix}{name}"), (Vec::new(), None));
            }
        }
    }
    found
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The path in the series of the record at `path` of an artifact, with `bytes`.
fn in_series(path: &str, bytes: &[u8]) -> String {
    let stem = path
        .strip_suffix(".evidence.json")
        .expect("a record is named <assessment>.evidence.json");
    format!("{stem}/{}.evidence.json", digest(bytes))
}

/// What one call of a program left.
struct Outcome {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

/// The place of one case.
struct Case {
    directory: PathBuf,
}

impl Case {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "series-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&directory);
        let case = Self { directory };
        let fixture = root().join("crates/inference-docs/tests/fixtures/adopt/artifact");
        for (path, (bytes, _)) in files_under(&fixture) {
            case.place(&case.artifact().join(path), &bytes);
        }
        fs::create_dir_all(case.series()).expect("the series is created");
        case
    }

    fn place(&self, path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().expect("a file has a directory"))
            .expect("the directory is created");
        fs::write(path, bytes).expect("the file is written");
    }

    fn artifact(&self) -> PathBuf {
        self.directory.join("artifact")
    }

    fn series(&self) -> PathBuf {
        self.directory.join("series")
    }

    /// The records of the artifact, by their path in it.
    fn records(&self) -> BTreeMap<String, Vec<u8>> {
        files_under(&self.artifact())
            .into_iter()
            .map(|(path, (bytes, _))| (path, bytes))
            .collect()
    }

    /// Replaces every `from` with `to` in the record at `path` of the artifact.
    #[track_caller]
    fn edit(&self, path: &str, from: &str, to: &str) {
        let at = self.artifact().join(path);
        let text = fs::read_to_string(&at).expect("the record is read");
        assert!(text.contains(from), "{path} holds {from:?}");
        fs::write(at, text.replace(from, to)).expect("the record is written");
    }

    fn call(&self, arguments: &[&str]) -> Outcome {
        let output = Command::new(env!("CARGO_BIN_EXE_inference-docs"))
            .args(arguments)
            .output()
            .expect("the program runs");
        Outcome {
            status: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    /// Runs `inference-docs series-append --from <artifact> --series <series> --root <tree>`.
    fn append(&self) -> Outcome {
        let artifact = self.artifact().display().to_string();
        let series = self.series().display().to_string();
        let tree = root().display().to_string();
        self.call(&[
            "series-append",
            "--from",
            &artifact,
            "--series",
            &series,
            "--root",
            &tree,
        ])
    }

    /// Runs `inference-docs history --series <series> --root <tree>` and `arguments`.
    fn history(&self, arguments: &[&str]) -> Outcome {
        let series = self.series().display().to_string();
        let tree = root().display().to_string();
        let mut call = vec!["history", "--series", &series, "--root", &tree];
        call.extend_from_slice(arguments);
        self.call(&call)
    }

    /// The files of the series, with their bytes and the time each was last changed.
    fn series_files(&self) -> BTreeMap<String, (Vec<u8>, Option<SystemTime>)> {
        files_under(&self.series())
    }
}

#[track_caller]
fn succeeded(outcome: &Outcome) {
    assert_eq!(
        outcome.status,
        Some(0),
        "stdout:\n{}\nstderr:\n{}",
        outcome.stdout,
        outcome.stderr
    );
}

#[test]
fn appending_an_artifact_adds_each_record_under_the_digest_of_its_bytes() {
    let case = Case::new();
    let outcome = case.append();
    succeeded(&outcome);
    let records = case.records();
    assert!(records.len() > 20, "the fixture artifact holds the records");
    let expected: BTreeMap<String, Vec<u8>> = records
        .iter()
        .map(|(path, bytes)| (in_series(path, bytes), bytes.clone()))
        .collect();
    let found: BTreeMap<String, Vec<u8>> = case
        .series_files()
        .into_iter()
        .map(|(path, (bytes, _))| (path, bytes))
        .collect();
    assert_eq!(
        found.keys().collect::<Vec<_>>(),
        expected.keys().collect::<Vec<_>>()
    );
    assert!(found == expected, "every record is written byte for byte");
    for path in expected.keys() {
        assert!(
            outcome
                .stdout
                .lines()
                .any(|line| line == format!("{path}: added")),
            "the program names {path} as added: {}",
            outcome.stdout
        );
    }
}

#[test]
fn appending_the_same_artifact_again_adds_nothing_and_changes_no_file() {
    let case = Case::new();
    succeeded(&case.append());
    let before = case.series_files();
    let outcome = case.append();
    succeeded(&outcome);
    assert_eq!(case.series_files(), before, "no file of the series changed");
    assert!(
        !outcome.stdout.contains(": added"),
        "nothing is added: {}",
        outcome.stdout
    );
    for path in before.keys() {
        assert!(
            outcome
                .stdout
                .lines()
                .any(|line| line == format!("{path}: in the series already")),
            "the program names {path} as there already: {}",
            outcome.stdout
        );
    }
}

#[test]
fn a_second_artifact_adds_only_the_records_whose_bytes_differ() {
    let case = Case::new();
    succeeded(&case.append());
    let before = case.series_files();
    case.edit(
        WORKSPACE_TESTS,
        "\"value\": \"8.011\"",
        "\"value\": \"7.250\"",
    );
    let changed = fs::read(case.artifact().join(WORKSPACE_TESTS)).unwrap();
    let outcome = case.append();
    succeeded(&outcome);
    let after = case.series_files();
    let added: Vec<&String> = after
        .keys()
        .filter(|path| !before.contains_key(*path))
        .collect();
    let new = in_series(WORKSPACE_TESTS, &changed);
    assert_eq!(added, vec![&new], "one record is added");
    assert_eq!(after[&new].0, changed);
    for (path, file) in &before {
        assert_eq!(after.get(path), Some(file), "{path} is unchanged");
    }
    let lines: Vec<&str> = outcome
        .stdout
        .lines()
        .filter(|line| line.ends_with(": added"))
        .collect();
    assert_eq!(lines, vec![format!("{new}: added")]);
}

/// What plants a defect into the artifact of a case.
type Plant = Box<dyn Fn(&Case)>;

/// The defects of an artifact the program refuses: the path the refusal names and what plants
/// the defect.
fn plants() -> Vec<(&'static str, Plant)> {
    vec![
        (
            "gate/unknown.evidence.json",
            Box::new(|case: &Case| {
                let bytes = fs::read(case.artifact().join(WORKSPACE_TESTS)).unwrap();
                case.place(&case.artifact().join("gate/unknown.evidence.json"), &bytes);
            }),
        ),
        (
            "notes.txt",
            Box::new(|case: &Case| case.place(&case.artifact().join("notes.txt"), b"notes\n")),
        ),
        (
            "adapters/workspace-tests.evidence.json",
            Box::new(|case: &Case| {
                let bytes = fs::read(case.artifact().join(WORKSPACE_TESTS)).unwrap();
                case.place(
                    &case
                        .artifact()
                        .join("adapters/workspace-tests.evidence.json"),
                    &bytes,
                );
            }),
        ),
        (
            "gate/extra",
            Box::new(|case: &Case| fs::create_dir_all(case.artifact().join("gate/extra")).unwrap()),
        ),
        (
            "gate/release-statuses.evidence.json",
            Box::new(|case: &Case| {
                let path = case.artifact().join("gate/release-statuses.evidence.json");
                fs::remove_file(&path).unwrap();
                std::os::unix::fs::symlink(case.artifact().join(WORKSPACE_TESTS), path).unwrap();
            }),
        ),
        (
            WORKSPACE_TESTS,
            Box::new(|case: &Case| {
                case.edit(
                    WORKSPACE_TESTS,
                    "\"assessment\": \"workspace-tests\"",
                    "\"assessment\": \"slow-lane\"",
                );
            }),
        ),
        (
            WORKSPACE_TESTS,
            Box::new(|case: &Case| {
                case.edit(
                    WORKSPACE_TESTS,
                    "\"format\": \"ratchet-evidence/1\"",
                    "\"format\": \"other/1\"",
                );
            }),
        ),
        (
            WORKSPACE_TESTS,
            Box::new(|case: &Case| {
                case.edit(
                    WORKSPACE_TESTS,
                    "\"project\": \"inference\"",
                    "\"project\": \"other\"",
                );
            }),
        ),
        (
            WORKSPACE_TESTS,
            Box::new(|case: &Case| {
                case.place(&case.artifact().join(WORKSPACE_TESTS), b"{\"format\": ");
            }),
        ),
    ]
}

#[test]
fn a_file_that_is_no_record_of_a_listed_assessment_is_refused_and_nothing_is_written() {
    for (index, (named, plant)) in plants().into_iter().enumerate() {
        let case = Case::new();
        succeeded(&case.append());
        case.edit(
            WORKSPACE_TESTS,
            "\"value\": \"8.011\"",
            "\"value\": \"7.250\"",
        );
        let before = case.series_files();
        plant(&case);
        let outcome = case.append();
        assert_eq!(
            outcome.status,
            Some(1),
            "plant {index} ({named}) is refused: {}{}",
            outcome.stdout,
            outcome.stderr
        );
        let at = case.artifact().join(named).display().to_string();
        assert!(
            outcome
                .stderr
                .lines()
                .any(|line| line.starts_with(&format!("inference-docs series-append: {at}: "))),
            "plant {index}: the refusal names {at}: {}",
            outcome.stderr
        );
        assert_eq!(
            case.series_files(),
            before,
            "plant {index} ({named}): nothing is written"
        );
        assert!(
            outcome.stdout.is_empty(),
            "plant {index}: {}",
            outcome.stdout
        );
    }
}

#[test]
fn a_file_of_the_series_that_is_not_the_record_its_name_says_is_refused() {
    let case = Case::new();
    succeeded(&case.append());
    let bytes = fs::read(case.artifact().join(WORKSPACE_TESTS)).unwrap();
    let path = in_series(WORKSPACE_TESTS, &bytes);
    fs::write(case.series().join(&path), b"{}\n").unwrap();
    case.edit("gate/release-statuses.evidence.json", RUN, "45000000018");
    let before = case.series_files();
    let outcome = case.append();
    assert_eq!(
        outcome.status,
        Some(1),
        "{}{}",
        outcome.stdout,
        outcome.stderr
    );
    let at = case.series().join(&path).display().to_string();
    assert!(
        outcome.stderr.contains(&format!("{at}: ")),
        "the refusal names {at}: {}",
        outcome.stderr
    );
    assert_eq!(case.series_files(), before, "nothing is written");

    let case = Case::new();
    let target = case.directory.join("elsewhere");
    fs::create_dir_all(case.series().join("gate/workspace-tests")).unwrap();
    std::os::unix::fs::symlink(&target, case.series().join(&path)).unwrap();
    let before = case.series_files();
    let outcome = case.append();
    assert_eq!(
        outcome.status,
        Some(1),
        "{}{}",
        outcome.stdout,
        outcome.stderr
    );
    assert!(
        outcome
            .stderr
            .contains(&format!("{}: ", case.series().join(&path).display())),
        "{}",
        outcome.stderr
    );
    assert_eq!(case.series_files(), before, "nothing is written");
    assert!(!target.exists(), "nothing is written through the link");
}

#[test]
fn a_wrong_call_exits_2() {
    let case = Case::new();
    let missing = case.directory.join("missing").display().to_string();
    let series = case.series().display().to_string();
    let tree = root().display().to_string();
    let outcome = case.call(&[
        "series-append",
        "--from",
        &missing,
        "--series",
        &series,
        "--root",
        &tree,
    ]);
    assert_eq!(outcome.status, Some(2), "{}", outcome.stderr);
    assert!(
        outcome.stderr.contains("--from") && outcome.stderr.contains("is no directory"),
        "{}",
        outcome.stderr
    );
    let artifact = case.artifact().display().to_string();
    let outcome = case.call(&[
        "series-append",
        "--from",
        &artifact,
        "--series",
        &missing,
        "--root",
        &tree,
    ]);
    assert_eq!(outcome.status, Some(2), "{}", outcome.stderr);
    assert!(
        outcome.stderr.contains("--series") && outcome.stderr.contains("is no directory"),
        "{}",
        outcome.stderr
    );
    assert!(!case.directory.join("missing").exists());
}

/// Three runs of `workspace-tests`, appended out of their order: the run, its commit and the
/// value of its slowest test.
const RUNS: [(&str, &str, &str); 3] = [
    (
        "45000000300",
        "3333333333333333333333333333333333333333",
        "9.500",
    ),
    (
        "45000000100",
        "1111111111111111111111111111111111111111",
        "8.011",
    ),
    (
        "45000000200",
        "2222222222222222222222222222222222222222",
        "7.250",
    ),
];

fn series_of_three_runs() -> Case {
    let case = Case::new();
    for (index, (run, commit, value)) in RUNS.iter().enumerate() {
        let previous = if index == 0 {
            (RUN, COMMIT, "8.011")
        } else {
            RUNS[index - 1]
        };
        case.edit(WORKSPACE_TESTS, previous.0, run);
        case.edit(WORKSPACE_TESTS, previous.1, commit);
        case.edit(
            WORKSPACE_TESTS,
            &format!("\"value\": \"{}\"", previous.2),
            &format!("\"value\": \"{value}\""),
        );
        succeeded(&case.append());
    }
    case
}

#[test]
fn history_lists_the_values_of_a_measure_oldest_run_first() {
    let case = series_of_three_runs();
    let outcome = case.history(&["workspace-tests", "slowest-test"]);
    succeeded(&outcome);
    assert_eq!(
        outcome.stdout,
        "45000000100 1111111111111111111111111111111111111111 slowest-test=8.011 s\n\
         45000000200 2222222222222222222222222222222222222222 slowest-test=7.250 s\n\
         45000000300 3333333333333333333333333333333333333333 slowest-test=9.500 s\n"
    );
    let all = case.history(&["workspace-tests"]);
    succeeded(&all);
    assert_eq!(all.stdout, outcome.stdout, "the one measure of the plan");
}

#[test]
fn history_names_the_item_of_each_value_of_a_measure_with_one_value_per_package() {
    let case = Case::new();
    succeeded(&case.append());
    let outcome = case.history(&["adapter-tests", "tests-run"]);
    succeeded(&outcome);
    let lines: Vec<&str> = outcome.stdout.lines().collect();
    assert_eq!(lines.len(), 1, "{}", outcome.stdout);
    assert!(
        lines[0].starts_with(&format!(
            "{RUN} {COMMIT} tests-run[inference-audio-device]=127 tests, "
        )),
        "{}",
        lines[0]
    );
    let all = case.history(&["adapter-tests"]);
    succeeded(&all);
    for measure in ["tests-run[", "tests-passed[", "slowest-test["] {
        assert!(all.stdout.contains(measure), "{measure}: {}", all.stdout);
    }
}

#[test]
fn history_of_an_assessment_with_no_record_prints_no_line_and_says_so() {
    let case = series_of_three_runs();
    let outcome = case.history(&["slow-lane"]);
    succeeded(&outcome);
    assert_eq!(outcome.stdout, "");
    assert!(
        outcome.stderr.contains("holds no record of slow-lane"),
        "{}",
        outcome.stderr
    );
}

#[test]
fn history_refuses_an_assessment_or_a_measure_the_plan_does_not_declare() {
    let case = series_of_three_runs();
    for arguments in [
        &["no-such-assessment"][..],
        &["workspace-tests", "fastest-test"],
    ] {
        let outcome = case.history(arguments);
        assert_eq!(outcome.status, Some(2), "{arguments:?}: {}", outcome.stderr);
        assert_eq!(outcome.stdout, "");
    }
}

/// A link in the series where the directory of a plan or of an assessment stands is refused,
/// and nothing is written through it.
#[test]
fn a_link_in_the_series_where_a_directory_of_records_stands_is_refused() {
    for link in ["gate", "gate/workspace-tests"] {
        let case = Case::new();
        let elsewhere = case.directory.join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        let at = case.series().join(link);
        fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &at).unwrap();
        let before = case.series_files();
        let outcome = case.append();
        assert_eq!(
            outcome.status,
            Some(1),
            "{link}: {}{}",
            outcome.stdout,
            outcome.stderr
        );
        assert!(
            outcome.stderr.contains(&format!("{}: ", at.display())),
            "{link}: the refusal names the link: {}",
            outcome.stderr
        );
        assert_eq!(case.series_files(), before, "{link}: nothing is written");
        assert_eq!(
            fs::read_dir(&elsewhere).unwrap().count(),
            0,
            "{link}: nothing is written through the link"
        );
    }
}

/// A link in the artifact is refused even when it leads to a record of the assessment its name
/// says.
#[test]
fn a_link_in_the_artifact_to_a_record_of_its_own_assessment_is_refused() {
    let case = Case::new();
    let path = case.artifact().join(WORKSPACE_TESTS);
    let outside = case.directory.join("outside.evidence.json");
    fs::rename(&path, &outside).unwrap();
    std::os::unix::fs::symlink(&outside, &path).unwrap();
    let outcome = case.append();
    assert_eq!(
        outcome.status,
        Some(1),
        "{}{}",
        outcome.stdout,
        outcome.stderr
    );
    assert!(
        outcome.stderr.contains(&format!("{}: ", path.display())),
        "{}",
        outcome.stderr
    );
    assert!(case.series_files().is_empty(), "nothing is written");
}

/// A record that cannot be written ends the call with 3, and the records the call wrote before
/// it are removed again. The records are written in the order of their paths, so
/// `gate/workspace-tests` comes last, after every record of `adapters`.
#[test]
fn a_record_that_cannot_be_written_leaves_no_record_of_the_call() {
    use std::os::unix::fs::PermissionsExt;
    let case = Case::new();
    let locked = case.series().join("gate/workspace-tests");
    fs::create_dir_all(&locked).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
    assert!(
        fs::write(locked.join("probe"), b"").is_err(),
        "the mode of the directory refuses a new file to this user"
    );
    let outcome = case.append();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        outcome.status,
        Some(3),
        "{}{}",
        outcome.stdout,
        outcome.stderr
    );
    assert!(
        outcome.stderr.contains("removed again"),
        "{}",
        outcome.stderr
    );
    assert_eq!(
        case.series_files().keys().collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "every record the call wrote is removed again"
    );
}

/// Two records of one run (a run attempted again) are ordered by their start, and a record
/// whose source names no forge run comes after every run. The names of the files are chosen
/// against both orders.
#[test]
fn history_orders_records_of_one_run_by_their_start_and_puts_records_of_no_run_last() {
    let case = Case::new();
    let fixture = fs::read_to_string(case.artifact().join(WORKSPACE_TESTS)).unwrap();
    let directory = case.series().join("gate/workspace-tests");
    fs::create_dir_all(&directory).unwrap();
    let location = format!("https://github.com/beyond10x/inference/actions/runs/{RUN}");
    for (file, source, started, value) in [
        (
            "b.evidence.json",
            "https://github.com/beyond10x/inference/actions/runs/45000000100",
            "2026-11-24T20:00:00Z",
            "1.000",
        ),
        (
            "a.evidence.json",
            "https://github.com/beyond10x/inference/actions/runs/45000000100",
            "2026-11-24T21:00:00Z",
            "2.000",
        ),
        (
            "0.evidence.json",
            "target/nextest/default/junit.xml",
            "2026-11-24T19:00:00Z",
            "3.000",
        ),
        (
            "c.evidence.json",
            "https://github.com/beyond10x/inference/actions/runs/45000000050",
            "2026-11-24T22:00:00Z",
            "0.500",
        ),
    ] {
        let text = fixture
            .replace(&location, source)
            .replace("2026-11-24T20:02:05Z", started)
            .replace("\"value\": \"8.011\"", &format!("\"value\": \"{value}\""));
        fs::write(directory.join(file), text).unwrap();
    }
    let outcome = case.history(&["workspace-tests", "slowest-test"]);
    succeeded(&outcome);
    assert_eq!(
        outcome.stdout,
        format!(
            "45000000050 {COMMIT} slowest-test=0.500 s\n\
             45000000100 {COMMIT} slowest-test=1.000 s\n\
             45000000100 {COMMIT} slowest-test=2.000 s\n\
             target/nextest/default/junit.xml {COMMIT} slowest-test=3.000 s\n"
        )
    );
}
