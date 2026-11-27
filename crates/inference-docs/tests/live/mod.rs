//! `task quality-live`, the lane that runs the keyed live measures of
//! `quality/adapters.ratchet.yaml` on this machine (`inference-docs live`), held without a hosted
//! call and without a key: a fake `secret-tool` first on `PATH` answers the attributes of the two
//! keys with fixed values that are no keys and fails for any other call, and stubs in the place of
//! the four commands (`--stubs`) print the outputs under `tests/fixtures/live/`, print the key they
//! were given and write down which key variables they saw.
//!
//! Every case runs the program on a root of its own under the target's scratch directory, with
//! the plan files, the assessor declarations and `ratchet.toml` copied from the tree and the pin
//! of the toolchain written from the fixture of `assess`, so that a new toolchain does not change
//! what a case expects.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

use crate::root;

const OPENAI_KEY: &str = "default/default/default/openai-api-key";
const ELEVENLABS_KEY: &str = "default/default/default/elevenlabs-api-key";
/// What the fake `secret-tool` prints for each key: fixed texts that are no keys.
const OPENAI_VALUE: &str = "not-a-key-openai-0001";
const ELEVENLABS_VALUE: &str = "not-a-key-elevenlabs-0002";
/// What the shell of a case holds in the two key variables before the lane starts.
const SHELL_VALUE: &str = "not-a-key-from-the-shell-0003";

/// The four measures, the key variable each command reads, and whether the lane writes a record
/// of it.
const MEASURES: [(&str, &str, bool); 4] = [
    ("openai-speech-live", "OPENAI_API_KEY", true),
    ("openai-gpt-live", "OPENAI_API_KEY", true),
    ("elevenlabs-live-example", "ELEVENLABS_API_KEY", false),
    ("elevenlabs-cancel-probe", "ELEVENLABS_API_KEY", true),
];

/// The count the lane prints before it calls anything, with a voice id.
const COUNT: &str = "\
quality-live: the hosted calls of one run, per provider; each is billed by its provider
  OpenAI, key default/default/default/openai-api-key: 4 calls
    openai-speech-live: 1 request
    openai-gpt-live: 3 sessions, billed per second of session
  ElevenLabs, key default/default/default/elevenlabs-api-key: 6 calls
    elevenlabs-live-example: 2 connections; inference-docs assess reads no record of its output, which is kept
    elevenlabs-cancel-probe: 4 connections
";

/// The count without `ELEVENLABS_VOICE_ID`.
const COUNT_WITHOUT_VOICE: &str = "\
quality-live: the hosted calls of one run, per provider; each is billed by its provider
  OpenAI, key default/default/default/openai-api-key: 4 calls
    openai-speech-live: 1 request
    openai-gpt-live: 3 sessions, billed per second of session
  ElevenLabs, key default/default/default/elevenlabs-api-key: 0 calls
    elevenlabs-live-example: skipped, ELEVENLABS_VOICE_ID is unset
    elevenlabs-cancel-probe: skipped, ELEVENLABS_VOICE_ID is unset
";

fn fixture(name: &str) -> PathBuf {
    root()
        .join("crates/inference-docs/tests/fixtures")
        .join(name)
}

fn place(directory: &Path, path: &str, text: &str) {
    let file = directory.join(path);
    fs::create_dir_all(file.parent().expect("a file has a directory"))
        .expect("the directory is created");
    fs::write(file, text).expect("the file is written");
}

fn executable(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("a file has a directory"))
        .expect("the directory is created");
    fs::write(path, text).expect("the script is written");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("the script runs");
}

