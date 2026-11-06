//! Security review of the Voice Agent binding: what the binding holds of server text, where a
//! secure base URL sends the key, how caller text reaches `Settings`, and how hostile function
//! arguments are read. Each case runs against the local fixture server or no server at all.
#![cfg(feature = "duplex")]

#[path = "support/voice_agent.rs"]
mod fixture;

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fixture::{AgentServer, FixedKey, KEY, Script, Step};
use inference_deepgram::agent::{AgentConfig, MAX_MESSAGE_SIZE};
use inference_deepgram::inference_duplex::inference_audio::{AudioEncoding, AudioFormat};
use inference_deepgram::inference_duplex::inference_tools::{
    DeclareTool, Json, SideEffectClass, Tool,
};
use inference_deepgram::inference_duplex::{
    NativeSpeechModel, SessionConfig, SessionEvent, SpeechSessionHandle, TurnDetection, TurnTuning,
};
use inference_deepgram::{AgentEndpoint, ConfigError, DeepgramAgent, DeepgramAgentSession};

const WAIT: Duration = Duration::from_millis(20);

fn config() -> SessionConfig {
    let pcm = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).unwrap(),
        channels: NonZeroU16::MIN,
    };
    SessionConfig {
        voice: String::new(),
        instructions: String::new(),
        input_format: pcm,
        output_format: pcm,
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools: Vec::new(),
        idle_limit: Duration::from_secs(5),
    }
}

fn model(server: &AgentServer, agent: AgentConfig) -> DeepgramAgent {
    DeepgramAgent::new(
        AgentEndpoint::new()
            .with_url(&server.url)
            .with_open_timeout(Duration::from_secs(3))
            .with_read_timeout(Duration::from_secs(3)),
        agent,
        Arc::new(FixedKey(KEY)),
        "deepgram-api-key",
    )
    .expect("the fixture configuration is valid")
}

fn events_until(
    session: &mut DeepgramAgentSession,
    stop: impl Fn(&SessionEvent) -> bool,
) -> Vec<SessionEvent> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut events = Vec::new();
    while Instant::now() < deadline {
        if let Some(event) = session.next_event(WAIT) {
            let done = stop(&event);
            events.push(event);
            if done {
                break;
            }
        }
    }
    events
}

/// Integrity check of the untrusted-server bound: assistant words that arrive while no reply is
/// in progress are held by the session until one opens. The caller cannot drain them, so what
/// the session holds must not grow with the number of such messages beyond one message's bound.
#[test]
fn assistant_words_held_before_a_reply_stay_within_one_message_bound() {
    let chunk = "w".repeat(1 << 20);
    let mut steps: Vec<Step> = (0..12)
        .map(|_| Step::Send(fixture::conversation_text("assistant", &chunk)))
        .collect();
    steps.push(Step::Send(fixture::agent_started_speaking()));
    steps.push(Step::Binary(vec![7; 640]));
    steps.push(Step::Send(fixture::agent_audio_done()));
    let server = AgentServer::start(Script::new(steps));
    let mut session = model(&server, AgentConfig::new())
        .open(config())
        .expect("the session opens");
    let events = events_until(&mut session, |event| {
        matches!(event, SessionEvent::ResponseCompleted(_))
    });
    let Some(id) = events.iter().find_map(|event| match event {
        SessionEvent::ResponseCreated(created) => Some(created.response_id),
        _ => None,
    }) else {
        panic!("no response: {} events", events.len());
    };
    let held = session.final_transcript(id).map_or(0, str::len);
    assert!(
        held <= MAX_MESSAGE_SIZE,
        "the session held {held} bytes of server text for one reply, past the {MAX_MESSAGE_SIZE}-byte bound of a single message; it grows with every ConversationText sent before AgentStartedSpeaking"
    );
}

