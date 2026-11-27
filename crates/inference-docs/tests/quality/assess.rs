//! `inference-docs assess`, the program that writes the record of one assessment from a file a
//! run left behind, held to the fixtures under `tests/fixtures/assess/`: one document for each
//! assessor that has a reader, compared byte for byte with an expected file or with the committed
//! record; the refusals; the forms of a printed `Duration`; the labels of the printed measures
//! against the sources of their commands; and the documents it writes, put in the place of the
//! committed records, against the rest of this check.
//!
//! Every case runs the program on a root of its own under the target's scratch directory: the
//! plan files, the assessor declarations, `ratchet.toml` and the files of the named tests the
//! cases read copied from the tree, and the pins of the tools, the manifest of
//! `inference-sherpa-onnx` and its capability record written from the fixtures, so that a new
//! toolchain or a release does not change what a case expects.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

use super::{load, problems, record_problems, report};
use crate::root;

/// The commit and the check run the hand-written records name.
const REVISION: &str = "59b8f4a858ab9c4eef94bc21de8e7bb85f90ddad";
const CHECK_RUN: &str = "https://github.com/beyond10x/inference/actions/runs/45474241015";
/// The times a case gives a printed output, which holds none.
const STARTED: &str = "2026-11-25T10:00:00Z";
const ENDED: &str = "2026-11-25T10:00:09Z";
const SHERPA_RECORD: &str = "adapters/inference-sherpa-onnx/records/stt-i9-10900k-cpu.json";
/// The files of the named tests the cases read: the program reads the inline modules around a
/// named test from its file.
const NAMED_FILES: [&str; 4] = [
    "crates/inference-docs/src/adapters.rs",
    "crates/inference-docs/src/status.rs",
    "adapters/inference-sherpa-onnx/tests/stt_record.rs",
    "adapters/inference-whispercpp/tests/stt_record.rs",
];

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
    copied.extend(NAMED_FILES.map(str::to_owned));
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

/// The lines `cargo test` ends the output of a live test with when the test passed.
const PASSED: &str = "test the_measured_test ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.00s\n";

/// A call for the printed measures of `assessment`, from an output that holds `printed` and
/// then the lines of a test that passed.
fn from_output(root: &Path, assessment: &str, printed: &str) -> Run {
    printed_call(root, assessment, &format!("{printed}{PASSED}"), &[])
}

/// A call for the printed measures of `assessment`, from an output that holds exactly
/// `printed`, with `more` arguments.
fn printed_call(root: &Path, assessment: &str, printed: &str, more: &[&str]) -> Run {
    let output = root.join("target/captured.out");
    place(root, "target/captured.out", printed);
    let mut arguments = vec![
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
    ];
    arguments.extend_from_slice(more);
    assess(root, &arguments)
}

