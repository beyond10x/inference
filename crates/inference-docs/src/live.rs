//! `live`: the lane `task quality-live` runs on this machine. It runs the keyed live measures of
//! the plan for OpenAI and ElevenLabs, each from the command its assessment names, and writes the
//! record of each with `inference-docs assess --class local`.
//!
//! It first prints the hosted calls of one run, per provider. Without `YES=1` in its environment
//! it stops there with exit 2 and starts nothing: no build, no command, no `secret-tool`. With
//! `YES=1` it reads each provider's key with `secret-tool lookup service b10x-secrets username
//! <key>`, once, only for a provider with a measure to run, and within a bound (10 s unless
//! `--key-timeout-ms` names another). A key that is missing or empty, or a `secret-tool` that
//! cannot be started or does not answer in time, skips its provider's measures with a line naming
//! the key and the measures; `ELEVENLABS_VOICE_ID` unset skips both ElevenLabs measures, which both
//! read it, by name, and their key is not read. The lane goes on with the others.
//!
//! Each measure is then built and run apart, because a key stays on this machine:
//!
//! - The build goes to the build pool as plain `cargo`, without either key variable: the plan's
//!   `cargo test <arguments> -- <test arguments>` is built as `cargo test --no-run
//!   --message-format=json-render-diagnostics <arguments>`, and its `cargo run <arguments>` as
//!   `cargo build --message-format=json-render-diagnostics <arguments>`. The executable is the one
//!   cargo's JSON names for the test of `--test` or the example of `--example`. A pool that builds
//!   on a node and copies a build's executables back leaves it here; when it is not here and
//!   lies under `target/` of the root, one job of [`FETCH`], `slot --fetch <path> true`, copies
//!   it back without a key; an executable still not here is refused.
//! - The run is that executable under [`RUN_PREFIX`], `slot --local`, which starts it here with
//!   the environment it is given, never on a build node. The executable gets the arguments the
//!   plan's command gives after `--`, and starts where cargo would start it: a test in the
//!   directory of its package's manifest, an example in the root. Its environment has neither key
//!   variable but its own provider's key, and for OpenAI `INFERENCE_OPENAI_KEY_SOURCE=env`.
//!
//! The key is never printed, passed as an argument or written: a line of a run's output that holds
//! it is kept with `[key]` in its place. The output of each run, standard output and error in one
//! file, is kept at `.ratchet/runs/<run id>/<assessment>.out` and its record written to
//! `.ratchet/records/<plan>/<assessment>/<run id>.evidence.json`. The run id is the time of the
//! start in UTC without `-` and `:`, with `-02` to `-99` after it for a later run in the same
//! second, so that the ids sort in the order of the runs. `inference-docs assess` reads no record
//! of `elevenlabs-live-example`, whose example prints its measure once per output format: its
//! output is kept and no record is written.
//!
//! Exit 0 when every measure that ran gave its record (or, for the one without a reader, exited
//! 0), 1 when one did not, 2 without `YES=1`, 3 when the plan cannot be read or the directory of
//! the run cannot be made.

use std::{
    env, fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, ExitCode, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
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
    /// How long `secret-tool` may take to give a key, in milliseconds; past it the provider's
    /// measures are skipped.
    #[arg(long, default_value_t = 10_000)]
    key_timeout_ms: u64,
}

/// The `service` attribute of every key in the Secret Service.
const SERVICE: &str = "b10x-secrets";
/// The voice both ElevenLabs commands read.
const VOICE: &str = "ELEVENLABS_VOICE_ID";
/// Every key variable a command of the lane reads; each build and run starts without all of them.
const KEY_VARIABLES: [&str; 2] = ["OPENAI_API_KEY", "ELEVENLABS_API_KEY"];
/// What stands in a kept output where the command printed its key.
const REDACTED: &[u8] = b"[key]";
/// What every run starts with: it runs the executable on this machine, outside the build pool and
/// its build nodes, with the environment the lane gives it.
const RUN_PREFIX: [&str; 2] = ["slot", "--local"];
/// The message format of every build: cargo's JSON on standard output, its diagnostics rendered.
const BUILD_FORMAT: &str = "--message-format=json-render-diagnostics";
/// What brings an executable a build left on a build node here, its path from the root between
/// the two: a job of its own that runs `true` and copies the path back after it.
const FETCH: [&str; 3] = ["slot", "--fetch", "true"];

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

