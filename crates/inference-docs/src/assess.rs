//! `assess`: writes the `ratchet-evidence/1` document of one assessment from a file a run left
//! behind. It runs no measure; it reads what a run wrote.
//!
//! The call is Ratchet's call of an assessor, with two arguments of this program's own:
//!
//! ```sh
//! inference-docs assess --project inference --assessment workspace-tests --root . --class ci \
//!   --out workspace-tests.evidence.json --revision <commit> \
//!   --from target/nextest/default/junit.xml --source <check run URL>
//! ```
//!
//! `--from` is the file the run left, `--source` what the document names as its origin: an http
//! or https URL, which is a forge run, or a path from the root that does not leave it.
//! `--started-at` and `--ended-at` give the times of a run whose file holds none, and
//! `--exit-status` the exit status of the command whose output is read.
//!
//! It finds the assessment in the plan files `ratchet.toml` names and the assessor's declaration
//! under the catalog sources there, and reads the file by the assessor:
//!
//! - `nextest-run@1`, `package-tests@1`, `named-tests@1` and `capability-record@1` read the JUnit
//!   report of cargo-nextest (`junit`); `capability-record@1` reads its value from the record and
//!   whether the record's test passed from the report. A report whose root counts other tests,
//!   failures, errors or skipped tests than the test cases it holds is refused, and so is one with
//!   a test case outside a `<testsuite>` directly under the root.
//! - `printed-duration@1` reads the output a live test or an example printed (`printed`), and
//!   whether the run held: from the lines of `cargo test` or cargo-nextest for a test, from
//!   `--exit-status` for an example. A run whose test failed is refused.
//!
//! The report or the output is the caller's choice: the document cannot tell which nextest
//! profile wrote a report, and writes the `profile` input the plan names.
//!
//! Exit 0: one document is written to `--out`. Exit 1: the file does not hold what the assessment
//! reads; no document, and one line on stderr for each thing it lacks. Exit 2: the call is wrong
//! (an argument, or an assessment this program has no reader for). Exit 3: the program cannot
//! read the plan or the declaration, or cannot write the document.
//!
//! The document holds no time but the run's, no host and no absolute path, so the same file
//! gives the same bytes.

mod junit;
mod printed;

use std::{
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::Args;
use serde::{Serialize, Serializer, ser::SerializeMap};
use serde_norway::Value as Yaml;
use sha2::{Digest, Sha256};

use self::junit::{Lookup, Report, Verdict};

/// The call of `assess`.
#[derive(Args)]
pub(crate) struct Arguments {
    /// The `project` of the plan files.
    #[arg(long)]
    project: String,
    /// The key of the assessment under `assessments`.
    #[arg(long)]
    assessment: String,
    /// The root of the tree that is assessed.
    #[arg(long, default_value = ".")]
    root: PathBuf,
    /// The class of the run: local, ci, ephemeral, staging or production.
    #[arg(long)]
    class: String,
    /// The file the document is written to.
    #[arg(long)]
    out: PathBuf,
    /// The commit of the tree, for a target of the kind revision.
    #[arg(long)]
    revision: Option<String>,
    /// The file the run left: a JUnit report or a printed output.
    #[arg(long)]
    from: PathBuf,
    /// Where the run came from: the URL of a forge run, or a path from the root.
    #[arg(long)]
    source: String,
    /// When the run started, `2026-11-24T20:02:05Z`, for a file that holds no time.
    #[arg(long)]
    started_at: Option<String>,
    /// When the run ended, in the same form.
    #[arg(long)]
    ended_at: Option<String>,
    /// The exit status of the command whose output `--from` is: required for an example, whose
    /// output says nothing of whether it held.
    #[arg(long, allow_negative_numbers = true)]
    exit_status: Option<i32>,
}

/// Why no document was written.
enum Failure {
    /// Exit 1: the file does not hold what the assessment reads, one line per thing.
    Refused(Vec<String>),
    /// Exit 2: the call is wrong.
    Call(String),
    /// Exit 3: the program cannot read the plan, or cannot write.
    Broken(String),
}

type Outcome<T> = Result<T, Failure>;

const CLASSES: [&str; 5] = ["local", "ci", "ephemeral", "staging", "production"];

/// Runs the call: the document, or the lines on stderr and the exit code of the failure.
pub(crate) fn run(arguments: &Arguments) -> ExitCode {
    let (code, lines) = match assess(arguments) {
        Ok(()) => return ExitCode::SUCCESS,
        Err(Failure::Refused(lines)) => (1, lines),
        Err(Failure::Call(line)) => (2, vec![line]),
        Err(Failure::Broken(line)) => (3, vec![line]),
    };
    for line in lines {
        eprintln!("inference-docs assess: {line}");
    }
    ExitCode::from(code)
}

fn assess(arguments: &Arguments) -> Outcome<()> {
    let plan = Plan::read(&arguments.root, &arguments.project, &arguments.assessment)?;
    let id = arguments.assessment.as_str();
    if arguments.exit_status.is_some() && plan.reader != Reader::PrintedDuration {
        return Err(Failure::Call(format!(
            "--exit-status is the exit status of a command whose output is read, and {id} reads a JUnit report"
        )));
    }
    let times = given_times(arguments)?;
    let source = source(&arguments.source, plan.reader)?;
    let target = target(arguments, &plan)?;
    let inputs = inputs(&plan);
    let read = match plan.reader {
        Reader::NextestRun => nextest_run(&plan, &report(arguments)?)?,
        Reader::PackageTests => package_tests(&plan, &report(arguments)?)?,
        Reader::NamedTests => named_tests(&plan, &report(arguments)?)?,
        Reader::CapabilityRecord => capability_record(arguments, &plan, &target)?,
        Reader::PrintedDuration => printed_duration(arguments, &plan, times.is_some())?,
    };
    let sampled = read.results.iter().any(|result| result.samples.is_some());
    let run = match (sampled, times.or(read.times)) {
        (false, _) => None,
        (true, Some((started, ended))) => Some(RunTimes {
            started_at: utc(started),
            ended_at: utc(ended),
        }),
        (true, None) => {
            return Err(Failure::Refused(vec![format!(
                "{id}: the report holds no timestamp and no time of the run; give --started-at and --ended-at"
            )]));
        }
    };
    let conditions = conditions(arguments, &plan)?;
    let coverage = coverage(&plan, read.counts)?;
    let document = Document {
        format: "ratchet-evidence/1",
        evidence: Evidence {
            project: arguments.project.clone(),
            assessment: arguments.assessment.clone(),
            assessor: plan.assessor.clone(),
            candidate: target.candidate.clone(),
            target: target.written,
            inputs,
            completion: "completed",
            conditions,
            coverage,
            results: read.results,
            source,
        },
        run,
    };
    let mut text = serde_json::to_string_pretty(&document)
        .map_err(|error| Failure::Broken(format!("the document cannot be written: {error}")))?;
    text.push('\n');
    fs::write(&arguments.out, text).map_err(|error| {
        Failure::Broken(format!(
            "{}: cannot be written: {error}",
            arguments.out.display()
        ))
    })
}

/// The assessors this program reads a file for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reader {
    NextestRun,
    PackageTests,
    NamedTests,
    CapabilityRecord,
    PrintedDuration,
}

