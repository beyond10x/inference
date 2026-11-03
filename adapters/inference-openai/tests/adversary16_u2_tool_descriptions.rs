//! Adversary pass 16 on story `duplex-tool-descriptions`: both OpenAI duplex bindings offer each
//! session tool with its own declared description, paired with its own name and in declaration
//! order, a description that needs escaping reaching the wire unchanged, and a tool declared
//! without one sent with no other text in its place than its name (`ess/domains/tools.yaml`,
//! `inference.tools.Tool.description`). Offline: the session configuration is built and rendered
//! without connecting.

#![cfg(all(feature = "gpt-live", feature = "duplex"))]

use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use inference_audio::{AudioEncoding, AudioFormat};
use inference_duplex::{SessionConfig, TurnDetection, TurnTuning};
use inference_openai::duplex::OpenAiRealtime;
use inference_openai::live::{Delegation, LIVE_MODEL, ResponsesBackend};
use inference_openai::live_duplex::{DEFAULT_BACKEND_MODEL, OpenAiLive};
use inference_openai::{
    CredentialError, CredentialFuture, CredentialSource, Endpoint, FunctionTool, OPENAI_KEY_NAME,
};
use inference_tools::{DeclareTool, SideEffectClass, Tool};
use serde_json::{Value, json};

struct NoSource;

impl CredentialSource for NoSource {
    fn resolve<'a>(&'a self, _name: &'a str) -> CredentialFuture<'a> {
        Box::pin(async { Err(CredentialError::Missing) })
    }
}

const ESCAPED: &str = "Books a \"table\" for the day it names;\nback\\slash, caf\u{e9} \u{1f37d}.";

fn pcm24() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(24_000).unwrap(),
        channels: NonZeroU16::MIN,
    }
}

