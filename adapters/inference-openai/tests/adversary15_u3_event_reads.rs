//! Adversary cases, pass 15, unit U3: `input_audio_buffer.committed` with its
//! `previous_item_id`, and `response.function_call_arguments.done` without its `name`, the name
//! carried from the `function_call` item of `response.output_item.added` or
//! `response.output_item.done`. A local WebSocket server replays the provider; no network and no
//! key.
//!
//! The published Realtime reference sends a function call's events in the order
//! `response.output_item.added`, `response.function_call_arguments.delta`,
//! `response.function_call_arguments.done`, `response.output_item.done`: the item's `done` comes
//! after its call.

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

async fn accept_session(socket: &mut Socket) {
    send(
        socket,
        json!({"type": "session.created", "event_id": "event_1", "session": {"type": "realtime", "id": "sess_1"}}),
    )
    .await;
    let update = receive(socket).await;
    assert_eq!(update["type"], "session.update", "{update}");
    let mut confirmed = update["session"].clone();
    confirmed["id"] = json!("sess_1");
    send(
        socket,
        json!({"type": "session.updated", "event_id": "event_2", "session": confirmed}),
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

fn arguments_done(item: &str, call: &str, name: Option<&str>) -> Value {
    let mut event = json!({
        "type": "response.function_call_arguments.done",
        "event_id": "event_fc",
        "response_id": "resp_1",
        "item_id": item,
        "output_index": 0,
        "call_id": call,
        "arguments": "{}"
    });
    if let Some(name) = name {
        event["name"] = json!(name);
    }
    event
}

fn call_name(event: &ServerEvent) -> Option<&str> {
    match event {
        ServerEvent::FunctionCall { name, .. } => name.as_deref(),
        other => panic!("expected a function call, got {other:?}"),
    }
}

/// Reads `count` events, returning the names of the function calls among them in order.
async fn call_names(client: &mut RealtimeClient, count: usize) -> Vec<Option<String>> {
    let mut names = Vec::new();
    for _ in 0..count {
        if let ServerEvent::FunctionCall { name, .. } = client.next_event().await.unwrap() {
            names.push(name);
        }
    }
    names
}

/// The reference order: the item's `done` arrives after its call was read and filled. The
/// documented memory forgets an entry "once the call of its item is read"; a repeated call of
/// that item must not be named again from the item's `done`.
#[tokio::test]
async fn an_output_item_done_after_its_call_is_not_remembered_again() {
    let (endpoint, server) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        send(
            &mut socket,
            function_call_item("response.output_item.added", "fc_1", "call_1", "weather"),
        )
        .await;
        send(&mut socket, arguments_done("fc_1", "call_1", None)).await;
        send(
            &mut socket,
            function_call_item("response.output_item.done", "fc_1", "call_1", "weather"),
        )
        .await;
        send(&mut socket, arguments_done("fc_1", "call_1", None)).await;
        let _ = socket.next().await;
    })
    .await;
    let mut client = connect(&endpoint).await;
    let names = call_names(&mut client, 4).await;
    assert_eq!(
        names,
        [Some("weather".to_owned()), None],
        "the entry of fc_1 was forgotten when its call was read; the later output_item.done \
         must not bring it back"
    );
    drop(client);
    server.await.unwrap();
}

/// A server that sends the call with an empty `name` sent no name the binding can use; the
/// output item named the function. The carried name should win over the empty string.
#[tokio::test]
async fn an_empty_name_on_the_call_is_filled_from_the_output_item() {
    let (endpoint, server) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        send(
            &mut socket,
            function_call_item("response.output_item.added", "fc_1", "call_1", "weather"),
        )
        .await;
        send(&mut socket, arguments_done("fc_1", "call_1", Some(""))).await;
        let _ = socket.next().await;
    })
    .await;
    let mut client = connect(&endpoint).await;
    let _added = client.next_event().await.unwrap();
    let call = client.next_event().await.unwrap();
    assert_eq!(call_name(&call), Some("weather"), "{call:?}");
    drop(client);
    server.await.unwrap();
}