impl Reader {
    fn of(assessor: &str) -> Option<Self> {
        match assessor {
            "nextest-run@1" => Some(Self::NextestRun),
            "package-tests@1" => Some(Self::PackageTests),
            "named-tests@1" => Some(Self::NamedTests),
            "capability-record@1" => Some(Self::CapabilityRecord),
            "printed-duration@1" => Some(Self::PrintedDuration),
            _ => None,
        }
    }
}

/// The assessment in its plan file, and the declaration of its assessor.
struct Plan {
    id: String,
    assessor: String,
    reader: Reader,
    assessment: Yaml,
    declaration: Yaml,
}

impl Plan {
    fn read(root: &Path, project: &str, id: &str) -> Outcome<Self> {
        let config: serde_json::Value = toml::from_str(&read_text(root, "ratchet.toml")?)
            .map_err(|error| Failure::Broken(format!("ratchet.toml: does not parse: {error}")))?;
        let paths = strings(config.pointer("/spec/paths")).ok_or_else(|| {
            Failure::Broken("ratchet.toml: names no plan file under [spec] paths".to_owned())
        })?;
        let mut found = None;
        for path in &paths {
            let plan: Yaml = serde_norway::from_str(&read_text(root, path)?)
                .map_err(|error| Failure::Broken(format!("{path}: does not parse: {error}")))?;
            let of = plan
                .get("project")
                .and_then(Yaml::as_str)
                .unwrap_or_default();
            if of != project {
                return Err(Failure::Call(format!(
                    "--project is {project}, the plan files of ratchet.toml are of the project {of}"
                )));
            }
            if found.is_none()
                && let Some(assessment) = plan.get("assessments").and_then(|all| all.get(id))
            {
                found = Some(assessment.clone());
            }
        }
        let assessment = found.ok_or_else(|| {
            Failure::Call(format!(
                "no plan file of ratchet.toml declares the assessment {id}"
            ))
        })?;
        let assessor = assessment
            .get("assessor")
            .and_then(Yaml::as_str)
            .ok_or_else(|| Failure::Broken(format!("{id}: the plan names no assessor")))?
            .to_owned();
        let reader = Reader::of(&assessor).ok_or_else(|| {
            Failure::Call(format!(
                "the assessor {assessor} of {id} has no reader in this program"
            ))
        })?;
        let declaration = declaration(root, &config, &assessor)?;
        Ok(Self {
            id: id.to_owned(),
            assessor,
            reader,
            assessment,
            declaration,
        })
    }

    /// The measurements of the assessment, in the order of the plan.
    fn measurements(&self) -> Vec<(String, &Yaml)> {
        self.assessment
            .get("measurements")
            .and_then(Yaml::as_mapping)
            .into_iter()
            .flatten()
            .filter_map(|(name, measurement)| Some((name.as_str()?.to_owned(), measurement)))
            .collect()
    }

    /// The input `name` of the plan.
    fn input(&self, name: &str) -> Option<&Yaml> {
        self.assessment
            .get("inputs")
            .and_then(|inputs| inputs.get(name))
    }

    /// The text of the plan at `key` of `value`, or a failure that names it.
    fn text<'v>(&self, value: &'v Yaml, key: &str) -> Outcome<&'v str> {
        value
            .get(key)
            .and_then(Yaml::as_str)
            .ok_or_else(|| Failure::Broken(format!("{}: the plan has no text at {key}", self.id)))
    }

    /// The declared output `name` of the assessor.
    fn output(&self, name: &str) -> Outcome<&Yaml> {
        self.declaration
            .get("outputs")
            .and_then(|outputs| outputs.get(name))
            .ok_or_else(|| {
                Failure::Broken(format!(
                    "{}: the assessor {} declares no output {name}",
                    self.id, self.assessor
                ))
            })
    }

    /// A result of the measurement, with what the plan and the declaration say of it.
    fn result(&self, measurement: &Yaml, item: Option<&str>) -> Outcome<MeasuredResult> {
        let output = self.text(measurement, "output")?;
        let declared = self.output(output)?;
        let kind = self.text(measurement, "kind")?;
        let unit = measurement
            .get("unit")
            .or_else(|| declared.get("unit"))
            .and_then(Yaml::as_str);
        Ok(MeasuredResult {
            output: output.to_owned(),
            kind: kind.to_owned(),
            scope: self.text(measurement, "scope")?.to_owned(),
            item: item.map(str::to_owned),
            boundary: measurement
                .get("boundary")
                .and_then(Yaml::as_str)
                .map(str::to_owned),
            value_type: self.text(declared, "value_type")?.to_owned(),
            value: None,
            unit: unit.map(str::to_owned),
            samples: (kind == "sampled").then_some(1),
            holds: None,
            details: None,
        })
    }
}

