//! Adversary cases for `LiveClient::into_split` and `LiveClient::reunite`, offline against a
//! local WebSocket server in the test. No network and no key.
//!
//! They follow the stalled-write scenario past the unit's own tests: a write that recovers once
//! the peer drains, with the graceful close run from the reunited halves, and the graceful
//! close's documented bound when the peer still does not drain.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::{SinkExt, StreamExt};
use inference_openai::live::{
    CLOSE_TIMEOUT, LiveClient, LiveClosed, LiveEvent, LiveSender, LiveSessionConfig,
};
use inference_openai::{
    Credential, CredentialError, CredentialFuture, CredentialSource, Endpoint, OPENAI_KEY_NAME,
    RealtimeError,
};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

const CREDENTIAL: &str = "offline-test-credential";
const LIVE_MODEL: &str = "gpt-live-1";

const BOUND: Duration = Duration::from_secs(10);

const CHUNK: usize = 256 * 1024;

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

async fn serve<T, F, Fut>(read_timeout: Duration, script: F) -> (Endpoint, JoinHandle<T>)
where
    T: Send + 'static,
    F: FnOnce(Socket) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = T> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
            .await
            .expect("the client did not connect within five seconds")
            .unwrap();
        let socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        script(socket).await
    });
    let endpoint = Endpoint::live(LIVE_MODEL)
        .with_url(format!("ws://{address}/v1/live/sessions"))
        .with_read_timeout(read_timeout);
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

