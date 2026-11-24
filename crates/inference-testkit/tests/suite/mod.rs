//! The synthesized conformance suite of `ess/`, made once and read by every test that runs it.
//!
//! `ess verify conform synthesize` takes seconds and writes megabytes, and cargo-nextest runs
//! each test in a process of its own, so nothing a process holds in memory is shared between
//! the tests: the suite is a file. It stands at
//! `CARGO_TARGET_TMPDIR/conformance-suite/<key>/suite.json`, where `<key>` is the SHA-256 of
//! what synthesis reads: every file under `ess/`, what `ess --version` prints, and the scenario
//! a test adds to the authored ones. A changed source is another key, so a suite that is there
//! is never stale, and one whose key no test asks for any more is only disk under `target/`.
//!
//! The first reader of a key takes the lock of the key's directory, synthesizes into
//! `suite.json.part` and renames the result into place. A reader that comes meanwhile waits on
//! the lock and then reads the file; a reader that comes later reads it without the lock. A
//! `suite.json` that is there is whole, because a rename is atomic, and a part a killed reader
//! left is written over by the next one, which holds the lock. Each synthesis appends one line
//! to `synthesized.log` beside the suite it wrote, so a log with two lines is a suite that was
//! synthesized twice.
//!
//! The tests of this module are in `tests/conformance_suite.rs`.

use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

/// A scenario a test adds to the authored ones of `ess/scenarios`: its file name and its text.
#[derive(Clone, Copy)]
pub struct Added<'a> {
    pub file: &'a str,
    pub yaml: &'a str,
}

/// What synthesizes a suite: `ess`, or a stand-in in the tests of this module.
pub trait Synthesizer {
    /// What names the synthesizer's version; a suite is kept for the version that wrote it.
    fn version(&self) -> String;

    /// Writes to `out` the suite of the specification under `root/ess` with the authored
    /// scenarios of the directory `scenarios`.
    fn synthesize(&self, root: &Path, scenarios: &Path, out: &Path);
}

/// The `ess` on `PATH`.
pub struct Ess;