/// The text of the file at `path` from the root, or a failure of the program.
fn read_text(root: &Path, path: &str) -> Outcome<String> {
    fs::read_to_string(root.join(path))
        .map_err(|error| Failure::Broken(format!("{path}: cannot be read: {error}")))
}

fn strings(value: Option<&serde_json::Value>) -> Option<Vec<String>> {
    value?
        .as_array()?
        .iter()
        .map(|item| item.as_str().map(str::to_owned))
        .collect()
}

/// The declaration of `assessor` (`name@version`) under the catalog sources of `ratchet.toml`.
fn declaration(root: &Path, config: &serde_json::Value, assessor: &str) -> Outcome<Yaml> {
    let (name, version) = assessor.split_once('@').unwrap_or((assessor, ""));
    let sources = config
        .pointer("/catalog/sources")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|source| source.get("path").and_then(serde_json::Value::as_str));
    for directory in sources {
        let path = format!("{directory}/{assessor}.assessor.yaml");
        if !root.join(&path).is_file() {
            continue;
        }
        let declaration: Yaml = serde_norway::from_str(&read_text(root, &path)?)
            .map_err(|error| Failure::Broken(format!("{path}: does not parse: {error}")))?;
        let declared = (
            declaration.get("name").and_then(Yaml::as_str),
            declaration.get("version").and_then(Yaml::as_str),
        );
        if declared != (Some(name), Some(version)) {
            return Err(Failure::Broken(format!(
                "{path}: declares another assessor than {assessor}"
            )));
        }
        return Ok(declaration);
    }
    Err(Failure::Broken(format!(
        "no catalog source of ratchet.toml holds the declaration of {assessor}"
    )))
}

/// The times `--started-at` and `--ended-at` give, in nanoseconds since 1970.
fn given_times(arguments: &Arguments) -> Outcome<Option<(i128, i128)>> {
    let parse = |flag: &str, text: &str| {
        exact_utc(text).ok_or_else(|| {
            Failure::Call(format!(
                "{flag} is {text:?}, not a time of the form 2026-11-24T20:02:05Z"
            ))
        })
    };
    match (&arguments.started_at, &arguments.ended_at) {
        (None, None) => Ok(None),
        (Some(started), Some(ended)) => {
            let started = parse("--started-at", started)?;
            let ended = parse("--ended-at", ended)?;
            if ended < started {
                return Err(Failure::Call(
                    "--ended-at is before --started-at".to_owned(),
                ));
            }
            Ok(Some((started, ended)))
        }
        _ => Err(Failure::Call(
            "give --started-at and --ended-at together, or neither".to_owned(),
        )),
    }
}

/// What `--source` names: an http or https URL, or a non-empty path from the root without `..`,
/// a scheme or a leading `~`.
fn source(location: &str, reader: Reader) -> Outcome<Source> {
    let wrong = |why: &str| -> Outcome<Source> {
        Err(Failure::Call(format!(
            "--source is {location:?}, {why}; give the http or https URL of a forge run or a path from the root"
        )))
    };
    if location.starts_with('/') || Path::new(location).is_absolute() {
        return Err(Failure::Call(
            "--source is an absolute path; a document names a file by its path from the root"
                .to_owned(),
        ));
    }
    let url = location
        .strip_prefix("https://")
        .or_else(|| location.strip_prefix("http://"));
    let kind = if let Some(rest) = url {
        if rest.is_empty() || rest.starts_with('/') || location.contains(char::is_whitespace) {
            return wrong("a URL without a host");
        }
        "forge_runs"
    } else if location.is_empty() {
        return wrong("which is empty");
    } else if location.starts_with('~') {
        return wrong("a path from a home directory");
    } else if location
        .split_once(':')
        .is_some_and(|(scheme, _)| !scheme.is_empty() && !scheme.contains('/'))
    {
        return wrong("a URL that is not http or https");
    } else if location.contains('\\') {
        return wrong("a path with `\\`");
    } else if location.split('/').any(|part| part == "..") {
        return wrong("a path that leaves the root");
    } else if reader == Reader::PrintedDuration {
        "build_pool_log"
    } else {
        "junit"
    };
    Ok(Source {
        kind,
        location: location.to_owned(),
    })
}

/// The resolved target, and the candidate it gives.
struct Resolved {
    written: Target,
    candidate: Option<String>,
    /// The bytes of the document of a target of the kind document.
    document: Option<(String, Vec<u8>)>,
}

