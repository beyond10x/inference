//! The quality documents of the repository (`ratchet.toml`, `quality/`) agree with each other and
//! with `Taskfile.yml`.
//!
//! This is not `ratchet validate`: no Ratchet command works at the commit the documents are
//! written against (`quality/README.md` names it). It holds only what would make the documents
//! untrue without anyone noticing: a file that stops parsing, a name that points at nothing (an
//! assessment, a measurement, an assessor, an output, a task), a unit that differs from the
//! assessor's, a record of another format, and a measure the README does not describe.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::root;

const SPEC_FORMAT: &str = "ratchet/1";
const ASSESSOR_FORMAT: &str = "ratchet-assessor/1";
const EVIDENCE_FORMAT: &str = "ratchet-evidence/1";
const ENTITY_MAPS: [&str; 4] = ["subjects", "assessments", "requirements", "gates"];

/// The files the check reads, each as its path from the repository root and its text.
#[derive(Clone)]
struct Documents {
    config: String,
    taskfile: String,
    readme: String,
    specifications: BTreeMap<String, String>,
    assessors: BTreeMap<String, String>,
    records: BTreeMap<String, String>,
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
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
    Documents {
        config: read(&root().join("ratchet.toml")),
        taskfile: read(&root().join("Taskfile.yml")),
        readme: read(&root().join("quality/README.md")),
        specifications: files("quality", ".ratchet.yaml"),
        assessors: files("quality/assessors", ".assessor.yaml"),
        records: files("quality/records", ".json"),
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

/// The task names of `Taskfile.yml`.
fn tasks(documents: &Documents, problems: &mut Vec<String>) -> BTreeSet<String> {
    yaml("Taskfile.yml", &documents.taskfile, problems)
        .map(|file| {
            entries(&file, "tasks")
                .map(|(name, _)| name.clone())
                .collect()
        })
        .unwrap_or_default()
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

/// Every way the documents disagree, one line each; empty when they agree.
fn problems(documents: &Documents) -> Vec<String> {
    let mut problems = Vec::new();
    check_config(documents, &mut problems);
    let (maps, project) = specification(documents, &mut problems);
    let declared = assessors(documents, &mut problems);
    let tasks = tasks(documents, &mut problems);
    let none = BTreeMap::new();
    let assessments = maps.get("assessments").unwrap_or(&none);

    for (id, assessment) in assessments {
        let assessor = text(assessment, "assessor").unwrap_or_default();
        let declaration = declared.get(assessor);
        if declaration.is_none() {
            problems.push(format!(
                "assessments.{id}: no declaration quality/assessors/{assessor}.assessor.yaml"
            ));
        }
        if let Some(task) = assessment
            .pointer("/observation/command")
            .and_then(Value::as_str)
            .and_then(|command| command.strip_prefix("task "))
            && !tasks.contains(task.split_whitespace().next().unwrap_or_default())
        {
            problems.push(format!(
                "assessments.{id}: Taskfile.yml has no task for the command `task {task}`"
            ));
        }
        for (name, measurement) in entries(assessment, "measurements") {
            if !documents.readme.contains(&format!("`{id}/{name}`")) {
                problems.push(format!(
                    "quality/README.md: no entry for the measure `{id}/{name}`"
                ));
            }
            let Some(declaration) = declaration else {
                continue;
            };
            let output = text(measurement, "output").unwrap_or_default();
            let Some(declared_output) = declaration.get("outputs").and_then(|o| o.get(output))
            else {
                problems.push(format!(
                    "assessments.{id}.measurements.{name}: {assessor} declares no output {output:?}"
                ));
                continue;
            };
            let unit = text(declared_output, "unit");
            if text(measurement, "unit") != unit {
                problems.push(format!(
                    "assessments.{id}.measurements.{name}: unit is {:?}, {assessor} declares {unit:?}",
                    text(measurement, "unit")
                ));
            }
        }
    }

    for (id, requirement) in maps.get("requirements").unwrap_or(&none) {
        let name = text(requirement, "assessment").unwrap_or_default();
        let Some(assessment) = assessments.get(name) else {
            problems.push(format!(
                "requirements.{id}: no assessment {name:?} is declared"
            ));
            continue;
        };
        let measurements: Vec<&String> = entries(assessment, "measurements")
            .map(|(name, _)| name)
            .collect();
        match text(requirement, "measurement") {
            Some(measurement) if !measurements.iter().any(|name| *name == measurement) => {
                problems.push(format!(
                    "requirements.{id}: the assessment {name} has no measurement {measurement:?}"
                ));
            }
            None if measurements.len() != 1 => problems.push(format!(
                "requirements.{id}: names no measurement, and the assessment {name} has {}",
                measurements.len()
            )),
            _ => {}
        }
    }

    for (path, source) in &documents.records {
        let record = match serde_json::from_str::<Value>(source) {
            Ok(record) => record,
            Err(error) => {
                problems.push(format!("{path}: is not valid JSON: {error}"));
                continue;
            }
        };
        match text(&record, "format") {
            Some(EVIDENCE_FORMAT) => {}
            other => problems.push(format!(
                "{path}: format is {other:?}, not {EVIDENCE_FORMAT:?}"
            )),
        }
        let evidence = record.get("evidence").unwrap_or(&Value::Null);
        let assessment = text(evidence, "assessment").unwrap_or_default();
        if !assessments.contains_key(assessment) {
            problems.push(format!("{path}: no assessment {assessment:?} is declared"));
        }
        if text(evidence, "project") != Some(project.as_str()) {
            problems.push(format!(
                "{path}: project is {:?}, the specification's is {project:?}",
                text(evidence, "project")
            ));
        }
    }
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

/// The documents with `from` replaced by `to` in the one specification file that holds it.
fn with_specification_changed(from: &str, to: &str) -> Documents {
    let mut documents = load();
    let holders: Vec<String> = documents
        .specifications
        .iter()
        .filter(|(_, source)| source.contains(from))
        .map(|(path, _)| path.clone())
        .collect();
    assert_eq!(holders.len(), 1, "one specification file holds {from:?}");
    let source = documents.specifications.get_mut(&holders[0]).unwrap();
    *source = source.replace(from, to);
    documents
}

#[test]
fn a_task_the_taskfile_does_not_have_is_a_problem() {
    let mut documents = load();
    documents.taskfile = documents
        .taskfile
        .replace("  test-run:", "  test-run-renamed:");
    let found = problems(&documents);
    assert!(
        found
            .iter()
            .any(|problem| problem.contains("no task for the command `task test-run`")),
        "{}",
        report(&found)
    );
}

#[test]
fn an_output_the_assessor_does_not_declare_is_a_problem() {
    let documents = with_specification_changed("output: slowest_test", "output: slowest_case");
    let found = problems(&documents);
    assert!(
        found
            .iter()
            .any(|problem| problem.contains("declares no output \"slowest_case\"")),
        "{}",
        report(&found)
    );
}

#[test]
fn a_measure_the_readme_does_not_describe_is_a_problem() {
    let mut documents = load();
    documents.readme = documents
        .readme
        .replace("`test-run-step/run-time`", "`test-run-step`");
    let found = problems(&documents);
    assert!(
        found
            .iter()
            .any(|problem| problem.contains("no entry for the measure `test-run-step/run-time`")),
        "{}",
        report(&found)
    );
}
