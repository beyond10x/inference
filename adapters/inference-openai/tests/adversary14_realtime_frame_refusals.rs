//! Adversary case, pass 14, story `duplex-late-usage-counted` at 8dca44a: a refusal the ledger
//! returns while a Realtime `response.done` is recorded through its `ProviderFrame` is dropped
//! and counted (`duplex.rs`: "Every provider event the ledger refuses is dropped and counted in
//! `SessionLedger::dropped_count`"). Since 8dca44a, `drop_refusal` records only refusals inside a
//! frame, and no case in the tree reads the count, so a `drop_refusal` that counts nothing
//! passes every suite. Here the server ends `resp_1` twice; the second end is refused and must
//! be counted. No network and no key.
#![cfg(feature = "duplex")]

use std::num::{NonZeroU16, NonZeroU32};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use inference_audio::{AudioEncoding, AudioFormat};
use inference_duplex::{
    NativeSpeechModel, SessionConfig, SessionError, SessionEvent, SpeechSessionHandle,
    TurnDetection, TurnTuning,
};
use inference_openai::duplex::OpenAiRealtime;
use inference_openai::{
    Credential, CredentialError, CredentialFuture, CredentialSource, Endpoint, OPENAI_KEY_NAME,
};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

const CREDENTIAL: &str = "offline-test-credential";
const WAIT: Duration = Duration::from_millis(50);

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

struct Server {
    endpoint: Endpoint,
    _runtime: Runtime,
    _task: JoinHandle<()>,
}

impl Server {
    fn start<F, Fut>(script: F) -> Self
    where
        F: FnOnce(Socket) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let runtime = Runtime::new().unwrap();
        let listener = runtime.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
        let address = listener.local_addr().unwrap();
        let task = runtime.spawn(async move {
            let (stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                .await
                .expect("the binding did not connect within five seconds")
                .unwrap();
            let socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            script(socket).await;
        });
        let endpoint = Endpoint::new("gpt-realtime-2.1")
            .with_url(format!("ws://{address}/v1/realtime"))
            .with_read_timeout(Duration::from_secs(5));
        Self {
            endpoint,
            _runtime: runtime,
            _task: task,
        }
    }

    fn model(&self) -> OpenAiRealtime {
        OpenAiRealtime::new(
            self.endpoint.clone(),
            Box::new(FixedSource),
            OPENAI_KEY_NAME,
        )
    }
}

async fn send(socket: &mut Socket, event: Value) -> bool {
    socket.send(Message::text(event.to_string())).await.is_ok()
}

async fn receive(socket: &mut Socket) -> Value {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("the binding sent nothing for five seconds")
            .expect("the binding closed the connection")
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

fn config() -> SessionConfig {
    let pcm24 = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(24_000).unwrap(),
        channels: NonZeroU16::MIN,
    };
    SessionConfig {
        voice: "marin".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: pcm24,
        output_format: pcm24,
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools: Vec::new(),
        idle_limit: Duration::from_secs(30),
    }
}

/// `resp_1` completes, then the server sends its `response.done` again. The second is recorded
/// through a frame, the ledger refuses to complete a response no longer in progress, and the
/// binding drops that refusal: it is counted, and kept among the dropped events.
#[test]
fn a_refusal_inside_a_response_done_frame_is_counted() {
    let done = json!({"type": "response.done", "event_id": "event_rd", "response": {"id": "resp_1", "object": "realtime.response", "status": "completed", "output": [],
        "usage": {"total_tokens": 28, "input_tokens": 25, "output_tokens": 3}}});
    let again = done.clone();
    let server = Server::start(move |mut socket| async move {
        accept_session(&mut socket).await;
        send(
            &mut socket,
            json!({"type": "input_audio_buffer.committed", "event_id": "event_cm", "previous_item_id": null, "item_id": "item_1"}),
        )
        .await;
        send(
            &mut socket,
            json!({"type": "response.created", "event_id": "event_rc", "response": {"id": "resp_1", "object": "realtime.response", "status": "in_progress", "output": []}}),
        )
        .await;
        send(&mut socket, done).await;
        send(&mut socket, again).await;
        while let Some(Ok(_)) = socket.next().await {}
    });
    let mut model = server.model();
    let mut session = model.open(config()).expect("the session opens");
    let opened = Instant::now();
    let mut events = Vec::new();
    let mut completed = false;
    while opened.elapsed() < Duration::from_secs(3) {
        if let Some(event) = session.next_event(WAIT) {
            completed |= matches!(event, SessionEvent::ResponseCompleted(_));
            events.push(event);
        }
        if completed && session.ledger().dropped_count() > 0 {
            break;
        }
    }
    assert!(completed, "resp_1 did not complete: {events:#?}");
    let ledger = session.ledger();
    assert!(
        ledger.dropped_count() >= 1,
        "the second `response.done` for resp_1 was refused inside its frame, and the binding \
         counted no dropped refusal: dropped_count {}, dropped_events {:?}",
        ledger.dropped_count(),
        ledger.dropped_events()
    );
    assert!(
        ledger
            .dropped_events()
            .iter()
            .any(|refusal| matches!(refusal, SessionError::ResponseNotInProgress(_))),
        "no dropped refusal is the completion of a response no longer in progress: {:?}",
        ledger.dropped_events()
    );
}
