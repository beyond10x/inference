//! Adversary pass 2 on story:elevenlabs-tts-provider-cancel: `Endpoint::from_env`, the one
//! function that reads `ELEVENLABS_RESIDENCY`, is called by no case that runs without feature
//! `live`; the unit tests in `src/config.rs` call the private `from_residency_value` and never
//! the read of the variable. So a `from_env` that read another variable, or took an unset one for
//! an error, would pass the suite.
//!
//! This case runs its own test binary again as a child, with the variable unset, set to each
//! name and set to a wrong value, and checks what `from_env` answers in each. The workspace
//! forbids `unsafe`, so the variable is set on the child process, never in this one.

use std::process::Command;

use inference_elevenlabs::{ConfigError, Endpoint, RESIDENCY_VARIABLE};

/// Set on the child only; the child case does nothing without it.
const CHILD: &str = "ADVERSARY2_U2_RESIDENCY_CHILD";

/// In the child: prints what `Endpoint::from_env` answers, after `RESULT ` on one line.
#[test]
fn child_prints_what_from_env_answers() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    match Endpoint::from_env() {
        Ok(endpoint) => println!("RESULT ok {}", endpoint.url),
        Err(ConfigError::Residency(value)) => println!("RESULT residency {value}"),
        Err(other) => println!("RESULT other {other:?}"),
    }
}

/// What `from_env` answers in a child whose `ELEVENLABS_RESIDENCY` is `value`, unset for `None`.
fn from_env_with(value: Option<&str>) -> String {
    let mut command = Command::new(std::env::current_exe().expect("the test binary"));
    command
        .args([
            "--exact",
            "child_prints_what_from_env_answers",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, "1");
    match value {
        Some(value) => command.env(RESIDENCY_VARIABLE, value),
        None => command.env_remove(RESIDENCY_VARIABLE),
    };
    let output = command.output().expect("the child runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find_map(|line| {
            line.split_once("RESULT ")
                .map(|(_, rest)| rest.trim().to_owned())
        })
        .unwrap_or_else(|| {
            panic!(
                "the child printed no result for {value:?}: {stdout}{}",
                String::from_utf8_lossy(&output.stderr)
            )
        })
}

#[test]
fn from_env_reads_elevenlabs_residency_and_unset_is_the_global_host() {
    assert_eq!(RESIDENCY_VARIABLE, "ELEVENLABS_RESIDENCY");
    for (value, expected) in [
        (None, "ok wss://api.elevenlabs.io"),
        (Some("global"), "ok wss://api.elevenlabs.io"),
        (Some("us"), "ok wss://api.us.elevenlabs.io"),
        (Some("eu"), "ok wss://api.eu.residency.elevenlabs.io"),
        (Some("in"), "ok wss://api.in.residency.elevenlabs.io"),
        (Some("sg"), "ok wss://api.sg.residency.elevenlabs.io"),
        (Some("europe"), "residency `europe`"),
    ] {
        assert_eq!(
            from_env_with(value),
            expected,
            "{RESIDENCY_VARIABLE}={value:?}"
        );
    }
}
