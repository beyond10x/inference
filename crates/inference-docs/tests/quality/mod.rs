//! The quality documents of the repository (`ratchet.toml`, `quality/`) agree with each other,
//! with `Taskfile.yml`, with the tree and with the pages that describe them, `quality/README.md`
//! and `quality/adapters.md`.
//!
//! This is not `ratchet validate`: no Ratchet command works at the commit the documents are
//! written against (the page names it). It holds what would make the documents or the pages
//! untrue without anyone noticing, and `quality/README.md` lists it under "The test that keeps
//! the documents true": that list and `problems` say the same.
//!
//! A page is read as it is written for a person. An entry is the text under a heading
//! `### \`<assessment>/<measurement>\``, up to the next heading; its lines are the bullets
//! `- <name>: <text>`, and what else stands under the heading is kept whole; a run is a
//! row of the table whose last cell links a run by its number; a table inside an entry holds one
//! row per scenario and one column per viewport. A section is the text under a heading
//! `## \`<name>\``: `adapters.rs` holds the sections of `quality/adapters.md` against the
//! directories under `adapters/`.

mod adapters;
mod adopt;
mod adversary_q1;
mod adversary_q2;
mod assess;
mod records;
mod series;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::root;

const SPEC_FORMAT: &str = "ratchet/1";
const ASSESSOR_FORMAT: &str = "ratchet-assessor/1";
const EVIDENCE_FORMAT: &str = "ratchet-evidence/1";
const ENTITY_MAPS: [&str; 4] = ["subjects", "assessments", "requirements", "gates"];
const PAGE: &str = "quality/README.md";
const ADAPTERS_PAGE: &str = "quality/adapters.md";
const NOT_MEASURED: &str = "quality/not-measured.md";
/// The counts of a coverage. An assessor declares the ones it reports; the others are 0 in its
/// records (`spec/domains/assessor.yaml` of the pinned commit, the field `coverage`).
const COVERAGE_COUNTS: [&str; 6] = [
    "executed",
    "skipped",
    "not_run",
    "unsupported",
    "passed",
    "failed",
];
/// The whole baseline of a measure that no run has produced a value for.
const UNOBSERVED: &str = "none. No run of this command is kept.";

/// The files the check reads, each as its path from the repository root and its text.
#[derive(Clone)]
struct Documents {
    config: String,
    taskfile: String,
    readme: String,
    /// `quality/adapters.md`.
    adapters_page: String,
    /// `quality/not-measured.md`.
    not_measured: String,
    specifications: BTreeMap<String, String>,
    assessors: BTreeMap<String, String>,
    records: BTreeMap<String, String>,
    /// The directories under `adapters/` that hold a `Cargo.toml`, by name.
    adapter_crates: BTreeSet<String>,
    /// The files under `adapters/*/records/` and `adapters/*/capability/`, by path.
    adapter_records: BTreeSet<String>,
    /// Files of the tree by path, in place of what the tree holds: `None` is a file that is not
    /// there. A test plants a defect of the tree here.
    replaced: BTreeMap<String, Option<String>>,
}

impl Documents {
    /// The text of a file of the tree by its path from the root, or `None` when the path leaves
    /// the tree or names no file.
    fn file(&self, path: &str) -> Option<String> {
        if path.starts_with('/') || path.contains("..") {
            return None;
        }
        match self.replaced.get(path) {
            Some(replaced) => replaced.clone(),
            None => fs::read_to_string(root().join(path)).ok(),
        }
    }

