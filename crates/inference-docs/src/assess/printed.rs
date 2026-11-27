//! The output a live test printed: one line per measure, found by the text the command prints
//! before the value, and the value read as a duration in milliseconds. The ANSI escape sequences
//! that `--color always` or `CARGO_TERM_COLOR=always` adds are taken out before it is read.
//!
//! A value is the debug form of a `Duration` (`1.234s`, `12.5ms`, `870µs`, `15ns`), also inside
//! `Some(...)`, or a number followed by the word `ms`, as two commands print whole milliseconds.
//! `None` is no value, and neither is a line the output holds twice: a measure of a run takes one.
//!
//! Whether the run held is read from the lines `cargo test` and cargo-nextest print. A pass needs
//! the closing line of the run: a `test result: ok.` line with at least one test passed, or a
//! `Summary` line of cargo-nextest with at least one test passed and no other count above 0 but
//! `skipped`. A `test <name> ... ok` line or a `PASS` line is the pass of one test, never the pass
//! of the run, and an output without a closing line is of a run that did not end. `cargo test`
//! closes the tests of each binary, from its `running <n> tests` line, with a `test result:` line
//! of their own: a `running` line without one before the next `running` line or the end is a run
//! that did not end, and the pass is read from the `test result:` line of the tests during which
//! the measures were printed. Where no closed tests hold them, every `test result:` line must be
//! `ok.` and one must count a test passed. A failure is a
//! `test <name> ... FAILED` line, a `test result:` line other than `ok.`, cargo's `error: test
//! failed` line, a panic of a test the output names (`thread '<name>' (<id>) panicked at`, as the
//! pinned rustc prints it, or `thread '<name>' panicked at`), a status line of cargo-nextest with
//! a failing status, also after a retry (`TRY 2 FAIL`, `TMT`, `TIMEOUT`, a signal), a count of its
//! `Summary` line other than `passed` and `skipped` (`failed`, `timed out`, `exec failed`), a
//! `Summary` line of a run that did not run all its tests, and its `error: test run failed` line.
//!
//! Not handled: a value on another line than its label, a duration printed by `Display` or in
//! another unit word (`2 s`, `1.5 seconds`). Not read at all, because no single line of a run holds
//! the value: `compare` and `compare-cascade`, which write their values to `runs.jsonl` and print
//! a table; `elevenlabs-live-example`, whose example prints one `first audio after` line per
//! output format; `cascade-local-adapters`, whose tests print one line per turn and per test. Not
//! read because nothing tells whether its run held: `cascade-talk`, whose reason `NOT_READ` gives.

/// For each measure the program reads: the assessment, the boundary of the measure, the text of
/// the one line that holds the value, and the text after which the value stands, where it does
/// not stand right after the first.
const LABELS: &[(&str, &str, &str, Option<&str>)] = &[
    (
        "cartesia-live",
        "end_of_utterance_to_first_audio",
        "first audio after",
        None,
    ),
    (
        "deepgram-live",
        "start_to_first_event",
        "first event after",
        None,
    ),
    (
        "elevenlabs-cancel-probe",
        "text_sent_to_first_audio",
        "P1 result:",
        Some("first audio"),
    ),
    (
        "llama-server-live",
        "start_to_first_delta",
        "first after",
        None,
    ),
    (
        "llama-server-live",
        "start_to_completed",
        "completed after",
        None,
    ),
    (
        "llama-server-live-cancellation",
        "start_to_first_delta",
        "first delta",
        None,
    ),
    (
        "llama-server-live-cancellation",
        "cancel_to_idle_slots",
        "no slot processing",
        None,
    ),
    (
        "llama-server-live-tools",
        "start_to_tool_proposal",
        "proposal:",
        Some(" after "),
    ),
    (
        "llama-server-live-tools",
        "start_to_completed",
        "completed after",
        None,
    ),
    ("openai-gpt-live", "connect_call", "started in", None),
    ("openai-gpt-live", "close_call", "closed in", None),
    (
        "openai-speech-live",
        "start_to_first_audio",
        "first frame after",
        None,
    ),
];

/// The assessments whose output holds each value on one line, and that the program does not read
/// because nothing in the run tells whether it held.
const NOT_READ: &[(&str, &str)] = &[(
    "cascade-talk",
    "its run ends by an interrupt (Ctrl-C), and its exit status does not tell a completed run from a failed one: the example exits 0 also when its run ended Failed",
)];

