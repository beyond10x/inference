//! Security pass on story:elevenlabs-tts-provider-cancel: `Endpoint::from_env` and what its
//! refusal repeats. The invariant under check: the API key never appears in an error or in Debug
//! output, including through `Endpoint::from_env`. The refusal of `ELEVENLABS_RESIDENCY` repeats
//! the variable's value verbatim, so a key set in that variable (the two variables sit side by
//! side in every setup this crate documents) is printed by every caller: the live example's
//! status-2 message, and the panics of `stt_live` and `tts_cancel_live`. The value also goes out
//! unescaped, so a value holding a line break writes a second line into whatever log takes the
//! error. Other refusals of untrusted text, such as `InvalidVoiceId`, carry no value.
//!
//! The crate forbids `unsafe`, so a case cannot set the variable in its own process: each parent
//! case runs the ignored child case in a child process of this test binary with the variable set.

use std::process::Command;

use inference_elevenlabs::Endpoint;

/// The variable the child case reads, as the crate names it.
const VARIABLE: &str = inference_elevenlabs::RESIDENCY_VARIABLE;

/// A value shaped as a key, the fixture key of the suite.
const KEY_SHAPED: &str = "fixture-key-d41d8cd98f00b204";

/// Runs the child case with the variable set to `value` and returns its standard output.
fn child_output(value: &str) -> String {
    let output = Command::new(std::env::current_exe().expect("the test binary"))
        .args([
            "--ignored",
            "--exact",
            "child_prints_the_refusal_of_from_env",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(VARIABLE, value)
        .output()
        .expect("the child process runs");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "the child case failed: {stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

/// The text between `start` and `>>END` in `stdout`.
fn between<'a>(stdout: &'a str, start: &str) -> &'a str {
    let from = stdout
        .find(start)
        .unwrap_or_else(|| panic!("the child printed no {start}: {stdout}"))
        + start.len();
    let to = stdout[from..]
        .find(">>END")
        .unwrap_or_else(|| panic!("the child printed no end marker: {stdout}"));
    &stdout[from..from + to]
}

/// Not a case of its own: run by the parent cases, in a child process with the variable set.
#[test]
#[ignore = "a child of the parent cases of this file, run with ELEVENLABS_RESIDENCY set"]
fn child_prints_the_refusal_of_from_env() {
    let error = Endpoint::from_env().expect_err("the value names no residency");
    println!("DISPLAY<<{error}>>END");
    println!("DEBUG<<{error:?}>>END");
}

/// A key set in `ELEVENLABS_RESIDENCY` is refused, and neither the refusal's Display nor its
/// Debug output repeats it.
#[test]
fn a_key_in_the_residency_variable_is_not_repeated_by_the_refusal() {
    let stdout = child_output(KEY_SHAPED);
    let display = between(&stdout, "DISPLAY<<");
    let debug = between(&stdout, "DEBUG<<");
    assert!(
        display.contains(VARIABLE),
        "the refusal names the variable: {display}"
    );
    assert!(
        !display.contains(KEY_SHAPED),
        "the refusal's Display repeats the value of {VARIABLE}: {display}"
    );
    assert!(
        !debug.contains(KEY_SHAPED),
        "the refusal's Debug output repeats the value of {VARIABLE}: {debug}"
    );
}

/// A value holding a line break does not put a line break into the refusal's Display, so the
/// refusal stays one line of any log that takes it.
#[test]
fn the_refusal_of_the_residency_variable_stays_one_line() {
    let stdout = child_output("eu\nERROR forged line");
    let display = between(&stdout, "DISPLAY<<");
    assert!(
        !display.contains('\n'),
        "the refusal's Display carries a line break from the value: {display:?}"
    );
}
