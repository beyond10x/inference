//! `adopt-records`: copies the records of a check run on `main` into `quality/records/`, byte for
//! byte, after it has checked every one of them. It downloads nothing.
//!
//! ```sh
//! gh run view <id> -R beyond10x/inference --json databaseId,headBranch,headSha,event,workflowName > run.json
//! gh run download <id> -R beyond10x/inference -n quality-records-<commit> --dir <dir>
//! inference-docs adopt-records --run <id> --run-json run.json --from <dir> --root .
//! ```
//!
//! `--from` is the directory the artifact `quality-records-<commit>` was downloaded to, as
//! `task quality-records` wrote it: `<plan>/<assessment>.evidence.json` for each line of
//! `inference-docs junit-assessments`. `--run-json` is what `gh run view` printed for the run.
//!
//! It refuses, with one line on stderr naming the file and the reason for each thing it refuses,
//! and writes nothing:
//!
//! - a run that is not a push to `main` of the workflow `check`, or whose id is not `--run`;
//! - a record that names another run than `--run`, or another commit than the run's;
//! - a missing record of an assessment `junit-assessments` lists, and any other file or directory;
//! - a record or a `run.json` that is not JSON, or in which one object holds a key twice, which
//!   readers of the copied bytes could read in two ways;
//! - a record of another project than the plan files', of another assessor than its assessment's,
//!   of another class than `ci`, or with another target than the one `assess` writes for its plan
//!   and the run: its kind, class, selection, revision or identifiers, and no other field;
//! - a record whose source `assess` refuses, or names no forge run;
//! - a record the record rules of the quality documents refuse: [`record_rules`], the rules
//!   `record_problems` of `tests/quality/mod.rs` holds.
//!
//! Exit 0: every record is written to `quality/records/<assessment>.evidence.json` under the root.
//! Exit 1: a refusal, nothing written. Exit 2: a wrong call. Exit 3: a plan file, a declaration or
//! the file of the tree a document target names cannot be read, or a record cannot be written;
//! the records written before it are put back, so the directory holds all adopted records or
//! none. A temporary file is created new, after whatever stood at its path is removed, so a link
//! there is never written through.

