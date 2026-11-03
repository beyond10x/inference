//! Adversary cases for `RealtimeClient::into_split` and `RealtimeClient::reunite`, offline
//! against a local WebSocket server in the test. No network and no key.
//!
//! The scenario the split exists for is a peer that stops draining the socket. These cases
//! follow that scenario past the point the unit's own tests stop: what the receiver does when
//! nothing arrives, whether the write recovers intact once the peer drains again, what state
//! survives a split and a reunite, and whether `close` keeps its documented bound when the
//! peer still does not drain.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::{SinkExt, StreamExt};
use inference_openai::{
    Credential, CredentialError, CredentialFuture, CredentialSource, Endpoint, OPENAI_KEY_NAME,
    RealtimeClient, RealtimeError, RealtimeSender, ServerEvent, SessionConfig, WireFormat,
};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

const CREDENTIAL: &str = "offline-test-credential";
const MODEL: &str = "gpt-realtime-2.1";

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
    let endpoint = Endpoint::new(MODEL)
        .with_url(format!("ws://{address}/v1/realtime"))
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

async fn accept_session(socket: &mut Socket, kept: &[Value]) {
    send(
        socket,
        json!({"type": "session.created", "event_id": "event_1", "session": {"type": "realtime", "id": "sess_1"}}),
    )
    .await;
    let update = receive(socket).await;
    assert_eq!(update["type"], "session.update", "{update}");
    for event in kept {
        send(socket, event.clone()).await;
    }
    send(
        socket,
        json!({"type": "session.updated", "event_id": "event_2", "session": update["session"]}),
    )
    .await;
}

async fn connect_with(endpoint: &Endpoint, session: &SessionConfig) -> RealtimeClient {
    tokio::time::timeout(
        BOUND,
        RealtimeClient::connect(endpoint, &FixedSource, OPENAI_KEY_NAME, session),
    )
    .await
    .expect("setup outlived its bound")
    .expect("session setup")
}

async fn connect(endpoint: &Endpoint) -> RealtimeClient {
    connect_with(endpoint, &SessionConfig::default()).await
}

async fn finish<T>(task: JoinHandle<T>) -> T {
    tokio::time::timeout(BOUND, task)
        .await
        .expect("the server script did not finish")
        .expect("the server script failed")
}

fn speech_started(n: u64) -> Value {
    json!({"type": "input_audio_buffer.speech_started", "event_id": format!("event_s{n}"), "audio_start_ms": n * 100, "item_id": format!("item_{n}")})
}

fn speech_started_event(n: u64) -> ServerEvent {
    ServerEvent::SpeechStarted {
        item_id: format!("item_{n}"),
        audio_start_ms: n * 100,
    }
}

