//! Offline tests of the GPT-Live keepalive ping against a local WebSocket server in the test. No
//! network and no key: the credential is a fixed test value from an in-test resolver. The ping
//! frame is RFC 6455 section 5.5.2.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use inference_openai::live::{LiveClient, LiveSessionConfig};
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

/// A local server running `script`; the endpoint points at it.
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

/// The next frame the client sent, whatever its kind; fails the script after five seconds.
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

/// Startup: reads `session.start` and answers `session.started` with the documented excerpt.
async fn accept_start(socket: &mut Socket) {
    let start = receive(socket).await;
    assert_eq!(start["type"], "session.start", "{start}");
    let started = json!({
        "type": "session.started",
        "event_id": "event_started_1",
        "session": {"id": "live_123", "expires_at": 1_788_307_200, "status": "active", "model": "gpt-live-1"}
    });
    socket
        .send(Message::text(started.to_string()))
        .await
        .unwrap();
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

#[tokio::test]
async fn live_ping_sends_a_ping_frame() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_start(&mut socket).await;
        match frame(&mut socket).await {
            Message::Ping(payload) => assert_eq!(payload.as_ref(), b"keepalive-live"),
            other => panic!("expected a ping frame, got {other:?}"),
        }
    })
    .await;
    let mut client = connect(&endpoint).await;
    client.ping(b"keepalive-live").await.unwrap();
    finish(task).await;
}

#[tokio::test]
async fn live_ping_payload_over_125_bytes_is_refused() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_start(&mut socket).await;
        match frame(&mut socket).await {
            Message::Ping(payload) => assert_eq!(payload.as_ref(), [3_u8; 125]),
            other => panic!("the refused ping reached the wire, or no ping did: {other:?}"),
        }
        assert_eq!(
            receive(&mut socket).await["type"],
            "session.input_audio.mute"
        );
    })
    .await;
    let mut client = connect(&endpoint).await;
    let refused = client.ping(&[3_u8; 126]).await;
    assert!(
        matches!(&refused, Err(RealtimeError::Transport(detail)) if detail.contains("125")),
        "{refused:?}"
    );
    client.ping(&[3_u8; 125]).await.unwrap();
    client.mute().await.unwrap();
    finish(task).await;
}