/// The coverage of a document: executed, passed and failed.
fn coverage(document: &str) -> [u64; 3] {
    let document: Value = serde_json::from_str(document).expect("the document is JSON");
    ["executed", "passed", "failed"].map(|count| {
        document
            .pointer(&format!("/evidence/coverage/{count}"))
            .and_then(Value::as_u64)
            .expect("the coverage has the count")
    })
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
/// The pages state the run of the committed records and the slowest test of `workspace-tests`,
/// so each call names the run and the commit its committed record names, and the report is the
/// fixture with its slowest test at the time of that committed record.
#[test]
fn written_documents_in_the_place_of_the_committed_records_leave_the_documents_without_a_problem() {
    let root_directory = scratch_root();
    let mut documents = load();
    let committed = |assessment: &str| -> Value {
        let path = format!("quality/records/{assessment}.evidence.json");
        let text = documents
            .records
            .get(&path)
            .unwrap_or_else(|| panic!("{assessment} has a committed record"));
        serde_json::from_str(text).expect("the committed record is JSON")
    };
    let field = |record: &Value, pointer: &str| -> String {
        record
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("the committed record has {pointer}"))
            .to_owned()
    };
    let slowest = field(&committed("workspace-tests"), "/evidence/results/0/value");
    let fixture_report = fixture_text("report.junit.xml");
    assert_eq!(
        fixture_report.matches("time=\"8.011\"").count(),
        1,
        "the fixture report has its slowest test at 8.011 s"
    );
    place(
        &root_directory,
        "target/report.junit.xml",
        &fixture_report.replace("time=\"8.011\"", &format!("time=\"{slowest}\"")),
    );
    let junit = root_directory.join("target/report.junit.xml");
    let mut written = Vec::new();
    for assessment in ["workspace-tests", "release-statuses"] {
        let record = committed(assessment);
        let revision = field(&record, "/evidence/target/revision");
        let source = field(&record, "/evidence/source/location");
        let document = assess(
            &root_directory,
            &[
                "--project",
                "inference",
                "--assessment",
                assessment,
                "--class",
                "ci",
                "--revision",
                &revision,
                "--from",
                path(&junit),
                "--source",
                &source,
            ],
        )
        .written();
        written.push((assessment, document));
    }
    let sherpa_source = field(
        &committed("sherpa-onnx-stt-record"),
        "/evidence/source/location",
    );
    for (assessment, document) in written {
        let path = format!("quality/records/{assessment}.evidence.json");
        assert!(
            documents.records.insert(path, document).is_some(),
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
            &sherpa_source,
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
        )
        .replace("tests=\"22\" skipped=\"1\"", "tests=\"22\" skipped=\"2\""),
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
    fs::write(
        &no_phonic,
        format!("{}{}", &full[..start], &full[end..]).replace("tests=\"22\"", "tests=\"21\""),
    )
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
        &["compare/first-audio: the program reads no single printed line of this measure"],
    );
    call(
        &root,
        "workspace-tests",
        &ci,
        &[&junit[..], &["--exit-status", "0"]].concat(),
    )
    .ended(
        2,
        &["--exit-status is the exit status of a command whose output is read, and workspace-tests reads a JUnit report"],
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

/// The macros whose first string literal is text a command prints.
const PRINTING: [&str; 6] = ["println", "eprintln", "print", "format", "write", "writeln"];

/// The first string literal of every printing macro of `source`, outside comments. The literal
/// of another macro (`panic!`, `assert!`) is not one. Not read: a literal built by `concat!`, and
/// a format string passed in a variable.
fn printed_literals(source: &str) -> Vec<String> {
    let text: Vec<char> = source.chars().collect();
    let mut literals = Vec::new();
    let mut pending: Option<bool> = None;
    let mut at = 0;
    while at < text.len() {
        let rest = &text[at..];
        if rest.starts_with(&['/', '/']) {
            while at < text.len() && text[at] != '\n' {
                at += 1;
            }
        } else if rest.starts_with(&['/', '*']) {
            at += 2;
            while at < text.len() && !text[at..].starts_with(&['*', '/']) {
                at += 1;
            }
            at += 2;
        } else if text[at] == '"' {
            let mut literal = String::new();
            at += 1;
            while at < text.len() && text[at] != '"' {
                if text[at] == '\\' && at + 1 < text.len() {
                    literal.push(text[at]);
                    at += 1;
                }
                literal.push(text[at]);
                at += 1;
            }
            at += 1;
            if pending.take() == Some(true) {
                literals.push(literal);
            }
        } else if text[at] == '\'' {
            at += match (text.get(at + 1), text.get(at + 2)) {
                (Some('\\'), _) => text
                    .get(at + 3..)
                    .and_then(|rest| rest.iter().position(|character| *character == '\''))
                    .map_or(text.len(), |end| end + 4),
                (Some(_), Some('\'')) => 3,
                _ => 1,
            };
        } else if text[at].is_alphabetic() || text[at] == '_' {
            let start = at;
            while at < text.len() && (text[at].is_alphanumeric() || text[at] == '_') {
                at += 1;
            }
            let name: String = text[start..at].iter().collect();
            let mut next = at;
            while next < text.len() && text[next].is_whitespace() {
                next += 1;
            }
            if text.get(next) == Some(&'!') {
                pending = Some(PRINTING.contains(&name.as_str()));
            }
        } else {
            at += 1;
        }
    }
    literals
}

/// Whether a printing macro of `source` writes `label`, and `marker` after it, in one literal,
/// with a `{...}` placeholder after the label, or after the last `marker` behind it, past spaces
/// only: where the program reads the value.
fn prints(source: &str, label: &str, marker: Option<&str>) -> bool {
    printed_literals(source).iter().any(|literal| {
        let Some((_, after)) = literal.split_once(label) else {
            return false;
        };
        let value = match marker {
            None => after,
            Some(marker) => match after.rfind(marker) {
                Some(at) => &after[at + marker.len()..],
                None => return false,
            },
        };
        let value = value.trim_start_matches(' ');
        value.starts_with('{') && !value.starts_with("{{") && value.contains('}')
    })
}

/// The check of the labels below holds only what a printing macro writes, and only where the
/// value follows its label.
#[test]
fn the_label_check_takes_a_label_only_from_the_text_a_printing_macro_writes() {
    let printed =
        "fn run() {\n    println!(\"first delta {:?} after the start: {:?}\", a, b);\n}\n";
    assert!(prints(printed, "first delta", None));
    assert!(prints(printed, "first delta", Some("after the start:")));
    assert!(!prints(printed, "first delta", Some("after the start")));
    let escaped_brace = "fn run() {\n    println!(\"first delta {{none}} {:?}\", a);\n}\n";
    assert!(!prints(escaped_brace, "first delta", None));
    let commented = "//! Cancels it after the first delta and checks the slots.\nfn run() {\n    // first delta\n    /* first delta */\n    println!(\"done\");\n}\n";
    assert!(!prints(commented, "first delta", None));
    let panicked = "fn run() {\n    panic!(\"the usage was taken before its first delta: {x:?}\");\n    assert!(ok, \"first delta\");\n}\n";
    assert!(!prints(panicked, "first delta", None));
    let without_marker = "fn run() {\n    println!(\"P1 result: last audio {:?} ms\", a);\n}\n";
    assert!(prints(without_marker, "P1 result:", Some("last audio")));
    assert!(!prints(without_marker, "P1 result:", None));
    assert!(!prints(without_marker, "P1 result:", Some("first audio")));
    let marker_before = "fn run() {\n    println!(\"first audio then P1 result: {:?}\", a);\n}\n";
    assert!(!prints(marker_before, "P1 result:", Some("first audio")));
    let escaped = "fn run() {\n    writeln!(out, \"[\\\"x\\\"] closed in {} ms: {c:?}\", t)?;\n    let c = 'a';\n    let q = '\\'';\n    let s: &'static str = \"closed in\";\n}\n";
    assert!(prints(escaped, "closed in", None));
    assert!(!prints(escaped, "closed in", Some(" ms")));
    assert_eq!(
        printed_literals(escaped).len(),
        1,
        "{:?}",
        printed_literals(escaped)
    );
}

/// The program finds a printed measure by the text before its value, and for some by a second
/// text the value follows. A printing macro of the source of the measure's command writes both,
/// the second after the first, in one string literal, with a placeholder right after the one
/// the value follows: run on an output that holds no measure, the program names, for each
/// measure of a plan of `printed-duration@1` that it reads, the text it found no line of and the
/// text its value follows.
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
            let command = assessment
                .pointer("/observation/command")
                .and_then(Value::as_str)
                .expect("a printed measure has a command");
            let example: &[&str] = if command.contains("--example") {
                &["--exit-status", "0"]
            } else {
                &[]
            };
            let run = printed_call(&root_directory, id, PASSED, example);
            if run.status == Some(2) {
                assert!(
                    run.stderr
                        .contains("the program reads no single printed line of this measure")
                        || run.stderr.contains(
                            "its exit status does not tell a completed run from a failed one"
                        ),
                    "{id}: {}",
                    run.stderr
                );
                continue;
            }
            assert_eq!(run.status, Some(1), "{id}: {}", run.stderr);
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
                let marker = line
                    .split_once("; its value follows `")
                    .and_then(|(_, rest)| rest.split('`').next());
                assert!(
                    !label.is_empty() && prints(&source, label, marker),
                    "{id}/{name}: no printing macro of {source_path} writes {label:?} with {marker:?} after it"
                );
                read.insert(format!("{id}/{name}"));
            }
        }
    }
    assert_eq!(read.len(), 12, "the printed measures read: {read:?}");
}

/// Two commands print the text of their measure on more than one line in every run that ends:
/// the example of `inference-elevenlabs` once per output format, the tests of
/// `local-voice-cascade` once per turn and per test. The program reads neither.
#[test]
fn two_commands_that_print_their_measure_more_than_once_per_run_are_not_read() {
    let root = scratch_root();
    printed_call(
        &root,
        "elevenlabs-live-example",
        "ulaw_8000: first audio after 412 ms, 30 frames, 1.20 s of audio, all mu-law at 8000 Hz mono\npcm_24000: first audio after 380 ms, 30 frames, 1.20 s of audio, all 16-bit little-endian PCM at 24000 Hz mono\n",
        &["--exit-status", "0"],
    )
    .ended(
        2,
        &["elevenlabs-live-example/first-audio: the program reads no single printed line of this measure"],
    );
    from_output(
        &root,
        "cascade-local-adapters",
        "committed input to first playable audio, turn 1: Some(640.25ms)\ninterruption to playback stop, turn 1: 18ms\n",
    )
    .ended(
        2,
        &[
            "cascade-local-adapters/first-audio: the program reads no single printed line of this measure",
        ],
    );
}

