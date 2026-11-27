//! `live`: the lane `task quality-live` runs on this machine. It runs the keyed live measures of
//! the plan for OpenAI and ElevenLabs, each by the command its assessment names, and writes the
//! record of each with `inference-docs assess --class local`.
//!
//! It first prints the hosted calls of one run, per provider. Without `YES=1` in its environment
//! it stops there with exit 2 and starts nothing: no command, no `secret-tool`. With `YES=1` it
//! reads each provider's key with `secret-tool lookup service b10x-secrets username <key>`, once,
//! and only for a provider with a measure to run, and puts it into the environment of that
//! provider's commands only: every command starts without either key variable, and gets its own.
//! The key is never printed, passed as an argument or written: a line of a command's output that
//! holds it is kept with `[key]` in its place. A key that is missing or empty skips its provider's
//! measures with a line naming the key and the measures; `ELEVENLABS_VOICE_ID` unset skips both
//! ElevenLabs measures, which both read it, by name; the lane goes on with the others.
//!
//! The output of each command, standard output and error in one file, is kept at
//! `.ratchet/runs/<run id>/<assessment>.out` and its record written to
//! `.ratchet/records/<plan>/<assessment>/<run id>.evidence.json`. `inference-docs assess` reads no
//! record of `elevenlabs-live-example`, whose example prints its measure once per output format:
//! its output is kept and no record is written.
//!
//! Exit 0 when every measure that ran gave its record (or, for the one without a reader, exited
//! 0), 1 when one did not, 2 without `YES=1`, 3 when the plan cannot be read or the directory of
//! the run cannot be made.

