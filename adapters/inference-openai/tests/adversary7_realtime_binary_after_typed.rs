//! Adversary case, pass 7, story `duplex-late-usage-counted` at 4cf1cf1: a binary message that
//! follows a JSON frame with a readable `type` the binding could not parse.
//!
//! `RealtimeClient::read_event` clears `unparsed_type` before each read, so a binary message
//! (no `type` at all, as likely a lost `response.done` as any other) counts as a message without
//! a readable `type` even when the frame before it had one. This case pins that reset: without
//! it, the binary message would inherit the previous frame's `type`, the first response would
//! not be marked, and the second response's id-less `response.done` would end the first. Green
//! at 4cf1cf1; it kills mutant M7e. No network and no key.
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

/// A `response.done` without its id, with every usage count, `output_audio` output audio tokens
/// among them.
fn id_less_done(output_audio: u64) -> Value {
    json!({"type": "response.done", "event_id": "event_rd", "response": {
        "object": "realtime.response", "status": "completed", "output": [],
        "usage": {
            "total_tokens": 28 + output_audio, "input_tokens": 25, "output_tokens": 3 + output_audio,
            "input_token_details": {"text_tokens": 5, "audio_tokens": 20, "cached_tokens": 0},
            "output_token_details": {"text_tokens": 3, "audio_tokens": output_audio}
        }
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

/// A transcription delta without its `item_id` (a readable `type`, so not a lost
/// `response.done`) and then a binary message (no readable `type`, the first response's lost
/// `response.done`) arrive while the first response is in progress. The second response's
/// id-less `response.done` must end the second response.
#[test]
fn a_binary_message_after_a_typed_unparsed_frame_still_marks_the_response_in_progress() {
    let server = Server::start(|mut socket| async move {
        accept_session(&mut socket).await;
        send(&mut socket, committed("item_1")).await;
        send(&mut socket, response_created("resp_1")).await;
        send(&mut socket, transcript_delta("resp_1", "Hello")).await;
        send(
            &mut socket,
            json!({"type": "conversation.item.input_audio_transcription.delta", "event_id": "event_x", "content_index": 0, "delta": "hi"}),
        )
        .await;
        socket
            .send(Message::binary(vec![0x7b_u8, 0x22, 0x74]))
            .await
            .unwrap();
        send(&mut socket, committed("item_2")).await;
        send(&mut socket, response_created("resp_2")).await;
        send(&mut socket, transcript_delta("resp_2", "World")).await;
        send(&mut socket, id_less_done(16)).await;
        while let Some(Ok(_)) = socket.next().await {}
    });
    let mut model = server.model();
    let mut session = model.open(config()).expect("the session opens");
    let mut events = until(&mut session, |event| {
        matches!(event, SessionEvent::ResponseCompleted(_))
    });
    assert_eq!(session.ledger().unreadable_frames(), 2, "{events:#?}");
    let ids = created(&events);
    assert_eq!(ids.len(), 2, "{events:#?}");
    assert_eq!(transcript(&events, ids[1]), "World", "{events:#?}");
    assert!(
        completed(&events, ids[1]),
        "the binary message did not mark the first response, so the second response's id-less \
         response.done ended the first: {events:#?}"
    );
    events.extend(close(&mut session));
    assert_eq!(output_audio(&events), (16, 1), "{events:#?}");
}
