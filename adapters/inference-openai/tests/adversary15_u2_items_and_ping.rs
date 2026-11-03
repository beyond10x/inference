//! Adversary cases for conversation items with placement and event ids, and for the keepalive
//! ping of `RealtimeClient`, against a local WebSocket server in the test. No network and no key:
//! the credential is a fixed test value from an in-test resolver.
//!
//! Each case reads a claim of the doc comments on `RealtimeClient::create_item`,
//! `RealtimeClient::ping` and `ItemOptions` and checks it on the wire.

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use inference_openai::{
    ConversationItem, Credential, CredentialError, CredentialFuture, CredentialSource, Endpoint,
    ItemOptions, OPENAI_KEY_NAME, RealtimeClient, RealtimeError, ServerEvent, SessionConfig,
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

async fn frame(socket: &mut Socket) -> Message {
    tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("the client sent nothing for five seconds")
        .expect("the client closed the connection")
        .unwrap()
}

async fn raw(socket: &mut Socket) -> String {
    match frame(socket).await {
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

/// `client.rs` `create_item`: both options at the top level, byte for byte, and no `null` for
/// an unset key anywhere in the event.
#[tokio::test]
async fn adv_create_item_with_both_options_exact_bytes() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        assert_eq!(
            raw(&mut socket).await,
            r#"{"event_id":"evt_a","item":{"content":[{"text":"Welcome.","type":"output_text"}],"role":"assistant","type":"message"},"previous_item_id":"item_root","type":"conversation.item.create"}"#
        );
        let unset = raw(&mut socket).await;
        assert!(!unset.contains("null"), "{unset}");
        assert!(!unset.contains("event_id"), "{unset}");
        assert!(!unset.contains("previous_item_id"), "{unset}");
    })
    .await;
    let mut client = connect(&endpoint).await;
    client
        .create_item(
            &ConversationItem::AssistantText("Welcome.".to_owned()),
            &ItemOptions {
                event_id: Some("evt_a".to_owned()),
                previous_item_id: Some("item_root".to_owned()),
            },
        )
        .await
        .unwrap();
    client
        .create_item(
            &ConversationItem::AssistantText("Welcome.".to_owned()),
            &ItemOptions::default(),
        )
        .await
        .unwrap();
    finish(task).await;
}

/// `UserText` through `create_item` with default options is the same bytes as `send_text`.
#[tokio::test]
async fn adv_user_text_item_equals_send_text_bytes() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        let via_item = raw(&mut socket).await;
        let via_send_text = raw(&mut socket).await;
        assert_eq!(via_item, via_send_text);
        assert_eq!(
            via_item,
            r#"{"item":{"content":[{"text":"Hi.","type":"input_text"}],"role":"user","type":"message"},"type":"conversation.item.create"}"#
        );
    })
    .await;
    let mut client = connect(&endpoint).await;
    client
        .create_item(
            &ConversationItem::UserText("Hi.".to_owned()),
            &ItemOptions::default(),
        )
        .await
        .unwrap();
    client.send_text("Hi.").await.unwrap();
    finish(task).await;
}

/// Text with quotes, a newline and characters outside ASCII reaches the wire unchanged.
#[tokio::test]
async fn adv_assistant_text_is_escaped_and_round_trips() {
    const TEXT: &str = "say \"hi\"\n\\ \u{2014} \u{2713} \u{1F3A7}";
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        let event = receive(&mut socket).await;
        assert_eq!(event["item"]["content"][0]["text"], TEXT, "{event}");
        assert_eq!(event["item"]["content"].as_array().map(Vec::len), Some(1));
    })
    .await;
    let mut client = connect(&endpoint).await;
    client
        .create_item(
            &ConversationItem::AssistantText(TEXT.to_owned()),
            &ItemOptions::default(),
        )
        .await
        .unwrap();
    finish(task).await;
}