    /// The pages and the list of what is not measured, each with its path.
    fn pages(&self) -> [(&'static str, &str); 3] {
        [
            (PAGE, &self.readme),
            (ADAPTERS_PAGE, &self.adapters_page),
            (NOT_MEASURED, &self.not_measured),
        ]
    }
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

/// The names in `directory`, sorted: the directories when `directories`, else the files.
fn names(directory: &str, directories: bool) -> Vec<String> {
    let Ok(entries) = fs::read_dir(root().join(directory)) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir() == directories)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort_unstable();
    names
}

/// The files of `directory` whose names end in `suffix`, keyed by their path from the root.
fn files(directory: &str, suffix: &str) -> BTreeMap<String, String> {
    let Ok(entries) = fs::read_dir(root().join(directory)) else {
        return BTreeMap::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(suffix))
        .map(|name| {
            let path = format!("{directory}/{name}");
            let text = read(&root().join(&path));
            (path, text)
        })
        .collect()
}

fn load() -> Documents {
    let adapter_crates: BTreeSet<String> = names("adapters", true)
        .into_iter()
        .filter(|name| root().join(format!("adapters/{name}/Cargo.toml")).is_file())
        .collect();
    let adapter_records = adapter_crates
        .iter()
        .flat_map(|name| ["records", "capability"].map(|kept| format!("adapters/{name}/{kept}")))
        .flat_map(|directory| {
            names(&directory, false)
                .into_iter()
                .map(move |file| format!("{directory}/{file}"))
        })
        .collect();
    Documents {
        config: read(&root().join("ratchet.toml")),
        taskfile: read(&root().join("Taskfile.yml")),
        readme: read(&root().join(PAGE)),
        adapters_page: read(&root().join(ADAPTERS_PAGE)),
        not_measured: read(&root().join(NOT_MEASURED)),
        specifications: files("quality", ".ratchet.yaml"),
        assessors: files("quality/assessors", ".assessor.yaml"),
        records: files("quality/records", ".json"),
        adapter_crates,
        adapter_records,
        replaced: BTreeMap::new(),
    }
}

fn yaml(path: &str, text: &str, problems: &mut Vec<String>) -> Option<Value> {
    match serde_norway::from_str::<Value>(text) {
        Ok(value) if value.is_object() => Some(value),
        Ok(_) => {
            problems.push(format!("{path}: is not a mapping"));
            None
        }
        Err(error) => {
            problems.push(format!("{path}: does not parse: {error}"));
            None
        }
    }
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn entries<'a>(value: &'a Value, key: &str) -> impl Iterator<Item = (&'a String, &'a Value)> {
    value
        .get(key)
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
}

/// The specification of all `quality/*.ratchet.yaml` files as one: each entity map merged, and
/// the one `project` of the files.
fn specification(
    documents: &Documents,
    problems: &mut Vec<String>,
) -> (BTreeMap<&'static str, BTreeMap<String, Value>>, String) {
    let mut maps: BTreeMap<&str, BTreeMap<String, Value>> = BTreeMap::new();
    let mut projects = BTreeSet::new();
    if documents.specifications.is_empty() {
        problems.push("quality/: no *.ratchet.yaml file".to_owned());
    }
    for (path, source) in &documents.specifications {
        let Some(file) = yaml(path, source, problems) else {
            continue;
        };
        match text(&file, "format") {
            Some(SPEC_FORMAT) => {}
            other => problems.push(format!("{path}: format is {other:?}, not {SPEC_FORMAT:?}")),
        }
        match text(&file, "project") {
            Some(project) => {
                projects.insert(project.to_owned());
            }
            None => problems.push(format!("{path}: no project")),
        }
        for map in ENTITY_MAPS {
            for (id, entity) in entries(&file, map) {
                let merged = maps.entry(map).or_default();
                if merged.insert(id.clone(), entity.clone()).is_some() {
                    problems.push(format!("{path}: {map}.{id} is declared in two files"));
                }
            }
        }
    }
    if projects.len() > 1 {
        problems.push(format!(
            "the files name more than one project: {projects:?}"
        ));
    }
    (maps, projects.into_iter().next().unwrap_or_default())
}

/// The assessor declarations by `<name>@<version>`, each from the file of that name.
fn assessors(documents: &Documents, problems: &mut Vec<String>) -> BTreeMap<String, Value> {
    let mut declared = BTreeMap::new();
    for (path, source) in &documents.assessors {
        let Some(file) = yaml(path, source, problems) else {
            continue;
        };
        match text(&file, "format") {
            Some(ASSESSOR_FORMAT) => {}
            other => problems.push(format!(
                "{path}: format is {other:?}, not {ASSESSOR_FORMAT:?}"
            )),
        }
        let id = format!(
            "{}@{}",
            text(&file, "name").unwrap_or_default(),
            text(&file, "version").unwrap_or_default()
        );
        if !path.ends_with(&format!("/{id}.assessor.yaml")) {
            problems.push(format!("{path}: declares {id}, which is not its file name"));
        }
        declared.insert(id, file);
    }
    declared
}

/// The task names of `Taskfile.yml`, and every command that is one text in a `cmds` list of it.
fn tasks(
    documents: &Documents,
    problems: &mut Vec<String>,
) -> (BTreeSet<String>, BTreeSet<String>) {
    let Some(file) = yaml("Taskfile.yml", &documents.taskfile, problems) else {
        return (BTreeSet::new(), BTreeSet::new());
    };
    let names = entries(&file, "tasks")
        .map(|(name, _)| name.clone())
        .collect();
    let commands = entries(&file, "tasks")
        .filter_map(|(_, task)| task.get("cmds").and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    (names, commands)
}

fn check_config(documents: &Documents, problems: &mut Vec<String>) {
    let config = match toml::from_str::<Value>(&documents.config) {
        Ok(config) => config,
        Err(error) => {
            problems.push(format!("ratchet.toml: does not parse: {error}"));
            return;
        }
    };
    let listed: BTreeSet<&str> = config
        .pointer("/spec/paths")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let found: BTreeSet<&str> = documents
        .specifications
        .keys()
        .map(String::as_str)
        .collect();
    if listed != found {
        problems.push(format!(
            "ratchet.toml: [spec] paths is {listed:?}, the files are {found:?}"
        ));
    }
    let catalog = config
        .pointer("/catalog/sources")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|source| text(source, "path") == Some("quality/assessors"));
    if !catalog {
        problems.push("ratchet.toml: [catalog] sources does not name quality/assessors".to_owned());
    }
}

/// One entry of a page: its bullets `- <name>: <text>` by name, and the rows of its table.
#[derive(Default)]
struct Entry {
    /// The page the entry stands on.
    page: &'static str,
    lines: BTreeMap<String, String>,
    /// Every bullet of the entry, whole.
    bullets: Vec<String>,
    /// Every other line under the heading of the entry, up to the next heading: a bullet
    /// without a name, a line of a paragraph, a row of a table.
    loose: Vec<String>,
    table: Vec<Vec<String>>,
}

/// One section of a page, the text under a heading `## \`<name>\``.
#[derive(Default)]
struct Section {
    /// Every line of the section, its entries included, joined by one space.
    text: String,
    /// The rows of the tables that stand in the section and in no entry.
    rows: Vec<Vec<String>>,
}

/// What the check reads from the pages.
#[derive(Default)]
struct Page {
    /// By `<assessment>/<measurement>`.
    entries: BTreeMap<String, Entry>,
    /// The number of each run by its name in the table of runs, in lower case.
    runs: BTreeMap<String, String>,
    /// The bullets that are one code span and nothing else: the commands the check cannot hold.
    commands: BTreeSet<String>,
    /// The sections of `quality/adapters.md`, in the order of the page.
    sections: Vec<(String, Section)>,
}

impl Page {
    fn section(&self, name: &str) -> Option<&Section> {
        self.sections
            .iter()
            .find(|(section, _)| section == name)
            .map(|(_, section)| section)
    }