fn target(arguments: &Arguments, plan: &Plan) -> Outcome<Resolved> {
    let id = plan.id.as_str();
    let class = arguments.class.as_str();
    if !CLASSES.contains(&class) {
        return Err(Failure::Call(format!(
            "--class is {class}, not one of {}",
            CLASSES.join(", ")
        )));
    }
    let declared = plan
        .assessment
        .get("target")
        .ok_or_else(|| Failure::Broken(format!("{id}: the plan has no target")))?;
    let environments: Vec<String> = declared
        .get("environments")
        .and_then(Yaml::as_sequence)
        .into_iter()
        .flatten()
        .filter_map(|environment| environment.as_str().map(str::to_owned))
        .collect();
    if !environments.iter().any(|environment| environment == class) {
        return Err(Failure::Call(format!(
            "{id}: the plan assesses the classes {environments:?}, not {class}"
        )));
    }
    let kind = plan.text(declared, "kind")?;
    let supported = plan
        .declaration
        .get("targets")
        .and_then(Yaml::as_sequence)
        .is_some_and(|targets| targets.iter().any(|target| target.as_str() == Some(kind)));
    if !supported {
        return Err(Failure::Broken(format!(
            "{id}: the assessor {} does not take a target of the kind {kind}",
            plan.assessor
        )));
    }
    let selection = declared
        .get("selection")
        .and_then(Yaml::as_str)
        .map(str::to_owned);
    let revision = match &arguments.revision {
        Some(revision)
            if !matches!(revision.len(), 40 | 64)
                || !revision
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) =>
        {
            return Err(Failure::Call(format!(
                "--revision is {revision}, not a full commit id in lowercase hex"
            )));
        }
        revision => revision.clone(),
    };
    let mut written = Target {
        kind: kind.to_owned(),
        class: class.to_owned(),
        selection: selection.clone(),
        revision: None,
        identifiers: None,
    };
    match kind {
        "revision" => {
            let revision = revision.ok_or_else(|| {
                Failure::Call(format!("{id}: its target is a revision; give --revision"))
            })?;
            written.revision = Some(revision.clone());
            Ok(Resolved {
                written,
                candidate: selection.map(|_| revision),
                document: None,
            })
        }
        "document" => {
            if revision.is_some() {
                return Err(Failure::Call(format!(
                    "{id}: its target is a document, which has no revision; give no --revision"
                )));
            }
            let input = plan.text(&plan.declaration, "target_input")?;
            let path = plan
                .input(input)
                .and_then(Yaml::as_str)
                .ok_or_else(|| {
                    Failure::Broken(format!("{id}: the plan gives no path as the input {input}"))
                })?
                .to_owned();
            let bytes = fs::read(arguments.root.join(&path)).map_err(|error| {
                Failure::Refused(vec![format!("{id}: {path} cannot be read: {error}")])
            })?;
            let digest = format!("sha256:{}", sha256(&bytes));
            written.identifiers = Some(Identifiers {
                path: path.clone(),
                digest: digest.clone(),
                digest_form: "bytes",
            });
            Ok(Resolved {
                written,
                candidate: selection.map(|_| digest),
                document: Some((path, bytes)),
            })
        }
        "build" => {
            if selection.is_some() {
                return Err(Failure::Broken(format!(
                    "{id}: a build with a selection is identified by an artifact digest, which this program does not read"
                )));
            }
            written.revision = revision;
            Ok(Resolved {
                written,
                candidate: None,
                document: None,
            })
        }
        other => Err(Failure::Broken(format!(
            "{id}: this program resolves no target of the kind {other}"
        ))),
    }
}

fn sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The inputs of the plan that are neither local nor the target, in the order of the plan.
fn inputs(plan: &Plan) -> Ordered<InputValue> {
    let target = plan.declaration.get("target_input").and_then(Yaml::as_str);
    let local = |name: &str| {
        plan.declaration
            .get("inputs")
            .and_then(|inputs| inputs.get(name))
            .and_then(|input| input.get("local"))
            .and_then(Yaml::as_bool)
            .unwrap_or(false)
    };
    Ordered(
        plan.assessment
            .get("inputs")
            .and_then(Yaml::as_mapping)
            .into_iter()
            .flatten()
            .filter_map(|(name, value)| {
                let name = name.as_str()?;
                if Some(name) == target || local(name) {
                    return None;
                }
                let value = serde_json::to_value(value).ok()?;
                Some((name.to_owned(), InputValue { value }))
            })
            .collect(),
    )
}

/// The conditions the plan declares for every class and for the class of the run.
fn conditions(arguments: &Arguments, plan: &Plan) -> Outcome<Conditions> {
    let id = plan.id.as_str();
    let mut written = Conditions {
        runner: None,
        tools: Ordered(Vec::new()),
        other: Ordered(Vec::new()),
    };
    let declared = plan
        .assessment
        .get("observation")
        .and_then(|observation| observation.get("conditions"));
    for class in ["all", arguments.class.as_str()] {
        let Some(conditions) = declared
            .and_then(|declared| declared.get(class))
            .and_then(Yaml::as_mapping)
        else {
            continue;
        };
        for (key, value) in conditions {
            match key.as_str() {
                Some("runner") => written.runner = value.as_str().map(str::to_owned),
                Some("tools") => {
                    for (tool, version) in value.as_mapping().into_iter().flatten() {
                        let tool = tool.as_str().unwrap_or_default();
                        let version = tool_version(&arguments.root, id, tool, version)?;
                        written.tools.put(tool, version);
                    }
                }
                Some(other) => {
                    let text = match value {
                        Yaml::String(text) => text.clone(),
                        Yaml::Number(number) => number.to_string(),
                        Yaml::Bool(flag) => flag.to_string(),
                        _ => {
                            return Err(Failure::Broken(format!(
                                "{id}: the condition {other} is not a text"
                            )));
                        }
                    };
                    written.other.put(other, text);
                }
                None => {}
            }
        }
    }
    Ok(written)
}

