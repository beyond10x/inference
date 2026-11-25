//! The JUnit report cargo-nextest writes, `target/nextest/<profile>/junit.xml`: the start, the
//! length and the counts of the run on the root `<testsuites>`, one `<testsuite>` per test
//! binary under it, and in each one `<testcase>` per test that the run reported, with its binary
//! in `classname`, its path in the binary in `name` and its time in seconds in `time`. A
//! `<failure>` or an `<error>` in a test case is a test that failed, a `<skipped>` one that did
//! not run; every other test case passed, also one with a `<flakyFailure>`, which nextest writes
//! for a test that passed on a retry. cargo-nextest 0.9.143 writes no ignored test into its
//! report, so a report of it holds no `<skipped>`: the reader counts the element where another
//! writer writes it, and no assessor of the program reports that count.
//!
//! A named test `<file>::<function>` is the test case whose name is the module path of the file
//! in its binary, the inline modules around the function in the file, and the function. The
//! inline modules are read from the text of the file: the `mod <name> {` blocks open where its
//! `fn <function>` stands, past comments, strings and characters.
//!
//! A `<testcase>` anywhere else, directly under the root, in a `<testsuite>` inside another one
//! or inside any other element, is not read as a test: the report names it in
//! [`Report::unplaced`], and the program refuses such a report. The counts of the root are kept
//! in [`Report::counts`] for the program to compare with the cases read.
//!
//! Not handled: an entity a DTD declares, the normalisation of white space in an attribute value,
//! and a namespace prefix (it is ignored).

use xmlparser::{ElementEnd, Token, Tokenizer};

/// What the program reads from a report.
pub(super) struct Report {
    /// The `timestamp` of the root: when the run started.
    pub(super) timestamp: Option<String>,
    /// The `time` of the root: how long the run took, in seconds.
    pub(super) time: Option<String>,
    /// The counts the root states, as far as it states them.
    pub(super) counts: Counts,
    pub(super) cases: Vec<Case>,
    /// The test cases outside a `<testsuite>` directly under the root, as `<classname> <name>`.
    pub(super) unplaced: Vec<String>,
}

/// The attributes `tests`, `failures`, `errors` and `skipped` of the root.
#[derive(Default)]
pub(super) struct Counts {
    pub(super) tests: Option<u64>,
    pub(super) failures: Option<u64>,
    pub(super) errors: Option<u64>,
    pub(super) skipped: Option<u64>,
}

/// One test case of a report.
pub(super) struct Case {
    /// The package and the binary: `inference-aws::nova_sonic_session`, `inference-deepslate`
    /// for the tests of a library, `inference-docs::bin/inference-docs` for those of a binary.
    pub(super) classname: String,
    /// The path of the test in its binary: `config::tests::the_endpoint_is_read`.
    pub(super) name: String,
    /// The time as the report writes it, in seconds.
    pub(super) time: String,
    /// The same time in nanoseconds.
    pub(super) nanos: u128,
    pub(super) verdict: Verdict,
    /// Whether the case holds a `<failure>`.
    pub(super) failure: bool,
    /// Whether the case holds an `<error>`.
    pub(super) error: bool,
}

impl Case {
    /// The package the test belongs to.
    pub(super) fn package(&self) -> &str {
        self.classname
            .split_once("::")
            .map_or(self.classname.as_str(), |(package, _)| package)
    }

