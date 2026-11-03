//! Offline tests of per-response options on `response.create`, against a local WebSocket server
//! in the test. No network and no key: the credential is a fixed test value from an in-test
//! resolver.
//!
//! Every client event the scripts expect is written from the published Realtime API reference,
//! client events page (`https://developers.openai.com/api/reference/resources/realtime/client-events`),
//! `response.create`: `event_id` at the top level, and the `response` object's `instructions`,
//! `tool_choice`, `conversation`, `input` and `tools`.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use inference_openai::{
    ConversationItem, Credential, CredentialError, CredentialFuture, CredentialSource, Endpoint,
    FunctionTool, OPENAI_KEY_NAME, RealtimeClient, ResponseConversation, ResponseOptions,
    SessionConfig, ToolChoice,
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

/// The text of the next client event, exactly as it was written; fails the script after five
/// seconds.
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

/// The server half of session setup: `session.created`, the client's `session.update`, then
/// `session.updated`, each trimmed to the fields the client reads.
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

fn lookup_tool() -> FunctionTool {
    FunctionTool {
        name: "lookup_order".to_owned(),
        description: "Looks up an order by its number.".to_owned(),
        parameters: json!({
            "type": "object",
            "properties": {"number": {"type": "string"}},
            "required": ["number"],
        }),
    }
}

#[tokio::test]
async fn create_response_with_options_sends_each_set_field() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        assert_eq!(
            receive(&mut socket).await,
            json!({
                "type": "response.create",
                "event_id": "evt_response_1",
                "response": {
                    "instructions": "Answer in one short sentence.",
                    "tool_choice": {"type": "function", "name": "lookup_order"},
                    "conversation": "auto",
                    "input": [
                        {
                            "type": "message",
                            "role": "user",
                            "content": [{"type": "input_text", "text": "Where is order 42?"}],
                        },
                        {
                            "type": "function_call_output",
                            "call_id": "call_1",
                            "output": "{\"status\":\"shipped\"}",
                        },
                    ],
                    "tools": [{
                        "type": "function",
                        "name": "lookup_order",
                        "description": "Looks up an order by its number.",
                        "parameters": {
                            "type": "object",
                            "properties": {"number": {"type": "string"}},
                            "required": ["number"],
                        },
                    }],
                },
            })
        );
    })
    .await;
    let mut client = connect(&endpoint).await;
    client
        .create_response_with(&ResponseOptions {
            event_id: Some("evt_response_1".to_owned()),
            instructions: Some("Answer in one short sentence.".to_owned()),
            tool_choice: Some(ToolChoice::Function {
                name: "lookup_order".to_owned(),
            }),
            conversation: Some(ResponseConversation::Auto),
            input: Some(vec![
                ConversationItem::UserText("Where is order 42?".to_owned()),
                ConversationItem::FunctionCallOutput {
                    call_id: "call_1".to_owned(),
                    output: "{\"status\":\"shipped\"}".to_owned(),
                },
            ]),
            tools: Some(vec![lookup_tool()]),
        })
        .await
        .unwrap();
    finish(task).await;
}

#[tokio::test]
async fn create_response_with_default_options_is_the_bare_event() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        let first: Value = serde_json::from_str(&raw(&mut socket).await).unwrap();
        assert_eq!(first, json!({"type": "response.create"}));
        let second: Value = serde_json::from_str(&raw(&mut socket).await).unwrap();
        assert_eq!(second, json!({"type": "response.create"}));
    })
    .await;
    let mut client = connect(&endpoint).await;
    client
        .create_response_with(&ResponseOptions::default())
        .await
        .unwrap();
    client.create_response().await.unwrap();
    finish(task).await;
}

#[tokio::test]
async fn out_of_band_response_sends_conversation_none() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        let event = receive(&mut socket).await;
        assert_eq!(
            event,
            json!({
                "type": "response.create",
                "response": {
                    "conversation": "none",
                    "instructions": "Classify the caller's intent in one word.",
                },
            })
        );
        assert!(
            event["response"]["conversation"].is_string(),
            "conversation is the string \"none\", not null: {event}"
        );
        assert!(event.get("event_id").is_none(), "no event_id set: {event}");
    })
    .await;
    let mut client = connect(&endpoint).await;
    client
        .create_response_with(&ResponseOptions {
            conversation: Some(ResponseConversation::None),
            instructions: Some("Classify the caller's intent in one word.".to_owned()),
            ..ResponseOptions::default()
        })
        .await
        .unwrap();
    finish(task).await;
}

#[tokio::test]
async fn empty_tools_list_is_sent_as_an_empty_array() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        assert_eq!(
            receive(&mut socket).await,
            json!({"type": "response.create", "response": {"tools": []}})
        );
        assert_eq!(
            receive(&mut socket).await,
            json!({"type": "response.create", "event_id": "evt_only"})
        );
    })
    .await;
    let mut client = connect(&endpoint).await;
    client
        .create_response_with(&ResponseOptions {
            tools: Some(Vec::new()),
            ..ResponseOptions::default()
        })
        .await
        .unwrap();
    client
        .create_response_with(&ResponseOptions {
            event_id: Some("evt_only".to_owned()),
            ..ResponseOptions::default()
        })
        .await
        .unwrap();
    finish(task).await;
}
