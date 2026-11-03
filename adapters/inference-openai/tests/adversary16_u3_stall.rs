//! Adversary cases for the Realtime stall bound across reads (story
//! `realtime-live-stall-bound-across-reads`), against a local WebSocket server in the test. No
//! network and no key: the credential is a fixed test value from an in-test resolver.
//!
//! The read timeout is 400 ms, a dropped read 100 ms.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use inference_openai::{
    Credential, CredentialError, CredentialFuture, CredentialSource, Endpoint, OPENAI_KEY_NAME,
    RealtimeClient, RealtimeError, ServerEvent, SessionConfig,
};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

const CREDENTIAL: &str = "offline-test-credential";
const MODEL: &str = "gpt-realtime-2.1";

const READ_TIMEOUT: Duration = Duration::from_millis(400);

const DROPPED_READ: Duration = Duration::from_millis(100);

const SLACK: Duration = Duration::from_millis(250);

const EARLY: Duration = Duration::from_millis(60);

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

async fn serve_with<F, Fut>(read_timeout: Duration, script: F) -> (Endpoint, JoinHandle<()>)
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
        .with_read_timeout(read_timeout);
    (endpoint, task)
}

async fn serve<F, Fut>(script: F) -> (Endpoint, JoinHandle<()>)
where
    F: FnOnce(Socket) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    serve_with(READ_TIMEOUT, script).await
}

async fn send(socket: &mut Socket, event: Value) {
    socket.send(Message::text(event.to_string())).await.unwrap();
}

async fn receive(socket: &mut Socket) -> Value {
    let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("the client sent nothing for five seconds")
        .expect("the client closed the connection")
        .unwrap();
    match frame {
        Message::Text(text) => serde_json::from_str(text.as_str()).unwrap(),
        other => panic!("expected a text frame, got {other:?}"),
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
    send(
        socket,
        json!({"type": "session.updated", "event_id": "event_2", "session": update["session"]}),
    )
    .await;
}

async fn hold_open(mut socket: Socket) {
    while let Some(Ok(_)) = socket.next().await {}
}

async fn connect(endpoint: &Endpoint) -> RealtimeClient {
    tokio::time::timeout(
        Duration::from_secs(10),
        RealtimeClient::connect(
            endpoint,
            &FixedSource,
            OPENAI_KEY_NAME,
            &SessionConfig::default(),
        ),
    )
    .await
    .expect("setup outlived its bound")
    .expect("session setup")
}

fn tick(n: u64) -> Value {
    json!({"type": "test.tick", "event_id": format!("tick_{n}")})
}

fn tick_event(n: u64) -> ServerEvent {
    ServerEvent::Unknown {
        event_type: "test.tick".to_owned(),
        raw: tick(n),
    }
}

macro_rules! read_dropping {
    ($read:expr) => {{
        let given_up = Instant::now() + READ_TIMEOUT * 4;
        loop {
            match tokio::time::timeout(DROPPED_READ, $read).await {
                Ok(answer) => break answer,
                Err(_) => assert!(
                    Instant::now() < given_up,
                    "no answer within four read timeouts of dropped reads"
                ),
            }
        }
    }};
}

fn assert_stalled_at(answer: &Result<ServerEvent, RealtimeError>, since: Instant) {
    let waited = since.elapsed();
    assert!(
        matches!(answer, Err(RealtimeError::Stalled(bound)) if *bound == READ_TIMEOUT),
        "expected a stall of {READ_TIMEOUT:?}, got {answer:?} after {waited:?}"
    );
    assert!(
        waited + EARLY >= READ_TIMEOUT,
        "stalled after {waited:?}, before the read timeout {READ_TIMEOUT:?}"
    );
    assert!(
        waited <= READ_TIMEOUT + SLACK,
        "stalled after {waited:?}, later than the read timeout {READ_TIMEOUT:?}"
    );
}

/// A binary message is a message: reading it moves the bound, so the stall after it comes one
/// read timeout after that read, not one read timeout after the setup. No other test sends a
/// binary message, so a read path that left the bound in place on binary stayed green.
#[tokio::test]
async fn adversary_a_binary_message_moves_the_stall_bound() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        tokio::time::sleep(READ_TIMEOUT * 3 / 4).await;
        socket
            .send(Message::Binary(vec![1, 2, 3].into()))
            .await
            .unwrap();
        hold_open(socket).await;
    })
    .await;
    let mut client = connect(&endpoint).await;
    let answer = read_dropping!(client.next_event());
    assert!(
        matches!(answer, Err(RealtimeError::Protocol(_))),
        "expected the binary message as a protocol error, got {answer:?}"
    );
    let since = Instant::now();
    let answer = read_dropping!(client.next_event());
    assert_stalled_at(&answer, since);
    drop(client);
    task.abort();
}

/// The bound runs from the end of setup, not from the first call: a caller that waits part of
/// the read timeout before it first reads sees the stall at the bound, not one read timeout
/// after its call.
#[tokio::test]
async fn adversary_the_bound_counts_the_time_before_the_first_read() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        hold_open(socket).await;
    })
    .await;
    let mut client = connect(&endpoint).await;
    let since = Instant::now();
    tokio::time::sleep(READ_TIMEOUT * 3 / 4).await;
    let answer = read_dropping!(client.next_event());
    assert_stalled_at(&answer, since);
    drop(client);
    task.abort();
}

/// A message that arrived behind pings before the bound passed is still returned by the first
/// read after the bound: the pings in front of it are skipped, not a reason to report a stall.
#[tokio::test]
async fn adversary_a_buffered_message_behind_pings_is_returned_after_the_bound() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        for _ in 0..3 {
            socket.send(Message::Ping(Vec::new().into())).await.unwrap();
        }
        send(&mut socket, tick(0)).await;
        hold_open(socket).await;
    })
    .await;
    let mut client = connect(&endpoint).await;
    tokio::time::sleep(READ_TIMEOUT * 2).await;
    let answer = tokio::time::timeout(DROPPED_READ, client.next_event())
        .await
        .expect("the buffered message is returned at once");
    assert_eq!(
        answer.expect("the buffered message, not a stall"),
        tick_event(0)
    );
    drop(client);
    task.abort();
}

/// `Endpoint::read_timeout` is a public `Duration` with no documented ceiling, so a caller that
/// wants no stall bound sets `Duration::MAX`. The bound is `last_message + read_timeout`, an
/// `Instant` addition that overflows: the read panics instead of waiting.
#[tokio::test]
async fn adversary_a_read_timeout_of_duration_max_does_not_panic() {
    let (endpoint, task) = serve_with(Duration::MAX, |mut socket| async move {
        accept_session(&mut socket).await;
        hold_open(socket).await;
    })
    .await;
    let reader = tokio::spawn(async move {
        let mut client = RealtimeClient::connect(
            &endpoint,
            &FixedSource,
            OPENAI_KEY_NAME,
            &SessionConfig::default(),
        )
        .await
        .expect("session setup");
        let waited = tokio::time::timeout(DROPPED_READ, client.next_event()).await;
        assert!(
            waited.is_err(),
            "an unbounded read answered before the server sent anything: {waited:?}"
        );
    });
    let joined = tokio::time::timeout(Duration::from_secs(10), reader)
        .await
        .expect("the reader outlived ten seconds");
    if let Err(error) = joined {
        let message = error
            .try_into_panic()
            .ok()
            .and_then(|payload| {
                payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            })
            .unwrap_or_default();
        panic!("a read with read_timeout Duration::MAX panicked: {message}");
    }
    task.abort();
}
