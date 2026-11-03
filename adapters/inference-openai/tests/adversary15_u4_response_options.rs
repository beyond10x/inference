//! Adversary cases for per-response options on `response.create`, against a local WebSocket
//! server in the test. No network and no key: the credential is a fixed test value from an
//! in-test resolver.
//!
//! The shapes are read from the published Realtime API reference, client events page,
//! `response.create`, and server events page, `response.created`, `response.done` and `error`.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use inference_openai::{
    ConversationItem, Credential, CredentialError, CredentialFuture, CredentialSource, Endpoint,
    FunctionTool, OPENAI_KEY_NAME, RealtimeClient, ResponseConversation, ResponseOptions,
    ResponseStatus, ServerEvent, SessionConfig, ToolChoice,
};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

const CREDENTIAL: &str = "offline-test-credential";
const MODEL: &str = "gpt-realtime-2.1";

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

async fn raw(socket: &mut Socket) -> String {
    let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("the client sent nothing for five seconds")
        .expect("the client closed the connection")
        .unwrap();
    match frame {
        Message::Text(text) => text.as_str().to_owned(),
        other => panic!("expected a text frame, got {other:?}"),
    }
}

async fn receive(socket: &mut Socket) -> Value {
    serde_json::from_str(&raw(socket).await).unwrap()
}

async fn accept_session(socket: &mut Socket) {
    send(
        socket,
        json!({"type": "session.created", "event_id": "event_1", "session": {"type": "realtime", "id": "sess_1"}}),
    )
    .await;
    let update = receive(socket).await;
    assert_eq!(
        update["type"], "session.update",
        "first client event: {update}"
    );
    send(
        socket,
        json!({"type": "session.updated", "event_id": "event_2", "session": update["session"]}),
    )
    .await;
}

async fn connect(endpoint: &Endpoint) -> RealtimeClient {
    RealtimeClient::connect(
        endpoint,
        &FixedSource,
        OPENAI_KEY_NAME,
        &SessionConfig::default(),
    )
    .await
    .expect("session setup")
}

async fn finish(task: JoinHandle<()>) {
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("the server script did not finish")
        .expect("the server script failed");
}

/// Each option set alone, and the one `response` key it must produce.
fn single_options() -> Vec<(ResponseOptions, Value)> {
    vec![
        (
            ResponseOptions {
                instructions: Some("Be brief.".to_owned()),
                ..ResponseOptions::default()
            },
            json!({"instructions": "Be brief."}),
        ),
        (
            ResponseOptions {
                tool_choice: Some(ToolChoice::Auto),
                ..ResponseOptions::default()
            },
            json!({"tool_choice": "auto"}),
        ),
        (
            ResponseOptions {
                tool_choice: Some(ToolChoice::None),
                ..ResponseOptions::default()
            },
            json!({"tool_choice": "none"}),
        ),
        (
            ResponseOptions {
                tool_choice: Some(ToolChoice::Required),
                ..ResponseOptions::default()
            },
            json!({"tool_choice": "required"}),
        ),
        (
            ResponseOptions {
                conversation: Some(ResponseConversation::Auto),
                ..ResponseOptions::default()
            },
            json!({"conversation": "auto"}),
        ),
        (
            ResponseOptions {
                conversation: Some(ResponseConversation::None),
                ..ResponseOptions::default()
            },
            json!({"conversation": "none"}),
        ),
        (
            ResponseOptions {
                input: Some(vec![ConversationItem::AssistantText(
                    "Your order shipped.".to_owned(),
                )]),
                ..ResponseOptions::default()
            },
            json!({"input": [{
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "Your order shipped."}],
            }]}),
        ),
        (
            ResponseOptions {
                input: Some(Vec::new()),
                ..ResponseOptions::default()
            },
            json!({"input": []}),
        ),
        (
            ResponseOptions {
                instructions: Some(String::new()),
                ..ResponseOptions::default()
            },
            json!({"instructions": ""}),
        ),
        (
            ResponseOptions {
                tools: Some(vec![FunctionTool {
                    name: "noop".to_owned(),
                    description: String::new(),
                    parameters: json!({"type": "object", "properties": {}}),
                }]),
                ..ResponseOptions::default()
            },
            json!({"tools": [{
                "type": "function",
                "name": "noop",
                "description": "",
                "parameters": {"type": "object", "properties": {}},
            }]}),
        ),
    ]
}