/// Where a measure stands in the plan files, and the build and run its command gives.
struct Planned {
    /// The plan, as the name of its file without `.ratchet.yaml`.
    plan: String,
    project: String,
    build: Vec<String>,
    /// `test` or `example`, and its name: the artifact whose executable is run.
    kind: &'static str,
    name: String,
    /// The arguments of the executable: what the plan's command gives after `--`.
    arguments: Vec<String>,
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
    let bound = Duration::from_millis(arguments.key_timeout_ms);
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
        let key = match lookup(provider.key, bound) {
            Ok(key) => key,
            Err(why) => {
                let names: Vec<&str> = runnable.iter().map(|(m, _)| m.assessment).collect();
                println!("quality-live: skipped {}: {why}", names.join(", "));
                skipped += runnable.len();
                continue;
            }
        };
        for (measure, plan) in runnable {
            match run_measure(root, &run, provider, measure, &plan, &key) {
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

/// The plan, project, build and run of `assessment` in the plan files `ratchet.toml` names.
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
        let (build, kind, artifact, arguments) =
            steps(command).map_err(|why| format!("{path}: {assessment}: {why}"))?;
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
            build,
            kind,
            name: artifact,
            arguments,
        });
    }
    Err(format!(
        "no plan file of ratchet.toml declares the assessment {assessment}"
    ))
}

/// The build of a plan's command, the kind and name of the artifact it runs, and the arguments of
/// its executable: `cargo test <arguments> [-- <test arguments>]` or `cargo run <arguments> [--
/// <arguments>]`, the second with `--example <name>`.
type Steps = (Vec<String>, &'static str, String, Vec<String>);

fn steps(command: &str) -> Result<Steps, String> {
    let words: Vec<&str> = command.split_whitespace().collect();
    let (cargo, rest) = match words.as_slice() {
        ["cargo", subcommand, rest @ ..] => (*subcommand, rest),
        _ => return Err(format!("the command `{command}` is not a cargo command")),
    };
    let (options, after) = match rest.iter().position(|word| *word == "--") {
        Some(at) => (&rest[..at], &rest[at + 1..]),
        None => (rest, &[][..]),
    };
    let named = |flag: &str| {
        options
            .iter()
            .position(|word| *word == flag)
            .and_then(|at| options.get(at + 1))
            .map(|name| (*name).to_owned())
    };
    let (mut build, kind, name) = match cargo {
        "test" => (
            vec!["cargo", "test", "--no-run", BUILD_FORMAT],
            "test",
            named("--test").ok_or(format!("the command `{command}` names no --test"))?,
        ),
        "run" => (
            vec!["cargo", "build", BUILD_FORMAT],
            "example",
            named("--example").ok_or(format!("the command `{command}` names no --example"))?,
        ),
        other => {
            return Err(format!(
                "the command `{command}` is cargo {other}, neither cargo test nor cargo run"
            ));
        }
    };
    build.extend(options.iter().copied());
    Ok((
        build.into_iter().map(str::to_owned).collect(),
        kind,
        name,
        after.iter().map(|word| (*word).to_owned()).collect(),
    ))
}

/// The id of the `attempt`th run that starts in the second `base` names: `base` for the first,
/// `base-02` to `base-99` after it, which sort after `base` and in their order.
fn run_id(base: &str, attempt: u32) -> String {
    if attempt == 1 {
        base.to_owned()
    } else {
        format!("{base}-{attempt:02}")
    }
}

/// Makes `.ratchet/runs/<run id>/` and returns the id.
fn run_directory(root: &Path) -> Result<String, String> {
    let runs = root.join(".ratchet/runs");
    fs::create_dir_all(&runs)
        .map_err(|error| format!("{}: cannot be made: {error}", runs.display()))?;
    let base: String = crate::assess::utc(now())
        .chars()
        .filter(|character| !matches!(character, '-' | ':'))
        .collect();
    for attempt in 1..=99 {
        let id = run_id(&base, attempt);
        match fs::create_dir(runs.join(&id)) {
            Ok(()) => return Ok(id),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(format!(".ratchet/runs/{id}: cannot be made: {error}")),
        }
    }
    Err(format!(
        ".ratchet/runs/: 99 runs started in the second {base} already"
    ))
}

/// Nanoseconds since 1970.
fn now() -> i128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| i128::try_from(since.as_nanos()).unwrap_or(0))
}

