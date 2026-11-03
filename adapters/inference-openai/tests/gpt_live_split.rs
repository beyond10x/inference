//! Offline tests of a GPT-Live session split into a sending and a receiving half
//! (`LiveClient::into_split`), against a local WebSocket server in the test. No network and no
//! key: the credential is a fixed test value from an in-test resolver.
//!
//! Every client and server event is written from the primary WebSocket reference,
//! `https://developers.openai.com/api/reference/resources/live/primary-websocket`.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use inference_openai::live::{LiveClient, LiveClosed, LiveEvent, LiveSender, LiveSessionConfig};
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

/// The bound on every wait of a test: a failure is a failed assertion, never a hang.
const BOUND: Duration = Duration::from_secs(10);

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

/// A local server running `script`; the endpoint points at it, and the task's output is the
/// script's.
async fn serve<T, F, Fut>(script: F) -> (Endpoint, JoinHandle<T>)
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

async fn receive(socket: &mut Socket) -> Value {
    match frame(socket).await {
        Message::Text(text) => serde_json::from_str(text.as_str()).unwrap(),
        other => panic!("expected a text frame, got {other:?}"),
    }
}

/// Startup: reads `session.start`, sends `kept` (events before the start, which the client keeps
/// for later) and answers `session.started` with the documented excerpt.
async fn accept_start(socket: &mut Socket, kept: &[Value]) {
    let start = receive(socket).await;
    assert_eq!(start["type"], "session.start", "{start}");
    for event in kept {
        send(socket, event.clone()).await;
    }
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

/// The server stops reading its socket after the start; a spawned task writes 256 KiB chunks
/// with the sender until one write stays pending, its task not finished. Then the server sends a
/// ping, whose pong the client cannot flush, and three events, and the receiver returns the
/// events, in order, while that write is still pending.
#[tokio::test]
async fn live_split_reads_events_while_a_write_stalls() {
    let (go_tx, go_rx) = oneshot::channel::<()>();
    let (done_tx, done_rx) = oneshot::channel::<()>();
    let (endpoint, task) = serve(move |mut socket| async move {
        accept_start(&mut socket, &[]).await;
        go_rx.await.expect("the test signals once the write stalls");
        socket
            .send(Message::Ping(b"stalled".to_vec().into()))
            .await
            .unwrap();
        for n in 1..=3 {
            send(&mut socket, transcript(n)).await;
        }
        let _ = done_rx.await;
    })
    .await;
    let (mut sender, mut receiver) = connect(&endpoint).await.into_split();

    let (written_tx, mut written_rx) = mpsc::unbounded_channel::<usize>();
    let writer = tokio::spawn(async move {
        let chunk = vec![0x55_u8; 256 * 1024];
        let mut written = 0;
        loop {
            sender.append_audio(&chunk).await?;
            written += 1;
            if written_tx.send(written).is_err() {
                return Ok::<LiveSender, RealtimeError>(sender);
            }
        }
    });

    let stalled_after = tokio::time::timeout(BOUND, async {
        let mut written = 0;
        loop {
            match tokio::time::timeout(Duration::from_millis(500), written_rx.recv()).await {
                Ok(Some(count)) => written = count,
                Ok(None) => panic!("the writer ended after {written} writes"),
                Err(_) => return written,
            }
        }
    })
    .await
    .expect("no write stayed pending within the bound");
    assert!(
        !writer.is_finished(),
        "the writer finished after {stalled_after} writes"
    );

    go_tx.send(()).unwrap();
    for n in 1..=3 {
        let event = tokio::time::timeout(BOUND, receiver.next_event())
            .await
            .unwrap_or_else(|_| panic!("event {n} waited behind the stalled write"))
            .expect("the receiver reads the event");
        assert_eq!(event, transcript_event(n));
    }
    assert!(
        !writer.is_finished(),
        "the write was no longer pending when the events were read"
    );

    writer.abort();
    let _ = writer.await;
    let _ = done_tx.send(());
    finish(task).await;
}

/// How many frames the send sequence writes.
const SENT: usize = 10;

/// The frames the client wrote after the start, `SENT` of them, exactly as written.
async fn record_sends(mut socket: Socket) -> Vec<Message> {
    accept_start(&mut socket, &[transcript(1)]).await;
    send(&mut socket, transcript(2)).await;
    let mut frames = Vec::new();
    for _ in 0..SENT {
        frames.push(frame(&mut socket).await);
    }
    frames
}

/// Each send method of the GPT-Live sender writes the same client event as the whole client's
/// method, and the receiver returns the events kept during start first and holds the session.
#[tokio::test]
async fn live_split_halves_send_and_read_as_the_client_did() {
    let (endpoint, whole_task) = serve(record_sends).await;
    let mut client = connect(&endpoint).await;
    let mut whole_events = Vec::new();
    for _ in 0..2 {
        whole_events.push(
            tokio::time::timeout(BOUND, client.next_event())
                .await
                .expect("an event within the bound")
                .unwrap(),
        );
    }
    let whole_session = client.session().clone();
    let whole_id = client.session_id().map(str::to_owned);
    client.append_audio(&[1, 2, 3, 4]).await.unwrap();
    client.mute().await.unwrap();
    client.unmute().await.unwrap();
    client.append_instructions(None, "Be brief.").await.unwrap();
    client
        .append_thinking(Some("del_1"), "The user is in Berlin.")
        .await
        .unwrap();
    client
        .append_commentary(Some("del_1"), "It is sunny.")
        .await
        .unwrap();
    client.send_function_output("call_abc", "{}").await.unwrap();
    client.create_response().await.unwrap();
    client.ping(b"keepalive").await.unwrap();
    client.request_close().await.unwrap();
    let whole_frames = finish(whole_task).await;

    let (endpoint, split_task) = serve(record_sends).await;
    let (mut sender, mut receiver) = connect(&endpoint).await.into_split();
    let mut split_events = Vec::new();
    for _ in 0..2 {
        split_events.push(
            tokio::time::timeout(BOUND, receiver.next_event())
                .await
                .expect("an event within the bound")
                .unwrap(),
        );
    }
    let split_session = receiver.session().clone();
    let split_id = receiver.session_id().map(str::to_owned);
    sender.append_audio(&[1, 2, 3, 4]).await.unwrap();
    sender.mute().await.unwrap();
    sender.unmute().await.unwrap();
    sender.append_instructions(None, "Be brief.").await.unwrap();
    sender
        .append_thinking(Some("del_1"), "The user is in Berlin.")
        .await
        .unwrap();
    sender
        .append_commentary(Some("del_1"), "It is sunny.")
        .await
        .unwrap();
    sender.send_function_output("call_abc", "{}").await.unwrap();
    sender.create_response().await.unwrap();
    sender.ping(b"keepalive").await.unwrap();
    sender.request_close().await.unwrap();
    let split_frames = finish(split_task).await;

    assert_eq!(split_frames, whole_frames);
    assert_eq!(split_frames.len(), SENT);
    assert_eq!(split_frames[8], Message::Ping(b"keepalive".to_vec().into()));
    assert_eq!(split_events, whole_events);
    assert_eq!(split_events, vec![transcript_event(1), transcript_event(2)]);
    assert_eq!(split_session, whole_session);
    assert_eq!(split_id.as_deref(), Some("live_123"));
    assert_eq!(split_id, whole_id);
}

/// The server side of a graceful close: reads until `session.close`, answers it with one event
/// and `session.closed`, then closes the connection. Returns the close events the client sent.
async fn answer_close(mut socket: Socket, seconds: f64) -> Value {
    let close = loop {
        let event = receive(&mut socket).await;
        if event["type"] == "session.close" {
            break event;
        }
    };
    send(
        &mut socket,
        json!({"type": "session.usage.updated", "event_id": "e_usage", "usage": {"seconds": seconds}}),
    )
    .await;
    send(
        &mut socket,
        json!({"type": "session.closed", "event_id": "e_closed", "reason": "close_requested", "session": {"id": "live_123", "model": "gpt-live-1", "status": "active"}, "usage": {"seconds": seconds}}),
    )
    .await;
    let _ = socket.close(None).await;
    close
}

/// The GPT-Live half of the scenario: halves put back together close the session gracefully as
/// `LiveClient::close` does, reading `session.closed`. Halves of two sessions are refused and
/// handed back whole, each still working.
#[tokio::test]
async fn realtime_split_halves_close_the_session() {
    let (endpoint_a, task_a) = serve(|mut socket| async move {
        accept_start(&mut socket, &[]).await;
        answer_close(socket, 2.5).await
    })
    .await;
    let (endpoint_b, task_b) = serve(|mut socket| async move {
        accept_start(&mut socket, &[transcript(7)]).await;
        answer_close(socket, 4.0).await
    })
    .await;
    let (sender_a, receiver_a) = connect(&endpoint_a).await.into_split();
    let (sender_b, receiver_b) = connect(&endpoint_b).await.into_split();

    let mismatch = LiveClient::reunite(sender_a, receiver_b)
        .expect_err("halves of two sessions are not one client");
    assert!(
        mismatch.to_string().contains("not halves of one"),
        "{mismatch}"
    );
    let (mut sender_a, mut receiver_b) = (mismatch.sender, mismatch.receiver);
    sender_a.mute().await.unwrap();
    assert_eq!(
        tokio::time::timeout(BOUND, receiver_b.next_event())
            .await
            .expect("an event within the bound")
            .unwrap(),
        transcript_event(7)
    );

    let client_a = LiveClient::reunite(sender_a, receiver_a).expect("one session's halves");
    let client_b = LiveClient::reunite(sender_b, receiver_b).expect("one session's halves");
    assert_eq!(
        tokio::time::timeout(BOUND, client_a.close())
            .await
            .expect("close outlived its bound")
            .expect("session.closed is read"),
        LiveClosed {
            reason: Some("close_requested".to_owned()),
            seconds: Some(2.5),
        }
    );
    assert_eq!(
        tokio::time::timeout(BOUND, client_b.close())
            .await
            .expect("close outlived its bound")
            .expect("session.closed is read"),
        LiveClosed {
            reason: Some("close_requested".to_owned()),
            seconds: Some(4.0),
        }
    );
    let close = json!({"type": "session.close", "event_id": "session_close"});
    assert_eq!(finish(task_a).await, close);
    assert_eq!(finish(task_b).await, close);
}