    fn bullet(&mut self, entry: Option<&str>, bullet: &str, problems: &mut Vec<String>) {
        if let Some(command) = bullet
            .strip_prefix('`')
            .and_then(|rest| rest.strip_suffix('`'))
            .filter(|inner| !inner.is_empty() && !inner.contains('`'))
        {
            self.commands.insert(command.to_owned());
        }
        let Some(key) = entry else {
            return;
        };
        let entry = self.entries.entry(key.to_owned()).or_default();
        entry.bullets.push(bullet.to_owned());
        let named = bullet
            .split_once(": ")
            .filter(|(name, _)| !name.contains('`'));
        let Some((name, rest)) = named else {
            entry.loose.push(bullet.to_owned());
            return;
        };
        if entry
            .lines
            .insert(name.to_owned(), rest.to_owned())
            .is_some()
        {
            problems.push(format!(
                "{}: the entry `{key}` has two lines `{name}:`",
                entry.page
            ));
        }
    }

    fn row(&mut self, entry: Option<&str>, in_section: bool, cells: Vec<String>) {
        if let [name, _, run] = cells.as_slice()
            && let Some((number, _)) = run.strip_prefix('[').and_then(|rest| rest.split_once("]("))
            && !number.is_empty()
            && number.bytes().all(|byte| byte.is_ascii_digit())
        {
            self.runs.insert(name.to_lowercase(), number.to_owned());
        }
        let separator = cells
            .iter()
            .all(|cell| cell.chars().all(|character| character == '-'));
        if separator {
            return;
        }
        if let Some(entry) = entry {
            self.entries
                .entry(entry.to_owned())
                .or_default()
                .table
                .push(cells);
        } else if in_section && let Some((_, section)) = self.sections.last_mut() {
            section.rows.push(cells);
        }
    }

    /// Reads the page at `path` into what the pages before it gave.
    fn read(&mut self, path: &'static str, source: &str, problems: &mut Vec<String>) {
        let mut entry: Option<String> = None;
        let mut in_section = false;
        let mut bullet: Option<String> = None;
        for line in source.lines().chain([""]) {
            if in_section && let Some((_, section)) = self.sections.last_mut() {
                section.text.push(' ');
                section.text.push_str(line.trim());
            }
            if let Some(open) = &mut bullet {
                if line.starts_with("  ") && !line.trim().is_empty() {
                    open.push(' ');
                    open.push_str(line.trim());
                    continue;
                }
                let done = bullet.take().unwrap_or_default();
                self.bullet(entry.as_deref(), &done, problems);
            }
            if line.starts_with('#') {
                if !line.starts_with("###") {
                    let section = line
                        .strip_prefix("## `")
                        .and_then(|rest| rest.strip_suffix('`'))
                        .filter(|_| path == ADAPTERS_PAGE);
                    in_section = section.is_some();
                    if let Some(name) = section {
                        self.sections.push((name.to_owned(), Section::default()));
                    }
                }
                entry = line
                    .strip_prefix("### `")
                    .and_then(|rest| rest.strip_suffix('`'))
                    .filter(|id| id.contains('/'))
                    .map(str::to_owned);
                if let Some(id) = &entry {
                    if self.entries.contains_key(id) {
                        problems.push(format!("{path}: the entry `{id}` stands twice"));
                    }
                    self.entries.entry(id.clone()).or_default().page = path;
                }
            } else if let Some(text) = line.strip_prefix("- ") {
                bullet = Some(text.trim().to_owned());
            } else {
                if line.starts_with('|') {
                    let cells = line
                        .trim()
                        .trim_matches('|')
                        .split('|')
                        .map(|cell| cell.trim().to_owned())
                        .collect();
                    self.row(entry.as_deref(), in_section, cells);
                }
                if let Some(id) = &entry
                    && !line.trim().is_empty()
                {
                    let open = self.entries.entry(id.clone()).or_default();
                    open.loose.push(line.trim().to_owned());
                }
            }
        }
    }
}

/// The two pages as one: the entries, runs and commands of both, and the sections.
fn pages(documents: &Documents, problems: &mut Vec<String>) -> Page {
    let mut page = Page::default();
    page.read(PAGE, &documents.readme, problems);
    page.read(ADAPTERS_PAGE, &documents.adapters_page, problems);
    page
}

/// Everything the documents say, parsed.
struct Model {
    maps: BTreeMap<&'static str, BTreeMap<String, Value>>,
    project: String,
    declared: BTreeMap<String, Value>,
    tasks: BTreeSet<String>,
    task_commands: BTreeSet<String>,
    page: Page,
    /// The parsed records by path.
    records: BTreeMap<String, Value>,
}

impl Model {
    fn all(&self, map: &str) -> impl Iterator<Item = (&String, &Value)> {
        self.maps.get(map).into_iter().flatten()
    }