/// Where the program finds the measure of `boundary` of `assessment`: the text of its line and the
/// text its value follows.
pub(super) fn label(
    assessment: &str,
    boundary: &str,
) -> Option<(&'static str, Option<&'static str>)> {
    LABELS
        .iter()
        .find(|(of, at, _, _)| *of == assessment && *at == boundary)
        .map(|(_, _, line, after)| (*line, *after))
}

/// Why the program reads no record of `assessment`, for one whose lines it could find.
pub(super) fn not_read(assessment: &str) -> Option<&'static str> {
    NOT_READ
        .iter()
        .find(|(of, _)| *of == assessment)
        .map(|(_, why)| *why)
}

/// `output` without its ANSI escape sequences: a control sequence, `ESC [` up to its final
/// character; an operating system command, `ESC ]` up to `BEL` or `ESC \`; and any other `ESC`
/// with the character after it.
pub(super) fn plain(output: &str) -> String {
    let mut text = String::with_capacity(output.len());
    let mut characters = output.chars();
    while let Some(character) = characters.next() {
        if character != '\u{1b}' {
            text.push(character);
            continue;
        }
        match characters.next() {
            Some('[') => {
                let _ = characters
                    .by_ref()
                    .find(|character| ('@'..='~').contains(character));
            }
            Some(']') => {
                let end = characters
                    .by_ref()
                    .find(|character| matches!(character, '\u{7}' | '\u{1b}'));
                if end == Some('\u{1b}') {
                    let _ = characters.next();
                }
            }
            _ => {}
        }
    }
    text
}

/// The lines of `output` that say a test was skipped.
pub(super) fn skipped(output: &str) -> Vec<&str> {
    output
        .lines()
        .filter(|line| line.trim_start().starts_with("skipped:"))
        .collect()
}

/// The status words of cargo-nextest for a test that did not pass: `TMT` is the word of 0.9.143
/// for a test ended at its timeout, also after `TRY <n>`.
const NEXTEST_FAILED: [&str; 12] = [
    "FAIL",
    "TMT",
    "TIMEOUT",
    "ABORT",
    "LEAK-FAIL",
    "SIGSEGV",
    "SIGABRT",
    "SIGKILL",
    "SIGTERM",
    "SIGBUS",
    "SIGILL",
    "SIGFPE",
];

/// The status word and the test of a status line of cargo-nextest, as 0.9.143 prints them:
/// `PASS [   0.075s] (1/4) <binary> <test>`, `TRY 2 FAIL [   0.029s] (2/4) <binary> <test>`,
/// `FLAKY 2/2 [   0.024s] (3/4) <binary> <test>`.
fn status_line(line: &str) -> Option<(&str, &str)> {
    let words: Vec<&str> = line.split_whitespace().take(4).collect();
    let status = match words.as_slice() {
        ["TRY", number, status, time, ..]
            if number.bytes().all(|byte| byte.is_ascii_digit()) && time.starts_with('[') =>
        {
            *status
        }
        [status, time, ..] if time.starts_with('[') => *status,
        [status, tries, time, ..] if tries.contains('/') && time.starts_with('[') => *status,
        _ => return None,
    };
    if !status
        .bytes()
        .all(|byte| byte.is_ascii_uppercase() || byte == b'-')
    {
        return None;
    }
    let (_, after) = line.split_once(']')?;
    let after = after.trim_start();
    let test = if after.starts_with('(') {
        after.split_once(") ").map_or("", |(_, test)| test)
    } else {
        after
    };
    Some((status, test.trim()))
}

/// The counts of a `Summary` line of cargo-nextest after its time, `4 tests run: 2 passed (1
/// flaky), 1 failed, 1 timed out, 100 skipped`: the tests passed, and whether any other count but
/// `skipped` is above 0 or the run did not run all its tests (`2/4 tests run`). `None` for a line
/// of another form.
fn summary(counts: &str) -> Option<(u64, bool)> {
    let (run, counted) = counts.split_once(':')?;
    let mut words = run.split_whitespace();
    let ran = words.next()?;
    if !matches!(words.next(), Some("test" | "tests")) || words.next() != Some("run") {
        return None;
    }
    let mut failing = ran.split_once('/').is_some_and(|(done, all)| done != all);
    let mut outside = String::new();
    let mut depth = 0_usize;
    for character in counted.chars() {
        match character {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 => outside.push(character),
            _ => {}
        }
    }
    let mut passed = 0;
    for item in outside.split(',') {
        let mut words = item.split_whitespace();
        let count: u64 = words.next()?.parse().ok()?;
        match words.collect::<Vec<_>>().join(" ").as_str() {
            "" => return None,
            "passed" => passed = count,
            "skipped" => {}
            _ => failing |= count > 0,
        }
    }
    Some((passed, failing))
}