/// What a key lookup of the fake `secret-tool` answers.
#[derive(Clone, Copy)]
enum Key {
    Given(&'static str),
    Empty,
    Missing,
}

/// One case: its directory, the root the lane runs on, and the files the fakes write to.
struct Case {
    dir: PathBuf,
    root: PathBuf,
}

/// What one run of the lane left.
struct Lane {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Case {
    /// A fresh case whose `secret-tool` answers `openai` and `elevenlabs`.
    fn new(openai: Key, elevenlabs: Key) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "live-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        let root = dir.join("root");
        let mut copied = vec!["ratchet.toml".to_owned()];
        for (from, suffix) in [
            ("quality", ".ratchet.yaml"),
            ("quality/assessors", ".assessor.yaml"),
        ] {
            for entry in fs::read_dir(crate::root().join(from)).expect("the directory is read") {
                let name = entry
                    .expect("an entry of the directory")
                    .file_name()
                    .into_string()
                    .expect("a name in UTF-8");
                if name.ends_with(suffix) {
                    copied.push(format!("{from}/{name}"));
                }
            }
        }
        for path in &copied {
            let text = fs::read_to_string(crate::root().join(path)).expect("a file is read");
            place(&root, path, &text);
        }
        place(
            &root,
            "rust-toolchain.toml",
            &fs::read_to_string(fixture("assess/toolchain.toml")).expect("the pin is read"),
        );
        let calls = dir.join("calls");
        let answer = |key: Key| match key {
            Key::Given(value) => format!("printf '%s' '{value}'"),
            Key::Empty => "exit 0".to_owned(),
            Key::Missing => "exit 1".to_owned(),
        };
        executable(
            &dir.join("bin/secret-tool"),
            &format!(
                "#!/bin/sh\nprintf 'secret-tool %s\\n' \"$*\" >> '{calls}'\n\
                 [ \"$#\" -eq 5 ] && [ \"$1\" = lookup ] && [ \"$2\" = service ] && [ \"$3\" = b10x-secrets ] && [ \"$4\" = username ] || exit 64\n\
                 case \"$5\" in\n\
                 {OPENAI_KEY}) {} ;;\n\
                 {ELEVENLABS_KEY}) {} ;;\n\
                 *) exit 1 ;;\n\
                 esac\n",
                answer(openai),
                answer(elevenlabs),
                calls = calls.display(),
            ),
        );
        for (measure, variable, _) in MEASURES {
            executable(
                &dir.join("stubs").join(measure),
                &format!(
                    "#!/bin/sh\nprintf '%s\\n' '{measure}' >> '{calls}'\n\
                     printf 'OPENAI_API_KEY=%s\\nELEVENLABS_API_KEY=%s\\nINFERENCE_OPENAI_KEY_SOURCE=%s\\n' \
                     \"${{OPENAI_API_KEY-<unset>}}\" \"${{ELEVENLABS_API_KEY-<unset>}}\" \"${{INFERENCE_OPENAI_KEY_SOURCE-<unset>}}\" > '{seen}'\n\
                     printf 'the key given: %s\\n' \"${{{variable}-}}\" >&2\n\
                     cat '{output}'\n",
                    calls = calls.display(),
                    seen = dir.join(format!("seen-{measure}")).display(),
                    output = fixture(&format!("live/{measure}.out")).display(),
                ),
            );
        }
        Self { dir, root }
    }

