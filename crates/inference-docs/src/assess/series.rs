//! `series-append` and `history`: the series of quality records, the records of each check run on
//! `main` kept once each on a ref only the workflow `quality-series` writes. Both work on a
//! directory, a checkout of that ref; neither fetches nor pushes.
//!
//! ```sh
//! inference-docs series-append --from <artifact dir> --series <checkout> --root .
//! inference-docs history --series <checkout> --root . <assessment> [<measurement>]
//! ```
//!
//! `series-append` reads the directory the artifact `quality-records-<commit>` was downloaded to,
//! `<plan>/<assessment>.evidence.json` for lines of `inference-docs junit-assessments`, and writes
//! each record to `<plan>/<assessment>/<SHA-256 of its bytes>.evidence.json` in the series, byte
//! for byte, unless that file holds those bytes already. It never changes or removes a file of
//! the series. It refuses, writes nothing and names the file and the reason on stderr for each of
//! these: a file or directory of the artifact that is no record of a listed assessment at its
//! place, a link, a record that is not JSON, of another format than `ratchet-evidence/1`, of
//! another project than the plan files or of another assessment than its name; a file of the
//! series at the path of a record that holds other bytes, or a link or directory there. Exit 0
//! with a line per record, `added` or `in the series already`; 1 with the refusals; 2 for a wrong
//! call; 3 when a plan file cannot be read or a record cannot be written, after the records this
//! call wrote are removed again.
//!
//! `history` prints one line per record of the assessment in the series, oldest run first: the
//! id of the check run its source names, the revision of its target and the value of each
//! measurement, or of the one given, as `<measurement>=<value> <unit>`, with `[<item>]` after
//! the name for a measurement with one value per item. Runs are ordered by their id, which the
//! forge counts up; a record whose source names no forge run comes last. Exit 0, also with no
//! record (a line on stderr says so); 2 for an assessment or measurement the plan files do not
//! declare or a series that is no directory; 3 when a plan file or a record cannot be read.

use std::{
    collections::BTreeSet,
    fs,
    io::{ErrorKind, Write as _},
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::Args;
use serde_json::Value;
use serde_norway::Value as Yaml;

use super::{
    Failure, Outcome,
    adopt::{artifact_files, project},
    junit_assessments, read_text, sha256, strings,
};

/// The format of a record.
const EVIDENCE_FORMAT: &str = "ratchet-evidence/1";
/// What the name of a record ends with.
const SUFFIX: &str = ".evidence.json";
/// What the location of a forge run holds before its id.
const RUNS: &str = "/actions/runs/";

/// The call of `series-append`.
#[derive(Args)]
pub(crate) struct AppendArguments {
    /// The directory the artifact `quality-records-<commit>` of a check run was downloaded to.
    #[arg(long)]
    from: PathBuf,
    /// The checkout of the series the records are appended to.
    #[arg(long)]
    series: PathBuf,
    /// The root of the tree whose plan files name the assessments.
    #[arg(long, default_value = ".")]
    root: PathBuf,
}

/// The call of `history`.
#[derive(Args)]
pub(crate) struct HistoryArguments {
    /// The checkout of the series.
    #[arg(long)]
    series: PathBuf,
    /// The root of the tree whose plan files declare the assessment.
    #[arg(long, default_value = ".")]
    root: PathBuf,
    /// The key of the assessment under `assessments`.
    assessment: String,
    /// One measurement of it; every measurement when not given.
    measurement: Option<String>,
}

/// Runs a call: its lines on stdout, or the lines on stderr and the exit code of the failure.
fn finish(command: &str, outcome: Outcome<Vec<String>>) -> ExitCode {
    let (code, lines) = match outcome {
        Ok(lines) => {
            for line in lines {
                println!("{line}");
            }
            return ExitCode::SUCCESS;
        }
        Err(Failure::Refused(lines)) => (1, lines),
        Err(Failure::Call(line)) => (2, vec![line]),
        Err(Failure::Broken(line)) => (3, vec![line]),
    };
    for line in lines {
        eprintln!("inference-docs {command}: {line}");
    }
    ExitCode::from(code)
}

pub(crate) fn run_append(arguments: &AppendArguments) -> ExitCode {
    finish("series-append", append(arguments))
}

pub(crate) fn run_history(arguments: &HistoryArguments) -> ExitCode {
    finish("history", history(arguments))
}

/// Whether `path` is a directory itself, not a link to one.
fn is_directory(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir())
}

