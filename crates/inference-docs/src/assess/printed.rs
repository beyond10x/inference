//! The output a live test or an example printed: one line per measure, found by the text the
//! command prints before the value, and the value read as a duration in milliseconds.
//!
//! A value is the debug form of a `Duration` (`1.234s`, `12.5ms`, `870µs`, `15ns`), also inside
//! `Some(...)`, or a number followed by the word `ms`, as two commands print whole milliseconds.
//! `None` is no value, and neither is a line the output holds twice: a measure of a run takes one.
//!
//! Not handled: a value on another line than its label, a duration printed by `Display` or in
//! another unit word (`2 s`, `1.5 seconds`), and a run of the commands of `compare` and
//! `compare-cascade`, which write their values to `runs.jsonl` and print a table.

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
        "elevenlabs-live-example",
        "start_to_first_audio",
        "first audio after",
        None,
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
    (
        "cascade-talk",
        "turn_completion_to_first_playable_audio",
        "committed input to first playable audio",
        Some(":"),
    ),
    (
        "cascade-talk",
        "interruption_to_playback_stop",
        "interruption to playback stop",
        Some(":"),
    ),
    (
        "cascade-local-adapters",
        "turn_completion_to_first_playable_audio",
        "committed input to first playable audio",
        Some(":"),
    ),
    (
        "cascade-local-adapters",
        "interruption_to_playback_stop",
        "interruption to playback stop",
        Some(":"),
    ),
];

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

/// The lines of `output` that say a test was skipped.
pub(super) fn skipped(output: &str) -> Vec<&str> {
    output
        .lines()
        .filter(|line| line.trim_start().starts_with("skipped:"))
        .collect()
}

/// The value of the measure whose line holds `line` and whose value follows `after`, in
/// milliseconds, or why `output` gives none.
pub(super) fn value(output: &str, line: &str, after: Option<&str>) -> Result<String, String> {
    let lines: Vec<&str> = output.lines().filter(|text| text.contains(line)).collect();
    let found = match lines.as_slice() {
        [] => return Err(format!("no line of the output holds `{line}`")),
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
