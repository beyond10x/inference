//! A local WebSocket fixture server speaking the realtime speech-to-text protocol of
//! `src/stt_protocol.rs`, the scripts it answers with, and helpers the STT test targets share.
//!
//! The server listens on 127.0.0.1 at a port the system picks. Each connection is checked
//! against the protocol: the `xi-api-key` header carries the fixture key, the path is
//! `/v1/speech-to-text/realtime`, the query names the model, the audio format, the commit
//! strategy and `include_timestamps=true`; every message is a text `input_audio_chunk` whose
//! `audio_base_64` is base64, whose `commit` is a boolean and whose `sample_rate` is the rate of
//! the format the query names; no message carries the key; no audio follows the last commit of a
//! connection the client then closes. Every departure is recorded as a violation.
//!
//! It answers with a [`Script`]: steps read from a file under `tests/fixtures/`, one JSON object
//! per line, each with `on` — `open` (at once), `audio:N` (once N chunks carrying audio have
//! arrived) or `commit:N` (once N chunks with `commit` true have arrived) — and
//! one of `send` (a message, sent as JSON text), `raw` (text sent as it is), `close` (`code` and
//! `reason` of a close frame the server sends, after which it reads only the client's close) or
//! `wait` (milliseconds before the next step). The steps run in file order, each waiting for its
//! own trigger.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::fs;
use std::num::{NonZeroU16, NonZeroU32};
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::{SinkExt, StreamExt};
use inference_elevenlabs::{
    Credential, CredentialFuture, CredentialSource, ElevenLabsRecogniser, ElevenLabsRecognition,
    Endpoint, InputFormat, TranscriptionConfig,
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
pub const KEY: &str = "fixture-key-3c59dc048e885024";

/// When a step runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trigger {
    /// As soon as the connection is open.
    Open,
    /// Once this many chunks carrying audio have arrived.
    Audio(usize),
    /// Once this many chunks with `commit` true have arrived.
    Commit(usize),
}

