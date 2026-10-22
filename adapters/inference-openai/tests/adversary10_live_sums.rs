//! Adversary case, pass 10, story `duplex-late-usage-counted` at b873a7e: GPT-Live token sums
//! past `u64::MAX`. `known_sum`: a sum "`None` once a count was left out, or once the sum would
//! pass `u64::MAX`, since the sum is then unknown"; `backend_usage_recorded`: "the contract's
//! `ResponseUsage`, which has no unknown, stops at `u64::MAX`"; and a backend usage without its
//! `total_tokens` derives it from its input and output only while the sum fits. The pass-9 case
//! accepts a saturated record as well as an unknown one, so saturating `known_sum`, wrapping the
//! `ResponseUsage` sum and saturating the derived total each pass every suite. No network and
//! no key.
#![cfg(all(feature = "gpt-live", feature = "duplex"))]

use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use inference_audio::{AudioEncoding, AudioFormat};
use inference_duplex::{
    NativeSpeechModel, QuantityKind, ResponseId, SessionConfig, SessionEvent, SpeechSessionHandle,
    TurnDetection, TurnTuning,
};
use inference_openai::live::LIVE_MODEL;
use inference_openai::live_duplex::{OpenAiLive, OpenAiLiveSession};
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
            Message::Text(text) => return serde_json::from_str(text.as_str()).unwrap(),
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

/// Answers the binding's `session.close` with `session.closed`.
async fn closed(socket: &mut Socket) {
    let close = receive(socket).await;
    assert_eq!(close["type"], "session.close", "{close}");
    send(
        socket,
        json!({"type": "session.closed", "event_id": "evt_c", "reason": "close_requested", "session": {"id": "live_123", "model": "gpt-live-1", "status": "active"}, "usage": {"seconds": 1.5}}),
    )
    .await;
    while let Some(Ok(_)) = socket.next().await {}
}

fn said(delta: &str, start_ms: u64) -> Value {
    json!({"type": "session.output_transcript.delta", "event_id": "evt_out", "delta": delta, "start_ms": start_ms, "end_ms": start_ms + 200})
}

/// A backend `response.completed` carrying `usage`.
fn completed_with(usage: Value) -> Value {
    json!({"type": "response.event", "event_id": "evt_backend", "delegation_id": "del_1", "event": {
        "type": "response.completed", "sequence_number": 9,
        "response": {"id": "resp_1", "status": "completed", "output": [], "usage": usage}
    }})
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

/// Opens a session against `script`, reads until the first transcript delta, waits for the rest
/// of what the script sends to arrive, closes, and returns every event and the session.
fn run<F, Fut>(script: F) -> (Vec<SessionEvent>, OpenAiLiveSession)
where
    F: FnOnce(Socket) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let server = Server::start(script);
    let mut model = server.model();
    let mut session = model.open(config()).expect("the session opens");
    let mut events = Vec::new();
    let mut idle = 0;
    while idle < 10 {
        match session.next_event(WAIT) {
            Some(event) => {
                idle = 0;
                let done = matches!(event, SessionEvent::OutputTranscriptDelta(_));
                events.push(event);
                if done {
                    break;
                }
            }
            None => idle += 1,
        }
    }
    std::thread::sleep(Duration::from_millis(300));
    session.close().expect("open");
    events.extend(std::iter::from_fn(|| session.next_event(Duration::ZERO)));
    drop(server);
    (events, session)
}

fn first_response(events: &[SessionEvent]) -> ResponseId {
    events
        .iter()
        .find_map(|event| match event {
            SessionEvent::ResponseCreated(created) => Some(created.response_id),
            _ => None,
        })
        .expect("a response was created")
}

fn values(events: &[SessionEvent], response: ResponseId, kind: QuantityKind) -> Vec<Option<i64>> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::UsageRecorded(record) if record.response_id == Some(response) => {
                Some(record)
            }
            _ => None,
        })
        .flat_map(|record| record.quantities.iter())
        .filter(|quantity| quantity.kind() == kind)
        .map(|quantity| quantity.value())
        .collect()
}

/// Two backend usages of one response whose input counts sum past `u64::MAX`, the first
/// without its `total_tokens` and with input and output summing past `u64::MAX` too: the record's
/// input total is unknown, the contract's `ResponseUsage` stops at `u64::MAX`, and the first
/// backend usage's derived total is unknown.
#[test]
fn sums_past_u64_max_are_unknown_in_the_record_and_stop_at_u64_max_in_the_usage() {
    let (events, session) = run(|mut socket| async move {
        accept_start(&mut socket).await;
        send(&mut socket, said(" Hi.", 0)).await;
        send(
            &mut socket,
            completed_with(json!({"input_tokens": u64::MAX, "output_tokens": 1})),
        )
        .await;
        send(
            &mut socket,
            completed_with(json!({"input_tokens": 1, "output_tokens": 0})),
        )
        .await;
        closed(&mut socket).await;
    });
    let response = first_response(&events);
    assert_eq!(
        session.backend_usage().len(),
        2,
        "{:?}",
        session.backend_usage()
    );
    assert_eq!(
        session.backend_usage()[0].total_tokens,
        None,
        "an input and output summing past u64::MAX gave a known total: {:?}",
        session.backend_usage()[0]
    );
    assert_eq!(
        values(&events, response, QuantityKind::InputTextTokens),
        vec![None],
        "an input sum past u64::MAX was recorded as known"
    );
    assert_eq!(
        session
            .ledger()
            .usage(response)
            .map(|usage| usage.input.text_tokens),
        Some(u64::MAX),
        "the contract's input sum did not stop at u64::MAX"
    );
}
