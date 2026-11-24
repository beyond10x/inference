//! Cases against the shared conformance suite of `suite/mod.rs` and the bound of `task test-run`.
//!
//! The bound cases copy the command line of `task test-run` out of the repository's
//! `Taskfile.yml` into a Taskfile of their own, with the bound cut to one second and parts that
//! only sleep, and run it with `task`, which must be on `PATH` as for `task check`. The cases of
//! the slowest list copy the tasks `test-workspace` and `test-slowest` the same way.
//!
//! The cases named `adversary_w1_…` are those of an adversary pass over the first form of the
//! shared suite and the bound, where five of its six were red. Where one was rewritten with the
//! change that answered it, its message says what was decided. The other cases came with those
//! changes.

mod suite;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use suite::{Cache, Product, Synthesizer};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root resolves")
}

fn scratch() -> tempfile::TempDir {
    let target = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::create_dir_all(target).expect("the target's temporary directory is there");
    tempfile::tempdir_in(target).expect("a directory under the target's temporary directory")
}

fn taskfile() -> String {
    fs::read_to_string(repository_root().join("Taskfile.yml")).expect("Taskfile.yml is read")
}

/// Runs the command line of the repository's `task test-run`, its bound cut to one second and
/// its kill one second later, over parts whose first is `part` under the deferred slowest list.
fn test_run_over(part: &str) -> Output {
    let taskfile = taskfile();
    let line = taskfile
        .lines()
        .map(str::trim_start)
        .find(|line| line.starts_with("- timeout ") && line.contains("task test-run-parts"))
        .expect("the command line of test-run");
    assert!(
        line.contains("--kill-after=10s 120s "),
        "the bound of test-run: {line}"
    );
    let line = line.replace("--kill-after=10s 120s ", "--kill-after=1s 1s ");
    let directory = scratch();
    fs::write(
        directory.path().join("Taskfile.yml"),
        format!(
            "version: '3'\n\ntasks:\n  test-run:\n    cmds:\n      {line}\n\n  test-run-parts:\n    cmds:\n      - task: test-workspace\n      - echo \"a later part ran\"\n\n  test-workspace:\n    cmds:\n      - defer: {{ task: test-slowest }}\n      - {part}\n\n  test-slowest:\n    cmds:\n      - echo \"the ten slowest tests, in seconds:\"\n"
        ),
    )
    .expect("the Taskfile is written");
    Command::new("task")
        .arg("test-run")
        .current_dir(directory.path())
        .output()
        .expect("`task` runs; it must be on PATH")
}

