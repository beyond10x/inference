//! A local WebSocket fixture server speaking the `/v1/listen` protocol of `src/listen.rs`, the
//! scripts it answers with, and helpers the test targets share.
//!
//! The server listens on 127.0.0.1 at a port the system picks. Each connection is checked
//! against the protocol: the `Authorization` header is `Token` and the fixture key, the path is
//! `/v1/listen`, the query names the model, language, encoding, rate, one channel and interim
//! results; binary messages carry audio and none is empty; text messages are `KeepAlive` or
//! `CloseStream` and carry no key; no audio follows `CloseStream`. Every departure is recorded as
//! a violation, and an empty binary message is refused with close 1008, as the endpoint does.
//!
//! It answers with a [`Script`]: steps read from a file under `tests/fixtures/`, one JSON object
//! per line, each with `on`, `audio:N` (once N audio messages have arrived, or `CloseStream`
//! has) or `close_stream`, and one of `send` (a message, sent as JSON text), `raw` (text sent as
//! it is) or `close` (`code` and `reason` of a close frame the server sends, after which it
//! reads only the client's close). The steps run in file order, each waiting for its own trigger.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use inference_deepgram::{
    Credential, CredentialError, CredentialFuture, CredentialSource, DeepgramRecogniser,
    DeepgramRecognition, Endpoint, ListenConfig,
};
use inference_stt::inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_stt::inference_core::envelope::EventEnvelope;
use inference_stt::inference_core::id::{NodeId, RunId, StreamId};
use inference_stt::inference_text::TurnId;
use inference_stt::{Recogniser, RecognitionHandle, RecognitionRequest, TranscriptEvent};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;

/// The key the fixture server expects; no real key appears in any test.
pub const KEY: &str = "fixture-key-5f2b8c0e91d3a7c4";

/// When a step runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trigger {
    /// Once this many audio messages have arrived, or `CloseStream` has.
    Audio(usize),
    /// Once `CloseStream` has arrived.
    CloseStream,
}

/// What a step does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Sends this text.
    Send(String),
    /// Sends a close frame with this code and reason.
    Close(u16, String),
}

/// One step of a script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub on: Trigger,
    pub action: Action,
}

/// How the server answers a connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Script {
    /// Accepts the handshake and runs the steps.
    Steps(Vec<Step>),
    /// Refuses the handshake with this HTTP status.
    Refuse(u16),
}

impl Script {
    /// The steps of the fixture file `name` under `tests/fixtures/`.
    pub fn fixture(name: &str) -> Self {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        let text =
            fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        Self::Steps(text.lines().map(step).collect())
    }

    /// One `Results` after the first audio message, then a close frame with `code` and `reason`.
    pub fn closes_after_first(code: u16, reason: &str) -> Self {
        Self::Steps(vec![
            Step {
                on: Trigger::Audio(1),
                action: Action::Send(results("hello", false, false)),
            },
            Step {
                on: Trigger::Audio(1),
                action: Action::Close(code, reason.to_owned()),
            },
        ])
    }
}

/// The step of one fixture line.
fn step(line: &str) -> Step {
    let value: Value = serde_json::from_str(line).expect("a fixture line is JSON");
    let on = match value["on"].as_str().expect("a step names its trigger") {
        "close_stream" => Trigger::CloseStream,
        other => Trigger::Audio(
            other
                .strip_prefix("audio:")
                .and_then(|count| count.parse().ok())
                .expect("audio:N"),
        ),
    };
    let action = if let Some(message) = value.get("send") {
        Action::Send(message.to_string())
    } else if let Some(raw) = value.get("raw") {
        Action::Send(raw.as_str().expect("raw text").to_owned())
    } else {
        let close = &value["close"];
        Action::Close(
            u16::try_from(close["code"].as_u64().expect("a close code")).expect("a u16"),
            close["reason"].as_str().unwrap_or_default().to_owned(),
        )
    };
    Step { on, action }
}

