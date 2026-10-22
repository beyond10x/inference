//! Adversary case, pass 4, story `duplex-late-usage-counted` at d2feb27: the failure kind of a
//! `session.closed` with reason `connection_lost`.
//!
//! `live_duplex.rs` documents that such a `session.closed` "fails the session as a closed
//! transport", and that one "without its reason or seconds still ends the session". d2feb27
//! counts a `session.closed` lacking `usage.seconds` as an unreadable frame before it fails the
//! session, so the very frame that reports the lost connection turns its own failure into a
//! provider error. No network and no key.
#![cfg(all(feature = "gpt-live", feature = "duplex"))]

use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use inference_audio::{AudioEncoding, AudioFormat};
use inference_duplex::{
    NativeSpeechModel, SessionConfig, SessionEvent, SessionFailureKind, SpeechSessionHandle,
    TurnDetection, TurnTuning,
};
use inference_openai::live::LIVE_MODEL;
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

fn lost(usage: Option<f64>) -> Value {
    let mut closed = json!({"type": "session.closed", "event_id": "evt_c", "reason": "connection_lost", "session": {"id": "live_123", "model": "gpt-live-1", "status": "active"}});
    if let Some(seconds) = usage {
        closed["usage"] = json!({"seconds": seconds});
    }
    closed
}

fn failure_kind(usage: Option<f64>) -> (SessionFailureKind, String) {
    let server = Server::start(move |mut socket| async move {
        accept_start(&mut socket).await;
        send(&mut socket, said(" Hi.", 0)).await;
        send(&mut socket, lost(usage)).await;
        while let Some(Ok(_)) = socket.next().await {}
    });
    let mut model = server.model();
    let mut session = model.open(config()).expect("the session opens");
    let events = until(&mut session, |event| {
        matches!(event, SessionEvent::SpeechSessionFailed(_))
    });
    let Some(SessionEvent::SpeechSessionFailed(failed)) = events.last() else {
        panic!("the session did not fail: {events:#?}");
    };
    (failed.failure.kind, failed.failure.detail.clone())
}

/// Control: a `connection_lost` that reports its seconds fails the session as a closed
/// transport.
#[test]
fn connection_lost_with_its_seconds_fails_as_a_closed_transport() {
    let (kind, detail) = failure_kind(Some(1.5));
    assert_eq!(kind, SessionFailureKind::TransportClosed, "{detail}");
}

/// A `connection_lost` without `usage.seconds` still reports a lost connection; its missing
/// seconds are no earlier unreadable frame, yet they turn its own failure into a provider error.
#[test]
fn connection_lost_without_its_seconds_fails_as_a_closed_transport() {
    let (kind, detail) = failure_kind(None);
    assert_eq!(kind, SessionFailureKind::TransportClosed, "{detail}");
}
