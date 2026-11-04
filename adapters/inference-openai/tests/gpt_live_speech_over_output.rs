//! The GPT-Live binding reports the user speaking over a response from the timing of the input
//! transcript, offline: a local WebSocket server replays the provider.
//!
//! Both transcripts carry `start_ms` and `end_ms` on the one session timeline
//! (`src/live.rs`, the module comment). An input fragment starting inside the interval the
//! response in progress has spoken, from its first output fragment's start to its latest
//! fragment's end, is speech over it: the binding reports `SpeechStarted` there, once per
//! response, before the fragment's `InputTranscribed`. A fragment starting before that interval
//! (the tail of the question the response answers) or after it (in the response's trailing
//! silence) reports no speech.

#![cfg(all(feature = "gpt-live", feature = "duplex"))]

use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::{SinkExt, StreamExt};
use inference_audio::{AudioEncoding, AudioFormat, SampleClockPosition};
use inference_duplex::{
    NativeSpeechModel, SessionConfig, SessionEvent, SpeechSessionHandle, TurnDetection, TurnTuning,
};
use inference_openai::live::LIVE_MODEL;
use inference_openai::live_duplex::OpenAiLive;
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

/// The samples of one 100 ms frame at 24 kHz.
const FRAME: usize = 2_400;

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

/// A local server sending `events` after the start, then waiting for the binding to go.
struct Server {
    endpoint: Endpoint,
    runtime: Runtime,
    task: JoinHandle<()>,
}

impl Server {
    fn replay(events: Vec<Value>) -> Self {
        let runtime = Runtime::new().unwrap();
        let listener = runtime.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
        let address = listener.local_addr().unwrap();
        let task = runtime.spawn(async move {
            let (stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                .await
                .expect("the binding did not connect within five seconds")
                .unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            accept_start(&mut socket).await;
            for event in events {
                socket.send(Message::text(event.to_string())).await.unwrap();
            }
            let _ = socket.next().await;
        });
        let endpoint = Endpoint::live(LIVE_MODEL)
            .with_url(format!("ws://{address}/v1/live/sessions"))
            .with_read_timeout(Duration::from_secs(5));
        Self {
            endpoint,
            runtime,
            task,
        }
    }

    /// The binding's events until the first response completed, then the close.
    fn events(self) -> Vec<SessionEvent> {
        let mut model = OpenAiLive::new(
            self.endpoint.clone(),
            Box::new(FixedSource),
            OPENAI_KEY_NAME,
        );
        let mut session = model.open(config()).expect("the session opens");
        let mut events = Vec::new();
        let mut idle = 0;
        while idle < 20 {
            match session.next_event(WAIT) {
                Some(event) => {
                    idle = 0;
                    let done = matches!(event, SessionEvent::ResponseCompleted(_));
                    events.push(event);
                    if done {
                        break;
                    }
                }
                None => idle += 1,
            }
        }
        session.close().expect("open");
        self.runtime.block_on(self.task).unwrap();
        events
    }
}

async fn accept_start(socket: &mut Socket) {
    let start = loop {
        match socket.next().await.unwrap().unwrap() {
            Message::Text(text) => break serde_json::from_str::<Value>(text.as_str()).unwrap(),
            Message::Ping(_) | Message::Pong(_) => {}
            other => panic!("expected a text frame, got {other:?}"),
        }
    };
    assert_eq!(start["type"], "session.start", "{start}");
    let mut resolved = start["session"].clone();
    resolved["id"] = json!("live_123");
    resolved["status"] = json!("active");
    let started =
        json!({"type": "session.started", "event_id": "evt_started", "session": resolved});
    socket
        .send(Message::text(started.to_string()))
        .await
        .unwrap();
}

fn pcm24() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(24_000).unwrap(),
        channels: NonZeroU16::MIN,
    }
}