/// The key of `username` from the Secret Service, or why there is none. A `secret-tool` that has
/// not exited within `bound` is ended.
fn lookup(username: &str, bound: Duration) -> Result<String, String> {
    let mut child = Command::new("secret-tool")
        .args(["lookup", "service", SERVICE, "username", username])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("secret-tool cannot be started ({error})"))?;
    let deadline = Instant::now() + bound;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "secret-tool gave no answer for {username} within {bound:?}"
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("secret-tool cannot be waited for ({error})"));
            }
        }
    };
    let missing = || format!("the Secret Service gives no key for {username}");
    if !status.success() {
        return Err(missing());
    }
    let mut bytes = Vec::new();
    if let Some(mut stdout) = child.stdout.take() {
        stdout.read_to_end(&mut bytes).map_err(|_| missing())?;
    }
    let mut key = String::from_utf8(bytes).map_err(|_| missing())?;
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

/// Builds `planned` through the pool without a key and returns its executable and the directory
/// it runs in, or why there is none.
fn build(root: &Path, planned: &Planned) -> Result<(PathBuf, PathBuf), String> {
    let (program, rest) = planned.build.split_first().ok_or("the build is empty")?;
    let mut command = Command::new(program);
    command
        .args(rest)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    for variable in KEY_VARIABLES {
        command.env_remove(variable);
    }
    for provider in &PROVIDERS {
        for (variable, _) in provider.also {
            command.env_remove(variable);
        }
    }
    let output = command
        .output()
        .map_err(|error| format!("its build cannot be started ({error})"))?;
    if !output.status.success() {
        return Err(format!(
            "its build exited {}",
            output.status.code().unwrap_or(-1)
        ));
    }
    let mut found = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if message.get("reason").and_then(serde_json::Value::as_str) != Some("compiler-artifact") {
            continue;
        }
        let Some(executable) = message
            .get("executable")
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        let target = message.get("target");
        let named = target
            .and_then(|target| target.get("name"))
            .and_then(serde_json::Value::as_str)
            == Some(planned.name.as_str());
        let of_kind = target
            .and_then(|target| target.get("kind"))
            .and_then(serde_json::Value::as_array)
            .is_some_and(|kinds| kinds.iter().any(|kind| kind.as_str() == Some(planned.kind)));
        if named && of_kind {
            let manifest = message
                .get("manifest_path")
                .and_then(serde_json::Value::as_str)
                .map(PathBuf::from);
            found.push((PathBuf::from(executable), manifest));
        }
    }
    let (executable, manifest) = match found.as_slice() {
        [one] => one.clone(),
        [] => {
            return Err(format!(
                "its build named no executable of the {} {}",
                planned.kind, planned.name
            ));
        }
        _ => {
            return Err(format!(
                "its build named more than one executable of the {} {}",
                planned.kind, planned.name
            ));
        }
    };
    let directory = match (planned.kind, manifest) {
        ("test", Some(manifest)) => manifest
            .parent()
            .map_or_else(|| root.to_path_buf(), Path::to_path_buf),
        _ => root.to_path_buf(),
    };
    Ok((executable, directory))
}

/// The executable a build named, on this machine. A build pool that builds on a node and copies
/// the executables of its build back leaves it here already. Otherwise one job of [`FETCH`]
/// copies it back, without a key, when it lies under `target/` of the root; an executable that is
/// still not here is refused.
fn here(root: &Path, id: &str, executable: &Path) -> Result<PathBuf, String> {
    if executable.is_file() {
        return Ok(executable.to_path_buf());
    }
    let not_here = format!(
        "the executable {} its build named is not on this machine",
        executable.display()
    );
    let top = std::path::absolute(root).map_err(|error| format!("{not_here} ({error})"))?;
    let Some(relative) = executable
        .strip_prefix(&top)
        .ok()
        .filter(|relative| relative.starts_with("target"))
        .map(|relative| relative.display().to_string())
    else {
        return Err(format!(
            "{not_here}, and is not under target/ of the root, where a fetch would bring it"
        ));
    };
    let [program, flag, job] = FETCH;
    println!("quality-live: {id}: fetch: {program} {flag} {relative} {job}");
    let mut command = Command::new(program);
    command
        .args([flag, relative.as_str(), job])
        .current_dir(root)
        .stdin(Stdio::null());
    for variable in KEY_VARIABLES {
        command.env_remove(variable);
    }
    for provider in &PROVIDERS {
        for (variable, _) in provider.also {
            command.env_remove(variable);
        }
    }
    let status = command.status().map_err(|error| {
        format!("{not_here}, and `{program} {flag}` cannot be started ({error})")
    })?;
    if !status.success() {
        return Err(format!(
            "{not_here}, and `{program} {flag} {relative} {job}` exited {}",
            status.code().unwrap_or(-1)
        ));
    }
    if !executable.is_file() {
        return Err(format!(
            "{not_here}, and `{program} {flag} {relative} {job}` did not bring it here"
        ));
    }
    Ok(executable.to_path_buf())
}