use std::{
    collections::BTreeSet,
    fmt, fs,
    io::{ErrorKind, Write as _},
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::Args;
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use serde_norway::Value as Yaml;

use super::{
    CLASSES, Failure, Outcome, Plan, PlanFiles, junit_assessments, read_text, sha256, source,
    strings,
};

/// The runs of the repository on the forge, by id.
const RUNS: &str = "https://github.com/beyond10x/inference/actions/runs/";
/// Where the committed records stand, from the root.
const RECORDS: &str = "quality/records";
/// What the run must be: `(field of gh run view, value)`.
const RUN_OF_MAIN: [(&str, &str); 3] = [
    ("event", "push"),
    ("headBranch", "main"),
    ("workflowName", "check"),
];
/// The format of a record.
const EVIDENCE_FORMAT: &str = "ratchet-evidence/1";
/// The counts of a coverage; a count the assessor does not report is 0.
const COVERAGE_COUNTS: [&str; 6] = [
    "executed",
    "skipped",
    "not_run",
    "unsupported",
    "passed",
    "failed",
];
/// What a temporary file of the write is named after: `.<assessment>.evidence.json` and this.
pub(crate) const TEMPORARY: &str = ".adopting";

/// The call of `adopt-records`.
#[derive(Args)]
pub(crate) struct Arguments {
    /// The id of the check run on `main` whose records are adopted.
    #[arg(long)]
    run: u64,
    /// The output of `gh run view <id> --json databaseId,headBranch,headSha,event,workflowName`
    /// for that run, saved by the caller.
    #[arg(long)]
    run_json: PathBuf,
    /// The directory the artifact `quality-records-<commit>` of the run was downloaded to.
    #[arg(long)]
    from: PathBuf,
    /// The root of the tree whose `quality/records/` receives the records.
    #[arg(long, default_value = ".")]
    root: PathBuf,
}

/// Runs the call: the records written, or the lines on stderr and the exit code of the failure.
pub(crate) fn run(arguments: &Arguments) -> ExitCode {
    let (code, lines) = match adopt(arguments) {
        Ok(written) => {
            for path in written {
                println!("{path}: adopted from the run {}", arguments.run);
            }
            return ExitCode::SUCCESS;
        }
        Err(Failure::Refused(lines)) => (1, lines),
        Err(Failure::Call(line)) => (2, vec![line]),
        Err(Failure::Broken(line)) => (3, vec![line]),
    };
    for line in lines {
        eprintln!("inference-docs adopt-records: {line}");
    }
    ExitCode::from(code)
}

/// One record the run should have: its plan, its assessment and the kind of its target.
struct Expected {
    plan: String,
    assessment: String,
    kind: String,
}

impl Expected {
    /// Its path in the artifact.
    fn path(&self) -> String {
        format!("{}/{}.evidence.json", self.plan, self.assessment)
    }
}

fn adopt(arguments: &Arguments) -> Outcome<Vec<String>> {
    let root = &arguments.root;
    let mut refused = Vec::new();
    let commit = run_commit(arguments, &mut refused);
    let expected: Vec<Expected> = junit_assessments(root)
        .map_err(Failure::Broken)?
        .iter()
        .filter_map(|line| {
            let mut words = line.split(' ');
            Some(Expected {
                plan: words.next()?.to_owned(),
                assessment: words.next()?.to_owned(),
                kind: words.next()?.to_owned(),
            })
        })
        .collect();
    let project = project(root)?;
    let files = PlanFiles::read(root, &project)?;
    let named = |path: &str| arguments.from.join(path).display().to_string();
    let found = artifact_files(&arguments.from)?;
    let paths: BTreeSet<String> = expected.iter().map(Expected::path).collect();
    let directories: BTreeSet<&str> = expected.iter().map(|one| one.plan.as_str()).collect();
    for (path, is_file) in &found {
        if !is_file && directories.contains(path.as_str()) {
            continue;
        }
        if !is_file || !paths.contains(path) {
            refused.push(format!(
                "{}: is no record of an assessment `inference-docs junit-assessments` lists, at <plan>/<assessment>.evidence.json",
                named(path)
            ));
        }
    }
    let mut adopted = Vec::new();
    for one in &expected {
        let path = one.path();
        let at = named(&path);
        if !found
            .iter()
            .any(|(file, is_file)| *is_file && *file == path)
        {
            refused.push(format!(
                "{at}: is missing; `inference-docs junit-assessments` lists {} of the plan {}",
                one.assessment, one.plan
            ));
            continue;
        }
        let bytes = match fs::read(arguments.from.join(&path)) {
            Ok(bytes) => bytes,
            Err(error) => {
                refused.push(format!("{at}: cannot be read: {error}"));
                continue;
            }
        };
        let plan = Plan::of(root, &files, &one.assessment)?;
        let record: Value = match serde_json::from_slice(&bytes) {
            Ok(record) => record,
            Err(error) => {
                refused.push(format!("{at}: is not valid JSON: {error}"));
                continue;
            }
        };
        if let Err(error) = serde_json::from_slice::<Unique>(&bytes) {
            refused.push(format!("{at}: {error}"));
            continue;
        }
        let before = refused.len();
        for problem in of_the_run(root, arguments.run, commit.as_deref(), one, &plan, &record)?
            .into_iter()
            .chain(record_rules(&project, &plan, &record))
        {
            refused.push(format!("{at}: {problem}"));
        }
        if refused.len() == before {
            adopted.push((one.assessment.clone(), bytes));
        }
    }
    if !refused.is_empty() {
        return Err(Failure::Refused(refused));
    }
    write_all(root, &adopted)
}

/// The commit of the run that `--run-json` describes, with a line in `refused` for each way the
/// run is not a push to `main` of the workflow `check` with the id `--run`.
fn run_commit(arguments: &Arguments, refused: &mut Vec<String>) -> Option<String> {
    let at = arguments.run_json.display();
    let run: Value = match fs::read_to_string(&arguments.run_json)
        .map_err(|error| format!("cannot be read: {error}"))
        .and_then(|text| {
            serde_json::from_str::<Unique>(&text)
                .and_then(|_| serde_json::from_str(&text))
                .map_err(|error| format!("is not JSON with each key once: {error}"))
        }) {
        Ok(run) => run,
        Err(why) => {
            refused.push(format!("{at}: {why}"));
            return None;
        }
    };
    match run.get("databaseId").and_then(Value::as_u64) {
        Some(id) if id == arguments.run => {}
        other => refused.push(format!(
            "{at}: describes the run {}, not the run {}",
            other.map_or_else(|| "with no databaseId".to_owned(), |id| id.to_string()),
            arguments.run
        )),
    }
    for (field, wanted) in RUN_OF_MAIN {
        let value = run.get(field).and_then(Value::as_str);
        if value != Some(wanted) {
            refused.push(format!(
                "{at}: {field} is {value:?}; a run whose records are adopted is a push to main of the workflow check, {field} {wanted:?}"
            ));
        }
    }
    let commit = run.get("headSha").and_then(Value::as_str);
    match commit {
        Some(commit) if full_commit(commit) => Some(commit.to_owned()),
        other => {
            refused.push(format!(
                "{at}: headSha is {other:?}, not a full commit id in lowercase hex"
            ));
            None
        }
    }
}

/// A JSON value read only to refuse an object that holds one key twice: `serde_json` keeps the
/// last value of such a key, and another reader of the same bytes may keep the first.
struct Unique;

impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(Unique)
    }
}

