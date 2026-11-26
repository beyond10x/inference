//! `inference-docs adopt-records`, the program that copies the records of a check run on `main`
//! into `quality/records/`, held to the fixture artifact under `tests/fixtures/adopt/`: the
//! committed records of the assessments `inference-docs junit-assessments` lists, laid out as the
//! artifact `quality-records-<commit>` holds them, with another run and commit, and the output of
//! `gh run view` for that run in `run.json`. Nothing is downloaded.
//!
//! Every case runs the program on a place of its own under the target's scratch directory: a
//! root with `ratchet.toml`, the plan files, the assessor declarations, the committed records and
//! the documents the document targets name, copied from the tree; the artifact; and `run.json`.
//! A case that refuses holds the root's records to be the committed ones, byte for byte.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

use super::{load, record_problems, report};
use crate::root;

/// The run and the commit of the fixture artifact.
const RUN: &str = "45000000017";
const COMMIT: &str = "abcdef0123456789abcdef0123456789abcdef01";
const RECORDS: &str = "quality/records";
/// What the program names a temporary file of a record after: `.<file name>` and this.
const TEMPORARY: &str = ".adopting";

fn fixture() -> PathBuf {
    root().join("crates/inference-docs/tests/fixtures/adopt")
}

/// The files under `directory`, by their path from it with `/`.
fn files_under(directory: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut found = BTreeMap::new();
    let mut open = vec![(directory.to_path_buf(), String::new())];
    while let Some((at, prefix)) = open.pop() {
        for entry in fs::read_dir(&at).expect("the directory is read") {
            let entry = entry.expect("an entry of the directory");
            let name = entry.file_name().into_string().expect("a name in UTF-8");
            if entry.path().is_dir() {
                open.push((entry.path(), format!("{prefix}{name}/")));
            } else {
                let bytes = fs::read(entry.path()).expect("the file is read");
                found.insert(format!("{prefix}{name}"), bytes);
            }
        }
    }
    found
}

fn place(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().expect("a file has a directory"))
        .expect("the directory is created");
    fs::write(path, bytes).expect("the file is written");
}

/// What one call of the program left.
struct Outcome {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

/// The place of one case, as the module documentation says.
struct Case {
    directory: PathBuf,
}

impl Case {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "adopt-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&directory);
        let case = Self { directory };
        let mut copied = vec!["ratchet.toml".to_owned()];
        for (from, suffix) in [
            ("quality", ".ratchet.yaml"),
            ("quality/assessors", ".assessor.yaml"),
            (RECORDS, ".evidence.json"),
        ] {
            for name in files_under(&root().join(from)).into_keys() {
                if name.ends_with(suffix) && !name.contains('/') {
                    copied.push(format!("{from}/{name}"));
                }
            }
        }
        for (path, bytes) in files_under(&fixture().join("artifact")) {
            place(&case.artifact().join(&path), &bytes);
            let record: Value = serde_json::from_slice(&bytes).expect("a fixture record is JSON");
            if let Some(document) = record
                .pointer("/evidence/target/identifiers/path")
                .and_then(Value::as_str)
            {
                copied.push(document.to_owned());
            }
        }
        for path in &copied {
            let bytes = fs::read(root().join(path)).expect("a file of the tree is read");
            place(&case.root().join(path), &bytes);
        }
        let run = fs::read(fixture().join("run.json")).expect("the fixture run.json is read");
        place(&case.run_json(), &run);
        case
    }

    fn root(&self) -> PathBuf {
        self.directory.join("root")
    }

    fn artifact(&self) -> PathBuf {
        self.directory.join("artifact")
    }

    fn run_json(&self) -> PathBuf {
        self.directory.join("run.json")
    }

    /// The text of the record at `path` of the artifact.
    fn record(&self, path: &str) -> String {
        fs::read_to_string(self.artifact().join(path)).expect("the record is read")
    }

    /// Replaces the first `from` with `to` in the file at `path` of the artifact.
    #[track_caller]
    fn edit(&self, path: &str, from: &str, to: &str) {
        let text = self.record(path);
        assert!(text.contains(from), "{path} holds {from:?}");
        fs::write(self.artifact().join(path), text.replacen(from, to, 1)).unwrap();
    }

