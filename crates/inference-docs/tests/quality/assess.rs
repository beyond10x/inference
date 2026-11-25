//! `inference-docs assess`, the program that writes the record of one assessment from a file a
//! run left behind, held to the fixtures under `tests/fixtures/assess/`: one document for each
//! assessor that has a reader, compared byte for byte with an expected file or with the committed
//! record; the refusals; the forms of a printed `Duration`; the labels of the printed measures
//! against the sources of their commands; and the documents it writes, put in the place of the
//! committed records, against the rest of this check.
//!
//! Every case runs the program on a root of its own under the target's scratch directory: the
//! plan files, the assessor declarations and `ratchet.toml` copied from the tree, and the pins of
//! the tools, the manifest of `inference-sherpa-onnx` and its capability record written from the
//! fixtures, so that a new toolchain or a release does not change what a case expects.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

use super::{load, problems, report};
use crate::root;

/// The commit and the check run the hand-written records name.
const REVISION: &str = "59b8f4a858ab9c4eef94bc21de8e7bb85f90ddad";
const CHECK_RUN: &str = "https://github.com/beyond10x/inference/actions/runs/45474241015";
/// The times a case gives a printed output, which holds none.
const STARTED: &str = "2026-11-25T10:00:00Z";
const ENDED: &str = "2026-11-25T10:00:09Z";
const SHERPA_RECORD: &str = "adapters/inference-sherpa-onnx/records/stt-i9-10900k-cpu.json";

fn fixture(name: &str) -> PathBuf {
    root()
        .join("crates/inference-docs/tests/fixtures/assess")
        .join(name)
}

fn fixture_text(name: &str) -> String {
    fs::read_to_string(fixture(name)).unwrap_or_else(|error| panic!("the fixture {name}: {error}"))
}

fn place(directory: &Path, path: &str, text: &str) {
    let file = directory.join(path);
    fs::create_dir_all(file.parent().expect("a file has a directory"))
        .expect("the directory is created");
    fs::write(file, text).expect("the file is written");
}

/// A fresh root for the program, as the module documentation says.
fn scratch_root() -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "assess-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&directory);
    let mut copied = vec!["ratchet.toml".to_owned()];
    for (from, suffix) in [
        ("quality", ".ratchet.yaml"),
        ("quality/assessors", ".assessor.yaml"),
    ] {
        for entry in fs::read_dir(root().join(from)).expect("the directory is read") {
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
        let text = fs::read_to_string(root().join(path)).expect("a file of the tree is read");
        place(&directory, path, &text);
    }
    for (path, name) in [
        ("rust-toolchain.toml", "toolchain.toml"),
        (".config/nextest.toml", "nextest.toml"),
        (
            "adapters/inference-sherpa-onnx/Cargo.toml",
            "sherpa-onnx-manifest.toml",
        ),
        (SHERPA_RECORD, "stt-record.json"),
    ] {
        place(&directory, path, &fixture_text(name));
    }
    directory
}

/// What one call of the program left: its exit status, what it printed to stderr, and the
/// document at `--out`, if it wrote one.
struct Run {
    status: Option<i32>,
    stderr: String,
    document: Option<String>,
}

impl Run {
    /// The document, held to have been written with exit 0 and nothing on stderr.
    #[track_caller]
    fn written(self) -> String {
        assert_eq!(self.status, Some(0), "stderr: {}", self.stderr);
        assert!(self.stderr.is_empty(), "stderr: {}", self.stderr);
        self.document.expect("a document is written")
    }

    /// Held to have ended with `status`, without a document, and with a line on stderr that
    /// holds each of `lines`.
    #[track_caller]
    fn ended(&self, status: i32, lines: &[&str]) {
        assert_eq!(self.status, Some(status), "stderr: {}", self.stderr);
        assert!(
            self.document.is_none(),
            "no document is written: {:?}",
            self.document
        );
        for line in lines {
            assert!(
                self.stderr.lines().any(|said| said.contains(line)),
                "no line of stderr holds {line:?}; stderr: {}",
                self.stderr
            );
        }
    }
}

