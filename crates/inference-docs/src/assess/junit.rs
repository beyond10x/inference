//! The JUnit report cargo-nextest writes, `target/nextest/<profile>/junit.xml`: the start and
//! the length of the run on the root `<testsuites>`, and one `<testcase>` per test that the run
//! reported, with its binary in `classname`, its path in the binary in `name` and its time in
//! seconds in `time`. A `<failure>` or an `<error>` in a test case is a test that failed, a
//! `<skipped>` one that did not run; every other test case passed, also one with a
//! `<flakyFailure>`, which nextest writes for a test that passed on a retry.
//!
//! Not handled: an entity a DTD declares, the normalisation of white space in an attribute value,
//! a namespace prefix (it is ignored), and a `<testcase>` that is not a child of a `<testsuite>`
//! under the root (it is not read).

use xmlparser::{ElementEnd, Token, Tokenizer};

/// What the program reads from a report.
pub(super) struct Report {
    /// The `timestamp` of the root: when the run started.
    pub(super) timestamp: Option<String>,
    /// The `time` of the root: how long the run took, in seconds.
    pub(super) time: Option<String>,
    pub(super) cases: Vec<Case>,
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
    classname: Option<String>,
    name: Option<String>,
    time: Option<String>,
    verdict: Verdict,
}

impl Open {
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
        Ok(Case {
            classname,
            name,
            time,
            nanos,
            verdict: self.verdict,
        })
    }
}

/// The report in `text`, or why it is not one.
pub(super) fn parse(text: &str) -> Result<Report, String> {
    let mut report = Report {
        timestamp: None,
        time: None,
        cases: Vec::new(),
    };
    let mut open: Vec<String> = Vec::new();
    let mut rooted = false;
    let mut suite = String::new();
    let mut case: Option<Open> = None;
    for token in Tokenizer::from(text) {
        match token.map_err(|error| error.to_string())? {
            Token::ElementStart { local, .. } => {
                let name = local.as_str();
                match (open.last().map(String::as_str), name) {
                    (None, "testsuites") => rooted = true,
                    (None, other) => {
                        return Err(format!("its root is <{other}>, not <testsuites>"));
                    }
                    (Some("testsuites"), "testsuite") => suite.clear(),
                    (Some("testsuite"), "testcase") => {
                        case = Some(Open {
                            suite: suite.clone(),
                            classname: None,
                            name: None,
                            time: None,
                            verdict: Verdict::Passed,
                        });
                    }
                    (Some("testcase"), "failure" | "error") => {
                        if let Some(case) = case.as_mut() {
                            case.verdict = Verdict::Failed;
                        }
                    }
                    (Some("testcase"), "skipped") => {
                        if let Some(case) = case.as_mut()
                            && case.verdict == Verdict::Passed
                        {
                            case.verdict = Verdict::Skipped;
                        }
                    }
                    _ => {}
                }
                open.push(name.to_owned());
            }
            Token::Attribute { local, value, .. } => {
                let value = unescape(value.as_str())?;
                match (open.last().map(String::as_str), local.as_str()) {
                    (Some("testsuites"), "timestamp") => report.timestamp = Some(value),
                    (Some("testsuites"), "time") => report.time = Some(value),
                    (Some("testsuite"), "name") => suite = value,
                    (Some("testcase"), attribute) => {
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
                    && open.last().map(String::as_str) == Some("testsuite")
                    && let Some(done) = case.take()
                {
                    report.cases.push(done.finish()?);
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

    /// Whether `case` is the test `function` of a file at this place.
    fn holds(&self, case: &Case, function: &str) -> bool {
        let path: Vec<&str> = case.name.split("::").collect();
        let function: Vec<&str> = function.split("::").collect();
        if !path.ends_with(&function) {
            return false;
        }
        let binary = case.binary();
        match self {
            Self::Library { modules } => {
                binary.is_none_or(|binary| binary.starts_with("bin/")) && starts(&path, modules)
            }
            Self::Binary { binary: expected } => binary == Some(expected.as_str()),
            Self::Module { directory, modules } => {
                binary.is_some_and(|binary| !binary.starts_with("bin/"))
                    && ((binary == Some(directory.as_str()) && starts(&path, modules))
                        || (path.first() == Some(&directory.as_str())
                            && starts(&path[1..], modules)))
            }
        }
    }
}

/// Whether `path` starts with `modules` and has the function after them.
fn starts(path: &[&str], modules: &[String]) -> bool {
    path.len() > modules.len()
        && modules
            .iter()
            .zip(path)
            .all(|(module, part)| module.as_str() == *part)
}

/// What a report holds for a named test.
pub(super) enum Lookup<'r> {
    Found(&'r Case),
    Absent,
    /// More than one test case the name can name.
    Many(Vec<&'r Case>),
    /// The file of the name is under no `src/` or `tests/`.
    Unplaced,
}

/// The test case of `report` that the named test `<file>::<function>` of `package` is.
pub(super) fn named<'r>(report: &'r Report, package: &str, test: &str) -> Lookup<'r> {
    let Some(place) = test
        .split_once("::")
        .and_then(|(file, function)| Some((Place::of(file)?, function)))
    else {
        return Lookup::Unplaced;
    };
    let (place, function) = place;
    let found: Vec<&Case> = report
        .cases
        .iter()
        .filter(|case| case.package() == package && place.holds(case, function))
        .collect();
    match found.len() {
        0 => Lookup::Absent,
        1 => Lookup::Found(found[0]),
        _ => Lookup::Many(found),
    }
}
