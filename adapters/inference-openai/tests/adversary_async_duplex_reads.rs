//! Adversary, story:async-speech-handles: the async form of the Realtime and GPT-Live duplex
//! sessions, which the story's scope names and the commit leaves on the trait's default. The
//! default takes `next_event(Duration::ZERO)`, and both bindings return at once on a zero wait
//! without reading their connection, so a session awaited through `event()` never sees what the
//! server sent. Each binding gets one replay, run twice: through the blocking `next_event`, the
//! control, and through `event()` on a current-thread runtime, bounded well below the session's
//! liveness bound.

#![cfg(feature = "duplex")]

use std::num::{NonZeroU16, NonZeroU32};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use inference_audio::{AudioEncoding, AudioFormat};
use inference_duplex::{
    NativeSpeechModel, SessionConfig, SessionEvent, SpeechSessionHandle, TurnDetection, TurnTuning,
};
use inference_openai::{Credential, CredentialError, CredentialFuture, CredentialSource};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

const CREDENTIAL: &str = "offline-test-credential";
/// How long either form may take to deliver the response the server already sent.
const BOUND: Duration = Duration::from_secs(2);

struct FixedSource;

impl CredentialSource for FixedSource {
    fn resolve<'a>(&'a self, name: &'a str) -> CredentialFuture<'a> {
        let found = name == inference_openai::OPENAI_KEY_NAME;
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

/// A local server running one script on its own runtime; `address` is where it listens.
struct Server {
    address: std::net::SocketAddr,
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
        Self {
            address,
            runtime,
            task,
        }
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

fn pcm24() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(24_000).unwrap(),
        channels: NonZeroU16::MIN,
    }
}

fn config(turn_detection: TurnDetection, auto: bool) -> SessionConfig {
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
        idle_limit: Duration::from_secs(5),
    }
}

