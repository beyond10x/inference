//! Adversary case, pass 6, story `duplex-late-usage-counted` at ecf0fb3: which response an
//! id-less `response.done` ends once a response is marked `by_id_only`, and the failure kind of a
//! stall after a frame the binding read only in part.
//!
//! The binding's module doc: "One without its id ends the oldest response still awaiting its
//! `response.done`, never a newer one, except a response the next `response.created` ended after
//! a frame the binding could not read arrived: that frame was likely its `response.done`".
//! The contract (`SpeechSessionHandle::next_event`): "Stalled turns into ProviderError only when
//! an unreadable frame arrived after the last readable frame." No network and no key.
#![cfg(feature = "duplex")]

use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use inference_audio::{AudioEncoding, AudioFormat};
use inference_duplex::{
    NativeSpeechModel, QuantityKind, ResponseId, SessionConfig, SessionEvent, SessionFailureKind,
    SpeechSessionHandle, TurnDetection, TurnTuning,
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

/// An input transcription the server sent without its `transcript`, with its usage in tokens:
/// the binding reads its type and its usage, and counts it as a frame read in part.
fn transcription_without_transcript() -> Value {
    json!({"type": "conversation.item.input_audio_transcription.completed", "event_id": "event_tc", "item_id": "item_1", "content_index": 0,
        "usage": {"type": "tokens", "total_tokens": 9, "input_tokens": 7, "output_tokens": 2}})
}

/// A `response.done` with every usage count, `output_audio` output audio tokens among them;
/// without `id` when `id` is `None`, without `status` when `status` is `None`.
fn response_done(id: Option<&str>, status: Option<&str>, output_audio: u64) -> Value {
    let mut response = json!({
        "object": "realtime.response", "output": [],
        "usage": {
            "total_tokens": 28 + output_audio, "input_tokens": 25, "output_tokens": 3 + output_audio,
            "input_token_details": {"text_tokens": 5, "audio_tokens": 20, "cached_tokens": 0},
            "output_token_details": {"text_tokens": 3, "audio_tokens": output_audio}
        }
    });
    if let Some(id) = id {
        response["id"] = json!(id);
    }
    if let Some(status) = status {
        response["status"] = json!(status);
    }
    json!({"type": "response.done", "event_id": "event_rd", "response": response})
}

fn config(idle_limit: Duration) -> SessionConfig {
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
        idle_limit,
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

/// Round 4's `an_id_less_done_after_an_unseen_end_ends_the_next_response`, with the first
/// response cancelled by the caller instead of ended by the next `response.created`: its
/// `response.done` arrives as text that is not JSON, the server creates the second response, and
/// the second one's `response.done` lacks its id. It must end the second response. ecf0fb3 marks
/// only a response `ended_unseen` ends as `by_id_only`, so the cancelled first response takes
/// the second one's done, and the second response never ends.
#[test]
fn an_id_less_done_after_a_cancelled_responses_unreadable_done_ends_the_next_response() {
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
        send(&mut socket, transcript_delta("resp_2", "World")).await;
        send(&mut socket, response_done(None, Some("completed"), 16)).await;
        while let Some(Ok(_)) = socket.next().await {}
    });
    let mut model = server.model();
    let mut session = model
        .open(config(Duration::from_secs(5)))
        .expect("the session opens");
    let mut events = until(&mut session, |event| {
        matches!(event, SessionEvent::OutputTranscriptDelta(_))
    });
    let first = created(&events)[0];
    session.cancel_response(first).expect("in progress");
    events.extend(until(&mut session, |event| {
        matches!(event, SessionEvent::ResponseCompleted(_))
    }));
    let ids = created(&events);
    assert_eq!(ids.len(), 2, "{events:#?}");
    assert_eq!(transcript(&events, ids[1]), "World", "{events:#?}");
    assert!(
        completed(&events, ids[1]),
        "the second response's id-less response.done went to the cancelled first response, \
         whose own done was the unreadable frame: {events:#?}"
    );
    events.extend(close(&mut session));
    assert_eq!(output_audio(&events), (16, 1), "{events:#?}");
}

/// The frame that arms `by_id_only` is an input transcription the binding read, type and usage,
/// and counted only because it lacks `transcript`: it is no `response.done`. The server creates
/// the second response, then sends the first one's `response.done` without its id (12 output
/// audio tokens), then the second one's with its id (16). The second response has its own done,
/// so the id-less one is the first response's. ecf0fb3 gives it to the second response, which
/// completes before its own transcript, and drops the second one's real usage.
#[test]
fn a_frame_read_as_a_transcription_does_not_make_an_id_less_done_skip_its_response() {
    let server = Server::start(|mut socket| async move {
        accept_session(&mut socket).await;
        send(&mut socket, committed("item_1")).await;
        send(&mut socket, response_created("resp_1")).await;
        send(&mut socket, transcript_delta("resp_1", "Hello")).await;
        send(&mut socket, transcription_without_transcript()).await;
        send(&mut socket, committed("item_2")).await;
        send(&mut socket, response_created("resp_2")).await;
        send(&mut socket, response_done(None, Some("completed"), 12)).await;
        send(&mut socket, transcript_delta("resp_2", "World")).await;
        send(
            &mut socket,
            response_done(Some("resp_2"), Some("completed"), 16),
        )
        .await;
        while let Some(Ok(_)) = socket.next().await {}
    });
    let mut model = server.model();
    let mut session = model
        .open(config(Duration::from_secs(5)))
        .expect("the session opens");
    let mut events = until(
        &mut session,
        |event| matches!(event, SessionEvent::OutputTranscriptDelta(delta) if delta.text == "World"),
    );
    events.extend(until(&mut session, |event| {
        matches!(event, SessionEvent::ResponseCompleted(_))
    }));
    events.extend(close(&mut session));
    let ids = created(&events);
    assert_eq!(ids.len(), 2, "{events:#?}");
    assert_eq!(
        output_audio(&events),
        (12 + 16, 0),
        "the first response's id-less done went to the second response, and the second one's \
         own usage (16 output audio tokens) was dropped: {events:#?}"
    );
    assert_eq!(transcript(&events, ids[1]), "World", "{events:#?}");
}