/// The output of `cargo test` says whether the measured test passed. A run whose test passed is
/// one passed run: its closing line, `test result: ok.` of cargo test or `Summary` of
/// cargo-nextest, says that its tests passed.
#[test]
fn the_output_of_a_live_test_that_passed_is_one_passed_run() {
    let root = scratch_root();
    let printed = "running 3 tests\nstarted in 812 ms: SessionId(\"s-1\")\nclosed in 95 ms: Ok(Closed)\ntest the_client_hears_speech_and_answers_with_audio_and_transcript ... ok\ntest one_spoken_exchange_through_the_duplex_contract ... ok\ntest native_duplex_evidence_of_gpt_live ... ok\n\ntest result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 21.40s\n";
    let written = printed_call(&root, "openai-gpt-live", printed, &[]).written();
    assert_eq!(values(&written), ["812", "95"]);
    assert_eq!(coverage(&written), [1, 1, 0]);
    let nextest = "started in 812 ms: SessionId(\"s-1\")\nclosed in 95 ms: Ok(Closed)\n        PASS [  21.400s] (1/1) inference-openai::live_gpt the_client_hears_speech_and_answers_with_audio_and_transcript\n────────────\n     Summary [  21.400s] 1 test run: 1 passed, 2 skipped\n";
    let written = printed_call(&root, "openai-gpt-live", nextest, &[]).written();
    assert_eq!(coverage(&written), [1, 1, 0]);
}

/// A live test that printed its measures and then failed gives no record: the values of a run
/// whose test failed are not written as those of a run that held.
#[test]
fn the_output_of_a_live_test_that_failed_gives_no_record() {
    let root = scratch_root();
    let measures = "started in 812 ms: SessionId(\"s-1\")\nclosed in 95 ms: Err(Timeout)\n";
    let test = "the_client_hears_speech_and_answers_with_audio_and_transcript";
    let panic = format!(
        "thread '{test}' panicked at adapters/inference-openai/tests/live_gpt.rs:266:25:\nsession.closed confirms the final usage: Timeout\n"
    );
    let failed = format!(
        "{measures}{panic}test {test} ... FAILED\ntest one_spoken_exchange_through_the_duplex_contract ... ok\n\nfailures:\n    {test}\n\ntest result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 21.40s\n"
    );
    printed_call(&root, "openai-gpt-live", &failed, &[]).ended(
        1,
        &[
            format!("openai-gpt-live: the output says the test {test} failed").as_str(),
            "openai-gpt-live: the output says `test result: FAILED. 2 passed; 1 failed;",
        ],
    );
    let pinned_panic = panic.replacen("' panicked at", "' (4123015) panicked at", 1);
    for panic in [&panic, &pinned_panic] {
        let cut = format!("{measures}test {test} ... {panic}");
        printed_call(&root, "openai-gpt-live", &cut, &[]).ended(
            1,
            &[format!("openai-gpt-live: the output says the test {test} panicked").as_str()],
        );
    }
    let nextest =
        format!("{measures}        FAIL [  21.400s] (1/1) inference-openai::live_gpt {test}\n");
    printed_call(&root, "openai-gpt-live", &nextest, &[]).ended(
        1,
        &[format!(
            "openai-gpt-live: the output says the test inference-openai::live_gpt {test} failed"
        )
        .as_str()],
    );
    printed_call(
        &root,
        "openai-gpt-live",
        &format!("{measures}{PASSED}"),
        &["--exit-status", "101"],
    )
    .ended(1, &["openai-gpt-live: its command exited 101"]);
    let without: &[&str] = &[];
    for status in [without, &["--exit-status", "0"]] {
        printed_call(&root, "openai-gpt-live", measures, status).ended(
            1,
            &["openai-gpt-live: the output ends before the closing line of its run"],
        );
    }
}

/// `cascade-talk` ends by an interrupt (Ctrl-C), and its example exits 0 also when its run ended
/// Failed: neither its output nor its exit status tells a completed run from a failed one. The
/// program reads no record of it, with or without an exit status.
#[test]
fn the_output_of_an_example_whose_exit_status_does_not_say_whether_its_run_held_is_not_read() {
    let root = scratch_root();
    let printed = "[committed input to first playable audio: 640ms]\n[interruption to playback stop: 18ms]\n[egress to 203.0.113.7:443: the run failed]\n[the run ended Failed]\n";
    let without: &[&str] = &[];
    for status in [without, &["--exit-status", "0"], &["--exit-status", "130"]] {
        printed_call(&root, "cascade-talk", printed, status).ended(
            2,
            &["cascade-talk: its run ends by an interrupt (Ctrl-C), and its exit status does not tell a completed run from a failed one"],
        );
    }
}

/// A named test `<file>::<function>` is the function of that file. A test of the same name in a
/// module of another file does not stand in for it when the named one did not run.
#[test]
fn a_test_of_the_same_name_in_another_module_does_not_hold_a_named_test() {
    let root = scratch_root();
    let moved = root.join("moved.junit.xml");
    fs::write(
        &moved,
        fixture_text("report.junit.xml").replace(
            "status::tests::a_hosted_binding_on_main_is_a_next_item_and_one_since_a_release_a_shipped_item",
            "status::legacy::tests::a_hosted_binding_on_main_is_a_next_item_and_one_since_a_release_a_shipped_item",
        ),
    )
    .expect("the report is written");
    from_check_run(&root, "release-statuses", &moved).ended(
        1,
        &["release-statuses: the report holds no test crates/inference-docs/src/status.rs::a_hosted_binding_on_main_is_a_next_item_and_one_since_a_release_a_shipped_item of the package inference-docs"],
    );
}

/// Adds the assessment `placed` of `named-tests@1`, for the one test `test` of the package
/// `inference-example`, to the plans of `root`, and writes `source` as the file of the test.
fn plan_one_named_test(root: &Path, test: &str, source: &str) {
    let (file, _) = test.split_once("::").expect("<file>::<function>");
    place(root, file, source);
    let plan = format!(
        "format: ratchet/1\nproject: inference\n\nassessments:\n  placed:\n    subject: source-tree\n    assessor: named-tests@1\n    inputs:\n      package: inference-example\n      tests:\n        - {test}\n    target: {{kind: revision, environments: [ci], selection: candidate}}\n    observation:\n      kind: snapshot\n      command: task test-run\n    measurements:\n      held: {{kind: exact, output: passed, scope: test}}\n"
    );
    place(root, "quality/placed.ratchet.yaml", &plan);
    let config = fs::read_to_string(root.join("ratchet.toml")).expect("ratchet.toml is read");
    if !config.contains("quality/placed.ratchet.yaml") {
        place(
            root,
            "ratchet.toml",
            &config.replacen(
                "paths = [",
                "paths = [\n  \"quality/placed.ratchet.yaml\",",
                1,
            ),
        );
    }
}

/// A report of one passed test case `<classname> <name>`, at `one.junit.xml` under `root`.
fn one_case_report(root: &Path, classname: &str, name: &str) -> PathBuf {
    let report = root.join("one.junit.xml");
    fs::write(
        &report,
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites name=\"nextest-run\" tests=\"1\" skipped=\"0\" failures=\"0\" errors=\"0\" timestamp=\"2026-11-24T21:02:05.400+01:00\" time=\"1.000\">\n    <testsuite name=\"{classname}\" tests=\"1\" skipped=\"0\" errors=\"0\" failures=\"0\">\n        <testcase name=\"{name}\" classname=\"{classname}\" timestamp=\"2026-11-24T21:02:05.500+01:00\" time=\"0.100\"/>\n    </testsuite>\n</testsuites>\n"
        ),
    )
    .expect("the report is written");
    report
}

