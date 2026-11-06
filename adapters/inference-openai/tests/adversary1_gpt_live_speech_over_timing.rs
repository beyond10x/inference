//! Adversary pass 2: the GPT-Live binding's speech-over rule (`speech_over`, `Spoken`) driven
//! offline through the binding itself, a local WebSocket server replaying the provider.
//!
//! The rule: an input transcript fragment whose `start_ms` lies in `[first output start_ms,
//! latest output end_ms)` of the response in progress reports `SpeechStarted`, once per response.
//! The binding segments a response by 800 ms of silent output, so a model that yields to the
//! listener and speaks again within that gap goes on with the same response: the hosted model
//! did so on 2026-10-20 (`src/live_duplex.rs`, `claims`, output "that took that input up" on the
//! same response).

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

    /// The binding's events until `responses` responses completed, then the close.
    fn events(self, responses: usize) -> Vec<SessionEvent> {
        let mut model = OpenAiLive::new(
            self.endpoint.clone(),
            Box::new(FixedSource),
            OPENAI_KEY_NAME,
        );
        let mut session = model.open(config()).expect("the session opens");
        let mut events = Vec::new();
        let mut idle = 0;
        let mut completed = 0;
        while idle < 20 {
            match session.next_event(WAIT) {
                Some(event) => {
                    idle = 0;
                    if matches!(event, SessionEvent::ResponseCompleted(_)) {
                        completed += 1;
                    }
                    events.push(event);
                    if completed == responses {
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

fn said(delta: &str, start_ms: u64, end_ms: u64) -> Value {
    json!({"type": "session.output_transcript.delta", "event_id": "evt_out", "delta": delta, "start_ms": start_ms, "end_ms": end_ms})
}

fn heard(delta: &str, start_ms: u64, end_ms: u64) -> Value {
    json!({"type": "session.input_transcript.delta", "event_id": "evt_in", "delta": delta, "start_ms": start_ms, "end_ms": end_ms})
}

fn gap() -> Vec<Value> {
    (0..8).map(|_| silent()).collect()
}

fn at(ms: u64) -> SampleClockPosition {
    SampleClockPosition::new(ms * 24).unwrap()
}

fn speech_started(events: &[SessionEvent]) -> Vec<SampleClockPosition> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::SpeechStarted(started) => Some(started.position),
            _ => None,
        })
        .collect()
}

fn created(events: &[SessionEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, SessionEvent::ResponseCreated(_)))
        .count()
}

/// The user interrupts, the model yields for 300 ms (less than the 800 ms gap) and goes on, and
/// the user interrupts again. Each interruption is speech over what the model has spoken since
/// the one before, and neither is lost.
///
/// story:cut-response-audio-is-told-from-the-next-words: the model's words after the first
/// interruption, starting after both its cut output and the user's speech ended, are a new
/// response, so the replay reads two responses and the second interruption is speech over the
/// one carrying the resumed words.
#[test]
fn a_second_interruption_of_the_same_response_is_reported() {
    let mut events = vec![
        heard("Tell me a story", 0, 900),
        said(" Once upon a time", 1_000, 1_600),
        voiced(),
        voiced(),
        heard("wait", 1_250, 1_400),
        silent(),
        silent(),
        silent(),
        said(" Sure, so the story goes", 1_900, 2_600),
        voiced(),
        voiced(),
        heard("stop", 2_100, 2_300),
    ];
    events.extend(gap());
    let events = Server::replay(events).events(2);

    assert_eq!(
        created(&events),
        2,
        "story:cut-response-audio-is-told-from-the-next-words: the cut answer, then the resumed \
         words on a response of their own: {events:#?}"
    );
    assert_eq!(
        speech_started(&events),
        vec![at(1_250), at(2_100)],
        "both interruptions of the answer are speech over it: {events:#?}"
    );
    let second = |which: fn(&SessionEvent) -> bool| {
        events
            .iter()
            .enumerate()
            .filter(|(_, event)| which(event))
            .map(|(index, _)| index)
            .nth(1)
    };
    let resumed = second(|event| matches!(event, SessionEvent::ResponseCreated(_)));
    let second_speech = second(|event| matches!(event, SessionEvent::SpeechStarted(_)));
    assert!(
        resumed
            .zip(second_speech)
            .is_some_and(|(created, spoken)| created < spoken),
        "story:cut-response-audio-is-told-from-the-next-words: the second interruption is \
         speech over the response carrying the resumed words: {events:#?}"
    );
}

/// A second response, after the first ended on the silence gap, is interrupted as the first was.
#[test]
fn the_next_response_is_interrupted_as_the_first() {
    let mut events = vec![said(" Hello.", 500, 900), voiced(), heard("hey", 600, 800)];
    events.extend(gap());
    events.extend([
        said(" Anything else?", 2_000, 2_600),
        voiced(),
        heard("no", 2_200, 2_400),
    ]);
    events.extend(gap());
    let events = Server::replay(events).events(2);

    assert_eq!(created(&events), 2, "{events:#?}");
    assert_eq!(speech_started(&events), vec![at(600), at(2_200)]);
}

/// The stretch is half-open: a fragment at its first start is over it, one at its latest end is
/// not.
#[test]
fn the_stretch_edges() {
    let mut first = vec![said(" Hello.", 500, 900), voiced(), heard("a", 500, 600)];
    first.extend(gap());
    assert_eq!(
        speech_started(&Server::replay(first).events(1)),
        vec![at(500)]
    );

    let mut second = vec![said(" Hello.", 500, 900), voiced(), heard("a", 900, 1_000)];
    second.extend(gap());
    assert_eq!(
        speech_started(&Server::replay(second).events(1)),
        Vec::new()
    );
}

/// Output fragments delivered out of order widen the stretch both ways.
#[test]
fn out_of_order_output_fragments_widen_the_stretch() {
    let mut events = vec![
        said(" time", 1_300, 1_600),
        said(" Once upon a", 1_000, 1_300),
        voiced(),
        heard("wait", 1_100, 1_200),
    ];
    events.extend(gap());
    assert_eq!(
        speech_started(&Server::replay(events).events(1)),
        vec![at(1_100)]
    );
}

/// The stretch of a response that ended is no stretch of the next: a response begun by audio,
/// with no output transcript yet, reports nothing for a fragment inside the old one.
#[test]
fn a_previous_responses_stretch_reports_nothing_over_the_next() {
    let mut events = vec![said(" Hello.", 500, 1_500), voiced()];
    events.extend(gap());
    events.extend([voiced(), heard("late", 1_000, 1_200)]);
    events.extend(gap());
    let events = Server::replay(events).events(2);
    assert_eq!(created(&events), 2, "{events:#?}");
    assert_eq!(speech_started(&events), Vec::new(), "{events:#?}");
}