/// Runs `inference-docs assess --root <root> --out <file> <arguments>`.
fn assess(root: &Path, arguments: &[&str]) -> Run {
    let out = root.join("written.evidence.json");
    let _ = fs::remove_file(&out);
    let output = Command::new(env!("CARGO_BIN_EXE_inference-docs"))
        .arg("assess")
        .arg("--root")
        .arg(root)
        .arg("--out")
        .arg(&out)
        .args(arguments)
        .output()
        .expect("the program runs");
    Run {
        status: output.status.code(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        document: fs::read_to_string(&out).ok(),
    }
}

fn path(file: &Path) -> &str {
    file.to_str().expect("a path in UTF-8")
}

/// A call for `assessment` of the check run at the hand-written records' commit, from `from`.
fn from_check_run(root: &Path, assessment: &str, from: &Path) -> Run {
    assess(
        root,
        &[
            "--project",
            "inference",
            "--assessment",
            assessment,
            "--class",
            "ci",
            "--revision",
            REVISION,
            "--from",
            path(from),
            "--source",
            CHECK_RUN,
        ],
    )
}

/// A call for the printed measures of `assessment`, from an output that holds `printed`.
fn from_output(root: &Path, assessment: &str, printed: &str) -> Run {
    let output = root.join("target/captured.out");
    place(root, "target/captured.out", printed);
    assess(
        root,
        &[
            "--project",
            "inference",
            "--assessment",
            assessment,
            "--class",
            "local",
            "--from",
            path(&output),
            "--source",
            "target/captured.out",
            "--started-at",
            STARTED,
            "--ended-at",
            ENDED,
        ],
    )
}

/// The `value` of each result of a document, in its order.
fn values(document: &str) -> Vec<String> {
    let document: Value = serde_json::from_str(document).expect("the document is JSON");
    document
        .pointer("/evidence/results")
        .and_then(Value::as_array)
        .expect("the document has results")
        .iter()
        .map(|result| {
            result
                .get("value")
                .and_then(Value::as_str)
                .expect("a result with a value")
                .to_owned()
        })
        .collect()
}

#[test]
fn a_report_of_the_workspace_run_gives_its_coverage_and_its_slowest_test() {
    let root = scratch_root();
    let written = from_check_run(&root, "workspace-tests", &fixture("report.junit.xml")).written();
    assert_eq!(written, fixture_text("workspace-tests.evidence.json"));
}

#[test]
fn a_report_gives_the_tests_run_and_passed_and_the_slowest_test_of_each_package() {
    let root = scratch_root();
    let written = from_check_run(&root, "adapter-tests", &fixture("report.junit.xml")).written();
    assert_eq!(written, fixture_text("adapter-tests.evidence.json"));
}

/// The record of `release-statuses` was written by hand from the check run, in which the four
/// tests passed. The program, given its commit and its run and a report in which they passed,
/// writes the same bytes.
#[test]
fn a_report_in_which_the_named_tests_passed_gives_the_committed_record_byte_for_byte() {
    let committed =
        fs::read_to_string(root().join("quality/records/release-statuses.evidence.json"))
            .expect("the committed record is read");
    let record: Value = serde_json::from_str(&committed).expect("the record is JSON");
    let field = |pointer: &str| {
        record
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("the record has {pointer}"))
            .to_owned()
    };
    let root_directory = scratch_root();
    let report = fixture("report.junit.xml");
    let written = assess(
        &root_directory,
        &[
            "--project",
            "inference",
            "--assessment",
            "release-statuses",
            "--class",
            &field("/evidence/target/class"),
            "--revision",
            &field("/evidence/target/revision"),
            "--from",
            path(&report),
            "--source",
            &field("/evidence/source/location"),
        ],
    )
    .written();
    assert_eq!(written, committed);
}

#[test]
fn a_capability_record_gives_its_value_and_its_test_in_the_report_whether_it_passed() {
    let root = scratch_root();
    let written = assess(
        &root,
        &[
            "--project",
            "inference",
            "--assessment",
            "sherpa-onnx-stt-record",
            "--class",
            "ci",
            "--from",
            path(&fixture("report.junit.xml")),
            "--source",
            CHECK_RUN,
        ],
    )
    .written();
    assert_eq!(
        written,
        fixture_text("sherpa-onnx-stt-record.evidence.json")
    );
}

#[test]
fn a_printed_output_gives_one_value_per_measure_in_milliseconds() {
    let root = scratch_root();
    let written = assess(
        &root,
        &[
            "--project",
            "inference",
            "--assessment",
            "llama-server-live",
            "--class",
            "local",
            "--from",
            path(&fixture("llama-server-live.out")),
            "--source",
            "target/llama-server-live.out",
            "--started-at",
            STARTED,
            "--ended-at",
            ENDED,
        ],
    )
    .written();
    assert_eq!(written, fixture_text("llama-server-live.evidence.json"));
}

