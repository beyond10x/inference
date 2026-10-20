//! Adversary pass 1 on the OpenAI Realtime usage records, over a local WebSocket server that
//! replays the provider: a response that fails with partial usage, and a response the caller
//! cancelled whose `response.done` never comes because the transport closes first.

use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::{SinkExt, StreamExt};
use inference_audio::{AudioEncoding, AudioFormat};
use inference_duplex::{
    NativeSpeechModel, Quantity, QuantityKind, SessionConfig, SessionEvent, SpeechSessionHandle,
    TurnDetection, TurnTuning, UsageRecorded,
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

const WAIT: Duration = Duration::from_millis(100);

struct FixedSource;

impl CredentialSource for FixedSource {
    fn resolve<'a>(&'a self, name: &'a str) -> CredentialFuture<'a> {
        let found = name == OPENAI_KEY_NAME;
        Box::pin(async move {
            if found {
                Ok(Credential::new(b"offline-test-credential".to_vec()))
            } else {
                Err(CredentialError::Missing)
            }
        })
    }
}

type Socket = WebSocketStream<TcpStream>;

struct Server {
    endpoint: Endpoint,
    runtime: Runtime,
    task: JoinHandle<()>,
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
            runtime,
            task,
        }
    }

    fn model(&self) -> OpenAiRealtime {
        OpenAiRealtime::new(
            self.endpoint.clone(),
            Box::new(FixedSource),
            OPENAI_KEY_NAME,
        )
    }

    fn finish(self) {
        self.runtime.block_on(self.task).unwrap();
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

fn committed(item: &str) -> Value {
    json!({"type": "input_audio_buffer.committed", "event_id": "event_cm", "previous_item_id": null, "item_id": item})
}

fn response_created(id: &str) -> Value {
    json!({"type": "response.created", "event_id": "event_rc", "response": {"id": id, "object": "realtime.response", "status": "in_progress", "output": []}})
}

fn audio_delta(response: &str) -> Value {
    json!({"type": "response.output_audio.delta", "event_id": "event_ad", "response_id": response, "item_id": "item_o1", "output_index": 0, "content_index": 0, "delta": STANDARD.encode([4u8; 480])})
}

fn pcm24() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(24_000).unwrap(),
        channels: NonZeroU16::MIN,
    }
}

fn config(auto_interrupt: bool) -> SessionConfig {
    SessionConfig {
        voice: "marin".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: pcm24(),
        output_format: pcm24(),
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt,
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

fn records(events: &[SessionEvent]) -> Vec<&UsageRecorded> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::UsageRecorded(recorded) => Some(recorded),
            _ => None,
        })
        .collect()
}

const TOKENS: [QuantityKind; 5] = [
    QuantityKind::InputTextTokens,
    QuantityKind::InputAudioTokens,
    QuantityKind::CachedInputTokens,
    QuantityKind::OutputTextTokens,
    QuantityKind::OutputAudioTokens,
];

/// `response.done` with `status: failed` and usage that reports only the input text tokens: the
/// record comes just before `ResponseFailed`, the input text tokens reported and every other
/// token count unknown.
#[test]
fn a_failed_response_is_recorded_before_its_failure_with_absent_counts_unknown() {
    let server = Server::start(|mut socket| async move {
        accept_session(&mut socket).await;
        send(&mut socket, committed("item_1")).await;
        send(&mut socket, response_created("resp_1")).await;
        send(
            &mut socket,
            json!({"type": "response.done", "event_id": "event_rd", "response": {
                "id": "resp_1", "object": "realtime.response", "status": "failed", "output": [],
                "status_details": {"type": "failed", "error": {"type": "server_error", "code": "x"}},
                "usage": {
                    "total_tokens": 10, "input_tokens": 10, "output_tokens": 0,
                    "input_token_details": {"text_tokens": 10}
                }
            }}),
        )
        .await;
        let _ = socket.next().await;
    });
    let mut model = server.model();
    let mut session = model.open(config(true)).expect("the session opens");
    let events = until(&mut session, |event| {
        matches!(event, SessionEvent::ResponseFailed(_))
    });
    session.close().expect("open");
    server.finish();
    let failed = events
        .iter()
        .position(|event| matches!(event, SessionEvent::ResponseFailed(_)))
        .expect("the response fails");
    let SessionEvent::ResponseFailed(failure) = &events[failed] else {
        unreachable!()
    };
    assert!(
        matches!(&events[failed - 1], SessionEvent::UsageRecorded(record)
            if record.response_id == Some(failure.response_id)),
        "{events:#?}"
    );
    let record = records(&events)[0];
    let expected: Vec<Quantity> = TOKENS
        .iter()
        .map(|&kind| match kind {
            QuantityKind::InputTextTokens => Quantity::provider_reported(kind, 10),
            _ => Quantity::unknown(kind),
        })
        .chain([Quantity::measured(QuantityKind::Requests, 1)])
        .collect();
    assert_eq!(record.quantities, expected);
}

/// The caller cancels a response and the transport closes before its `response.done`: the
/// failing session records the cancelled response once, every token count unknown, before
/// `SpeechSessionFailed`.
#[test]
fn a_cancelled_response_whose_done_never_comes_is_recorded_before_the_sessions_failure() {
    let server = Server::start(|mut socket| async move {
        accept_session(&mut socket).await;
        send(&mut socket, committed("item_1")).await;
        send(&mut socket, response_created("resp_1")).await;
        send(&mut socket, audio_delta("resp_1")).await;
        assert_eq!(
            receive(&mut socket).await,
            json!({"type": "response.cancel", "response_id": "resp_1"})
        );
        socket.close(None).await.unwrap();
    });
    let mut model = server.model();
    let mut session = model.open(config(false)).expect("the session opens");
    let events = until(&mut session, |event| {
        matches!(event, SessionEvent::OutputAudio(_))
    });
    assert!(records(&events).is_empty(), "{events:#?}");
    let response = session.ledger().in_progress().expect("in progress");
    session.cancel_response(response).expect("in progress");
    let rest = until(&mut session, |event| {
        matches!(event, SessionEvent::SpeechSessionFailed(_))
    });
    server.finish();
    let failed = rest
        .iter()
        .position(|event| matches!(event, SessionEvent::SpeechSessionFailed(_)))
        .expect("the transport closes");
    let recorded = records(&rest);
    assert_eq!(recorded.len(), 1, "{rest:#?}");
    assert_eq!(recorded[0].response_id, Some(response));
    assert!(
        matches!(&rest[failed - 1], SessionEvent::UsageRecorded(_)),
        "{rest:#?}"
    );
    for kind in TOKENS {
        assert!(
            recorded[0].quantities.contains(&Quantity::unknown(kind)),
            "{:#?}",
            recorded[0]
        );
    }
    let after: Vec<_> = std::iter::from_fn(|| session.next_event(Duration::ZERO)).collect();
    assert!(records(&after).is_empty(), "{after:#?}");
    assert_eq!(session.ledger().usage_records().len(), 1);
}
