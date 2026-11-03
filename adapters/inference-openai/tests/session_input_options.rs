//! The session input options of `session.update`: input noise reduction, the transcription
//! language and prompt, and the session tool choice. No network and no key: the offline case
//! runs against a local WebSocket server in the test with a fixed test credential.
//!
//! Every field is written from the session configuration of `session.update` on the published
//! Realtime API reference, client events page
//! (`https://developers.openai.com/api/reference/resources/realtime/client-events`):
//! `audio.input.noise_reduction`, `audio.input.transcription` (`model`, `language`, `prompt`) and
//! `tool_choice`.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use inference_openai::{
    Credential, CredentialError, CredentialFuture, CredentialSource, Endpoint, FunctionTool,
    NoiseReduction, OPENAI_KEY_NAME, RealtimeClient, SessionConfig, ToolChoice,
};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

const CREDENTIAL: &str = "offline-test-credential";
const MODEL: &str = "gpt-realtime-2.1";

#[test]
fn noise_reduction_is_sent_under_audio_input() {
    let input = |noise_reduction| {
        SessionConfig {
            noise_reduction,
            ..SessionConfig::default()
        }
        .to_session_json()["audio"]["input"]
            .clone()
    };
    assert_eq!(
        input(Some(NoiseReduction::NearField))["noise_reduction"],
        json!({"type": "near_field"})
    );
    assert_eq!(
        input(Some(NoiseReduction::FarField))["noise_reduction"],
        json!({"type": "far_field"})
    );
    let off = input(Some(NoiseReduction::Off));
    assert_eq!(off.get("noise_reduction"), Some(&Value::Null), "{off}");
    let unset = input(None);
    assert!(unset.get("noise_reduction").is_none(), "{unset}");
}

#[test]
fn transcription_language_and_prompt_are_sent_with_the_model() {
    let session = SessionConfig {
        input_transcription: Some("gpt-4o-mini-transcribe".to_owned()),
        transcription_language: Some("de".to_owned()),
        transcription_prompt: Some("Names: Anna, Jakob.".to_owned()),
        ..SessionConfig::default()
    };
    assert_eq!(
        session.to_session_json()["audio"]["input"]["transcription"],
        json!({
            "model": "gpt-4o-mini-transcribe",
            "language": "de",
            "prompt": "Names: Anna, Jakob."
        })
    );
    let model_only = SessionConfig {
        input_transcription: Some("gpt-4o-mini-transcribe".to_owned()),
        ..SessionConfig::default()
    };
    assert_eq!(
        model_only.to_session_json()["audio"]["input"]["transcription"],
        json!({"model": "gpt-4o-mini-transcribe"})
    );
    let without_model = SessionConfig {
        transcription_language: Some("de".to_owned()),
        transcription_prompt: Some("Names: Anna, Jakob.".to_owned()),
        ..SessionConfig::default()
    }
    .to_session_json();
    assert!(
        without_model["audio"]["input"]
            .get("transcription")
            .is_none(),
        "{without_model}"
    );
}

#[test]
fn session_tool_choice_is_sent() {
    let choice = |tool_choice| {
        SessionConfig {
            tool_choice: Some(tool_choice),
            ..SessionConfig::default()
        }
        .to_session_json()["tool_choice"]
            .clone()
    };
    assert_eq!(choice(ToolChoice::Auto), json!("auto"));
    assert_eq!(choice(ToolChoice::None), json!("none"));
    assert_eq!(choice(ToolChoice::Required), json!("required"));
    assert_eq!(
        choice(ToolChoice::Function {
            name: "get_weather".to_owned()
        }),
        json!({"type": "function", "name": "get_weather"})
    );
    let unset = SessionConfig::default().to_session_json();
    assert!(unset.get("tool_choice").is_none(), "{unset}");
}

#[test]
fn default_session_json_unchanged() {
    assert_eq!(
        SessionConfig::default().to_session_json().to_string(),
        concat!(
            r#"{"audio":{"input":{"format":{"rate":24000,"type":"audio/pcm"},"turn_detection":null},"#,
            r#""output":{"format":{"rate":24000,"type":"audio/pcm"}}},"#,
            r#""output_modalities":["audio"],"type":"realtime"}"#
        )
    );
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

/// Binds a local server, accepts one connection and runs `script` on it. Returns the endpoint the
/// client connects to and the server task, whose panics carry the script's assertion failures.
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

/// `session.created`, from the server events page, example trimmed to the fields a client reads.
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
            "max_output_tokens": "inf",
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

#[tokio::test]
async fn the_session_update_on_connect_carries_the_input_options() {
    let (endpoint, server) = serve(|mut socket| async move {
        send(&mut socket, session_created()).await;
        let update = receive(&mut socket).await;
        assert_eq!(update["type"], "session.update", "{update}");
        let session = &update["session"];
        assert_eq!(
            session["audio"]["input"]["noise_reduction"],
            json!({"type": "far_field"}),
            "{update}"
        );
        assert_eq!(
            session["audio"]["input"]["transcription"],
            json!({"model": "gpt-4o-mini-transcribe", "language": "en", "prompt": "Weather."}),
            "{update}"
        );
        assert_eq!(
            session["tool_choice"],
            json!({"type": "function", "name": "get_weather"}),
            "{update}"
        );
        send(&mut socket, session_updated(session)).await;
    })
    .await;
    let session = SessionConfig {
        noise_reduction: Some(NoiseReduction::FarField),
        input_transcription: Some("gpt-4o-mini-transcribe".to_owned()),
        transcription_language: Some("en".to_owned()),
        transcription_prompt: Some("Weather.".to_owned()),
        tools: vec![FunctionTool {
            name: "get_weather".to_owned(),
            description: "Current weather for a city.".to_owned(),
            parameters: json!({"type": "object", "properties": {"location": {"type": "string"}}}),
        }],
        tool_choice: Some(ToolChoice::Function {
            name: "get_weather".to_owned(),
        }),
        ..SessionConfig::default()
    };
    let client = RealtimeClient::connect(&endpoint, &FixedSource, OPENAI_KEY_NAME, &session)
        .await
        .expect("session setup");
    server.await.unwrap();
    drop(client);
}