/// The acceptance of the story: a document the program wrote stands in the place of the
/// committed record of its assessment, and the test of the quality documents finds no problem.
#[test]
fn written_documents_in_the_place_of_the_committed_records_leave_the_documents_without_a_problem() {
    let root_directory = scratch_root();
    let junit = fixture("report.junit.xml");
    let mut documents = load();
    for assessment in ["workspace-tests", "release-statuses"] {
        let written = from_check_run(&root_directory, assessment, &junit).written();
        let path = format!("quality/records/{assessment}.evidence.json");
        assert!(
            documents.records.insert(path, written).is_some(),
            "{assessment} has a committed record"
        );
    }
    let written = assess(
        &root_directory,
        &[
            "--project",
            "inference",
            "--assessment",
            "sherpa-onnx-stt-record",
            "--class",
            "ci",
            "--from",
            path(&junit),
            "--source",
            CHECK_RUN,
        ],
    )
    .written();
    assert!(
        documents
            .records
            .insert(
                "quality/records/sherpa-onnx-stt-record.evidence.json".to_owned(),
                written
            )
            .is_some(),
        "sherpa-onnx-stt-record has a committed record"
    );
    let found = problems(&documents);
    assert!(found.is_empty(), "{}", report(&found));
}

/// The debug form of `Duration` in each of its units, bare and inside `Some(...)`, and the whole
/// milliseconds that two commands print with ` ms` after them.
#[test]
fn every_printed_form_of_a_duration_is_read_as_milliseconds() {
    let root = scratch_root();
    for ([first, completed], expected) in [
        (["1.234s", "90s"], ["1234", "90000"]),
        (["12.5ms", "12ms"], ["12.5", "12"]),
        (["870µs", "870.5µs"], ["0.87", "0.8705"]),
        (["15ns", "0ns"], ["0.000015", "0"]),
        (["Some(2s)", "Some(1.000000001s)"], ["2000", "1000.000001"]),
    ] {
        let printed = format!("deltas: 3, first after {first}, completed after {completed}\n");
        let written = from_output(&root, "llama-server-live", &printed).written();
        assert_eq!(values(&written), expected, "printed: {printed}");
    }
    let printed = "started in 812 ms: SessionId(\"s-1\")\nt=1 out=1 said \"hello\"\nclosed in 95 ms: Some(Closed)\n";
    let written = from_output(&root, "openai-gpt-live", printed).written();
    assert_eq!(values(&written), ["812", "95"]);
    let printed = "P1: 52 characters with flush, no close\nP1 result: first audio Some(312) ms, last audio Some(1500) ms, audio 48000 B (1500 ms), final true\n";
    let written = from_output(&root, "elevenlabs-cancel-probe", printed).written();
    assert_eq!(values(&written), ["312"]);
    let printed = "committed input to first playable audio, turn 1: Some(640.25ms)\ninterruption to playback stop, turn 1: 18ms\n";
    let written = from_output(&root, "cascade-local-adapters", printed).written();
    assert_eq!(values(&written), ["640.25", "18"]);
}

#[test]
fn a_printed_none_a_skipped_test_a_second_value_or_no_value_is_refused() {
    let root = scratch_root();
    from_output(
        &root,
        "llama-server-live",
        "deltas: 0, first after None, completed after 1.2s\n",
    )
    .ended(
        1,
        &["llama-server-live/first-token: the line `deltas: 0, first after None, completed after 1.2s` prints None"],
    );
    from_output(
        &root,
        "llama-server-live",
        "skipped: set INFERENCE_LLAMA_SERVER_URL and INFERENCE_LLAMA_SERVER_MODEL to run against a llama-server\n",
    )
    .ended(
        1,
        &[
            "llama-server-live: the output says a test skipped: `skipped: set INFERENCE_LLAMA_SERVER_URL",
            "llama-server-live/first-token: no line of the output holds `first after`",
        ],
    );
    from_output(
        &root,
        "llama-server-live",
        "deltas: 3, first after 20ms, completed after 1s\ndeltas: 4, first after 22ms, completed after 2s\n",
    )
    .ended(
        1,
        &[
            "llama-server-live/first-token: 2 lines of the output hold `first after`, and the measure takes one value per run",
            "llama-server-live/generation: 2 lines of the output hold `completed after`",
        ],
    );
    from_output(
        &root,
        "elevenlabs-live-example",
        "ulaw_8000: first audio after 412 ms, 30 frames, 1.20 s of audio, all mu-law at 8000 Hz mono\npcm_24000: first audio after 380 ms, 30 frames, 1.20 s of audio, all 16-bit little-endian PCM at 24000 Hz mono\n",
    )
    .ended(
        1,
        &["elevenlabs-live-example/first-audio: 2 lines of the output hold `first audio after`"],
    );
    from_output(
        &root,
        "llama-server-live",
        "deltas: 3, first after soon, completed after 1s\n",
    )
    .ended(
        1,
        &["llama-server-live/first-token: the line `deltas: 3, first after soon, completed after 1s` holds no duration after `first after`"],
    );
    from_output(&root, "openai-speech-live", "").ended(
        1,
        &["openai-speech-live/first-audio: no line of the output holds `first frame after`"],
    );
}

