//! What the check holds against the tree: the sections of `quality/adapters.md` against the
//! directories under `adapters/`, the tests a measure names against the files that hold them,
//! and the `cargo test` and `cargo run` commands against the packages of the workspace.
//!
//! A test is named in the documents as `<file>::<function>` in the `tests` input of an
//! assessment, and on a page by the words "test \`<function>\` of \`<file>\`". A file is a path
//! from the root of the repository.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use super::{
    ADAPTERS_PAGE, Documents, Model, change, changed, entries, load, names, problems,
    record_problems, report, results, text,
};

/// A package of the workspace: its directory from the root, and its manifest.
struct Package {
    directory: String,
    manifest: Value,
}

/// A manifest of the tree, parsed.
fn manifest(documents: &Documents, path: &str) -> Option<Value> {
    documents
        .file(path)
        .and_then(|source| toml::from_str::<Value>(&source).ok())
}

/// The packages of the workspace by name. A manifest is read when a command first names its
/// package: every test runs the whole check, and no command names most of the packages.
struct Packages<'a> {
    documents: &'a Documents,
    /// The directories of the members, from the root.
    directories: Vec<String>,
    read: BTreeMap<String, Option<Package>>,
}

impl<'a> Packages<'a> {
    /// The members of the root manifest; a member that ends in `/*` is each directory it holds.
    fn new(documents: &'a Documents, problems: &mut Vec<String>) -> Self {
        let workspace = manifest(documents, "Cargo.toml");
        if workspace.is_none() {
            problems.push("Cargo.toml: does not parse".to_owned());
        }
        let members = workspace
            .as_ref()
            .and_then(|workspace| workspace.pointer("/workspace/members"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str);
        let mut directories = Vec::new();
        for member in members {
            match member.strip_suffix("/*") {
                Some(parent) => directories.extend(
                    names(parent, true)
                        .into_iter()
                        .map(|name| format!("{parent}/{name}")),
                ),
                None => directories.push(member.to_owned()),
            }
        }
        Self {
            documents,
            directories,
            read: BTreeMap::new(),
        }
    }

    /// The package of this name, or `None` when no member's manifest names it.
    fn get(&mut self, name: &str) -> Option<&Package> {
        if !self.read.contains_key(name) {
            let found = self.find(name);
            self.read.insert(name.to_owned(), found);
        }
        self.read.get(name).and_then(Option::as_ref)
    }

    /// The member whose manifest names the package. The directory called like the package is
    /// read first, the others only when it is not the one.
    fn find(&self, name: &str) -> Option<Package> {
        let called = |directory: &String| directory.rsplit('/').next() == Some(name);
        let first = self
            .directories
            .iter()
            .filter(|directory| called(directory));
        let others = self
            .directories
            .iter()
            .filter(|directory| !called(directory));
        first.chain(others).find_map(|directory| {
            let manifest = manifest(self.documents, &format!("{directory}/Cargo.toml"))?;
            (manifest.pointer("/package/name").and_then(Value::as_str) == Some(name)).then(|| {
                Package {
                    directory: directory.clone(),
                    manifest,
                }
            })
        })
    }
}

/// The files of the tree a check reads, each read once.
struct Files<'a> {
    documents: &'a Documents,
    read: BTreeMap<String, Option<String>>,
}

impl Files<'_> {
    fn get(&mut self, path: &str) -> Option<&str> {
        if !self.read.contains_key(path) {
            self.read.insert(path.to_owned(), self.documents.file(path));
        }
        self.read.get(path).and_then(Option::as_deref)
    }
}

/// A test or an example of a package, as a `cargo test` or a `cargo run` command names it.
pub(super) struct Target<'a> {
    package: &'a str,
    /// `tests` or `examples`: the directory of the target.
    directory: &'static str,
    /// `test` or `example`: the table of the target in the manifest.
    table: &'static str,
    name: &'a str,
    features: Vec<&'a str>,
    all_features: bool,
}

/// The target of a command of the form `cargo test -p <package> ... --test <name>` or
/// `cargo run -p <package> ... --example <name>`; `None` for every other command. What follows
/// a lone `--` belongs to the program and is not read.
pub(super) fn target(command: &str) -> Option<Target<'_>> {
    let words: Vec<&str> = command.split_whitespace().collect();
    let cargo = words
        .windows(2)
        .position(|pair| pair[0] == "cargo" && matches!(pair[1], "test" | "run"))?;
    let (flag, directory, table) = if words[cargo + 1] == "test" {
        ("--test", "tests", "test")
    } else {
        ("--example", "examples", "example")
    };
    let arguments: Vec<&str> = words[cargo + 2..]
        .iter()
        .copied()
        .take_while(|word| *word != "--")
        .collect();
    let after = |flag: &str| {
        arguments
            .windows(2)
            .find(|pair| pair[0] == flag)
            .map(|pair| pair[1])
    };
    Some(Target {
        package: after("-p").or_else(|| after("--package"))?,
        directory,
        table,
        name: after(flag)?,
        features: after("--features")
            .map(|list| list.split(',').collect())
            .unwrap_or_default(),
        all_features: arguments.contains(&"--all-features"),
    })
}

/// The features that `named` turn on: those, the default ones, and what each turns on in turn.
fn turned_on<'a>(features: Option<&'a Map<String, Value>>, named: &[&'a str]) -> BTreeSet<&'a str> {
    let mut on = BTreeSet::new();
    let mut open: Vec<&str> = named.to_vec();
    open.push("default");
    while let Some(feature) = open.pop() {
        if on.insert(feature) {
            open.extend(
                features
                    .and_then(|features| features.get(feature))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str),
            );
        }
    }
    on
}