/// Whether the report of one test case `<classname> <name>` holds the named test of `placed`.
fn holds(root: &Path, classname: &str, name: &str) -> bool {
    let report = one_case_report(root, classname, name);
    let run = from_check_run(root, "placed", &report);
    match run.status {
        Some(0) => true,
        Some(1) if run.stderr.contains("placed: the report holds no test ") => false,
        _ => panic!("{classname} {name}: {:?} {}", run.status, run.stderr),
    }
}

/// A file whose named test stands in a `mod tests`, with a function of the same name outside it
/// in no other place.
const IN_TESTS: &str = "pub fn rule() -> bool {\n    true\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn the_rule_holds() {\n        assert!(rule());\n    }\n}\n";

/// A file whose named test stands outside its `mod tests`.
const AT_THE_TOP: &str = "#[test]\nfn the_rule_holds() {}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn another_rule_holds() {}\n}\n";

/// A file whose named test stands in two inline modules, after a module that closed before it,
/// with a `mod`, a brace and a quote in comments, strings and characters that open no module.
const NESTED: &str = "mod helpers {\n    pub fn wait() {}\n}\n\n#[cfg(feature = \"live\")]\nmod live {\n    use super::*;\n\n    // mod commented {\n    /* mod blocked { /* } */ } */\n    const TEXT: &str = \"mod quoted { \\\" }\";\n    const RAW: &str = r#\"mod raw { \" }\"#;\n    const BRACE: char = '{';\n    const QUOTE: char = '\\'';\n\n    pub(crate) mod nested {\n        fn helper<'a>(text: &'a str) -> &'a str {\n            text\n        }\n\n        #[tokio::test]\n        async fn the_bound_holds() {\n            let _ = (helper(\"}\"), '}');\n        }\n    }\n}\n";

/// A named test is held only by the test case whose name is the module path of its file, the
/// inline modules around its function in that file, and the function: nothing between them,
/// nothing before them, and `tests` only where the file has the function in a `mod tests`.
#[test]
fn a_named_test_is_held_only_by_the_module_path_of_its_file_and_its_function() {
    let root = scratch_root();
    let library = "inference-example";
    let binary = "inference-example::bin/inference-example";
    let status = "crates/example/src/status.rs::the_rule_holds";
    plan_one_named_test(&root, status, IN_TESTS);
    for (classname, name, expected) in [
        (library, "status::tests::the_rule_holds", true),
        (binary, "status::tests::the_rule_holds", true),
        (library, "status::the_rule_holds", false),
        (library, "status::legacy::tests::the_rule_holds", false),
        (library, "legacy::status::tests::the_rule_holds", false),
        (library, "status::tests::inner::the_rule_holds", false),
        (library, "tests::the_rule_holds", false),
        (
            "inference-example::stall",
            "status::tests::the_rule_holds",
            false,
        ),
    ] {
        assert_eq!(
            holds(&root, classname, name),
            expected,
            "src, in mod tests: {classname} {name}"
        );
    }
    plan_one_named_test(&root, status, AT_THE_TOP);
    for (classname, name, expected) in [
        (library, "status::the_rule_holds", true),
        (library, "status::tests::the_rule_holds", false),
    ] {
        assert_eq!(
            holds(&root, classname, name),
            expected,
            "src, at the top: {classname} {name}"
        );
    }
    let stall = "crates/example/tests/stall.rs::the_bound_holds";
    plan_one_named_test(&root, stall, "#[test]\nfn the_bound_holds() {}\n");
    for (classname, name, expected) in [
        ("inference-example::stall", "the_bound_holds", true),
        ("inference-example::stall", "tests::the_bound_holds", false),
        ("inference-example::stall", "inner::the_bound_holds", false),
        ("inference-example::other", "the_bound_holds", false),
        (library, "the_bound_holds", false),
    ] {
        assert_eq!(
            holds(&root, classname, name),
            expected,
            "tests/x.rs: {classname} {name}"
        );
    }
    plan_one_named_test(&root, stall, NESTED);
    for (name, expected) in [
        ("live::nested::the_bound_holds", true),
        ("live::the_bound_holds", false),
        ("nested::the_bound_holds", false),
        ("the_bound_holds", false),
        ("helpers::live::nested::the_bound_holds", false),
        ("live::commented::nested::the_bound_holds", false),
        ("live::blocked::nested::the_bound_holds", false),
        ("live::quoted::nested::the_bound_holds", false),
        ("live::raw::nested::the_bound_holds", false),
        ("tests::live::nested::the_bound_holds", false),
    ] {
        assert_eq!(
            holds(&root, "inference-example::stall", name),
            expected,
            "tests/x.rs, nested: {name}"
        );
    }
    plan_one_named_test(
        &root,
        "crates/example/tests/quality/assess.rs::the_record_is_written",
        "#[test]\nfn the_record_is_written() {}\n",
    );
    for (classname, name, expected) in [
        (
            "inference-example::workflows",
            "quality::assess::the_record_is_written",
            true,
        ),
        (
            "inference-example::quality",
            "assess::the_record_is_written",
            true,
        ),
        (
            "inference-example::workflows",
            "quality::assess::tests::the_record_is_written",
            false,
        ),
        (
            "inference-example::workflows",
            "quality::assess::legacy::the_record_is_written",
            false,
        ),
        (
            "inference-example::workflows",
            "other::quality::assess::the_record_is_written",
            false,
        ),
        (
            "inference-example::workflows",
            "quality::the_record_is_written",
            false,
        ),
        ("inference-example::quality", "the_record_is_written", false),
    ] {
        assert_eq!(
            holds(&root, classname, name),
            expected,
            "tests/dir: {classname} {name}"
        );
    }
}

/// The name of a named test is read from its file: a file that defines no function of that name,
/// or that the root does not hold, gives no record, also from a report whose test case has the
/// name the test would have.
#[test]
fn a_named_test_whose_file_does_not_define_it_or_is_not_in_the_root_is_refused() {
    let root = scratch_root();
    let report = one_case_report(&root, "inference-example", "status::tests::the_rule_holds");
    plan_one_named_test(
        &root,
        "crates/example/src/status.rs::the_rule_holds",
        "#[cfg(test)]\nmod tests {\n    // fn the_rule_holds() {}\n    const NAME: &str = \"fn the_rule_holds() {}\";\n}\n",
    );
    from_check_run(&root, "placed", &report).ended(
        1,
        &["placed: crates/example/src/status.rs holds no function the_rule_holds"],
    );
    plan_one_named_test(
        &root,
        "crates/example/src/status.rs::the_rule_holds",
        IN_TESTS,
    );
    fs::remove_file(root.join("crates/example/src/status.rs")).expect("the file is removed");
    from_check_run(&root, "placed", &report)
        .ended(1, &["placed: crates/example/src/status.rs cannot be read"]);
}