#[test]
fn a_report_that_does_not_hold_what_the_assessment_reads_is_refused() {
    let root = scratch_root();
    let full = fixture_text("report.junit.xml");
    let named = "crates/inference-docs/src/status.rs::a_hosted_binding_on_main_is_a_next_item_and_one_since_a_release_a_shipped_item";
    let without = root.join("without.junit.xml");
    fs::write(
        &without,
        full.replace(
            "status::tests::a_hosted_binding_on_main_is_a_next_item_and_one_since_a_release_a_shipped_item",
            "status::tests::a_test_of_another_name",
        ),
    )
    .expect("the report is written");
    from_check_run(&root, "release-statuses", &without).ended(
        1,
        &[format!(
            "release-statuses: the report holds no test {named} of the package inference-docs"
        )
        .as_str()],
    );
    let skipped = root.join("skipped.junit.xml");
    fs::write(
        &skipped,
        full.replace(
            "classname=\"inference-whispercpp::stt_record\" timestamp=\"2026-11-24T21:02:07.000+01:00\" time=\"0.334\"/>",
            "classname=\"inference-whispercpp::stt_record\" timestamp=\"2026-11-24T21:02:07.000+01:00\" time=\"0.000\"><skipped/></testcase>",
        ),
    )
    .expect("the report is written");
    let record_test = "adapters/inference-whispercpp/tests/stt_record.rs::the_resource_profile_is_a_measured_cpu_profile";
    place(
        &root,
        "adapters/inference-whispercpp/Cargo.toml",
        "[package]\nname = \"inference-whispercpp\"\n",
    );
    place(
        &root,
        "adapters/inference-whispercpp/records/stt-i9-10900k-cpu.json",
        &fixture_text("stt-record.json"),
    );
    assess(
        &root,
        &[
            "--project",
            "inference",
            "--assessment",
            "whispercpp-stt-record",
            "--class",
            "ci",
            "--from",
            path(&skipped),
            "--source",
            CHECK_RUN,
        ],
    )
    .ended(
        1,
        &[format!(
            "whispercpp-stt-record: the report holds the test {record_test} of the package inference-whispercpp as skipped"
        )
        .as_str()],
    );
    let no_phonic = root.join("no-phonic.junit.xml");
    let start = full
        .find("    <testsuite name=\"inference-phonic::duplex\"")
        .expect("the report has the suite");
    let end = start
        + full[start..]
            .find("</testsuite>\n")
            .expect("the suite ends")
        + "</testsuite>\n".len();
    fs::write(&no_phonic, format!("{}{}", &full[..start], &full[end..]))
        .expect("the report is written");
    from_check_run(&root, "adapter-tests", &no_phonic).ended(
        1,
        &["adapter-tests: the report holds no test of the package inference-phonic that ran"],
    );
    let broken = root.join("broken.junit.xml");
    fs::write(&broken, "<testsuites><testsuite name=\"a\"></testsuites>")
        .expect("the report is written");
    from_check_run(&root, "workspace-tests", &broken).ended(1, &["is not a JUnit report"]);
    let empty = root.join("empty.junit.xml");
    fs::write(
        &empty,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites name=\"nextest-run\" tests=\"0\" timestamp=\"2026-11-24T21:02:05.400+01:00\" time=\"0.100\">\n</testsuites>\n",
    )
    .expect("the report is written");
    from_check_run(&root, "workspace-tests", &empty)
        .ended(1, &["workspace-tests: the report holds no test that ran"]);
    let untimed = root.join("untimed.junit.xml");
    fs::write(
        &untimed,
        full.replace(
            " timestamp=\"2026-11-24T21:02:05.400+01:00\" time=\"101.000\">",
            ">",
        ),
    )
    .expect("the report is written");
    from_check_run(&root, "workspace-tests", &untimed).ended(
        1,
        &["workspace-tests: the report holds no timestamp and no time of the run; give --started-at and --ended-at"],
    );
    place(
        &root,
        SHERPA_RECORD,
        "{\n  \"resource_profile\": {\n    \"cpu_real_time_factor_milli\": null\n  }\n}\n",
    );
    assess(
        &root,
        &[
            "--project",
            "inference",
            "--assessment",
            "sherpa-onnx-stt-record",
            "--class",
            "ci",
            "--from",
            path(&fixture("report.junit.xml")),
            "--source",
            CHECK_RUN,
        ],
    )
    .ended(
        1,
        &[format!(
            "sherpa-onnx-stt-record: {SHERPA_RECORD} holds no whole number at resource_profile.cpu_real_time_factor_milli"
        )
        .as_str()],
    );
}