    /// The binary of the test, `None` for the tests of a library.
    fn binary(&self) -> Option<&str> {
        self.classname.split_once("::").map(|(_, binary)| binary)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Verdict {
    Passed,
    Failed,
    Skipped,
}

/// A test case whose element is still open.
struct Open {
    suite: String,
    /// Whether it stands in a `<testsuite>` directly under the root.
    placed: bool,
    classname: Option<String>,
    name: Option<String>,
    time: Option<String>,
    failure: bool,
    error: bool,
    skipped: bool,
}

impl Open {
    /// The case as `<classname> <name>`, for a case the report does not place.
    fn label(&self) -> String {
        format!(
            "{} {}",
            self.classname.as_deref().unwrap_or(&self.suite),
            self.name.as_deref().unwrap_or("without a name")
        )
    }

    fn finish(self) -> Result<Case, String> {
        let name = self
            .name
            .ok_or_else(|| format!("a <testcase> of {} has no name", self.suite))?;
        let classname = self.classname.unwrap_or(self.suite);
        let time = self
            .time
            .ok_or_else(|| format!("the test {classname} {name} has no time"))?;
        let nanos = super::scaled(&time, 9).ok_or_else(|| {
            format!("the test {classname} {name} has the time {time:?}, not a number of seconds")
        })?;
        let verdict = if self.failure || self.error {
            Verdict::Failed
        } else if self.skipped {
            Verdict::Skipped
        } else {
            Verdict::Passed
        };
        Ok(Case {
            classname,
            name,
            time,
            nanos,
            verdict,
            failure: self.failure,
            error: self.error,
        })
    }
}

/// A count of the root, or why it is not one.
fn count(attribute: &str, value: &str) -> Result<Option<u64>, String> {
    value
        .parse()
        .map(Some)
        .map_err(|_| format!("its root has {attribute}={value:?}, not a count"))
}

/// The report in `text`, or why it is not one.
pub(super) fn parse(text: &str) -> Result<Report, String> {
    let mut report = Report {
        timestamp: None,
        time: None,
        counts: Counts::default(),
        cases: Vec::new(),
        unplaced: Vec::new(),
    };
    let mut open: Vec<String> = Vec::new();
    let mut rooted = false;
    let mut suite = String::new();
    let mut case: Option<Open> = None;
    for token in Tokenizer::from(text) {
        match token.map_err(|error| error.to_string())? {
            Token::ElementStart { local, .. } => {
                let name = local.as_str();
                let parents: Vec<&str> = open.iter().map(String::as_str).collect();
                match (parents.as_slice(), name) {
                    ([], "testsuites") => rooted = true,
                    ([], other) => {
                        return Err(format!("its root is <{other}>, not <testsuites>"));
                    }
                    (["testsuites"], "testsuite") => suite.clear(),
                    (_, "testcase") => {
                        if case.is_some() {
                            return Err("a <testcase> stands inside a <testcase>".to_owned());
                        }
                        case = Some(Open {
                            suite: suite.clone(),
                            placed: parents == ["testsuites", "testsuite"],
                            classname: None,
                            name: None,
                            time: None,
                            failure: false,
                            error: false,
                            skipped: false,
                        });
                    }
                    ([.., "testcase"], child) => {
                        if let Some(case) = case.as_mut() {
                            match child {
                                "failure" => case.failure = true,
                                "error" => case.error = true,
                                "skipped" => case.skipped = true,
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
                open.push(name.to_owned());
            }
            Token::Attribute { local, value, .. } => {
                let value = unescape(value.as_str())?;
                let parents: Vec<&str> = open.iter().map(String::as_str).collect();
                match (parents.as_slice(), local.as_str()) {
                    (["testsuites"], "timestamp") => report.timestamp = Some(value),
                    (["testsuites"], "time") => report.time = Some(value),
                    (["testsuites"], "tests") => report.counts.tests = count("tests", &value)?,
                    (["testsuites"], "failures") => {
                        report.counts.failures = count("failures", &value)?;
                    }
                    (["testsuites"], "errors") => {
                        report.counts.errors = count("errors", &value)?;
                    }
                    (["testsuites"], "skipped") => {
                        report.counts.skipped = count("skipped", &value)?;
                    }
                    (["testsuites", "testsuite"], "name") => suite = value,
                    ([.., "testcase"], attribute) => {
                        if let Some(case) = case.as_mut() {
                            match attribute {
                                "name" => case.name = Some(value),
                                "classname" => case.classname = Some(value),
                                "time" => case.time = Some(value),
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            Token::ElementEnd { end, .. } => {
                let closed = match end {
                    ElementEnd::Open => continue,
                    ElementEnd::Empty => open.pop(),
                    ElementEnd::Close(_, local) => {
                        let closed = open.pop();
                        if closed.as_deref() != Some(local.as_str()) {
                            return Err(format!(
                                "</{}> closes <{}>",
                                local.as_str(),
                                closed.unwrap_or_default()
                            ));
                        }
                        closed
                    }
                };
                if closed.as_deref() == Some("testcase")
                    && let Some(done) = case.take()
                {
                    if done.placed {
                        report.cases.push(done.finish()?);
                    } else {
                        report.unplaced.push(done.label());
                    }
                }
            }
            _ => {}
        }
    }
    if !rooted {
        return Err("it holds no <testsuites>".to_owned());
    }
    if let Some(name) = open.last() {
        return Err(format!("it ends inside <{name}>"));
    }
    Ok(report)
}

/// An attribute value with its character and entity references replaced.
fn unescape(raw: &str) -> Result<String, String> {
    let mut text = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(at) = rest.find('&') {
        text.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let Some(end) = after.find(';') else {
            return Err(format!("the attribute value {raw:?} holds a lone `&`"));
        };
        let reference = &after[..end];
        let character = match reference {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => reference
                .strip_prefix("#x")
                .map(|hex| u32::from_str_radix(hex, 16).ok())
                .unwrap_or_else(|| {
                    reference
                        .strip_prefix('#')
                        .and_then(|decimal| decimal.parse().ok())
                })
                .and_then(char::from_u32),
        };
        let Some(character) = character else {
            return Err(format!(
                "the attribute value {raw:?} holds the reference &{reference};"
            ));
        };
        text.push(character);
        rest = &after[end + 1..];
    }
    text.push_str(rest);
    Ok(text)
}

/// Where a named test `<file>::<function>` stands in its crate, read from its file.
enum Place {
    /// Under `src/`: the tests of the library or of a binary; the modules of the file.
    Library { modules: Vec<String> },
    /// `tests/<binary>.rs`, `tests/<binary>/main.rs` or `src/bin/<binary>.rs`.
    Binary { binary: String },
    /// Another file under `tests/<directory>/`: a module of the binary `<directory>`, or of
    /// another binary that declares `mod <directory>`.
    Module {
        directory: String,
        modules: Vec<String>,
    },
}

impl Place {
    fn of(file: &str) -> Option<Self> {
        let parts: Vec<&str> = file.split('/').collect();
        let at = parts
            .iter()
            .position(|part| *part == "src" || *part == "tests")?;
        let rest = &parts[at + 1..];
        let stem = |file: &str| file.strip_suffix(".rs").map(str::to_owned);
        let modules = |files: &[&str]| -> Option<Vec<String>> {
            let (last, directories) = files.split_last()?;
            let mut modules: Vec<String> =
                directories.iter().map(|part| (*part).to_owned()).collect();
            let last = stem(last)?;
            if last != "mod" {
                modules.push(last);
            }
            Some(modules)
        };
        if parts[at] == "src" {
            return match rest {
                ["lib.rs" | "main.rs"] => Some(Self::Library {
                    modules: Vec::new(),
                }),
                ["bin", file] => Some(Self::Binary {
                    binary: format!("bin/{}", stem(file)?),
                }),
                _ => Some(Self::Library {
                    modules: modules(rest)?,
                }),
            };
        }
        match rest {
            [] => None,
            [file] => Some(Self::Binary {
                binary: stem(file)?,
            }),
            [directory, "main.rs"] => Some(Self::Binary {
                binary: (*directory).to_owned(),
            }),
            [directory, more @ ..] => Some(Self::Module {
                directory: (*directory).to_owned(),
                modules: modules(more)?,
            }),
        }
    }

    /// Whether `case` is the test `function` of a file at this place: its name is the module
    /// path of the file in its binary, the inline modules around one definition of the function
    /// in the file (one entry of `inline` each), and the function.
    fn holds(&self, case: &Case, function: &str, inline: &[Vec<String>]) -> bool {
        let binary = case.binary();
        let mut paths: Vec<Vec<&str>> = Vec::new();
        match self {
            Self::Library { modules } => {
                if binary.is_none_or(|binary| binary.starts_with("bin/")) {
                    paths.push(modules.iter().map(String::as_str).collect());
                }
            }
            Self::Binary { binary: expected } => {
                if binary == Some(expected.as_str()) {
                    paths.push(Vec::new());
                }
            }
            Self::Module { directory, modules } => {
                if binary.is_some_and(|binary| !binary.starts_with("bin/")) {
                    if binary == Some(directory.as_str()) {
                        paths.push(modules.iter().map(String::as_str).collect());
                    }
                    paths.push(
                        std::iter::once(directory.as_str())
                            .chain(modules.iter().map(String::as_str))
                            .collect(),
                    );
                }
            }
        }
        paths.into_iter().any(|path| {
            inline.iter().any(|modules| {
                let mut path = path.clone();
                path.extend(modules.iter().map(String::as_str));
                path.push(function);
                case.name == path.join("::")
            })
        })
    }
}

/// What a report holds for a named test.
pub(super) enum Lookup<'r> {
    Found(&'r Case),
    Absent,
    /// More than one test case the name can name.
    Many(Vec<&'r Case>),
    /// The file of the name is under no `src/` or `tests/`.
    Unplaced,
    /// The file of the name cannot be read: why.
    Unread(String),
    /// The file of the name defines no function of the name.
    Undefined,
}

/// The test case of `report` that the named test `<file>::<function>` of `package` is, with the
/// text of the file that `read` gives for its path.
pub(super) fn named<'r>(
    report: &'r Report,
    package: &str,
    test: &str,
    read: impl FnOnce(&str) -> Result<String, String>,
) -> Lookup<'r> {
    let Some((place, file, function)) = test
        .split_once("::")
        .and_then(|(file, function)| Some((Place::of(file)?, file, function)))
    else {
        return Lookup::Unplaced;
    };
    let source = match read(file) {
        Ok(source) => source,
        Err(why) => return Lookup::Unread(why),
    };
    let inline = inline_modules(&source, function);
    if inline.is_empty() {
        return Lookup::Undefined;
    }
    let found: Vec<&Case> = report
        .cases
        .iter()
        .filter(|case| case.package() == package && place.holds(case, function, &inline))
        .collect();
    match found.len() {
        0 => Lookup::Absent,
        1 => Lookup::Found(found[0]),
        _ => Lookup::Many(found),
    }
}

/// For each `fn <function>` of `source`, the names of the inline modules around it, outermost
/// first: the `mod <name> {` blocks open where it stands, whatever attribute they carry. Line and
/// block comments, string, raw string and character literals open and close nothing. Empty when
/// `source` defines no such function.
fn inline_modules(source: &str, function: &str) -> Vec<Vec<String>> {
    let text: Vec<char> = source.chars().collect();
    let mut found = Vec::new();
    // One entry for each open brace: the module it opens, if it opens one.
    let mut open: Vec<Option<String>> = Vec::new();
    let mut module: Option<String> = None;
    let mut at = 0;
    while at < text.len() {
        match text[at] {
            '/' if text.get(at + 1) == Some(&'/') => {
                while at < text.len() && text[at] != '\n' {
                    at += 1;
                }
            }
            '/' if text.get(at + 1) == Some(&'*') => at = block_comment_end(&text, at),
            '"' => at = string_end(&text, at + 1, None),
            '\'' => at = character_end(&text, at),
            '{' => {
                open.push(module.take());
                at += 1;
            }
            '}' => {
                open.pop();
                module = None;
                at += 1;
            }
            ';' => {
                module = None;
                at += 1;
            }
            character if character.is_alphabetic() || character == '_' => {
                let (word, end) = word_at(&text, at);
                at = end;
                if matches!(word.as_str(), "r" | "br" | "cr")
                    && matches!(text.get(at), Some('"' | '#'))
                {
                    let mut hashes = 0;
                    while text.get(at) == Some(&'#') {
                        hashes += 1;
                        at += 1;
                    }
                    if text.get(at) == Some(&'"') {
                        at = string_end(&text, at + 1, Some(hashes));
                    }
                    continue;
                }
                let next = next_word(&text, at);
                match word.as_str() {
                    "mod" => module = next.map(|(name, _)| name),
                    "fn" => {
                        if let Some((name, after)) = next
                            && name == function
                            && matches!(text.get(next_character(&text, after)), Some('(' | '<'))
                        {
                            found.push(open.iter().flatten().cloned().collect());
                        }
                    }
                    _ => {}
                }
            }
            _ => at += 1,
        }
    }
    found
}

/// The identifier that starts at `at`, and where it ends.
fn word_at(text: &[char], mut at: usize) -> (String, usize) {
    let start = at;
    while text
        .get(at)
        .is_some_and(|character| character.is_alphanumeric() || *character == '_')
    {
        at += 1;
    }
    (text[start..at].iter().collect(), at)
}

/// Where the first character after `at` that is no white space stands.
fn next_character(text: &[char], mut at: usize) -> usize {
    while text
        .get(at)
        .is_some_and(|character| character.is_whitespace())
    {
        at += 1;
    }
    at
}

/// The identifier after the white space at `at`, and where it ends.
fn next_word(text: &[char], at: usize) -> Option<(String, usize)> {
    let at = next_character(text, at);
    let (word, end) = word_at(text, at);
    (end > at).then_some((word, end))
}

/// Where the block comment that starts at `at` ends, its nested comments with it.
fn block_comment_end(text: &[char], mut at: usize) -> usize {
    let mut depth = 0_usize;
    while at < text.len() {
        if text[at] == '/' && text.get(at + 1) == Some(&'*') {
            depth += 1;
            at += 2;
        } else if text[at] == '*' && text.get(at + 1) == Some(&'/') {
            depth = depth.saturating_sub(1);
            at += 2;
            if depth == 0 {
                return at;
            }
        } else {
            at += 1;
        }
    }
    text.len()
}

/// Where the string whose text starts at `at` ends: at its `"`, after an escape in a plain one,
/// or at its `"` with `hashes` times `#` in a raw one.
fn string_end(text: &[char], mut at: usize, hashes: Option<usize>) -> usize {
    while at < text.len() {
        match (text[at], hashes) {
            ('\\', None) => at += 2,
            ('"', None) => return at + 1,
            ('"', Some(hashes))
                if text
                    .get(at + 1..at + 1 + hashes)
                    .is_some_and(|after| after.iter().all(|character| *character == '#')) =>
            {
                return at + 1 + hashes;
            }
            _ => at += 1,
        }
    }
    text.len()
}

/// Where the character literal that starts at `at` ends, or the character after the `'` of a
/// lifetime or a label.
fn character_end(text: &[char], at: usize) -> usize {
    match (text.get(at + 1), text.get(at + 2)) {
        (Some('\\'), _) => text
            .get(at + 3..)
            .and_then(|rest| rest.iter().position(|character| *character == '\''))
            .map_or(text.len(), |end| at + 3 + end + 1),
        (Some(_), Some('\'')) => at + 3,
        _ => at + 1,
    }
}
