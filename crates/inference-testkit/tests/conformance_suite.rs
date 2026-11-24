//! The tests of `suite/mod.rs`: the synthesized conformance suite is made once and read by every
//! test, not made by each.
//!
//! cargo-nextest runs each test in a process of its own, so the first test starts eight
//! processes of this binary that read one empty cache at the same time and counts the syntheses.
//! A stand-in takes the place of `ess` in every test but the last: it counts its calls in a
//! file, takes long enough for the readers to overlap, and writes its suite in two halves, so a
//! reader that saw a suite before it was whole would read half of one.

mod suite;

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use suite::{Added, Cache, Synthesizer};

/// Set for a process started by `eight_processes_reading_one_empty_cache_synthesize_once`: the
/// directory its specification, its cache and the count of syntheses are under.
const READER_DIRECTORY: &str = "INFERENCE_TESTKIT_SUITE_READER_DIRECTORY";
const READERS: usize = 8;

/// A stand-in for `ess`. Its suite is every file of the scenarios directory, by name, after the
/// version and `ess/system.yaml`.
struct StandIn<'a> {
    calls: &'a Path,
    version: &'a str,
    takes: Duration,
}

impl Synthesizer for StandIn<'_> {
    fn version(&self) -> String {
        self.version.to_owned()
    }

    fn synthesize(&self, root: &Path, scenarios: &Path, out: &Path) {
        let mut calls = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.calls)
            .expect("the count of syntheses opens");
        writeln!(calls, "process {}", std::process::id()).expect("the count takes a line");
        let text = stand_in_suite(self.version, root, scenarios);
        let (first, second) = text.split_at(text.len() / 2);
        let mut suite = fs::File::create(out).expect("the suite's file is created");
        suite.write_all(first.as_bytes()).expect("the first half");
        suite.sync_all().expect("the first half is on disk");
        thread::sleep(self.takes);
        suite.write_all(second.as_bytes()).expect("the second half");
    }
}

fn stand_in_suite(version: &str, root: &Path, scenarios: &Path) -> String {
    let mut names: Vec<PathBuf> = fs::read_dir(scenarios)
        .expect("the scenarios directory is readable")
        .map(|entry| entry.expect("an entry").path())
        .collect();
    names.sort();
    let mut text = format!(
        "version {version}\nsystem {}\n",
        fs::read_to_string(root.join("ess/system.yaml")).expect("system.yaml is readable")
    );
    for path in names {
        text.push_str(&format!(
            "scenario {} {}\n",
            path.file_name().expect("a file name").to_string_lossy(),
            fs::read_to_string(&path).expect("a scenario is readable")
        ));
    }
    text
}

/// A specification of two files and one authored scenario under `directory/root`.
fn specification(directory: &Path) {
    let ess = directory.join("root/ess");
    fs::create_dir_all(ess.join("domains")).expect("ess/domains is created");
    fs::create_dir_all(ess.join("scenarios")).expect("ess/scenarios is created");
    fs::write(ess.join("system.yaml"), "the system").expect("system.yaml is written");
    fs::write(ess.join("domains/one.yaml"), "a domain").expect("a domain is written");
    fs::write(ess.join("scenarios/authored.yaml"), "authored").expect("a scenario is written");
}

fn scratch() -> tempfile::TempDir {
    let target = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::create_dir_all(target).expect("the target's temporary directory is there");
    tempfile::tempdir_in(target).expect("a directory under the target's temporary directory")
}

fn calls(directory: &Path) -> usize {
    fs::read_to_string(directory.join("calls")).map_or(0, |text| text.lines().count())
}

fn read(directory: &Path, version: &str, takes: Duration, added: Option<Added<'_>>) -> String {
    Cache {
        root: &directory.join("root"),
        directory: &directory.join("cache"),
        synthesizer: &StandIn {
            calls: &directory.join("calls"),
            version,
            takes,
        },
    }
    .suite(added)
}

/// One of the eight readers; a test that does nothing when the binary is run as a whole.
#[test]
fn reader_process() {
    let Some(directory) = std::env::var_os(READER_DIRECTORY) else {
        return;
    };
    let directory = PathBuf::from(directory);
    let text = read(&directory, "1", Duration::from_millis(400), None);
    assert_eq!(
        text,
        stand_in_suite(
            "1",
            &directory.join("root"),
            &directory.join("root/ess/scenarios")
        ),
        "the reader has the whole suite"
    );
}