impl<'de> Visitor<'de> for Unique {
    type Value = Unique;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E>(self, _: bool) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_i64<E>(self, _: i64) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_u64<E>(self, _: u64) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_f64<E>(self, _: f64) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_str<E>(self, _: &str) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_unit<E>(self) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut items: A) -> Result<Unique, A::Error> {
        while items.next_element::<Unique>()?.is_some() {}
        Ok(Unique)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut entries: A) -> Result<Unique, A::Error> {
        let mut keys = BTreeSet::new();
        while let Some(key) = entries.next_key::<String>()? {
            entries.next_value::<Unique>()?;
            if !keys.insert(key.clone()) {
                return Err(de::Error::custom(format!(
                    "the key {key:?} stands twice in one object"
                )));
            }
        }
        Ok(Unique)
    }
}

/// Writes `bytes` to a file created new at `path`, after whatever stood there is removed: a link
/// at `path` is removed, not written through.
fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn full_commit(text: &str) -> bool {
    text.len() == 40
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The `project` of the first plan file `ratchet.toml` names; `PlanFiles::read` holds the others
/// to it.
pub(super) fn project(root: &Path) -> Outcome<String> {
    let config: Value = toml::from_str(&read_text(root, "ratchet.toml")?)
        .map_err(|error| Failure::Broken(format!("ratchet.toml: does not parse: {error}")))?;
    let paths = strings(config.pointer("/spec/paths")).unwrap_or_default();
    let first = paths.first().ok_or_else(|| {
        Failure::Broken("ratchet.toml: names no plan file under [spec] paths".to_owned())
    })?;
    let plan: Yaml = serde_norway::from_str(&read_text(root, first)?)
        .map_err(|error| Failure::Broken(format!("{first}: does not parse: {error}")))?;
    plan.get("project")
        .and_then(Yaml::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Failure::Broken(format!("{first}: names no project")))
}

/// Every entry under `from`, by its path from it with `/`, and whether it is a plain file: a
/// link is neither a file nor a directory and is not followed.
pub(super) fn artifact_files(from: &Path) -> Outcome<Vec<(String, bool)>> {
    let mut found = Vec::new();
    let mut open = vec![(from.to_path_buf(), String::new())];
    while let Some((directory, prefix)) = open.pop() {
        let entries = fs::read_dir(&directory).map_err(|error| {
            Failure::Refused(vec![format!(
                "{}: cannot be read as a directory: {error}",
                directory.display()
            )])
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                Failure::Refused(vec![format!("{}: {error}", directory.display())])
            })?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = format!("{prefix}{name}");
            let kind = entry.file_type().map_err(|error| {
                Failure::Refused(vec![format!("{}: {error}", entry.path().display())])
            })?;
            if kind.is_dir() {
                open.push((entry.path(), format!("{path}/")));
            }
            found.push((path, kind.is_file()));
        }
    }
    found.sort();
    Ok(found)
}

/// The text at `pointer` of `value`.
fn text_at<'v>(value: &'v Value, pointer: &str) -> Option<&'v str> {
    value.pointer(pointer).and_then(Value::as_str)
}

