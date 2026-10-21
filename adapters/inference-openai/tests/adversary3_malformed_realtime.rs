//! Adversary cases, pass 3, story `duplex-late-usage-counted` at 4f5a175: what the Realtime
//! binding does with a server message it cannot decode, now that it drops one and reads on.
//!
//! Each server below sends one message that `ServerEvent::parse` refuses: a `response.done` whose
//! `response` lacks a field the reference requires, or text that is not JSON. Before 4f5a175 such
//! a message failed the session as a provider error; now it is dropped, and the cases ask what
//! that drop costs. No network and no key.
#![cfg(feature = "duplex")]

use std::num::{NonZeroU16, NonZeroU32};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use inference_audio::{AudioEncoding, AudioFormat};
use inference_duplex::{
    NativeSpeechModel, QuantityKind, ResponseId, SessionConfig, SessionEvent, SessionFailureKind,
    SpeechSessionHandle, TurnDetection, TurnTuning, UsageRecorded,
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

/// A local server running one script on its own runtime; `runtime` and `task` are held so the
/// server lives as long as the test.
#[allow(dead_code)]
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

fn response_created(id: &str) -> Value {
    json!({"type": "response.created", "event_id": "event_rc", "response": {"id": id, "object": "realtime.response", "status": "in_progress", "output": []}})
}

fn transcript_delta(response: &str, delta: &str) -> Value {
    json!({"type": "response.output_audio_transcript.delta", "event_id": "event_td", "response_id": response, "item_id": "item_o", "output_index": 0, "content_index": 0, "delta": delta})
}

fn usage() -> Value {
    json!({
        "total_tokens": 40, "input_tokens": 25, "output_tokens": 15,
        "input_token_details": {"text_tokens": 5, "audio_tokens": 20, "cached_tokens": 4},
        "output_token_details": {"text_tokens": 3, "audio_tokens": 12}
    })
}

fn response_done(id: &str) -> Value {
    json!({"type": "response.done", "event_id": "event_rd", "response": {
        "id": id, "object": "realtime.response", "status": "completed", "output": [],
        "usage": usage()
    }})
}

/// A `response.done` whose `response` carries its id and a well-formed usage but no `status`:
/// `ServerEvent::parse` refuses it (`response.string("status")?`), usage and all.
fn response_done_without_status(id: &str) -> Value {
    json!({"type": "response.done", "event_id": "event_rd", "response": {
        "id": id, "object": "realtime.response", "output": [],
        "usage": usage()
    }})
}

fn committed(item: &str) -> Value {
    json!({"type": "input_audio_buffer.committed", "event_id": "event_cm", "previous_item_id": null, "item_id": item})
}

fn pcm24() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(24_000).unwrap(),
        channels: NonZeroU16::MIN,
    }
}

fn config(turn_detection: TurnDetection, auto: bool, idle_limit: Duration) -> SessionConfig {
    SessionConfig {
        voice: "marin".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: pcm24(),
        output_format: pcm24(),
        turn_detection,
        turn_tuning: TurnTuning::default(),
        auto_response: auto,
        auto_interrupt: auto,
        tools: Vec::new(),
        idle_limit,
    }
}

/// Events until one `stop` accepts, or until twenty waits in a row deliver nothing.
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

fn created(events: &[SessionEvent]) -> Vec<ResponseId> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ResponseCreated(created) => Some(created.response_id),
            _ => None,
        })
        .collect()
}

fn transcript(events: &[SessionEvent], response_id: ResponseId) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::OutputTranscriptDelta(delta) if delta.response_id == response_id => {
                Some(delta.text.as_str())
            }
            _ => None,
        })
        .collect()
}

fn output_text_tokens(record: &UsageRecorded) -> Option<i64> {
    record
        .quantities
        .iter()
        .find(|quantity| quantity.kind() == QuantityKind::OutputTextTokens)
        .and_then(|quantity| quantity.value())
}

/// One malformed `response.done` must not cost the session every later response. The commit
/// says what follows an undecodable message "is recorded too"; here the second turn is
/// well-formed throughout, and the binding still drops it, because the first response never
/// ended and the ledger refuses a second `ResponseCreated` while one is in progress.
#[test]
fn a_malformed_response_done_does_not_swallow_the_next_response() {
    let server = Server::start(|mut socket| async move {
        accept_session(&mut socket).await;
        send(&mut socket, committed("item_1")).await;
        send(&mut socket, response_created("resp_1")).await;
        send(&mut socket, transcript_delta("resp_1", "Hello")).await;
        send(&mut socket, response_done_without_status("resp_1")).await;
        send(&mut socket, committed("item_2")).await;
        send(&mut socket, response_created("resp_2")).await;
        send(&mut socket, transcript_delta("resp_2", "World")).await;
        send(&mut socket, response_done("resp_2")).await;
        while let Some(Ok(_)) = socket.next().await {}
    });
    let mut model = server.model();
    let mut session = model
        .open(config(
            TurnDetection::ServerVad,
            true,
            Duration::from_secs(5),
        ))
        .expect("the session opens");
    let events = until(&mut session, |event| {
        matches!(event, SessionEvent::ResponseCompleted(_))
    });
    let ids = created(&events);
    assert_eq!(
        ids.len(),
        2,
        "the second, well-formed response was dropped after a malformed response.done: {events:#?}"
    );
    assert_eq!(transcript(&events, ids[1]), "World", "{events:#?}");
    session.close().expect("open");
}