/// A `Results` message with `transcript` as its first alternative.
pub fn results(transcript: &str, is_final: bool, speech_final: bool) -> String {
    serde_json::json!({
        "type": "Results",
        "channel_index": [0, 1],
        "duration": 0.5,
        "start": 0.0,
        "is_final": is_final,
        "speech_final": speech_final,
        "channel": {"alternatives": [{"transcript": transcript, "confidence": 0.9, "words": []}]},
        "metadata": {"request_id": "fixture-request", "model_uuid": "fixture-model"}
    })
    .to_string()
}

/// What the server saw of one connection.
#[derive(Debug, Clone, Default)]
pub struct Seen {
    pub path: String,
    pub query: String,
    pub authorization: Option<String>,
    pub audio_messages: usize,
    pub audio_bytes: usize,
    pub keepalives: usize,
    pub close_streams: usize,
    pub violations: Vec<String>,
    /// The client closed the connection, by a close frame or by dropping it.
    pub client_closed: bool,
}

/// A running fixture server; it stops when dropped.
pub struct FixtureServer {
    pub url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    closes: Mutex<mpsc::Receiver<usize>>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FixtureServer {
    /// A server answering every connection with `script`.
    pub fn start(script: Script) -> Self {
        Self::scripted(vec![script])
    }

    /// A server answering its n-th connection with `scripts[n]`, the last script for every
    /// connection past them.
    pub fn scripted(scripts: Vec<Script>) -> Self {
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        std_listener.set_nonblocking(true).expect("non-blocking");
        let url = format!("ws://{}", std_listener.local_addr().expect("an address"));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (close_tx, close_rx) = mpsc::channel();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let thread_seen = Arc::clone(&seen);
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime");
            runtime.block_on(async move {
                let listener = TcpListener::from_std(std_listener).expect("a listener");
                let mut stop_rx = stop_rx;
                let mut index = 0;
                loop {
                    let accepted = tokio::select! {
                        _ = &mut stop_rx => break,
                        accepted = listener.accept() => accepted,
                    };
                    let Ok((stream, _)) = accepted else { break };
                    let script = scripts[index.min(scripts.len() - 1)].clone();
                    thread_seen.lock().unwrap().push(Seen::default());
                    tokio::spawn(serve(
                        stream,
                        index,
                        script,
                        Arc::clone(&thread_seen),
                        close_tx.clone(),
                    ));
                    index += 1;
                }
            });
        });
        Self {
            url,
            seen,
            closes: Mutex::new(close_rx),
            stop: Some(stop_tx),
            thread: Some(thread),
        }
    }

    /// What the server saw of every connection so far.
    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    /// Every violation of every connection so far.
    pub fn violations(&self) -> Vec<String> {
        self.seen()
            .into_iter()
            .flat_map(|seen| seen.violations)
            .collect()
    }

    /// The index of the next connection the client closed, waiting at most `bound`.
    pub fn next_client_close(&self, bound: Duration) -> Option<usize> {
        self.closes.lock().unwrap().recv_timeout(bound).ok()
    }

    /// An endpoint at this server, with short bounds.
    pub fn endpoint(&self) -> Endpoint {
        Endpoint::new()
            .with_url(&self.url)
            .with_read_timeout(Duration::from_secs(3))
            .with_connect_timeout(Duration::from_secs(5))
    }

    /// A recogniser of the default configuration at this server, with the fixture key.
    pub fn recogniser(&self) -> DeepgramRecogniser {
        self.recogniser_with(ListenConfig::new())
    }

    /// A recogniser of `config` at this server, with the fixture key.
    pub fn recogniser_with(&self, config: ListenConfig) -> DeepgramRecogniser {
        DeepgramRecogniser::new(
            self.endpoint(),
            config,
            Arc::new(FixedKey(KEY)),
            "deepgram-api-key",
        )
        .expect("the fixture configuration is valid")
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The handshake callback's error type is tungstenite's `ErrorResponse`, which its `Callback`
/// trait fixes, so the size of that error is not this function's to change.
#[allow(clippy::result_large_err)]
async fn serve(
    stream: tokio::net::TcpStream,
    index: usize,
    script: Script,
    seen: Arc<Mutex<Vec<Seen>>>,
    closes: mpsc::Sender<usize>,
) {
    let record = |update: &dyn Fn(&mut Seen)| update(&mut seen.lock().unwrap()[index]);
    let head = Arc::new(Mutex::new((String::new(), String::new(), None)));
    let callback_head = Arc::clone(&head);
    let refusal = match &script {
        Script::Refuse(status) => Some(*status),
        Script::Steps(_) => None,
    };
    let callback = move |request: &Request, response: Response| {
        let authorization = request
            .headers()
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        *callback_head.lock().unwrap() = (
            request.uri().path().to_owned(),
            request.uri().query().unwrap_or_default().to_owned(),
            authorization,
        );
        if let Some(status) = refusal {
            let mut refused =
                ErrorResponse::new(Some("{\"err_code\":\"INVALID_AUTH\"}".to_owned()));
            *refused.status_mut() = StatusCode::from_u16(status).expect("a status");
            return Err(refused);
        }
        Ok(response)
    };
    let accepted = tokio_tungstenite::accept_hdr_async(stream, callback).await;
    let (path, query, authorization) = head.lock().unwrap().clone();
    let mut violations = Vec::new();
    if authorization.as_deref() != Some(&format!("Token {KEY}")) {
        violations.push("the Authorization header is not `Token` and the fixture key".to_owned());
    }
    if path != "/v1/listen" {
        violations.push(format!("path {path}"));
    }
    for needed in [
        "model=",
        "language=",
        "encoding=",
        "sample_rate=",
        "channels=1",
        "interim_results=true",
    ] {
        if !query.split('&').any(|pair| pair.starts_with(needed)) {
            violations.push(format!("the query lacks {needed}"));
        }
    }
    record(&|seen| {
        seen.path.clone_from(&path);
        seen.query.clone_from(&query);
        seen.authorization.clone_from(&authorization);
        seen.violations.clone_from(&violations);
    });
    let Ok(mut socket) = accepted else { return };
    let Script::Steps(steps) = script else { return };
    let mut steps: VecDeque<Step> = steps.into();
    let mut audio = 0;
    let mut closed_stream = false;
    let mut server_closed = false;
    loop {
        while !server_closed && let Some(step) = steps.front() {
            let due = match step.on {
                Trigger::Audio(count) => audio >= count || closed_stream,
                Trigger::CloseStream => closed_stream,
            };
            if !due {
                break;
            }
            let step = steps.pop_front().expect("a step is due");
            match step.action {
                Action::Send(text) => {
                    let _ = socket.send(Message::text(text)).await;
                }
                Action::Close(code, reason) => {
                    let _ = socket
                        .send(Message::Close(Some(CloseFrame {
                            code: code.into(),
                            reason: reason.into(),
                        })))
                        .await;
                    server_closed = true;
                }
            }
        }
        let violation = match socket.next().await {
            Some(Ok(Message::Binary(data))) => {
                if data.is_empty() {
                    let _ = socket
                        .send(Message::Close(Some(CloseFrame {
                            code: 1008.into(),
                            reason: "DATA-0000".into(),
                        })))
                        .await;
                    server_closed = true;
                    Some("an empty binary message".to_owned())
                } else {
                    audio += 1;
                    let length = data.len();
                    record(&|seen| {
                        seen.audio_messages += 1;
                        seen.audio_bytes += length;
                    });
                    closed_stream.then(|| "audio after CloseStream".to_owned())
                }
            }
            Some(Ok(Message::Text(text))) => {
                let text = text.as_str().to_owned();
                let kind = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|value| value["type"].as_str().map(str::to_owned));
                if text.contains(KEY) {
                    Some("a text message carries the key".to_owned())
                } else {
                    match kind.as_deref() {
                        Some("KeepAlive") if !closed_stream => {
                            record(&|seen| seen.keepalives += 1);
                            None
                        }
                        Some("CloseStream") if !closed_stream => {
                            closed_stream = true;
                            record(&|seen| seen.close_streams += 1);
                            None
                        }
                        _ => Some(format!("an unexpected text message: {text}")),
                    }
                }
            }
            Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => None,
            Some(Ok(Message::Close(_)) | Err(_)) | None => break,
        };
        if let Some(violation) = violation {
            record(&|seen| seen.violations.push(violation.clone()));
        }
    }
    record(&|seen| seen.client_closed = true);
    let _ = closes.send(index);
}

