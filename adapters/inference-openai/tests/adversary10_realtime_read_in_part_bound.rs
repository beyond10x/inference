//! Adversary case, pass 10, story `duplex-late-usage-counted` at b873a7e: a Realtime frame the
//! binding read only in part leaves the liveness bound where it stood before the frame
//! (`duplex.rs`, `unread`: "leaves the liveness bound where it stood at `mark`, before the
//! frame"). The pass-6 cases pin only the failure kind after such a frame, not when the session
//! fails, so a binding that lets each such frame restart the bound passes them. Here the server
//! commits the input, so the caller awaits a response, then sends only frames read in part, one
//! every 150 ms for 3 s, against a 500 ms bound. Nothing readable arrives after the commit, so
//! the session fails within about 500 ms of it, not after the frames stop. No network and no key.
#![cfg(feature = "duplex")]

use std::num::{NonZeroU16, NonZeroU32};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use inference_audio::{AudioEncoding, AudioFormat};
use inference_duplex::{
    NativeSpeechModel, SessionConfig, SessionEvent, SessionFailureKind, SpeechSessionHandle,
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
const WAIT: Duration = Duration::from_millis(50);

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

async fn send(socket: &mut Socket, event: Value) -> bool {
    socket.send(Message::text(event.to_string())).await.is_ok()
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

fn committed(item: &str) -> Value {
    json!({"type": "input_audio_buffer.committed", "event_id": "event_cm", "previous_item_id": null, "item_id": item})
}

/// The server sends `lead`, then `partial` every 150 ms for 3 s, against a 500 ms bound; returns
/// the time from opening to the session's failure, and the failure kind.
fn failure_after(lead: Vec<Value>, partial: Value) -> (Duration, SessionFailureKind, u64) {
    let server = Server::start(move |mut socket| async move {
        accept_session(&mut socket).await;
        for event in lead {
            send(&mut socket, event).await;
        }
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(150)).await;
            if !send(&mut socket, partial.clone()).await {
                return;
            }
        }
        while let Some(Ok(_)) = socket.next().await {}
    });
    let mut model = server.model();
    let opened = Instant::now();
    let mut session = model
        .open(config(Duration::from_millis(500)))
        .expect("the session opens");
    let mut events = Vec::new();
    loop {
        assert!(
            opened.elapsed() < Duration::from_secs(8),
            "the session did not fail: {events:#?}"
        );
        if let Some(event) = session.next_event(WAIT) {
            if let SessionEvent::SpeechSessionFailed(failed) = &event {
                return (
                    opened.elapsed(),
                    failed.failure.kind,
                    session.ledger().unreadable_frames(),
                );
            }
            events.push(event);
        }
    }
}

/// Input transcriptions without their `transcript`, after a commit: `unread` at the end of the
/// transcription arm.
#[test]
fn transcriptions_read_in_part_do_not_refresh_the_liveness_bound() {
    let partial = json!({"type": "conversation.item.input_audio_transcription.completed", "event_id": "event_tc", "item_id": "item_1", "content_index": 0,
        "usage": {"type": "tokens", "total_tokens": 9, "input_tokens": 7, "output_tokens": 2}});
    let (after, kind, counted) = failure_after(vec![committed("item_1")], partial);
    assert!(
        after < Duration::from_millis(1_600),
        "frames the binding read only in part kept the session alive for {after:?} after it \
         opened, against a 500 ms bound; {counted} were counted"
    );
    assert_eq!(kind, SessionFailureKind::ProviderError);
}

/// A `response.done` without `status` for a response already completed, after the next commit:
/// the ledger refuses to end it again, and the binding counts the frame through `read_in_part`.
#[test]
fn a_refused_done_without_status_does_not_refresh_the_liveness_bound() {
    let usage = json!({"total_tokens": 28, "input_tokens": 25, "output_tokens": 3});
    let done = |status: Option<&str>| {
        let mut response =
            json!({"id": "resp_1", "object": "realtime.response", "output": [], "usage": usage});
        if let Some(status) = status {
            response["status"] = json!(status);
        }
        json!({"type": "response.done", "event_id": "event_rd", "response": response})
    };
    let lead = vec![
        committed("item_1"),
        json!({"type": "response.created", "event_id": "event_rc", "response": {"id": "resp_1", "object": "realtime.response", "status": "in_progress", "output": []}}),
        done(Some("completed")),
        committed("item_2"),
    ];
    let (after, kind, counted) = failure_after(lead, done(None));
    assert!(
        after < Duration::from_millis(1_600),
        "refused status-less `response.done` frames kept the session alive for {after:?} after \
         it opened, against a 500 ms bound; {counted} were counted"
    );
    assert_eq!(kind, SessionFailureKind::ProviderError);
}