/// The test whose thread `line` says panicked: `thread '<name>' (<id>) panicked at`, as the
/// pinned rustc prints it, or `thread '<name>' panicked at`.
fn panicked(line: &str) -> Option<&str> {
    let (_, rest) = line.split_once("thread '")?;
    let (thread, after) = rest.split_once('\'')?;
    let mut after = after.trim_start();
    if let Some(inner) = after.strip_prefix('(') {
        let (id, rest) = inner.split_once(')')?;
        if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        after = rest.trim_start();
    }
    after.starts_with("panicked").then_some(thread)
}

/// Whether `line` is the line `cargo test` starts the tests of one binary with,
/// `running 3 tests`.
fn running(line: &str) -> bool {
    line.strip_prefix("running ")
        .and_then(|rest| {
            rest.strip_suffix(" tests")
                .or_else(|| rest.strip_suffix(" test"))
        })
        .is_some_and(|count| !count.is_empty() && count.bytes().all(|byte| byte.is_ascii_digit()))
}

/// What the output of a test command says of its run: `Ok` when its closing line says that its
/// tests passed and no line says that a test failed, or else one line for each reason it held
/// not. `measures` are the texts of the lines the program reads values from.
///
/// For `cargo test`, the tests of each binary run from a `running` line to their own `test
/// result:` line. A `running` line without its `test result:` line before the next `running`
/// line or the end is a run that did not end. A pass is read from the `test result:` line of the
/// tests during which the measures were printed; where no closed tests hold them, every
/// `test result:` line must be `ok.` and one must count a test passed. Where cargo-nextest
/// closes the run with its `Summary` line, that line decides.
pub(super) fn verdict(output: &str, measures: &[&str]) -> Result<(), Vec<String>> {
    let mut named: Vec<&str> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    let mut said: Vec<&str> = Vec::new();
    let mut threads: Vec<&str> = Vec::new();
    let mut closed = false;
    let mut passed = false;
    let mut nextest: Option<bool> = None;
    // The tests of the binary that started last and have not closed: whether they printed a
    // measure.
    let mut open: Option<bool> = None;
    let mut unclosed = false;
    let mut measured: Option<bool> = None;
    for line in output.lines() {
        let trimmed = line.trim();
        if running(trimmed) {
            unclosed |= open.is_some();
            open = Some(false);
            continue;
        }
        if let Some(printed) = open.as_mut()
            && measures.iter().any(|measure| line.contains(measure))
        {
            *printed = true;
        }
        if let Some(result) = trimmed.strip_prefix("test result: ") {
            closed = true;
            let printed = open.take() == Some(true);
            match result.strip_prefix("ok. ") {
                Some(counts) => {
                    let count = counts
                        .split_whitespace()
                        .next()
                        .and_then(|count| count.parse::<u64>().ok());
                    let ran = count.is_some_and(|count| count > 0);
                    passed |= ran;
                    if printed {
                        measured = Some(measured.unwrap_or(false) || ran);
                    }
                }
                None => said.push(trimmed),
            }
            continue;
        }
        if trimmed.starts_with("error: test failed")
            || trimmed.starts_with("error: test run failed")
        {
            said.push(trimmed);
            continue;
        }
        if let Some((_, counts)) = trimmed
            .strip_prefix("Summary [")
            .and_then(|rest| rest.split_once(']'))
        {
            closed = true;
            match summary(counts) {
                Some((count, false)) => nextest = Some(nextest.unwrap_or(false) || count > 0),
                _ => {
                    nextest = Some(false);
                    said.push(trimmed);
                }
            }
            continue;
        }
        if let Some((name, after)) = trimmed
            .strip_prefix("test ")
            .and_then(|rest| rest.split_once(" ... "))
        {
            named.push(name);
            if after.trim_end().ends_with("FAILED") {
                failed.push(name.to_owned());
            }
        }
        if let Some(thread) = panicked(line) {
            threads.push(thread);
        }
        if let Some((status, test)) = status_line(trimmed)
            && NEXTEST_FAILED.contains(&status)
        {
            failed.push(test.to_owned());
        }
    }
    let mut lines: Vec<String> = Vec::new();
    for name in &failed {
        once(
            &mut lines,
            format!("the output says the test {name} failed"),
        );
    }
    for thread in threads {
        if named.contains(&thread) && !failed.iter().any(|name| name == thread) {
            once(
                &mut lines,
                format!("the output says the test {thread} panicked"),
            );
        }
    }
    for line in said {
        once(&mut lines, format!("the output says `{line}`"));
    }
    unclosed |= open.is_some();
    if !closed || (nextest.is_none() && unclosed) {
        lines.push(
            "the output ends before the closing line of its run, a `test result:` line of cargo test or a `Summary` line of cargo-nextest: the run did not end, or the output is not all of it"
                .to_owned(),
        );
    } else if lines.is_empty() {
        let none = match (nextest, measured) {
            (Some(false), _) => Some("the closing line of its run"),
            (None, Some(false)) => {
                Some("the `test result:` line of the tests that printed the measures")
            }
            (None, None) if !passed => Some("the closing line of its run"),
            _ => None,
        };
        if let Some(line) = none {
            lines.push(format!("{line} says that no test passed"));
        }
    }
    if lines.is_empty() { Ok(()) } else { Err(lines) }
}