/// A command that names a target: the package is one of the workspace, the file of the target
/// exists, the package declares each feature the command turns on, and the command turns on
/// each feature the target requires.
fn check_target(
    source: &str,
    command: &str,
    packages: &mut Packages<'_>,
    files: &mut Files<'_>,
    problems: &mut Vec<String>,
) {
    let Some(target) = target(command) else {
        return;
    };
    let Some(package) = packages.get(target.package) else {
        problems.push(format!(
            "{source}: the command `{command}` names {}, which is no package of the workspace",
            target.package
        ));
        return;
    };
    let declared = package
        .manifest
        .get(target.table)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|declared| text(declared, "name") == Some(target.name));
    let file = declared
        .and_then(|declared| text(declared, "path"))
        .map_or_else(
            || {
                format!(
                    "{}/{}/{}.rs",
                    package.directory, target.directory, target.name
                )
            },
            |path| format!("{}/{path}", package.directory),
        );
    if files.get(&file).is_none() {
        problems.push(format!(
            "{source}: the command `{command}` names the {} {}, and {file} does not exist",
            target.table, target.name
        ));
    }
    let features = package.manifest.get("features").and_then(Value::as_object);
    for feature in &target.features {
        if !features.is_some_and(|features| features.contains_key(*feature)) {
            problems.push(format!(
                "{source}: the command `{command}` turns on the feature {feature:?}, which {} does not declare",
                target.package
            ));
        }
    }
    if target.all_features {
        return;
    }
    let on = turned_on(features, &target.features);
    let required = declared
        .and_then(|declared| declared.get("required-features"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str);
    for feature in required {
        if !on.contains(feature) {
            problems.push(format!(
                "{source}: the command `{command}` does not turn on the feature {feature:?}, which the {} {} requires",
                target.table, target.name
            ));
        }
    }
}

/// The code spans of a page that call cargo, each with its white space as single spaces.
fn spans(source: &str) -> BTreeSet<String> {
    source
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|span| span.contains("cargo"))
        .map(|span| span.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect()
}

/// Every command of an assessment and every code span of a page that names a target.
fn check_commands(
    documents: &Documents,
    model: &Model,
    files: &mut Files<'_>,
    problems: &mut Vec<String>,
) {
    let mut packages = Packages::new(documents, problems);
    for (id, assessment) in model.all("assessments") {
        if let Some(command) = assessment
            .pointer("/observation/command")
            .and_then(Value::as_str)
        {
            check_target(
                &format!("assessments.{id}"),
                command,
                &mut packages,
                files,
                problems,
            );
        }
    }
    for (path, source) in documents.pages() {
        for span in spans(source) {
            check_target(path, &span, &mut packages, files, problems);
        }
    }
}

/// Every "test \`<function>\` of \`<file>\`" in `text`, as `<file>::<function>`. The words may
/// stand on two lines.
fn named_tests(text: &str) -> BTreeSet<String> {
    let joined = text.lines().map(str::trim).collect::<Vec<_>>().join(" ");
    let mut found = BTreeSet::new();
    let mut rest = joined.as_str();
    while let Some(at) = rest.find("test `") {
        rest = &rest[at + "test `".len()..];
        let Some((function, after)) = rest.split_once('`') else {
            break;
        };
        if let Some((file, _)) = after
            .strip_prefix(" of `")
            .and_then(|after| after.split_once('`'))
        {
            found.insert(format!("{file}::{function}"));
        }
    }
    found
}

/// Whether `code` holds a function item of this name: a line whose code starts with
/// `fn <function>(`, with nothing before it but `pub`, `pub(...)`, `async`, `const` or `unsafe`.
/// The name in a comment or in a string of one line is no item. Not told apart: a line of a
/// block comment or of a string over several lines that starts like the item.
fn holds_function(code: &str, function: &str) -> bool {
    let item = format!("fn {function}(");
    code.match_indices(&item).any(|(at, _)| {
        let line = code[..at].rfind('\n').map_or(0, |end| end + 1);
        code[line..at].split_whitespace().all(|word| {
            matches!(word, "pub" | "async" | "const" | "unsafe")
                || (word.starts_with("pub(") && word.ends_with(')'))
        })
    })
}

/// A named test: its file exists and holds a function item of its name.
fn check_test(source: &str, test: &str, files: &mut Files<'_>, problems: &mut Vec<String>) {
    let Some((file, function)) = test.split_once("::") else {
        problems.push(format!(
            "{source}: the test {test:?} is not `<file>::<function>`"
        ));
        return;
    };
    match files.get(file) {
        None => problems.push(format!(
            "{source}: names the test `{function}` of `{file}`, and that file does not exist"
        )),
        Some(code) if !holds_function(code, function) => problems.push(format!(
            "{source}: names the test `{function}` of `{file}`, and that file has no `fn {function}`"
        )),
        Some(_) => {}
    }
}

/// The tests an assessment lists as its `tests` input, if it lists any.
fn listed_tests(assessment: &Value) -> Option<BTreeSet<String>> {
    let listed = assessment
        .pointer("/inputs/tests")
        .and_then(Value::as_array)?;
    Some(
        listed
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
    )
}