async fn receive(socket: &mut Socket) -> Value {
    match frame(socket).await {
        Message::Text(text) => serde_json::from_str(text.as_str()).unwrap(),
        other => panic!("expected a text frame, got {other:?}"),
    }
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
    tokio::time::timeout(
        BOUND,
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

async fn finish<T>(task: JoinHandle<T>) -> T {
    tokio::time::timeout(BOUND, task)
        .await
        .expect("the server script did not finish")
        .expect("the server script failed")
}

fn transcript(n: u64) -> Value {
    json!({"type": "session.output_transcript.delta", "event_id": format!("e{n}"), "delta": format!("part {n}"), "start_ms": n * 100, "end_ms": n * 100 + 50})
}

fn transcript_event(n: u64) -> LiveEvent {
    LiveEvent::OutputTranscript {
        delta: format!("part {n}"),
        start_ms: n * 100,
        end_ms: n * 100 + 50,
    }
}

/// The peer stops draining, the receiver reads events meanwhile, then the peer drains again:
/// the pending write completes, every append arrives whole, and the graceful close from the
/// reunited halves sends `session.close` and reads `session.closed`.
#[tokio::test]
async fn adversary16_live_stalled_write_recovers_and_the_reunited_halves_close_gracefully() {
    let (go_tx, go_rx) = oneshot::channel::<()>();
    let (resume_tx, resume_rx) = oneshot::channel::<()>();
    let expected_audio = STANDARD.encode(vec![0x55_u8; CHUNK]);
    let (endpoint, task) = serve(Duration::from_secs(5), move |mut socket| async move {
        accept_start(&mut socket).await;
        go_rx.await.unwrap();
        for n in 1..=3 {
            send(&mut socket, transcript(n)).await;
        }
        resume_rx.await.unwrap();
        let mut appends = 0_usize;
        let close = loop {
            let event = receive(&mut socket).await;
            match event["type"].as_str() {
                Some("session.input_audio.append") => {
                    assert_eq!(
                        event["audio"].as_str(),
                        Some(expected_audio.as_str()),
                        "append {appends} arrived altered"
                    );
                    appends += 1;
                }
                Some("session.close") => break event,
                other => panic!("unexpected client event {other:?}"),
            }
        };
        send(
            &mut socket,
            json!({"type": "session.closed", "event_id": "e_closed", "reason": "close_requested", "session": {"id": "live_123", "model": "gpt-live-1", "status": "active"}, "usage": {"seconds": 3.0}}),
        )
        .await;
        let _ = socket.close(None).await;
        (appends, close)
    })
    .await;
    let (mut sender, mut receiver) = connect(&endpoint).await.into_split();

    let (count_tx, mut count_rx) = mpsc::unbounded_channel::<usize>();
    let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
    let writer = tokio::spawn(async move {
        let chunk = vec![0x55_u8; CHUNK];
        let mut written = 0_usize;
        loop {
            sender.append_audio(&chunk).await?;
            written += 1;
            let _ = count_tx.send(written);
            if stop_rx.try_recv().is_ok() {
                break;
            }
        }
        Ok::<(LiveSender, usize), RealtimeError>((sender, written))
    });

    tokio::time::timeout(BOUND, async {
        loop {
            match tokio::time::timeout(Duration::from_millis(500), count_rx.recv()).await {
                Ok(Some(_)) => {}
                Ok(None) => panic!("the writer ended"),
                Err(_) => return,
            }
        }
    })
    .await
    .expect("no write stayed pending within the bound");
    assert!(!writer.is_finished(), "the writer finished");

    go_tx.send(()).unwrap();
    for n in 1..=3 {
        let event = tokio::time::timeout(BOUND, receiver.next_event())
            .await
            .unwrap_or_else(|_| panic!("event {n} waited behind the stalled write"))
            .expect("the receiver reads the event");
        assert_eq!(event, transcript_event(n));
    }
    stop_tx.send(()).unwrap();
    resume_tx.send(()).unwrap();

    let (sender, written) = tokio::time::timeout(BOUND, writer)
        .await
        .expect("the stalled write did not complete once the peer drained")
        .unwrap()
        .expect("every write succeeds once the peer drains");
    let client = LiveClient::reunite(sender, receiver).expect("one session's halves");
    let closed = tokio::time::timeout(BOUND, client.close())
        .await
        .expect("close outlived its bound")
        .expect("session.closed is read");
    assert_eq!(
        closed,
        LiveClosed {
            reason: Some("close_requested".to_owned()),
            seconds: Some(3.0),
        }
    );
    let (appends, close) = finish(task).await;
    assert_eq!(
        appends, written,
        "appends the server read against appends that returned"
    );
    assert_eq!(
        close,
        json!({"type": "session.close", "event_id": "session_close"})
    );
}

/// The adopter's stopgap bounds each write; one outlives its bound because the peer stopped
/// draining, and the caller closes from the reunited halves. `LiveClient::close` documents that
/// "the whole close ... takes at most the longer of the endpoint's read timeout and
/// CLOSE_TIMEOUT" and fails with `Stalled` when that bound passes, so it returns within
/// `CLOSE_TIMEOUT` (the read timeout here is 1 s) plus a margin.
#[tokio::test]
async fn adversary16_live_close_after_a_stalled_write_keeps_its_documented_bound() {
    let (done_tx, done_rx) = oneshot::channel::<()>();
    let (endpoint, task) = serve(Duration::from_secs(1), move |mut socket| async move {
        accept_start(&mut socket).await;
        let _ = done_rx.await;
    })
    .await;
    let (mut sender, receiver) = connect(&endpoint).await.into_split();
    let chunk = vec![0x55_u8; CHUNK];
    let mut written = 0_usize;
    tokio::time::timeout(BOUND, async {
        loop {
            match tokio::time::timeout(Duration::from_millis(500), sender.append_audio(&chunk))
                .await
            {
                Ok(result) => {
                    result.expect("an append the peer drains is written");
                    written += 1;
                }
                Err(_) => return,
            }
        }
    })
    .await
    .expect("no write stayed pending within the bound");
    let client = LiveClient::reunite(sender, receiver).expect("one session's halves");
    let limit = CLOSE_TIMEOUT + Duration::from_secs(5);
    let closed = tokio::time::timeout(limit, client.close())
        .await
        .unwrap_or_else(|_| {
            panic!(
                "close outlived its documented bound: still pending after {limit:?}, CLOSE_TIMEOUT {CLOSE_TIMEOUT:?}, after {written} appends"
            )
        });
    assert!(
        matches!(closed, Err(RealtimeError::Stalled(d)) if d == CLOSE_TIMEOUT),
        "{closed:?}"
    );
    let _ = done_tx.send(());
    finish(task).await;
}