/// The name cargo-nextest gives the named test `<file>::<function>` of `package`, the binary it
/// gives it to, and the inline modules around the function: the module path of the file in its
/// crate, each `mod <name> {` around the function, read here from the indentation of the file
/// as rustfmt writes it, and the function.
fn nextest_name(package: &str, test: &str) -> (String, String, usize) {
    let (file, function) = test.split_once("::").expect("<file>::<function>");
    let source =
        fs::read_to_string(root().join(file)).unwrap_or_else(|error| panic!("{file}: {error}"));
    let lines: Vec<&str> = source.lines().collect();
    let indent = |line: &str| line.len() - line.trim_start().len();
    let bare = |line: &str| -> String {
        let mut rest = line.trim_start();
        while let Some(after) = ["pub(crate) ", "pub(super) ", "pub ", "async ", "unsafe "]
            .iter()
            .find_map(|word| rest.strip_prefix(word))
        {
            rest = after;
        }
        rest.to_owned()
    };
    let at = lines
        .iter()
        .position(|line| {
            let line = bare(line);
            line.starts_with(&format!("fn {function}("))
                || line.starts_with(&format!("fn {function}<"))
        })
        .unwrap_or_else(|| panic!("{file} has no line that starts the function {function}"));
    let mut level = indent(lines[at]);
    let mut inline = Vec::new();
    for &line in lines[..at].iter().rev() {
        if line.trim().is_empty() || indent(line) >= level {
            continue;
        }
        level = indent(line);
        if let Some(name) = bare(line)
            .strip_prefix("mod ")
            .and_then(|rest| rest.strip_suffix(" {"))
        {
            inline.push(name.to_owned());
        }
    }
    inline.reverse();
    let (classname, mut path) = if let Some((directory, rest)) = file.split_once("/src/") {
        let source_directory = root().join(directory).join("src");
        let binary = source_directory.join("main.rs").is_file()
            && !source_directory.join("lib.rs").is_file();
        let classname = if binary {
            format!("{package}::bin/{package}")
        } else {
            package.to_owned()
        };
        let modules: Vec<String> = rest
            .trim_end_matches(".rs")
            .split('/')
            .filter(|part| !matches!(*part, "lib" | "main" | "mod"))
            .map(str::to_owned)
            .collect();
        (classname, modules)
    } else if let Some((_, rest)) = file.split_once("/tests/")
        && !rest.contains('/')
    {
        (
            format!("{package}::{}", rest.trim_end_matches(".rs")),
            Vec::new(),
        )
    } else {
        panic!("{test}: this test gives no name of cargo-nextest to a test of {file}");
    };
    let modules = inline.len();
    path.extend(inline);
    path.push(function.to_owned());
    (classname, path.join("::"), modules)
}

/// cargo-nextest names a test by its module path in its binary: the function of
/// `tests/transport_deadlines.rs` that stands in the file's `mod live` is `live::<function>`.
/// For every committed record of `named-tests@1`, a report in which its named tests passed under
/// the names cargo-nextest gives them, with the record's class, revision and source, gives the
/// committed record byte for byte.
#[test]
fn every_named_tests_record_comes_out_byte_equal_from_a_report_under_the_names_of_cargo_nextest() {
    let root_directory = scratch_root();
    let mut records: Vec<PathBuf> = fs::read_dir(root().join("quality/records"))
        .expect("the records are listed")
        .map(|entry| entry.expect("an entry of the directory").path())
        .collect();
    records.sort();
    let mut checked = Vec::new();
    let mut differ = Vec::new();
    let mut inline = 0;
    for record_path in records {
        let committed = fs::read_to_string(&record_path).expect("the committed record is read");
        let record: Value = serde_json::from_str(&committed).expect("the record is JSON");
        if record.pointer("/evidence/assessor").and_then(Value::as_str) != Some("named-tests@1") {
            continue;
        }
        let field = |pointer: &str| {
            record
                .pointer(pointer)
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("{}: the record has {pointer}", record_path.display()))
                .to_owned()
        };
        let id = field("/evidence/assessment");
        let package = field("/evidence/inputs/package/value");
        let tests: Vec<String> = record
            .pointer("/evidence/inputs/tests/value")
            .and_then(Value::as_array)
            .expect("the record names its tests")
            .iter()
            .map(|test| test.as_str().expect("a test is a text").to_owned())
            .collect();
        let mut suites = String::new();
        for test in &tests {
            let (file, _) = test.split_once("::").expect("<file>::<function>");
            let text = fs::read_to_string(root().join(file)).expect("the file of the test is read");
            place(&root_directory, file, &text);
            let (classname, name, modules) = nextest_name(&package, test);
            if file.contains("/tests/") {
                inline += modules;
            }
            suites.push_str(&format!(
                "    <testsuite name=\"{classname}\" tests=\"1\" skipped=\"0\" errors=\"0\" failures=\"0\">\n        <testcase name=\"{name}\" classname=\"{classname}\" timestamp=\"2026-11-24T21:02:06.000+01:00\" time=\"0.100\"/>\n    </testsuite>\n"
            ));
        }
        let report = root_directory.join(format!("{id}.junit.xml"));
        fs::write(
            &report,
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites name=\"nextest-run\" tests=\"{}\" skipped=\"0\" failures=\"0\" errors=\"0\" timestamp=\"2026-11-24T21:02:05.400+01:00\" time=\"101.000\">\n{suites}</testsuites>\n",
                tests.len()
            ),
        )
        .expect("the report is written");
        let run = assess(
            &root_directory,
            &[
                "--project",
                "inference",
                "--assessment",
                &id,
                "--class",
                &field("/evidence/target/class"),
                "--revision",
                &field("/evidence/target/revision"),
                "--from",
                path(&report),
                "--source",
                &field("/evidence/source/location"),
            ],
        );
        if run.status != Some(0) || run.document.as_deref() != Some(committed.as_str()) {
            differ.push(format!("{id}: exit {:?}: {}", run.status, run.stderr));
        }
        checked.push(id);
    }
    assert!(differ.is_empty(), "{}", differ.join("\n"));
    assert_eq!(
        checked.len(),
        18,
        "the records of named-tests@1: {checked:?}"
    );
    assert!(
        inline > 0,
        "no named test of a test binary stands in an inline module: the case the test is for"
    );
}

/// `evidence` holds no absolute path (Ratchet, evidence format, section 4, rule 9), and a path
/// from the root does not leave the root.
#[test]
fn a_source_that_is_neither_a_web_url_nor_a_path_in_the_root_is_a_wrong_call() {
    let root = scratch_root();
    let report = fixture("report.junit.xml");
    let ci = ["--class", "ci", "--revision", REVISION];
    for source in [
        "file:///srv/runs/junit.xml",
        "../../outside/junit.xml",
        "target/../../junit.xml",
        "",
        "~/junit.xml",
        "https://",
    ] {
        call(
            &root,
            "workspace-tests",
            &ci,
            &["--from", path(&report), "--source", source],
        )
        .ended(2, &["--source"]);
    }
    for (source, kind) in [
        ("target/nextest/default/junit.xml", "junit"),
        (CHECK_RUN, "forge_runs"),
    ] {
        let written = call(
            &root,
            "workspace-tests",
            &ci,
            &["--from", path(&report), "--source", source],
        )
        .written();
        let document: Value = serde_json::from_str(&written).expect("the document is JSON");
        assert_eq!(
            document.pointer("/evidence/source"),
            Some(&serde_json::json!({"kind": kind, "location": source}))
        );
    }
}