use std::{
    env, fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, ExitCode, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use clap::Args;
use serde_norway::Value as Yaml;

/// The call of `live`.
#[derive(Args)]
pub(crate) struct Arguments {
    /// The root of the tree whose plan files name the commands; the outputs and records go
    /// under its `.ratchet/`.
    #[arg(long, default_value = ".")]
    root: PathBuf,
    /// A directory whose executable `<assessment>` runs in the place of the plan's command of
    /// each measure, for tests.
    #[arg(long)]
    stubs: Option<PathBuf>,
}

/// The `service` attribute of every key in the Secret Service.
const SERVICE: &str = "b10x-secrets";
/// The voice both ElevenLabs commands read.
const VOICE: &str = "ELEVENLABS_VOICE_ID";
/// Every key variable a command of the lane reads; each command starts without all of them.
const KEY_VARIABLES: [&str; 2] = ["OPENAI_API_KEY", "ELEVENLABS_API_KEY"];
/// What stands in a kept output where the command printed its key.
const REDACTED: &[u8] = b"[key]";

/// One provider: its key, the variable its commands read it from, and its measures.
struct Provider {
    name: &'static str,
    /// The `username` attribute of its key in the Secret Service.
    key: &'static str,
    variable: &'static str,
    /// Further variables its commands need to read the key from the environment.
    also: &'static [(&'static str, &'static str)],
    measures: &'static [Measure],
}

/// One measure: its assessment, the hosted calls of one run and what they are.
struct Measure {
    assessment: &'static str,
    calls: u32,
    what: &'static str,
    needs_voice: bool,
    /// Whether `inference-docs assess` reads a record of its output.
    recorded: bool,
}

const PROVIDERS: [Provider; 2] = [
    Provider {
        name: "OpenAI",
        key: "default/default/default/openai-api-key",
        variable: "OPENAI_API_KEY",
        also: &[("INFERENCE_OPENAI_KEY_SOURCE", "env")],
        measures: &[
            Measure {
                assessment: "openai-speech-live",
                calls: 1,
                what: "1 request",
                needs_voice: false,
                recorded: true,
            },
            Measure {
                assessment: "openai-gpt-live",
                calls: 3,
                what: "3 sessions, billed per second of session",
                needs_voice: false,
                recorded: true,
            },
        ],
    },
    Provider {
        name: "ElevenLabs",
        key: "default/default/default/elevenlabs-api-key",
        variable: "ELEVENLABS_API_KEY",
        also: &[],
        measures: &[
            Measure {
                assessment: "elevenlabs-live-example",
                calls: 2,
                what: "2 connections",
                needs_voice: true,
                recorded: false,
            },
            Measure {
                assessment: "elevenlabs-cancel-probe",
                calls: 4,
                what: "4 connections",
                needs_voice: true,
                recorded: true,
            },
        ],
    },
];

/// Where a measure stands in the plan files.
struct Planned {
    /// The plan, as the name of its file without `.ratchet.yaml`.
    plan: String,
    project: String,
    command: String,
}

/// What became of one measure that ran.
enum Ran {
    Written,
    Kept,
    Failed,
}

/// Runs the lane: the count, and with `YES=1` the measures.
pub(crate) fn run(arguments: &Arguments) -> ExitCode {
    match lane(arguments) {
        Ok(code) => ExitCode::from(code),
        Err(line) => {
            eprintln!("inference-docs live: {line}");
            ExitCode::from(3)
        }
    }
}

fn lane(arguments: &Arguments) -> Result<u8, String> {
    let root = &arguments.root;
    let mut planned = Vec::new();
    for provider in &PROVIDERS {
        for measure in provider.measures {
            planned.push(planned_in(root, measure.assessment)?);
        }
    }
    let voice = env::var(VOICE).is_ok_and(|voice| !voice.trim().is_empty());
    print!("{}", count(voice));
    if env::var("YES").as_deref() != Ok("1") {
        println!(
            "quality-live: called nothing; with the owner's yes to this count, run `YES=1 task quality-live`"
        );
        return Ok(2);
    }
    let run = run_directory(root)?;
    let mut planned = planned.into_iter();
    let (mut written, mut skipped, mut failed) = (0, 0, 0);
    for provider in &PROVIDERS {
        let mut runnable = Vec::new();
        let mut voiceless = Vec::new();
        for measure in provider.measures {
            let plan = planned.next().expect("one plan entry per measure");
            if measure.needs_voice && !voice {
                voiceless.push(measure.assessment);
            } else {
                runnable.push((measure, plan));
            }
        }
        if !voiceless.is_empty() {
            println!(
                "quality-live: skipped {}: {VOICE} is unset",
                voiceless.join(", ")
            );
            skipped += voiceless.len();
        }
        if runnable.is_empty() {
            continue;
        }
        let key = match lookup(provider.key) {
            Ok(key) => key,
            Err(why) => {
                let names: Vec<&str> = runnable.iter().map(|(m, _)| m.assessment).collect();
                println!("quality-live: skipped {}: {why}", names.join(", "));
                skipped += runnable.len();
                continue;
            }
        };
        for (measure, plan) in runnable {
            match run_measure(arguments, &run, provider, measure, &plan, &key) {
                Ran::Written => written += 1,
                Ran::Kept => {}
                Ran::Failed => failed += 1,
            }
        }
    }
    println!(
        "quality-live: {written} records written under .ratchet/records/, {skipped} measures skipped, {failed} failed"
    );
    Ok(u8::from(failed > 0))
}

/// The hosted calls of one run, per provider, as the lane prints them before it calls anything.
fn count(voice: bool) -> String {
    let mut text = String::from(
        "quality-live: the hosted calls of one run, per provider; each is billed by its provider\n",
    );
    for provider in &PROVIDERS {
        let runs = |measure: &&Measure| voice || !measure.needs_voice;
        let calls: u32 = provider
            .measures
            .iter()
            .filter(runs)
            .map(|measure| measure.calls)
            .sum();
        text.push_str(&format!(
            "  {}, key {}: {calls} calls\n",
            provider.name, provider.key
        ));
        for measure in provider.measures {
            let line = if !runs(&measure) {
                format!("skipped, {VOICE} is unset")
            } else if measure.recorded {
                measure.what.to_owned()
            } else {
                format!(
                    "{}; inference-docs assess reads no record of its output, which is kept",
                    measure.what
                )
            };
            text.push_str(&format!("    {}: {line}\n", measure.assessment));
        }
    }
    text
}

/// The plan, project and command of `assessment` in the plan files `ratchet.toml` names.
fn planned_in(root: &Path, assessment: &str) -> Result<Planned, String> {
    let read = |path: &str| {
        fs::read_to_string(root.join(path))
            .map_err(|error| format!("{path}: cannot be read: {error}"))
    };
    let config: serde_json::Value = toml::from_str(&read("ratchet.toml")?)
        .map_err(|error| format!("ratchet.toml: does not parse: {error}"))?;
    let paths = config
        .pointer("/spec/paths")
        .and_then(serde_json::Value::as_array)
        .ok_or("ratchet.toml: names no plan file under [spec] paths")?;
    for path in paths.iter().filter_map(serde_json::Value::as_str) {
        let plan: Yaml = serde_norway::from_str(&read(path)?)
            .map_err(|error| format!("{path}: does not parse: {error}"))?;
        let Some(found) = plan.get("assessments").and_then(|all| all.get(assessment)) else {
            continue;
        };
        let name = path.rsplit('/').next().unwrap_or(path);
        let command = found
            .get("observation")
            .and_then(|observation| observation.get("command"))
            .and_then(Yaml::as_str)
            .ok_or_else(|| format!("{path}: {assessment} names no command"))?;
        return Ok(Planned {
            plan: name
                .strip_suffix(".ratchet.yaml")
                .unwrap_or(name)
                .to_owned(),
            project: plan
                .get("project")
                .and_then(Yaml::as_str)
                .ok_or_else(|| format!("{path}: names no project"))?
                .to_owned(),
            command: command.to_owned(),
        });
    }
    Err(format!(
        "no plan file of ratchet.toml declares the assessment {assessment}"
    ))
}

/// Makes `.ratchet/runs/<run id>/`, the id the time of the start in UTC, and returns the id.
fn run_directory(root: &Path) -> Result<String, String> {
    let runs = root.join(".ratchet/runs");
    fs::create_dir_all(&runs)
        .map_err(|error| format!("{}: cannot be made: {error}", runs.display()))?;
    let base: String = crate::assess::utc(now())
        .chars()
        .filter(|character| !matches!(character, '-' | ':'))
        .collect();
    for attempt in 1.. {
        let id = if attempt == 1 {
            base.clone()
        } else {
            format!("{base}-{attempt}")
        };
        match fs::create_dir(runs.join(&id)) {
            Ok(()) => return Ok(id),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(format!(".ratchet/runs/{id}: cannot be made: {error}")),
        }
    }
    unreachable!("the attempts are unbounded")
}

/// Nanoseconds since 1970.
fn now() -> i128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| i128::try_from(since.as_nanos()).unwrap_or(0))
}

