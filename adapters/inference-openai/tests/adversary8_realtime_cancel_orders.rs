//! Adversary case, pass 8, story `duplex-late-usage-counted` at e9a4d4b: which response an
//! id-less `response.done` ends after a caller's cancel, in the orders the round-6 fix did not
//! pin.
//!
//! The binding's module doc: "One without its id ends the oldest response still awaiting its
//! `response.done`, never a newer one, except a response after whose creation a message without
//! a readable `type` arrived before the next `response.created`, which ended it or followed the
//! caller's cancel of it". Each case below builds an order in which that rule names the
//! cancelled response as the one the id-less `response.done` ends, and so holds one line of
//! `cancelled_before_created` and `Awaiting::undecodable_at_creation`. No network and no key.
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

async fn unreadable(socket: &mut Socket) {
    socket
        .send(Message::text("<html>bad gateway"))
        .await
        .unwrap();
}

async fn await_cancel(socket: &mut Socket) {
    let cancel = receive(socket).await;
    assert_eq!(cancel["type"], "response.cancel", "{cancel}");
}

fn response_created(id: &str) -> Value {
    json!({"type": "response.created", "event_id": "event_rc", "response": {"id": id, "object": "realtime.response", "status": "in_progress", "output": []}})
}

fn transcript_delta(response: &str, delta: &str) -> Value {
    json!({"type": "response.output_audio_transcript.delta", "event_id": "event_td", "response_id": response, "item_id": "item_o", "output_index": 0, "content_index": 0, "delta": delta})
}

fn usage(output_audio: u64) -> Value {
    json!({
        "total_tokens": 28 + output_audio, "input_tokens": 25, "output_tokens": 3 + output_audio,
        "input_token_details": {"text_tokens": 5, "audio_tokens": 20, "cached_tokens": 0},
        "output_token_details": {"text_tokens": 3, "audio_tokens": output_audio}
    })
}

/// A `response.done` without its id, `output_audio` output audio tokens in its usage.
fn id_less_done(output_audio: u64) -> Value {
    json!({"type": "response.done", "event_id": "event_rd", "response": {
        "object": "realtime.response", "status": "completed", "output": [], "usage": usage(output_audio)
    }})
}

/// A `response.done` with the id `id`, `output_audio` output audio tokens in its usage.
fn done(id: &str, output_audio: u64) -> Value {
    json!({"type": "response.done", "event_id": "event_rd", "response": {
        "id": id, "object": "realtime.response", "status": "completed", "output": [], "usage": usage(output_audio)
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

fn created(events: &[SessionEvent]) -> Vec<ResponseId> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ResponseCreated(created) => Some(created.response_id),
            _ => None,
        })
        .collect()
}

/// The output audio tokens of each usage record of `response`: `Some(n)` known, `None` unknown.
fn output_audio_of(events: &[SessionEvent], response: ResponseId) -> Vec<Option<i64>> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::UsageRecorded(record) if record.response_id == Some(response) => {
                Some(record)
            }
            _ => None,
        })
        .flat_map(|record| record.quantities.iter())
        .filter(|quantity| quantity.kind() == QuantityKind::OutputAudioTokens)
        .map(|quantity| quantity.value())
        .collect()
}

/// Runs `script`; the caller cancels the first response once its transcript delta arrived,
/// then the session reads until two seconds pass with nothing and closes.
fn cancel_first<F, Fut>(script: F) -> (Vec<SessionEvent>, u64)
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
    let first = created(&events)[0];
    session.cancel_response(first).expect("in progress");
    events.extend(until(&mut session, |_| false));
    session.close().expect("open");
    events.extend(until(&mut session, |event| {
        matches!(event, SessionEvent::SpeechSessionClosed(_))
    }));
    let unreadable = session.ledger().unreadable_frames();
    (events, unreadable)
}

/// The caller cancels the first response; no message without a readable `type` follows before
/// the second `response.created`, so the first stays a response an id-less `response.done` ends.
/// One then arrives while the second is in progress, and the third `response.created` ends the
/// second unseen. The rule marks only the second: the message did not arrive before the
/// `response.created` that followed the first one's cancel. So the late id-less done ends the
/// first, not the third. A cancel mark that outlives the `response.created` it waits for (the
/// one-shot `cancelled` flag not cleared) marks the first at the third `response.created`.
#[test]
fn a_cancel_is_judged_once_at_the_next_response_created() {
    let (events, unreadable) = cancel_first(|mut socket| async move {
        accept_session(&mut socket).await;
        send(&mut socket, response_created("resp_1")).await;
        send(&mut socket, transcript_delta("resp_1", "Hello")).await;
        await_cancel(&mut socket).await;
        send(&mut socket, response_created("resp_2")).await;
        send(&mut socket, transcript_delta("resp_2", "World")).await;
        unreadable(&mut socket).await;
        send(&mut socket, response_created("resp_3")).await;
        send(&mut socket, transcript_delta("resp_3", "Again")).await;
        send(&mut socket, id_less_done(16)).await;
        send(&mut socket, done("resp_3", 8)).await;
        while let Some(Ok(_)) = socket.next().await {}
    });
    let ids = created(&events);
    assert_eq!(ids.len(), 3, "{events:#?}");
    assert_eq!(unreadable, 1, "{events:#?}");
    assert_eq!(
        output_audio_of(&events, ids[0]),
        vec![Some(16)],
        "the id-less done did not end the cancelled first response: {events:#?}"
    );
    assert_eq!(output_audio_of(&events, ids[1]), vec![None], "{events:#?}");
    assert_eq!(
        output_audio_of(&events, ids[2]),
        vec![Some(8)],
        "{events:#?}"
    );
}