/// An event's variant name, without its payload.
fn kind(event: &SessionEvent) -> String {
    let debug = format!("{event:?}");
    debug
        .split(['(', ' ', '{'])
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// The kinds of the events delivered until a response completed or [`BOUND`] passed, and
/// whether one completed: through `event()` on a current-thread runtime when `through_async`,
/// otherwise through the blocking `next_event`.
fn until_completed(
    session: &mut impl SpeechSessionHandle,
    through_async: bool,
) -> (Vec<String>, bool) {
    let mut kinds = Vec::new();
    if !through_async {
        let deadline = Instant::now() + BOUND;
        while Instant::now() < deadline {
            if let Some(event) = session.next_event(Duration::from_millis(100)) {
                kinds.push(kind(&event));
                if matches!(event, SessionEvent::ResponseCompleted(_)) {
                    return (kinds, true);
                }
            }
        }
        return (kinds, false);
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let completed = runtime.block_on(async {
        tokio::time::timeout(BOUND, async {
            while let Some(event) = session.event().await {
                kinds.push(kind(&event));
                if matches!(event, SessionEvent::ResponseCompleted(_)) {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false)
    });
    (kinds, completed)
}

/// A Realtime session asked for a response, against a replay that answers with a whole
/// response.
fn realtime(through_async: bool) -> (Vec<String>, bool) {
    let server = Server::start(|mut socket| async move {
        send(
            &mut socket,
            json!({"type": "session.created", "event_id": "event_1", "session": {"type": "realtime", "id": "sess_1"}}),
        )
        .await;
        let update = receive(&mut socket).await;
        assert_eq!(update["type"], "session.update", "{update}");
        let mut confirmed = update["session"].clone();
        confirmed["id"] = json!("sess_1");
        send(
            &mut socket,
            json!({"type": "session.updated", "event_id": "event_2", "session": confirmed}),
        )
        .await;
        assert_eq!(
            receive(&mut socket).await,
            json!({"type": "response.create"})
        );
        send(
            &mut socket,
            json!({"type": "response.created", "event_id": "event_rc", "response": {"id": "resp_1", "object": "realtime.response", "status": "in_progress", "output": []}}),
        )
        .await;
        send(
            &mut socket,
            json!({"type": "response.output_audio_transcript.delta", "event_id": "event_td", "response_id": "resp_1", "item_id": "item_out", "output_index": 0, "content_index": 0, "delta": "It is noon"}),
        )
        .await;
        send(
            &mut socket,
            json!({"type": "response.done", "event_id": "event_rd", "response": {"id": "resp_1", "object": "realtime.response", "status": "completed", "output": []}}),
        )
        .await;
        let _ = socket.next().await;
    });
    let endpoint = inference_openai::Endpoint::new("gpt-realtime-2.1")
        .with_url(format!("ws://{}/v1/realtime", server.address))
        .with_read_timeout(Duration::from_secs(5));
    let mut model = inference_openai::duplex::OpenAiRealtime::new(
        endpoint,
        Box::new(FixedSource),
        inference_openai::OPENAI_KEY_NAME,
    );
    let mut session = model
        .open(config(TurnDetection::Disabled, false))
        .expect("the session opens");
    session.create_response().expect("none in progress");
    let outcome = until_completed(&mut session, through_async);
    session.close().expect("open");
    server.finish();
    outcome
}

#[test]
fn control_the_realtime_replay_completes_through_the_blocking_form() {
    let (kinds, completed) = realtime(false);
    assert!(completed, "{kinds:?}");
}

#[test]
fn the_realtime_session_delivers_the_servers_response_through_the_async_form() {
    let (kinds, completed) = realtime(true);
    assert!(
        completed,
        "the server sent response.created, a delta and response.done; the async form \
         delivered within {BOUND:?} only {kinds:?}"
    );
}

/// A GPT-Live session given three frames, against a replay that speaks one voiced frame and
/// then a silence gap, which completes the response.
#[cfg(feature = "gpt-live")]
fn gpt_live(through_async: bool) -> (Vec<String>, bool) {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use inference_audio::{AudioFrame, SampleClockPosition};

    const FRAME: usize = 2_400;
    let pcm = |level: i16| -> Vec<u8> {
        level
            .to_le_bytes()
            .iter()
            .copied()
            .cycle()
            .take(FRAME * 2)
            .collect()
    };
    let silent = pcm(12);
    let voiced = pcm(3_000);
    let server = Server::start(move |mut socket| async move {
        let start = receive(&mut socket).await;
        assert_eq!(start["type"], "session.start", "{start}");
        let mut resolved = start["session"].clone();
        resolved["id"] = json!("live_123");
        resolved["status"] = json!("active");
        send(
            &mut socket,
            json!({"type": "session.started", "event_id": "event_started_1", "session": resolved}),
        )
        .await;
        for _ in 0..3 {
            let append = receive(&mut socket).await;
            assert_eq!(append["type"], "session.input_audio.append", "{append}");
        }
        let audio = |bytes: &[u8]| json!({"type": "session.output_audio.delta", "delta": STANDARD.encode(bytes)});
        send(
            &mut socket,
            json!({"type": "session.output_transcript.delta", "event_id": "evt_out", "delta": " Paris.", "start_ms": 0, "end_ms": 200}),
        )
        .await;
        send(&mut socket, audio(&voiced)).await;
        for _ in 0..10 {
            send(&mut socket, audio(&silent)).await;
        }
        let _ = socket.next().await;
    });
    let endpoint = inference_openai::Endpoint::live(inference_openai::live::LIVE_MODEL)
        .with_url(format!("ws://{}/v1/live/sessions", server.address))
        .with_read_timeout(Duration::from_secs(5));
    let mut model = inference_openai::live_duplex::OpenAiLive::new(
        endpoint,
        Box::new(FixedSource),
        inference_openai::OPENAI_KEY_NAME,
    );
    let mut session = model
        .open(config(TurnDetection::SemanticVad, true))
        .expect("the session opens");
    for at in 0..3_u64 {
        session
            .append_audio(AudioFrame {
                format: pcm24(),
                position: SampleClockPosition::new(at * FRAME as u64).unwrap(),
                sample_count: u32::try_from(FRAME).unwrap(),
                data: vec![0; FRAME * 2],
            })
            .expect("PCM16 at 24 kHz");
    }
    let outcome = until_completed(&mut session, through_async);
    session.close().expect("open");
    server.finish();
    outcome
}

#[cfg(feature = "gpt-live")]
#[test]
fn control_the_gpt_live_replay_completes_through_the_blocking_form() {
    let (kinds, completed) = gpt_live(false);
    assert!(completed, "{kinds:?}");
}

#[cfg(feature = "gpt-live")]
#[test]
fn the_gpt_live_session_delivers_the_servers_response_through_the_async_form() {
    let (kinds, completed) = gpt_live(true);
    assert!(
        completed,
        "the server sent a voiced frame and a silence gap; the async form delivered within \
         {BOUND:?} only {kinds:?}"
    );
}
