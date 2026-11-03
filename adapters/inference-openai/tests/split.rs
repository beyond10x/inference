//! Offline tests of a Realtime session split into a sending and a receiving half
//! (`RealtimeClient::into_split`), against a local WebSocket server in the test. No network and
//! no key: the credential is a fixed test value from an in-test resolver.
//!
//! Every client event the scripts expect is written from the published Realtime API reference,
//! client events page (`https://developers.openai.com/api/reference/resources/realtime/client-events`);
//! the server events from its server events page
//! (`https://developers.openai.com/api/reference/resources/realtime/server-events`).

use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_openai::{
    ConversationItem, Credential, CredentialError, CredentialFuture, CredentialSource, Endpoint,
    ItemOptions, OPENAI_KEY_NAME, RealtimeClient, RealtimeError, RealtimeSender, ResponseOptions,
    ServerEvent, SessionConfig, WireFormat,
};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

const CREDENTIAL: &str = "offline-test-credential";
const MODEL: &str = "gpt-realtime-2.1";

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

/// Binds a local server, accepts one connection and runs `script` on it. Returns the endpoint the
/// client connects to and the server task, whose panics carry the script's assertion failures and
/// whose output is the script's.
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
    let endpoint = Endpoint::new(MODEL)
        .with_url(format!("ws://{address}/v1/realtime"))
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

/// The server half of session setup: `session.created`, the client's `session.update`, then
/// `kept` (events sent before `session.updated`, which the client keeps for later) and
/// `session.updated`.
async fn accept_session(socket: &mut Socket, kept: &[Value]) {
    send(
        socket,
        json!({"type": "session.created", "event_id": "event_1", "session": {"type": "realtime", "id": "sess_1"}}),
    )
    .await;
    let update = receive(socket).await;
    assert_eq!(
        update["type"], "session.update",
        "first client event: {update}"
    );
    for event in kept {
        send(socket, event.clone()).await;
    }
    send(
        socket,
        json!({"type": "session.updated", "event_id": "event_2", "session": update["session"]}),
    )
    .await;
}

