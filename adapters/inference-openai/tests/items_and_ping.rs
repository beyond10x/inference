//! Offline tests of conversation items with placement and event ids, and of the keepalive ping,
//! against a local WebSocket server in the test. No network and no key: the credential is a fixed
//! test value from an in-test resolver.
//!
//! Every client event the scripts expect is written from the published Realtime API reference,
//! client events page (`https://developers.openai.com/api/reference/resources/realtime/client-events`),
//! `conversation.item.create`; the ping frame is RFC 6455 section 5.5.2.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use inference_openai::{
    ConversationItem, Credential, CredentialError, CredentialFuture, CredentialSource, Endpoint,
    ItemOptions, OPENAI_KEY_NAME, RealtimeClient, RealtimeError, SessionConfig,
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

/// The next frame the client sent, whatever its kind; fails the script after five seconds.
async fn frame(socket: &mut Socket) -> Message {
    tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("the client sent nothing for five seconds")
        .expect("the client closed the connection")
        .unwrap()
}

/// The text of the next client event, exactly as it was written.
async fn raw(socket: &mut Socket) -> String {
    match frame(socket).await {
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

#[tokio::test]
async fn assistant_item_is_sent_with_role_assistant() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        assert_eq!(
            receive(&mut socket).await,
            json!({
                "type": "conversation.item.create",
                "item": {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "Hello, how can I help?"}],
                },
            })
        );
    })
    .await;
    let mut client = connect(&endpoint).await;
    client
        .create_item(
            &ConversationItem::AssistantText("Hello, how can I help?".to_owned()),
            &ItemOptions::default(),
        )
        .await
        .unwrap();
    finish(task).await;
}

#[tokio::test]
async fn item_placement_sends_previous_item_id() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        let placed = receive(&mut socket).await;
        assert_eq!(placed["previous_item_id"], "item_root", "{placed}");
        assert!(placed.get("event_id").is_none(), "{placed}");
        assert_eq!(placed["item"]["role"], "assistant", "{placed}");
        assert!(placed["item"].get("previous_item_id").is_none(), "{placed}");
        let unplaced = receive(&mut socket).await;
        assert!(unplaced.get("previous_item_id").is_none(), "{unplaced}");
        assert_eq!(unplaced["item"]["role"], "user", "{unplaced}");
    })
    .await;
    let mut client = connect(&endpoint).await;
    client
        .create_item(
            &ConversationItem::AssistantText("Welcome.".to_owned()),
            &ItemOptions {
                previous_item_id: Some("item_root".to_owned()),
                ..ItemOptions::default()
            },
        )
        .await
        .unwrap();
    client
        .create_item(
            &ConversationItem::UserText("Hi.".to_owned()),
            &ItemOptions::default(),
        )
        .await
        .unwrap();
    finish(task).await;
}

#[tokio::test]
async fn item_event_id_is_sent() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        let named = receive(&mut socket).await;
        assert_eq!(named["event_id"], "event_greeting", "{named}");
        assert!(named.get("previous_item_id").is_none(), "{named}");
        assert_eq!(
            named["item"],
            json!({"type": "function_call_output", "call_id": "call_1", "output": "{}"})
        );
        let unnamed = receive(&mut socket).await;
        assert!(unnamed.get("event_id").is_none(), "{unnamed}");
        let both = receive(&mut socket).await;
        assert_eq!(both["event_id"], "event_both", "{both}");
        assert_eq!(both["previous_item_id"], "item_root", "{both}");
    })
    .await;
    let mut client = connect(&endpoint).await;
    let output = ConversationItem::FunctionCallOutput {
        call_id: "call_1".to_owned(),
        output: "{}".to_owned(),
    };
    client
        .create_item(
            &output,
            &ItemOptions {
                event_id: Some("event_greeting".to_owned()),
                ..ItemOptions::default()
            },
        )
        .await
        .unwrap();
    client
        .create_item(&output, &ItemOptions::default())
        .await
        .unwrap();
    client
        .create_item(
            &output,
            &ItemOptions {
                event_id: Some("event_both".to_owned()),
                previous_item_id: Some("item_root".to_owned()),
            },
        )
        .await
        .unwrap();
    finish(task).await;
}

#[tokio::test]
async fn send_text_wire_shape_unchanged() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        assert_eq!(
            raw(&mut socket).await,
            r#"{"item":{"content":[{"text":"Hello?","type":"input_text"}],"role":"user","type":"message"},"type":"conversation.item.create"}"#
        );
    })
    .await;
    let mut client = connect(&endpoint).await;
    client.send_text("Hello?").await.unwrap();
    finish(task).await;
}

#[tokio::test]
async fn send_function_output_wire_shape_unchanged() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        assert_eq!(
            raw(&mut socket).await,
            r#"{"item":{"call_id":"call_1","output":"{\"temperature_c\": 18}","type":"function_call_output"},"type":"conversation.item.create"}"#
        );
    })
    .await;
    let mut client = connect(&endpoint).await;
    client
        .send_function_output("call_1", "{\"temperature_c\": 18}")
        .await
        .unwrap();
    finish(task).await;
}

#[tokio::test]
async fn realtime_ping_sends_a_ping_frame() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        match frame(&mut socket).await {
            Message::Ping(payload) => assert_eq!(payload.as_ref(), b"keepalive-1"),
            other => panic!("expected a ping frame, got {other:?}"),
        }
        match frame(&mut socket).await {
            Message::Ping(payload) => assert!(payload.is_empty()),
            other => panic!("expected an empty ping frame, got {other:?}"),
        }
    })
    .await;
    let mut client = connect(&endpoint).await;
    client.ping(b"keepalive-1").await.unwrap();
    client.ping(&[]).await.unwrap();
    finish(task).await;
}

#[tokio::test]
async fn ping_payload_over_125_bytes_is_refused() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        match frame(&mut socket).await {
            Message::Ping(payload) => assert_eq!(payload.as_ref(), [7_u8; 125]),
            other => panic!("the refused ping reached the wire, or no ping did: {other:?}"),
        }
        assert_eq!(
            receive(&mut socket).await,
            json!({"type": "input_audio_buffer.commit"})
        );
    })
    .await;
    let mut client = connect(&endpoint).await;
    let refused = client.ping(&[7_u8; 126]).await;
    assert!(
        matches!(&refused, Err(RealtimeError::Transport(detail)) if detail.contains("125")),
        "{refused:?}"
    );
    client.ping(&[7_u8; 125]).await.unwrap();
    client.commit_audio().await.unwrap();
    finish(task).await;
}
