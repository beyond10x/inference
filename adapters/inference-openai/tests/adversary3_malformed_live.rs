//! Adversary cases, pass 3, story `duplex-late-usage-counted` at 4f5a175: what the GPT-Live
//! binding and `LiveClient::close` do with a server message they cannot decode, now that they
//! drop one and read on. Before 4f5a175 such a message ended the read with a protocol error.
//! No network and no key.
#![cfg(all(feature = "gpt-live", feature = "duplex"))]

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::mpsc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use inference_audio::{AudioEncoding, AudioFormat};
use inference_duplex::{
    NativeSpeechModel, SessionConfig, SessionEvent, SpeechSessionHandle, TurnDetection, TurnTuning,
};
use inference_openai::live::{CLOSE_TIMEOUT, LIVE_MODEL};
use inference_openai::live_duplex::OpenAiLive;
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
const WAIT: Duration = Duration::from_millis(100);

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
        let endpoint = Endpoint::live(LIVE_MODEL)
            .with_url(format!("ws://{address}/v1/live/sessions"))
            .with_read_timeout(Duration::from_secs(5));
        Self {
            endpoint,
            _runtime: runtime,
            _task: task,
        }
    }

    fn model(&self) -> OpenAiLive {
        OpenAiLive::new(
            self.endpoint.clone(),
            Box::new(FixedSource),
            OPENAI_KEY_NAME,
        )
    }
}

async fn send(socket: &mut Socket, event: Value) {
    socket.send(Message::text(event.to_string())).await.unwrap();
}

async fn receive(socket: &mut Socket) -> Value {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("the binding sent nothing for five seconds")
            .expect("the binding closed the connection")
            .unwrap();
        match message {
            Message::Text(text) => {
                let mut event: Value = serde_json::from_str(text.as_str()).unwrap();
                if let Some(object) = event.as_object_mut() {
                    object.remove("event_id");
                }
                return event;
            }
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("expected a text frame, got {other:?}"),
        }
    }
}

async fn accept_start(socket: &mut Socket) {
    let start = receive(socket).await;
    assert_eq!(start["type"], "session.start", "{start}");
    let mut resolved = start["session"].clone();
    resolved["id"] = json!("live_123");
    resolved["status"] = json!("active");
    send(
        socket,
        json!({"type": "session.started", "event_id": "event_started_1", "session": resolved}),
    )
    .await;
}

fn said(delta: &str, start_ms: u64) -> Value {
    json!({"type": "session.output_transcript.delta", "event_id": "evt_out", "delta": delta, "start_ms": start_ms, "end_ms": start_ms + 200})
}

fn backend(delegation: &str, event: Value) -> Value {
    json!({"type": "response.event", "event_id": "evt_backend", "delegation_id": delegation, "event": event})
}

fn backend_usage(input: u64, cached: u64, output: u64) -> Value {
    json!({"input_tokens": input, "input_tokens_details": {"cached_tokens": cached}, "output_tokens": output, "output_tokens_details": {"reasoning_tokens": 0}, "total_tokens": input + output})
}

fn backend_completed(response: &str, input: u64, cached: u64, output: u64) -> Value {
    json!({"type": "response.completed", "sequence_number": 9, "response": {
        "id": response, "status": "completed", "output": [],
        "usage": backend_usage(input, cached, output)
    }})
}

/// A `response.completed` with its usage and no `response.id`. The binding sums backend usage
/// onto its own response and never reads the backend's id, yet `LiveEvent::parse` refuses the
/// whole event for the missing id.
fn backend_completed_without_id(input: u64, cached: u64, output: u64) -> Value {
    json!({"type": "response.completed", "sequence_number": 8, "response": {
        "status": "completed", "output": [],
        "usage": backend_usage(input, cached, output)
    }})
}

fn closed() -> Value {
    json!({"type": "session.closed", "event_id": "evt_c", "reason": "close_requested", "session": {"id": "live_123", "model": "gpt-live-1", "status": "active"}, "usage": {"seconds": 1.5}})
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
        turn_detection: TurnDetection::SemanticVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools: Vec::new(),
        idle_limit: Duration::from_secs(5),
    }
}

fn until(
    session: &mut impl SpeechSessionHandle,
    stop: impl Fn(&SessionEvent) -> bool,
) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    let mut idle = 0;
    while idle < 20 {
        match session.next_event(WAIT) {
            Some(event) => {
                idle = 0;
                let done = stop(&event);
                events.push(event);
                if done {
                    break;
                }
            }
            None => idle += 1,
        }
    }
    events
}

/// The close of a GPT-Live session ends whatever the server sends. A server that answers
/// `session.close` with undecodable text every 100 ms and never `session.closed` holds
/// `LiveClient::close`, and with it `SpeechSessionHandle::close`, for as long as it keeps
/// sending: the read timeout is per read, and each message resets it. A silent server is given
/// up after `CLOSE_TIMEOUT`; the close is given that bound and five seconds more here.
#[test]
fn a_close_answered_only_by_undecodable_messages_returns() {
    let server = Server::start(|mut socket| async move {
        accept_start(&mut socket).await;
        let close = receive(&mut socket).await;
        assert_eq!(close["type"], "session.close");
        let until = tokio::time::Instant::now() + CLOSE_TIMEOUT * 3;
        while tokio::time::Instant::now() < until {
            if socket.send(Message::text("not json")).await.is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
    let mut model = server.model();
    let mut session = model.open(config()).expect("the session opens");
    let (done, returned) = mpsc::channel();
    std::thread::spawn(move || {
        let closed = session.close();
        let _ = done.send(closed.is_ok());
    });
    let bound = CLOSE_TIMEOUT + Duration::from_secs(5);
    assert!(
        returned.recv_timeout(bound).is_ok(),
        "close() had not returned after {bound:?} of undecodable messages"
    );
}

/// The usage a backend `response.completed` reports is billable, and the binding needs nothing
/// else from it: it sums usage onto its own response. One without `response.id` is dropped whole,
/// so its tokens are never counted and nothing says so.
#[test]
fn backend_usage_of_a_completed_event_without_an_id_is_counted() {
    let server = Server::start(|mut socket| async move {
        accept_start(&mut socket).await;
        send(&mut socket, said(" Hi.", 0)).await;
        send(
            &mut socket,
            backend("del_1", backend_completed_without_id(20, 0, 7)),
        )
        .await;
        send(
            &mut socket,
            backend("del_2", backend_completed("resp_2", 12, 4, 3)),
        )
        .await;
        let close = receive(&mut socket).await;
        assert_eq!(close["type"], "session.close");
        send(&mut socket, closed()).await;
        let _ = socket.close(None).await;
    });
    let mut model = server.model();
    let mut session = model.open(config()).expect("the session opens");
    let events = until(&mut session, |_| false);
    assert_eq!(
        session.backend_usage().len(),
        2,
        "two backend responses reported usage; the binding counted {:?}: {events:#?}",
        session.backend_usage()
    );
    session.close().expect("open");
}