    fn one(&self, map: &str, id: &str) -> Option<&Value> {
        self.maps.get(map)?.get(id)
    }

    /// The output a measurement of an assessment names, as the assessor declares it.
    fn output(&self, assessment: &Value, measurement: &Value) -> Option<&Value> {
        self.declared
            .get(text(assessment, "assessor")?)?
            .get("outputs")?
            .get(text(measurement, "output")?)
    }

    /// The record of an assessment: the file named after it.
    fn record(&self, assessment: &str) -> Option<&Value> {
        self.records
            .get(&format!("quality/records/{assessment}.evidence.json"))
    }
}

/// The results a record holds for a measurement: those of its output and, where the measurement
/// names one, of its boundary.
fn results<'a>(record: &'a Value, measurement: &Value) -> Vec<&'a Value> {
    record
        .pointer("/evidence/results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|result| text(result, "output") == text(measurement, "output"))
        .filter(|result| {
            text(measurement, "boundary")
                .is_none_or(|boundary| text(result, "boundary") == Some(boundary))
        })
        .collect()
}

/// A result as the page writes it: the decimal text of a number, or whether an assertion holds.
fn stated(result: &Value) -> String {
    match result.get("holds").and_then(Value::as_bool) {
        Some(true) => "holds".to_owned(),
        Some(false) => "does not hold".to_owned(),
        None => text(result, "value").unwrap_or_default().to_owned(),
    }
}

/// The first word of a baseline without the punctuation that ends it: its number.
fn first_word(baseline: &str) -> &str {
    baseline
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_end_matches(['.', ',', ';'])
}

/// Whether `text` says `phrase` as a whole number: `under 120` is not said by `under 1200`.
fn says(text: &str, phrase: &str) -> bool {
    text.match_indices(phrase).any(|(at, _)| {
        let mut rest = text[at + phrase.len()..].chars();
        match rest.next() {
            Some(next) if next.is_ascii_digit() => false,
            Some('.') => !rest.next().is_some_and(|next| next.is_ascii_digit()),
            _ => true,
        }
    })
}

/// Whether a baseline names a file of the tree between backticks.
fn names_a_file(baseline: &str) -> bool {
    baseline.split('`').skip(1).step_by(2).any(|span| {
        !span.is_empty()
            && !span.contains(char::is_whitespace)
            && !span.starts_with('/')
            && !span.contains("..")
            && root().join(span).is_file()
    })
}

/// The cells of a table of an entry by `<first cell of the row>/<heading of the column>`.
fn cells(table: &[Vec<String>]) -> BTreeMap<String, String> {
    let Some((header, rows)) = table.split_first() else {
        return BTreeMap::new();
    };
    rows.iter()
        .flat_map(|row| {
            let scenario = row.first().cloned().unwrap_or_default();
            header
                .iter()
                .zip(row)
                .skip(1)
                .map(move |(column, cell)| (format!("{scenario}/{column}"), cell.clone()))
        })
        .collect()
}

/// The baseline of an entry against the results the record holds for its measure.
fn check_baseline(
    key: &str,
    entry: &Entry,
    baseline: &str,
    results: &[&Value],
    problems: &mut Vec<String>,
) {
    let at = entry.page;
    let assertion = results.iter().any(|result| result.get("holds").is_some());
    let by_item: BTreeMap<String, String> = results
        .iter()
        .filter_map(|result| Some((text(result, "item")?.to_owned(), stated(result))))
        .collect();
    let expected = if by_item.is_empty() {
        results.first().map(|result| stated(result))
    } else if !entry.table.is_empty() {
        let table = cells(&entry.table);
        for (item, value) in &by_item {
            if table.get(item) != Some(value) {
                problems.push(format!(
                    "{at}: the table of `{key}` has {:?} for {item}, the record has {value}",
                    table.get(item)
                ));
            }
        }
        for item in table.keys().filter(|item| !by_item.contains_key(*item)) {
            problems.push(format!(
                "{at}: the table of `{key}` has {item}, the record has no such item"
            ));
        }
        None
    } else if assertion {
        let held = by_item.values().filter(|value| *value == "holds").count();
        Some(format!("holds for {held} of {}", by_item.len()))
    } else {
        let values: BTreeSet<&String> = by_item.values().collect();
        if values.len() > 1 {
            problems.push(format!(
                "{at}: the record has more than one value for `{key}`, and the entry has no table"
            ));
        }
        values.first().map(|value| (*value).clone())
    };
    let Some(expected) = expected else {
        return;
    };
    let equal = if assertion {
        baseline.starts_with(&expected)
    } else {
        first_word(baseline) == expected
    };
    if !equal {
        problems.push(format!(
            "{at}: the baseline of `{key}` is {baseline:?}, the record has {expected:?}"
        ));
    }
}