/// The version of `tool` the plan declares: given, or read from the file that pins it.
fn tool_version(root: &Path, id: &str, tool: &str, declared: &Yaml) -> Outcome<String> {
    if let Some(version) = declared.get("version").and_then(Yaml::as_str) {
        return Ok(version.to_owned());
    }
    let Some(pin) = declared.get("pin").and_then(Yaml::as_str) else {
        return Err(Failure::Broken(format!(
            "{id}: the plan gives the tool {tool} neither a version nor a pin"
        )));
    };
    let text = fs::read_to_string(root.join(pin))
        .map_err(|error| Failure::Refused(vec![format!("{id}: {pin} cannot be read: {error}")]))?;
    let file: serde_json::Value = toml::from_str(&text)
        .map_err(|error| Failure::Refused(vec![format!("{id}: {pin} does not parse: {error}")]))?;
    let version = match tool {
        "rustc" => file.pointer("/toolchain/channel"),
        "cargo-nextest" => file
            .pointer("/nextest-version/required")
            .or_else(|| file.pointer("/nextest-version")),
        _ => {
            return Err(Failure::Broken(format!(
                "{id}: this program reads no version of {tool} from a pin"
            )));
        }
    };
    version
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Failure::Refused(vec![format!("{id}: {pin} pins no version of {tool}")]))
}

/// The counts of a run, before the declaration says which it reports.
#[derive(Default)]
struct Counts {
    executed: u64,
    skipped: u64,
    passed: u64,
    failed: u64,
}

impl Counts {
    fn add(&mut self, verdict: Verdict) {
        match verdict {
            Verdict::Passed => {
                self.executed += 1;
                self.passed += 1;
            }
            Verdict::Failed => {
                self.executed += 1;
                self.failed += 1;
            }
            Verdict::Skipped => self.skipped += 1,
        }
    }
}

/// The coverage in the declared unit, with each count the assessor does not report at 0.
fn coverage(plan: &Plan, counts: Counts) -> Outcome<Coverage> {
    let unit = plan.text(&plan.declaration, "coverage_unit")?.to_owned();
    let reported: Vec<&str> = plan
        .declaration
        .get("coverage")
        .and_then(Yaml::as_sequence)
        .into_iter()
        .flatten()
        .filter_map(Yaml::as_str)
        .collect();
    let count = |name: &str, value: u64| if reported.contains(&name) { value } else { 0 };
    Ok(Coverage {
        unit,
        executed: count("executed", counts.executed),
        skipped: count("skipped", counts.skipped),
        not_run: 0,
        unsupported: 0,
        passed: count("passed", counts.passed),
        failed: count("failed", counts.failed),
    })
}

/// What a reader found.
struct Read {
    counts: Counts,
    results: Vec<MeasuredResult>,
    /// The start and the end of the run, where the file holds them.
    times: Option<(i128, i128)>,
}

/// The JUnit report at `--from`.
fn report(arguments: &Arguments) -> Outcome<Report> {
    let id = &arguments.assessment;
    let from = arguments.from.display();
    let text = fs::read_to_string(&arguments.from)
        .map_err(|error| Failure::Refused(vec![format!("{id}: {from} cannot be read: {error}")]))?;
    let report = junit::parse(&text).map_err(|why| {
        Failure::Refused(vec![format!("{id}: {from} is not a JUnit report: {why}")])
    })?;
    let mut refused: Vec<String> = report
        .unplaced
        .iter()
        .map(|case| {
            format!(
                "{id}: {from} holds the test case {case} outside a <testsuite> directly under <testsuites>, where this program does not read it"
            )
        })
        .collect();
    let cases = |test: fn(&junit::Case) -> bool| {
        u64::try_from(report.cases.iter().filter(|case| test(case)).count()).unwrap_or(u64::MAX)
    };
    let plural = |count: u64| {
        if count == 1 {
            "test case"
        } else {
            "test cases"
        }
    };
    let counted = &report.counts;
    for (attribute, stated, held, with) in [
        ("tests", counted.tests, cases(|_| true), ""),
        (
            "failures",
            counted.failures,
            cases(|case| case.failure),
            " with <failure>",
        ),
        (
            "errors",
            counted.errors,
            cases(|case| case.error),
            " with <error>",
        ),
        (
            "skipped",
            counted.skipped,
            cases(|case| case.verdict == Verdict::Skipped),
            " with <skipped>",
        ),
    ] {
        if let Some(stated) = stated
            && stated != held
        {
            refused.push(format!(
                "{id}: {from}: its root says {attribute}=\"{stated}\", and it holds {held} {}{with}",
                plural(held)
            ));
        }
    }
    if refused.is_empty() {
        Ok(report)
    } else {
        Err(Failure::Refused(refused))
    }
}

/// The start and the end of the run a report covers.
fn report_times(report: &Report) -> Option<(i128, i128)> {
    let started = instant(report.timestamp.as_deref()?)?;
    let length = i128::try_from(scaled(report.time.as_deref()?, 9)?).ok()?;
    Some((started, started + length))
}

/// Of the cases, the first that took the longest.
fn slowest<'c>(cases: impl IntoIterator<Item = &'c junit::Case>) -> Option<&'c junit::Case> {
    cases.into_iter().fold(
        None,
        |slowest: Option<&'c junit::Case>, case| match slowest {
            Some(slowest) if slowest.nanos >= case.nanos => Some(slowest),
            _ => Some(case),
        },
    )
}

fn no_reading(plan: &Plan, name: &str) -> Failure {
    Failure::Broken(format!(
        "{}/{name}: this program has no reading of that output for {}",
        plan.id, plan.assessor
    ))
}