/// One record of the artifact to keep: its path in the series and its bytes.
struct Planned {
    path: String,
    bytes: Vec<u8>,
}

fn append(arguments: &AppendArguments) -> Outcome<Vec<String>> {
    for (flag, path) in [("--from", &arguments.from), ("--series", &arguments.series)] {
        if !is_directory(path) {
            return Err(Failure::Call(format!(
                "{flag} {} is no directory",
                path.display()
            )));
        }
    }
    let root = &arguments.root;
    let listed: BTreeSet<(String, String)> = junit_assessments(root)
        .map_err(Failure::Broken)?
        .iter()
        .filter_map(|line| {
            let mut words = line.split(' ');
            Some((words.next()?.to_owned(), words.next()?.to_owned()))
        })
        .collect();
    let plans: BTreeSet<&str> = listed.iter().map(|(plan, _)| plan.as_str()).collect();
    let project = project(root)?;
    let mut refused = Vec::new();
    let mut planned = Vec::new();
    for (path, is_file) in artifact_files(&arguments.from)? {
        let at = arguments.from.join(&path);
        if plans.contains(path.as_str()) && is_directory(&at) {
            continue;
        }
        let at = at.display();
        let place = path
            .split_once('/')
            .and_then(|(plan, file)| Some((plan, file.strip_suffix(SUFFIX)?)))
            .filter(|(plan, assessment)| {
                listed.contains(&((*plan).to_owned(), (*assessment).to_owned()))
            });
        let Some((plan, assessment)) = place.filter(|_| is_file) else {
            refused.push(format!(
                "{at}: is no record of an assessment `inference-docs junit-assessments` lists, at <plan>/<assessment>{SUFFIX}"
            ));
            continue;
        };
        let bytes = match fs::read(arguments.from.join(&path)) {
            Ok(bytes) => bytes,
            Err(error) => {
                refused.push(format!("{at}: cannot be read: {error}"));
                continue;
            }
        };
        let record: Value = match serde_json::from_slice(&bytes) {
            Ok(record) => record,
            Err(error) => {
                refused.push(format!("{at}: is not valid JSON: {error}"));
                continue;
            }
        };
        for (pointer, wanted) in [
            ("/format", EVIDENCE_FORMAT),
            ("/evidence/project", project.as_str()),
            ("/evidence/assessment", assessment),
        ] {
            let found = record.pointer(pointer).and_then(Value::as_str);
            if found != Some(wanted) {
                refused.push(format!("{at}: {pointer} is {found:?}, not {wanted:?}"));
            }
        }
        planned.push(Planned {
            path: format!("{plan}/{assessment}/{}{SUFFIX}", sha256(&bytes)),
            bytes,
        });
    }
    let mut lines = Vec::new();
    let mut new = Vec::new();
    for record in planned {
        let at = arguments.series.join(&record.path);
        let shown = at.display();
        match fs::symlink_metadata(&at) {
            Ok(meta) if meta.is_file() => match fs::read(&at) {
                Ok(bytes) if bytes == record.bytes => {
                    lines.push(format!("{}: in the series already", record.path));
                }
                Ok(_) => refused.push(format!(
                    "{shown}: holds other bytes than the record whose digest names it; the series is not changed"
                )),
                Err(error) => refused.push(format!("{shown}: cannot be read: {error}")),
            },
            Ok(_) => refused.push(format!(
                "{shown}: is no file; a record of the series is a file"
            )),
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let mut directory = arguments.series.clone();
                for part in record.path.split('/').take(2) {
                    directory.push(part);
                    if fs::symlink_metadata(&directory).is_ok() && !is_directory(&directory) {
                        refused.push(format!(
                            "{}: is no directory; the records of the series stand in directories",
                            directory.display()
                        ));
                    }
                }
                new.push(record);
            }
            Err(error) => refused.push(format!("{shown}: cannot be read: {error}")),
        }
    }
    if !refused.is_empty() {
        refused.dedup();
        return Err(Failure::Refused(refused));
    }
    write_new(&arguments.series, &new)?;
    lines.extend(new.iter().map(|record| format!("{}: added", record.path)));
    Ok(lines)
}

