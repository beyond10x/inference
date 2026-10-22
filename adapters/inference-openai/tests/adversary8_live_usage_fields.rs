//! Adversary case, pass 8, story `duplex-late-usage-counted` at e9a4d4b: GPT-Live backend usage
//! read field by field.
//!
//! The story's rule since d2feb27: a missing optional or unused field no longer discards the
//! frame's billable usage. The Realtime binding takes each count from its own field and keeps a
//! count the server left out unknown. e9a4d4b reads a GPT-Live backend usage without
//! `output_tokens` that way, but one without `input_tokens` records nothing, its readable
//! `output_tokens` with it (`Fields::backend_usage`, `live.rs`). And a backend end whose
//! `response` is not an object is no frame the binding could not read, although its usage is
//! unread. No network and no key.
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

/// A backend end of `kind` (`response.completed` or `response.failed`) whose `response` is
/// `response`.
fn backend_end(kind: &str, response: Value) -> Value {
    json!({"type": "response.event", "event_id": "evt_backend", "delegation_id": "del_1", "event": {
        "type": kind, "sequence_number": 9, "response": response
    }})
}

/// A backend `response.completed` with `usage` as its usage.
fn completed_with(usage: Value) -> Value {
    backend_end(
        "response.completed",
        json!({"id": "resp_1", "status": "completed", "output": [], "usage": usage}),
    )
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
    while idle < 10 {
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

/// Opens a session against `script`, reads until the first transcript delta, waits for the rest
/// of what the script sends to arrive, closes, and returns every event and the session.
fn run<F, Fut>(
    script: F,
) -> (
    Vec<SessionEvent>,
    inference_openai::live_duplex::OpenAiLiveSession,
)
where
    F: FnOnce(Socket) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let server = Server::start(script);
    let mut model = server.model();
    let mut session = model.open(config()).expect("the session opens");
    let mut events = until(&mut session, |event| {
        matches!(event, SessionEvent::OutputTranscriptDelta(_))
    });
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

/// Every reported value of `kind` in the usage records of `response`: `Some(n)` for a known
/// count, `None` for an unknown one.
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

/// The server reports 7 output tokens for a backend response and leaves out `input_tokens`.
/// Read field by field, as a usage without `output_tokens` already is, the 7 output tokens count
/// and the input is unknown; the frame counts once as read in part. e9a4d4b drops the whole
/// usage, so the response's record carries no output tokens at all.
#[test]
fn a_backend_usage_without_input_tokens_keeps_its_output_tokens() {
    let (events, session) = run(|mut socket| async move {
        accept_start(&mut socket).await;
        send(&mut socket, said(" Hi.", 0)).await;
        send(
            &mut socket,
            completed_with(json!({"output_tokens": 7, "output_tokens_details": {"reasoning_tokens": 0}, "total_tokens": 7})),
        )
        .await;
        closed(&mut socket).await;
    });
    let response = first_response(&events);
    assert_eq!(
        session.ledger().unreadable_frames(),
        1,
        "the frame read in part counts once: {events:#?}"
    );
    assert_eq!(
        values(&events, response, QuantityKind::OutputTextTokens),
        vec![Some(7)],
        "the 7 output tokens the server reported were dropped with the missing input_tokens \
         (backend usage {:?}): {events:#?}",
        session.backend_usage()
    );
}

/// A backend `response.completed` whose `response` is a string, and a `response.failed` without
/// any `response`: neither usage could be read, so each counts once as a frame the binding could
/// not read in full, as a Realtime `response.done` without its `response` object does (through
/// its missing `status`). e9a4d4b counts neither.
#[test]
fn a_backend_end_without_a_response_object_counts_as_unreadable() {
    let (events, session) = run(|mut socket| async move {
        accept_start(&mut socket).await;
        send(&mut socket, said(" Hi.", 0)).await;
        send(
            &mut socket,
            backend_end("response.completed", json!("resp_1")),
        )
        .await;
        send(
            &mut socket,
            json!({"type": "response.event", "event_id": "evt_backend", "delegation_id": "del_1", "event": {
                "type": "response.failed", "sequence_number": 10
            }}),
        )
        .await;
        closed(&mut socket).await;
    });
    assert_eq!(
        session.ledger().unreadable_frames(),
        2,
        "two backend ends whose usage could not be read left no trace: {events:#?}"
    );
}

/// Two backend responses on one GPT-Live response: the first leaves out `output_tokens`, the
/// second reports 5. The response's output tokens stay unknown, since one count was left out;
/// its input tokens are the sum, 30; the first frame counts once as read in part.
#[test]
fn an_output_count_left_out_keeps_the_sum_unknown_after_a_known_one() {
    let (events, session) = run(|mut socket| async move {
        accept_start(&mut socket).await;
        send(&mut socket, said(" Hi.", 0)).await;
        send(
            &mut socket,
            completed_with(json!({"input_tokens": 20, "total_tokens": 20})),
        )
        .await;
        send(
            &mut socket,
            completed_with(json!({"input_tokens": 10, "output_tokens": 5, "total_tokens": 15})),
        )
        .await;
        closed(&mut socket).await;
    });
    let response = first_response(&events);
    assert_eq!(session.ledger().unreadable_frames(), 1, "{events:#?}");
    assert_eq!(
        values(&events, response, QuantityKind::InputTextTokens),
        vec![Some(30)],
        "{events:#?}"
    );
    assert_eq!(
        values(&events, response, QuantityKind::OutputTextTokens),
        vec![None],
        "a later known count made the output sum known again: {events:#?}"
    );
}

/// Each field missing or of the wrong type, one backend end per shape. Exactly the shapes whose
/// usage was read only in part or not at all count, once each, and no input token is counted
/// twice or lost where `input_tokens` was readable.
#[test]
fn each_unreadable_usage_shape_counts_once_and_no_readable_input_is_lost() {
    let (events, session) = run(|mut socket| async move {
        accept_start(&mut socket).await;
        send(&mut socket, said(" Hi.", 0)).await;
        for usage in [
            json!({"input_tokens": "20", "output_tokens": 3}),
            json!({"input_tokens": 20, "output_tokens": "3"}),
            json!({"input_tokens": -1, "output_tokens": 3}),
            json!({"input_tokens": 2.5, "output_tokens": 3}),
            json!([]),
            Value::Null,
            json!({"input_tokens": 4, "output_tokens": 2}),
        ] {
            send(&mut socket, completed_with(usage)).await;
        }
        send(
            &mut socket,
            backend_end(
                "response.failed",
                json!({"id": "resp_9", "status": "failed", "usage": {"input_tokens": 6}}),
            ),
        )
        .await;
        closed(&mut socket).await;
    });
    let response = first_response(&events);
    assert_eq!(
        session.ledger().unreadable_frames(),
        6,
        "string, negative and fractional input, string output, an array and a missing output \
         each count once; null and a whole usage count nothing: {events:#?}"
    );
    let inputs: Vec<u64> = session
        .backend_usage()
        .iter()
        .filter_map(|usage| usage.input_tokens)
        .collect();
    assert_eq!(inputs, vec![20, 4, 6], "{events:#?}");
    // a malformed input count makes the input total unknown (field-by-field rule)
    assert_eq!(
        values(&events, response, QuantityKind::InputTextTokens),
        vec![None],
        "{events:#?}"
    );
    assert_eq!(
        values(&events, response, QuantityKind::OutputTextTokens),
        vec![None],
        "{events:#?}"
    );
}