/// Runs the program for `assessment` of the project `inference` with `first` and then `then` as
/// its further arguments.
fn call(root: &Path, assessment: &str, first: &[&str], then: &[&str]) -> Run {
    let mut arguments = vec!["--project", "inference", "--assessment", assessment];
    arguments.extend_from_slice(first);
    arguments.extend_from_slice(then);
    assess(root, &arguments)
}

#[test]
fn a_wrong_call_exits_2_and_a_plan_the_program_cannot_read_exits_3() {
    let root = scratch_root();
    let report = fixture("report.junit.xml");
    let output = fixture("llama-server-live.out");
    let junit = ["--from", path(&report), "--source", CHECK_RUN];
    let printed = ["--from", path(&output), "--source", "target/out"];
    let times = ["--started-at", STARTED, "--ended-at", ENDED];
    let ci = ["--class", "ci", "--revision", REVISION];
    call(&root, "no-such-assessment", &ci, &junit).ended(
        2,
        &["no plan file of ratchet.toml declares the assessment no-such-assessment"],
    );
    call(&root, "lint", &ci, &junit).ended(
        2,
        &["the assessor command-run@1 of lint has no reader in this program"],
    );
    call(&root, "workspace-tests", &["--class", "ci"], &junit).ended(
        2,
        &["workspace-tests: its target is a revision; give --revision"],
    );
    call(&root, "sherpa-onnx-stt-record", &ci, &junit).ended(
        2,
        &["sherpa-onnx-stt-record: its target is a document, which has no revision; give no --revision"],
    );
    call(&root, "llama-server-live", &["--class", "local"], &printed).ended(
        2,
        &["llama-server-live: the output of a command holds no time; give --started-at and --ended-at"],
    );
    call(
        &root,
        "llama-server-live",
        &[
            "--class",
            "local",
            "--started-at",
            "2026-11-25 10:00:00",
            "--ended-at",
            ENDED,
        ],
        &printed,
    )
    .ended(
        2,
        &["--started-at is \"2026-11-25 10:00:00\", not a time of the form 2026-11-24T20:02:05Z"],
    );
    call(
        &root,
        "llama-server-live",
        &[
            "--class",
            "local",
            "--started-at",
            ENDED,
            "--ended-at",
            STARTED,
        ],
        &printed,
    )
    .ended(2, &["--ended-at is before --started-at"]);
    let compare = [&["--class", "local"][..], &times].concat();
    call(&root, "compare", &compare, &printed).ended(
        2,
        &["compare/first-audio: the program reads no printed line of this measure"],
    );
    call(
        &root,
        "workspace-tests",
        &["--class", "production", "--revision", REVISION],
        &junit,
    )
    .ended(
        2,
        &["workspace-tests: the plan assesses the classes [\"local\", \"ci\"], not production"],
    );
    call(
        &root,
        "workspace-tests",
        &["--class", "nightly", "--revision", REVISION],
        &junit,
    )
    .ended(
        2,
        &["--class is nightly, not one of local, ci, ephemeral, staging, production"],
    );
    call(
        &root,
        "workspace-tests",
        &["--class", "ci", "--revision", "59b8f4a8"],
        &junit,
    )
    .ended(
        2,
        &["--revision is 59b8f4a8, not a full commit id in lowercase hex"],
    );
    let mut other = vec!["--project", "another", "--assessment", "workspace-tests"];
    other.extend_from_slice(&ci);
    other.extend_from_slice(&junit);
    assess(&root, &other).ended(
        2,
        &["--project is another, the plan files of ratchet.toml are of the project inference"],
    );
    call(
        &root,
        "workspace-tests",
        &ci,
        &["--from", path(&report), "--source", "/an/absolute/path.xml"],
    )
    .ended(
        2,
        &["--source is an absolute path; a document names a file by its path from the root"],
    );
    call(&root, "workspace-tests", &ci, &["--source", CHECK_RUN]).ended(2, &["--from"]);
    place(&root, "quality/gate.ratchet.yaml", "format: [ratchet/1\n");
    call(&root, "workspace-tests", &ci, &junit)
        .ended(3, &["quality/gate.ratchet.yaml: does not parse"]);
    fs::remove_file(root.join("ratchet.toml")).expect("ratchet.toml is removed");
    call(&root, "workspace-tests", &ci, &junit).ended(3, &["ratchet.toml: cannot be read"]);
}