/// The entry of one measure: it exists, has a baseline with a source, and agrees with the record.
fn check_entry(
    model: &Model,
    id: &str,
    name: &str,
    measurement: &Value,
    problems: &mut Vec<String>,
) {
    let key = format!("{id}/{name}");
    let Some(entry) = model.page.entries.get(&key) else {
        problems.push(format!(
            "{PAGE}, {ADAPTERS_PAGE}: no entry for the measure `{key}`"
        ));
        return;
    };
    let at = entry.page;
    let Some(baseline) = entry.lines.get("Baseline") else {
        problems.push(format!("{at}: the entry `{key}` has no `Baseline:` line"));
        return;
    };
    if baseline.starts_with("none.") {
        check_unobserved(model, id, name, measurement, problems);
        return;
    }
    let lower = baseline.to_lowercase();
    let runs: Vec<&String> = model
        .page
        .runs
        .iter()
        .filter(|(run, _)| lower.contains(run.as_str()))
        .map(|(_, number)| number)
        .collect();
    if runs.is_empty() && !names_a_file(baseline) {
        problems.push(format!(
            "{at}: the baseline of `{key}` names no run of the table and no file of the tree"
        ));
    }
    let record = model.record(id);
    let results = record
        .map(|record| results(record, measurement))
        .unwrap_or_default();
    for run in runs {
        let Some(record) = record else {
            problems.push(format!(
                "{at}: the entry `{key}` names the run {run}, and quality/records/{id}.evidence.json does not exist"
            ));
            continue;
        };
        let location = record
            .pointer("/evidence/source/location")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !location.ends_with(&format!("/{run}")) {
            problems.push(format!(
                "{at}: the entry `{key}` names the run {run}, the record is of {location:?}"
            ));
        }
        if results.is_empty() {
            problems.push(format!(
                "{at}: the entry `{key}` names the run {run}, the record of {id} has no value for the measure"
            ));
        }
    }
    if !results.is_empty() && !adapters::per_crate(measurement) {
        check_baseline(&key, entry, baseline, &results, problems);
    }
}

/// An entry whose baseline starts with `none.`: the baseline is that one sentence, no record
/// holds a value for the measure, no requirement names it, and nothing under its heading but
/// the lines `Command` and `Needs` holds a digit, so no number stands where a baseline would.
/// Every line up to the next heading is read: a paragraph, a bullet without a name, a table.
fn check_unobserved(
    model: &Model,
    id: &str,
    name: &str,
    measurement: &Value,
    problems: &mut Vec<String>,
) {
    let key = format!("{id}/{name}");
    let Some(entry) = model.page.entries.get(&key) else {
        return;
    };
    let at = entry.page;
    for (line, said) in &entry.lines {
        let free = line == "Command" || line == "Needs";
        if line == "Baseline" && said != UNOBSERVED {
            problems.push(format!(
                "{at}: the baseline of `{key}` is {said:?}; a baseline that starts with `none.` is {UNOBSERVED:?} and nothing else"
            ));
        } else if !free && said.contains(|character: char| character.is_ascii_digit()) {
            problems.push(format!(
                "{at}: the entry `{key}` says `Baseline: none.` and its line `{line}:` holds a number"
            ));
        }
    }
    for line in &entry.loose {
        if line.contains(|character: char| character.is_ascii_digit()) {
            problems.push(format!(
                "{at}: the entry `{key}` says `Baseline: none.` and a line under it holds a number: {line:?}"
            ));
        }
    }
    let held = model
        .record(id)
        .is_some_and(|record| !results(record, measurement).is_empty());
    if held {
        problems.push(format!(
            "{at}: the entry `{key}` says `Baseline: none.`, and quality/records/{id}.evidence.json holds a value for the measure"
        ));
    }
    let measurements = model
        .one("assessments", id)
        .map_or(0, |assessment| entries(assessment, "measurements").count());
    for (requirement, declared) in model.all("requirements") {
        let named = text(declared, "measurement");
        if text(declared, "assessment") == Some(id)
            && (named == Some(name) || (named.is_none() && measurements == 1))
        {
            problems.push(format!(
                "{at}: the entry `{key}` says `Baseline: none.`, and requirements.{requirement} bounds the measure"
            ));
        }
    }
}

