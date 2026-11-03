//! Offline tests of two server event reads: `input_audio_buffer.committed` with its
//! `previous_item_id`, and `response.function_call_arguments.done` without its `name`, the name
//! carried from the `function_call` output item of `response.output_item.added` or
//! `response.output_item.done`. No network and no key: the credential is a fixed test value and
//! the server is local.
//!
//! Every replayed server event is written in the shape of the published Realtime server events
//! reference, as `tests/offline.rs` writes them.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use inference_openai::{
    Credential, CredentialError, CredentialFuture, CredentialSource, Endpoint, OPENAI_KEY_NAME,
    RealtimeClient, ServerEvent, SessionConfig,
};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

const CREDENTIAL: &str = "offline-test-credential";

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
    let endpoint = Endpoint::new("gpt-realtime-2.1")
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

fn session_created() -> Value {
    json!({"type": "session.created", "event_id": "event_1", "session": {"type": "realtime", "id": "sess_1"}})
}

/// `session.updated` confirming the session the client sent in `update`.
fn session_updated(update: &Value) -> Value {
    let mut confirmed = update["session"].clone();
    confirmed["id"] = json!("sess_1");
    json!({"type": "session.updated", "event_id": "event_2", "session": confirmed})
}

/// The server half of session setup: `session.created`, the client's `session.update`, then
/// `session.updated`.
async fn accept_session(socket: &mut Socket) {
    send(socket, session_created()).await;
    let update = receive(socket).await;
    assert_eq!(update["type"], "session.update", "{update}");
    send(socket, session_updated(&update)).await;
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

/// `response.output_item.added` (or `done`, by `event_type`) of a `function_call` item.
fn function_call_item(event_type: &str, item: &str, call: &str, name: &str) -> Value {
    json!({
        "type": event_type,
        "event_id": "event_oi",
        "response_id": "resp_1",
        "output_index": 0,
        "item": {
            "id": item,
            "object": "realtime.item",
            "type": "function_call",
            "status": "in_progress",
            "call_id": call,
            "name": name,
            "arguments": ""
        }
    })
}

/// `response.function_call_arguments.done` for `item` and `call`, with `name` when given.
fn arguments_done(item: &str, call: &str, name: Option<&str>) -> Value {
    let mut event = json!({
        "type": "response.function_call_arguments.done",
        "event_id": "event_fc",
        "response_id": "resp_1",
        "item_id": item,
        "output_index": 0,
        "call_id": call,
        "arguments": "{\"location\": \"San Francisco\"}"
    });
    if let Some(name) = name {
        event["name"] = json!(name);
    }
    event
}

fn parse(event: &Value) -> ServerEvent {
    ServerEvent::parse(&event.to_string()).expect("a server event")
}

fn call_name(event: &ServerEvent) -> Option<&str> {
    match event {
        ServerEvent::FunctionCall { name, .. } => name.as_deref(),
        other => panic!("expected a function call, got {other:?}"),
    }
}

#[test]
fn committed_keeps_previous_item_id() {
    let committed = parse(&json!({
        "event_id": "event_1121",
        "type": "input_audio_buffer.committed",
        "previous_item_id": "msg_001",
        "item_id": "msg_002"
    }));
    assert_eq!(
        committed,
        ServerEvent::AudioCommitted {
            item_id: "msg_002".to_owned(),
            previous_item_id: Some("msg_001".to_owned()),
        }
    );
}

#[test]
fn committed_with_null_previous_item_id_is_none() {
    let null = parse(&json!({
        "event_id": "event_1121",
        "type": "input_audio_buffer.committed",
        "previous_item_id": null,
        "item_id": "msg_002"
    }));
    let missing = parse(&json!({
        "event_id": "event_1121",
        "type": "input_audio_buffer.committed",
        "item_id": "msg_002"
    }));
    for committed in [null, missing] {
        assert_eq!(
            committed,
            ServerEvent::AudioCommitted {
                item_id: "msg_002".to_owned(),
                previous_item_id: None,
            }
        );
    }
}

#[test]
fn a_committed_previous_item_id_that_is_not_a_string_is_refused() {
    let error = ServerEvent::parse(
        &json!({
            "type": "input_audio_buffer.committed",
            "previous_item_id": 7,
            "item_id": "msg_002"
        })
        .to_string(),
    )
    .expect_err("a mistyped previous_item_id");
    assert!(error.to_string().contains("previous_item_id"), "{error}");
}

#[test]
fn function_call_without_name_parses() {
    let missing = parse(&arguments_done("fc_001", "call_001", None));
    let mut null = arguments_done("fc_001", "call_001", None);
    null["name"] = Value::Null;
    for call in [missing, parse(&null)] {
        assert_eq!(
            call,
            ServerEvent::FunctionCall {
                response_id: "resp_1".to_owned(),
                item_id: "fc_001".to_owned(),
                call_id: "call_001".to_owned(),
                name: None,
                arguments: "{\"location\": \"San Francisco\"}".to_owned(),
            }
        );
    }
}

#[test]
fn function_call_with_name_unchanged() {
    let call = parse(&json!({
        "event_id": "event_5556",
        "type": "response.function_call_arguments.done",
        "response_id": "resp_002",
        "item_id": "fc_001",
        "output_index": 0,
        "call_id": "call_001",
        "name": "get_weather",
        "arguments": "{\"location\": \"San Francisco\"}"
    }));
    assert_eq!(
        call,
        ServerEvent::FunctionCall {
            response_id: "resp_002".to_owned(),
            item_id: "fc_001".to_owned(),
            call_id: "call_001".to_owned(),
            name: Some("get_weather".to_owned()),
            arguments: "{\"location\": \"San Francisco\"}".to_owned(),
        }
    );
}

#[tokio::test]
async fn function_call_name_carried_from_output_item() {
    let (endpoint, server) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        send(
            &mut socket,
            function_call_item(
                "response.output_item.added",
                "fc_001",
                "call_001",
                "get_weather",
            ),
        )
        .await;
        send(&mut socket, arguments_done("fc_001", "call_001", None)).await;
        send(
            &mut socket,
            function_call_item("response.output_item.done", "fc_002", "call_002", "lookup"),
        )
        .await;
        send(&mut socket, arguments_done("fc_002", "call_002", None)).await;
        let _ = socket.next().await;
    })
    .await;
    let mut client = connect(&endpoint).await;
    let added = client.next_event().await.unwrap();
    assert!(
        matches!(&added, ServerEvent::Unknown { event_type, .. } if event_type == "response.output_item.added"),
        "the output item is still returned as it was: {added:?}"
    );
    assert_eq!(
        client.next_event().await.unwrap(),
        ServerEvent::FunctionCall {
            response_id: "resp_1".to_owned(),
            item_id: "fc_001".to_owned(),
            call_id: "call_001".to_owned(),
            name: Some("get_weather".to_owned()),
            arguments: "{\"location\": \"San Francisco\"}".to_owned(),
        }
    );
    let done = client.next_event().await.unwrap();
    assert!(
        matches!(&done, ServerEvent::Unknown { event_type, .. } if event_type == "response.output_item.done"),
        "{done:?}"
    );
    let call = client.next_event().await.unwrap();
    assert_eq!(call_name(&call), Some("lookup"));
    drop(client);
    server.await.unwrap();
}

