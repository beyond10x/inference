//! Adversary cases for the session input options of `session.update`: input noise reduction,
//! the transcription language and prompt, and the session tool choice. No network and no key:
//! the offline case runs against a local WebSocket server in the test with a fixed test
//! credential.
//!
//! The 0.9.0 byte-identity cases assert literal strings written from the 0.9.0
//! `SessionConfig::to_session_json`, so a reordering or a stray key in a fully populated 0.9.0
//! configuration fails, not only in the default one.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use inference_openai::{
    Credential, CredentialError, CredentialFuture, CredentialSource, Eagerness, Endpoint,
    FunctionTool, NoiseReduction, OPENAI_KEY_NAME, OutputModality, RealtimeClient, SessionConfig,
    ToolChoice, TurnDetection, WireFormat,
};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

const CREDENTIAL: &str = "offline-test-credential";
const MODEL: &str = "gpt-realtime-2.1";

fn weather_tool() -> FunctionTool {
    FunctionTool {
        name: "get_weather".to_owned(),
        description: "Current weather for a city.".to_owned(),
        parameters: json!({"type": "object", "properties": {"location": {"type": "string"}}}),
    }
}

const WEATHER_TOOL_JSON: &str = concat!(
    r#"{"description":"Current weather for a city.","name":"get_weather","#,
    r#""parameters":{"properties":{"location":{"type":"string"}},"type":"object"},"type":"function"}"#
);

/// Every 0.9.0 field set, server turn detection: the bytes 0.9.0 sent.
#[test]
fn a_full_0_9_0_server_vad_config_serialises_as_it_did() {
    let session = SessionConfig {
        instructions: Some("Be brief.".to_owned()),
        voice: Some("marin".to_owned()),
        output_modality: OutputModality::Text,
        input_format: WireFormat::Pcmu,
        output_format: WireFormat::Pcma,
        turn_detection: TurnDetection::Server {
            threshold: Some(0.5),
            prefix_padding_ms: Some(300),
            silence_duration_ms: Some(500),
            create_response: true,
            interrupt_response: false,
        },
        input_transcription: Some("gpt-4o-mini-transcribe".to_owned()),
        tools: vec![weather_tool()],
        speed: Some(1.1),
        ..SessionConfig::default()
    };
    let expected = [
        r#"{"audio":{"input":{"format":{"type":"audio/pcmu"},"#,
        r#""transcription":{"model":"gpt-4o-mini-transcribe"},"#,
        r#""turn_detection":{"create_response":true,"interrupt_response":false,"#,
        r#""prefix_padding_ms":300,"silence_duration_ms":500,"threshold":0.5,"type":"server_vad"}},"#,
        r#""output":{"format":{"type":"audio/pcma"},"speed":1.1,"voice":"marin"}},"#,
        r#""instructions":"Be brief.","output_modalities":["text"],"tools":["#,
        WEATHER_TOOL_JSON,
        r#"],"type":"realtime"}"#,
    ]
    .concat();
    assert_eq!(session.to_session_json().to_string(), expected);
}

/// Every 0.9.0 field set, semantic turn detection and PCM16: the bytes 0.9.0 sent.
#[test]
fn a_full_0_9_0_semantic_vad_config_serialises_as_it_did() {
    let session = SessionConfig {
        instructions: Some("Be brief.".to_owned()),
        voice: Some("cedar".to_owned()),
        turn_detection: TurnDetection::Semantic {
            eagerness: Eagerness::High,
            create_response: false,
            interrupt_response: true,
        },
        input_transcription: Some("whisper-1".to_owned()),
        speed: Some(0.25),
        ..SessionConfig::default()
    };
    let expected = concat!(
        r#"{"audio":{"input":{"format":{"rate":24000,"type":"audio/pcm"},"#,
        r#""transcription":{"model":"whisper-1"},"#,
        r#""turn_detection":{"create_response":false,"eagerness":"high","interrupt_response":true,"type":"semantic_vad"}},"#,
        r#""output":{"format":{"rate":24000,"type":"audio/pcm"},"speed":0.25,"voice":"cedar"}},"#,
        r#""instructions":"Be brief.","output_modalities":["audio"],"type":"realtime"}"#,
    );
    assert_eq!(session.to_session_json().to_string(), expected);
}