/// What makes a record one of this run: its class, its source, its run and its target.
fn of_the_run(
    root: &Path,
    run: u64,
    commit: Option<&str>,
    expected: &Expected,
    plan: &Plan,
    record: &Value,
) -> Outcome<Vec<String>> {
    let mut problems = Vec::new();
    let class = text_at(record, "/evidence/target/class");
    match class {
        Some("ci") => {}
        Some(class) if CLASSES.contains(&class) => problems.push(format!(
            "the class is {class:?}; a record of a run on the forge has the class \"ci\""
        )),
        other => problems.push(format!(
            "the class is {other:?}, not one of {}",
            CLASSES.join(", ")
        )),
    }
    let location = text_at(record, "/evidence/source/location").unwrap_or_default();
    match source(location, plan.reader) {
        Err(Failure::Call(why) | Failure::Broken(why)) => {
            problems.push(format!("assess refuses its source: {why}"));
        }
        Err(Failure::Refused(lines)) => {
            problems.push(format!("assess refuses its source: {}", lines.join("; ")));
        }
        Ok(source) => {
            let kind = text_at(record, "/evidence/source/kind");
            if kind != Some(source.kind) {
                problems.push(format!(
                    "the source kind is {kind:?}, and assess writes {:?} for its location",
                    source.kind
                ));
            }
            if source.kind != "forge_runs" {
                problems.push(format!(
                    "the source is {location:?}, which is no run on the forge"
                ));
            } else if location != format!("{RUNS}{run}") {
                problems.push(format!(
                    "the source is {location:?}, not the run {run}, {RUNS}{run}"
                ));
            }
        }
    }
    let kind = text_at(record, "/evidence/target/kind");
    if kind != Some(expected.kind.as_str()) {
        problems.push(format!(
            "the target kind is {kind:?}, its plan's is {:?}",
            expected.kind
        ));
        return Ok(problems);
    }
    let selected = plan
        .assessment
        .get("target")
        .and_then(|target| target.get("selection"))
        .is_some();
    let candidate = text_at(record, "/evidence/candidate");
    let mut written = serde_json::Map::new();
    written.insert("kind".to_owned(), expected.kind.clone().into());
    written.insert("class".to_owned(), "ci".into());
    if let Some(selection) = plan
        .assessment
        .get("target")
        .and_then(|target| target.get("selection"))
        .and_then(Yaml::as_str)
    {
        written.insert("selection".to_owned(), selection.into());
    }
    let identity = match expected.kind.as_str() {
        "revision" => {
            let revision = text_at(record, "/evidence/target/revision");
            if let Some(commit) = commit
                && revision != Some(commit)
            {
                problems.push(format!(
                    "the revision is {revision:?}, the commit of the run is {commit}"
                ));
            }
            if record.pointer("/evidence/target/identifiers").is_some() {
                problems.push(
                    "the target is a revision and names identifiers, which assess never writes for a revision"
                        .to_owned(),
                );
            }
            if let Some(revision) = commit.or(revision) {
                written.insert("revision".to_owned(), revision.into());
            }
            revision.map(str::to_owned)
        }
        "document" => {
            let input = plan.text(&plan.declaration, "target_input")?;
            let path = plan.input(input).and_then(Yaml::as_str).ok_or_else(|| {
                Failure::Broken(format!(
                    "{}: the plan gives no path as the input {input}",
                    plan.id
                ))
            })?;
            let digest =
                format!(
                    "sha256:{}",
                    sha256(&fs::read(root.join(path)).map_err(|error| Failure::Broken(
                        format!("{path}: cannot be read: {error}")
                    ),)?)
                );
            if text_at(record, "/evidence/target/revision").is_some() {
                problems.push("the target is a document and has a revision".to_owned());
            }
            let named = text_at(record, "/evidence/target/identifiers/path");
            if named != Some(path) {
                problems.push(format!("the target is {named:?}, its plan's is {path:?}"));
            }
            let held = text_at(record, "/evidence/target/identifiers/digest");
            if held != Some(digest.as_str()) {
                problems.push(format!(
                    "the digest of the target is {held:?}, {path} of the tree has {digest}"
                ));
            }
            let form = text_at(record, "/evidence/target/identifiers/digest_form");
            if form != Some("bytes") {
                problems.push(format!(
                    "the digest form of the target is {form:?}, assess writes \"bytes\""
                ));
            }
            written.insert(
                "identifiers".to_owned(),
                serde_json::json!({"path": path, "digest": digest, "digest_form": "bytes"}),
            );
            Some(digest)
        }
        other => {
            problems.push(format!("a target of the kind {other} is not adopted"));
            None
        }
    };
    let wanted = identity.filter(|_| selected);
    if candidate != wanted.as_deref() {
        problems.push(format!(
            "the candidate is {candidate:?}, its target gives {wanted:?}"
        ));
    }
    let written = Value::Object(written);
    let target = record.pointer("/evidence/target").unwrap_or(&Value::Null);
    if *target != written {
        problems.push(format!(
            "the target is {target}, assess writes {written} for its plan and the run"
        ));
    }
    Ok(problems)
}