#[tokio::test]
async fn each_option_alone_sends_only_its_key_under_response() {
    let cases = single_options();
    let expected: Vec<Value> = cases.iter().map(|(_, wire)| wire.clone()).collect();
    let (endpoint, task) = serve(move |mut socket| async move {
        accept_session(&mut socket).await;
        for wire in expected {
            let event = receive(&mut socket).await;
            assert_eq!(
                event,
                json!({"type": "response.create", "response": wire}),
                "one option set"
            );
        }
    })
    .await;
    let mut client = connect(&endpoint).await;
    for (options, _) in &cases {
        client.create_response_with(options).await.unwrap();
    }
    finish(task).await;
}

#[tokio::test]
async fn bare_create_response_is_the_0_9_0_text_exactly() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        assert_eq!(raw(&mut socket).await, r#"{"type":"response.create"}"#);
        assert_eq!(raw(&mut socket).await, r#"{"type":"response.create"}"#);
    })
    .await;
    let mut client = connect(&endpoint).await;
    client.create_response().await.unwrap();
    client
        .create_response_with(&ResponseOptions::default())
        .await
        .unwrap();
    finish(task).await;
}

#[tokio::test]
async fn hostile_text_in_options_is_escaped_not_injected() {
    let hostile = "\"}, \"type\": \"session.update\", \"x\": {\"\u{0000}\u{2028}\n\\";
    let (endpoint, task) = serve(move |mut socket| async move {
        accept_session(&mut socket).await;
        let event = receive(&mut socket).await;
        assert_eq!(event["type"], "response.create", "{event}");
        assert_eq!(event["event_id"], hostile, "{event}");
        assert_eq!(event["response"]["instructions"], hostile, "{event}");
        assert_eq!(
            event.as_object().unwrap().len(),
            3,
            "type, event_id and response only: {event}"
        );
    })
    .await;
    let mut client = connect(&endpoint).await;
    client
        .create_response_with(&ResponseOptions {
            event_id: Some(hostile.to_owned()),
            instructions: Some(hostile.to_owned()),
            ..ResponseOptions::default()
        })
        .await
        .unwrap();
    finish(task).await;
}

#[tokio::test]
async fn same_options_sent_twice_are_the_same_event() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        let first = receive(&mut socket).await;
        let second = receive(&mut socket).await;
        assert_eq!(first, second);
        assert_eq!(first["response"]["tools"], json!([]), "{first}");
    })
    .await;
    let mut client = connect(&endpoint).await;
    let options = ResponseOptions {
        event_id: Some("evt_same".to_owned()),
        conversation: Some(ResponseConversation::None),
        tools: Some(Vec::new()),
        ..ResponseOptions::default()
    };
    let copy = options.clone();
    client.create_response_with(&options).await.unwrap();
    client.create_response_with(&copy).await.unwrap();
    assert_eq!(options, copy);
    finish(task).await;
}