/// The tests the pages name, and the tests an assessment lists as its `tests` input: each is in
/// its file, and the entry of each measure of the assessment names exactly the listed tests.
/// That the record has one result per listed test is `check_named_records`.
fn check_named_tests(
    documents: &Documents,
    model: &Model,
    files: &mut Files<'_>,
    problems: &mut Vec<String>,
) {
    for (path, source) in documents.pages() {
        for test in named_tests(source) {
            check_test(path, &test, files, problems);
        }
    }
    for (id, assessment) in model.all("assessments") {
        let Some(listed) = listed_tests(assessment) else {
            continue;
        };
        for test in &listed {
            check_test(&format!("assessments.{id}"), test, files, problems);
        }
        for (name, _) in entries(assessment, "measurements") {
            let key = format!("{id}/{name}");
            if let Some(entry) = model.page.entries.get(&key) {
                let said = named_tests(&entry.bullets.join(" "));
                for test in listed.difference(&said) {
                    problems.push(format!(
                        "{}: the entry `{key}` does not name the test {test}, which assessments.{id} lists",
                        entry.page
                    ));
                }
                for test in said.difference(&listed) {
                    problems.push(format!(
                        "{}: the entry `{key}` names the test {test}, which assessments.{id} does not list",
                        entry.page
                    ));
                }
            }
        }
    }
}

/// The record of an assessment that lists tests as its `tests` input has one result per listed
/// test for each measure, and none for another test.
fn check_named_records(model: &Model, problems: &mut Vec<String>) {
    for (id, assessment) in model.all("assessments") {
        let Some(listed) = listed_tests(assessment) else {
            continue;
        };
        for (name, measurement) in entries(assessment, "measurements") {
            let key = format!("{id}/{name}");
            let Some(record) = model.record(id) else {
                continue;
            };
            let held: BTreeSet<String> = results(record, measurement)
                .iter()
                .filter_map(|result| text(result, "item"))
                .map(str::to_owned)
                .collect();
            for test in listed.difference(&held) {
                problems.push(format!(
                    "quality/records/{id}.evidence.json: no result of `{key}` for the test {test}, which assessments.{id} lists"
                ));
            }
            for test in held.difference(&listed) {
                problems.push(format!(
                    "quality/records/{id}.evidence.json: a result of `{key}` for the test {test}, which assessments.{id} does not list"
                ));
            }
        }
    }
}

/// The sections of `quality/adapters.md` are the directories under `adapters/` that hold a
/// `Cargo.toml`, in their order, and each names the committed records of its adapter.
fn check_sections(documents: &Documents, model: &Model, problems: &mut Vec<String>) {
    let sections: Vec<&String> = model.page.sections.iter().map(|(name, _)| name).collect();
    for name in &documents.adapter_crates {
        if !sections.contains(&name) {
            problems.push(format!(
                "{ADAPTERS_PAGE}: no section for the adapter crate `adapters/{name}`"
            ));
        }
    }
    let mut seen = BTreeSet::new();
    for name in &sections {
        if !documents.adapter_crates.contains(*name) {
            problems.push(format!(
                "{ADAPTERS_PAGE}: the section `{name}` names no directory under adapters/ that holds a Cargo.toml"
            ));
        }
        if !seen.insert(name) {
            problems.push(format!(
                "{ADAPTERS_PAGE}: the section `{name}` stands twice"
            ));
        }
    }
    if !sections.is_sorted() {
        problems.push(format!(
            "{ADAPTERS_PAGE}: the sections are not in the order of the directories under adapters/"
        ));
    }
    for path in &documents.adapter_records {
        let adapter = path.split('/').nth(1).unwrap_or_default();
        if let Some(section) = model.page.section(adapter)
            && !section.text.contains(&format!("`{path}`"))
        {
            problems.push(format!(
                "{ADAPTERS_PAGE}: the section `{adapter}` does not name the committed record `{path}`"
            ));
        }
    }
}

/// Whether a measurement has one value per package: its values stand in the sections.
pub(super) fn per_crate(measurement: &Value) -> bool {
    text(measurement, "scope") == Some("package")
}

/// The sentence a section of `quality/adapters.md` carries in the place of its rows while no
/// adopted record holds its crate. A crate added under `adapters/` is added to the packages of
/// its plan in the same change; the first check run on `main` after it writes a record that
/// holds it, and the adoption of that record turns the sentence into rows.
pub(super) const NO_RECORD_YET: &str =
    "No adopted record holds this crate yet: its rows come with the next run adopted from `main`.";

/// Whether the section of `adapter` carries [`NO_RECORD_YET`].
fn says_no_record_yet(model: &Model, adapter: &str) -> bool {
    model
        .page
        .section(adapter)
        .is_some_and(|section| section.text.contains(NO_RECORD_YET))
}

/// The packages of `packages` whose section says [`NO_RECORD_YET`] and of which `record` holds
/// no result at all. The rules of a record against its plan leave these out, and no other.
pub(super) fn awaiting_a_record<'a>(
    model: &Model,
    packages: impl IntoIterator<Item = &'a str>,
    record: &Value,
) -> BTreeSet<&'a str> {
    let held: BTreeSet<&str> = record
        .pointer("/evidence/results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|result| text(result, "item"))
        .collect();
    packages
        .into_iter()
        .filter(|package| !held.contains(package) && says_no_record_yet(model, package))
        .collect()
}