/// The rules a record is held to by itself and against its plan, as `record_problems` of the
/// test of the quality documents holds them (`tests/quality/mod.rs`): its format, project,
/// assessment, assessor, outputs, units and the counts of its coverage; a result for each
/// measure of its plan, and for each package of a measure with one value per package; the
/// packages and the coverage sums of a plan that names packages; one result per test of a plan
/// that lists tests. A test of `tests/quality/adopt.rs` plants a defect of each rule in a record
/// and holds both to refuse it.
///
/// One rule of `record_problems` is not held here: it leaves out a package whose section of
/// `quality/adapters.md` says that no adopted record holds the crate and that the record's own
/// run did not plan, so that the committed record written before the plan named the crate
/// stands. This program reads no page and refuses such a record: the record of a run on `main`
/// of a tree whose plan names the crate holds it, and one without it is of an older plan. A test
/// of `tests/quality/adopt.rs` holds that difference.
fn record_rules(project: &str, plan: &Plan, record: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    let evidence = record.get("evidence").unwrap_or(&Value::Null);
    let field = |key: &str| evidence.get(key).and_then(Value::as_str);
    let format = record.get("format").and_then(Value::as_str);
    if format != Some(EVIDENCE_FORMAT) {
        problems.push(format!("format is {format:?}, not {EVIDENCE_FORMAT:?}"));
    }
    if field("project") != Some(project) {
        problems.push(format!(
            "project is {:?}, the specification's is {project:?}",
            field("project")
        ));
    }
    let name = field("assessment").unwrap_or_default();
    if name != plan.id {
        problems.push(format!(
            "is the record of {name:?}, which is not its file name"
        ));
    }
    let assessor = field("assessor").unwrap_or_default();
    if assessor != plan.assessor {
        problems.push(format!(
            "the assessor is {assessor:?}, the assessment's is Some({:?})",
            plan.assessor
        ));
    }
    let declared = |key: &str| plan.declaration.get(key);
    let reported: Vec<&str> = declared("coverage")
        .and_then(Yaml::as_sequence)
        .into_iter()
        .flatten()
        .filter_map(Yaml::as_str)
        .collect();
    for count in COVERAGE_COUNTS {
        let counted = evidence
            .get("coverage")
            .and_then(|coverage| coverage.get(count))
            .and_then(Value::as_u64);
        if !reported.contains(&count) && counted != Some(0) {
            problems.push(format!(
                "the coverage has {counted:?} for `{count}`, a count {assessor} does not report; such a count is 0"
            ));
        }
    }
    let all: Vec<&Value> = evidence
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    for result in &all {
        let output = result
            .get("output")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(output_declared) = declared("outputs").and_then(|outputs| outputs.get(output))
        else {
            problems.push(format!("{assessor} declares no output {output:?}"));
            continue;
        };
        let unit = result.get("unit").and_then(Value::as_str);
        let declared_unit = output_declared.get("unit").and_then(Yaml::as_str);
        if unit != declared_unit {
            problems.push(format!(
                "the unit of {output} is {unit:?}, {assessor} declares {declared_unit:?}"
            ));
        }
    }
    let listed = |name: &str| -> Option<BTreeSet<&str>> {
        Some(
            plan.input(name)?
                .as_sequence()?
                .iter()
                .filter_map(Yaml::as_str)
                .collect(),
        )
    };
    let packages = listed("packages");
    let tests = listed("tests");
    let of = |measurement: &Yaml| -> Vec<&Value> {
        let output = measurement.get("output").and_then(Yaml::as_str);
        let boundary = measurement.get("boundary").and_then(Yaml::as_str);
        all.iter()
            .copied()
            .filter(|result| result.get("output").and_then(Value::as_str) == output)
            .filter(|result| {
                boundary.is_none_or(|boundary| {
                    result.get("boundary").and_then(Value::as_str) == Some(boundary)
                })
            })
            .collect()
    };
    let items = |results: &[&Value]| -> BTreeSet<String> {
        results
            .iter()
            .filter_map(|result| result.get("item").and_then(Value::as_str))
            .map(str::to_owned)
            .collect()
    };
    let id = &plan.id;
    for (measure, measurement) in plan.measurements() {
        let key = format!("{id}/{measure}");
        let found = of(measurement);
        if found.is_empty() {
            problems.push(format!("no result of `{key}`"));
            continue;
        }
        let held = items(&found);
        if measurement.get("scope").and_then(Yaml::as_str) == Some("package") {
            for package in packages.iter().flatten() {
                if !held.contains(*package) {
                    problems.push(format!("no result of `{key}` for the package {package}"));
                }
            }
        }
        if let Some(tests) = &tests {
            for test in tests.iter().filter(|test| !held.contains(**test)) {
                problems.push(format!(
                    "no result of `{key}` for the test {test}, which assessments.{id} lists"
                ));
            }
            for test in held.iter().filter(|test| !tests.contains(test.as_str())) {
                problems.push(format!(
                    "a result of `{key}` for the test {test}, which assessments.{id} does not list"
                ));
            }
        }
    }
    if let Some(packages) = &packages {
        let with = |output: &str| -> Vec<&Value> {
            all.iter()
                .copied()
                .filter(|result| result.get("output").and_then(Value::as_str) == Some(output))
                .collect()
        };
        let held = items(&with("tests_run"));
        let named: BTreeSet<String> = packages
            .iter()
            .map(|package| (*package).to_owned())
            .collect();
        if named != held {
            problems.push(format!(
                "the `tests_run` results are of {held:?}, assessments.{id} names the packages {named:?}"
            ));
        }
        for (count, output) in [("executed", "tests_run"), ("passed", "tests_passed")] {
            let sum: u64 = with(output)
                .iter()
                .filter_map(|result| result.get("value")?.as_str()?.parse::<u64>().ok())
                .sum();
            let counted = evidence
                .get("coverage")
                .and_then(|coverage| coverage.get(count))
                .and_then(Value::as_u64);
            if counted != Some(sum) {
                problems.push(format!(
                    "the coverage counts {counted:?} {count} tests, and the `{output}` results of the packages of assessments.{id} sum to {sum}"
                ));
            }
        }
    }
    problems
}