/// Every new option at once beside every old input key: the exact `audio.input` object, so no
/// option overwrites another and none lands at the wrong level.
#[test]
fn every_input_option_together_is_the_exact_audio_input_object() {
    let session = SessionConfig {
        input_format: WireFormat::Pcmu,
        turn_detection: TurnDetection::Semantic {
            eagerness: Eagerness::Auto,
            create_response: true,
            interrupt_response: true,
        },
        input_transcription: Some("gpt-4o-transcribe".to_owned()),
        transcription_language: Some("de".to_owned()),
        transcription_prompt: Some("Names: Anna.".to_owned()),
        noise_reduction: Some(NoiseReduction::NearField),
        tools: vec![weather_tool()],
        tool_choice: Some(ToolChoice::Required),
        ..SessionConfig::default()
    }
    .to_session_json();
    assert_eq!(
        session["audio"]["input"],
        json!({
            "format": {"type": "audio/pcmu"},
            "noise_reduction": {"type": "near_field"},
            "transcription": {"model": "gpt-4o-transcribe", "language": "de", "prompt": "Names: Anna."},
            "turn_detection": {
                "type": "semantic_vad",
                "eagerness": "auto",
                "create_response": true,
                "interrupt_response": true
            }
        })
    );
    assert_eq!(session["tool_choice"], json!("required"));
    let keys: Vec<&String> = session.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        ["audio", "output_modalities", "tool_choice", "tools", "type"],
        "{session}"
    );
    assert!(
        session["audio"]["output"].get("noise_reduction").is_none(),
        "{session}"
    );
    assert!(session["audio"].get("tool_choice").is_none(), "{session}");
}

/// Noise reduction off beside turn detection off: two explicit `null`s, exact bytes.
#[test]
fn noise_reduction_off_and_turn_detection_off_are_both_null() {
    let session = SessionConfig {
        noise_reduction: Some(NoiseReduction::Off),
        ..SessionConfig::default()
    };
    assert_eq!(
        session.to_session_json().to_string(),
        concat!(
            r#"{"audio":{"input":{"format":{"rate":24000,"type":"audio/pcm"},"#,
            r#""noise_reduction":null,"turn_detection":null},"#,
            r#""output":{"format":{"rate":24000,"type":"audio/pcm"}}},"#,
            r#""output_modalities":["audio"],"type":"realtime"}"#
        )
    );
}

/// The language alone and the prompt alone: each is sent without the other.
#[test]
fn language_alone_and_prompt_alone_are_sent_alone() {
    let transcription = |language: Option<&str>, prompt: Option<&str>| {
        SessionConfig {
            input_transcription: Some("gpt-4o-mini-transcribe".to_owned()),
            transcription_language: language.map(str::to_owned),
            transcription_prompt: prompt.map(str::to_owned),
            ..SessionConfig::default()
        }
        .to_session_json()["audio"]["input"]["transcription"]
            .clone()
    };
    assert_eq!(
        transcription(Some("ja"), None),
        json!({"model": "gpt-4o-mini-transcribe", "language": "ja"})
    );
    assert_eq!(
        transcription(None, Some("Expect technical terms.")),
        json!({"model": "gpt-4o-mini-transcribe", "prompt": "Expect technical terms."})
    );
}

/// Only the language, no model: nothing under `transcription`, and no stray key elsewhere.
#[test]
fn a_language_without_a_model_leaves_no_trace() {
    let with_language = SessionConfig {
        transcription_language: Some("de".to_owned()),
        transcription_prompt: Some("Names.".to_owned()),
        ..SessionConfig::default()
    };
    assert_eq!(
        with_language.to_session_json(),
        SessionConfig::default().to_session_json()
    );
}

/// A function name that needs escaping reaches `tool_choice` intact, and the choice is sent
/// even when the session lists no tools (the server decides whether that is valid).
#[test]
fn a_function_tool_choice_is_sent_verbatim_without_tools() {
    let name = "lookup \"order\"\n\\ü";
    let session = SessionConfig {
        tool_choice: Some(ToolChoice::Function {
            name: name.to_owned(),
        }),
        ..SessionConfig::default()
    }
    .to_session_json();
    assert_eq!(
        session["tool_choice"],
        json!({"type": "function", "name": name})
    );
    assert!(session.get("tools").is_none(), "{session}");
    let parsed: Value = serde_json::from_str(&session.to_string()).unwrap();
    assert_eq!(parsed["tool_choice"]["name"], json!(name));
}

/// `ToolChoice::None` is the `"none"` string, never a JSON `null` and never left out.
#[test]
fn tool_choice_none_is_the_string_none() {
    let session = SessionConfig {
        tool_choice: Some(ToolChoice::None),
        ..SessionConfig::default()
    }
    .to_session_json();
    assert_eq!(
        session.get("tool_choice"),
        Some(&json!("none")),
        "{session}"
    );
}

/// The exported types are usable as values: copy, compare and hash.
#[test]
fn the_new_types_are_exported_with_their_derives() {
    use std::collections::HashSet;
    let near = NoiseReduction::NearField;
    let copy = near;
    assert_eq!(near, copy);
    let choices: HashSet<ToolChoice> = [
        ToolChoice::Auto,
        ToolChoice::None,
        ToolChoice::Required,
        ToolChoice::Function {
            name: "a".to_owned(),
        },
        ToolChoice::Function {
            name: "a".to_owned(),
        },
    ]
    .into_iter()
    .collect();
    assert_eq!(choices.len(), 4);
    assert_ne!(NoiseReduction::Off, NoiseReduction::FarField);
}