/// A measure with one value per package: every section has a row `| \`<measure>\` | <value>
/// <unit> | <run> |` that says what the record holds for the package of the section. A section
/// that says [`NO_RECORD_YET`] has no such row instead, the record holds no result for its
/// crate, and a plan with one value per package names the crate.
fn check_crate_rows(model: &Model, problems: &mut Vec<String>) {
    let mut measures = BTreeSet::new();
    let mut planned = BTreeSet::new();
    for (id, assessment) in model.all("assessments") {
        for (name, measurement) in entries(assessment, "measurements") {
            if !per_crate(measurement) {
                continue;
            }
            planned.extend(
                assessment
                    .pointer("/inputs/packages")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str),
            );
            let key = format!("`{id}/{name}`");
            let record = model.record(id);
            let results = record
                .map(|record| results(record, measurement))
                .unwrap_or_default();
            let location = record
                .and_then(|record| record.pointer("/evidence/source/location"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            for (adapter, section) in &model.page.sections {
                let row = section.rows.iter().find(|row| row.first() == Some(&key));
                if section.text.contains(NO_RECORD_YET) {
                    if row.is_some() {
                        problems.push(format!(
                            "{ADAPTERS_PAGE}: the section `{adapter}` says that no adopted record holds the crate and has a row for the measure {key}"
                        ));
                    }
                    if results
                        .iter()
                        .any(|result| text(result, "item") == Some(adapter))
                    {
                        problems.push(format!(
                            "{ADAPTERS_PAGE}: the section `{adapter}` says that no adopted record holds the crate, and quality/records/{id}.evidence.json has a result of {key} for it"
                        ));
                    }
                    continue;
                }
                let Some(row) = row else {
                    problems.push(format!(
                        "{ADAPTERS_PAGE}: the section `{adapter}` has no row for the measure {key}"
                    ));
                    continue;
                };
                let Some(result) = results
                    .iter()
                    .find(|result| text(result, "item") == Some(adapter))
                else {
                    problems.push(format!(
                        "quality/records/{id}.evidence.json: no result of {key} for {adapter}"
                    ));
                    continue;
                };
                let mut expected = format!(
                    "{} {}",
                    text(result, "value").unwrap_or_default(),
                    text(result, "unit").unwrap_or_default()
                );
                if let Some(test) = result.pointer("/details/test").and_then(Value::as_str) {
                    expected = format!("{expected}, `{test}`");
                }
                if row.get(1) != Some(&expected) {
                    problems.push(format!(
                        "{ADAPTERS_PAGE}: the section `{adapter}` has {:?} for {key}, the record has {expected:?}",
                        row.get(1)
                    ));
                }
                let run = row
                    .get(2)
                    .and_then(|cell| model.page.runs.get(&cell.to_lowercase()));
                if !run.is_some_and(|run| location.ends_with(&format!("/{run}"))) {
                    problems.push(format!(
                        "{ADAPTERS_PAGE}: the section `{adapter}` names {:?} as the source of {key}, the record is of {location:?}",
                        row.get(2)
                    ));
                }
            }
            measures.insert(key);
        }
    }
    for (adapter, section) in &model.page.sections {
        if section.text.contains(NO_RECORD_YET) && !planned.contains(adapter.as_str()) {
            problems.push(format!(
                "{ADAPTERS_PAGE}: the section `{adapter}` says that no adopted record holds the crate, and no plan with one value per package names it"
            ));
        }
        for measure in section.rows.iter().filter_map(|row| row.first()) {
            if measure.starts_with('`') && !measures.contains(measure) {
                problems.push(format!(
                    "{ADAPTERS_PAGE}: the section `{adapter}` has a row for {measure}, which is no measure with one value per package"
                ));
            }
        }
    }
}

/// A record whose plan names packages as its input: it has one `tests_run` result for each
/// package the plan names and for no other, and its coverage counts the tests of those packages,
/// `executed` the sum of the `tests_run` results and `passed` the sum of the `tests_passed` ones.
/// A package awaiting a record ([`awaiting_a_record`]) needs no result.
fn check_package_coverage(model: &Model, problems: &mut Vec<String>) {
    for (id, assessment) in model.all("assessments") {
        let Some(packages) = assessment
            .pointer("/inputs/packages")
            .and_then(Value::as_array)
        else {
            continue;
        };
        let Some(record) = model.record(id) else {
            continue;
        };
        let path = format!("quality/records/{id}.evidence.json");
        let of = |output: &'static str| {
            record
                .pointer("/evidence/results")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(move |result| text(result, "output") == Some(output))
        };
        let named: BTreeSet<&str> = packages.iter().filter_map(Value::as_str).collect();
        let held: BTreeSet<&str> = of("tests_run")
            .filter_map(|result| text(result, "item"))
            .collect();
        let awaiting = awaiting_a_record(model, named.iter().copied(), record);
        if named
            .difference(&awaiting)
            .copied()
            .collect::<BTreeSet<&str>>()
            != held
        {
            problems.push(format!(
                "{path}: the `tests_run` results are of {held:?}, assessments.{id} names the packages {named:?}"
            ));
        }
        for (count, output) in [("executed", "tests_run"), ("passed", "tests_passed")] {
            let sum: u64 = of(output)
                .filter_map(|result| text(result, "value")?.parse::<u64>().ok())
                .sum();
            let counted = record
                .pointer("/evidence/coverage")
                .and_then(|coverage| coverage.get(count))
                .and_then(Value::as_u64);
            if counted != Some(sum) {
                problems.push(format!(
                    "{path}: the coverage counts {counted:?} {count} tests, and the `{output}` results of the packages of assessments.{id} sum to {sum}"
                ));
            }
        }
    }
}

