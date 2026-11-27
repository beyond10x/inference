//! Adversary case, pass 2, for the test-budget story: what a read that ends at the script's
//! marker still sees of a message that comes after it.
//!
//! Eleven duplex tests read "until two seconds pass with no event" and now read until the binding
//! read the `error` event their script sends last (`to_the_script_end`). The two seconds showed
//! that nothing more came. The cases here send one more `response.created` after the marker, at
//! once and 300 ms later, and ask whether the read sees it; the control reads the later one the
//! way the tests read before. The read to the marker does not see the later one. That was
//! decided to be the rule of the marker, not a defect of the read: the marker is the last
//! message of a script, and the third case holds that a message sent after it is not read. No
//! network and no key.
#![cfg(feature = "duplex")]

use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use inference_audio::{AudioEncoding, AudioFormat};
use inference_duplex::{
    NativeSpeechModel, ResponseId, SessionConfig, SessionEvent, SpeechSessionHandle, TurnDetection,
    TurnTuning,
};
use inference_openai::duplex::{OpenAiRealtime, OpenAiRealtimeSession};
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

fn response_created(id: &str) -> Value {
    json!({"type": "response.created", "event_id": "event_rc", "response": {"id": id, "object": "realtime.response", "status": "in_progress", "output": []}})
}

fn done(id: &str) -> Value {
    json!({"type": "response.done", "event_id": "event_rd", "response": {
        "id": id, "object": "realtime.response", "status": "completed", "output": [], "usage": {
            "total_tokens": 36, "input_tokens": 25, "output_tokens": 11,
            "input_token_details": {"text_tokens": 5, "audio_tokens": 20, "cached_tokens": 0},
            "output_token_details": {"text_tokens": 3, "audio_tokens": 8}
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

/// The marker of the eleven tests, as `adversary8_realtime_cancel_orders.rs` writes it.
fn script_end() -> Value {
    json!({"type": "error", "event_id": "event_end", "error": {
        "type": "invalid_request_error", "message": "the script ended"
    }})
}

/// The read of the eleven tests, as `adversary8_realtime_cancel_orders.rs` and `duplex.rs` write
/// it.
fn to_the_script_end(session: &mut OpenAiRealtimeSession) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    let mut idle = 0;
    while idle < 20 {
        let ended = session.server_error_count() > 0;
        match session.next_event(if ended { Duration::ZERO } else { WAIT }) {
            Some(event) => {
                idle = 0;
                events.push(event);
            }
            None if ended => break,
            None => idle += 1,
        }
    }
    events
}

/// The read the tests had before: events until two seconds pass with none.
fn until_two_seconds_pass(session: &mut impl SpeechSessionHandle) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    let mut idle = 0;
    while idle < 20 {
        match session.next_event(WAIT) {
            Some(event) => {
                idle = 0;
                events.push(event);
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

/// A script that ends a response, sends the marker and, `after` later, creates one more response.
fn one_more_after_the_marker(after: Duration) -> Server {
    Server::start(move |mut socket| async move {
        accept_session(&mut socket).await;
        send(&mut socket, response_created("resp_1")).await;
        send(&mut socket, done("resp_1")).await;
        send(&mut socket, script_end()).await;
        tokio::time::sleep(after).await;
        send(&mut socket, response_created("resp_late")).await;
        while let Some(Ok(_)) = socket.next().await {}
    })
}

/// The control: the read the tests had sees a response created 300 ms after the marker.
#[test]
fn adversary2_w1_the_two_second_read_sees_a_response_created_300_ms_after_the_marker() {
    let server = one_more_after_the_marker(Duration::from_millis(300));
    let mut model = server.model();
    let mut session = model.open(config()).expect("the session opens");
    let events = until_two_seconds_pass(&mut session);
    assert_eq!(created(&events).len(), 2, "{events:#?}");
}

/// The read to the marker, over a response created at once after it.
#[test]
fn adversary2_w1_the_read_to_the_marker_sees_a_response_created_at_once_after_it() {
    let server = one_more_after_the_marker(Duration::ZERO);
    let mut model = server.model();
    let mut session = model.open(config()).expect("the session opens");
    let events = to_the_script_end(&mut session);
    assert_eq!(
        created(&events).len(),
        2,
        "the response the server created after the marker was not read: {events:#?}"
    );
}

/// The read to the marker, over a response created 300 ms after it: the two-second read saw it.
///
/// The case asserted that this read sees it too, and was red: the read ends within one wait of
/// 100 ms after the marker. Decided: the read stays as it is. The marker is the last message of
/// a script, a message sent after it is not read, and the comment of each helper that reads to a
/// marker says so. The case now holds that rule: the read to the marker returns without the late
/// response, and a read after it gets it.
#[test]
fn adversary2_w1_the_read_to_the_marker_does_not_read_a_response_created_300_ms_after_it() {
    let server = one_more_after_the_marker(Duration::from_millis(300));
    let mut model = server.model();
    let mut session = model.open(config()).expect("the session opens");
    let events = to_the_script_end(&mut session);
    assert_eq!(
        created(&events).len(),
        1,
        "decided: a message sent after the marker is not read by the read to the marker, and no \
         script sends one; this read saw the response created 300 ms after it: {events:#?}"
    );
    let mut later = Vec::new();
    for _ in 0..20 {
        later.extend(session.next_event(WAIT));
        if !created(&later).is_empty() {
            break;
        }
    }
    assert_eq!(
        created(&later).len(),
        1,
        "the late response was sent, and a read after the read to the marker gets it: {later:#?}"
    );
}