/// The directory of a package of the workspace by its name.
fn package_directory(package: &str) -> String {
    ["adapters", "examples", "crates"]
        .iter()
        .map(|parent| format!("{parent}/{package}"))
        .find(|directory| root().join(directory).join("Cargo.toml").is_file())
        .unwrap_or_else(|| panic!("no directory holds the package {package}"))
}

/// The file of the test or the example a `cargo test` or `cargo run` command names.
fn command_source(command: &str) -> String {
    let words: Vec<&str> = command
        .split_whitespace()
        .take_while(|word| *word != "--")
        .collect();
    let after = |flag: &str| {
        words
            .windows(2)
            .find(|pair| pair[0] == flag)
            .map(|pair| pair[1])
    };
    let package = after("-p").expect("the command names a package");
    let (directory, name) = after("--test")
        .map(|name| ("tests", name))
        .or_else(|| after("--example").map(|name| ("examples", name)))
        .expect("the command names a test or an example");
    format!("{}/{directory}/{name}.rs", package_directory(package))
}

/// The program finds a printed measure by the text before its value. Each such text is one the
/// source of the measure's command prints: run on an empty output, the program names, for each
/// measure of a plan of `printed-duration@1` that it reads, the text it found no line of.
#[test]
fn every_printed_measure_the_program_reads_has_its_label_in_the_source_of_its_command() {
    let root_directory = scratch_root();
    let mut read = BTreeSet::new();
    for plan in [
        "quality/adapters.ratchet.yaml",
        "quality/gate.ratchet.yaml",
        "quality/docs-site.ratchet.yaml",
    ] {
        let text = fs::read_to_string(root().join(plan)).expect("the plan file is read");
        let plan: Value = serde_norway::from_str(&text).expect("the plan file parses");
        for (id, assessment) in plan
            .get("assessments")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            if assessment.get("assessor").and_then(Value::as_str) != Some("printed-duration@1") {
                continue;
            }
            let run = from_output(&root_directory, id, "");
            if run.status == Some(2) {
                assert!(
                    run.stderr
                        .contains("the program reads no printed line of this measure"),
                    "{id}: {}",
                    run.stderr
                );
                continue;
            }
            assert_eq!(run.status, Some(1), "{id}: {}", run.stderr);
            let command = assessment
                .pointer("/observation/command")
                .and_then(Value::as_str)
                .expect("a printed measure has a command");
            let source_path = command_source(command);
            let source = fs::read_to_string(root().join(&source_path))
                .unwrap_or_else(|error| panic!("{source_path}: {error}"));
            for (name, _) in assessment
                .get("measurements")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                let prefix = format!("{id}/{name}: no line of the output holds `");
                let line = run
                    .stderr
                    .lines()
                    .find_map(|line| line.split_once(prefix.as_str()).map(|(_, rest)| rest))
                    .unwrap_or_else(|| panic!("{id}/{name}: {}", run.stderr));
                let label = line.split('`').next().unwrap_or_default();
                assert!(
                    !label.is_empty() && source.contains(label),
                    "{id}/{name}: {source_path} does not print {label:?}"
                );
                read.insert(format!("{id}/{name}"));
            }
        }
    }
    assert_eq!(read.len(), 17, "the printed measures read: {read:?}");
}