/// An entry of `quality/adapters.md` gives the command of its assessment, whole, as one code
/// span of its `Command:` line.
fn check_entry_commands(model: &Model, problems: &mut Vec<String>) {
    for (key, entry) in &model.page.entries {
        if entry.page != ADAPTERS_PAGE {
            continue;
        }
        let Some((id, _)) = key.split_once('/') else {
            continue;
        };
        let Some(command) = model
            .one("assessments", id)
            .and_then(|assessment| assessment.pointer("/observation/command"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let said = entry.lines.get("Command").is_some_and(|line| {
            line.split('`')
                .skip(1)
                .step_by(2)
                .any(|span| span == command)
        });
        if !said {
            problems.push(format!(
                "{ADAPTERS_PAGE}: the `Command:` line of `{key}` does not give the command of assessments.{id}, `{command}`"
            ));
        }
    }
}

pub(super) fn check(documents: &Documents, model: &Model, problems: &mut Vec<String>) {
    let mut files = Files {
        documents,
        read: BTreeMap::new(),
    };
    check_sections(documents, model, problems);
    check_crate_rows(model, problems);
    check_package_coverage(model, problems);
    check_named_tests(documents, model, &mut files, problems);
    check_named_records(model, problems);
    check_commands(documents, model, &mut files, problems);
    check_entry_commands(model, problems);
}

/// The rules of this file that a record is held to by itself and against its plan, and no value
/// of a page: the packages and coverage sums of a plan that names packages, and one result per
/// test of a plan that lists tests.
pub(super) fn check_records(model: &Model, problems: &mut Vec<String>) {
    check_package_coverage(model, problems);
    check_named_records(model, problems);
}

const ADAPTERS: &str = "quality/adapters.ratchet.yaml";

/// The problems of the documents after `plant` changed them, held to name each of `expected`.
/// A test plants the defects of one rule together: every test is a process of its own that
/// reads and checks all documents, so one check per rule keeps the step short.
#[track_caller]
fn assert_problems(expected: &[&str], plant: impl FnOnce(&mut Documents)) {
    let mut documents = load();
    plant(&mut documents);
    let found = problems(&documents);
    for expected in expected {
        assert!(
            found.iter().any(|problem| problem.contains(expected)),
            "no problem names {expected:?}; {}",
            report(&found)
        );
    }
}

/// `from` becomes `to` on the page of the adapters.
fn on_page(documents: &mut Documents, from: &str, to: &str) {
    documents.adapters_page = changed(&documents.adapters_page, from, to);
}

/// `from` becomes `to` in the plans of the adapters.
fn in_plans(documents: &mut Documents, from: &str, to: &str) {
    change(&mut documents.specifications, ADAPTERS, from, to);
}

#[test]
fn the_sections_are_the_adapter_crates_once_each_and_in_their_order() {
    assert_problems(
        &[
            "quality/adapters.md: no section for the adapter crate `adapters/inference-new`",
            "quality/adapters.md: no section for the adapter crate `adapters/inference-phonic`",
            "the section `inference-phonics` names no directory under adapters/ that holds a Cargo.toml",
            "the section `inference-aws` stands twice",
            "the sections are not in the order of the directories under adapters/",
        ],
        |documents| {
            let page = &documents.adapters_page;
            let at = |heading: &str| page.find(heading).expect("the page has the section");
            let (deepgram, deepslate, elevenlabs) = (
                at("## `inference-deepgram`\n"),
                at("## `inference-deepslate`\n"),
                at("## `inference-elevenlabs`\n"),
            );
            documents.adapters_page = [
                &page[..deepgram],
                &page[deepslate..elevenlabs],
                &page[deepgram..deepslate],
                &page[elevenlabs..],
            ]
            .concat();
            documents.adapter_crates.insert("inference-new".to_owned());
            on_page(
                documents,
                "## `inference-phonic`\n",
                "## `inference-phonics`\n",
            );
            on_page(
                documents,
                "## `inference-cartesia`\n",
                "## `inference-aws`\n",
            );
        },
    );
}

#[test]
fn a_section_says_what_the_record_and_the_tree_hold_for_its_adapter() {
    assert_problems(
        &[
            "the section `inference-aws` has Some(\"135 tests\") for `adapter-tests/tests-run`, the record has \"134 tests\"",
            "the section `inference-cartesia` has no row for the measure `adapter-tests/tests-passed`",
            "the section `inference-deepgram` names Some(\"Slow run\") as the source of `adapter-tests/tests-run`",
            "the section `inference-deepslate` has a row for `adapter-tests/tests-skipped`, which is no measure with one value per package",
            "for `adapter-tests/slowest-test`, the record has \"0.494 s, `inference-sherpa-onnx::stt_network the_archive_download_is_reached_only_through_build_dependencies`\"",
            "the section `inference-phonic` does not name the committed record `adapters/inference-phonic/records/claims-new.json`",
            "the section `inference-llama-server` does not name the committed record `adapters/inference-llama-server/capability/reference-machine-cpu-tools.json`",
        ],
        |documents| {
            on_page(
                documents,
                "| `adapter-tests/tests-run` | 134 tests | Check run |",
                "| `adapter-tests/tests-run` | 135 tests | Check run |",
            );
            on_page(
                documents,
                "| `adapter-tests/tests-passed` | 37 tests | Check run |\n",
                "",
            );
            on_page(
                documents,
                "| `adapter-tests/tests-run` | 126 tests | Check run |",
                "| `adapter-tests/tests-run` | 126 tests | Slow run |",
            );
            on_page(
                documents,
                "| `adapter-tests/tests-passed` | 70 tests | Check run |",
                "| `adapter-tests/tests-skipped` | 70 tests | Check run |",
            );
            on_page(
                documents,
                "| 0.494 s, `inference-sherpa-onnx::stt_network the_archive_download",
                "| 0.494 s, `inference-sherpa-onnx::stt_network the_archive",
            );
            documents
                .adapter_records
                .insert("adapters/inference-phonic/records/claims-new.json".to_owned());
            on_page(
                documents,
                "`adapters/inference-llama-server/capability/reference-machine-cpu-tools.json`",
                "the record for tools",
            );
        },
    );
}

#[test]
fn a_function_or_a_file_that_left_the_tree_is_a_problem_for_what_names_it() {
    let renamed = "adapters/inference-cartesia/tests/cancel.rs";
    let gone = [
        "adapters/inference-sherpa-onnx/tests/stt_record.rs",
        "crates/inference-testkit/tests/conform.rs",
        "adapters/inference-sherpa-onnx/tests/stt_resources.rs",
        "adapters/inference-whispercpp/tests/stt_resources.rs",
    ];
    assert_problems(
        &[
            "quality/adapters.md: names the test `a_cancel_does_not_wait_for_the_server` of `adapters/inference-cartesia/tests/cancel.rs`, and that file has no `fn a_cancel_does_not_wait_for_the_server`",
            "assessments.cartesia-time-bounds: names the test `a_cancel_does_not_wait_for_the_server` of `adapters/inference-cartesia/tests/cancel.rs`, and that file has no `fn a_cancel_does_not_wait_for_the_server`",
            "quality/adapters.md: names the test `the_record_of_the_reference_machine_loads_and_evidences_exactly_the_claims` of `adapters/inference-sherpa-onnx/tests/stt_record.rs`, and that file does not exist",
            "quality/README.md: names the test `the_resource_profile_is_a_measured_cpu_profile` of `adapters/inference-sherpa-onnx/tests/stt_record.rs`, and that file does not exist",
            "assessments.conformance: the command `cargo test -p inference-testkit --features run --test conform` names the test conform, and crates/inference-testkit/tests/conform.rs does not exist",
            "names the test stt_resources, and adapters/inference-sherpa-onnx/tests/stt_resources.rs does not exist",
            "names the test stt_resources, and adapters/inference-whispercpp/tests/stt_resources.rs does not exist",
        ],
        |documents| {
            let code = documents
                .file(renamed)
                .expect("the test file is in the tree");
            let code = changed(
                &code,
                "fn a_cancel_does_not_wait_for_the_server(",
                "fn a_cancel_returns_at_once(",
            );
            documents.replaced.insert(renamed.to_owned(), Some(code));
            for file in gone {
                documents.replaced.insert(file.to_owned(), None);
            }
        },
    );
}

#[test]
fn an_entry_and_a_record_name_the_tests_their_plan_lists() {
    assert_problems(
        &[
            "the entry `aws-time-bounds/held` does not name the test adapters/inference-aws/tests/nova_sonic_stall_and_bounds.rs::the_open_deadline_is_stated, which assessments.aws-time-bounds lists",
            "the entry `aws-time-bounds/held` names the test adapters/inference-aws/tests/nova_sonic_stall_and_bounds.rs::a_server_that_never_answers_fails_open_within_the_deadline, which assessments.aws-time-bounds does not list",
            "no result of `cartesia-time-bounds/held` for the test adapters/inference-cartesia/tests/cancel.rs::a_cancel_does_not_wait_for_the_server",
            "a result of `cartesia-time-bounds/held` for the test adapters/inference-cartesia/tests/cancel.rs::another, which assessments.cartesia-time-bounds does not list",
        ],
        |documents| {
            on_page(
                documents,
                "test `the_open_deadline_is_stated` of",
                "test `a_server_that_never_answers_fails_open_within_the_deadline` of",
            );
            change(
                &mut documents.records,
                "quality/records/cartesia-time-bounds.evidence.json",
                "\"item\": \"adapters/inference-cartesia/tests/cancel.rs::a_cancel_does_not_wait_for_the_server\"",
                "\"item\": \"adapters/inference-cartesia/tests/cancel.rs::another\"",
            );
        },
    );
}

#[test]
fn a_command_names_a_package_a_target_and_features_that_exist() {
    let live = "cargo test -p inference-cartesia --features live --test live";
    let misspelt = "cargo test -p inference-cartesia --features lives --test live";
    assert_problems(
        &[
            "assessments.cartesia-live: the command `cargo test -p inference-cartesia --features lives --test live -- --ignored --nocapture` turns on the feature \"lives\", which inference-cartesia does not declare",
            "quality/adapters.md: the command `cargo test -p inference-cartesia --features lives --test live -- --ignored --nocapture` turns on the feature \"lives\", which inference-cartesia does not declare",
            "does not turn on the feature \"tools\", which the test live_tools requires",
            "does not turn on the feature \"live\", which the example live requires",
            "names inference-deepgrams, which is no package of the workspace",
            "names the test live_cancelation, and adapters/inference-llama-server/tests/live_cancelation.rs does not exist",
            "names the example talks, and examples/local-voice-cascade/examples/talks.rs does not exist",
        ],
        |documents| {
            in_plans(documents, live, misspelt);
            on_page(documents, live, misspelt);
            in_plans(
                documents,
                "--features live,tools --test live_tools",
                "--features live --test live_tools",
            );
            in_plans(
                documents,
                "cargo run -p inference-elevenlabs --features live --example live",
                "cargo run -p inference-elevenlabs --example live",
            );
            in_plans(
                documents,
                "cargo test -p inference-deepgram --features live --test live",
                "cargo test -p inference-deepgrams --features live --test live",
            );
            in_plans(
                documents,
                "--test live_cancellation",
                "--test live_cancelation",
            );
            in_plans(
                documents,
                "cargo run -p local-voice-cascade --features local --example talk",
                "cargo run -p local-voice-cascade --features local --example talks",
            );
        },
    );
}

#[test]
fn an_entry_without_a_baseline_has_no_number_no_record_and_no_requirement() {
    let none = "- Baseline: none. No run of this command is kept.";
    let bound = "- Bound: none. Nothing fails on this value.";
    assert_problems(
        &[
            "the baseline of `cartesia-live/first-audio` is \"none. No run of this command is kept. About 300 ms.\"",
            "the entry `deepgram-live/first-event` says `Baseline: none.` and its line `Bound:` holds a number",
            "the baseline of `elevenlabs-live-example/first-audio` names no run of the table and no file of the tree",
            "the entry `cartesia-live/first-audio` says `Baseline: none.`, and quality/records/cartesia-live.evidence.json holds a value for the measure",
            "the entry `deepgram-live/first-event` says `Baseline: none.`, and requirements.deepgram-first-event bounds the measure",
        ],
        |documents| {
            on_page(
                documents,
                &format!("so the connect is inside.\n{bound}\n{none}\n\n## `inference-deepgram`"),
                &format!(
                    "so the connect is inside.\n{bound}\n{none} About 300 ms.\n\n## `inference-deepgram`"
                ),
            );
            on_page(
                documents,
                &format!("  connect is inside.\n{bound}"),
                "  connect is inside.\n- Bound: under 500 ms.",
            );
            on_page(
                documents,
                &format!("first segment, so the connect is inside.\n{bound}\n{none}"),
                &format!("first segment, so the connect is inside.\n{bound}\n- Baseline: 300 ms."),
            );
            documents.records.insert(
                "quality/records/cartesia-live.evidence.json".to_owned(),
                concat!(
                    "{\"format\": \"ratchet-evidence/1\", \"evidence\": {\"project\": \"inference\", ",
                    "\"assessment\": \"cartesia-live\", \"assessor\": \"printed-duration@1\", ",
                    "\"results\": [{\"output\": \"duration\", \"kind\": \"sampled\", ",
                    "\"boundary\": \"end_of_utterance_to_first_audio\", \"scope\": \"run\", ",
                    "\"value_type\": \"number\", \"value\": \"300\", \"unit\": \"ms\"}]}}"
                )
                .to_owned(),
            );
            in_plans(
                documents,
                "\ngates:\n",
                concat!(
                    "  deepgram-first-event:\n",
                    "    meaning: The first words of a transcript come soon.\n",
                    "    assessment: deepgram-live\n",
                    "    measurement: first-event\n",
                    "    condition: {operator: lt, value: 500, unit: ms}\n",
                    "\ngates:\n"
                ),
            );
        },
    );
}

#[test]
fn a_coverage_counts_the_packages_of_its_plan_and_what_its_assessor_reports() {
    let record = "quality/records/adapter-tests.evidence.json";
    assert_problems(
        &[
            "quality/records/adapter-tests.evidence.json: the coverage counts Some(3059) executed tests, and the `tests_run` results of the packages of assessments.adapter-tests sum to 1166",
            "quality/records/adapter-tests.evidence.json: the coverage counts Some(1165) passed tests, and the `tests_passed` results of the packages of assessments.adapter-tests sum to 1166",
            "quality/records/adapter-tests.evidence.json: the coverage has Some(72) for `skipped`, a count package-tests@1 does not report; such a count is 0",
            "the `tests_run` results are of {\"inference-audio-device\", \"inference-aws\"",
            "assessments.adapter-tests names the packages {\"inference-audio-device\", \"inference-aws\", \"inference-cartesia\", \"inference-deepgram\", \"inference-deepslate\", \"inference-elevenlabs\", \"inference-llama-server\", \"inference-new\"",
        ],
        |documents| {
            change(
                &mut documents.records,
                record,
                "\"executed\": 1166",
                "\"executed\": 3059",
            );
            change(
                &mut documents.records,
                record,
                "\"passed\": 1166",
                "\"passed\": 1165",
            );
            change(
                &mut documents.records,
                record,
                "\"skipped\": 0",
                "\"skipped\": 72",
            );
            in_plans(
                documents,
                "        - inference-llama-server\n        - inference-openai\n",
                "        - inference-llama-server\n        - inference-new\n        - inference-openai\n",
            );
        },
    );
}

const ADAPTER_TESTS: &str = "quality/records/adapter-tests.evidence.json";

/// The committed record of `adapter-tests` without the results of `adapter`, its coverage
/// counting the rest: the record of a run before `adapter` was added.
fn record_without(documents: &mut Documents, adapter: &str) {
    let mut record: Value = serde_json::from_str(&documents.records[ADAPTER_TESTS]).unwrap();
    let results = record
        .pointer_mut("/evidence/results")
        .and_then(Value::as_array_mut)
        .expect("the record has results");
    let count = |results: &[Value], output: &str| -> u64 {
        results
            .iter()
            .filter(|result| text(result, "item") == Some(adapter))
            .filter(|result| text(result, "output") == Some(output))
            .filter_map(|result| text(result, "value")?.parse::<u64>().ok())
            .sum()
    };
    let (run, passed) = (count(results, "tests_run"), count(results, "tests_passed"));
    assert!(run > 0, "the record holds {adapter}");
    results.retain(|result| text(result, "item") != Some(adapter));
    for (field, less) in [("executed", run), ("passed", passed)] {
        let at = record
            .pointer_mut(&format!("/evidence/coverage/{field}"))
            .expect("the record has a coverage");
        *at = Value::from(at.as_u64().unwrap() - less);
    }
    documents.records.insert(
        ADAPTER_TESTS.to_owned(),
        serde_json::to_string_pretty(&record).unwrap(),
    );
}

/// The table of the section of `adapter` replaced by [`NO_RECORD_YET`].
fn sentence_for_rows(documents: &mut Documents, adapter: &str) {
    let page = &documents.adapters_page;
    let heading = format!("## `{adapter}`\n");
    let start = page.find(&heading).expect("the page has the section") + heading.len();
    let end = page[start..]
        .find("\n## ")
        .map_or(page.len(), |at| start + at);
    let section: String = page[start..end]
        .lines()
        .map(|line| {
            if line.starts_with("| Measure ") {
                format!("{NO_RECORD_YET}\n")
            } else if line.starts_with('|') {
                String::new()
            } else {
                format!("{line}\n")
            }
        })
        .collect();
    documents.adapters_page = format!("{}{section}{}", &page[..start], &page[end..]);
}

#[test]
fn a_crate_the_plan_names_and_no_adopted_record_holds_stands_with_the_sentence_and_no_rows() {
    let mut documents = load();
    record_without(&mut documents, "inference-whispercpp");
    sentence_for_rows(&mut documents, "inference-whispercpp");
    let found = problems(&documents);
    assert!(found.is_empty(), "{}", report(&found));
    let found = record_problems(&documents);
    assert!(found.is_empty(), "{}", report(&found));
}

#[test]
fn the_sentence_on_a_crate_the_record_holds_or_the_plan_does_not_name_is_refused() {
    assert_problems(
        &[
            "the section `inference-aws` says that no adopted record holds the crate and has a row for the measure `adapter-tests/tests-run`",
            "the section `inference-aws` says that no adopted record holds the crate, and quality/records/adapter-tests.evidence.json has a result of `adapter-tests/slowest-test` for it",
            "the section `inference-cartesia` says that no adopted record holds the crate, and quality/records/adapter-tests.evidence.json has a result of `adapter-tests/tests-passed` for it",
            "the section `inference-whispercpp` says that no adopted record holds the crate, and no plan with one value per package names it",
        ],
        |documents| {
            on_page(
                documents,
                "## `inference-aws`\n",
                &format!("## `inference-aws`\n\n{NO_RECORD_YET}\n"),
            );
            sentence_for_rows(documents, "inference-cartesia");
            record_without(documents, "inference-whispercpp");
            sentence_for_rows(documents, "inference-whispercpp");
            in_plans(documents, "        - inference-whispercpp\n", "");
        },
    );
}

#[test]
fn rows_for_a_crate_the_record_does_not_hold_are_refused() {
    let mut documents = load();
    record_without(&mut documents, "inference-whispercpp");
    let found = problems(&documents);
    for expected in [
        "quality/records/adapter-tests.evidence.json: no result of `adapter-tests/tests-run` for inference-whispercpp",
        "quality/records/adapter-tests.evidence.json: the `tests_run` results are of",
    ] {
        assert!(
            found.iter().any(|problem| problem.contains(expected)),
            "no problem names {expected:?}; {}",
            report(&found)
        );
    }
    let found = record_problems(&documents);
    assert!(
        found.iter().any(|problem| problem.contains(
            "no result of `adapter-tests/tests-run` for the package inference-whispercpp"
        )),
        "{}",
        report(&found)
    );
}

#[test]
fn a_package_of_the_record_the_plan_does_not_name_is_refused_with_a_crate_awaiting_its_record() {
    assert_problems(
        &[
            "quality/records/adapter-tests.evidence.json: the `tests_run` results are of {\"inference-audio-device\", \"inference-aws\"",
        ],
        |documents| {
            record_without(documents, "inference-whispercpp");
            sentence_for_rows(documents, "inference-whispercpp");
            in_plans(documents, "        - inference-aws\n", "");
        },
    );
}

#[test]
fn a_boundary_is_one_the_assessor_declares_for_the_output() {
    assert_problems(
        &[
            "assessments.cartesia-live.measurements.first-audio: printed-duration@1 declares no boundary \"end_of_utterance_to_first_sound\" for the output \"duration\"",
        ],
        |documents| {
            in_plans(
                documents,
                "boundary: end_of_utterance_to_first_audio",
                "boundary: end_of_utterance_to_first_sound",
            );
        },
    );
}

#[test]
fn an_entry_gives_the_command_of_its_plan_and_stands_once() {
    assert_problems(
        &[
            "the `Command:` line of `cascade-talk/first-audio` does not give the command of assessments.cascade-talk, `cargo run -p local-voice-cascade --features local --example talk`",
            "quality/adapters.md: the entry `formatting/clean` stands twice",
        ],
        |documents| {
            on_page(
                documents,
                "- Command: `cargo run -p local-voice-cascade --features local --example talk`. It prints\n  `committed input",
                "- Command: `cargo run -p local-voice-cascade --example talk`. It prints\n  `committed input",
            );
            on_page(
                documents,
                "### `cartesia-live/first-audio`\n",
                "### `formatting/clean`\n",
            );
        },
    );
}

mod adversary_u4;