/// Builds and runs `measure` with the key in its run's environment, keeps its output and writes
/// its record.
fn run_measure(
    root: &Path,
    run: &str,
    provider: &Provider,
    measure: &Measure,
    planned: &Planned,
    key: &str,
) -> Ran {
    let id = measure.assessment;
    println!("quality-live: {id}: build: {}", planned.build.join(" "));
    let (executable, directory) = match build(root, planned)
        .and_then(|(executable, directory)| Ok((here(root, id, &executable)?, directory)))
    {
        Ok(built) => built,
        Err(why) => {
            println!("quality-live: {id}: {why}");
            return Ran::Failed;
        }
    };
    let mut words: Vec<String> = RUN_PREFIX.iter().map(|word| (*word).to_owned()).collect();
    words.push(executable.display().to_string());
    words.extend(planned.arguments.iter().cloned());
    println!("quality-live: {id}: run: {}", words.join(" "));
    let kept = format!(".ratchet/runs/{run}/{id}.out");
    let started = now();
    let status = match capture(&directory, &words, provider, key, &root.join(&kept)) {
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
    let records = format!(".ratchet/records/{}/{id}", planned.plan);
    if let Err(error) = fs::create_dir_all(root.join(&records)) {
        println!("quality-live: {id}: {records} cannot be made: {error}");
        return Ran::Failed;
    }
    let record = format!("{records}/{run}.evidence.json");
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

/// Runs `words` in `directory` with `key` in the environment of `provider` and neither key
/// variable besides, its standard output and error into `path` line by line, the key replaced;
/// returns its exit status (-1 when a signal ended it) and whether the key was replaced.
fn capture(
    directory: &Path,
    words: &[String],
    provider: &Provider,
    key: &str,
    path: &Path,
) -> Result<(i32, bool), String> {
    let (program, rest) = words.split_first().ok_or("the run is empty")?;
    let mut file = fs::File::create(path)
        .map_err(|error| format!("{}: cannot be written: {error}", path.display()))?;
    let (reader, writer) =
        std::io::pipe().map_err(|error| format!("no pipe for its output: {error}"))?;
    let mut command = Command::new(program);
    command
        .args(rest)
        .current_dir(directory)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_run_ids_of_one_second_sort_in_the_order_of_the_runs() {
        let base = "20261127T101647Z";
        let ids: Vec<String> = (1..=99).map(|attempt| run_id(base, attempt)).collect();
        assert_eq!(ids[0], base);
        assert_eq!(ids[1], format!("{base}-02"));
        assert_eq!(ids[98], format!("{base}-99"));
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(sorted, ids);
        assert!(run_id("20261127T101647Z", 99) < run_id("20261127T101648Z", 1));
    }

    #[test]
    fn the_plan_commands_give_a_build_without_a_key_and_the_arguments_of_the_executable() {
        let (build, kind, name, arguments) = steps(
            "cargo test -p inference-openai --features live,speech --test speech_live -- --ignored --nocapture",
        )
        .unwrap();
        assert_eq!(
            build.join(" "),
            "cargo test --no-run --message-format=json-render-diagnostics -p inference-openai --features live,speech --test speech_live"
        );
        assert_eq!((kind, name.as_str()), ("test", "speech_live"));
        assert_eq!(arguments, ["--ignored", "--nocapture"]);
        let (build, kind, name, arguments) =
            steps("cargo run -p inference-elevenlabs --features live --example live").unwrap();
        assert_eq!(
            build.join(" "),
            "cargo build --message-format=json-render-diagnostics -p inference-elevenlabs --features live --example live"
        );
        assert_eq!((kind, name.as_str()), ("example", "live"));
        assert!(arguments.is_empty());
        assert!(steps("cargo test -p inference-openai -- --ignored").is_err());
        assert!(steps("cargo bench -p inference-openai --bench x").is_err());
        assert!(steps("task test-run").is_err());
    }
}