/// `create_item`'s doc: "an `error` naming the `event_id` when the event is refused, arrive[s]
/// through `next_event`". The story's reason for the event id is matching that error.
#[tokio::test]
async fn adv_error_names_the_event_id_create_item_sent() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        let sent = receive(&mut socket).await;
        send(
            &mut socket,
            json!({
                "type": "error",
                "event_id": "event_9",
                "error": {
                    "type": "invalid_request_error",
                    "code": "item_not_found",
                    "message": "previous_item_id not found",
                    "param": "previous_item_id",
                    "event_id": sent["event_id"],
                },
            }),
        )
        .await;
    })
    .await;
    let mut client = connect(&endpoint).await;
    client
        .create_item(
            &ConversationItem::AssistantText("Welcome.".to_owned()),
            &ItemOptions {
                event_id: Some("evt_greeting".to_owned()),
                previous_item_id: Some("item_missing".to_owned()),
            },
        )
        .await
        .unwrap();
    match client.next_event().await {
        Ok(ServerEvent::Error(error)) => {
            assert_eq!(error.event_id.as_deref(), Some("evt_greeting"));
        }
        other => panic!("expected the error event, got {other:?}"),
    }
    finish(task).await;
}

/// `ping`'s doc: "The server's pong is not a server event: `next_event` skips it". The server
/// answers the ping with its pong, then sends an event: the event is what `next_event` returns.
#[tokio::test]
async fn adv_pong_answer_is_skipped_and_the_next_event_returned() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        match frame(&mut socket).await {
            Message::Ping(payload) => {
                socket.send(Message::Pong(payload)).await.unwrap();
            }
            other => panic!("expected a ping frame, got {other:?}"),
        }
        send(
            &mut socket,
            json!({"type": "after.pong", "event_id": "event_3"}),
        )
        .await;
    })
    .await;
    let mut client = connect(&endpoint).await;
    client.ping(b"keepalive").await.unwrap();
    match client.next_event().await {
        Ok(ServerEvent::Unknown { event_type, .. }) => assert_eq!(event_type, "after.pong"),
        other => panic!("expected the event after the pong, got {other:?}"),
    }
    finish(task).await;
}

/// `ping`'s doc: a pong "does not reset the read timeout". Pongs every 50 ms and no event: the
/// 200 ms read timeout still passes.
#[tokio::test]
async fn adv_pongs_without_events_still_stall() {
    let (endpoint, server) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        for _ in 0..24 {
            socket
                .send(Message::Pong(b"keepalive".to_vec().into()))
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        send(
            &mut socket,
            json!({"type": "a.late.event", "event_id": "event_9"}),
        )
        .await;
        while let Some(Ok(_)) = socket.next().await {}
    })
    .await;
    let endpoint = endpoint.with_read_timeout(Duration::from_millis(200));
    let mut client = connect(&endpoint).await;
    let started = Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(5), client.next_event())
        .await
        .expect("next_event returned within five seconds");
    let waited = started.elapsed();
    drop(client);
    server.abort();
    assert!(
        matches!(result, Err(RealtimeError::Stalled(_))),
        "no server event for {waited:?} with a 200 ms read timeout, got {result:?}"
    );
}

/// A refused payload far over the limit writes nothing: the next frame on the wire is the event
/// sent after it.
#[tokio::test]
async fn adv_large_ping_payload_is_refused_and_nothing_written() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        assert_eq!(
            receive(&mut socket).await,
            json!({"type": "input_audio_buffer.clear"})
        );
    })
    .await;
    let mut client = connect(&endpoint).await;
    for size in [126_usize, 1 << 16, (1 << 16) + 1] {
        let refused = client.ping(&vec![0_u8; size]).await;
        assert!(
            matches!(&refused, Err(RealtimeError::Transport(detail)) if detail.contains(&size.to_string())),
            "{size}: {refused:?}"
        );
    }
    client.clear_audio().await.unwrap();
    finish(task).await;
}

/// `ping`'s doc: `Transport` "when the write fails". After the server closed, a ping is that
/// error, not a panic.
#[tokio::test]
async fn adv_ping_after_server_close_is_a_transport_error() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        socket.close(None).await.unwrap();
        while let Some(Ok(_)) = socket.next().await {}
    })
    .await;
    let mut client = connect(&endpoint).await;
    let closed = client.next_event().await;
    assert!(
        matches!(closed, Err(RealtimeError::Closed { .. })),
        "{closed:?}"
    );
    let ping = client.ping(b"late").await;
    assert!(matches!(ping, Err(RealtimeError::Transport(_))), "{ping:?}");
    drop(client);
    finish(task).await;
}