/// The key of `username` from the Secret Service, or why there is none.
fn lookup(username: &str) -> Result<String, String> {
    let output = Command::new("secret-tool")
        .args(["lookup", "service", SERVICE, "username", username])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("secret-tool cannot be started ({error})"))?;
    let missing = || format!("the Secret Service gives no key for {username}");
    if !output.status.success() {
        return Err(missing());
    }
    let mut key = String::from_utf8(output.stdout).map_err(|_| missing())?;
    while key.ends_with(['\n', '\r']) {
        key.pop();
    }
    if key.trim().is_empty() {
        return Err(missing());
    }
    Ok(key)
}

/// `line` with every occurrence of `key` replaced by [`REDACTED`], and whether there was one.
fn redact(line: &[u8], key: &[u8]) -> (Vec<u8>, bool) {
    let mut kept = Vec::with_capacity(line.len());
    let mut found = false;
    let mut at = 0;
    while at < line.len() {
        if !key.is_empty() && line[at..].starts_with(key) {
            kept.extend_from_slice(REDACTED);
            at += key.len();
            found = true;
        } else {
            kept.push(line[at]);
            at += 1;
        }
    }
    (kept, found)
}

/// Runs the command of `measure` with the key in its environment, keeps its output and writes
/// its record.
fn run_measure(
    arguments: &Arguments,
    run: &str,
    provider: &Provider,
    measure: &Measure,
    planned: &Planned,
    key: &str,
) -> Ran {
    let id = measure.assessment;
    let root = &arguments.root;
    let words: Vec<String> = match &arguments.stubs {
        Some(stubs) => vec![stubs.join(id).display().to_string()],
        None => planned
            .command
            .split_whitespace()
            .map(str::to_owned)
            .collect(),
    };
    println!("quality-live: {id}: {}", words.join(" "));
    let kept = format!(".ratchet/runs/{run}/{id}.out");
    let started = now();
    let status = match capture(root, &words, provider, key, &root.join(&kept)) {
        Ok((status, redacted)) => {
            if redacted {
                println!(
                    "quality-live: {id}: its output held its key; the kept output has [key] in its place"
                );
            }
            status
        }
        Err(why) => {
            println!("quality-live: {id}: {why}");
            return Ran::Failed;
        }
    };
    let ended = now();
    println!("quality-live: {id}: exited {status}; output kept at {kept}");
    if !measure.recorded {
        println!("quality-live: {id}: no record; inference-docs assess reads none of this output");
        return if status == 0 { Ran::Kept } else { Ran::Failed };
    }
    let directory = format!(".ratchet/records/{}/{id}", planned.plan);
    if let Err(error) = fs::create_dir_all(root.join(&directory)) {
        println!("quality-live: {id}: {directory} cannot be made: {error}");
        return Ran::Failed;
    }
    let record = format!("{directory}/{run}.evidence.json");
    let assess = env::current_exe().and_then(|program| {
        Command::new(program)
            .arg("assess")
            .args(["--project", &planned.project, "--assessment", id])
            .arg("--root")
            .arg(root)
            .args(["--class", "local", "--out"])
            .arg(root.join(&record))
            .arg("--from")
            .arg(root.join(&kept))
            .args(["--source", &kept])
            .args(["--started-at", &crate::assess::utc(started)])
            .args(["--ended-at", &crate::assess::utc(ended)])
            .args(["--exit-status", &status.to_string()])
            .stdin(Stdio::null())
            .output()
    });
    match assess {
        Ok(output) if output.status.success() => {
            println!("quality-live: {id}: record written to {record}");
            Ran::Written
        }
        Ok(output) => {
            println!(
                "quality-live: {id}: no record; inference-docs assess exited {}",
                output.status.code().unwrap_or(-1)
            );
            for line in String::from_utf8_lossy(&output.stderr).lines() {
                println!("  {line}");
            }
            Ran::Failed
        }
        Err(error) => {
            println!("quality-live: {id}: inference-docs assess cannot be started ({error})");
            Ran::Failed
        }
    }
}