#[tokio::test]
async fn a_name_sent_with_the_call_wins_over_the_output_item() {
    let (endpoint, server) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        send(
            &mut socket,
            function_call_item(
                "response.output_item.added",
                "fc_001",
                "call_001",
                "from_item",
            ),
        )
        .await;
        send(
            &mut socket,
            arguments_done("fc_001", "call_001", Some("from_call")),
        )
        .await;
        send(&mut socket, arguments_done("fc_001", "call_001", None)).await;
        let _ = socket.next().await;
    })
    .await;
    let mut client = connect(&endpoint).await;
    let _added = client.next_event().await.unwrap();
    let call = client.next_event().await.unwrap();
    assert_eq!(call_name(&call), Some("from_call"));
    let again = client.next_event().await.unwrap();
    assert_eq!(
        call_name(&again),
        None,
        "the entry of an item whose call was read is gone"
    );
    drop(client);
    server.await.unwrap();
}

#[tokio::test]
async fn a_carried_name_fills_one_call_only() {
    let (endpoint, server) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        send(
            &mut socket,
            function_call_item(
                "response.output_item.added",
                "fc_001",
                "call_001",
                "get_weather",
            ),
        )
        .await;
        send(&mut socket, arguments_done("fc_001", "call_001", None)).await;
        send(&mut socket, arguments_done("fc_001", "call_001", None)).await;
        send(&mut socket, arguments_done("fc_other", "call_009", None)).await;
        let _ = socket.next().await;
    })
    .await;
    let mut client = connect(&endpoint).await;
    let _added = client.next_event().await.unwrap();
    assert_eq!(
        call_name(&client.next_event().await.unwrap()),
        Some("get_weather")
    );
    assert_eq!(call_name(&client.next_event().await.unwrap()), None);
    assert_eq!(
        call_name(&client.next_event().await.unwrap()),
        None,
        "an item never named by an output item stays without a name"
    );
    drop(client);
    server.await.unwrap();
}

#[tokio::test]
async fn the_remembered_names_are_bounded_and_the_oldest_is_evicted() {
    const SENT: usize = 65;
    let (endpoint, server) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        for n in 0..SENT {
            send(
                &mut socket,
                function_call_item(
                    "response.output_item.added",
                    &format!("fc_{n}"),
                    &format!("call_{n}"),
                    &format!("tool_{n}"),
                ),
            )
            .await;
        }
        send(&mut socket, arguments_done("fc_0", "call_0", None)).await;
        send(&mut socket, arguments_done("fc_1", "call_1", None)).await;
        send(&mut socket, arguments_done("fc_64", "call_64", None)).await;
        let _ = socket.next().await;
    })
    .await;
    let mut client = connect(&endpoint).await;
    for _ in 0..SENT {
        let _added = client.next_event().await.unwrap();
    }
    assert_eq!(
        call_name(&client.next_event().await.unwrap()),
        None,
        "the oldest of 65 output items is no longer remembered"
    );
    assert_eq!(
        call_name(&client.next_event().await.unwrap()),
        Some("tool_1")
    );
    assert_eq!(
        call_name(&client.next_event().await.unwrap()),
        Some("tool_64")
    );
    drop(client);
    server.await.unwrap();
}

#[tokio::test]
async fn a_call_kept_during_setup_is_filled_too() {
    let (endpoint, server) = serve(|mut socket| async move {
        send(&mut socket, session_created()).await;
        let update = receive(&mut socket).await;
        assert_eq!(update["type"], "session.update", "{update}");
        send(
            &mut socket,
            function_call_item(
                "response.output_item.added",
                "fc_001",
                "call_001",
                "get_weather",
            ),
        )
        .await;
        send(&mut socket, arguments_done("fc_001", "call_001", None)).await;
        send(&mut socket, session_updated(&update)).await;
        let _ = socket.next().await;
    })
    .await;
    let mut client = connect(&endpoint).await;
    let _added = client.next_event().await.unwrap();
    assert_eq!(
        call_name(&client.next_event().await.unwrap()),
        Some("get_weather")
    );
    drop(client);
    server.await.unwrap();
}