/// The counts the root of a report states are those of the test cases the program read: a test
/// case it does not read, here one outside every `<testsuite>` and one in a nested one, is not
/// dropped in silence, and a report whose counts disagree with its cases is refused.
#[test]
fn a_report_whose_root_counts_disagree_with_the_cases_read_is_refused() {
    let root = scratch_root();
    let loose = root.join("loose.junit.xml");
    fs::write(
        &loose,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites name=\"nextest-run\" tests=\"3\" skipped=\"0\" failures=\"2\" errors=\"0\" timestamp=\"2026-11-24T21:02:05.400+01:00\" time=\"10.000\">\n    <testcase name=\"loose_failed\" classname=\"inference-core\" time=\"9.000\"><failure/></testcase>\n    <testsuite name=\"inference-core\" tests=\"2\" skipped=\"0\" errors=\"0\" failures=\"1\">\n        <testsuite name=\"inner\">\n            <testcase name=\"nested_failed\" classname=\"inference-core\" time=\"9.500\"><failure/></testcase>\n        </testsuite>\n        <testcase name=\"a_passed\" classname=\"inference-core\" time=\"0.100\"/>\n    </testsuite>\n</testsuites>\n",
    )
    .expect("the report is written");
    from_check_run(&root, "workspace-tests", &loose).ended(
        1,
        &[
            "holds the test case inference-core loose_failed outside a <testsuite> directly under <testsuites>",
            "holds the test case inference-core nested_failed outside a <testsuite> directly under <testsuites>",
        ],
    );
    let full = fixture_text("report.junit.xml");
    let root_line = "tests=\"22\" skipped=\"1\" failures=\"1\" errors=\"0\"";
    for (counts, said) in [
        (
            "tests=\"23\" skipped=\"1\" failures=\"1\" errors=\"0\"",
            "its root says tests=\"23\", and it holds 22 test cases",
        ),
        (
            "tests=\"22\" skipped=\"0\" failures=\"1\" errors=\"0\"",
            "its root says skipped=\"0\", and it holds 1 test case with <skipped>",
        ),
        (
            "tests=\"22\" skipped=\"1\" failures=\"2\" errors=\"0\"",
            "its root says failures=\"2\", and it holds 1 test case with <failure>",
        ),
        (
            "tests=\"22\" skipped=\"1\" failures=\"1\" errors=\"1\"",
            "its root says errors=\"1\", and it holds 0 test cases with <error>",
        ),
    ] {
        let report = root.join("counted.junit.xml");
        fs::write(&report, full.replacen(root_line, counts, 1)).expect("the report is written");
        from_check_run(&root, "workspace-tests", &report).ended(1, &[said]);
    }
}

/// The test of a capability record checks the record against its adapter. A record whose test
/// failed gives no value, as one whose test was skipped gives none.
#[test]
fn a_capability_record_whose_test_failed_is_refused() {
    let root = scratch_root();
    let failed = root.join("failed.junit.xml");
    fs::write(
        &failed,
        fixture_text("report.junit.xml")
            .replace(
                "classname=\"inference-sherpa-onnx::stt_record\" timestamp=\"2026-11-24T21:02:06.900+01:00\" time=\"0.494\"/>",
                "classname=\"inference-sherpa-onnx::stt_record\" timestamp=\"2026-11-24T21:02:06.900+01:00\" time=\"0.494\"><failure message=\"the record differs\"/></testcase>",
            )
            .replace("failures=\"1\" errors=\"0\" uuid", "failures=\"2\" errors=\"0\" uuid"),
    )
    .expect("the report is written");
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
            path(&failed),
            "--source",
            CHECK_RUN,
        ],
    )
    .ended(
        1,
        &["sherpa-onnx-stt-record: the report holds the test adapters/inference-sherpa-onnx/tests/stt_record.rs::the_resource_profile_is_a_measured_cpu_profile of the package inference-sherpa-onnx as failed"],
    );
}

/// The measures of `openai-gpt-live` as its test prints them, before the test ends.
const GPT_MEASURES: &str = "started in 812 ms: SessionId(\"s-1\")\nclosed in 95 ms: Err(Timeout)\n";
const GPT_MEASURED: &str = "the_client_hears_speech_and_answers_with_audio_and_transcript";
const GPT_BINARY: &str = "inference-openai::live_gpt";
/// What the program says of an output without the closing line of its run.
const UNENDED: &str = "openai-gpt-live: the output ends before the closing line of its run";

/// The run ended before libtest reported the measured test: it printed its measures, another
/// test of the binary had passed, and then the process was killed or aborted. The `ok` of another
/// test is not the pass of the run: without the closing line of the run the output gives no
/// record, with or without an exit status and with or without its measures.
#[test]
fn a_run_that_ended_before_the_measured_test_reported_gives_no_record() {
    let root = scratch_root();
    let other = "test one_spoken_exchange_through_the_duplex_contract ... ok\n";
    let killed = format!("running 3 tests\n{other}{GPT_MEASURES}");
    printed_call(&root, "openai-gpt-live", &killed, &[]).ended(1, &[UNENDED]);
    printed_call(&root, "openai-gpt-live", &killed, &["--exit-status", "0"]).ended(1, &[UNENDED]);
    printed_call(
        &root,
        "openai-gpt-live",
        &format!("running 3 tests\n{other}"),
        &[],
    )
    .ended(
        1,
        &[
            UNENDED,
            "openai-gpt-live/session-start: no line of the output holds `started in`",
        ],
    );
    let aborted = format!(
        "running 3 tests\n{other}{GPT_MEASURES}\nthread '{GPT_MEASURED}' (4123015) has overflowed its stack\nfatal runtime error: stack overflow, aborting\nerror: test failed, to rerun pass `-p inference-openai --test live_gpt`\n\nCaused by:\n  process didn't exit successfully: `target/debug/deps/live_gpt-0123456789abcdef --ignored --nocapture` (signal: 6, SIGABRT: process abort signal)\n"
    );
    printed_call(&root, "openai-gpt-live", &aborted, &[]).ended(
        1,
        &[
            UNENDED,
            "openai-gpt-live: the output says `error: test failed, to rerun pass",
        ],
    );
}