impl Synthesizer for Ess {
    fn version(&self) -> String {
        let output = Command::new("ess")
            .arg("--version")
            .output()
            .expect("`ess` runs; it must be on PATH");
        assert!(output.status.success(), "`ess --version`: {output:?}");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn synthesize(&self, root: &Path, scenarios: &Path, out: &Path) {
        let output = Command::new("ess")
            .args(["verify", "conform", "synthesize", "--path", "ess"])
            .arg("--scenarios")
            .arg(scenarios)
            .arg("--out")
            .arg(out)
            .current_dir(root)
            .output()
            .expect("`ess` runs; it must be on PATH");
        assert!(
            output.status.success(),
            "`ess verify conform synthesize` exited {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}

/// The suites of one specification, kept under one directory.
pub struct Cache<'a> {
    /// The directory whose `ess/` is the specification.
    pub root: &'a Path,
    /// The directory the suites are kept under, one directory per key.
    pub directory: &'a Path,
    pub synthesizer: &'a dyn Synthesizer,
}

impl Cache<'_> {
    /// The suite of the specification with its authored scenarios and `added`, as the JSON text
    /// synthesis wrote: read from the cache, and synthesized first when no reader has yet.
    pub fn suite(&self, added: Option<Added<'_>>) -> String {
        let directory = self.directory.join(self.key(added));
        let suite = directory.join("suite.json");
        if let Ok(text) = fs::read_to_string(&suite) {
            return text;
        }
        fs::create_dir_all(&directory).expect("the suite's directory is created");
        let lock = OpenOptions::new()
            .create(true)
            .append(true)
            .open(directory.join("lock"))
            .expect("the lock file of the suite's directory opens");
        lock.lock()
            .expect("the lock of the suite's directory is taken");
        if let Ok(text) = fs::read_to_string(&suite) {
            return text;
        }
        let scenarios = self.scenarios(&directory, added);
        let part = directory.join("suite.json.part");
        self.synthesizer.synthesize(self.root, &scenarios, &part);
        fs::rename(&part, &suite).expect("the synthesized suite is moved into place");
        record(&directory);
        fs::read_to_string(&suite).expect("suite.json is readable")
    }

    /// The SHA-256, in hex, of everything the suite is synthesized from.
    fn key(&self, added: Option<Added<'_>>) -> String {
        let specification = self.root.join("ess");
        let mut files = Vec::new();
        collect(&specification, &mut files);
        files.sort();
        let mut hash = Sha256::new();
        let mut part = |bytes: &[u8]| {
            hash.update((bytes.len() as u64).to_le_bytes());
            hash.update(bytes);
        };
        part(b"inference-testkit conformance suite 1");
        part(self.synthesizer.version().as_bytes());
        for file in &files {
            let name = file
                .strip_prefix(&specification)
                .expect("a file under ess/");
            part(name.to_string_lossy().as_bytes());
            part(&fs::read(file).expect("a file under ess/ is readable"));
        }
        match added {
            Some(added) => {
                part(b"added");
                part(added.file.as_bytes());
                part(added.yaml.as_bytes());
            }
            None => part(b"authored only"),
        }
        let mut key = String::new();
        for byte in hash.finalize() {
            write!(key, "{byte:02x}").expect("a string takes the digits");
        }
        key
    }

    /// The directory of the authored scenarios: `ess/scenarios`, or a copy of it under
    /// `directory` with `added` beside them.
    fn scenarios(&self, directory: &Path, added: Option<Added<'_>>) -> PathBuf {
        let authored = self.root.join("ess/scenarios");
        let Some(added) = added else {
            return authored;
        };
        let scenarios = directory.join("scenarios");
        fs::create_dir_all(&scenarios).expect("the scenarios directory is created");
        for entry in fs::read_dir(&authored).expect("ess/scenarios is readable") {
            let path = entry.expect("an entry of ess/scenarios").path();
            if path
                .extension()
                .is_some_and(|extension| extension == "yaml")
            {
                fs::copy(
                    &path,
                    scenarios.join(path.file_name().expect("a file name")),
                )
                .expect("the scenario is copied");
            }
        }
        fs::write(scenarios.join(added.file), added.yaml).expect("the added scenario is written");
        scenarios
    }
}

/// Every file under `directory`, at any depth.
fn collect(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("a directory under ess/ is readable") {
        let path = entry.expect("an entry under ess/").path();
        if path.is_dir() {
            collect(&path, files);
        } else {
            files.push(path);
        }
    }
}

/// Appends the line of one synthesis to the log beside the suite it wrote.
fn record(directory: &Path) {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs();
    let mut log: File = OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join("synthesized.log"))
        .expect("the log of the suite's directory opens");
    writeln!(log, "{seconds} process {}", std::process::id()).expect("the log takes a line");
}

/// The suite `text` with only the scenarios whose id `keep` takes, as JSON text: for a test that
/// reads the statuses of a few scenarios, which then admits and runs those and not the whole
/// suite. Admitting the whole suite takes seconds, a run of it runs every scenario, and neither
/// tells such a test anything: a scenario starts from empty stores and its status does not
/// depend on another's.
///
/// The suite writes its event catalog, `events`, once for the scenarios that refer to it by
/// `absent_events` or `unpublished_events`, and admission refuses a catalog no scenario refers
/// to: the cut leaves it out when no scenario it keeps does.
///
/// # Panics
/// When `text` is not a suite, or `keep` takes no scenario of it.
#[allow(
    dead_code,
    reason = "a test file that runs the whole suite does not cut it"
)]
pub fn cut(text: &str, keep: impl Fn(&str) -> bool) -> String {
    let mut suite: serde_json::Value = serde_json::from_str(text).expect("the suite is JSON");
    let scenarios = suite["scenarios"]
        .as_object_mut()
        .expect("the suite's scenarios are a map");
    scenarios.retain(|id, _| keep(id));
    assert!(!scenarios.is_empty(), "the cut keeps a scenario");
    let refers = scenarios.values().any(|scenario| {
        scenario.get("absent_events").is_some() || scenario.get("unpublished_events").is_some()
    });
    if !refers {
        suite
            .as_object_mut()
            .expect("the suite is a map")
            .remove("events");
    }
    suite.to_string()
}

/// Where the suites of this repository are kept: under the target directory.
pub fn cache_directory() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("conformance-suite")
}

/// The suite `ess` synthesizes from this repository's `ess/`, with `added` beside the authored
/// scenarios when a test adds one.
pub fn synthesized(added: Option<Added<'_>>) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root resolves");
    Cache {
        root: &root,
        directory: &cache_directory(),
        synthesizer: &Ess,
    }
    .suite(added)
}