/// `nextest-run@1`: the coverage of the run, its slowest test and its time.
fn nextest_run(plan: &Plan, report: &Report) -> Outcome<Read> {
    let mut counts = Counts::default();
    for case in &report.cases {
        counts.add(case.verdict);
    }
    let Some(slowest) = slowest(
        report
            .cases
            .iter()
            .filter(|case| case.verdict != Verdict::Skipped),
    ) else {
        return Err(Failure::Refused(vec![format!(
            "{}: the report holds no test that ran",
            plan.id
        )]));
    };
    let mut results = Vec::new();
    let mut refused = Vec::new();
    for (name, measurement) in plan.measurements() {
        let mut result = plan.result(measurement, None)?;
        match (result.output.as_str(), result.boundary.as_deref()) {
            ("slowest_test", None) => result.value = Some(slowest.time.clone()),
            ("duration", Some("run_time")) => match &report.time {
                Some(time) => result.value = Some(time.clone()),
                None => {
                    refused.push(format!(
                        "{}/{name}: the report holds no time of the run",
                        plan.id
                    ));
                    continue;
                }
            },
            _ => return Err(no_reading(plan, &name)),
        }
        results.push(result);
    }
    if !refused.is_empty() {
        return Err(Failure::Refused(refused));
    }
    Ok(Read {
        counts,
        results,
        times: report_times(report),
    })
}

