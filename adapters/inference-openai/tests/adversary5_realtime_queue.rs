//! Adversary case, pass 5, story `duplex-late-usage-counted` at 457724a: the queue of responses
//! awaiting their `response.done` (`awaiting_done`), the response a `response.created` ends while
//! another is in progress (`ended_unseen`), and the usage a late `response.done` carries.
//!
//! The story's acceptance: "A usage delta for a response that arrives after the response's usage
//! was recorded appears in the session's usage records, and the session total counts it once."
//! The binding's own premise (`awaiting_done`): "Realtime ends its responses in the order it
//! created them". No network and no key.
#![cfg(feature = "duplex")]

use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use inference_audio::{AudioEncoding, AudioFormat};
use inference_duplex::{
    NativeSpeechModel, QuantityKind, ResponseId, SessionConfig, SessionEvent, SpeechSessionHandle,
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

/// A `response.done` with every usage count, `output_audio` output audio tokens among them;
/// without `id` when `id` is `None`.
fn response_done(id: Option<&str>, status: &str, output_audio: u64) -> Value {
    let mut response = json!({
        "object": "realtime.response", "status": status, "output": [],
        "usage": {
            "total_tokens": 28 + output_audio, "input_tokens": 25, "output_tokens": 3 + output_audio,
            "input_token_details": {"text_tokens": 5, "audio_tokens": 20, "cached_tokens": 0},
            "output_token_details": {"text_tokens": 3, "audio_tokens": output_audio}
        }
    });
    if let Some(id) = id {
        response["id"] = json!(id);
    }
    json!({"type": "response.done", "event_id": "event_rd", "response": response})
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

/// Events until `stop` says so, or until two seconds pass with none.
fn until(
    session: &mut impl SpeechSessionHandle,
    mut stop: impl FnMut(&SessionEvent) -> bool,
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

/// Closes the session and returns every event up to `SpeechSessionClosed`.
fn close(session: &mut impl SpeechSessionHandle) -> Vec<SessionEvent> {
    session.close().expect("open");
    until(session, |event| {
        matches!(event, SessionEvent::SpeechSessionClosed(_))
    })
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

fn completed(events: &[SessionEvent], response_id: ResponseId) -> bool {
    events.iter().any(|event| {
        matches!(event, SessionEvent::ResponseCompleted(done) if done.response_id == response_id)
    })
}

/// The known output audio tokens over every usage record of the session, and the records with
/// that count unknown.
fn output_audio(events: &[SessionEvent]) -> (i64, usize) {
    let mut known = 0;
    let mut unknown = 0;
    for event in events {
        if let SessionEvent::UsageRecorded(record) = event {
            for quantity in &record.quantities {
                if quantity.kind() == QuantityKind::OutputAudioTokens {
                    match quantity.value() {
                        Some(value) => known += value,
                        None => unknown += 1,
                    }
                }
            }
        }
    }
    (known, unknown)
}

/// Round 3's own case (`an_id_less_late_done_of_a_cancelled_response_does_not_end_the_next`),
/// with the usage the first response's late `response.done` carries: the caller cancels the
/// first response, the server creates the second, and only then sends the first one's
/// `response.done`, without its id, carrying 12 output audio tokens. 457724a now gives that
/// done to the first response, but the first response was recorded with every count unknown
/// when the second was created, and the late usage is dropped.
#[test]
fn the_usage_of_a_cancelled_responses_late_id_less_done_is_counted_once() {
    let server = Server::start(|mut socket| async move {
        accept_session(&mut socket).await;
        send(&mut socket, committed("item_1")).await;
        send(&mut socket, response_created("resp_1")).await;
        send(&mut socket, transcript_delta("resp_1", "Hello")).await;
        let cancel = receive(&mut socket).await;
        assert_eq!(cancel["type"], "response.cancel", "{cancel}");
        send(&mut socket, committed("item_2")).await;
        send(&mut socket, response_created("resp_2")).await;
        send(&mut socket, response_done(None, "cancelled", 12)).await;
        send(&mut socket, transcript_delta("resp_2", "World")).await;
        send(&mut socket, response_done(Some("resp_2"), "completed", 16)).await;
        while let Some(Ok(_)) = socket.next().await {}
    });
    let mut model = server.model();
    let mut session = model.open(config()).expect("the session opens");
    let mut events = until(&mut session, |event| {
        matches!(event, SessionEvent::OutputTranscriptDelta(_))
    });
    let first = created(&events)[0];
    session.cancel_response(first).expect("in progress");
    events.extend(until(&mut session, |event| {
        matches!(event, SessionEvent::ResponseCompleted(_))
    }));
    events.extend(close(&mut session));
    let ids = created(&events);
    assert_eq!(ids.len(), 2, "{events:#?}");
    assert!(completed(&events, ids[1]), "{events:#?}");
    assert_eq!(
        output_audio(&events),
        (12 + 16, 0),
        "the first response's late usage (12 output audio tokens) is not in the usage records: \
         {events:#?}"
    );
}

/// The same with the late `response.done` carrying the first response's id.
#[test]
fn the_usage_of_a_cancelled_responses_late_done_with_its_id_is_counted_once() {
    let server = Server::start(|mut socket| async move {
        accept_session(&mut socket).await;
        send(&mut socket, committed("item_1")).await;
        send(&mut socket, response_created("resp_1")).await;
        send(&mut socket, transcript_delta("resp_1", "Hello")).await;
        let cancel = receive(&mut socket).await;
        assert_eq!(cancel["type"], "response.cancel", "{cancel}");
        send(&mut socket, committed("item_2")).await;
        send(&mut socket, response_created("resp_2")).await;
        send(&mut socket, response_done(Some("resp_1"), "cancelled", 12)).await;
        send(&mut socket, transcript_delta("resp_2", "World")).await;
        send(&mut socket, response_done(Some("resp_2"), "completed", 16)).await;
        while let Some(Ok(_)) = socket.next().await {}
    });
    let mut model = server.model();
    let mut session = model.open(config()).expect("the session opens");
    let mut events = until(&mut session, |event| {
        matches!(event, SessionEvent::OutputTranscriptDelta(_))
    });
    let first = created(&events)[0];
    session.cancel_response(first).expect("in progress");
    events.extend(until(&mut session, |event| {
        matches!(event, SessionEvent::ResponseCompleted(_))
    }));
    events.extend(close(&mut session));
    let ids = created(&events);
    assert_eq!(ids.len(), 2, "{events:#?}");
    assert!(completed(&events, ids[1]), "{events:#?}");
    assert_eq!(
        output_audio(&events),
        (12 + 16, 0),
        "the first response's late usage (12 output audio tokens) is not in the usage records: \
         {events:#?}"
    );
}

/// The caller cancels the first response, and its `response.done` arrives as text that is not
/// JSON. The server then creates and ends the second response, its `response.done` carrying its
/// id, so by the binding's premise the first one's end is behind it. The third response's
/// `response.done` lacks its id: it must end the third response. 457724a keeps the cancelled
/// first response at the head of `awaiting_done` for good (only its own readable done removes
/// it), so every later id-less `response.done` goes to it and the third response never ends.
#[test]
fn a_cancelled_response_whose_done_was_unreadable_does_not_take_a_later_id_less_done() {
    let server = Server::start(|mut socket| async move {
        accept_session(&mut socket).await;
        send(&mut socket, committed("item_1")).await;
        send(&mut socket, response_created("resp_1")).await;
        send(&mut socket, transcript_delta("resp_1", "Hello")).await;
        let cancel = receive(&mut socket).await;
        assert_eq!(cancel["type"], "response.cancel", "{cancel}");
        socket
            .send(Message::text("<html>bad gateway"))
            .await
            .unwrap();
        send(&mut socket, committed("item_2")).await;
        send(&mut socket, response_created("resp_2")).await;
        send(&mut socket, transcript_delta("resp_2", "Two")).await;
        send(&mut socket, response_done(Some("resp_2"), "completed", 16)).await;
        send(&mut socket, committed("item_3")).await;
        send(&mut socket, response_created("resp_3")).await;
        send(&mut socket, transcript_delta("resp_3", "Three")).await;
        send(&mut socket, response_done(None, "completed", 9)).await;
        while let Some(Ok(_)) = socket.next().await {}
    });
    let mut model = server.model();
    let mut session = model.open(config()).expect("the session opens");
    let mut events = until(&mut session, |event| {
        matches!(event, SessionEvent::OutputTranscriptDelta(_))
    });
    let first = created(&events)[0];
    session.cancel_response(first).expect("in progress");
    let mut ends = 0;
    events.extend(until(&mut session, |event| {
        if matches!(event, SessionEvent::ResponseCompleted(_)) {
            ends += 1;
        }
        ends == 2
    }));
    let ids = created(&events);
    assert_eq!(ids.len(), 3, "{events:#?}");
    assert_eq!(transcript(&events, ids[2]), "Three", "{events:#?}");
    assert!(
        completed(&events, ids[2]),
        "the third response's id-less response.done went to the cancelled first response: \
         {events:#?}"
    );
    events.extend(close(&mut session));
    assert_eq!(output_audio(&events).0, 16 + 9, "{events:#?}");
}

/// A response in progress whose `response.done` arrives as text that is not JSON: the next
/// `response.created` ends it (`ended_unseen`), and the next response's `response.done` lacks
/// its id. It must end the next response, not the one `ended_unseen` already failed.
#[test]
fn an_id_less_done_after_an_unseen_end_ends_the_next_response() {
    let server = Server::start(|mut socket| async move {
        accept_session(&mut socket).await;
        send(&mut socket, committed("item_1")).await;
        send(&mut socket, response_created("resp_1")).await;
        send(&mut socket, transcript_delta("resp_1", "Hello")).await;
        socket
            .send(Message::text("<html>bad gateway"))
            .await
            .unwrap();
        send(&mut socket, committed("item_2")).await;
        send(&mut socket, response_created("resp_2")).await;
        send(&mut socket, transcript_delta("resp_2", "World")).await;
        send(&mut socket, response_done(None, "completed", 16)).await;
        while let Some(Ok(_)) = socket.next().await {}
    });
    let mut model = server.model();
    let mut session = model.open(config()).expect("the session opens");
    let mut events = until(&mut session, |event| {
        matches!(event, SessionEvent::ResponseCompleted(_))
    });
    let ids = created(&events);
    assert_eq!(ids.len(), 2, "{events:#?}");
    assert_eq!(transcript(&events, ids[1]), "World", "{events:#?}");
    assert!(completed(&events, ids[1]), "{events:#?}");
    events.extend(close(&mut session));
    assert_eq!(output_audio(&events), (16, 1), "{events:#?}");
}

/// No frame was unreadable: the server creates the second response before it sends the first
/// one's `response.done`, which carries the first response's id, status `completed` and 12
/// output audio tokens. `ended_unseen` fails the first response as a provider error "in a frame
/// the binding did not read" although no frame was unreadable, records it with every count
/// unknown, and the first response's own `response.done` then counts nothing.
#[test]
fn a_done_after_the_next_created_without_unreadable_frames_keeps_its_usage() {
    let server = Server::start(|mut socket| async move {
        accept_session(&mut socket).await;
        send(&mut socket, committed("item_1")).await;
        send(&mut socket, response_created("resp_1")).await;
        send(&mut socket, transcript_delta("resp_1", "Hello")).await;
        send(&mut socket, committed("item_2")).await;
        send(&mut socket, response_created("resp_2")).await;
        send(&mut socket, response_done(Some("resp_1"), "completed", 12)).await;
        send(&mut socket, transcript_delta("resp_2", "World")).await;
        send(&mut socket, response_done(Some("resp_2"), "completed", 16)).await;
        while let Some(Ok(_)) = socket.next().await {}
    });
    let mut model = server.model();
    let mut session = model.open(config()).expect("the session opens");
    let mut events = until(&mut session, |event| {
        matches!(event, SessionEvent::ResponseCompleted(_))
    });
    events.extend(close(&mut session));
    let ids = created(&events);
    assert_eq!(ids.len(), 2, "{events:#?}");
    assert_eq!(session.ledger().unreadable_frames(), 0);
    assert_eq!(
        output_audio(&events),
        (12 + 16, 0),
        "the first response's own usage is not in the usage records: {events:#?}"
    );
}