fn declared(name: &str, description: &str) -> Tool {
    Tool::declare(DeclareTool {
        name: name.to_owned(),
        description: description.to_owned(),
        schema: json!({"type": "object", "properties": {"arg": {"type": "string"}}, "title": name}),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0
}

/// Three tools: described, declared without a description, described with text that needs
/// escaping on the wire.
fn three_tools() -> Vec<Tool> {
    vec![
        declared("weather", "The current weather in the city named."),
        declared("local_time", ""),
        declared("book", ESCAPED),
    ]
}

fn config(turn_detection: TurnDetection, auto: bool, tools: Vec<Tool>) -> SessionConfig {
    SessionConfig {
        voice: "marin".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: pcm24(),
        output_format: pcm24(),
        turn_detection,
        turn_tuning: TurnTuning::default(),
        auto_response: auto,
        auto_interrupt: auto,
        tools,
        idle_limit: Duration::from_secs(5),
    }
}

/// `(name, description)` of each rendered function tool, in order.
fn described(tools: &Value) -> Vec<(String, String)> {
    tools
        .as_array()
        .expect("a tools array")
        .iter()
        .map(|tool| {
            assert_eq!(tool["type"], "function", "{tool}");
            (
                tool["name"].as_str().expect("a name").to_owned(),
                tool["description"]
                    .as_str()
                    .expect("a description string")
                    .to_owned(),
            )
        })
        .collect()
}

/// A tool declared without a description is sent with no other text than its name: the empty
/// string or the name itself.
fn assert_bare(name: &str, description: &str) {
    assert!(
        description.is_empty() || description == name,
        "a tool declared without a description was sent {description:?}, text other than its name {name:?}"
    );
}

fn realtime() -> OpenAiRealtime {
    OpenAiRealtime::new(
        Endpoint::new("gpt-realtime-2.1").with_url("ws://127.0.0.1:1/v1/realtime"),
        Box::new(NoSource),
        OPENAI_KEY_NAME,
    )
}

fn live() -> OpenAiLive {
    OpenAiLive::new(
        Endpoint::live(LIVE_MODEL).with_url("ws://127.0.0.1:1/v1/live/sessions"),
        Box::new(NoSource),
        OPENAI_KEY_NAME,
    )
}

#[test]
fn realtime_session_update_pairs_each_description_with_its_own_tool() {
    let tools = three_tools();
    let session = realtime()
        .client_config(&config(TurnDetection::Disabled, false, tools.clone()))
        .expect("the configuration is admitted");
    let rendered = session.to_session_json();
    let offered = described(&rendered["tools"]);
    assert_eq!(offered.len(), 3, "{rendered}");
    assert_eq!(
        offered[0],
        (
            "weather".to_owned(),
            "The current weather in the city named.".to_owned()
        )
    );
    assert_eq!(offered[1].0, "local_time");
    assert_bare("local_time", &offered[1].1);
    assert_eq!(offered[2], ("book".to_owned(), ESCAPED.to_owned()));
    for (index, tool) in tools.iter().enumerate() {
        assert_eq!(
            rendered["tools"][index]["parameters"],
            *tool.schema(),
            "each tool keeps its own schema next to its description"
        );
    }
}

#[test]
fn realtime_session_update_text_carries_an_escaped_description_unchanged() {
    let session = realtime()
        .client_config(&config(TurnDetection::Disabled, false, three_tools()))
        .expect("the configuration is admitted");
    let wire = json!({"type": "session.update", "session": session.to_session_json()}).to_string();
    let parsed: Value = serde_json::from_str(&wire).expect("the frame is JSON");
    assert_eq!(parsed["session"]["tools"][2]["description"], ESCAPED);
}

#[test]
fn live_session_start_keeps_the_backend_tools_and_appends_each_described_session_tool() {
    let mut backend = ResponsesBackend::new(DEFAULT_BACKEND_MODEL);
    backend.tools.push(FunctionTool {
        name: "lookup".to_owned(),
        description: "The backend's own tool.".to_owned(),
        parameters: json!({"type": "object"}),
    });
    let model = live().with_backend(backend);
    let session = model
        .client_config(&config(TurnDetection::SemanticVad, true, three_tools()))
        .expect("the configuration is admitted");

    let Delegation::Responses(responses) = &session.delegation else {
        panic!("a session with tools delegates to a Responses backend")
    };
    let names: Vec<&str> = responses
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    assert_eq!(names, ["lookup", "weather", "local_time", "book"]);
    assert_eq!(responses.tools[0].description, "The backend's own tool.");
    assert_eq!(
        responses.tools[1].description,
        "The current weather in the city named."
    );
    assert_bare("local_time", &responses.tools[2].description);
    assert_eq!(responses.tools[3].description, ESCAPED);

    let rendered = session.to_session_json(LIVE_MODEL);
    let offered = described(&rendered["delegation"]["responses"]["tools"]);
    assert_eq!(
        offered[1],
        (
            "weather".to_owned(),
            "The current weather in the city named.".to_owned()
        ),
        "{rendered}"
    );
    assert_eq!(offered[3], ("book".to_owned(), ESCAPED.to_owned()));
    assert_eq!(
        rendered["delegation"]["responses"]["tools"][3]["strict"],
        false
    );
}

#[test]
fn live_client_config_twice_offers_each_described_tool_once() {
    let model = live();
    let tools = three_tools();
    let first = model
        .client_config(&config(TurnDetection::SemanticVad, true, tools.clone()))
        .expect("admitted");
    let second = model
        .client_config(&config(TurnDetection::SemanticVad, true, tools))
        .expect("admitted");
    assert_eq!(first, second, "building the configuration changes nothing");
    let Delegation::Responses(responses) = &second.delegation else {
        panic!("a session with tools delegates to a Responses backend")
    };
    let descriptions: Vec<&str> = responses
        .tools
        .iter()
        .filter(|tool| tool.name != "local_time")
        .map(|tool| tool.description.as_str())
        .collect();
    assert_eq!(
        descriptions,
        ["The current weather in the city named.", ESCAPED],
        "the default backend has no tools of its own"
    );
}