/// A list of text the plan gives as the input `name`.
fn list_input(plan: &Plan, name: &str) -> Outcome<Vec<String>> {
    plan.input(name)
        .and_then(Yaml::as_sequence)
        .and_then(|items| {
            items
                .iter()
                .map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .ok_or_else(|| {
            Failure::Broken(format!(
                "{}: the plan gives no list of text as the input {name}",
                plan.id
            ))
        })
}

/// `package-tests@1`: per package, the tests that ran, those that passed, and the slowest.
fn package_tests(plan: &Plan, report: &Report) -> Outcome<Read> {
    let packages = list_input(plan, "packages")?;
    let mut counts = Counts::default();
    let mut results = Vec::new();
    let mut refused = Vec::new();
    for package in &packages {
        let ran: Vec<&junit::Case> = report
            .cases
            .iter()
            .filter(|case| case.package() == package && case.verdict != Verdict::Skipped)
            .collect();
        let Some(slowest) = slowest(ran.iter().copied()) else {
            refused.push(format!(
                "{}: the report holds no test of the package {package} that ran",
                plan.id
            ));
            continue;
        };
        for case in &ran {
            counts.add(case.verdict);
        }
        let passed = ran
            .iter()
            .filter(|case| case.verdict == Verdict::Passed)
            .count();
        for (name, measurement) in plan.measurements() {
            let mut result = plan.result(measurement, Some(package))?;
            match result.output.as_str() {
                "tests_run" => result.value = Some(ran.len().to_string()),
                "tests_passed" => result.value = Some(passed.to_string()),
                "slowest_test" => {
                    result.value = Some(slowest.time.clone());
                    result.details = Some(Details {
                        test: format!("{} {}", slowest.classname, slowest.name),
                    });
                }
                _ => return Err(no_reading(plan, &name)),
            }
            results.push(result);
        }
    }
    if !refused.is_empty() {
        return Err(Failure::Refused(refused));
    }
    Ok(Read {
        counts,
        results,
        times: report_times(report),
    })
}

/// The case of the named test, or the line that refuses the report.
fn named_case<'r>(
    plan: &Plan,
    report: &'r Report,
    package: &str,
    test: &str,
) -> Outcome<Result<&'r junit::Case, String>> {
    let id = &plan.id;
    Ok(match junit::named(report, package, test) {
        Lookup::Found(case) if case.verdict == Verdict::Skipped => Err(format!(
            "{id}: the report holds the test {test} of the package {package} as skipped"
        )),
        Lookup::Found(case) => Ok(case),
        Lookup::Absent => Err(format!(
            "{id}: the report holds no test {test} of the package {package}"
        )),
        Lookup::Many(cases) => Err(format!(
            "{id}: the report holds {} tests the name {test} of the package {package} can name: {}",
            cases.len(),
            cases
                .iter()
                .map(|case| format!("{} {}", case.classname, case.name))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        Lookup::Unplaced => {
            return Err(Failure::Broken(format!(
                "{id}: the test {test} names no file under src/ or tests/"
            )));
        }
    })
}

/// `named-tests@1`: whether each named test passed.
fn named_tests(plan: &Plan, report: &Report) -> Outcome<Read> {
    let package = plan
        .input("package")
        .and_then(Yaml::as_str)
        .ok_or_else(|| Failure::Broken(format!("{}: the plan gives no package", plan.id)))?
        .to_owned();
    let tests = list_input(plan, "tests")?;
    let mut counts = Counts::default();
    let mut results = Vec::new();
    let mut refused = Vec::new();
    for test in &tests {
        let case = match named_case(plan, report, &package, test)? {
            Ok(case) => case,
            Err(line) => {
                refused.push(line);
                continue;
            }
        };
        counts.add(case.verdict);
        for (name, measurement) in plan.measurements() {
            let mut result = plan.result(measurement, Some(test))?;
            if result.output != "passed" {
                return Err(no_reading(plan, &name));
            }
            result.holds = Some(case.verdict == Verdict::Passed);
            results.push(result);
        }
    }
    if !refused.is_empty() {
        return Err(Failure::Refused(refused));
    }
    Ok(Read {
        counts,
        results,
        times: report_times(report),
    })
}

/// The function that checks a capability record against its adapter, in the crate's
/// `tests/stt_record.rs`.
const RECORD_TEST: &str = "tests/stt_record.rs::the_resource_profile_is_a_measured_cpu_profile";

/// `capability-record@1`: the value the record states, and whether its test passed.
fn capability_record(arguments: &Arguments, plan: &Plan, target: &Resolved) -> Outcome<Read> {
    let id = &plan.id;
    let Some((path, bytes)) = &target.document else {
        return Err(Failure::Broken(format!(
            "{id}: its target is not a document"
        )));
    };
    let Some((directory, _)) = path.split_once("/records/") else {
        return Err(Failure::Broken(format!(
            "{id}: {path} is not under the records/ of a crate"
        )));
    };
    let manifest_path = format!("{directory}/Cargo.toml");
    let manifest = fs::read_to_string(arguments.root.join(&manifest_path)).map_err(|error| {
        Failure::Refused(vec![format!(
            "{id}: {manifest_path} cannot be read: {error}"
        )])
    })?;
    let manifest: serde_json::Value = toml::from_str(&manifest).map_err(|error| {
        Failure::Refused(vec![format!(
            "{id}: {manifest_path} does not parse: {error}"
        )])
    })?;
    let package = manifest
        .pointer("/package/name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Failure::Refused(vec![format!("{id}: {manifest_path} names no package")]))?;
    let mut refused = Vec::new();
    let record: Option<serde_json::Value> = match serde_json::from_slice(bytes) {
        Ok(record) => Some(record),
        Err(error) => {
            refused.push(format!("{id}: {path} is not JSON: {error}"));
            None
        }
    };
    let report = report(arguments)?;
    let test = format!("{directory}/{RECORD_TEST}");
    let case = match named_case(plan, &report, package, &test)? {
        Ok(case) if case.verdict == Verdict::Failed => {
            refused.push(format!(
                "{id}: the report holds the test {test} of the package {package} as failed"
            ));
            None
        }
        Ok(case) => Some(case),
        Err(line) => {
            refused.push(line);
            None
        }
    };
    let mut results = Vec::new();
    for (name, measurement) in plan.measurements() {
        let mut result = plan.result(measurement, None)?;
        if result.output != "cpu_real_time_factor" {
            return Err(no_reading(plan, &name));
        }
        let field = "resource_profile.cpu_real_time_factor_milli";
        match record.as_ref().map(|record| {
            record
                .pointer("/resource_profile/cpu_real_time_factor_milli")
                .and_then(serde_json::Value::as_u64)
        }) {
            Some(Some(value)) => result.value = Some(value.to_string()),
            Some(None) => refused.push(format!("{id}: {path} holds no whole number at {field}")),
            None => {}
        }
        results.push(result);
    }
    if !refused.is_empty() {
        return Err(Failure::Refused(refused));
    }
    let mut counts = Counts::default();
    if let Some(case) = case {
        counts.add(case.verdict);
    }
    Ok(Read {
        counts,
        results,
        times: report_times(&report),
    })
}

/// `printed-duration@1`: one value per measure from the output a command printed.
fn printed_duration(arguments: &Arguments, plan: &Plan, timed: bool) -> Outcome<Read> {
    let id = &plan.id;
    let mut labels = Vec::new();
    for (name, measurement) in plan.measurements() {
        let label = match (
            measurement.get("output").and_then(Yaml::as_str),
            measurement.get("scope").and_then(Yaml::as_str),
            measurement.get("unit").and_then(Yaml::as_str),
            measurement.get("boundary").and_then(Yaml::as_str),
        ) {
            (Some("duration"), Some("run"), Some("ms"), Some(boundary)) => {
                printed::label(id, boundary)
            }
            _ => None,
        };
        let Some((line, after)) = label else {
            return Err(Failure::Call(format!(
                "{id}/{name}: the program reads no single printed line of this measure"
            )));
        };
        labels.push((name, measurement, line, after));
    }
    if !timed {
        return Err(Failure::Call(format!(
            "{id}: the output of a command holds no time; give --started-at and --ended-at"
        )));
    }
    let example = plan
        .assessment
        .get("observation")
        .and_then(|observation| observation.get("command"))
        .and_then(Yaml::as_str)
        .is_some_and(|command| command.split_whitespace().any(|word| word == "--example"));
    if example && arguments.exit_status.is_none() {
        return Err(Failure::Call(format!(
            "{id}: the output of an example holds no verdict; give --exit-status, the exit status of its command"
        )));
    }
    let bytes = fs::read(&arguments.from).map_err(|error| {
        Failure::Refused(vec![format!(
            "{id}: {} cannot be read: {error}",
            arguments.from.display()
        )])
    })?;
    let output = String::from_utf8_lossy(&bytes);
    let mut refused: Vec<String> = printed::skipped(&output)
        .into_iter()
        .map(|line| format!("{id}: the output says a test skipped: `{line}`"))
        .collect();
    if let Some(status) = arguments.exit_status
        && status != 0
    {
        refused.push(format!("{id}: its command exited {status}"));
    }
    if !example {
        match printed::verdict(&output) {
            printed::Ran::Failed(lines) => {
                refused.extend(lines.into_iter().map(|line| format!("{id}: {line}")));
            }
            printed::Ran::Passed => {}
            printed::Ran::Unknown => {
                if arguments.exit_status.is_none() {
                    refused.push(format!(
                        "{id}: the output holds no `test result:` line and no line of cargo-nextest that says whether a test passed; give --exit-status"
                    ));
                }
            }
        }
    }
    let mut results = Vec::new();
    for (name, measurement, line, after) in labels {
        match printed::value(&output, line, after) {
            Ok(value) => {
                let mut result = plan.result(measurement, None)?;
                result.value = Some(value);
                results.push(result);
            }
            Err(why) => refused.push(format!("{id}/{name}: {why}")),
        }
    }
    if !refused.is_empty() {
        return Err(Failure::Refused(refused));
    }
    Ok(Read {
        counts: Counts {
            executed: 1,
            passed: 1,
            ..Counts::default()
        },
        results,
        times: None,
    })
}

/// `number`, a decimal without a sign, times ten to the `scale`, if it is whole then.
fn scaled(number: &str, scale: u32) -> Option<u128> {
    let (whole, fraction) = match number.split_once('.') {
        Some((whole, fraction)) if !fraction.is_empty() => (whole, fraction),
        Some(_) => return None,
        None => (number, ""),
    };
    let digits = |text: &str| text.bytes().all(|byte| byte.is_ascii_digit());
    if whole.is_empty() || !digits(whole) || !digits(fraction) || fraction.len() > scale as usize {
        return None;
    }
    let padding = scale - u32::try_from(fraction.len()).ok()?;
    let whole: u128 = whole.parse().ok()?;
    let fraction: u128 = if fraction.is_empty() {
        0
    } else {
        fraction.parse().ok()?
    };
    whole
        .checked_mul(10u128.checked_pow(scale)?)?
        .checked_add(fraction.checked_mul(10u128.checked_pow(padding)?)?)
}

/// The instant of an RFC 3339 time, `2026-11-24T21:02:05.400+01:00`, in nanoseconds since 1970.
fn instant(text: &str) -> Option<i128> {
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = text.get(range)?;
        part.bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    let bytes = text.as_bytes();
    if bytes.len() < 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b't')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if hour > 23 || minute > 59 || second > 59 || !(1..=12).contains(&month) {
        return None;
    }
    let days = days_from_civil(year, month, day);
    if civil_from_days(days) != (year, month, day) {
        return None;
    }
    let mut rest = &text[19..];
    let mut nanos: i128 = 0;
    if let Some(fraction) = rest.strip_prefix('.') {
        let end = fraction
            .find(|character: char| !character.is_ascii_digit())
            .unwrap_or(fraction.len());
        if end == 0 || end > 9 {
            return None;
        }
        nanos = i128::try_from(scaled(&format!("0.{}", &fraction[..end]), 9)?).ok()?;
        rest = &fraction[end..];
    }
    let offset = match rest {
        "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let zone = rest.get(1..)?;
            let hours: i64 = zone.get(0..2)?.parse().ok()?;
            let minutes: i64 = zone.get(3..5)?.parse().ok()?;
            if zone.len() != 5 || zone.as_bytes()[2] != b':' || hours > 23 || minutes > 59 {
                return None;
            }
            sign * (hours * 3600 + minutes * 60)
        }
    };
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second - offset;
    Some(i128::from(seconds) * 1_000_000_000 + nanos)
}

/// `nanos` since 1970 as a UTC time to the second, `2026-11-24T20:02:05Z`.
fn utc(nanos: i128) -> String {
    let seconds = i64::try_from(nanos.div_euclid(1_000_000_000)).unwrap_or_default();
    let (days, second) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        second / 3600,
        second % 3600 / 60,
        second % 60
    )
}