/// Every event until the session's failure, and its failure kind.
fn until_failed(session: &mut impl SpeechSessionHandle) -> (Vec<SessionEvent>, SessionFailureKind) {
    let events = until(session, |event| {
        matches!(event, SessionEvent::SpeechSessionFailed(_))
    });
    let kind = match events.last() {
        Some(SessionEvent::SpeechSessionFailed(failed)) => failed.failure.kind,
        _ => panic!("the session did not fail: {events:#?}"),
    };
    (events, kind)
}

/// The server's turn detection commits the input, so the caller awaits a response; the last
/// frame is an input transcription without its `transcript`, which the binding counts as a frame
/// it could not read (`unreadable_frames` 1), and the server then sends nothing. By the
/// contract an unreadable frame arrived after the last readable one, so the stall is a
/// provider error, as it is after a `response.done` without `status` (the companion case).
/// ecf0fb3 clears the state again inside the same frame (`transcription_usage_reported` calls
/// `heard` after `unread`), and the stall stays a stall.
#[test]
fn a_stall_after_a_transcription_read_in_part_is_a_provider_error() {
    let server = Server::start(|mut socket| async move {
        accept_session(&mut socket).await;
        send(&mut socket, committed("item_1")).await;
        send(&mut socket, transcription_without_transcript()).await;
        while let Some(Ok(_)) = socket.next().await {}
    });
    let mut model = server.model();
    let mut session = model
        .open(config(Duration::from_millis(400)))
        .expect("the session opens");
    let (events, kind) = until_failed(&mut session);
    assert_eq!(session.ledger().unreadable_frames(), 1, "{events:#?}");
    assert_eq!(
        kind,
        SessionFailureKind::ProviderError,
        "the last frame was counted unreadable, yet the stall stayed a stall: {events:#?}"
    );
}

/// The companion: a `response.done` without `status`, also read in part and counted, followed
/// by silence while the caller still awaits the server. ecf0fb3 reports a provider error here.
#[test]
fn a_stall_after_a_done_read_in_part_is_a_provider_error() {
    let server = Server::start(|mut socket| async move {
        accept_session(&mut socket).await;
        send(&mut socket, committed("item_1")).await;
        send(&mut socket, response_created("resp_1")).await;
        send(&mut socket, response_done(Some("resp_1"), None, 12)).await;
        while let Some(Ok(_)) = socket.next().await {}
    });
    let mut model = server.model();
    let mut session = model
        .open(config(Duration::from_millis(400)))
        .expect("the session opens");
    let (events, kind) = until_failed(&mut session);
    assert_eq!(session.ledger().unreadable_frames(), 1, "{events:#?}");
    assert_eq!(kind, SessionFailureKind::ProviderError, "{events:#?}");
}

/// A guard for `unreadable_done`'s `retain`, which no existing case kills: the first response's
/// `response.done` arrives truncated, not JSON, its `type` still readable, so
/// `ServerEvent::UnreadableResponseDone` ends it. The second response's `response.done` lacks
/// its id and must end the second response, with its 16 output audio tokens. Without the
/// `retain` the first response stays at the head of `awaiting_done` and takes that done.
#[test]
fn an_id_less_done_after_an_unreadable_typed_done_ends_the_next_response() {
    let server = Server::start(|mut socket| async move {
        accept_session(&mut socket).await;
        send(&mut socket, committed("item_1")).await;
        send(&mut socket, response_created("resp_1")).await;
        send(&mut socket, transcript_delta("resp_1", "Hello")).await;
        socket
            .send(Message::text(
                "{\"type\": \"response.done\", \"event_id\": \"event_rd\", \"response\": {\"id\": \"resp_1\", \"stat",
            ))
            .await
            .unwrap();
        send(&mut socket, committed("item_2")).await;
        send(&mut socket, response_created("resp_2")).await;
        send(&mut socket, transcript_delta("resp_2", "World")).await;
        send(&mut socket, response_done(None, Some("completed"), 16)).await;
        while let Some(Ok(_)) = socket.next().await {}
    });
    let mut model = server.model();
    let mut session = model
        .open(config(Duration::from_secs(5)))
        .expect("the session opens");
    let mut events = until(&mut session, |event| {
        matches!(event, SessionEvent::ResponseCompleted(_))
    });
    let ids = created(&events);
    assert_eq!(ids.len(), 2, "{events:#?}");
    assert_eq!(transcript(&events, ids[1]), "World", "{events:#?}");
    assert!(
        completed(&events, ids[1]),
        "the id-less response.done went to the first response, already ended: {events:#?}"
    );
    events.extend(close(&mut session));
    assert_eq!(output_audio(&events), (16, 1), "{events:#?}");
}