/// Hostile output items: `item` not an object, `item.id` or `item.name` not a string, `item.type`
/// another kind. None panics, each is still returned as it was, and none names a call.
#[tokio::test]
async fn hostile_output_items_pass_through_and_name_nothing() {
    let hostile = [
        json!({"type": "response.output_item.added", "item": "fc_1"}),
        json!({"type": "response.output_item.added", "item": ["fc_1", "weather"]}),
        json!({"type": "response.output_item.added", "item": null}),
        json!({"type": "response.output_item.added"}),
        json!({"type": "response.output_item.added", "item": {"id": 1, "type": "function_call", "name": "weather"}}),
        json!({"type": "response.output_item.added", "item": {"id": "fc_1", "type": "function_call", "name": 5}}),
        json!({"type": "response.output_item.added", "item": {"id": "fc_1", "type": "function_call", "name": null}}),
        json!({"type": "response.output_item.added", "item": {"id": "fc_1", "type": "message", "name": "weather"}}),
        json!({"type": "response.output_item.added", "item": {"id": "fc_1", "type": ["function_call"], "name": "weather"}}),
        json!({"type": "conversation.item.added", "item": {"id": "fc_1", "type": "function_call", "name": "weather"}}),
    ];
    let count = hostile.len();
    let (endpoint, server) = serve(move |mut socket| async move {
        accept_session(&mut socket).await;
        for event in hostile {
            send(&mut socket, event).await;
        }
        send(&mut socket, arguments_done("fc_1", "call_1", None)).await;
        let _ = socket.next().await;
    })
    .await;
    let mut client = connect(&endpoint).await;
    for _ in 0..count {
        let event = client.next_event().await.unwrap();
        assert!(matches!(event, ServerEvent::Unknown { .. }), "{event:?}");
    }
    assert_eq!(call_name(&client.next_event().await.unwrap()), None);
    drop(client);
    server.await.unwrap();
}

/// The fill matches on `item_id`: an output item with the call's `call_id` but another `id`
/// names nothing.
#[tokio::test]
async fn the_fill_matches_the_item_id_not_the_call_id() {
    let (endpoint, server) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        send(
            &mut socket,
            function_call_item(
                "response.output_item.added",
                "fc_other",
                "call_1",
                "weather",
            ),
        )
        .await;
        send(&mut socket, arguments_done("fc_1", "call_1", None)).await;
        let _ = socket.next().await;
    })
    .await;
    let mut client = connect(&endpoint).await;
    let _added = client.next_event().await.unwrap();
    assert_eq!(call_name(&client.next_event().await.unwrap()), None);
    drop(client);
    server.await.unwrap();
}

/// A later output item of the same id renames the entry: the call reads the latest name.
#[tokio::test]
async fn a_later_output_item_of_the_same_id_renames_the_call() {
    let (endpoint, server) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        send(
            &mut socket,
            function_call_item("response.output_item.added", "fc_1", "call_1", "first"),
        )
        .await;
        send(
            &mut socket,
            function_call_item("response.output_item.done", "fc_1", "call_1", "second"),
        )
        .await;
        send(&mut socket, arguments_done("fc_1", "call_1", None)).await;
        let _ = socket.next().await;
    })
    .await;
    let mut client = connect(&endpoint).await;
    let names = call_names(&mut client, 3).await;
    assert_eq!(names, [Some("second".to_owned())]);
    drop(client);
    server.await.unwrap();
}

/// A call whose own parse fails (no `arguments`) is an error, not a fill; the entry stays for the
/// well-formed call of the same item.
#[tokio::test]
async fn a_malformed_call_does_not_consume_the_remembered_name() {
    let (endpoint, server) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        send(
            &mut socket,
            function_call_item("response.output_item.added", "fc_1", "call_1", "weather"),
        )
        .await;
        let mut broken = arguments_done("fc_1", "call_1", None);
        broken.as_object_mut().unwrap().remove("arguments");
        send(&mut socket, broken).await;
        send(&mut socket, arguments_done("fc_1", "call_1", None)).await;
        let _ = socket.next().await;
    })
    .await;
    let mut client = connect(&endpoint).await;
    let _added = client.next_event().await.unwrap();
    let refused = client.next_event().await;
    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(
        client.unparsed_type(),
        Some("response.function_call_arguments.done")
    );
    assert_eq!(
        call_name(&client.next_event().await.unwrap()),
        Some("weather")
    );
    drop(client);
    server.await.unwrap();
}

/// `previous_item_id` is read alongside `item_id` from the reference example; the 0.9.0 field
/// is unchanged.
#[test]
fn committed_reads_both_ids_and_an_empty_previous_item_id_is_kept() {
    let committed = ServerEvent::parse(
        &json!({"type": "input_audio_buffer.committed", "event_id": "event_1121", "previous_item_id": "", "item_id": "msg_002"})
            .to_string(),
    )
    .unwrap();
    assert_eq!(
        committed,
        ServerEvent::AudioCommitted {
            item_id: "msg_002".to_owned(),
            previous_item_id: Some(String::new()),
        }
    );
    let refused = ServerEvent::parse(
        &json!({"type": "input_audio_buffer.committed", "previous_item_id": "msg_001"}).to_string(),
    );
    assert!(refused.is_err(), "item_id stays required: {refused:?}");
}