/// Runs `words` under `root` with `key` in the environment of `provider`, its standard output and
/// error into `path` line by line, the key replaced; returns its exit status (-1 when a signal
/// ended it) and whether the key was replaced.
fn capture(
    root: &Path,
    words: &[String],
    provider: &Provider,
    key: &str,
    path: &Path,
) -> Result<(i32, bool), String> {
    let (program, rest) = words.split_first().ok_or("the plan's command is empty")?;
    let mut file = fs::File::create(path)
        .map_err(|error| format!("{}: cannot be written: {error}", path.display()))?;
    let (reader, writer) =
        std::io::pipe().map_err(|error| format!("no pipe for its output: {error}"))?;
    let mut command = Command::new(program);
    command
        .args(rest)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(
            writer
                .try_clone()
                .map_err(|error| format!("no pipe for its output: {error}"))?,
        )
        .stderr(writer);
    for variable in KEY_VARIABLES {
        command.env_remove(variable);
    }
    for (variable, _) in provider.also {
        command.env_remove(variable);
    }
    command.env(provider.variable, key);
    for (variable, value) in provider.also {
        command.env(variable, value);
    }
    let spawned = command.spawn();
    drop(command);
    let mut child = spawned.map_err(|error| format!("cannot be started ({error})"))?;
    let mut redacted = false;
    let mut lines = BufReader::new(reader);
    let mut line = Vec::new();
    loop {
        line.clear();
        match lines.read_until(b'\n', &mut line) {
            Ok(0) => break,
            Ok(_) => {
                let (kept, found) = redact(&line, key.as_bytes());
                redacted |= found;
                file.write_all(&kept)
                    .map_err(|error| format!("{}: cannot be written: {error}", path.display()))?;
            }
            Err(error) => return Err(format!("its output cannot be read: {error}")),
        }
    }
    let status = child
        .wait()
        .map_err(|error| format!("cannot be waited for ({error})"))?;
    Ok((status.code().unwrap_or(-1), redacted))
}