async fn connect(endpoint: &Endpoint) -> RealtimeClient {
    connect_with(endpoint, &SessionConfig::default()).await
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

/// The server stops reading its socket after setup; a spawned task writes 256 KiB chunks with
/// the sender until one write stays pending, its task not finished. Then the server sends a ping,
/// whose pong the client cannot flush, and three events, and the receiver returns the events, in
/// order, while that write is still pending.
#[tokio::test]
async fn realtime_split_reads_events_while_a_write_stalls() {
    let (go_tx, go_rx) = oneshot::channel::<()>();
    let (done_tx, done_rx) = oneshot::channel::<()>();
    let (endpoint, task) = serve(move |mut socket| async move {
        accept_session(&mut socket, &[]).await;
        go_rx.await.expect("the test signals once the write stalls");
        socket
            .send(Message::Ping(b"stalled".to_vec().into()))
            .await
            .unwrap();
        for n in 1..=3 {
            send(&mut socket, speech_started(n)).await;
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
            sender.append_encoded(&chunk).await?;
            written += 1;
            if written_tx.send(written).is_err() {
                return Ok::<RealtimeSender, RealtimeError>(sender);
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
        assert_eq!(event, speech_started_event(n));
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

fn pcm16_frame(samples: &[i16]) -> AudioFrame {
    AudioFrame {
        format: AudioFormat {
            encoding: AudioEncoding::PcmS16le,
            sample_rate: NonZeroU32::new(24_000).unwrap(),
            channels: NonZeroU16::new(1).unwrap(),
        },
        position: SampleClockPosition::new(0).unwrap(),
        sample_count: samples.len() as u32,
        data: samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect(),
    }
}

/// How many frames the send sequence writes: sixteen calls, the refused append writing none.
const SENT: usize = 15;

/// The frames the client wrote after setup, `SENT` of them, exactly as written.
async fn record_sends(mut socket: Socket, kept: Vec<Value>, later: Vec<Value>) -> Vec<Message> {
    accept_session(&mut socket, &kept).await;
    for event in later {
        send(&mut socket, event).await;
    }
    let mut frames = Vec::new();
    for _ in 0..SENT {
        frames.push(frame(&mut socket).await);
    }
    frames
}

fn g711_session() -> SessionConfig {
    SessionConfig {
        input_format: WireFormat::Pcmu,
        ..SessionConfig::default()
    }
}

fn item_options() -> ItemOptions {
    ItemOptions {
        event_id: Some("evt_item".to_owned()),
        previous_item_id: Some("item_prev".to_owned()),
    }
}

fn response_options() -> ResponseOptions {
    ResponseOptions {
        event_id: Some("evt_resp".to_owned()),
        instructions: Some("Answer briefly.".to_owned()),
        ..ResponseOptions::default()
    }
}

/// The events kept during setup and the events sent after it: a named `function_call` output
/// item kept, then its call without a name, which the receiver names from the kept item.
fn read_script() -> (Vec<Value>, Vec<Value>) {
    let kept = vec![
        speech_started(1),
        json!({"type": "response.output_item.added", "event_id": "event_item", "response_id": "resp_1", "output_index": 0, "item": {"id": "item_fc", "type": "function_call", "status": "in_progress", "name": "weather", "call_id": "call_abc", "arguments": ""}}),
    ];
    let later = vec![
        json!({"type": "response.function_call_arguments.done", "event_id": "event_fc", "response_id": "resp_1", "item_id": "item_fc", "output_index": 0, "call_id": "call_abc", "arguments": "{}"}),
        speech_started(2),
    ];
    (kept, later)
}

/// Each send method of the sender writes the same client event as the whole client's method,
/// the input format `update_session` sets included, and the receiver returns the events kept
/// during setup first, names a call from the item read before it and holds the session, as the
/// whole client does.
#[tokio::test]
async fn realtime_split_halves_send_and_read_as_the_client_did() {
    let frame = pcm16_frame(&[1, -2, 300, -400]);

    let (kept, later) = read_script();
    let (endpoint, whole_task) =
        serve(move |socket| async move { record_sends(socket, kept, later).await }).await;
    let mut client = connect(&endpoint).await;
    let mut whole_events = Vec::new();
    for _ in 0..4 {
        whole_events.push(
            tokio::time::timeout(BOUND, client.next_event())
                .await
                .expect("an event within the bound")
                .unwrap(),
        );
    }
    let whole_session = client.session().clone();
    let whole_unparsed = client.unparsed_type().map(str::to_owned);
    client
        .update_session(&SessionConfig::default())
        .await
        .unwrap();
    client.append_audio(&frame).await.unwrap();
    client.append_encoded(&[1, 2, 3]).await.unwrap();
    client.commit_audio().await.unwrap();
    client.ping(b"keepalive").await.unwrap();
    client
        .create_item(
            &ConversationItem::AssistantText("Hi.".to_owned()),
            &item_options(),
        )
        .await
        .unwrap();
    client.send_text("hello").await.unwrap();
    client.send_function_output("call_abc", "{}").await.unwrap();
    client.create_response().await.unwrap();
    client
        .create_response_with(&response_options())
        .await
        .unwrap();
    client.cancel_response(Some("resp_1")).await.unwrap();
    client.cancel_response(None).await.unwrap();
    client.truncate("item_1", 0, 1500).await.unwrap();
    client.update_session(&g711_session()).await.unwrap();
    let whole_refused = client.append_audio(&frame).await;
    client.clear_audio().await.unwrap();
    let whole_frames = finish(whole_task).await;

    let (kept, later) = read_script();
    let (endpoint, split_task) =
        serve(move |socket| async move { record_sends(socket, kept, later).await }).await;
    let (mut sender, mut receiver) = connect(&endpoint).await.into_split();
    let mut split_events = Vec::new();
    for _ in 0..4 {
        split_events.push(
            tokio::time::timeout(BOUND, receiver.next_event())
                .await
                .expect("an event within the bound")
                .unwrap(),
        );
    }
    let split_session = receiver.session().clone();
    let split_unparsed = receiver.unparsed_type().map(str::to_owned);
    sender
        .update_session(&SessionConfig::default())
        .await
        .unwrap();
    sender.append_audio(&frame).await.unwrap();
    sender.append_encoded(&[1, 2, 3]).await.unwrap();
    sender.commit_audio().await.unwrap();
    sender.ping(b"keepalive").await.unwrap();
    sender
        .create_item(
            &ConversationItem::AssistantText("Hi.".to_owned()),
            &item_options(),
        )
        .await
        .unwrap();
    sender.send_text("hello").await.unwrap();
    sender.send_function_output("call_abc", "{}").await.unwrap();
    sender.create_response().await.unwrap();
    sender
        .create_response_with(&response_options())
        .await
        .unwrap();
    sender.cancel_response(Some("resp_1")).await.unwrap();
    sender.cancel_response(None).await.unwrap();
    sender.truncate("item_1", 0, 1500).await.unwrap();
    sender.update_session(&g711_session()).await.unwrap();
    let split_refused = sender.append_audio(&frame).await;
    sender.clear_audio().await.unwrap();
    let split_frames = finish(split_task).await;

    assert_eq!(split_frames, whole_frames);
    assert_eq!(split_frames.len(), SENT);
    assert_eq!(
        split_frames[4],
        Message::Ping(b"keepalive".to_vec().into()),
        "{split_frames:?}"
    );
    assert!(
        matches!(&whole_refused, Err(RealtimeError::Audio(_))),
        "{whole_refused:?}"
    );
    assert!(
        matches!(&split_refused, Err(RealtimeError::Audio(_))),
        "{split_refused:?}"
    );

    assert_eq!(split_events, whole_events);
    assert_eq!(split_events[0], speech_started_event(1));
    assert!(
        matches!(&split_events[1], ServerEvent::Unknown { event_type, .. } if event_type == "response.output_item.added"),
        "{:?}",
        split_events[1]
    );
    assert_eq!(
        split_events[2],
        ServerEvent::FunctionCall {
            response_id: "resp_1".to_owned(),
            item_id: "item_fc".to_owned(),
            call_id: "call_abc".to_owned(),
            name: Some("weather".to_owned()),
            arguments: "{}".to_owned(),
        }
    );
    assert_eq!(split_events[3], speech_started_event(2));
    assert_eq!(split_session, whole_session);
    assert_eq!(split_session["type"], "realtime", "{split_session}");
    assert_eq!(split_unparsed, whole_unparsed);
}

/// A receiver that cannot read a message names its `type`, as the whole client does.
#[tokio::test]
async fn realtime_split_receiver_names_the_type_it_could_not_read() {
    let (endpoint, task) = serve(|mut socket| async move {
        accept_session(&mut socket, &[]).await;
        send(
            &mut socket,
            json!({"type": "input_audio_buffer.speech_started"}),
        )
        .await;
        let _ = frame(&mut socket).await;
    })
    .await;
    let (mut sender, mut receiver) = connect(&endpoint).await.into_split();
    let read = tokio::time::timeout(BOUND, receiver.next_event())
        .await
        .expect("a read within the bound");
    assert!(matches!(read, Err(RealtimeError::Protocol(_))), "{read:?}");
    assert_eq!(
        receiver.unparsed_type(),
        Some("input_audio_buffer.speech_started")
    );
    sender.commit_audio().await.unwrap();
    finish(task).await;
}

/// The server side of a close: reads frames until the client's close frame, which the server
/// answers, then reads until the connection ends. Returns whether a close frame came.
async fn await_close(mut socket: Socket) -> bool {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), socket.next()).await {
            Err(_) => return false,
            Ok(None | Some(Err(_))) => return false,
            Ok(Some(Ok(Message::Close(_)))) => break,
            Ok(Some(Ok(_))) => {}
        }
    }
    while let Ok(Some(Ok(_))) = tokio::time::timeout(Duration::from_secs(5), socket.next()).await {}
    true
}

/// Halves put back together close the session as `RealtimeClient::close` does: a close frame,
/// answered. Halves of two sessions are refused and handed back whole, each still working.
#[tokio::test]
async fn realtime_split_halves_close_the_session() {
    let (endpoint_a, task_a) = serve(|mut socket| async move {
        accept_session(&mut socket, &[]).await;
        let commit = receive(&mut socket).await;
        assert_eq!(commit, json!({"type": "input_audio_buffer.commit"}));
        await_close(socket).await
    })
    .await;
    let (endpoint_b, task_b) = serve(|mut socket| async move {
        accept_session(&mut socket, &[speech_started(7)]).await;
        await_close(socket).await
    })
    .await;
    let (sender_a, receiver_a) = connect(&endpoint_a).await.into_split();
    let (sender_b, receiver_b) = connect(&endpoint_b).await.into_split();

    let mismatch = RealtimeClient::reunite(sender_a, receiver_b)
        .expect_err("halves of two sessions are not one client");
    assert!(
        mismatch.to_string().contains("not halves of one"),
        "{mismatch}"
    );
    let (mut sender_a, mut receiver_b) = (mismatch.sender, mismatch.receiver);
    sender_a.commit_audio().await.unwrap();
    assert_eq!(
        tokio::time::timeout(BOUND, receiver_b.next_event())
            .await
            .expect("an event within the bound")
            .unwrap(),
        speech_started_event(7)
    );

    let client_a = RealtimeClient::reunite(sender_a, receiver_a).expect("one session's halves");
    let client_b = RealtimeClient::reunite(sender_b, receiver_b).expect("one session's halves");
    tokio::time::timeout(BOUND, client_a.close())
        .await
        .expect("close outlived its bound")
        .expect("the close handshake completes");
    tokio::time::timeout(BOUND, client_b.close())
        .await
        .expect("close outlived its bound")
        .expect("the close handshake completes");
    assert!(finish(task_a).await, "the server saw no close frame");
    assert!(finish(task_b).await, "the server saw no close frame");
}