fn check_assessments(model: &Model, problems: &mut Vec<String>) {
    let mut measures = BTreeSet::new();
    let mut unheld: BTreeMap<&str, &String> = BTreeMap::new();
    for (id, assessment) in model.all("assessments") {
        let subject = text(assessment, "subject").unwrap_or_default();
        if model.one("subjects", subject).is_none() {
            problems.push(format!(
                "assessments.{id}: no subject {subject:?} is declared"
            ));
        }
        let assessor = text(assessment, "assessor").unwrap_or_default();
        let declared = model.declared.contains_key(assessor);
        if !declared {
            problems.push(format!(
                "assessments.{id}: no declaration quality/assessors/{assessor}.assessor.yaml"
            ));
        }
        if let Some(command) = assessment
            .pointer("/observation/command")
            .and_then(Value::as_str)
        {
            let words: Vec<&str> = command.split_whitespace().collect();
            let called: Vec<&str> = words
                .windows(2)
                .filter(|pair| pair[0] == "task")
                .map(|pair| pair[1])
                .collect();
            for task in &called {
                if !model.tasks.contains(*task) {
                    problems.push(format!(
                        "assessments.{id}: Taskfile.yml has no task for the command `task {task}`"
                    ));
                }
            }
            if called.is_empty()
                && !model.task_commands.contains(command)
                && adapters::target(command).is_none()
            {
                unheld.insert(command, id);
            }
        }
        for (name, measurement) in entries(assessment, "measurements") {
            measures.insert(format!("{id}/{name}"));
            check_entry(model, id, name, measurement, problems);
            if !declared {
                continue;
            }
            let Some(output) = model.output(assessment, measurement) else {
                problems.push(format!(
                    "assessments.{id}.measurements.{name}: {assessor} declares no output {:?}",
                    text(measurement, "output").unwrap_or_default()
                ));
                continue;
            };
            if text(measurement, "unit") != text(output, "unit") {
                problems.push(format!(
                    "assessments.{id}.measurements.{name}: unit is {:?}, {assessor} declares {:?}",
                    text(measurement, "unit"),
                    text(output, "unit")
                ));
            }
            if let Some(boundary) = text(measurement, "boundary")
                && !output
                    .get("boundaries")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .any(|declared| declared.as_str() == Some(boundary))
            {
                problems.push(format!(
                    "assessments.{id}.measurements.{name}: {assessor} declares no boundary {boundary:?} for the output {:?}",
                    text(measurement, "output").unwrap_or_default()
                ));
            }
        }
    }
    for (command, id) in &unheld {
        if !model.page.commands.contains(*command) {
            problems.push(format!(
                "assessments.{id}: the command `{command}` calls no task, stands in no `cmds` list of Taskfile.yml, names no test or example of a package and is not in the list of {PAGE}"
            ));
        }
    }
    for command in &model.page.commands {
        if !unheld.contains_key(command.as_str()) {
            problems.push(format!(
                "{PAGE}: lists the command `{command}`, which is no command of an assessment that neither Taskfile.yml nor a test or example holds"
            ));
        }
    }
    for (key, entry) in &model.page.entries {
        if !measures.contains(key) {
            problems.push(format!("{}: the entry `{key}` has no measure", entry.page));
        }
    }
}

/// How the page writes a comparison with a number.
fn bound_word(operator: &str) -> Option<&'static str> {
    match operator {
        "lt" => Some("under"),
        "lte" => Some("at most"),
        "gt" => Some("over"),
        "gte" => Some("at least"),
        "eq" => Some("exactly"),
        _ => None,
    }
}

fn check_requirements(model: &Model, problems: &mut Vec<String>) {
    for (id, requirement) in model.all("requirements") {
        let name = text(requirement, "assessment").unwrap_or_default();
        let Some(assessment) = model.one("assessments", name) else {
            problems.push(format!(
                "requirements.{id}: no assessment {name:?} is declared"
            ));
            continue;
        };
        let measurements: Vec<&String> = entries(assessment, "measurements")
            .map(|(name, _)| name)
            .collect();
        let measurement = match text(requirement, "measurement") {
            Some(measurement) if !measurements.iter().any(|name| *name == measurement) => {
                problems.push(format!(
                    "requirements.{id}: the assessment {name} has no measurement {measurement:?}"
                ));
                continue;
            }
            None if measurements.len() != 1 => {
                problems.push(format!(
                    "requirements.{id}: names no measurement, and the assessment {name} has {}",
                    measurements.len()
                ));
                continue;
            }
            Some(measurement) => measurement,
            None => measurements[0].as_str(),
        };
        let Some(condition) = requirement.get("condition") else {
            continue;
        };
        if let Some(output) = assessment
            .pointer(&format!("/measurements/{measurement}"))
            .and_then(|declared| model.output(assessment, declared))
            && text(condition, "unit") != text(output, "unit")
        {
            problems.push(format!(
                "requirements.{id}: the unit of the condition is {:?}, the assessor declares {:?}",
                text(condition, "unit"),
                text(output, "unit")
            ));
        }
        let (Some(word), Some(value)) = (
            text(condition, "operator").and_then(bound_word),
            condition.get("value").filter(|value| value.is_number()),
        ) else {
            continue;
        };
        let key = format!("{name}/{measurement}");
        let entry = model.page.entries.get(&key);
        let at = entry.map_or(PAGE, |entry| entry.page);
        let bound = entry.and_then(|entry| entry.lines.get("Bound"));
        if !bound.is_some_and(|bound| says(bound, &format!("{word} {value}"))) {
            problems.push(format!(
                "{at}: the bound of `{key}` is {bound:?}, requirements.{id} says `{word} {value}`"
            ));
        }
    }
}

fn check_gates(model: &Model, problems: &mut Vec<String>) {
    for (id, gate) in model.all("gates") {
        for entry in gate
            .get("requires")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let name = entry.as_str().or_else(|| text(entry, "requirement"));
            if !name.is_some_and(|name| model.one("requirements", name).is_some()) {
                problems.push(format!(
                    "gates.{id}: requires {}, which is no requirement",
                    name.map_or_else(|| entry.to_string(), |name| format!("{name:?}"))
                ));
            }
        }
    }
}