    /// Runs the lane with `YES` set to `yes` and `ELEVENLABS_VOICE_ID` to `voice`, the two key
    /// variables of the shell set to [`SHELL_VALUE`].
    fn run(&self, yes: Option<&str>, voice: Option<&str>) -> Lane {
        let path = format!(
            "{}:{}",
            self.dir.join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut command = Command::new(env!("CARGO_BIN_EXE_inference-docs"));
        command
            .arg("live")
            .arg("--root")
            .arg(&self.root)
            .arg("--stubs")
            .arg(self.dir.join("stubs"))
            .env("PATH", path)
            .env("OPENAI_API_KEY", SHELL_VALUE)
            .env("ELEVENLABS_API_KEY", SHELL_VALUE)
            .env_remove("INFERENCE_OPENAI_KEY_SOURCE")
            .env_remove("YES")
            .env_remove("ELEVENLABS_VOICE_ID");
        if let Some(yes) = yes {
            command.env("YES", yes);
        }
        if let Some(voice) = voice {
            command.env("ELEVENLABS_VOICE_ID", voice);
        }
        let output = command.output().expect("the lane runs");
        Lane {
            status: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    /// The calls the fakes wrote down: one line per `secret-tool` call and per stub started.
    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.dir.join("calls"))
            .map(|text| text.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    /// The key variables the stub of `measure` saw, by name.
    fn seen(&self, measure: &str) -> BTreeMap<String, String> {
        let text = fs::read_to_string(self.dir.join(format!("seen-{measure}")))
            .unwrap_or_else(|_| panic!("the stub of {measure} ran"));
        text.lines()
            .filter_map(|line| line.split_once('='))
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect()
    }

    /// The one run directory under `.ratchet/runs/`.
    fn run_id(&self) -> String {
        let runs: Vec<String> = fs::read_dir(self.root.join(".ratchet/runs"))
            .expect("the lane kept its outputs under .ratchet/runs")
            .map(|entry| {
                entry
                    .expect("an entry")
                    .file_name()
                    .into_string()
                    .expect("UTF-8")
            })
            .collect();
        assert_eq!(runs.len(), 1, "one run directory: {runs:?}");
        runs[0].clone()
    }

    /// The files under `.ratchet/records/adapters/<measure>/`.
    fn records(&self, measure: &str) -> Vec<PathBuf> {
        let directory = self.root.join(".ratchet/records/adapters").join(measure);
        let Ok(entries) = fs::read_dir(&directory) else {
            return Vec::new();
        };
        entries
            .map(|entry| entry.expect("an entry").path())
            .collect()
    }

    /// Every file under `.ratchet/`, with its text.
    fn kept(&self) -> Vec<(PathBuf, String)> {
        fn walk(directory: &Path, into: &mut Vec<(PathBuf, String)>) {
            let Ok(entries) = fs::read_dir(directory) else {
                return;
            };
            for entry in entries {
                let path = entry.expect("an entry").path();
                if path.is_dir() {
                    walk(&path, into);
                } else {
                    let text = String::from_utf8_lossy(&fs::read(&path).expect("a file is read"))
                        .into_owned();
                    into.push((path, text));
                }
            }
        }
        let mut kept = Vec::new();
        walk(&self.root.join(".ratchet"), &mut kept);
        kept
    }

    /// Held to have called no stub and no `secret-tool`, and to have written nothing.
    #[track_caller]
    fn called_nothing(&self) {
        assert_eq!(self.calls(), Vec::<String>::new(), "nothing is started");
        assert!(
            !self.root.join(".ratchet").exists(),
            "nothing is written under .ratchet"
        );
    }
}

impl Lane {
    #[track_caller]
    fn says(&self, text: &str) {
        assert!(
            self.stdout.contains(text),
            "the lane does not say {text:?}; stdout:\n{}\nstderr:\n{}",
            self.stdout,
            self.stderr
        );
    }
}

/// A record with its run times and run id put back to the words of the expected files, held to
/// give times in the form of a record, the end not before the start.
fn normalised(path: &Path, run: &str) -> Value {
    let text = fs::read_to_string(path).expect("the record is read");
    let mut record: Value = serde_json::from_str(&text.replace(run, "RUN")).expect("JSON");
    let started = record["run"]["started_at"].as_str().expect("a start");
    let ended = record["run"]["ended_at"].as_str().expect("an end");
    for time in [started, ended] {
        assert!(
            time.len() == 20 && time.ends_with('Z') && time.as_bytes()[10] == b'T',
            "a time of a record: {time}"
        );
    }
    assert!(started <= ended, "{started} is not after {ended}");
    record["run"]["started_at"] = Value::from("STARTED");
    record["run"]["ended_at"] = Value::from("ENDED");
    record
}

#[test]
fn without_yes_the_lane_prints_its_count_per_provider_and_calls_nothing() {
    for yes in [None, Some(""), Some("0"), Some("yes")] {
        let case = Case::new(Key::Given(OPENAI_VALUE), Key::Given(ELEVENLABS_VALUE));
        let lane = case.run(yes, Some("voice-0001"));
        assert_eq!(lane.status, Some(2), "YES={yes:?}; stderr: {}", lane.stderr);
        assert!(
            lane.stdout.starts_with(COUNT),
            "YES={yes:?}; stdout:\n{}",
            lane.stdout
        );
        lane.says("quality-live: called nothing; with the owner's yes to this count, run `YES=1 task quality-live`");
        case.called_nothing();
    }
    let case = Case::new(Key::Given(OPENAI_VALUE), Key::Given(ELEVENLABS_VALUE));
    let lane = case.run(None, None);
    assert_eq!(lane.status, Some(2), "stderr: {}", lane.stderr);
    assert!(
        lane.stdout.starts_with(COUNT_WITHOUT_VOICE),
        "stdout:\n{}",
        lane.stdout
    );
    case.called_nothing();
}

#[test]
fn with_yes_each_command_sees_only_its_own_key_and_one_record_per_read_measure_is_written() {
    let case = Case::new(Key::Given(OPENAI_VALUE), Key::Given(ELEVENLABS_VALUE));
    let lane = case.run(Some("1"), Some("voice-0001"));
    assert_eq!(
        lane.status,
        Some(0),
        "stdout:\n{}\nstderr:\n{}",
        lane.stdout,
        lane.stderr
    );
    assert!(lane.stdout.starts_with(COUNT), "stdout:\n{}", lane.stdout);
    assert_eq!(
        case.calls(),
        [
            format!("secret-tool lookup service b10x-secrets username {OPENAI_KEY}"),
            "openai-speech-live".to_owned(),
            "openai-gpt-live".to_owned(),
            format!("secret-tool lookup service b10x-secrets username {ELEVENLABS_KEY}"),
            "elevenlabs-live-example".to_owned(),
            "elevenlabs-cancel-probe".to_owned(),
        ],
        "each key is read once, before the commands of its provider"
    );
    for (measure, variable, _) in MEASURES {
        let seen = case.seen(measure);
        let (own, other, value) = if variable == "OPENAI_API_KEY" {
            ("OPENAI_API_KEY", "ELEVENLABS_API_KEY", OPENAI_VALUE)
        } else {
            ("ELEVENLABS_API_KEY", "OPENAI_API_KEY", ELEVENLABS_VALUE)
        };
        assert_eq!(seen[own], value, "{measure} sees its own key");
        assert_eq!(seen[other], "<unset>", "{measure} sees no other key");
        let source = if own == "OPENAI_API_KEY" {
            "env"
        } else {
            "<unset>"
        };
        assert_eq!(
            seen["INFERENCE_OPENAI_KEY_SOURCE"], source,
            "{measure}: the OpenAI tests read the key from the environment only when told so"
        );
    }
    let run = case.run_id();
    for (measure, _, read) in MEASURES {
        let records = case.records(measure);
        if read {
            assert_eq!(
                records,
                [case.root.join(format!(
                    ".ratchet/records/adapters/{measure}/{run}.evidence.json"
                ))],
                "one record of {measure}"
            );
            let expected: Value = serde_json::from_str(
                &fs::read_to_string(fixture(&format!("live/{measure}.evidence.json")))
                    .expect("the expected record is read"),
            )
            .expect("JSON");
            assert_eq!(
                normalised(&records[0], &run),
                expected,
                "the record of {measure}"
            );
            lane.says(&format!(
                "quality-live: {measure}: record written to .ratchet/records/adapters/{measure}/{run}.evidence.json"
            ));
        } else {
            assert_eq!(records, Vec::<PathBuf>::new(), "no record of {measure}");
            lane.says(&format!(
                "quality-live: {measure}: no record; inference-docs assess reads none of this output"
            ));
        }
        assert!(
            case.root
                .join(format!(".ratchet/runs/{run}/{measure}.out"))
                .is_file(),
            "the output of {measure} is kept"
        );
    }
    lane.says(
        "quality-live: 3 records written under .ratchet/records/, 0 measures skipped, 0 failed",
    );
    for value in [OPENAI_VALUE, ELEVENLABS_VALUE, SHELL_VALUE] {
        assert!(!lane.stdout.contains(value), "stdout holds a key");
        assert!(!lane.stderr.contains(value), "stderr holds a key");
        for (path, text) in case.kept() {
            assert!(!text.contains(value), "{} holds a key", path.display());
        }
    }
    lane.says("quality-live: openai-speech-live: its output held its key; the kept output has [key] in its place");
}

/// The lane's word that `inference-docs assess` reads no record of `elevenlabs-live-example` is
/// held to the program: its fixture output is refused as a call for that assessment.
#[test]
fn the_measure_the_lane_writes_no_record_of_is_the_one_assess_reads_none_of() {
    let case = Case::new(Key::Missing, Key::Missing);
    for (measure, _, read) in MEASURES {
        let out = case.dir.join(format!("{measure}.evidence.json"));
        let output = Command::new(env!("CARGO_BIN_EXE_inference-docs"))
            .args(["assess", "--project", "inference", "--assessment", measure])
            .arg("--root")
            .arg(&case.root)
            .args(["--class", "local", "--out"])
            .arg(&out)
            .arg("--from")
            .arg(fixture(&format!("live/{measure}.out")))
            .args([
                "--source",
                "captured.out",
                "--started-at",
                "2026-11-27T10:00:00Z",
                "--ended-at",
                "2026-11-27T10:00:09Z",
                "--exit-status",
                "0",
            ])
            .output()
            .expect("the program runs");
        let stderr = String::from_utf8_lossy(&output.stderr);
        if read {
            assert_eq!(output.status.code(), Some(0), "{measure}: {stderr}");
        } else {
            assert_eq!(output.status.code(), Some(2), "{measure}: {stderr}");
            assert!(
                stderr.contains("the program reads no single printed line of this measure"),
                "{measure}: {stderr}"
            );
        }
    }
}

#[test]
fn a_missing_or_empty_key_skips_the_measures_of_its_provider_by_name_and_the_others_run() {
    for key in [Key::Missing, Key::Empty] {
        let case = Case::new(key, Key::Given(ELEVENLABS_VALUE));
        let lane = case.run(Some("1"), Some("voice-0001"));
        assert_eq!(
            lane.status,
            Some(0),
            "stdout:\n{}\nstderr:\n{}",
            lane.stdout,
            lane.stderr
        );
        lane.says(&format!(
            "quality-live: skipped openai-speech-live, openai-gpt-live: the Secret Service gives no key for {OPENAI_KEY}"
        ));
        assert_eq!(
            case.calls(),
            [
                format!("secret-tool lookup service b10x-secrets username {OPENAI_KEY}"),
                format!("secret-tool lookup service b10x-secrets username {ELEVENLABS_KEY}"),
                "elevenlabs-live-example".to_owned(),
                "elevenlabs-cancel-probe".to_owned(),
            ]
        );
        assert!(case.records("openai-speech-live").is_empty());
        assert!(case.records("openai-gpt-live").is_empty());
        assert_eq!(case.records("elevenlabs-cancel-probe").len(), 1);
        lane.says(
            "quality-live: 1 records written under .ratchet/records/, 2 measures skipped, 0 failed",
        );
    }
    let case = Case::new(Key::Given(OPENAI_VALUE), Key::Missing);
    let lane = case.run(Some("1"), Some("voice-0001"));
    assert_eq!(lane.status, Some(0), "stderr: {}", lane.stderr);
    lane.says(&format!(
        "quality-live: skipped elevenlabs-live-example, elevenlabs-cancel-probe: the Secret Service gives no key for {ELEVENLABS_KEY}"
    ));
    assert_eq!(case.records("openai-speech-live").len(), 1);
    assert_eq!(case.records("openai-gpt-live").len(), 1);
    assert!(case.records("elevenlabs-cancel-probe").is_empty());
}

/// Both ElevenLabs commands read `ELEVENLABS_VOICE_ID`: the example exits 2 without it, and the
/// probe's test panics while it builds its URL.
#[test]
fn without_a_voice_id_the_elevenlabs_measures_are_skipped_by_name_and_their_key_is_not_read() {
    let case = Case::new(Key::Given(OPENAI_VALUE), Key::Given(ELEVENLABS_VALUE));
    let lane = case.run(Some("1"), None);
    assert_eq!(lane.status, Some(0), "stderr: {}", lane.stderr);
    assert!(
        lane.stdout.starts_with(COUNT_WITHOUT_VOICE),
        "stdout:\n{}",
        lane.stdout
    );
    lane.says(
        "quality-live: skipped elevenlabs-live-example, elevenlabs-cancel-probe: ELEVENLABS_VOICE_ID is unset",
    );
    assert_eq!(
        case.calls(),
        [
            format!("secret-tool lookup service b10x-secrets username {OPENAI_KEY}"),
            "openai-speech-live".to_owned(),
            "openai-gpt-live".to_owned(),
        ]
    );
    assert_eq!(case.records("openai-speech-live").len(), 1);
    assert_eq!(case.records("openai-gpt-live").len(), 1);
    lane.says(
        "quality-live: 2 records written under .ratchet/records/, 2 measures skipped, 0 failed",
    );
}

/// The lane is a task of its own, outside `task check` and every workflow, its flag named in its
/// description, and what it writes is ignored by git.
#[test]
fn the_lane_is_a_task_outside_the_gate_and_its_directory_is_ignored() {
    let taskfile: serde_norway::Value = serde_norway::from_str(
        &fs::read_to_string(root().join("Taskfile.yml")).expect("Taskfile.yml is read"),
    )
    .expect("Taskfile.yml parses");
    let tasks = &taskfile["tasks"];
    let description = tasks["quality-live"]["desc"]
        .as_str()
        .expect("the task quality-live has a description");
    assert!(description.contains("YES=1"), "{description}");
    let mut reached = vec!["check".to_owned()];
    let mut at = 0;
    while at < reached.len() {
        let name = reached[at].clone();
        at += 1;
        for command in tasks[name.as_str()]["cmds"]
            .as_sequence()
            .into_iter()
            .flatten()
        {
            let text = serde_json::to_string(command).expect("a command");
            assert!(
                !text.contains("quality-live") && !text.contains("-- live"),
                "task {name} runs the lane: {text}"
            );
            if let Some(task) = command.get("task").and_then(serde_norway::Value::as_str)
                && !reached.iter().any(|seen| seen == task)
            {
                reached.push(task.to_owned());
            }
        }
    }
    for entry in fs::read_dir(root().join(".github/workflows")).expect("the workflows are read") {
        let path = entry.expect("an entry").path();
        let text = fs::read_to_string(&path).expect("a workflow is read");
        assert!(
            !text.contains("quality-live") && !text.contains("inference-docs -- live"),
            "{} runs the lane",
            path.display()
        );
    }
    let ignored = fs::read_to_string(root().join(".gitignore")).expect(".gitignore is read");
    assert!(
        ignored.lines().any(|line| line == "/.ratchet/"),
        ".gitignore: {ignored}"
    );
}

/// A command that exits other than 0 gives no record, the measure counts as failed and the lane
/// exits 1; the example without a reader counts as failed too.
#[test]
fn a_command_that_fails_gives_no_record_and_the_lane_exits_1() {
    let case = Case::new(Key::Given(OPENAI_VALUE), Key::Given(ELEVENLABS_VALUE));
    for (measure, status) in [("openai-speech-live", 1), ("elevenlabs-live-example", 3)] {
        executable(
            &case.dir.join("stubs").join(measure),
            &format!(
                "#!/bin/sh\ncat '{}'\nexit {status}\n",
                fixture(&format!("live/{measure}.out")).display()
            ),
        );
    }
    let lane = case.run(Some("1"), Some("voice-0001"));
    assert_eq!(
        lane.status,
        Some(1),
        "stdout:\n{}\nstderr:\n{}",
        lane.stdout,
        lane.stderr
    );
    assert!(case.records("openai-speech-live").is_empty());
    assert_eq!(case.records("openai-gpt-live").len(), 1);
    assert!(case.records("elevenlabs-live-example").is_empty());
    assert_eq!(case.records("elevenlabs-cancel-probe").len(), 1);
    lane.says("quality-live: openai-speech-live: no record; inference-docs assess exited 1");
    lane.says("openai-speech-live: its command exited 1");
    lane.says("quality-live: elevenlabs-live-example: exited 3;");
    lane.says(
        "quality-live: 2 records written under .ratchet/records/, 0 measures skipped, 2 failed",
    );
}

/// Two runs keep two run directories, and the records of the first stay beside those of the
/// second.
#[test]
fn two_runs_keep_two_run_directories_and_neither_overwrites_the_other() {
    let case = Case::new(Key::Given(OPENAI_VALUE), Key::Given(ELEVENLABS_VALUE));
    for _ in 0..2 {
        let lane = case.run(Some("1"), Some("voice-0001"));
        assert_eq!(lane.status, Some(0), "stdout:\n{}", lane.stdout);
    }
    let mut runs: Vec<String> = fs::read_dir(case.root.join(".ratchet/runs"))
        .expect("the runs are kept")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .into_string()
                .expect("UTF-8")
        })
        .collect();
    runs.sort();
    assert_eq!(runs.len(), 2, "two run directories: {runs:?}");
    for (measure, _, read) in MEASURES {
        for run in &runs {
            assert!(
                case.root
                    .join(format!(".ratchet/runs/{run}/{measure}.out"))
                    .is_file(),
                "the output of {measure} in {run} is kept"
            );
        }
        if read {
            let mut records = case.records(measure);
            records.sort();
            let mut expected: Vec<PathBuf> = runs
                .iter()
                .map(|run| {
                    case.root.join(format!(
                        ".ratchet/records/adapters/{measure}/{run}.evidence.json"
                    ))
                })
                .collect();
            expected.sort();
            assert_eq!(records, expected, "one record of {measure} per run");
        }
    }
}

/// A key the Secret Service gives with a line end, spaces and `=` reaches its command as it is
/// stored, without the line end, and stays out of every output.
#[test]
fn a_key_given_with_a_line_end_reaches_its_command_exactly() {
    const SPACED: &str = "not a=key 0004";
    let case = Case::new(Key::Given(OPENAI_VALUE), Key::Missing);
    executable(
        &case.dir.join("bin/secret-tool"),
        &format!(
            "#!/bin/sh\n[ \"$5\" = '{OPENAI_KEY}' ] || exit 1\nprintf '%s\\n' '{SPACED}'\n"
        ),
    );
    let exact = case.dir.join("exact");
    executable(
        &case.dir.join("stubs/openai-speech-live"),
        &format!(
            "#!/bin/sh\nif [ \"$OPENAI_API_KEY\" = '{SPACED}' ]; then echo exact; else echo other; fi > '{}'\n\
             printf 'the key given: %s\\n' \"$OPENAI_API_KEY\" >&2\ncat '{}'\n",
            exact.display(),
            fixture("live/openai-speech-live.out").display()
        ),
    );
    let lane = case.run(Some("1"), None);
    assert_eq!(lane.status, Some(0), "stdout:\n{}", lane.stdout);
    assert_eq!(
        fs::read_to_string(&exact).expect("the stub ran").trim(),
        "exact",
        "the command sees the key without the line end"
    );
    assert!(!lane.stdout.contains(SPACED) && !lane.stderr.contains(SPACED));
    for (path, text) in case.kept() {
        assert!(!text.contains(SPACED), "{} holds the key", path.display());
    }
}

/// Only `YES=1` lets the lane call anything: a value near it does not.
#[test]
fn a_yes_that_is_not_exactly_1_calls_nothing() {
    for yes in ["10", " 1", "1 ", "01", "true", "Y", "1\n"] {
        let case = Case::new(Key::Given(OPENAI_VALUE), Key::Given(ELEVENLABS_VALUE));
        let lane = case.run(Some(yes), Some("voice-0001"));
        assert_eq!(lane.status, Some(2), "YES={yes:?}; stdout:\n{}", lane.stdout);
        case.called_nothing();
    }
}

/// Without `secret-tool` on `PATH` every measure is skipped by name and nothing is started.
#[test]
fn without_secret_tool_every_measure_is_skipped_by_name() {
    let case = Case::new(Key::Missing, Key::Missing);
    let empty = case.dir.join("empty");
    fs::create_dir_all(&empty).expect("the directory is made");
    let output = Command::new(env!("CARGO_BIN_EXE_inference-docs"))
        .arg("live")
        .arg("--root")
        .arg(&case.root)
        .arg("--stubs")
        .arg(case.dir.join("stubs"))
        .env("PATH", &empty)
        .env("YES", "1")
        .env("ELEVENLABS_VOICE_ID", "voice-0001")
        .env_remove("OPENAI_API_KEY")
        .env_remove("ELEVENLABS_API_KEY")
        .output()
        .expect("the lane runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "stdout:\n{stdout}");
    assert!(
        stdout.contains("quality-live: skipped openai-speech-live, openai-gpt-live: secret-tool cannot be started"),
        "{stdout}"
    );
    assert!(
        stdout.contains("quality-live: skipped elevenlabs-live-example, elevenlabs-cancel-probe: secret-tool cannot be started"),
        "{stdout}"
    );
    assert_eq!(case.calls(), Vec::<String>::new(), "no stub is started");
}

/// Whether a command line starts `inference-docs live`, by `cargo run` or by the binary.
fn starts_the_lane(text: &str) -> bool {
    let words: Vec<&str> = text
        .split(|character: char| character.is_whitespace() || matches!(character, '"' | '\''))
        .filter(|word| !word.is_empty())
        .collect();
    words.iter().enumerate().any(|(at, word)| {
        word.ends_with("inference-docs")
            && words[at + 1..]
                .iter()
                .find(|next| !next.starts_with('-'))
                .is_some_and(|next| *next == "live")
    })
}

/// No task that `task check` or a workflow reaches, through its commands or its dependencies,
/// starts the lane, by its task or by the program, and no workflow starts the program.
#[test]
fn no_task_a_workflow_or_check_reaches_starts_the_lane() {
    let taskfile: serde_norway::Value = serde_norway::from_str(
        &fs::read_to_string(root().join("Taskfile.yml")).expect("Taskfile.yml is read"),
    )
    .expect("Taskfile.yml parses");
    let tasks = &taskfile["tasks"];
    assert!(starts_the_lane(
        &serde_json::to_string(&tasks["quality-live"]["cmds"]).expect("the commands")
    ));
    let mut reached = vec!["check".to_owned()];
    for entry in fs::read_dir(root().join(".github/workflows")).expect("the workflows are read") {
        let path = entry.expect("an entry").path();
        let text = fs::read_to_string(&path).expect("a workflow is read");
        assert!(
            !starts_the_lane(&text),
            "{} starts the lane",
            path.display()
        );
        let words: Vec<&str> = text.split_whitespace().collect();
        for pair in words.windows(2) {
            if pair[0] == "task" && !pair[1].starts_with('-') {
                reached.push(pair[1].to_owned());
            }
        }
    }
    let mut at = 0;
    while at < reached.len() {
        let name = reached[at].clone();
        at += 1;
        assert_ne!(name, "quality-live", "the lane is reached");
        let task = &tasks[name.as_str()];
        let mut next = Vec::new();
        for command in task["cmds"].as_sequence().into_iter().flatten() {
            let text = serde_json::to_string(command).expect("a command");
            assert!(!starts_the_lane(&text), "task {name} starts the lane: {text}");
            if let Some(called) = command.get("task").and_then(serde_norway::Value::as_str) {
                next.push(called.to_owned());
            }
        }
        for dependency in task["deps"].as_sequence().into_iter().flatten() {
            let called = dependency
                .as_str()
                .or_else(|| dependency.get("task").and_then(serde_norway::Value::as_str));
            if let Some(called) = called {
                next.push(called.to_owned());
            }
        }
        for called in next {
            if !reached.contains(&called) {
                reached.push(called);
            }
        }
    }
}