/// Writes `CHUNK`-byte appends, each bounded by `per_write` as the adopter's stopgap bounds it,
/// until one write outlives the bound: the peer has stopped draining. Returns the sender and
/// the number of appends that returned.
async fn write_until_a_write_times_out(
    mut sender: RealtimeSender,
    per_write: Duration,
) -> (RealtimeSender, usize) {
    let chunk = vec![0x55_u8; CHUNK];
    let mut written = 0;
    tokio::time::timeout(BOUND, async {
        loop {
            match tokio::time::timeout(per_write, sender.append_encoded(&chunk)).await {
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
    (sender, written)
}

/// Nothing arrives while a write is pending: the receiver answers `Stalled` after its read
/// timeout, as the whole client's `next_event` does, instead of waiting on the write.
#[tokio::test]
async fn adversary16_receiver_reports_stalled_after_its_read_timeout_while_a_write_is_pending() {
    let (done_tx, done_rx) = oneshot::channel::<()>();
    let (endpoint, task) = serve(Duration::from_secs(1), move |mut socket| async move {
        accept_session(&mut socket, &[]).await;
        let _ = done_rx.await;
    })
    .await;
    let (mut sender, mut receiver) = connect(&endpoint).await.into_split();
    let (stalled_tx, stalled_rx) = oneshot::channel::<()>();
    let writer = tokio::spawn(async move {
        let chunk = vec![0x55_u8; CHUNK];
        let mut stalled_tx = Some(stalled_tx);
        loop {
            match tokio::time::timeout(Duration::from_millis(500), sender.append_encoded(&chunk))
                .await
            {
                Ok(result) => result.expect("a drained append"),
                Err(_) => {
                    if let Some(tx) = stalled_tx.take() {
                        let _ = tx.send(());
                    }
                    sender.append_encoded(&chunk).await.expect("never ends");
                }
            }
        }
    });
    tokio::time::timeout(BOUND, stalled_rx)
        .await
        .expect("no write stalled within the bound")
        .unwrap();
    let started = tokio::time::Instant::now();
    let read = tokio::time::timeout(BOUND, receiver.next_event())
        .await
        .expect("the read waited behind the pending write past the bound");
    assert!(
        matches!(read, Err(RealtimeError::Stalled(d)) if d == Duration::from_secs(1)),
        "{read:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "Stalled came after {:?}",
        started.elapsed()
    );
    assert!(!writer.is_finished(), "the write was not pending");
    writer.abort();
    let _ = writer.await;
    let _ = done_tx.send(());
    finish(task).await;
}

/// The peer stops draining, the receiver reads events and a ping meanwhile, then the peer drains
/// again: the pending write completes, every append arrives whole and in order with the pong
/// among them, the writer goes on to `commit`, and the reunited halves close the session.
#[tokio::test]
async fn adversary16_a_stalled_write_recovers_intact_once_the_peer_drains() {
    let (go_tx, go_rx) = oneshot::channel::<()>();
    let (resume_tx, resume_rx) = oneshot::channel::<()>();
    let expected_audio = STANDARD.encode(vec![0x55_u8; CHUNK]);
    let (endpoint, task) = serve(Duration::from_secs(5), move |mut socket| async move {
        accept_session(&mut socket, &[]).await;
        go_rx.await.unwrap();
        socket
            .send(Message::Ping(b"stalled".to_vec().into()))
            .await
            .unwrap();
        for n in 1..=3 {
            send(&mut socket, speech_started(n)).await;
        }
        resume_rx.await.unwrap();
        let mut appends = 0_usize;
        let mut pong = false;
        loop {
            match frame(&mut socket).await {
                Message::Text(text) => {
                    let event: Value = serde_json::from_str(text.as_str()).unwrap();
                    match event["type"].as_str() {
                        Some("input_audio_buffer.append") => {
                            assert_eq!(
                                event["audio"].as_str(),
                                Some(expected_audio.as_str()),
                                "append {appends} arrived altered"
                            );
                            appends += 1;
                        }
                        Some("input_audio_buffer.commit") => break,
                        other => panic!("unexpected client event {other:?}"),
                    }
                }
                Message::Pong(payload) => {
                    assert_eq!(payload.as_ref(), b"stalled");
                    pong = true;
                }
                other => panic!("unexpected frame {other:?}"),
            }
        }
        loop {
            match tokio::time::timeout(Duration::from_secs(5), socket.next()).await {
                Ok(Some(Ok(Message::Close(_)))) => break,
                Ok(Some(Ok(_))) => {}
                other => panic!("no close frame: {other:?}"),
            }
        }
        while let Ok(Some(Ok(_))) =
            tokio::time::timeout(Duration::from_secs(5), socket.next()).await
        {}
        (appends, pong)
    })
    .await;
    let (mut sender, mut receiver) = connect(&endpoint).await.into_split();

    let (count_tx, mut count_rx) = mpsc::unbounded_channel::<usize>();
    let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
    let writer = tokio::spawn(async move {
        let chunk = vec![0x55_u8; CHUNK];
        let mut written = 0_usize;
        loop {
            sender.append_encoded(&chunk).await?;
            written += 1;
            let _ = count_tx.send(written);
            if stop_rx.try_recv().is_ok() {
                break;
            }
        }
        sender.commit_audio().await?;
        Ok::<(RealtimeSender, usize), RealtimeError>((sender, written))
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
        assert_eq!(event, speech_started_event(n));
    }
    stop_tx.send(()).unwrap();
    resume_tx.send(()).unwrap();

    let (sender, written) = tokio::time::timeout(BOUND, writer)
        .await
        .expect("the stalled write did not complete once the peer drained")
        .unwrap()
        .expect("every write succeeds once the peer drains");
    let client = RealtimeClient::reunite(sender, receiver).expect("one session's halves");
    tokio::time::timeout(BOUND, client.close())
        .await
        .expect("close outlived its bound")
        .expect("the close handshake completes");
    let (appends, pong) = finish(task).await;
    assert_eq!(
        appends, written,
        "appends the server read against appends that returned"
    );
    assert!(pong, "the pong to the server's ping never arrived");
}

/// The adopter's stopgap bounds each write; one outlives its bound because the peer stopped
/// draining, and the caller ends the session from the halves. `RealtimeClient::close` documents
/// `Stalled` "when the server does not close within the read timeout", so it returns within
/// that timeout (1 s here) with `Stalled`, not never.
#[tokio::test]
async fn adversary16_close_after_a_stalled_write_keeps_its_read_timeout_bound() {
    let (done_tx, done_rx) = oneshot::channel::<()>();
    let (endpoint, task) = serve(Duration::from_secs(1), move |mut socket| async move {
        accept_session(&mut socket, &[]).await;
        let _ = done_rx.await;
    })
    .await;
    let (sender, receiver) = connect(&endpoint).await.into_split();
    let (sender, written) = write_until_a_write_times_out(sender, Duration::from_millis(500)).await;
    let client = RealtimeClient::reunite(sender, receiver).expect("one session's halves");
    let closed = tokio::time::timeout(BOUND, client.close())
        .await
        .unwrap_or_else(|_| {
            panic!(
                "close outlived its bound: still pending after {BOUND:?} with a read timeout of 1s, after {written} appends"
            )
        });
    assert!(
        matches!(closed, Err(RealtimeError::Stalled(_))),
        "{closed:?}"
    );
    let _ = done_tx.send(());
    finish(task).await;
}

/// The receiver keeps the events left from setup across a reunite: one read on the receiver,
/// the rest on the reunited client, then a fresh event read from the socket.
#[tokio::test]
async fn adversary16_reunited_client_returns_the_events_the_receiver_still_kept() {
    let (endpoint, task) = serve(Duration::from_secs(5), |mut socket| async move {
        accept_session(&mut socket, &[speech_started(1), speech_started(2)]).await;
        let commit = receive(&mut socket).await;
        assert_eq!(commit, json!({"type": "input_audio_buffer.commit"}));
        send(&mut socket, speech_started(3)).await;
        let _ = receive(&mut socket).await;
    })
    .await;
    let (mut sender, mut receiver) = connect(&endpoint).await.into_split();
    assert_eq!(
        tokio::time::timeout(BOUND, receiver.next_event())
            .await
            .unwrap()
            .unwrap(),
        speech_started_event(1)
    );
    sender.commit_audio().await.unwrap();
    let mut client = RealtimeClient::reunite(sender, receiver).expect("one session's halves");
    assert_eq!(
        tokio::time::timeout(BOUND, client.next_event())
            .await
            .unwrap()
            .unwrap(),
        speech_started_event(2)
    );
    assert_eq!(
        tokio::time::timeout(BOUND, client.next_event())
            .await
            .unwrap()
            .unwrap(),
        speech_started_event(3)
    );
    client.clear_audio().await.unwrap();
    finish(task).await;
}

/// The input format `connect` set travels with the sender: a G.711 session's sender refuses
/// PCM frames as the whole client does and passes encoded bytes through.
#[tokio::test]
async fn adversary16_sender_keeps_the_input_format_connect_set() {
    let (endpoint, task) = serve(Duration::from_secs(5), |mut socket| async move {
        accept_session(&mut socket, &[]).await;
        receive(&mut socket).await
    })
    .await;
    let session = SessionConfig {
        input_format: WireFormat::Pcmu,
        ..SessionConfig::default()
    };
    let (mut sender, _receiver) = connect_with(&endpoint, &session).await.into_split();
    let frame = inference_audio::AudioFrame {
        format: inference_audio::AudioFormat {
            encoding: inference_audio::AudioEncoding::PcmS16le,
            sample_rate: std::num::NonZeroU32::new(24_000).unwrap(),
            channels: std::num::NonZeroU16::new(1).unwrap(),
        },
        position: inference_audio::SampleClockPosition::new(0).unwrap(),
        sample_count: 2,
        data: vec![1, 0, 2, 0],
    };
    let refused = sender.append_audio(&frame).await;
    assert!(
        matches!(refused, Err(RealtimeError::Audio(_))),
        "{refused:?}"
    );
    sender.append_encoded(&[0xFF, 0x7F]).await.unwrap();
    assert_eq!(
        finish(task).await,
        json!({"type": "input_audio_buffer.append", "audio": STANDARD.encode([0xFF_u8, 0x7F])})
    );
}

/// Two tasks own the halves: the receiver moves into its own task and reads there while the
/// sender writes from this one, and the sender keeps writing once the receiver is dropped.
#[tokio::test]
async fn adversary16_receiver_reads_in_its_own_task_and_the_sender_outlives_it() {
    let (endpoint, task) = serve(Duration::from_secs(5), |mut socket| async move {
        accept_session(&mut socket, &[]).await;
        let first = receive(&mut socket).await;
        send(&mut socket, speech_started(4)).await;
        let second = receive(&mut socket).await;
        (first, second)
    })
    .await;
    let (mut sender, mut receiver) = connect(&endpoint).await.into_split();
    let reader = tokio::spawn(async move { receiver.next_event().await });
    sender.commit_audio().await.unwrap();
    let event = tokio::time::timeout(BOUND, reader)
        .await
        .expect("the reader task did not finish")
        .unwrap()
        .unwrap();
    assert_eq!(event, speech_started_event(4));
    sender.clear_audio().await.unwrap();
    let (first, second) = finish(task).await;
    assert_eq!(first, json!({"type": "input_audio_buffer.commit"}));
    assert_eq!(second, json!({"type": "input_audio_buffer.clear"}));
}