/// A credential source that always returns one fixed key.
pub struct FixedKey(pub &'static str);

impl CredentialSource for FixedKey {
    fn resolve<'a>(&'a self, _name: &'a str) -> CredentialFuture<'a> {
        Box::pin(std::future::ready(Ok(Credential::new(
            self.0.as_bytes().to_vec(),
        ))))
    }
}

/// A credential source that holds nothing.
pub struct NoKey;

impl CredentialSource for NoKey {
    fn resolve<'a>(&'a self, _name: &'a str) -> CredentialFuture<'a> {
        Box::pin(std::future::ready(Err(CredentialError::Missing)))
    }
}

/// A fresh request on a fresh stream.
pub fn request() -> RecognitionRequest {
    RecognitionRequest::new(
        RunId::generate(),
        TurnId::generate(),
        StreamId::generate(),
        NodeId::generate(),
        "text_out",
    )
}

/// The first `count` frames of the synthetic tone and silence, 16 kHz `pcm_s16le`.
pub fn tone(count: usize) -> Vec<AudioFrame> {
    let mut frames = inference_testkit::stt::synthetic_audio::tone_silence_tone_silence();
    frames.truncate(count);
    frames
}

/// `count` frames of 10 ms of μ-law silence at 8 kHz.
pub fn mulaw(count: usize) -> Vec<AudioFrame> {
    let format = AudioFormat {
        encoding: AudioEncoding::PcmMulaw,
        sample_rate: std::num::NonZeroU32::new(8_000).expect("not zero"),
        channels: std::num::NonZeroU16::MIN,
    };
    (0..count as u64)
        .map(|index| AudioFrame {
            format,
            position: SampleClockPosition::new(index * 80).expect("small"),
            sample_count: 80,
            data: vec![0xFF; 80],
        })
        .collect()
}

/// An event as a short word: `hypothesis <text>`, `revision <text>`, `commit` or `usage`.
pub fn describe(envelope: &EventEnvelope<TranscriptEvent>) -> String {
    match &envelope.payload {
        TranscriptEvent::Hypothesis(event) => format!("hypothesis {}", event.text),
        TranscriptEvent::Revision(event) => format!("revision {}", event.text),
        TranscriptEvent::Committed(_) => "commit".to_owned(),
        TranscriptEvent::Usage(_) => "usage".to_owned(),
    }
}

/// Offers `frames` to a fresh recognition of `recogniser`, iterating after each, ends input and
/// takes every event until the iteration ends. Returns the handle and every event, in order.
pub fn recognise(
    recogniser: &mut DeepgramRecogniser,
    frames: Vec<AudioFrame>,
) -> (DeepgramRecognition, Vec<EventEnvelope<TranscriptEvent>>) {
    let mut handle = recogniser.start(request()).expect("a slot is free");
    let mut events = Vec::new();
    for frame in frames {
        handle.offer_frame(frame).expect("input is open");
        events.extend(handle.by_ref());
    }
    handle.end_input().expect("input ends once");
    events.extend(handle.by_ref().take(1_000));
    (handle, events)
}