/// Serves one fixed test value under [`OPENAI_KEY_NAME`]; any other name is `Missing`.
struct FixedSource;

impl CredentialSource for FixedSource {
    fn resolve<'a>(&'a self, name: &'a str) -> CredentialFuture<'a> {
        let found = name == OPENAI_KEY_NAME;
        Box::pin(async move {
            if found {
                Ok(Credential::new(CREDENTIAL.as_bytes().to_vec()))
            } else {
                Err(CredentialError::Missing)
            }
        })
    }
}

type Socket = WebSocketStream<TcpStream>;

/// Binds a local server, accepts one connection and runs `script` on it.
async fn serve<F, Fut>(script: F) -> (Endpoint, JoinHandle<()>)
where
    F: FnOnce(Socket) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
            .await
            .expect("the client did not connect within five seconds")
            .unwrap();
        let socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        script(socket).await;
    });
    let endpoint = Endpoint::new(MODEL)
        .with_url(format!("ws://{address}/v1/realtime"))
        .with_read_timeout(Duration::from_secs(5));
    (endpoint, task)
}

async fn send(socket: &mut Socket, event: Value) {
    socket.send(Message::text(event.to_string())).await.unwrap();
}

/// The next client event as JSON; fails the script after five seconds of silence.
async fn receive(socket: &mut Socket) -> Value {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("the client sent nothing for five seconds")
            .expect("the client closed the connection")
            .unwrap();
        match message {
            Message::Text(text) => return serde_json::from_str(text.as_str()).unwrap(),
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("expected a text frame, got {other:?}"),
        }
    }
}

/// `session.created`, from the server events page, trimmed to the fields a client reads.
fn session_created() -> Value {
    json!({
        "type": "session.created",
        "event_id": "event_C9G5RJeJ2gF77mV7f2B1j",
        "session": {
            "type": "realtime",
            "object": "realtime.session",
            "id": "sess_C9G5QPteg4UIbotdKLoYQ",
            "model": "gpt-realtime-2025-08-28",
            "output_modalities": ["audio"],
            "tools": [],
            "tool_choice": "auto",
            "audio": {
                "input": {
                    "format": {"type": "audio/pcm", "rate": 24000},
                    "transcription": null,
                    "noise_reduction": null,
                    "turn_detection": null
                },
                "output": {"format": {"type": "audio/pcm", "rate": 24000}, "voice": "marin", "speed": 1}
            }
        }
    })
}

/// `session.updated`, from the server events page: the session as the server now holds it.
fn session_updated(session: &Value) -> Value {
    let mut confirmed = session.clone();
    confirmed["object"] = json!("realtime.session");
    confirmed["id"] = json!("sess_C9G5QPteg4UIbotdKLoYQ");
    json!({"type": "session.updated", "event_id": "event_C9G8mqI3IucaojlVKE8Cs", "session": confirmed})
}

/// A default connect sends none of the new keys; a later `update_session` sends each of them.
#[tokio::test]
async fn update_session_mid_session_carries_the_input_options() {
    let (endpoint, server) = serve(|mut socket| async move {
        send(&mut socket, session_created()).await;
        let first = receive(&mut socket).await;
        assert_eq!(first["type"], "session.update", "{first}");
        let input = &first["session"]["audio"]["input"];
        assert!(input.get("noise_reduction").is_none(), "{first}");
        assert!(input.get("transcription").is_none(), "{first}");
        assert!(first["session"].get("tool_choice").is_none(), "{first}");
        send(&mut socket, session_updated(&first["session"])).await;
        let second = receive(&mut socket).await;
        assert_eq!(second["type"], "session.update", "{second}");
        let session = &second["session"];
        assert_eq!(
            session["audio"]["input"]["noise_reduction"],
            Value::Null,
            "{second}"
        );
        assert!(
            session["audio"]["input"]
                .as_object()
                .unwrap()
                .contains_key("noise_reduction"),
            "{second}"
        );
        assert_eq!(
            session["audio"]["input"]["transcription"],
            json!({"model": "gpt-4o-mini-transcribe", "language": "fr", "prompt": "Paris."}),
            "{second}"
        );
        assert_eq!(session["tool_choice"], json!("auto"), "{second}");
    })
    .await;
    let mut client = RealtimeClient::connect(
        &endpoint,
        &FixedSource,
        OPENAI_KEY_NAME,
        &SessionConfig::default(),
    )
    .await
    .expect("session setup");
    let update = SessionConfig {
        noise_reduction: Some(NoiseReduction::Off),
        input_transcription: Some("gpt-4o-mini-transcribe".to_owned()),
        transcription_language: Some("fr".to_owned()),
        transcription_prompt: Some("Paris.".to_owned()),
        tool_choice: Some(ToolChoice::Auto),
        ..SessionConfig::default()
    };
    client.update_session(&update).await.expect("update");
    server.await.unwrap();
    drop(client);
}