fn check_records(model: &Model, problems: &mut Vec<String>) {
    for (path, record) in &model.records {
        match text(record, "format") {
            Some(EVIDENCE_FORMAT) => {}
            other => problems.push(format!(
                "{path}: format is {other:?}, not {EVIDENCE_FORMAT:?}"
            )),
        }
        let evidence = record.get("evidence").unwrap_or(&Value::Null);
        if text(evidence, "project") != Some(model.project.as_str()) {
            problems.push(format!(
                "{path}: project is {:?}, the specification's is {:?}",
                text(evidence, "project"),
                model.project
            ));
        }
        let name = text(evidence, "assessment").unwrap_or_default();
        if *path != format!("quality/records/{name}.evidence.json") {
            problems.push(format!(
                "{path}: is the record of {name:?}, which is not its file name"
            ));
        }
        let assessor = text(evidence, "assessor").unwrap_or_default();
        match model.one("assessments", name) {
            None => problems.push(format!("{path}: no assessment {name:?} is declared")),
            Some(assessment) if text(assessment, "assessor") != Some(assessor) => {
                problems.push(format!(
                    "{path}: the assessor is {assessor:?}, the assessment's is {:?}",
                    text(assessment, "assessor")
                ));
            }
            Some(_) => {}
        }
        let Some(declaration) = model.declared.get(assessor) else {
            problems.push(format!(
                "{path}: no declaration quality/assessors/{assessor}.assessor.yaml"
            ));
            continue;
        };
        let reported: BTreeSet<&str> = declaration
            .get("coverage")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        for count in COVERAGE_COUNTS {
            let counted = evidence
                .get("coverage")
                .and_then(|coverage| coverage.get(count))
                .and_then(Value::as_u64);
            if !reported.contains(count) && counted != Some(0) {
                problems.push(format!(
                    "{path}: the coverage has {counted:?} for `{count}`, a count {assessor} does not report; such a count is 0"
                ));
            }
        }
        for result in evidence
            .get("results")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let output = text(result, "output").unwrap_or_default();
            let Some(declared) = declaration.get("outputs").and_then(|all| all.get(output)) else {
                problems.push(format!("{path}: {assessor} declares no output {output:?}"));
                continue;
            };
            if text(result, "unit") != text(declared, "unit") {
                problems.push(format!(
                    "{path}: the unit of {output} is {:?}, {assessor} declares {:?}",
                    text(result, "unit"),
                    text(declared, "unit")
                ));
            }
        }
    }
}

/// The documents parsed, with a line in `problems` for each that does not parse.
fn model(documents: &Documents, problems: &mut Vec<String>) -> Model {
    let (maps, project) = specification(documents, problems);
    let declared = assessors(documents, problems);
    let (tasks, task_commands) = tasks(documents, problems);
    let mut records = BTreeMap::new();
    for (path, source) in &documents.records {
        match serde_json::from_str::<Value>(source) {
            Ok(record) => {
                records.insert(path.clone(), record);
            }
            Err(error) => problems.push(format!("{path}: is not valid JSON: {error}")),
        }
    }
    let page = pages(documents, problems);
    Model {
        maps,
        project,
        declared,
        tasks,
        task_commands,
        page,
        records,
    }
}

/// Every way the documents and the page disagree, one line each; empty when they agree.
fn problems(documents: &Documents) -> Vec<String> {
    let mut problems = Vec::new();
    check_config(documents, &mut problems);
    let model = model(documents, &mut problems);
    check_assessments(&model, &mut problems);
    check_requirements(&model, &mut problems);
    check_gates(&model, &mut problems);
    check_records(&model, &mut problems);
    adapters::check(documents, &model, &mut problems);
    problems
}

/// The record of an assessment holds a result for each measure of its plan, and for a measure
/// with one value per package a result for each package the plan names. `problems` holds the
/// same through the pages: an entry that names a run needs a value of its measure in the record
/// (`check_entry`), and a section of `quality/adapters.md` a result for its adapter
/// (`adapters::check_crate_rows`). This rule reads the plan alone.
fn check_measure_results(model: &Model, problems: &mut Vec<String>) {
    for (id, assessment) in model.all("assessments") {
        let Some(record) = model.record(id) else {
            continue;
        };
        let path = format!("quality/records/{id}.evidence.json");
        let packages: Vec<&str> = assessment
            .pointer("/inputs/packages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        for (name, measurement) in entries(assessment, "measurements") {
            let key = format!("{id}/{name}");
            let found = results(record, measurement);
            if found.is_empty() {
                problems.push(format!("{path}: no result of `{key}`"));
                continue;
            }
            if !adapters::per_crate(measurement) {
                continue;
            }
            for package in &packages {
                if !found
                    .iter()
                    .any(|result| text(result, "item") == Some(package))
                {
                    problems.push(format!(
                        "{path}: no result of `{key}` for the package {package}"
                    ));
                }
            }
        }
    }
}