/// Adds `line` to `lines` unless they hold it already.
fn once(lines: &mut Vec<String>, line: String) {
    if !lines.contains(&line) {
        lines.push(line);
    }
}

/// The value of the measure whose line holds `line` and whose value follows `after`, in
/// milliseconds, or why `output` gives none.
pub(super) fn value(output: &str, line: &str, after: Option<&str>) -> Result<String, String> {
    let lines: Vec<&str> = output.lines().filter(|text| text.contains(line)).collect();
    let found = match lines.as_slice() {
        [] => {
            return Err(match after {
                None => format!("no line of the output holds `{line}`"),
                Some(after) => {
                    format!("no line of the output holds `{line}`; its value follows `{after}`")
                }
            });
        }
        [one] => *one,
        many => {
            return Err(format!(
                "{} lines of the output hold `{line}`, and the measure takes one value per run",
                many.len()
            ));
        }
    };
    let (_, rest) = found.split_once(line).unwrap_or_default();
    let marker = after.unwrap_or(line).trim();
    let rest = match after {
        None => Some(rest),
        Some(after) => rest.rfind(after).map(|at| &rest[at + after.len()..]),
    };
    let mut words = rest.unwrap_or_default().split_whitespace();
    let first = words
        .next()
        .map(|word| {
            word.trim_start_matches('[')
                .trim_end_matches([',', ';', ':', '.', ']'])
        })
        .unwrap_or_default();
    if first == "None" {
        return Err(format!("the line `{found}` prints None"));
    }
    let inner = first
        .strip_prefix("Some(")
        .and_then(|inner| inner.strip_suffix(')'))
        .unwrap_or(first);
    let nanos = debug_duration(inner).or_else(|| {
        let unit = words
            .next()?
            .trim_end_matches([',', ';', ':', '.', ']', ')']);
        if unit == "ms" {
            super::scaled(inner, 6)
        } else {
            None
        }
    });
    nanos
        .map(milliseconds)
        .ok_or_else(|| format!("the line `{found}` holds no duration after `{marker}`"))
}

/// The nanoseconds of the debug form of a `Duration`.
fn debug_duration(text: &str) -> Option<u128> {
    for (suffix, scale) in [("ns", 0), ("µs", 3), ("μs", 3), ("ms", 6), ("s", 9)] {
        if let Some(number) = text.strip_suffix(suffix) {
            return super::scaled(number, scale);
        }
    }
    None
}

/// `nanos` as milliseconds: the whole number and the fraction down to the nanosecond, without
/// trailing zeros.
fn milliseconds(nanos: u128) -> String {
    let whole = nanos / 1_000_000;
    let fraction = nanos % 1_000_000;
    if fraction == 0 {
        return whole.to_string();
    }
    let digits = format!("{fraction:06}");
    let digits = digits.trim_end_matches('0');
    format!("{whole}.{digits}")
}