#[test]
fn eight_processes_reading_one_empty_cache_synthesize_once() {
    let scratch = scratch();
    let directory = scratch.path();
    specification(directory);
    let readers: Vec<_> = (0..READERS)
        .map(|_| {
            Command::new(std::env::current_exe().expect("this binary has a path"))
                .args(["--exact", "reader_process", "--nocapture"])
                .env(READER_DIRECTORY, directory)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("a reader starts")
        })
        .collect();
    for reader in readers {
        let output = reader.wait_with_output().expect("a reader ends");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "a reader failed: {}\nstdout:\n{stdout}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr),
        );
    }
    assert_eq!(
        calls(directory),
        1,
        "{READERS} processes read one suite, so it is synthesized once"
    );

    let keys: Vec<PathBuf> = fs::read_dir(directory.join("cache"))
        .expect("the cache is readable")
        .map(|entry| entry.expect("an entry").path())
        .collect();
    assert_eq!(keys.len(), 1, "one specification is one key: {keys:?}");
    let mut left: Vec<String> = fs::read_dir(&keys[0])
        .expect("the key's directory is readable")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    left.sort();
    assert_eq!(left, ["lock", "suite.json", "synthesized.log"]);
    let log = fs::read_to_string(keys[0].join("synthesized.log")).expect("the log is readable");
    assert_eq!(log.lines().count(), 1, "the log has one line: {log}");
}

#[test]
fn a_suite_is_synthesized_again_only_for_another_source_version_or_added_scenario() {
    let scratch = scratch();
    let directory = scratch.path();
    specification(directory);
    let suite_of = |version: &str, added| read(directory, version, Duration::ZERO, added);
    let witness = Added {
        file: "witness.yaml",
        yaml: "a witness",
    };

    let plain = suite_of("1", None);
    assert_eq!(calls(directory), 1);
    assert_eq!(suite_of("1", None), plain);
    assert_eq!(calls(directory), 1, "a second read synthesizes nothing");

    let with_witness = suite_of("1", Some(witness));
    assert!(
        with_witness.contains("scenario authored.yaml authored\n")
            && with_witness.contains("scenario witness.yaml a witness\n"),
        "the added scenario stands beside the authored one: {with_witness}"
    );
    assert_eq!(calls(directory), 2, "an added scenario is another suite");
    assert_eq!(suite_of("1", Some(witness)), with_witness);
    assert_eq!(suite_of("1", None), plain);
    assert_eq!(calls(directory), 2, "each suite is read again as it was");

    let other_text = Added {
        yaml: "another witness",
        ..witness
    };
    assert!(suite_of("1", Some(other_text)).contains("another witness"));
    assert_eq!(calls(directory), 3, "another text under the same name");

    assert!(suite_of("2", None).starts_with("version 2\n"));
    assert_eq!(calls(directory), 4, "another version of the synthesizer");

    let system = directory.join("root/ess/system.yaml");
    fs::write(&system, "the system, changed").expect("system.yaml is rewritten");
    assert!(suite_of("1", None).contains("the system, changed"));
    assert_eq!(calls(directory), 5, "a changed source");

    fs::write(
        directory.join("root/ess/domains/two.yaml"),
        "a second domain",
    )
    .expect("a domain is added");
    suite_of("1", None);
    assert_eq!(calls(directory), 6, "an added source");

    fs::remove_file(directory.join("root/ess/domains/two.yaml")).expect("the domain is removed");
    fs::write(&system, "the system").expect("system.yaml is restored");
    assert_eq!(suite_of("1", None), plain);
    assert_eq!(
        calls(directory),
        6,
        "the first sources again are the first suite"
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

/// A test that passed `ess` the argument itself would synthesize once per test again.
#[test]
fn no_test_of_this_crate_synthesizes_outside_the_shared_cache() {
    let argument = concat!("\"synth", "esize\"");
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut files = Vec::new();
    rust_files(&tests, &mut files);
    assert!(files.len() > 60, "the tests of this crate are read");
    let own: Vec<String> = files
        .iter()
        .filter(|path| **path != tests.join("suite/mod.rs"))
        .filter(|path| {
            fs::read_to_string(path)
                .expect("a test file is readable")
                .contains(argument)
        })
        .map(|path| {
            path.strip_prefix(&tests)
                .unwrap_or(path)
                .display()
                .to_string()
        })
        .collect();
    assert!(
        own.is_empty(),
        "these files run synthesis themselves; `suite::synthesized` reads the shared suite: {own:#?}"
    );
}

/// The suite of this repository, through `ess`: after a read, no suite the cache holds was
/// synthesized more than once.
#[test]
fn every_suite_of_this_repository_was_synthesized_at_most_once() {
    let text = suite::synthesized(None);
    assert!(
        text.contains("\"scenarios\""),
        "the suite carries its scenarios"
    );
    let mut suites = 0;
    for entry in fs::read_dir(suite::cache_directory()).expect("the cache is readable") {
        let key = entry.expect("an entry").path();
        let log = fs::read_to_string(key.join("synthesized.log")).unwrap_or_default();
        println!(
            "{}: {} line(s) in synthesized.log",
            key.display(),
            log.lines().count()
        );
        assert!(
            log.lines().count() <= 1,
            "{} was synthesized {} times:\n{log}",
            key.display(),
            log.lines().count()
        );
        suites += usize::from(key.join("suite.json").is_file());
    }
    assert!(suites >= 1, "the cache holds the suite just read");
}
