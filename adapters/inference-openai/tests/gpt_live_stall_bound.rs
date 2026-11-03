//! Offline tests of the GPT-Live stall bound across reads (story
//! `realtime-live-stall-bound-across-reads`), against a local WebSocket server in the test. No
//! network and no key: the credential is a fixed test value from an in-test resolver.
//!
//! The bound runs from the last message the client read (text or binary; control frames do not
//! count), starting at the end of the handshake, not from each call of `next_event`: a caller
//! that drops its read and calls again still sees `Stalled` once the read timeout has passed
//! since the last message. The read timeout is 400 ms, a dropped read 100 ms.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use inference_openai::live::{LiveClient, LiveEvent, LiveSessionConfig};
use inference_openai::{
    Credential, CredentialError, CredentialFuture, CredentialSource, Endpoint, OPENAI_KEY_NAME,
    RealtimeError,
};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

const CREDENTIAL: &str = "offline-test-credential";
const LIVE_MODEL: &str = "gpt-live-1";

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
    let endpoint = Endpoint::live(LIVE_MODEL)
        .with_url(format!("ws://{address}/v1/live/sessions"))
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

/// Startup: reads `session.start` and answers `session.started`.
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

/// Reads on until the client goes away, answering nothing.
async fn hold_open(mut socket: Socket) {
    while let Some(Ok(_)) = socket.next().await {}
}

async fn connect(endpoint: &Endpoint) -> LiveClient {
    tokio::time::timeout(
        Duration::from_secs(10),
        LiveClient::connect(
            endpoint,
            &FixedSource,
            OPENAI_KEY_NAME,
            &LiveSessionConfig::default(),
        ),
    )
    .await
    .expect("the start outlived its bound")
    .expect("the session starts")
}

fn tick(n: u64) -> Value {
    json!({"type": "test.tick", "event_id": format!("tick_{n}")})
}

fn tick_event(n: u64) -> LiveEvent {
    LiveEvent::Unknown {
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
fn assert_stalled_at(answer: &Result<LiveEvent, RealtimeError>, since: Instant) {
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
async fn live_stall_bound_holds_across_dropped_reads() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_start(&mut socket).await;
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
async fn live_receiver_stall_bound_holds_across_dropped_reads() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_start(&mut socket).await;
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
async fn live_stall_bound_restarts_on_each_message() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_start(&mut socket).await;
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
async fn live_pings_do_not_restart_the_stall_bound() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_start(&mut socket).await;
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
async fn live_buffered_message_is_returned_after_the_bound_passed() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_start(&mut socket).await;
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

/// A close after a silence longer than the read timeout still waits for `session.closed`: the
/// close keeps its own bound and does not inherit the stall of the reads before it.
#[tokio::test]
async fn live_close_after_a_silence_still_reads_session_closed() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_start(&mut socket).await;
        let close = loop {
            let event = receive(&mut socket).await;
            if event["type"] == "session.close" {
                break event;
            }
        };
        assert_eq!(close["type"], "session.close");
        tokio::time::sleep(Duration::from_millis(100)).await;
        send(
            &mut socket,
            json!({"type": "session.closed", "event_id": "e_closed", "reason": "close_requested", "session": {"id": "live_123", "model": "gpt-live-1", "status": "active"}, "usage": {"seconds": 1.5}}),
        )
        .await;
        let _ = socket.close(None).await;
    })
    .await;
    let mut client = connect(&endpoint).await;
    let answer = read_dropping!(client.next_event());
    assert!(
        matches!(answer, Err(RealtimeError::Stalled(_))),
        "{answer:?}"
    );
    tokio::time::sleep(READ_TIMEOUT).await;
    let closed = tokio::time::timeout(Duration::from_secs(10), client.close())
        .await
        .expect("close() outlived its bound")
        .expect("the close reads session.closed");
    assert_eq!(closed.reason.as_deref(), Some("close_requested"));
    assert_eq!(closed.seconds, Some(1.5));
    task.abort();
}

/// `Endpoint::with_read_timeout` takes any bound, so a caller that wants none sets `Duration::MAX`.
/// A close deadline too large to represent never passes: `close` waits for `session.closed`
/// instead of panicking.
#[tokio::test]
async fn live_close_with_a_read_timeout_of_duration_max_reads_session_closed() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_start(&mut socket).await;
        let close = receive(&mut socket).await;
        assert_eq!(close["type"], "session.close");
        send(
            &mut socket,
            json!({"type": "session.closed", "event_id": "e_closed", "reason": "close_requested", "session": {"id": "live_123", "model": "gpt-live-1", "status": "active"}, "usage": {"seconds": 1.5}}),
        )
        .await;
        let _ = socket.close(None).await;
    })
    .await;
    let client = connect(&endpoint.with_read_timeout(Duration::MAX)).await;
    let closed = tokio::time::timeout(Duration::from_secs(10), client.close())
        .await
        .expect("the server answered the close at once")
        .expect("the close reads session.closed");
    assert_eq!(closed.reason.as_deref(), Some("close_requested"));
    task.abort();
}