/// The rules a record is held to by itself and against its plan, and no value of a page: that
/// it parses; its format, project, file name, assessor, outputs, units and the counts of its
/// coverage (`check_records`); a result for each measure of its plan, and for each package of a
/// measure with one value per package (`check_measure_results`); the packages and the coverage
/// sums of a plan that names packages, and one result per test of a plan that lists tests
/// (`adapters::check_records`). The baselines, tables and runs the pages state are left out.
fn record_problems(documents: &Documents) -> Vec<String> {
    let mut parsed = Vec::new();
    let model = model(documents, &mut parsed);
    let mut problems: Vec<String> = parsed
        .into_iter()
        .filter(|problem| problem.starts_with("quality/records/"))
        .collect();
    check_records(&model, &mut problems);
    check_measure_results(&model, &mut problems);
    adapters::check_records(&model, &mut problems);
    problems
}

fn report(problems: &[String]) -> String {
    format!("{} problem(s):\n{}", problems.len(), problems.join("\n"))
}

#[test]
fn the_quality_documents_agree_with_each_other_and_with_the_taskfile() {
    let found = problems(&load());
    assert!(found.is_empty(), "{}", report(&found));
}

const GATE: &str = "quality/gate.ratchet.yaml";

/// `source` with every `from` replaced by `to`; `from` is in it.
fn changed(source: &str, from: &str, to: &str) -> String {
    assert!(source.contains(from), "the text holds {from:?}");
    source.replace(from, to)
}

/// The problems of the documents after `plant` changed them, held to name `expected`.
#[track_caller]
fn assert_problem(expected: &str, plant: impl FnOnce(&mut Documents)) {
    let mut documents = load();
    plant(&mut documents);
    let found = problems(&documents);
    assert!(
        found.iter().any(|problem| problem.contains(expected)),
        "no problem names {expected:?}; {}",
        report(&found)
    );
}

fn change(files: &mut BTreeMap<String, String>, path: &str, from: &str, to: &str) {
    let source = files
        .get_mut(path)
        .unwrap_or_else(|| panic!("the documents hold {path}"));
    *source = changed(source, from, to);
}

#[test]
fn a_task_the_taskfile_does_not_have_is_a_problem() {
    assert_problem("no task for the command `task test-run`", |documents| {
        documents.taskfile = changed(&documents.taskfile, "  test-run:", "  test-run-renamed:");
    });
}

#[test]
fn a_command_that_left_the_taskfile_is_a_problem() {
    assert_problem(
        "the command `cargo clippy --workspace --all-targets --all-features -- -D warnings` calls no task",
        |documents| {
            documents.taskfile = changed(
                &documents.taskfile,
                "      - cargo clippy --workspace --all-targets --all-features -- -D warnings\n",
                "",
            );
        },
    );
}

#[test]
fn a_command_the_page_stops_listing_is_a_problem() {
    assert_problem(
        "the command `npx playwright test --workers=2` calls no task",
        |documents| {
            documents.readme = changed(
                &documents.readme,
                "- `npx playwright test --workers=2`\n",
                "",
            );
        },
    );
}

#[test]
fn an_output_the_assessor_does_not_declare_is_a_problem() {
    assert_problem("declares no output \"slowest_case\"", |documents| {
        change(
            &mut documents.specifications,
            GATE,
            "output: slowest_test",
            "output: slowest_case",
        );
    });
}

#[test]
fn a_gate_entry_with_a_condition_that_names_no_requirement_is_a_problem() {
    assert_problem(
        "gates.slow: requires \"slow-test-budgets\", which is no requirement",
        |documents| {
            change(
                &mut documents.specifications,
                GATE,
                "      - requirement: slow-test-budget\n",
                "      - requirement: slow-test-budgets\n",
            );
        },
    );
}

#[test]
fn a_record_of_another_assessor_than_its_assessment_is_a_problem() {
    assert_problem(
        "the assessor is \"forge-job@1\", the assessment's is Some(\"command-run@1\")",
        |documents| {
            change(
                &mut documents.records,
                "quality/records/lint.evidence.json",
                "\"assessor\": \"command-run@1\"",
                "\"assessor\": \"forge-job@1\"",
            );
        },
    );
}

#[test]
fn a_measure_the_readme_does_not_describe_is_a_problem() {
    assert_problem(
        "no entry for the measure `test-run-step/run-time`",
        |documents| {
            documents.readme = changed(
                &documents.readme,
                "`test-run-step/run-time`",
                "`test-run-step`",
            );
        },
    );
}

#[test]
fn a_baseline_that_names_no_source_is_a_problem() {
    assert_problem(
        "the baseline of `check-workflow/run-time` names no run of the table and no file of the tree",
        |documents| {
            documents.readme = changed(
                &documents.readme,
                "- Baseline: 589 s. Check run.\n",
                "- Baseline: 589 s.\n",
            );
        },
    );
}

#[test]
fn an_entry_that_names_a_run_without_its_record_is_a_problem() {
    assert_problem(
        "the entry `slow-workflow/run-time` names the run 45474240955, and quality/records/slow-workflow.evidence.json does not exist",
        |documents| {
            documents
                .records
                .remove("quality/records/slow-workflow.evidence.json")
                .expect("the record is in the tree");
        },
    );
}

#[test]
fn a_cell_of_a_table_that_differs_from_the_record_is_a_problem() {
    assert_problem(
        "the table of `docs-ux/actions` has Some(\"5\") for find-error-kinds/390x844, the record has 4",
        |documents| {
            documents.readme = changed(
                &documents.readme,
                "| find-error-kinds | 2 | 4 |",
                "| find-error-kinds | 2 | 5 |",
            );
        },
    );
}