/// `cargo test` closes the tests of each binary with its own `test result:` line. The closing
/// line of another binary does not close the run of the binary that printed the measures: a
/// binary killed after its measures, after another one passed, gives no record. A pass is read
/// from the `test result:` line of the tests that printed the measures, and a `running` line
/// whose tests never closed before the next one is a run that did not end.
#[test]
fn the_closing_line_of_another_test_binary_does_not_pass_the_run() {
    let root = scratch_root();
    let first = "running 3 tests\ntest a ... ok\ntest b ... ok\ntest c ... ok\n\ntest result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s\n\n";
    let killed = format!("{first}running 1 test\n{GPT_MEASURES}");
    printed_call(&root, "openai-gpt-live", &killed, &[]).ended(1, &[UNENDED]);
    printed_call(&root, "openai-gpt-live", &killed, &["--exit-status", "0"]).ended(1, &[UNENDED]);
    let unclosed = format!("running 1 test\n{GPT_MEASURES}{first}");
    printed_call(&root, "openai-gpt-live", &unclosed, &[]).ended(1, &[UNENDED]);
    let none_passed = format!(
        "running 1 test\n{GPT_MEASURES}\ntest result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s\n\n{first}"
    );
    printed_call(&root, "openai-gpt-live", &none_passed, &[]).ended(
        1,
        &["openai-gpt-live: the `test result:` line of the tests that printed the measures says that no test passed"],
    );
    let measured = format!(
        "{first}running 1 test\n{GPT_MEASURES}test {GPT_MEASURED} ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.00s\n"
    );
    let written = printed_call(&root, "openai-gpt-live", &measured, &[]).written();
    assert_eq!(values(&written), ["812", "95"]);
    assert_eq!(coverage(&written), [1, 1, 0]);
}

/// The rustc of `rust-toolchain.toml` prints a panic as `thread '<name>' (<id>) panicked at`, as
/// the failures of this suite show. An output cut after the panic of the measured test, in that
/// form.
#[test]
fn a_panic_in_the_form_the_pinned_rustc_prints_is_read_as_a_failure_of_its_test() {
    let root = scratch_root();
    let cut = format!(
        "{GPT_MEASURES}test {GPT_MEASURED} ... thread '{GPT_MEASURED}' (4123015) panicked at adapters/inference-openai/tests/live_gpt.rs:266:25:\nsession.closed confirms the final usage: Timeout\n"
    );
    printed_call(&root, "openai-gpt-live", &cut, &[]).ended(
        1,
        &[format!("openai-gpt-live: the output says the test {GPT_MEASURED} panicked").as_str()],
    );
}

/// A print that keeps the text of its measure and puts other words between it and the value
/// gives an output the program reads no value from. The label check of the sources must fail
/// for that change of the source; here it is made to the source of `deepgram-live`.
#[test]
fn the_label_check_fails_for_a_print_that_no_longer_puts_the_value_after_its_label() {
    let source_path = "adapters/inference-deepgram/tests/live.rs";
    let source = fs::read_to_string(root().join(source_path)).expect("the source is read");
    let printed = "first event after {first:?}";
    assert!(source.contains(printed), "{source_path} prints {printed:?}");
    assert!(prints(&source, "first event after", None));
    let mutated = source.replace(printed, "first event after the start: {first:?}");
    let root_directory = scratch_root();
    printed_call(
        &root_directory,
        "deepgram-live",
        &format!("first event after the start: 412ms; committed true\n{PASSED}"),
        &[],
    )
    .ended(1, &["holds no duration after `first event after`"]);
    assert!(
        !prints(&mutated, "first event after", None),
        "the label check passes a source whose print the program reads no value from"
    );
}

/// cargo-nextest 0.9.143 run with retries writes the last failure of a test as `TRY <n> FAIL`
/// and a timeout as `TRY <n> TMT`, in the line of the test and in the list after `Summary`. The
/// measured test failed before its measures on the first try and after them on the second;
/// another test passed. The output gives no record.
#[test]
fn a_failure_that_cargo_nextest_reports_after_a_retry_gives_no_record() {
    let root = scratch_root();
    let other = "one_spoken_exchange_through_the_duplex_contract";
    for (first, last, counted) in [
        ("TRY 1 FAIL", "TRY 2 FAIL", "1 failed"),
        ("TRY 1 TMT", "TRY 2 TMT", "1 timed out"),
    ] {
        let summary = format!("Summary [  26.400s] 2 tests run: 1 passed, {counted}, 0 skipped");
        let printed = format!(
            "  {first} [   0.400s] (───) {GPT_BINARY} {GPT_MEASURED}\n  RETRY 2/2 [         ] (───) {GPT_BINARY} {GPT_MEASURED}\n{GPT_MEASURES}TRY 2 TRMNTG [> 21.000s] (───) {GPT_BINARY} {GPT_MEASURED}\n  {last} [  21.400s] (1/2) {GPT_BINARY} {GPT_MEASURED}\n        PASS [   5.000s] (2/2) {GPT_BINARY} {other}\n────────────\n     {summary}\n  {last} [  21.400s] (1/2) {GPT_BINARY} {GPT_MEASURED}\nerror: test run failed\n"
        );
        printed_call(&root, "openai-gpt-live", &printed, &[]).ended(
            1,
            &[
                format!(
                    "openai-gpt-live: the output says the test {GPT_BINARY} {GPT_MEASURED} failed"
                )
                .as_str(),
                format!("openai-gpt-live: the output says `{summary}`").as_str(),
                "openai-gpt-live: the output says `error: test run failed`",
            ],
        );
    }
}

/// cargo-nextest without retries: a test ended at its timeout is `TMT` (`TIMEOUT` in older
/// releases). The counts of its `Summary` line say a failure also where no line of the test
/// stands in the output. A run whose `Summary` says that its tests passed is one passed run.
#[test]
fn cargo_nextest_says_a_timeout_or_a_failure_in_its_status_and_its_summary_lines() {
    let root = scratch_root();
    let summary = "Summary [  65.002s] 2 tests run: 1 passed, 1 timed out, 0 skipped";
    for status in ["TMT", "TIMEOUT"] {
        let printed = format!(
            "{GPT_MEASURES}     {status} [  60.002s] (1/2) {GPT_BINARY} {GPT_MEASURED}\n        PASS [   5.000s] (2/2) {GPT_BINARY} other\n────────────\n     {summary}\n"
        );
        printed_call(&root, "openai-gpt-live", &printed, &[]).ended(
            1,
            &[
                format!(
                    "openai-gpt-live: the output says the test {GPT_BINARY} {GPT_MEASURED} failed"
                )
                .as_str(),
                format!("openai-gpt-live: the output says `{summary}`").as_str(),
            ],
        );
    }
    for counted in [
        "1 passed, 1 failed, 0 skipped",
        "1 passed, 1 timed out, 0 skipped",
        "1 passed, 1 exec failed, 0 skipped",
        "0 passed, 2 skipped",
    ] {
        let summary = format!("Summary [  65.002s] 2 tests run: {counted}");
        let printed = format!(
            "{GPT_MEASURES}        PASS [   5.000s] (1/2) {GPT_BINARY} other\n────────────\n     {summary}\n"
        );
        let run = printed_call(&root, "openai-gpt-live", &printed, &[]);
        assert_eq!(run.status, Some(1), "{summary}: {}", run.stderr);
        assert!(run.document.is_none(), "{summary}: a record was written");
    }
    let passed = format!(
        "{GPT_MEASURES}        PASS [  21.400s] (1/1) {GPT_BINARY} {GPT_MEASURED}\n────────────\n     Summary [  21.400s] 1 test run: 1 passed (1 flaky), 2 skipped\n"
    );
    let written = printed_call(&root, "openai-gpt-live", &passed, &[]).written();
    assert_eq!(values(&written), ["812", "95"]);
    assert_eq!(coverage(&written), [1, 1, 0]);
}