/// Creates each new record; on a failure removes the ones this call created and fails.
fn write_new(series: &Path, new: &[Planned]) -> Outcome<()> {
    let mut created: Vec<PathBuf> = Vec::new();
    for record in new {
        let at = series.join(&record.path);
        let written = at
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| {
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&at)?;
                created.push(at.clone());
                file.write_all(&record.bytes)?;
                file.sync_all()
            });
        if let Err(error) = written {
            for path in &created {
                let _ = fs::remove_file(path);
            }
            return Err(Failure::Broken(format!(
                "{}: cannot be written: {error}; the records this call wrote are removed again",
                at.display()
            )));
        }
    }
    Ok(())
}

/// The plan file of the assessment `id`, by the name of its file without `.ratchet.yaml`, and
/// the assessment.
fn declared(root: &Path, id: &str) -> Outcome<(String, Yaml)> {
    let config: Value = toml::from_str(&read_text(root, "ratchet.toml")?)
        .map_err(|error| Failure::Broken(format!("ratchet.toml: does not parse: {error}")))?;
    let paths = strings(config.pointer("/spec/paths")).ok_or_else(|| {
        Failure::Broken("ratchet.toml: names no plan file under [spec] paths".to_owned())
    })?;
    for path in &paths {
        let plan: Yaml = serde_norway::from_str(&read_text(root, path)?)
            .map_err(|error| Failure::Broken(format!("{path}: does not parse: {error}")))?;
        if let Some(assessment) = plan.get("assessments").and_then(|all| all.get(id)) {
            let name = path.rsplit('/').next().unwrap_or(path);
            let name = name.strip_suffix(".ratchet.yaml").ok_or_else(|| {
                Failure::Broken(format!("{path}: a plan file is named <plan>.ratchet.yaml"))
            })?;
            return Ok((name.to_owned(), assessment.clone()));
        }
    }
    Err(Failure::Call(format!(
        "no plan file of ratchet.toml declares the assessment {id}"
    )))
}

/// One measurement of the plan: its name, its output and its boundary.
struct Measurement {
    name: String,
    output: String,
    boundary: Option<String>,
}

/// One record of the series, with what orders it.
struct Entry {
    run: Option<u64>,
    started_at: String,
    file: String,
    record: Value,
}

