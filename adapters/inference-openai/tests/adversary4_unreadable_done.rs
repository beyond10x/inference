//! Adversary case, pass 4, story `duplex-late-usage-counted` at d2feb27: round 2's finding A
//! (one malformed `response.done` costs the session every later response) for a `response.done`
//! that cannot be decoded at all.
//!
//! d2feb27 reads `response.done` field by field, so one that lacks `status` or `id` still ends
//! the response in progress. A `response.done` that is not JSON is still dropped whole: the
//! response it ended stays in progress, the ledger refuses the next `response.created`, and the
//! next turn is dropped, although the binding's own premise ("Realtime runs one response at a
//! time", `awaiting_done`) says a new `response.created` means the earlier one is over.
//! No network and no key.
#![cfg(feature = "duplex")]

use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use inference_audio::{AudioEncoding, AudioFormat};
use inference_duplex::{
    NativeSpeechModel, ResponseId, SessionConfig, SessionEvent, SpeechSessionHandle, TurnDetection,
    TurnTuning,
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

fn transcript_delta(response: &str, delta: &str) -> Value {
    json!({"type": "response.output_audio_transcript.delta", "event_id": "event_td", "response_id": response, "item_id": "item_o", "output_index": 0, "content_index": 0, "delta": delta})
}

fn response_done(id: &str) -> Value {
    json!({"type": "response.done", "event_id": "event_rd", "response": {
        "id": id, "object": "realtime.response", "status": "completed", "output": [],
        "usage": {"total_tokens": 40, "input_tokens": 25, "output_tokens": 15}
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
        turn_detection: TurnDetection::ServerVad,
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

/// The first turn's `response.done` arrives as text that is not JSON; the second turn is
/// well-formed throughout and must still reach the caller.
#[test]
fn an_unreadable_response_done_does_not_swallow_the_next_response() {
    let server = Server::start(|mut socket| async move {
        accept_session(&mut socket).await;
        send(&mut socket, committed("item_1")).await;
        send(&mut socket, response_created("resp_1")).await;
        send(&mut socket, transcript_delta("resp_1", "Hello")).await;
        socket
            .send(Message::text(
                r#"{"type":"response.done","event_id":"event_rd","response":{"id":"resp_1","status":"completed""#,
            ))
            .await
            .unwrap();
        send(&mut socket, committed("item_2")).await;
        send(&mut socket, response_created("resp_2")).await;
        send(&mut socket, transcript_delta("resp_2", "World")).await;
        send(&mut socket, response_done("resp_2")).await;
        while let Some(Ok(_)) = socket.next().await {}
    });
    let mut model = server.model();
    let mut session = model.open(config()).expect("the session opens");
    let events = until(&mut session, |event| {
        matches!(event, SessionEvent::ResponseCompleted(_))
    });
    let ids = created(&events);
    assert_eq!(
        ids.len(),
        2,
        "the second, well-formed response was dropped after an unreadable response.done: \
         {events:#?}"
    );
    assert_eq!(transcript(&events, ids[1]), "World", "{events:#?}");
    session.close().expect("open");
}

/// The caller cancels the first response; its `response.done` lacks `id` and arrives only after
/// the server created the next response. The binding gives an id-less `response.done` to the
/// response it awaits last, so the first response's late end cancels the second, whose words
/// are then refused.
#[test]
fn an_id_less_late_done_of_a_cancelled_response_does_not_end_the_next() {
    let server = Server::start(|mut socket| async move {
        accept_session(&mut socket).await;
        send(&mut socket, committed("item_1")).await;
        send(&mut socket, response_created("resp_1")).await;
        send(&mut socket, transcript_delta("resp_1", "Hello")).await;
        let cancel = receive(&mut socket).await;
        assert_eq!(cancel["type"], "response.cancel", "{cancel}");
        send(&mut socket, committed("item_2")).await;
        send(&mut socket, response_created("resp_2")).await;
        send(
            &mut socket,
            json!({"type": "response.done", "event_id": "event_rd1", "response": {"object": "realtime.response", "status": "cancelled", "output": []}}),
        )
        .await;
        send(&mut socket, transcript_delta("resp_2", "World")).await;
        send(&mut socket, response_done("resp_2")).await;
        while let Some(Ok(_)) = socket.next().await {}
    });
    let mut model = server.model();
    let mut session = model.open(config()).expect("the session opens");
    let first = until(&mut session, |event| {
        matches!(event, SessionEvent::OutputTranscriptDelta(_))
    });
    let first_id = created(&first)[0];
    session.cancel_response(first_id).expect("in progress");
    let events = until(&mut session, |event| {
        matches!(event, SessionEvent::ResponseCompleted(_))
    });
    let ids = created(&events);
    assert_eq!(ids.len(), 1, "{events:#?}");
    assert!(
        !events.iter().any(|event| matches!(
            event,
            SessionEvent::ResponseCancelled(cancelled) if cancelled.response_id == ids[0]
        )),
        "the first response's late end cancelled the second: {events:#?}"
    );
    assert_eq!(transcript(&events, ids[0]), "World", "{events:#?}");
    session.close().expect("open");
}