/// The caller cancels the first response and no message without a readable `type` arrives at
/// all. The first response's own id-less `response.done` comes after the second
/// `response.created`: it ends the first, the oldest awaiting one, and the second ends by its
/// own done. Marking every cancelled response (`>=` for `>`) sends the first one's usage to the
/// second.
#[test]
fn a_cancel_without_an_unreadable_message_keeps_the_id_less_done() {
    let (events, unreadable) = cancel_first(|mut socket| async move {
        accept_session(&mut socket).await;
        send(&mut socket, response_created("resp_1")).await;
        send(&mut socket, transcript_delta("resp_1", "Hello")).await;
        await_cancel(&mut socket).await;
        send(&mut socket, response_created("resp_2")).await;
        send(&mut socket, transcript_delta("resp_2", "World")).await;
        send(&mut socket, id_less_done(16)).await;
        send(&mut socket, done("resp_2", 8)).await;
        while let Some(Ok(_)) = socket.next().await {}
    });
    let ids = created(&events);
    assert_eq!(ids.len(), 2, "{events:#?}");
    assert_eq!(unreadable, 0, "{events:#?}");
    assert_eq!(
        output_audio_of(&events, ids[0]),
        vec![Some(16)],
        "{events:#?}"
    );
    assert_eq!(
        output_audio_of(&events, ids[1]),
        vec![Some(8)],
        "{events:#?}"
    );
}

/// A message without a readable `type` arrives before the first response is created; the
/// caller then cancels the first, and the second is created. The message came before the first
/// one's creation, so it was not its `response.done`: the first one's late id-less done still
/// ends it. Counting from session start instead of from creation marks it.
#[test]
fn an_unreadable_message_before_creation_does_not_mark_a_cancelled_response() {
    let (events, unreadable) = cancel_first(|mut socket| async move {
        accept_session(&mut socket).await;
        unreadable(&mut socket).await;
        send(&mut socket, response_created("resp_1")).await;
        send(&mut socket, transcript_delta("resp_1", "Hello")).await;
        await_cancel(&mut socket).await;
        send(&mut socket, response_created("resp_2")).await;
        send(&mut socket, transcript_delta("resp_2", "World")).await;
        send(&mut socket, id_less_done(16)).await;
        send(&mut socket, done("resp_2", 8)).await;
        while let Some(Ok(_)) = socket.next().await {}
    });
    let ids = created(&events);
    assert_eq!(ids.len(), 2, "{events:#?}");
    assert_eq!(unreadable, 1, "{events:#?}");
    assert_eq!(
        output_audio_of(&events, ids[0]),
        vec![Some(16)],
        "{events:#?}"
    );
    assert_eq!(
        output_audio_of(&events, ids[1]),
        vec![Some(8)],
        "{events:#?}"
    );
}

/// Two cancelled responses: the first is cancelled with no unreadable message, the second is
/// cancelled and a message without a readable `type` follows it. At the third
/// `response.created` only the second is marked; the late id-less done ends the first, and the
/// second ends by its id. Every token is counted once.
#[test]
fn of_two_cancelled_responses_only_the_one_an_unreadable_message_followed_is_marked() {
    let server = Server::start(|mut socket| async move {
        accept_session(&mut socket).await;
        send(&mut socket, response_created("resp_1")).await;
        send(&mut socket, transcript_delta("resp_1", "Hello")).await;
        await_cancel(&mut socket).await;
        send(&mut socket, response_created("resp_2")).await;
        send(&mut socket, transcript_delta("resp_2", "World")).await;
        await_cancel(&mut socket).await;
        unreadable(&mut socket).await;
        send(&mut socket, response_created("resp_3")).await;
        send(&mut socket, transcript_delta("resp_3", "Again")).await;
        send(&mut socket, id_less_done(16)).await;
        send(&mut socket, done("resp_2", 4)).await;
        send(&mut socket, done("resp_3", 8)).await;
        while let Some(Ok(_)) = socket.next().await {}
    });
    let mut model = server.model();
    let mut session = model.open(config()).expect("the session opens");
    let mut events = until(&mut session, |event| {
        matches!(event, SessionEvent::OutputTranscriptDelta(_))
    });
    session
        .cancel_response(created(&events)[0])
        .expect("in progress");
    events.extend(until(&mut session, |event| {
        matches!(event, SessionEvent::OutputTranscriptDelta(delta) if delta.text == "World")
    }));
    session
        .cancel_response(created(&events)[1])
        .expect("in progress");
    events.extend(until(&mut session, |_| false));
    session.close().expect("open");
    events.extend(until(&mut session, |event| {
        matches!(event, SessionEvent::SpeechSessionClosed(_))
    }));
    let ids = created(&events);
    assert_eq!(ids.len(), 3, "{events:#?}");
    assert_eq!(session.ledger().unreadable_frames(), 1, "{events:#?}");
    assert_eq!(
        output_audio_of(&events, ids[0]),
        vec![Some(16)],
        "{events:#?}"
    );
    assert_eq!(
        output_audio_of(&events, ids[1]),
        vec![Some(4)],
        "{events:#?}"
    );
    assert_eq!(
        output_audio_of(&events, ids[2]),
        vec![Some(8)],
        "{events:#?}"
    );
}