/// A time given in the form the document writes, in nanoseconds since 1970.
fn exact_utc(text: &str) -> Option<i128> {
    let nanos = instant(text)?;
    (utc(nanos) == text).then_some(nanos)
}

/// Days since 1970-01-01 of a date of the proleptic Gregorian calendar.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The date of a number of days since 1970-01-01.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted + 2) / 5 + 1;
    let month = if shifted < 10 {
        shifted + 3
    } else {
        shifted - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// A map written in the order of its entries.
struct Ordered<T>(Vec<(String, T)>);

impl<T> Ordered<T> {
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Sets `key` to `value`, in the place of an earlier value.
    fn put(&mut self, key: &str, value: T) {
        match self.0.iter_mut().find(|(at, _)| at == key) {
            Some((_, old)) => *old = value,
            None => self.0.push((key.to_owned(), value)),
        }
    }
}

impl<T: Serialize> Serialize for Ordered<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, value) in &self.0 {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

#[derive(Serialize)]
struct Document {
    format: &'static str,
    evidence: Evidence,
    #[serde(skip_serializing_if = "Option::is_none")]
    run: Option<RunTimes>,
}

#[derive(Serialize)]
struct Evidence {
    project: String,
    assessment: String,
    assessor: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    candidate: Option<String>,
    target: Target,
    #[serde(skip_serializing_if = "Ordered::is_empty")]
    inputs: Ordered<InputValue>,
    completion: &'static str,
    conditions: Conditions,
    coverage: Coverage,
    results: Vec<MeasuredResult>,
    source: Source,
}

#[derive(Serialize)]
struct Target {
    kind: String,
    class: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    selection: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    identifiers: Option<Identifiers>,
}

#[derive(Serialize)]
struct Identifiers {
    path: String,
    digest: String,
    digest_form: &'static str,
}

#[derive(Serialize)]
struct InputValue {
    value: serde_json::Value,
}

#[derive(Serialize)]
struct Conditions {
    #[serde(skip_serializing_if = "Option::is_none")]
    runner: Option<String>,
    #[serde(skip_serializing_if = "Ordered::is_empty")]
    tools: Ordered<String>,
    #[serde(skip_serializing_if = "Ordered::is_empty")]
    other: Ordered<String>,
}

#[derive(Serialize)]
struct Coverage {
    unit: String,
    executed: u64,
    skipped: u64,
    not_run: u64,
    unsupported: u64,
    passed: u64,
    failed: u64,
}

#[derive(Serialize)]
struct MeasuredResult {
    output: String,
    kind: String,
    scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    item: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    boundary: Option<String>,
    value_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    samples: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    holds: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<Details>,
}

#[derive(Serialize)]
struct Details {
    test: String,
}

#[derive(Serialize)]
struct Source {
    kind: &'static str,
    location: String,
}

#[derive(Serialize)]
struct RunTimes {
    started_at: String,
    ended_at: String,
}