fn history(arguments: &HistoryArguments) -> Outcome<Vec<String>> {
    if !is_directory(&arguments.series) {
        return Err(Failure::Call(format!(
            "--series {} is no directory",
            arguments.series.display()
        )));
    }
    let id = arguments.assessment.as_str();
    let (plan, assessment) = declared(&arguments.root, id)?;
    let text = |value: &Yaml, key: &str| value.get(key).and_then(Yaml::as_str).map(str::to_owned);
    let all: Vec<Measurement> = assessment
        .get("measurements")
        .and_then(Yaml::as_mapping)
        .into_iter()
        .flatten()
        .filter_map(|(name, measurement)| {
            Some(Measurement {
                name: name.as_str()?.to_owned(),
                output: text(measurement, "output")?,
                boundary: text(measurement, "boundary"),
            })
        })
        .collect();
    let names: Vec<&str> = all.iter().map(|one| one.name.as_str()).collect();
    let chosen: Vec<&Measurement> = match &arguments.measurement {
        None => all.iter().collect(),
        Some(name) => {
            let found: Vec<&Measurement> = all.iter().filter(|one| one.name == *name).collect();
            if found.is_empty() {
                return Err(Failure::Call(format!(
                    "the assessment {id} declares no measurement {name}; it declares {}",
                    names.join(", ")
                )));
            }
            found
        }
    };
    let directory = arguments.series.join(&plan).join(id);
    let mut entries = Vec::new();
    if is_directory(&directory) {
        let listing = fs::read_dir(&directory).map_err(|error| {
            Failure::Broken(format!("{}: cannot be read: {error}", directory.display()))
        })?;
        for item in listing {
            let item = item.map_err(|error| {
                Failure::Broken(format!("{}: cannot be read: {error}", directory.display()))
            })?;
            let file = item.file_name().to_string_lossy().into_owned();
            let path = item.path();
            if !file.ends_with(SUFFIX) || !fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
                continue;
            }
            let record: Value = fs::read(&path)
                .map_err(|error| error.to_string())
                .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()))
                .map_err(|error| {
                    Failure::Broken(format!("{}: cannot be read: {error}", path.display()))
                })?;
            if record
                .pointer("/evidence/assessment")
                .and_then(Value::as_str)
                != Some(id)
            {
                return Err(Failure::Broken(format!(
                    "{}: is no record of {id}",
                    path.display()
                )));
            }
            let run = record
                .pointer("/evidence/source/location")
                .and_then(Value::as_str)
                .and_then(|location| location.trim_end_matches('/').rsplit_once(RUNS))
                .and_then(|(_, id)| id.parse::<u64>().ok());
            let started_at = record
                .pointer("/run/started_at")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            entries.push(Entry {
                run,
                started_at,
                file,
                record,
            });
        }
    }
    if entries.is_empty() {
        eprintln!(
            "inference-docs history: the series {} holds no record of {id}",
            arguments.series.display()
        );
        return Ok(Vec::new());
    }
    entries.sort_by(|a, b| {
        (a.run.is_none(), a.run, &a.started_at, &a.file).cmp(&(
            b.run.is_none(),
            b.run,
            &b.started_at,
            &b.file,
        ))
    });
    Ok(entries.iter().map(|entry| line(entry, &chosen)).collect())
}

/// The line of one record: its run, its revision and the values of the measurements.
fn line(entry: &Entry, measurements: &[&Measurement]) -> String {
    let record = &entry.record;
    let at = |pointer: &str| record.pointer(pointer).and_then(Value::as_str);
    let run = entry.run.map_or_else(
        || at("/evidence/source/location").unwrap_or("-").to_owned(),
        |run| run.to_string(),
    );
    let revision = at("/evidence/target/revision").unwrap_or("-");
    let results: Vec<&Value> = record
        .pointer("/evidence/results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    let mut values = Vec::new();
    for measurement in measurements {
        let of: Vec<&&Value> = results
            .iter()
            .filter(|result| {
                result.get("output").and_then(Value::as_str) == Some(measurement.output.as_str())
            })
            .filter(|result| {
                measurement.boundary.as_deref().is_none_or(|boundary| {
                    result.get("boundary").and_then(Value::as_str) == Some(boundary)
                })
            })
            .collect();
        if of.is_empty() {
            values.push(format!("{}=none", measurement.name));
        }
        for result in of {
            let mut value = measurement.name.clone();
            if let Some(item) = result.get("item").and_then(Value::as_str) {
                value.push_str(&format!("[{item}]"));
            }
            let stated = match (
                result.get("holds").and_then(Value::as_bool),
                result.get("value"),
            ) {
                (Some(true), _) => "holds".to_owned(),
                (Some(false), _) => "fails".to_owned(),
                (None, Some(Value::String(text))) => text.clone(),
                (None, Some(Value::Null) | None) => "none".to_owned(),
                (None, Some(other)) => other.to_string(),
            };
            value.push('=');
            value.push_str(&stated);
            if let Some(unit) = result.get("unit").and_then(Value::as_str) {
                value.push(' ');
                value.push_str(unit);
            }
            values.push(value);
        }
    }
    format!("{run} {revision} {}", values.join(", "))
}