fn text(output: &Output) -> String {
    format!(
        "exit status: {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

/// `website/docs/guides/run-the-checks.md` said: "The test step itself is ended after 120 seconds
/// and fails then, with exit status 124 and the line `task test-run was ended at its bound of
/// 120 s`." The case asserted 124, and `task` exits 201: 124 is the status of `timeout` inside
/// the command line. Decided: the guide was wrong. The step fails, `task` exits 201 as it does
/// for a failing test, and the line tells the bound from a failing test.
#[test]
fn adversary_w1_the_test_run_task_exits_201_and_names_its_bound_when_the_bound_ends_it() {
    let output = test_run_over("sleep 30");
    let said = text(&output);
    assert!(
        said.contains("task test-run was ended at its bound of 120 s\n"),
        "the line of the bound is printed:\n{said}"
    );
    assert!(
        said.contains("the ten slowest tests, in seconds:\n"),
        "the slowest list is printed when the bound ends the step:\n{said}"
    );
    assert!(
        !said.contains("a later part ran\n"),
        "no part runs after the bound:\n{said}"
    );
    assert_eq!(
        output.status.code(),
        Some(201),
        "decided: `task test-run` exits 201 when its bound ends it, not the 124 of `timeout`, \
         and the guide says 201:\n{said}"
    );
    let guide = fs::read_to_string(repository_root().join("website/docs/guides/run-the-checks.md"))
        .expect("the guide is read");
    assert!(
        guide.contains("`task test-run` exits 201") && !guide.contains("exit status 124"),
        "the guide names the status `task` exits with when the bound ends the step"
    );
}

/// A part that fails inside the bound: the step fails with the same status and without the line
/// of the bound, which is what the line is for.
#[test]
fn a_failing_part_inside_the_bound_exits_201_without_the_line_of_the_bound() {
    let output = test_run_over("sh -c 'exit 3'");
    let said = text(&output);
    assert_eq!(output.status.code(), Some(201), "{said}");
    assert!(
        !said.contains("task test-run was ended at its bound of 120 s\n"),
        "a failing part is not told as the bound:\n{said}"
    );
    assert!(
        said.contains("the ten slowest tests, in seconds:\n"),
        "the slowest list is printed when a part failed:\n{said}"
    );
}

/// The same sentence of the guide, for a part that is still there when the kill follows the
/// first signal: the step is ended then too, and has to say that its bound ended it.
#[test]
fn adversary_w1_the_test_run_task_names_its_bound_when_a_part_outlives_the_first_signal() {
    let output = test_run_over("sh -c 'trap \"\" TERM; exec sleep 30'");
    let said = text(&output);
    assert!(
        said.contains("task test-run was ended at its bound of 120 s\n"),
        "the line of the bound is printed when the kill ends the step:\n{said}"
    );
    assert_eq!(output.status.code(), Some(201), "the step fails:\n{said}");
}

/// The story's acceptance: "The conformance suite is synthesised at most once per test run, not
/// once per test: a run of the testkit's tests calls synthesis once, or never when the committed
/// file's digest matches."
///
/// The case asserted one call and counted three: the authored suite and one suite for each of
/// two witness scenarios. Decided: once for each distinct suite. The two witness scenarios are
/// now synthesized together, so the suites are two and the case asserts two.
///
/// Reads every suite the tests of this crate read, as they read it, and counts the lines the
/// cache wrote to `synthesized.log` beside each: one line is one call of synthesis.
#[test]
fn adversary_w1_a_run_of_the_testkits_tests_calls_synthesis_once_for_each_of_two_suites() {
    let mut calls = Vec::new();
    let mut lines = 0;
    let mut directories = Vec::new();
    for product in Product::ALL {
        let name = match product {
            Product::Authored => {
                "the suite of ess/ with the authored scenarios, the bytes conform.rs hands to \
                 `ess verify conform report`"
            }
            Product::Witnessed => {
                "the same with the witness scenarios of suite/witness, read by \
                 adversary12_duplex_close_witness.rs and adversary_w23_descriptor_view.rs"
            }
        };
        let read = suite::synthesized(product);
        let directory = suite::kept_in(product);
        assert_eq!(
            fs::read_to_string(directory.join("suite.json")).expect("the suite is kept"),
            read,
            "{name} is kept in {}",
            directory.display()
        );
        let log = fs::read_to_string(directory.join("synthesized.log")).unwrap_or_default();
        lines += log.lines().count();
        calls.push(format!("{name}: {} line(s)", log.lines().count()));
        if !directories.contains(&directory) {
            directories.push(directory);
        }
    }
    assert_eq!(
        (directories.len(), lines),
        (2, 2),
        "decided: synthesis is called once for each distinct suite, and the suites `ess` \
         synthesizes for the tests of this crate are these two. A third is one more call in \
         every run that starts without a kept suite: {calls:#?}"
    );
}

/// The two witness scenarios stand in one suite, and neither test that reads it runs the other's
/// witness: `adversary_w23_descriptor_view.rs` runs the scenarios that read the Adapters view,
/// which the close witness does not, and `adversary12_duplex_close_witness.rs` runs its three
/// scenarios by id. Every scenario of the authored suite is the same scenario in it.
#[test]
fn the_two_witnesses_are_in_one_suite_and_out_of_each_others_cut() {
    let close = "inference.duplex/authored/adversary-close-after-out-of-order-delivery";
    let descriptor = "inference.model/authored/adversary-w23-descriptor-whole";
    let adapters = "inference.model.Adapters";
    let witnessed: serde_json::Value =
        serde_json::from_str(&suite::synthesized(Product::Witnessed)).expect("JSON");
    let authored: serde_json::Value =
        serde_json::from_str(&suite::synthesized(Product::Authored)).expect("JSON");
    let witnessed = witnessed["scenarios"].as_object().expect("a map");
    let authored = authored["scenarios"].as_object().expect("a map");
    assert!(witnessed.contains_key(close) && witnessed.contains_key(descriptor));
    assert!(!authored.contains_key(close) && !authored.contains_key(descriptor));
    assert_eq!(witnessed.len(), authored.len() + 2);
    for (id, scenario) in authored {
        assert!(
            witnessed.get(id) == Some(scenario),
            "`{id}` is the same scenario with the witnesses beside it"
        );
    }
    assert!(
        !witnessed[close].to_string().contains(adapters),
        "the close witness does not read the Adapters view"
    );
    assert!(witnessed[descriptor].to_string().contains(adapters));
}

/// A synthesizer whose suite is one line.
struct OneLine;

impl Synthesizer for OneLine {
    fn version(&self) -> String {
        "1".to_owned()
    }

    fn synthesize(&self, _root: &Path, _scenarios: &Path, out: &Path) {
        fs::write(out, "the whole suite\n").expect("the suite is written");
    }
}

/// `suite/mod.rs` said: "A `suite.json` that is there is whole, because a rename is atomic". A
/// rename orders nothing about the file's bytes: after a crash of the machine between the rename
/// and the write-back the name is there and the file is empty. The cache served that file as the
/// suite. Decided: the bytes are written to disk before the rename, and a `suite.json` without a
/// byte counts as not there.
#[test]
fn adversary_w1_an_empty_suite_file_is_not_served_as_the_suite() {
    let scratch = scratch();
    let root = scratch.path().join("root");
    fs::create_dir_all(root.join("ess/scenarios")).expect("ess/scenarios is created");
    fs::write(root.join("ess/system.yaml"), "the system").expect("system.yaml is written");
    let directory = scratch.path().join("cache");
    let cache = Cache {
        root: &root,
        directory: &directory,
        synthesizer: &OneLine,
    };
    assert_eq!(cache.suite(&[]), "the whole suite\n");
    let keys: Vec<PathBuf> = fs::read_dir(&directory)
        .expect("the cache is readable")
        .map(|entry| entry.expect("an entry").path())
        .collect();
    assert_eq!(keys.len(), 1, "one key: {keys:?}");
    fs::write(keys[0].join("suite.json"), "").expect("the suite's file is emptied");
    assert_eq!(
        cache.suite(&[]),
        "the whole suite\n",
        "an empty suite.json is synthesized again, not served"
    );
}

/// Every Rust file under `directory`, at any depth.
fn rust_files(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("a directory of the tests is readable") {
        let path = entry.expect("an entry").path();
        if path.is_dir() {
            rust_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

/// The argument that has `ess` synthesize, as a test file would write it.
const ARGUMENT: &str = concat!("\"synth", "esize\"");
/// The module's synthesizer, as a test file would name it.
const SYNTHESIZER: &str = concat!("suite::", "Ess");

/// The files under `tests`, but `suite/mod.rs`, that reach `ess` for a suite of their own: by
/// passing the argument themselves or by naming the module's synthesizer. Sorted.
fn outside_the_shared_cache(tests: &Path) -> Vec<String> {
    let mut files = Vec::new();
    rust_files(tests, &mut files);
    let mut own: Vec<String> = files
        .iter()
        .filter(|path| **path != tests.join("suite/mod.rs"))
        .filter(|path| {
            let text = fs::read_to_string(path).expect("a test file is readable");
            text.contains(ARGUMENT) || text.contains(SYNTHESIZER)
        })
        .map(|path| {
            path.strip_prefix(tests)
                .unwrap_or(path)
                .display()
                .to_string()
        })
        .collect();
    own.sort();
    own
}

/// The planted defect of the case below was a test here that had `ess` synthesize the suite
/// into a directory of its own through the module's `Cache` and its `Ess`, which were public,
/// without the argument the scan of `conformance_suite.rs` looks for in its text. It passed
/// every guard.
///
/// Decided: `Ess` is private to `suite/mod.rs`, and the one function that reaches it reads the
/// shared cache and takes a [`Product`], not a scenario of the caller's. That test does not
/// compile any more, so it is planted as text: every way left to a test file to synthesize a
/// suite of its own writes the argument, or names the synthesizer and does not compile, and the
/// scan names each.
#[test]
fn adversary_w1_planted_every_way_left_to_synthesize_outside_the_shared_cache_is_named() {
    let module =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/suite/mod.rs"))
            .expect("suite/mod.rs is read");
    assert!(
        module.contains("\nstruct Ess;\n") && !module.contains("pub struct Ess"),
        "decided: the module's synthesizer is private to it"
    );
    assert_eq!(
        module.matches("&Ess").count(),
        1,
        "one place of the module hands `ess` to a cache"
    );
    assert!(
        module.contains("        directory: &cache_directory(),\n        synthesizer: &Ess,\n"),
        "and that cache is the shared one"
    );
    assert!(
        module.contains("pub fn synthesized(product: Product) -> String {"),
        "the reader of the shared cache takes a product, not a scenario of the caller's"
    );

    let scratch = scratch();
    let tests = scratch.path();
    fs::create_dir_all(tests.join("suite")).expect("suite/ is created");
    fs::create_dir_all(tests.join("deeper")).expect("deeper/ is created");
    fs::write(
        tests.join("suite/mod.rs"),
        format!(".args([\"verify\", \"conform\", {ARGUMENT}, \"--path\", \"ess\"])"),
    )
    .expect("the module is written");
    fs::write(
        tests.join("reads_the_shared_suite.rs"),
        "mod suite;\nfn suite() -> String { suite::synthesized(suite::Product::Authored) }\n",
    )
    .expect("a reader of the shared cache is written");
    fs::write(
        tests.join("runs_ess_itself.rs"),
        format!("Command::new(\"ess\").args([\"verify\", \"conform\", {ARGUMENT}])"),
    )
    .expect("a test that runs ess is written");
    fs::write(
        tests.join("deeper/has_a_synthesizer_of_its_own.rs"),
        format!(
            "impl suite::Synthesizer for Mine {{ fn synthesize(&self, root: &Path, scenarios: \
             &Path, out: &Path) {{ Command::new(\"ess\").args([\"verify\", \"conform\", \
             {ARGUMENT}]); }} }}\nfn mine() {{ Cache {{ synthesizer: &Mine }}.suite(&[]); }}"
        ),
    )
    .expect("a test with a synthesizer of its own is written");
    fs::write(
        tests.join("names_the_synthesizer.rs"),
        format!("Cache {{ root, directory: scratch.path(), synthesizer: &{SYNTHESIZER} }}"),
    )
    .expect("the planted test of the first form is written");
    assert_eq!(
        outside_the_shared_cache(tests),
        [
            "deeper/has_a_synthesizer_of_its_own.rs",
            "names_the_synthesizer.rs",
            "runs_ess_itself.rs",
        ],
        "the scan names every planted file and neither the module nor a reader of the shared \
         cache"
    );
}

/// `no_test_of_this_crate_synthesizes_outside_the_shared_cache` of `conformance_suite.rs`,
/// rewritten to see a file that reaches `ess` by naming the module's synthesizer too, not only
/// by passing the argument itself.
#[test]
fn adversary_w1_no_test_names_the_ess_synthesizer_but_the_shared_cache() {
    let own = outside_the_shared_cache(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests"));
    assert!(
        own.is_empty(),
        "these files run synthesis outside the shared cache: {own:#?}"
    );
}

/// The lines of the task `name` in the repository's `Taskfile.yml`.
fn task_of(taskfile: &str, name: &str) -> String {
    let heading = format!("  {name}:");
    let mut lines = taskfile.lines().skip_while(|line| *line != heading);
    let mut task = format!("{}\n", lines.next().expect("the task is in Taskfile.yml"));
    for line in lines.take_while(|line| line.starts_with("    ")) {
        task.push_str(line);
        task.push('\n');
    }
    task
}

fn testcase(name: &str, seconds: &str) -> String {
    format!(
        "        <testcase name=\"{name}\" classname=\"a-crate::a_binary\" \
         timestamp=\"2026-01-01T00:00:00.000+00:00\" time=\"{seconds}\">\n        </testcase>\n"
    )
}

/// Runs the repository's tasks `test-workspace` and `test-slowest` in `directory`, which holds
/// the report of an earlier run, with `part` in the place of cargo-nextest.
fn test_workspace_over(directory: &Path, part: &str) -> Output {
    let taskfile = taskfile();
    let run = "      - cargo nextest run --workspace --all-features\n";
    let workspace = task_of(&taskfile, "test-workspace");
    assert!(workspace.contains(run), "the run of the tests: {workspace}");
    fs::write(
        directory.join("Taskfile.yml"),
        format!(
            "version: '3'\n\ntasks:\n{}\n{}",
            workspace.replace(run, &format!("      - {part}\n")),
            task_of(&taskfile, "test-slowest"),
        ),
    )
    .expect("the Taskfile is written");
    let report = directory.join("target/nextest/default");
    fs::create_dir_all(&report).expect("the report's directory is created");
    fs::write(
        report.join("junit.xml"),
        testcase("a_test_of_an_earlier_run", "59.000"),
    )
    .expect("the report of an earlier run is written");
    Command::new("task")
        .arg("test-workspace")
        .current_dir(directory)
        .output()
        .expect("`task` runs; it must be on PATH")
}

/// When the bound ended the step before cargo-nextest started its run, the list printed the ten
/// slowest tests of the run before, out of the report that run left. Decided: the report is
/// removed before the run, and the list says `no test ran` when there is none.
#[test]
fn the_slowest_list_of_a_step_that_ran_no_test_says_so() {
    let scratch = scratch();
    let output = test_workspace_over(scratch.path(), "sh -c 'exit 3'");
    let said = text(&output);
    assert!(
        said.contains("the ten slowest tests, in seconds:\nno test ran\n"),
        "the list says that no test ran:\n{said}"
    );
    assert!(
        !said.contains("a_test_of_an_earlier_run"),
        "no test of an earlier run is listed:\n{said}"
    );
    assert_eq!(output.status.code(), Some(201), "the step fails:\n{said}");
}

/// The list of a run that wrote its report: its ten slowest tests, slowest first, and no test of
/// the run before.
#[test]
fn the_slowest_list_is_of_the_run_that_just_ended() {
    let scratch = scratch();
    let mut report = String::new();
    for test in 1..=12 {
        report.push_str(&testcase(&format!("test_{test}"), &format!("{test}.500")));
    }
    fs::write(scratch.path().join("fresh.xml"), report).expect("the new report is written");
    let output = test_workspace_over(
        scratch.path(),
        "cp fresh.xml target/nextest/default/junit.xml",
    );
    let said = text(&output);
    let mut listed = String::from("the ten slowest tests, in seconds:\n");
    for test in (3..=12).rev() {
        listed.push_str(&format!("{test}.500 a-crate::a_binary test_{test}\n"));
    }
    assert!(
        said.contains(&format!("{listed}\nstderr:")),
        "the ten slowest of the twelve, slowest first:\n{said}"
    );
    assert_eq!(output.status.code(), Some(0), "the step passes:\n{said}");
}