/// The usage a malformed `response.done` carried is well-formed: only `status` is missing. It is
/// billable and the binding has the response it names, yet nothing counts it: the response is
/// recorded at the close with every count unknown, and no event says why.
#[test]
fn the_well_formed_usage_of_a_response_done_without_status_is_counted() {
    let server = Server::start(|mut socket| async move {
        accept_session(&mut socket).await;
        send(&mut socket, committed("item_1")).await;
        send(&mut socket, response_created("resp_1")).await;
        send(&mut socket, transcript_delta("resp_1", "Hello")).await;
        send(&mut socket, response_done_without_status("resp_1")).await;
        while let Some(Ok(_)) = socket.next().await {}
    });
    let mut model = server.model();
    let mut session = model
        .open(config(
            TurnDetection::ServerVad,
            true,
            Duration::from_secs(5),
        ))
        .expect("the session opens");
    let mut events = until(&mut session, |event| {
        matches!(event, SessionEvent::OutputTranscriptDelta(_))
    });
    std::thread::sleep(Duration::from_millis(300));
    session.close().expect("open");
    events.extend(std::iter::from_fn(|| session.next_event(Duration::ZERO)));
    let response = created(&events)[0];
    let recorded: Vec<&UsageRecorded> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::UsageRecorded(recorded) if recorded.response_id == Some(response) => {
                Some(recorded)
            }
            _ => None,
        })
        .collect();
    assert_eq!(recorded.len(), 1, "{events:#?}");
    assert_eq!(
        output_text_tokens(recorded[0]),
        Some(3),
        "the response.done reported 3 output text tokens; the record says {:?}",
        recorded[0]
    );
}

/// The drop is silent and the failure that follows names the wrong cause. The provider ended the
/// response with a `response.done` the binding could not read, then went quiet; before 4f5a175
/// the session failed at once as a provider error naming `response.done`. Now the only end is
/// the liveness bound, which reports that "the provider sent nothing".
#[test]
fn a_malformed_response_done_is_reported_as_what_it_is() {
    let server = Server::start(|mut socket| async move {
        accept_session(&mut socket).await;
        assert_eq!(
            receive(&mut socket).await,
            json!({"type": "response.create"})
        );
        send(&mut socket, response_created("resp_1")).await;
        send(&mut socket, transcript_delta("resp_1", "Hello")).await;
        send(&mut socket, response_done_without_status("resp_1")).await;
        tokio::time::sleep(Duration::from_secs(3)).await;
    });
    let mut model = server.model();
    let mut session = model
        .open(config(
            TurnDetection::Disabled,
            false,
            Duration::from_millis(400),
        ))
        .expect("the session opens");
    session.create_response().expect("none in progress");
    let events = until(&mut session, |event| {
        matches!(event, SessionEvent::SpeechSessionFailed(_))
    });
    let Some(SessionEvent::SpeechSessionFailed(failed)) = events.last() else {
        panic!("the session never ended: {events:#?}");
    };
    assert_eq!(
        failed.failure.kind,
        SessionFailureKind::ProviderError,
        "the provider sent a response.done the binding could not read; the failure says: {:?}",
        failed.failure
    );
    assert!(
        failed.failure.detail.contains("response.done"),
        "{:?}",
        failed.failure
    );
}

/// A server that sends nothing but undecodable text keeps an awaited session open: each message
/// refreshes the liveness bound although none is an event, so the stall the bound exists to
/// catch never fires and the caller waits on a response that cannot come.
#[test]
fn endless_undecodable_messages_do_not_keep_an_awaited_session_open() {
    let server = Server::start(|mut socket| async move {
        accept_session(&mut socket).await;
        assert_eq!(
            receive(&mut socket).await,
            json!({"type": "response.create"})
        );
        let until = tokio::time::Instant::now() + Duration::from_secs(6);
        while tokio::time::Instant::now() < until {
            if socket.send(Message::text("not json")).await.is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });
    let mut model = server.model();
    let mut session = model
        .open(config(
            TurnDetection::Disabled,
            false,
            Duration::from_millis(400),
        ))
        .expect("the session opens");
    let asked = Instant::now();
    session.create_response().expect("none in progress");
    let events = until(&mut session, |event| {
        matches!(event, SessionEvent::SpeechSessionFailed(_))
    });
    let waited = asked.elapsed();
    assert!(
        matches!(events.last(), Some(SessionEvent::SpeechSessionFailed(_))),
        "after {waited:?} of undecodable messages and a 400 ms bound the session is {:?}: {events:#?}",
        session.ledger().session().state()
    );
}