    /// Replaces `from` with `to` in `run.json`.
    #[track_caller]
    fn edit_run(&self, from: &str, to: &str) {
        let text = fs::read_to_string(self.run_json()).unwrap();
        assert!(text.contains(from), "run.json holds {from:?}");
        fs::write(self.run_json(), text.replace(from, to)).unwrap();
    }

    /// Runs `inference-docs adopt-records --run <run> --run-json <run.json> --from <artifact>
    /// --root <root>`.
    fn adopt_as(&self, run: &str) -> Outcome {
        let output = Command::new(env!("CARGO_BIN_EXE_inference-docs"))
            .arg("adopt-records")
            .arg("--run")
            .arg(run)
            .arg("--run-json")
            .arg(self.run_json())
            .arg("--from")
            .arg(self.artifact())
            .arg("--root")
            .arg(self.root())
            .output()
            .expect("the program runs");
        Outcome {
            status: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    fn adopt(&self) -> Outcome {
        self.adopt_as(RUN)
    }

    /// The records of the root are the committed ones, byte for byte, and nothing else stands
    /// there but the paths of `planted`.
    #[track_caller]
    fn untouched(&self, planted: &[&str]) {
        let committed = files_under(&root().join(RECORDS));
        let mut held = files_under(&self.root().join(RECORDS));
        held.retain(|path, _| !planted.iter().any(|plant| path.starts_with(plant)));
        assert!(
            held == committed,
            "the records of the root are not the committed ones: {:?}",
            held.keys()
                .filter(|path| committed.get(*path) != held.get(*path))
                .chain(committed.keys().filter(|path| !held.contains_key(*path)))
                .collect::<Vec<_>>()
        );
    }

    /// Ended with exit 1, a line on stderr holding both parts of each of `lines`, and the
    /// records of the root untouched.
    #[track_caller]
    fn refused(&self, outcome: &Outcome, lines: &[(&str, &str)]) {
        assert_eq!(outcome.status, Some(1), "stderr: {}", outcome.stderr);
        assert!(outcome.stdout.is_empty(), "stdout: {}", outcome.stdout);
        for (file, reason) in lines {
            assert!(
                outcome
                    .stderr
                    .lines()
                    .any(|line| line.contains(file) && line.contains(reason)),
                "no line of stderr names {file:?} with {reason:?}; stderr: {}",
                outcome.stderr
            );
        }
        self.untouched(&[]);
    }
}

/// The record of `path` of the artifact, `<plan>/<assessment>.evidence.json`, by the path of its
/// committed record.
fn committed_path(path: &str) -> String {
    let name = path.rsplit('/').next().expect("a path has a file name");
    format!("{RECORDS}/{name}")
}

/// The acceptance of the command: every record of the artifact stands in `quality/records/` as
/// the artifact holds it, the others stay, and the test of the quality documents finds no problem
/// of a record in them.
#[test]
fn the_records_of_a_run_on_main_are_adopted_byte_for_byte_and_hold_the_record_rules() {
    let case = Case::new();
    let outcome = case.adopt();
    assert_eq!(outcome.status, Some(0), "stderr: {}", outcome.stderr);
    assert!(outcome.stderr.is_empty(), "stderr: {}", outcome.stderr);
    let artifact = files_under(&case.artifact());
    assert_eq!(
        artifact.len(),
        22,
        "the fixture holds the 22 records of a run"
    );
    let written = files_under(&case.root().join(RECORDS));
    let mut documents = load();
    for (path, bytes) in &artifact {
        let committed = committed_path(path);
        let name = committed.rsplit('/').next().unwrap();
        assert!(
            written.get(name) == Some(bytes),
            "{committed} is not the record of the artifact"
        );
        assert!(
            outcome
                .stdout
                .lines()
                .any(|line| line == format!("{committed}: adopted from the run {RUN}")),
            "stdout does not say {committed} was adopted: {}",
            outcome.stdout
        );
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(text.contains(&format!("/actions/runs/{RUN}\"")));
        assert!(
            documents.records.insert(committed.clone(), text).is_some(),
            "{committed} has a committed record"
        );
    }
    assert_eq!(outcome.stdout.lines().count(), artifact.len());
    let committed = files_under(&root().join(RECORDS));
    assert_eq!(
        written.keys().collect::<Vec<_>>(),
        committed.keys().collect::<Vec<_>>(),
        "no other file stands in {RECORDS}"
    );
    for (name, bytes) in &committed {
        let adopted = artifact
            .keys()
            .any(|path| path.ends_with(&format!("/{name}")));
        if !adopted {
            assert_eq!(written.get(name), Some(bytes), "{name} stays as it is");
        }
    }
    let found = record_problems(&documents);
    assert!(found.is_empty(), "{}", report(&found));
}

#[test]
fn a_run_that_is_not_a_push_to_main_of_the_workflow_check_is_refused() {
    let case = Case::new();
    case.edit_run("\"event\":\"push\"", "\"event\":\"pull_request\"");
    case.edit_run("\"headBranch\":\"main\"", "\"headBranch\":\"feature\"");
    case.edit_run("\"workflowName\":\"check\"", "\"workflowName\":\"slow\"");
    let outcome = case.adopt();
    case.refused(
        &outcome,
        &[
            ("run.json", "event is Some(\"pull_request\")"),
            ("run.json", "headBranch is Some(\"feature\")"),
            ("run.json", "workflowName is Some(\"slow\")"),
        ],
    );

    let case = Case::new();
    let outcome = case.adopt_as("45000000018");
    case.refused(
        &outcome,
        &[(
            "run.json",
            "describes the run 45000000017, not the run 45000000018",
        )],
    );

    let case = Case::new();
    fs::remove_file(case.run_json()).unwrap();
    let outcome = case.adopt();
    case.refused(&outcome, &[("run.json", "cannot be read")]);
}

#[test]
fn a_run_id_or_a_commit_the_records_do_not_name_is_refused() {
    let case = Case::new();
    case.edit(
        "gate/workspace-tests.evidence.json",
        &format!("/runs/{RUN}"),
        "/runs/45000000099",
    );
    case.edit(
        "gate/release-statuses.evidence.json",
        &format!("\"revision\": \"{COMMIT}\""),
        "\"revision\": \"0123456789abcdef0123456789abcdef01234567\"",
    );
    case.edit(
        "adapters/aws-records.evidence.json",
        &format!("\"candidate\": \"{COMMIT}\""),
        "\"candidate\": \"0123456789abcdef0123456789abcdef01234567\"",
    );
    case.edit(
        "adapters/sherpa-onnx-stt-record.evidence.json",
        "\"digest\": \"sha256:6d8d",
        "\"digest\": \"sha256:0d8d",
    );
    let outcome = case.adopt();
    case.refused(
        &outcome,
        &[
            (
                "gate/workspace-tests.evidence.json",
                &format!("not the run {RUN}"),
            ),
            (
                "gate/release-statuses.evidence.json",
                &format!(
                    "the revision is Some(\"0123456789abcdef0123456789abcdef01234567\"), the commit of the run is {COMMIT}"
                ),
            ),
            (
                "adapters/aws-records.evidence.json",
                "the candidate is Some(\"0123456789abcdef0123456789abcdef01234567\")",
            ),
            (
                "adapters/sherpa-onnx-stt-record.evidence.json",
                "the digest of the target is Some(\"sha256:0d8d",
            ),
        ],
    );

    let case = Case::new();
    let other = "fedcba9876543210fedcba9876543210fedcba98";
    case.edit_run(COMMIT, other);
    let outcome = case.adopt();
    case.refused(
        &outcome,
        &[
            (
                "adapters/adapter-tests.evidence.json",
                &format!("the commit of the run is {other}"),
            ),
            (
                "gate/workspace-tests.evidence.json",
                &format!("the commit of the run is {other}"),
            ),
        ],
    );
}

#[test]
fn a_missing_or_an_extra_record_against_the_list_of_junit_assessments_is_refused() {
    let case = Case::new();
    let artifact = case.artifact();
    fs::remove_file(artifact.join("adapters/phonic-records.evidence.json")).unwrap();
    fs::rename(
        artifact.join("gate/workspace-tests.evidence.json"),
        artifact.join("adapters/workspace-tests.evidence.json"),
    )
    .unwrap();
    place(&artifact.join("gate/notes.txt"), b"a note\n");
    let lint = fs::read(root().join(RECORDS).join("lint.evidence.json")).unwrap();
    place(&artifact.join("gate/lint.evidence.json"), &lint);
    place(
        &artifact.join("extra/release-statuses.evidence.json"),
        case.record("gate/release-statuses.evidence.json")
            .as_bytes(),
    );
    let outcome = case.adopt();
    let extra = "is no record of an assessment `inference-docs junit-assessments` lists";
    case.refused(
        &outcome,
        &[
            ("adapters/phonic-records.evidence.json", "is missing"),
            ("gate/workspace-tests.evidence.json", "is missing"),
            ("adapters/workspace-tests.evidence.json", extra),
            ("gate/notes.txt", extra),
            ("gate/lint.evidence.json", extra),
            ("artifact/extra:", extra),
            ("extra/release-statuses.evidence.json", extra),
        ],
    );
}

#[test]
fn a_record_of_another_project_assessor_or_class_than_ci_is_refused() {
    let case = Case::new();
    case.edit(
        "gate/workspace-tests.evidence.json",
        "\"project\": \"inference\"",
        "\"project\": \"elsewhere\"",
    );
    case.edit(
        "adapters/aws-records.evidence.json",
        "\"assessor\": \"named-tests@1\"",
        "\"assessor\": \"package-tests@1\"",
    );
    case.edit(
        "gate/release-statuses.evidence.json",
        "\"class\": \"ci\"",
        "\"class\": \"local\"",
    );
    case.edit(
        "adapters/whispercpp-stt-record.evidence.json",
        "\"class\": \"ci\"",
        "\"class\": \"nightly\"",
    );
    let outcome = case.adopt();
    case.refused(
        &outcome,
        &[
            (
                "gate/workspace-tests.evidence.json",
                "project is Some(\"elsewhere\"), the specification's is \"inference\"",
            ),
            (
                "adapters/aws-records.evidence.json",
                "the assessor is \"package-tests@1\", the assessment's is Some(\"named-tests@1\")",
            ),
            (
                "gate/release-statuses.evidence.json",
                "the class is \"local\"; a record of a run on the forge has the class \"ci\"",
            ),
            (
                "adapters/whispercpp-stt-record.evidence.json",
                "the class is Some(\"nightly\"), not one of local, ci",
            ),
        ],
    );
}

#[test]
fn a_record_whose_source_assess_would_refuse_is_refused() {
    let case = Case::new();
    let location = format!("https://github.com/beyond10x/inference/actions/runs/{RUN}");
    case.edit(
        "gate/workspace-tests.evidence.json",
        &location,
        &format!("https://token@github.com/beyond10x/inference/actions/runs/{RUN}"),
    );
    case.edit(
        "adapters/deepgram-records.evidence.json",
        &location,
        &format!("https://github.com/beyond10x/inference/actions/../runs/{RUN}"),
    );
    case.edit(
        "adapters/aws-records.evidence.json",
        &location,
        "target/nextest/default/junit.xml",
    );
    case.edit(
        "adapters/cartesia-records.evidence.json",
        "\"kind\": \"forge_runs\"",
        "\"kind\": \"junit\"",
    );
    let outcome = case.adopt();
    case.refused(
        &outcome,
        &[
            (
                "gate/workspace-tests.evidence.json",
                "assess refuses its source",
            ),
            ("gate/workspace-tests.evidence.json", "user info"),
            (
                "adapters/deepgram-records.evidence.json",
                "assess refuses its source",
            ),
            (
                "adapters/aws-records.evidence.json",
                "which is no run on the forge",
            ),
            (
                "adapters/aws-records.evidence.json",
                "the source kind is Some(\"forge_runs\"), and assess writes \"junit\"",
            ),
            (
                "adapters/cartesia-records.evidence.json",
                "the source kind is Some(\"junit\"), and assess writes \"forge_runs\"",
            ),
        ],
    );
}

/// `text`, a record, parsed, changed by `change` and written again.
fn changed_record(text: &str, change: impl FnOnce(&mut Value)) -> String {
    let mut record: Value = serde_json::from_str(text).unwrap();
    change(&mut record);
    serde_json::to_string_pretty(&record).unwrap()
}

fn results(record: &mut Value) -> &mut Vec<Value> {
    record
        .pointer_mut("/evidence/results")
        .and_then(Value::as_array_mut)
        .expect("the record has results")
}

/// A defect of one rule of a record: the record of the artifact it is planted in, the change
/// of its text, and the words `record_problems` and the program both name it with.
type Plant = (&'static str, fn(&str) -> String, &'static str);

/// One plant for each rule `record_problems` holds a record to.
const PLANTS: [Plant; 14] = [
    (
        "adapters/phonic-records.evidence.json",
        |_| "{ not JSON".to_owned(),
        "is not valid JSON",
    ),
    (
        "adapters/deepslate-records.evidence.json",
        |text| {
            changed_record(text, |record| {
                record["format"] = "ratchet-evidence/2".into()
            })
        },
        "format is Some(\"ratchet-evidence/2\"), not \"ratchet-evidence/1\"",
    ),
    (
        "adapters/elevenlabs-records.evidence.json",
        |text| {
            changed_record(text, |record| {
                record["evidence"]["project"] = "elsewhere".into();
            })
        },
        "project is Some(\"elsewhere\"), the specification's is \"inference\"",
    ),
    (
        "adapters/openai-records.evidence.json",
        |text| {
            changed_record(text, |record| {
                record["evidence"]["assessment"] = "phonic-records".into();
            })
        },
        "is the record of \"phonic-records\", which is not its file name",
    ),
    (
        "adapters/llama-server-records.evidence.json",
        |text| {
            changed_record(text, |record| {
                record["evidence"]["assessor"] = "package-tests@1".into();
            })
        },
        "the assessor is \"package-tests@1\", the assessment's is Some(\"named-tests@1\")",
    ),
    (
        "gate/release-statuses.evidence.json",
        |text| {
            changed_record(text, |record| {
                results(record)[0]["output"] = "unheard".into()
            })
        },
        "named-tests@1 declares no output \"unheard\"",
    ),
    (
        "gate/workspace-tests.evidence.json",
        |text| {
            changed_record(text, |record| {
                for result in results(record) {
                    result["unit"] = "ms".into();
                }
            })
        },
        "the unit of slowest_test is Some(\"ms\"), nextest-run@1 declares Some(\"s\")",
    ),
    (
        "adapters/cartesia-records.evidence.json",
        |text| {
            changed_record(text, |record| {
                record["evidence"]["coverage"]["skipped"] = 3.into();
            })
        },
        "the coverage has Some(3) for `skipped`, a count named-tests@1 does not report",
    ),
    (
        "gate/workspace-tests.evidence.json",
        |text| {
            changed_record(text, |record| {
                results(record).retain(|result| result["output"] != "slowest_test");
            })
        },
        "no result of `workspace-tests/slowest-test`",
    ),
    (
        "adapters/adapter-tests.evidence.json",
        |text| {
            changed_record(text, |record| {
                results(record).retain(|result| {
                    result["output"] != "slowest_test" || result["item"] != "inference-aws"
                });
            })
        },
        "no result of `adapter-tests/slowest-test` for the package inference-aws",
    ),
    (
        "adapters/adapter-tests.evidence.json",
        |text| {
            changed_record(text, |record| {
                for result in results(record) {
                    if result["output"] == "tests_run" && result["item"] == "inference-aws" {
                        result["item"] = "inference-elsewhere".into();
                    }
                }
            })
        },
        "the `tests_run` results are of",
    ),
    (
        "adapters/adapter-tests.evidence.json",
        |text| {
            changed_record(text, |record| {
                let executed = record["evidence"]["coverage"]["executed"].as_u64().unwrap();
                record["evidence"]["coverage"]["executed"] = (executed + 1).into();
            })
        },
        "executed tests, and the `tests_run` results of the packages of assessments.adapter-tests sum to",
    ),
    (
        "adapters/aws-time-bounds.evidence.json",
        |text| {
            changed_record(text, |record| {
                results(record).remove(0);
            })
        },
        "which assessments.aws-time-bounds lists",
    ),
    (
        "adapters/deepgram-records.evidence.json",
        |text| {
            changed_record(text, |record| {
                let mut extra = results(record)[0].clone();
                extra["item"] = "crates/inference-docs/src/adapters.rs::another_test".into();
                results(record).push(extra);
            })
        },
        "for the test crates/inference-docs/src/adapters.rs::another_test, which assessments.deepgram-records does not list",
    ),
];

/// The record rules live twice: in `record_problems` of this check, and in the program, which
/// is a binary the check cannot call into. Each plant breaks one rule of `record_problems` in
/// one record of the artifact; `record_problems` must name it for that record, in the place of
/// the committed one, and the program must refuse that record with the same words, so that a
/// rule that changes in one place and not in the other fails here. Plants in different records
/// stand in one round, so a round checks several at once; a record holds one plant per round.
#[test]
fn a_record_the_record_rules_refuse_is_refused_by_the_program_in_the_same_words() {
    let mut rounds: Vec<Vec<&Plant>> = Vec::new();
    for plant in &PLANTS {
        match rounds
            .iter_mut()
            .find(|round| round.iter().all(|planted| planted.0 != plant.0))
        {
            Some(round) => round.push(plant),
            None => rounds.push(vec![plant]),
        }
    }
    assert_eq!(rounds.len(), 3, "adapter-tests holds three plants");
    for round in rounds {
        let case = Case::new();
        let mut documents = load();
        for (path, change, _) in &round {
            let text = change(&case.record(path));
            documents.records.insert(committed_path(path), text.clone());
            fs::write(case.artifact().join(path), text).unwrap();
        }
        let found = record_problems(&documents);
        for (path, _, expected) in &round {
            let committed = committed_path(path);
            assert!(
                found
                    .iter()
                    .any(|problem| problem.starts_with(&committed) && problem.contains(expected)),
                "record_problems() does not name {expected:?} for {committed}: {}",
                report(&found)
            );
        }
        let outcome = case.adopt();
        let lines: Vec<(&str, &str)> = round
            .iter()
            .map(|(path, _, expected)| (*path, *expected))
            .collect();
        case.refused(&outcome, &lines);
    }
}

/// The last assessment `inference-docs junit-assessments` lists, which the program writes last.
fn last_listed(case: &Case) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_inference-docs"))
        .arg("junit-assessments")
        .arg("--root")
        .arg(case.root())
        .output()
        .expect("the program runs");
    let list = String::from_utf8(output.stdout).unwrap();
    let last = list.lines().last().expect("the list is not empty");
    last.split(' ')
        .nth(1)
        .expect("a line names an assessment")
        .to_owned()
}

/// A write that fails, of a temporary file or of the rename onto a record, leaves the records as
/// they were: the program writes all of them or none.
#[test]
fn a_write_that_fails_leaves_every_record_as_it_was() {
    let case = Case::new();
    let last = last_listed(&case);
    let temporary = format!(".{last}.evidence.json{TEMPORARY}");
    fs::create_dir_all(case.root().join(RECORDS).join(&temporary).join("blocked")).unwrap();
    let outcome = case.adopt();
    assert_eq!(outcome.status, Some(3), "stderr: {}", outcome.stderr);
    assert!(
        outcome.stderr.contains(&format!(
            "{RECORDS}/{last}.evidence.json: its temporary file"
        )),
        "stderr: {}",
        outcome.stderr
    );
    case.untouched(&[&temporary]);

    let case = Case::new();
    let record = case
        .root()
        .join(RECORDS)
        .join(format!("{last}.evidence.json"));
    let earlier = fs::read(&record).unwrap();
    fs::remove_file(&record).unwrap();
    fs::create_dir_all(record.join("blocked")).unwrap();
    let outcome = case.adopt();
    assert_eq!(outcome.status, Some(3), "stderr: {}", outcome.stderr);
    assert!(
        outcome.stderr.contains(&format!(
            "{RECORDS}/{last}.evidence.json: cannot be replaced"
        )),
        "stderr: {}",
        outcome.stderr
    );
    fs::remove_dir_all(&record).unwrap();
    fs::write(&record, earlier).unwrap();
    case.untouched(&[]);
}
