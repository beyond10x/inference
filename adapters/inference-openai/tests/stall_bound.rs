//! Offline tests of the Realtime stall bound across reads (story
//! `realtime-live-stall-bound-across-reads`), against a local WebSocket server in the test. No
//! network and no key: the credential is a fixed test value from an in-test resolver.
//!
//! The bound runs from the last message the client read (text or binary; control frames do not
//! count), starting at the end of the handshake, not from each call of `next_event`: a caller
//! that drops its read and calls again still sees `Stalled` once the read timeout has passed
//! since the last message. The read timeout is 400 ms, a dropped read 100 ms.

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

/// The endpoint's read timeout in every test.
const READ_TIMEOUT: Duration = Duration::from_millis(400);

/// How long a caller waits before it drops a read and calls again, shorter than the read timeout.
const DROPPED_READ: Duration = Duration::from_millis(100);

/// How late after the bound a stall may still be reported under load.
const SLACK: Duration = Duration::from_millis(250);

/// How early before the bound a stall may be reported: the last message was read just before the
/// test takes its instant.
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

/// Binds a local server, accepts one connection and runs `script` on it.
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
        .with_read_timeout(READ_TIMEOUT);
    (endpoint, task)
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

/// The server half of session setup: `session.created`, the client's `session.update`, then
/// `session.updated`.
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

/// Reads on until the client goes away, answering nothing.
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

/// Calls `$read` again each time [`DROPPED_READ`] passes without an answer, dropping the read,
/// until a read answers; fails after four read timeouts without one.
macro_rules! read_dropping {
    ($read:expr) => {{
        let given_up = Instant::now() + READ_TIMEOUT * 4;
        loop {
            match tokio::time::timeout(DROPPED_READ, $read).await {
                Ok(answer) => break answer,
                Err(_) => assert!(
                    Instant::now() < given_up,
                    "no answer within four read timeouts of dropped reads: the bound moved with each call"
                ),
            }
        }
    }};
}

/// Asserts `answer` is a stall reported one read timeout after `since`, not earlier and not
/// later.
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

#[tokio::test]
async fn realtime_stall_bound_holds_across_dropped_reads() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        hold_open(socket).await;
    })
    .await;
    let mut client = connect(&endpoint).await;
    let since = Instant::now();
    let answer = read_dropping!(client.next_event());
    assert_stalled_at(&answer, since);
    drop(client);
    task.abort();
}

#[tokio::test]
async fn realtime_receiver_stall_bound_holds_across_dropped_reads() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        hold_open(socket).await;
    })
    .await;
    let (sender, mut receiver) = connect(&endpoint).await.into_split();
    let since = Instant::now();
    let answer = read_dropping!(receiver.next_event());
    assert_stalled_at(&answer, since);
    drop((sender, receiver));
    task.abort();
}

#[tokio::test]
async fn stall_bound_restarts_on_each_message() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        for n in 0..6 {
            tokio::time::sleep(READ_TIMEOUT / 2).await;
            send(&mut socket, tick(n)).await;
        }
        hold_open(socket).await;
    })
    .await;
    let mut client = connect(&endpoint).await;
    for n in 0..6 {
        let answer = read_dropping!(client.next_event());
        assert_eq!(
            answer.expect("a message every half read timeout never stalls the reader"),
            tick_event(n)
        );
    }
    let since = Instant::now();
    let answer = read_dropping!(client.next_event());
    assert_stalled_at(&answer, since);
    drop(client);
    task.abort();
}

#[tokio::test]
async fn pings_do_not_restart_the_stall_bound() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        for _ in 0..16 {
            tokio::time::sleep(READ_TIMEOUT / 4).await;
            if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                return;
            }
            if socket.send(Message::Pong(Vec::new().into())).await.is_err() {
                return;
            }
        }
        hold_open(socket).await;
    })
    .await;
    let mut client = connect(&endpoint).await;
    let since = Instant::now();
    let answer = read_dropping!(client.next_event());
    assert_stalled_at(&answer, since);
    drop(client);
    task.abort();
}

/// A message that arrived before the bound passed is returned by the first read after it, and
/// the bound then runs from that read.
#[tokio::test]
async fn buffered_message_is_returned_after_the_bound_passed() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
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
        answer.expect("a buffered message, not a stall"),
        tick_event(0)
    );
    let since = Instant::now();
    let answer = read_dropping!(client.next_event());
    assert_stalled_at(&answer, since);
    drop(client);
    task.abort();
}

/// After a stall the bound runs again from the stall: the next read waits one read timeout more
/// instead of returning at once, so a caller that reads on past `Stalled` does not spin.
#[tokio::test]
async fn the_bound_runs_again_from_a_stall() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket).await;
        hold_open(socket).await;
    })
    .await;
    let mut client = connect(&endpoint).await;
    let since = Instant::now();
    let answer = read_dropping!(client.next_event());
    assert_stalled_at(&answer, since);
    let since = Instant::now();
    let answer = read_dropping!(client.next_event());
    assert_stalled_at(&answer, since);
    drop(client);
    task.abort();
}