/// What a step does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Sends this text.
    Send(String),
    /// Sends a close frame with this code and reason.
    Close(u16, String),
    /// Waits this long before the next step.
    Wait(Duration),
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
        Self::Steps(
            text.lines()
                .filter(|line| !line.trim().is_empty())
                .map(step)
                .collect(),
        )
    }

    /// `session_started`, one partial transcript after the first chunk, then `message` as it is.
    pub fn sends_after_first(message: &str) -> Self {
        Self::Steps(vec![
            Step {
                on: Trigger::Open,
                action: Action::Send(session_started()),
            },
            Step {
                on: Trigger::Audio(1),
                action: Action::Send(partial("hallo")),
            },
            Step {
                on: Trigger::Audio(1),
                action: Action::Send(message.to_owned()),
            },
        ])
    }

    /// `session_started`, one partial transcript after the first chunk, then a close frame with
    /// `code` and `reason`.
    pub fn closes_after_first(code: u16, reason: &str) -> Self {
        Self::Steps(vec![
            Step {
                on: Trigger::Open,
                action: Action::Send(session_started()),
            },
            Step {
                on: Trigger::Audio(1),
                action: Action::Send(partial("hallo")),
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
        "open" => Trigger::Open,
        other => {
            let (kind, count) = other.split_once(':').expect("audio:N or commit:N");
            let count = count.parse().expect("a count");
            match kind {
                "audio" => Trigger::Audio(count),
                "commit" => Trigger::Commit(count),
                _ => panic!("an unknown trigger {other}"),
            }
        }
    };
    let action = if let Some(message) = value.get("send") {
        Action::Send(message.to_string())
    } else if let Some(raw) = value.get("raw") {
        Action::Send(raw.as_str().expect("raw text").to_owned())
    } else if let Some(wait) = value.get("wait") {
        Action::Wait(Duration::from_millis(wait.as_u64().expect("milliseconds")))
    } else {
        let close = &value["close"];
        Action::Close(
            u16::try_from(close["code"].as_u64().expect("a close code")).expect("a u16"),
            close["reason"].as_str().unwrap_or_default().to_owned(),
        )
    };
    Step { on, action }
}

/// A `session_started` message.
pub fn session_started() -> String {
    serde_json::json!({
        "message_type": "session_started",
        "session_id": "fixture-session",
        "config": {"sample_rate": 16000, "audio_format": "pcm_16000", "model_id": "scribe_v2_realtime"}
    })
    .to_string()
}

/// A `partial_transcript` message reading `text`.
pub fn partial(text: &str) -> String {
    serde_json::json!({"message_type": "partial_transcript", "text": text}).to_string()
}

/// What the server saw of one connection.
#[derive(Debug, Clone, Default)]
pub struct Seen {
    pub path: String,
    pub query: String,
    pub key: Option<String>,
    /// Chunks carrying audio.
    pub audio_chunks: usize,
    pub audio_bytes: usize,
    /// Chunks with `commit` true, with or without audio.
    pub commits: usize,
    /// For each commit, the number of chunks carrying audio before it.
    pub commits_after: Vec<usize>,
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
            .with_read_timeout(Duration::from_secs(2))
            .with_connect_timeout(Duration::from_secs(5))
    }

    /// A recogniser of `config` at this server, with the fixture key.
    pub fn recogniser(&self, config: TranscriptionConfig) -> ElevenLabsRecogniser {
        ElevenLabsRecogniser::new(
            self.endpoint(),
            config,
            Arc::new(FixedKey(KEY)),
            "elevenlabs-api-key",
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

/// The sample rate the `audio_format` of `query` names.
fn query_rate(query: &str) -> Option<u64> {
    let format = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("audio_format="))?;
    format
        .strip_prefix("pcm_")
        .or_else(|| format.strip_prefix("ulaw_"))
        .and_then(|rate| rate.parse().ok())
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
        let key = request
            .headers()
            .get("xi-api-key")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        *callback_head.lock().unwrap() = (
            request.uri().path().to_owned(),
            request.uri().query().unwrap_or_default().to_owned(),
            key,
        );
        if let Some(status) = refusal {
            let mut refused =
                ErrorResponse::new(Some("{\"detail\":\"invalid_api_key\"}".to_owned()));
            *refused.status_mut() = StatusCode::from_u16(status).expect("a status");
            return Err(refused);
        }
        Ok(response)
    };
    let accepted = tokio_tungstenite::accept_hdr_async(stream, callback).await;
    if accepted.is_err() && refusal.is_none() {
        record(&|seen| seen.client_closed = true);
        let _ = closes.send(index);
        return;
    }
    let (path, query, key) = head.lock().unwrap().clone();
    let mut violations = Vec::new();
    if key.as_deref() != Some(KEY) {
        violations.push("the xi-api-key header is not the fixture key".to_owned());
    }
    if path != "/v1/speech-to-text/realtime" {
        violations.push(format!("path {path}"));
    }
    for needed in [
        "model_id=",
        "audio_format=",
        "commit_strategy=",
        "include_timestamps=true",
    ] {
        if !query.split('&').any(|pair| pair.starts_with(needed)) {
            violations.push(format!("the query lacks {needed}"));
        }
    }
    if query.contains(KEY) {
        violations.push("the query carries the key".to_owned());
    }
    let rate = query_rate(&query);
    record(&|seen| {
        seen.path.clone_from(&path);
        seen.query.clone_from(&query);
        seen.key.clone_from(&key);
        seen.violations.clone_from(&violations);
    });
    let Ok(mut socket) = accepted else {
        record(&|seen| seen.client_closed = true);
        let _ = closes.send(index);
        return;
    };
    let Script::Steps(steps) = script else { return };
    let mut steps: VecDeque<Step> = steps.into();
    let mut audio = 0;
    let mut commits = 0;
    let mut server_closed = false;
    loop {
        while !server_closed && let Some(step) = steps.front() {
            let due = match step.on {
                Trigger::Open => true,
                Trigger::Audio(count) => audio >= count,
                Trigger::Commit(count) => commits >= count,
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
                Action::Wait(pause) => tokio::time::sleep(pause).await,
            }
        }
        let violation = match socket.next().await {
            Some(Ok(Message::Text(text))) => {
                let text = text.as_str().to_owned();
                if text.contains(KEY) {
                    Some("a message carries the key".to_owned())
                } else {
                    match chunk(&text, rate) {
                        Ok((bytes, commit)) => {
                            if bytes > 0 {
                                audio += 1;
                                record(&|seen| {
                                    seen.audio_chunks += 1;
                                    seen.audio_bytes += bytes;
                                });
                            }
                            if commit {
                                commits += 1;
                                let before = audio;
                                record(&|seen| {
                                    seen.commits += 1;
                                    seen.commits_after.push(before);
                                });
                            }
                            None
                        }
                        Err(violation) => Some(violation),
                    }
                }
            }
            Some(Ok(Message::Binary(data))) => {
                Some(format!("a binary message of {} bytes", data.len()))
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

/// The audio bytes and the commit flag of one client message, or why it is not an
/// `input_audio_chunk` at `rate`.
fn chunk(text: &str, rate: Option<u64>) -> Result<(usize, bool), String> {
    let value: Value =
        serde_json::from_str(text).map_err(|_| format!("a message that is not JSON: {text}"))?;
    if value["message_type"] != "input_audio_chunk" {
        return Err(format!("a message that is not input_audio_chunk: {text}"));
    }
    let Some(encoded) = value["audio_base_64"].as_str() else {
        return Err("a chunk without audio_base_64".to_owned());
    };
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| "audio_base_64 that is not base64".to_owned())?;
    let Some(commit) = value["commit"].as_bool() else {
        return Err("a chunk whose commit is not a boolean".to_owned());
    };
    if value["sample_rate"].as_u64() != rate {
        return Err(format!(
            "sample_rate {} where the query names {rate:?}",
            value["sample_rate"]
        ));
    }
    Ok((bytes.len(), commit))
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

/// The configuration of `format` with every option left at its default: manual commit.
pub fn manual(format: InputFormat) -> TranscriptionConfig {
    TranscriptionConfig::new(format)
}

/// `count` frames of 20 ms of G.711 μ-law at 8 kHz, a 440 Hz tone at half scale.
pub fn mulaw(count: usize) -> Vec<AudioFrame> {
    let format = AudioFormat {
        encoding: AudioEncoding::PcmMulaw,
        sample_rate: NonZeroU32::new(8_000).expect("not zero"),
        channels: NonZeroU16::MIN,
    };
    (0..count as u64)
        .map(|index| {
            let data = (0..160_u64)
                .map(|sample| {
                    let at = (index * 160 + sample) as f64 / 8_000.0;
                    let value = (at * 440.0 * std::f64::consts::TAU).sin() * 16_000.0;
                    inference_stt::inference_audio::encode_mulaw(value as i16)
                })
                .collect();
            AudioFrame {
                format,
                position: SampleClockPosition::new(index * 160).expect("small"),
                sample_count: 160,
                data,
            }
        })
        .collect()
}

/// `count` frames of 10 ms of 16 kHz `pcm_s16le` silence.
pub fn pcm16k(count: usize) -> Vec<AudioFrame> {
    inference_testkit::stt::frames(count)
}

/// An event as a short word: `hypothesis <text>`, `revision <text>`, `commit <text>` or `usage`.
pub fn describe(envelope: &EventEnvelope<TranscriptEvent>) -> String {
    match &envelope.payload {
        TranscriptEvent::Hypothesis(event) => format!("hypothesis {}", event.text),
        TranscriptEvent::Revision(event) => format!("revision {}", event.text),
        TranscriptEvent::Committed(event) => format!("commit {}", event.text),
        TranscriptEvent::Usage(_) => "usage".to_owned(),
    }
}

/// Offers `frames` to a fresh recognition of `recogniser`, iterating after each, ends input and
/// takes every event until the iteration ends. Returns the handle and every event, in order.
pub fn recognise(
    recogniser: &mut ElevenLabsRecogniser,
    frames: Vec<AudioFrame>,
) -> (ElevenLabsRecognition, Vec<EventEnvelope<TranscriptEvent>>) {
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

/// Iterates `handle` until `events` holds an event `describe` reads as `wanted`, waiting at most
/// `bound`.
pub fn wait_for(
    handle: &mut ElevenLabsRecognition,
    events: &mut Vec<EventEnvelope<TranscriptEvent>>,
    wanted: &str,
    bound: Duration,
) {
    let deadline = std::time::Instant::now() + bound;
    while !events.iter().any(|event| describe(event) == wanted) {
        assert!(
            std::time::Instant::now() < deadline,
            "no `{wanted}` within {bound:?}; events {:?}",
            events.iter().map(describe).collect::<Vec<_>>()
        );
        events.extend(handle.by_ref());
        std::thread::sleep(Duration::from_millis(10));
    }
}