fn config() -> SessionConfig {
    SessionConfig {
        voice: "marin".to_owned(),
        instructions: "answer briefly".to_owned(),
        input_format: pcm24(),
        output_format: pcm24(),
        turn_detection: TurnDetection::SemanticVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools: Vec::new(),
        idle_limit: Duration::from_secs(5),
    }
}

fn audio(level: i16) -> Value {
    let bytes: Vec<u8> = level
        .to_le_bytes()
        .into_iter()
        .cycle()
        .take(FRAME * 2)
        .collect();
    json!({"type": "session.output_audio.delta", "delta": STANDARD.encode(bytes)})
}

fn voiced() -> Value {
    audio(3_000)
}

fn silent() -> Value {
    audio(12)
}

/// `session.output_transcript.delta` over `[start_ms, end_ms)`.
fn said(delta: &str, start_ms: u64, end_ms: u64) -> Value {
    json!({"type": "session.output_transcript.delta", "event_id": "evt_out", "delta": delta, "start_ms": start_ms, "end_ms": end_ms})
}

/// `session.input_transcript.delta` over `[start_ms, end_ms)`.
fn heard(delta: &str, start_ms: u64, end_ms: u64) -> Value {
    json!({"type": "session.input_transcript.delta", "event_id": "evt_in", "delta": delta, "start_ms": start_ms, "end_ms": end_ms})
}

/// The silence that ends a response under the default segmentation.
fn gap() -> Vec<Value> {
    (0..8).map(|_| silent()).collect()
}

fn speech_started(events: &[SessionEvent]) -> Vec<(usize, SampleClockPosition)> {
    events
        .iter()
        .enumerate()
        .filter_map(|(at, event)| match event {
            SessionEvent::SpeechStarted(started) => Some((at, started.position)),
            _ => None,
        })
        .collect()
}

fn transcribed(events: &[SessionEvent], text: &str) -> usize {
    events
        .iter()
        .position(
            |event| matches!(event, SessionEvent::InputTranscribed(heard) if heard.text == text),
        )
        .unwrap_or_else(|| panic!("no transcript {text:?}: {events:#?}"))
}

#[test]
fn speech_inside_the_spoken_answer_is_reported_once_before_its_transcript() {
    let mut events = vec![
        heard("Tell me a story", 0, 900),
        said(" Once upon a time", 1_000, 1_600),
        voiced(),
        voiced(),
        heard("wait", 1_250, 1_400),
        heard(" a moment", 1_400, 1_700),
    ];
    events.extend(gap());
    let events = Server::replay(events).events();

    assert_eq!(
        speech_started(&events),
        vec![(
            transcribed(&events, "wait") - 1,
            SampleClockPosition::new(1_250 * 24).unwrap()
        )],
        "speech started where the user's first word over the answer starts, on the input: \
         {events:#?}"
    );
    assert!(transcribed(&events, " a moment") > transcribed(&events, "wait"));
}

#[test]
fn a_trailing_fragment_of_the_question_reports_no_speech() {
    let mut events = vec![
        heard("What is the", 0, 600),
        said(" Paris is", 1_000, 1_300),
        voiced(),
        heard(" capital of France?", 600, 950),
        said(" the capital.", 1_300, 1_700),
        voiced(),
    ];
    events.extend(gap());
    let events = Server::replay(events).events();

    let _ = transcribed(&events, " capital of France?");
    assert_eq!(speech_started(&events), Vec::new(), "{events:#?}");
}

#[test]
fn the_next_question_in_the_answers_trailing_silence_reports_no_speech() {
    let mut events = vec![
        heard("Hi", 0, 300),
        said(" Hello.", 500, 900),
        voiced(),
        silent(),
        silent(),
        heard(" What time", 1_100, 1_400),
    ];
    events.extend(gap());
    let events = Server::replay(events).events();

    let _ = transcribed(&events, " What time");
    assert_eq!(speech_started(&events), Vec::new(), "{events:#?}");
}