/// The doc comment of `create_response_with`: an out-of-band response's server events still
/// arrive through `next_event`, told apart from the default response's by their `response_id`.
#[tokio::test]
async fn out_of_band_events_interleaved_with_the_default_response_arrive_by_response_id() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        let default = receive(&mut socket).await;
        assert_eq!(default, json!({"type": "response.create"}));
        let oob = receive(&mut socket).await;
        assert_eq!(oob["response"]["conversation"], "none", "{oob}");
        for event in [
            json!({"type": "response.created", "event_id": "e1", "response": {
                "object": "realtime.response", "id": "resp_default", "status": "in_progress",
                "conversation_id": "conv_1", "output": []}}),
            json!({"type": "response.created", "event_id": "e2", "response": {
                "object": "realtime.response", "id": "resp_oob", "status": "in_progress",
                "conversation_id": null, "metadata": {"topic": "intent"}, "output": []}}),
            json!({"type": "response.output_text.delta", "event_id": "e3",
                "response_id": "resp_oob", "item_id": "item_oob", "output_index": 0,
                "content_index": 0, "delta": "billing"}),
            json!({"type": "response.output_audio_transcript.delta", "event_id": "e4",
                "response_id": "resp_default", "item_id": "item_default", "output_index": 0,
                "content_index": 0, "delta": "Hello"}),
            json!({"type": "response.done", "event_id": "e5", "response": {
                "object": "realtime.response", "id": "resp_oob", "status": "completed",
                "conversation_id": null, "metadata": {"topic": "intent"}, "output": []}}),
            json!({"type": "response.done", "event_id": "e6", "response": {
                "object": "realtime.response", "id": "resp_default", "status": "completed",
                "conversation_id": "conv_1", "output": []}}),
        ] {
            send(&mut socket, event).await;
        }
    })
    .await;
    let mut client = connect(&endpoint).await;
    client.create_response().await.unwrap();
    client
        .create_response_with(&ResponseOptions {
            conversation: Some(ResponseConversation::None),
            instructions: Some("Classify the intent in one word.".to_owned()),
            ..ResponseOptions::default()
        })
        .await
        .unwrap();
    let mut seen = Vec::new();
    for _ in 0..6 {
        seen.push(client.next_event().await.expect("an event"));
    }
    assert_eq!(
        seen[0],
        ServerEvent::ResponseCreated {
            response_id: "resp_default".to_owned()
        }
    );
    assert_eq!(
        seen[1],
        ServerEvent::ResponseCreated {
            response_id: "resp_oob".to_owned()
        }
    );
    assert_eq!(
        seen[2],
        ServerEvent::TextDelta {
            response_id: "resp_oob".to_owned(),
            item_id: "item_oob".to_owned(),
            delta: "billing".to_owned(),
        }
    );
    assert!(
        matches!(&seen[3], ServerEvent::OutputTranscriptDelta { response_id, .. } if response_id == "resp_default"),
        "{:?}",
        seen[3]
    );
    assert!(
        matches!(&seen[4], ServerEvent::ResponseDone { response_id: Some(id), status: Some(ResponseStatus::Completed), .. } if id == "resp_oob"),
        "{:?}",
        seen[4]
    );
    assert!(
        matches!(&seen[5], ServerEvent::ResponseDone { response_id: Some(id), .. } if id == "resp_default"),
        "{:?}",
        seen[5]
    );
    finish(task).await;
}

/// `ResponseOptions::event_id`'s doc comment: a server `error` caused by this event names it.
#[tokio::test]
async fn a_refused_response_create_names_its_event_id_in_the_error() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        let event = receive(&mut socket).await;
        assert_eq!(event["event_id"], "evt_bad", "{event}");
        send(
            &mut socket,
            json!({"type": "error", "event_id": "event_9", "error": {
                "type": "invalid_request_error", "code": "invalid_value",
                "message": "Invalid tool_choice", "param": "response.tool_choice",
                "event_id": "evt_bad"}}),
        )
        .await;
    })
    .await;
    let mut client = connect(&endpoint).await;
    client
        .create_response_with(&ResponseOptions {
            event_id: Some("evt_bad".to_owned()),
            tool_choice: Some(ToolChoice::Function {
                name: "missing".to_owned(),
            }),
            ..ResponseOptions::default()
        })
        .await
        .unwrap();
    match client.next_event().await.expect("an event") {
        ServerEvent::Error(error) => {
            assert_eq!(error.event_id.as_deref(), Some("evt_bad"));
            assert_eq!(error.param.as_deref(), Some("response.tool_choice"));
        }
        other => panic!("expected error, got {other:?}"),
    }
    finish(task).await;
}

#[test]
fn the_public_types_carry_the_designed_derives() {
    fn copy_eq_hash<T: Copy + Eq + std::hash::Hash + std::fmt::Debug>() {}
    fn clone_default_eq<T: Clone + Default + PartialEq + std::fmt::Debug>() {}
    copy_eq_hash::<ResponseConversation>();
    clone_default_eq::<ResponseOptions>();
    let options = ResponseOptions::default();
    assert_eq!(options.event_id, None);
    assert_eq!(options.instructions, None);
    assert_eq!(options.tool_choice, None);
    assert_eq!(options.conversation, None);
    assert_eq!(options.input, None);
    assert_eq!(options.tools, None);
}