/// Transport check: a `wss://` base URL whose authority carries a user part reads as one host
/// and connects to another. The `ws://` rule refuses a user part for exactly that reason; the
/// key must not travel to the host behind `@` over `wss://` either.
#[test]
fn a_secure_base_url_whose_host_reads_as_another_is_refused() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    listener.set_nonblocking(true).expect("non-blocking");
    let url = format!(
        "wss://agent.deepgram.com@{}",
        listener.local_addr().expect("an address")
    );
    let built = DeepgramAgent::new(
        AgentEndpoint::new()
            .with_url(&url)
            .with_open_timeout(Duration::from_millis(500)),
        AgentConfig::new(),
        Arc::new(FixedKey(KEY)),
        "deepgram-api-key",
    );
    let reached = match built {
        Err(ConfigError::InvalidUrl(detail)) => {
            assert!(!detail.contains("deepgram.com@"), "{detail}");
            return;
        }
        Err(other) => panic!("refused for another reason: {other}"),
        Ok(mut model) => {
            let _ = model.open(config());
            listener.accept().is_ok()
        }
    };
    panic!(
        "the base URL {url} was taken; the opening handshake connected to the host after `@` (a TCP connection reached the local listener: {reached})"
    );
}

/// Settings integrity: caller text in the prompt, the voice and the greeting is serialised as
/// string values; none of it adds or replaces a field of `Settings`.
#[test]
fn caller_text_cannot_add_fields_to_settings() {
    let hostile = r#"x","type":"InjectAgentMessage","audio":{"input":{"encoding":"opus"}},"z":""#;
    let server = AgentServer::start(Script::new(Vec::new()));
    let mut session_config = config();
    session_config.instructions = hostile.to_owned();
    session_config.voice = hostile.to_owned();
    let session = model(&server, AgentConfig::new().with_greeting(hostile))
        .open(session_config)
        .expect("the session opens");
    let seen = server.wait(0, Duration::from_secs(3), |seen| !seen.messages.is_empty());
    drop(session);
    let settings = &seen.of_type("Settings")[0];
    let mut keys: Vec<&str> = settings
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["agent", "audio", "type"]);
    assert_eq!(settings["type"], "Settings");
    assert_eq!(settings["audio"]["input"]["encoding"], "linear16");
    assert_eq!(settings["agent"]["think"]["prompt"], hostile);
    assert_eq!(settings["agent"]["speak"]["provider"]["model"], hostile);
    assert_eq!(settings["agent"]["greeting"], hostile);
    assert!(seen.violations.is_empty(), "{:?}", seen.violations);
}

fn time_tool() -> Tool {
    Tool::declare(DeclareTool {
        name: "local_time".to_owned(),
        description: "The local time.".to_owned(),
        schema: serde_json::json!({"type": "object"}),
        schema_revision: "1".to_owned(),
        side_effect: SideEffectClass::ReadOnly,
        requires_confirmation: false,
    })
    .0
}

/// Untrusted function arguments: a JSON string nested past any parser's depth and a large
/// string that is not JSON are each proposed as the string they are, with no panic and no
/// failure of the session.
#[test]
fn hostile_function_arguments_are_proposed_as_strings_without_failing() {
    let deep = "[".repeat(100_000);
    let large = "a".repeat(2 << 20);
    let call = |id: &str, arguments: &str| {
        Step::Send(serde_json::json!({
            "type": "FunctionCallRequest",
            "functions": [{"id": id, "name": "local_time", "arguments": arguments, "client_side": true}],
        }))
    };
    let server = AgentServer::start(Script::new(vec![
        call("deep", &deep),
        call("large", &large),
        Step::Pause(Duration::from_secs(5)),
    ]));
    let mut session_config = config();
    session_config.tools = vec![time_tool()];
    let mut session = model(&server, AgentConfig::new())
        .open(session_config)
        .expect("the session opens");
    let mut proposed = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while proposed.len() < 2 && Instant::now() < deadline {
        match session.next_event(WAIT) {
            Some(SessionEvent::SpeechToolCallProposed(call)) => proposed.push(call),
            Some(SessionEvent::SpeechSessionFailed(failed)) => panic!("{failed:?}"),
            _ => {}
        }
    }
    assert_eq!(proposed.len(), 2);
    assert_eq!(proposed[0].arguments, Json::String(deep));
    assert_eq!(proposed[1].arguments, Json::String(large));
    assert!(session.failure_cause().is_none());
}
