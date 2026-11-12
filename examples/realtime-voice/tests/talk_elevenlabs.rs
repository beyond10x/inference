//! `talk --provider elevenlabs-cascade`: the provider table, the variables the provider needs
//! and the exit status when one is missing, and the ElevenLabs endpoints `talk` and `compare`
//! build, which follow `ELEVENLABS_RESIDENCY`.
//!
//! The workspace forbids `unsafe`, so no case sets a variable in this process: the table reads a
//! lookup, and the endpoint cases run this test binary again as a child with the variable set on
//! the child only.

use std::process::Command;
use std::sync::Arc;

use inference_elevenlabs::{ELEVENLABS_KEY_NAME, EnvironmentSource, RESIDENCY_VARIABLE};
use realtime_voice::elevenlabs;
use realtime_voice::talk::{Provider, Refused, USAGE_EXIT};

/// A lookup that answers `set` and nothing else.
fn lookup(set: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
    let set = set.to_vec();
    move |name| {
        set.iter()
            .find(|(variable, _)| *variable == name)
            .map(|(_, value)| (*value).to_owned())
    }
}

#[test]
fn talk_offers_the_elevenlabs_cascade_and_no_elevenlabs_agent() {
    let names: Vec<&str> = Provider::ALL
        .iter()
        .map(|provider| provider.name())
        .collect();
    assert_eq!(
        names,
        ["openai", "openai-live", "nova", "elevenlabs-cascade"]
    );
    for provider in Provider::ALL {
        assert_eq!(Provider::named(provider.name()), Some(provider));
        assert!(!provider.about().is_empty(), "{provider:?}");
    }
    assert_eq!(Provider::named("elevenlabs"), None);
    assert_eq!(USAGE_EXIT, 2);
}

#[test]
fn the_other_providers_need_no_variable_of_the_table() {
    for provider in [Provider::Openai, Provider::OpenaiLive, Provider::Nova] {
        assert_eq!(provider.required(lookup(&[])), Ok(()), "{provider:?}");
    }
}

#[cfg(feature = "local")]
#[test]
fn the_cascade_exits_2_naming_its_first_missing_variable() {
    let chain = [
        ("INFERENCE_LLAMA_SERVER_URL", "http://127.0.0.1:8080/v1"),
        ("INFERENCE_LLAMA_SERVER_MODEL", "qwen2.5-1.5b-instruct"),
        ("ELEVENLABS_VOICE_ID", "a-voice"),
    ];
    for missing in 0..chain.len() {
        let variable = chain[missing].0;
        let mut set = chain[..missing].to_vec();
        let refused = Provider::ElevenlabsCascade
            .required(lookup(&set))
            .expect_err(variable);
        assert_eq!(refused, Refused::Missing(variable));
        assert_eq!(refused.exit_code(), 2);
        assert!(refused.to_string().contains(variable), "{refused}");
        set.push((variable, "  "));
        assert_eq!(
            Provider::ElevenlabsCascade.required(lookup(&set)),
            Err(Refused::Missing(variable)),
            "a blank {variable} is unset"
        );
    }
    assert_eq!(Provider::ElevenlabsCascade.required(lookup(&chain)), Ok(()));
}

#[cfg(not(feature = "local"))]
#[test]
fn the_cascade_without_feature_local_exits_2_naming_the_feature() {
    let refused = Provider::ElevenlabsCascade
        .required(lookup(&[]))
        .expect_err("no LLM without feature local");
    assert_eq!(refused, Refused::Unbuilt { feature: "local" });
    assert_eq!(refused.exit_code(), 2);
    assert!(
        refused.to_string().contains("--features local"),
        "{refused}"
    );
}

/// Set on the child only; the child case does nothing without it.
const CHILD: &str = "TALK_ELEVENLABS_RESIDENCY_CHILD";

/// The variable the child's key source reads, so that no real key is needed or read.
const CHILD_KEY: &str = "TALK_ELEVENLABS_TEST_KEY";

/// The base URL in a `Debug` rendering holding one `url: "…"` field.
fn debug_url(debug: &str) -> String {
    debug
        .split_once("url: \"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .map_or_else(|| format!("no url in {debug}"), |(url, _)| url.to_owned())
}

/// In the child: prints the endpoint `talk` and `compare` read, and where the recogniser and
/// the synthesiser made over it connect, after `RESULT ` on one line.
#[test]
fn child_prints_the_endpoints_talk_and_compare_build() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    let endpoint = match elevenlabs::endpoint() {
        Ok(endpoint) => endpoint,
        Err(refused) => {
            println!("RESULT refused {} {refused}", refused.exit_code());
            return;
        }
    };
    let source = Arc::new(EnvironmentSource::new([(
        ELEVENLABS_KEY_NAME.to_owned(),
        CHILD_KEY.to_owned(),
    )]));
    let recogniser =
        elevenlabs::recogniser(endpoint.clone(), source.clone()).expect("the recogniser is made");
    let synthesiser = elevenlabs::synthesiser(endpoint.clone(), source, Some("a-voice"))
        .expect("the synthesiser is made");
    println!(
        "RESULT endpoint={} stt={} tts={}",
        endpoint.url,
        recogniser.request_url(),
        debug_url(&format!("{synthesiser:?}"))
    );
}

/// What the child prints with `ELEVENLABS_RESIDENCY` set to `value`, unset for `None`.
fn child_with(value: Option<&str>) -> String {
    let mut command = Command::new(std::env::current_exe().expect("the test binary"));
    command
        .args([
            "--exact",
            "child_prints_the_endpoints_talk_and_compare_build",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, "1")
        .env(CHILD_KEY, "not-a-key");
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
fn the_endpoints_talk_and_compare_build_follow_elevenlabs_residency() {
    for (value, host) in [
        (None, "wss://api.elevenlabs.io"),
        (Some(""), "wss://api.elevenlabs.io"),
        (Some("eu"), "wss://api.eu.residency.elevenlabs.io"),
        (Some("us"), "wss://api.us.elevenlabs.io"),
    ] {
        let result = child_with(value);
        let stt = format!(
            "{host}/v1/speech-to-text/realtime?model_id=scribe_v2_realtime&audio_format=pcm_16000\
             &commit_strategy=vad&"
        );
        assert!(
            result.starts_with(&format!("endpoint={host} stt={stt}")),
            "{value:?}: {result}"
        );
        assert!(
            result.ends_with(&format!(" tts={host}")),
            "{value:?}: {result}"
        );
    }
}

#[test]
fn a_residency_not_taken_exits_2_naming_the_variable_and_not_the_value() {
    let result = child_with(Some("mars"));
    assert!(result.starts_with("refused 2 "), "{result}");
    assert!(result.contains(RESIDENCY_VARIABLE), "{result}");
    assert!(!result.contains("mars"), "{result}");
}