/// One record to write: where, through which temporary file, and what stood there before.
struct Planned<'a> {
    /// Its path from the root, as the program names it.
    shown: String,
    path: PathBuf,
    temporary: PathBuf,
    earlier: Option<Vec<u8>>,
    bytes: &'a [u8],
}

/// Writes every record to `quality/records/<assessment>.evidence.json` under `root`: each to a
/// temporary file first, then each renamed over its record. When a write or a rename fails, the
/// temporary files are removed and the records renamed before it are put back.
fn write_all(root: &Path, adopted: &[(String, Vec<u8>)]) -> Outcome<Vec<String>> {
    let directory = root.join(RECORDS);
    fs::create_dir_all(&directory)
        .map_err(|error| Failure::Broken(format!("{RECORDS}: cannot be created: {error}")))?;
    let planned: Vec<Planned<'_>> = adopted
        .iter()
        .map(|(assessment, bytes)| {
            let name = format!("{assessment}.evidence.json");
            let path = directory.join(&name);
            Planned {
                shown: format!("{RECORDS}/{name}"),
                earlier: fs::read(&path).ok(),
                path,
                temporary: directory.join(format!(".{name}{TEMPORARY}")),
                bytes: bytes.as_slice(),
            }
        })
        .collect();
    let remove_temporaries = |upto: usize| {
        for one in &planned[..upto] {
            let _ = fs::remove_file(&one.temporary);
        }
    };
    for (at, one) in planned.iter().enumerate() {
        if let Err(error) = write_new(&one.temporary, one.bytes) {
            remove_temporaries(at + 1);
            return Err(Failure::Broken(format!(
                "{}: its temporary file {} cannot be written: {error}; no record was adopted",
                one.shown,
                one.temporary.display()
            )));
        }
    }
    for (at, one) in planned.iter().enumerate() {
        if let Err(error) = fs::rename(&one.temporary, &one.path) {
            remove_temporaries(planned.len());
            let mut restored = Vec::new();
            for earlier in &planned[..at] {
                let put_back = match &earlier.earlier {
                    Some(bytes) => fs::write(&earlier.path, bytes),
                    None => fs::remove_file(&earlier.path),
                };
                if let Err(error) = put_back {
                    restored.push(format!("{} cannot be put back: {error}", earlier.shown));
                }
            }
            let after = if restored.is_empty() {
                "the records adopted before it are put back; no record was adopted".to_owned()
            } else {
                restored.join("; ")
            };
            return Err(Failure::Broken(format!(
                "{}: cannot be replaced: {error}; {after}",
                one.shown
            )));
        }
    }
    Ok(planned.into_iter().map(|one| one.shown).collect())
}
