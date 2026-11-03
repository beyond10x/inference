//! Adversary cases for the keepalive ping of `LiveClient`, against a local WebSocket server in the
//! test. No network and no key: the credential is a fixed test value from an in-test resolver.
//!
//! Each case reads a claim of the doc comment on `LiveClient::ping` and checks it on the wire.

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use inference_openai::live::{LiveClient, LiveEvent, LiveSessionConfig};
use inference_openai::{
    Credential, CredentialError, CredentialFuture, CredentialSource, Endpoint, OPENAI_KEY_NAME,
    RealtimeError,
};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

const CREDENTIAL: &str = "offline-test-credential";
const LIVE_MODEL: &str = "gpt-live-1";

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
    let endpoint = Endpoint::live(LIVE_MODEL)
        .with_url(format!("ws://{address}/v1/live/sessions"))
        .with_read_timeout(Duration::from_secs(5));
    (endpoint, task)
}

async fn frame(socket: &mut Socket) -> Message {
    tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("the client sent nothing for five seconds")
        .expect("the client closed the connection")
        .unwrap()
}

async fn receive(socket: &mut Socket) -> Value {
    match frame(socket).await {
        Message::Text(text) => serde_json::from_str(text.as_str()).unwrap(),
        other => panic!("expected a text frame, got {other:?}"),
    }
}

async fn send(socket: &mut Socket, event: Value) {
    socket.send(Message::text(event.to_string())).await.unwrap();
}

async fn accept_start(socket: &mut Socket) {
    let start = receive(socket).await;
    assert_eq!(start["type"], "session.start", "{start}");
    send(
        socket,
        json!({
            "type": "session.started",
            "event_id": "event_started_1",
            "session": {"id": "live_123", "expires_at": 1_788_307_200, "status": "active", "model": "gpt-live-1"}
        }),
    )
    .await;
}

async fn connect(endpoint: &Endpoint) -> LiveClient {
    LiveClient::connect(
        endpoint,
        &FixedSource,
        OPENAI_KEY_NAME,
        &LiveSessionConfig::default(),
    )
    .await
    .expect("the session starts")
}

async fn finish(task: JoinHandle<()>) {
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("the server script did not finish")
        .expect("the server script failed");
}

/// `LiveClient::ping`'s doc: the pong "is not a server event: `next_event` skips it".
#[tokio::test]
async fn adv_live_pong_answer_is_skipped_and_the_next_event_returned() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_start(&mut socket).await;
        match frame(&mut socket).await {
            Message::Ping(payload) => {
                assert_eq!(payload.as_ref(), [0xff_u8, 0x00, 0x7f]);
                socket.send(Message::Pong(payload)).await.unwrap();
            }
            other => panic!("expected a ping frame, got {other:?}"),
        }
        send(
            &mut socket,
            json!({"type": "info", "code": "after_pong", "message": "after the pong"}),
        )
        .await;
    })
    .await;
    let mut client = connect(&endpoint).await;
    client.ping(&[0xff, 0x00, 0x7f]).await.unwrap();
    match client.next_event().await {
        Ok(LiveEvent::Info { code, .. }) => assert_eq!(code, "after_pong"),
        other => panic!("expected the event after the pong, got {other:?}"),
    }
    finish(task).await;
}

/// `LiveClient::ping`'s doc: a pong "does not reset the read timeout".
#[tokio::test]
async fn adv_live_pongs_without_events_still_stall() {
    let (endpoint, server) = serve(|mut socket| async move {
        accept_start(&mut socket).await;
        for _ in 0..24 {
            socket
                .send(Message::Pong(b"keepalive".to_vec().into()))
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        send(
            &mut socket,
            json!({"type": "info", "code": "late", "message": "late"}),
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

/// `LiveClient::ping`'s doc: `Transport` "when the write fails"; after the server closed, a
/// ping is that error, not a panic.
#[tokio::test]
async fn adv_live_ping_after_server_close_is_a_transport_error() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_start(&mut socket).await;
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