/// cargo test and cargo-nextest colour their status words with `--color always` or
/// `CARGO_TERM_COLOR=always`. The colour codes are taken out before the output is read.
#[test]
fn a_coloured_output_is_read_as_the_same_output_without_colour() {
    let root = scratch_root();
    let green = |word: &str| format!("\u{1b}[32m{word}\u{1b}[0m");
    let red = |word: &str| format!("\u{1b}[1;31m{word}\u{1b}[0m");
    let passed = format!(
        "running 1 test\nstarted in \u{1b}[1m812\u{1b}[0m ms: SessionId(\"s-1\")\nclosed in 95 ms: Ok(Closed)\ntest {GPT_MEASURED} ... {}\n\ntest result: {}. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.00s\n",
        green("ok"),
        green("ok")
    );
    let written = printed_call(&root, "openai-gpt-live", &passed, &[]).written();
    assert_eq!(values(&written), ["812", "95"]);
    assert_eq!(coverage(&written), [1, 1, 0]);
    let nextest = format!(
        "{GPT_MEASURES}{} [  21.400s] (1/1) {GPT_BINARY} {GPT_MEASURED}\n────────────\n{} [  21.400s] 1 test run: 1 passed, 0 skipped\n",
        green("        PASS"),
        green("     Summary")
    );
    let written = printed_call(&root, "openai-gpt-live", &nextest, &[]).written();
    assert_eq!(coverage(&written), [1, 1, 0]);
    let failed = format!(
        "{GPT_MEASURES}test {GPT_MEASURED} ... {}\n\ntest result: {}. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.00s\n",
        red("FAILED"),
        red("FAILED")
    );
    printed_call(&root, "openai-gpt-live", &failed, &[]).ended(
        1,
        &[
            format!("openai-gpt-live: the output says the test {GPT_MEASURED} failed").as_str(),
            "openai-gpt-live: the output says `test result: FAILED. 0 passed; 1 failed;",
        ],
    );
}

/// A URL with user info would put a credential into a public record: it is a wrong call, and the
/// line on stderr does not repeat it. The scheme is read without regard to case. A URL path that
/// leaves its directory with `..`, the root (`.`) and a directory (a path that ends in `/`) are
/// no file of a run, and a file name with a colon reads as a URL of another scheme.
#[test]
fn a_source_with_user_info_a_parent_segment_or_a_directory_is_a_wrong_call() {
    let root = scratch_root();
    let report = fixture("report.junit.xml");
    let ci = ["--class", "ci", "--revision", REVISION];
    let run = |source: &str| {
        call(
            &root,
            "workspace-tests",
            &ci,
            &["--from", path(&report), "--source", source],
        )
    };
    for source in [
        "https://name:s3cr3t@github.com/beyond10x/inference/actions/runs/1",
        "HTTPS://t0ken@github.com/beyond10x/inference/actions/runs/1",
        "http://name:s3cr3t@github.com",
    ] {
        let refused = run(source);
        refused.ended(2, &["--source is a URL with user info"]);
        for secret in ["s3cr3t", "t0ken", "name:"] {
            assert!(
                !refused.stderr.contains(secret),
                "{source}: stderr repeats {secret}: {}",
                refused.stderr
            );
        }
    }
    for source in [
        "https://github.com/x/../../y",
        "https://github.com/x/..",
        "https://github.com/x/..?a=b",
        ".",
        "target/",
        "target/nextest/.",
        "run:1.xml",
        "target/run:1.xml",
    ] {
        run(source).ended(2, &["--source"]);
    }
    for source in [
        "HTTPS://github.com/beyond10x/inference/actions/runs/45474241015",
        "Http://github.com/x?mail=a@b",
        "https://github.com/x/a..b/#..",
    ] {
        let written = run(source).written();
        let document: Value = serde_json::from_str(&written).expect("the document is JSON");
        assert_eq!(
            document.pointer("/evidence/source"),
            Some(&serde_json::json!({"kind": "forge_runs", "location": source}))
        );
    }
}

/// cargo-nextest 0.9.143 writes no ignored test into its JUnit report: a run that skipped 72
/// tests has `skipped="0"` on its root and no `<skipped>` element. `nextest-run@1` reports no
/// `skipped` count, and its documents hold 0 there, as Ratchet's format says for a count an
/// assessor does not report, also from a report that holds a `<skipped>` case.
#[test]
fn the_workspace_run_reports_no_skipped_count_because_its_report_holds_none() {
    let text = fs::read_to_string(root().join("quality/assessors/nextest-run@1.assessor.yaml"))
        .expect("the declaration is read");
    let declaration: Value = serde_norway::from_str(&text).expect("the declaration parses");
    let reported: Vec<&str> = declaration
        .get("coverage")
        .and_then(Value::as_array)
        .expect("the declaration lists its counts")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(reported, ["executed", "passed", "failed"]);
    assert!(
        fixture_text("report.junit.xml").contains("<skipped/>"),
        "the fixture holds a skipped case"
    );
    let root_directory = scratch_root();
    let written = from_check_run(
        &root_directory,
        "workspace-tests",
        &fixture("report.junit.xml"),
    )
    .written();
    let document: Value = serde_json::from_str(&written).expect("the document is JSON");
    assert_eq!(
        document.pointer("/evidence/coverage/skipped"),
        Some(&Value::from(0))
    );
}

/// The acceptance of `story:quality-records-written-by-each-check-run`, as amended: a record
/// written as `task quality-records` writes it holds every rule the test of the quality documents
/// sets a record by itself and against its plan (`record_problems`), in the place of the committed
/// record of its assessment. On the forge the task gives the class `ci` and the URL of the run,
/// elsewhere the class `local` and the path of the report; `--revision` is the commit of the run,
/// here another one than the committed records name. The values the pages state and the run they
/// name are the next story's: no check of a page value is asked here.
#[test]
fn a_record_as_the_records_task_writes_it_holds_the_rules_of_a_record_in_the_place_of_its_own() {
    let root_directory = scratch_root();
    let junit = fixture("report.junit.xml");
    let other_commit = "0123456789abcdef0123456789abcdef01234567";
    for (class, source) in [
        (
            "ci",
            "https://github.com/beyond10x/inference/actions/runs/1",
        ),
        ("local", "target/nextest/default/junit.xml"),
    ] {
        let mut documents = load();
        for (assessment, revision) in [
            ("workspace-tests", true),
            ("adapter-tests", true),
            ("release-statuses", true),
            ("sherpa-onnx-stt-record", false),
        ] {
            let mut arguments = vec![
                "--project",
                "inference",
                "--assessment",
                assessment,
                "--class",
                class,
                "--from",
                path(&junit),
                "--source",
                source,
            ];
            if revision {
                arguments.extend(["--revision", other_commit]);
            }
            let written = assess(&root_directory, &arguments).written();
            assert!(
                documents
                    .records
                    .insert(
                        format!("quality/records/{assessment}.evidence.json"),
                        written
                    )
                    .is_some(),
                "{assessment} has a committed record"
            );
        }
        let found = record_problems(&documents);
        assert!(found.is_empty(), "class {class}: {}", report(&found));
    }
}
