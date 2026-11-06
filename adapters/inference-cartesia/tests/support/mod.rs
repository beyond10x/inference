//! A local WebSocket fixture server speaking the text-to-speech protocol of `docs/protocol.md`,
//! and helpers the test targets share.
//!
//! The server listens on 127.0.0.1 at a port the system picks. Each connection is checked
//! against the protocol: the `X-API-Key` header carries the fixture key, the path is
//! `/tts/websocket` with the API version, no message carries a key, every generation request
//! names the model, a transcript, the fixture voice, the expected raw output format, a context
//! not used before on the connection and `continue` false, and every cancel names a context the
//! connection opened. Every departure is recorded as a violation. Each generation request is
//! answered with a synthesized 200 ms tone in the requested format, in four `chunk` messages and
//! then `done`, as its [`Script`] says. No recording, no network beyond loopback.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::{SinkExt, StreamExt};
use inference_cartesia::{
    CartesiaSynthesiser, Credential, CredentialError, CredentialFuture, CredentialSource, Endpoint,
    OutputFormat, VoiceConfig,
};
use inference_tts::inference_audio::{AudioEncoding, encode_alaw, encode_mulaw};
use inference_tts::inference_core::generation::{Generation, StartGeneration};
use inference_tts::inference_core::id::{NodeId, RunId};
use inference_tts::inference_text::OutputSegment;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;

/// The key the fixture server expects; no real key appears in any test.
pub const KEY: &str = "fixture-key-0f343b0931126a20";

/// The voice the fixture server expects.
pub const VOICE: &str = "fixture-voice-01";

/// The longest any test may run before it fails instead of holding the build lock.
pub const DEADLINE: Duration = Duration::from_secs(25);

/// How the server answers one generation request, by its index across the server's life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Script {
    /// Every chunk, then `done`.
    Serve,
    /// Every chunk, the last with `done` true, and no `done` message.
    DoneInChunk,
    /// The first chunk; the rest and `done` only once the context is cancelled, as a context
    /// that had begun generating goes on after a cancel.
    HoldAfterFirst,
    /// The first chunk, then the connection closed with code 1011.
    CloseAfterFirst,
    /// The first chunk, then an `error` for the context.
    ErrorAfterFirst,
    /// Nothing at all.
    Silent,
    /// The first chunk, then a chunk of a context the client never opened.
    ForeignAfterFirst,
}

/// What the server saw of one connection.
#[derive(Debug, Clone, Default)]
pub struct Seen {
    pub path: String,
    pub query: String,
    pub key: Option<String>,
    pub messages: Vec<String>,
    pub requests: Vec<Value>,
    pub cancels: Vec<String>,
    pub violations: Vec<String>,
    /// The client closed the connection, by a close frame or by dropping it.
    pub client_closed: bool,
}

/// A running fixture server; it stops when dropped.
pub struct FixtureServer {
    pub url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    closes: Mutex<mpsc::Receiver<usize>>,
    cancels: Mutex<mpsc::Receiver<String>>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// What every connection of one server shares.
struct Plan {
    format: OutputFormat,
    scripts: Vec<Script>,
    refuse: bool,
    requests: Mutex<usize>,
    seen: Arc<Mutex<Vec<Seen>>>,
    closes: mpsc::Sender<usize>,
    cancels: mpsc::Sender<String>,
}

impl FixtureServer {
    /// A server answering every generation request with `script`, expecting `format`.
    pub fn start(format: OutputFormat, script: Script) -> Self {
        Self::scripted(format, vec![script])
    }

    /// A server answering its n-th generation request with `scripts[n]`, the last script for
    /// every request past them.
    pub fn scripted(format: OutputFormat, scripts: Vec<Script>) -> Self {
        Self::launch(format, scripts, false)
    }

    /// A server refusing every handshake with HTTP 401.
    pub fn refusing(format: OutputFormat) -> Self {
        Self::launch(format, vec![Script::Serve], true)
    }

    fn launch(format: OutputFormat, scripts: Vec<Script>, refuse: bool) -> Self {
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        std_listener.set_nonblocking(true).expect("non-blocking");
        let url = format!("ws://{}", std_listener.local_addr().expect("an address"));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (close_tx, close_rx) = mpsc::channel();
        let (cancel_tx, cancel_rx) = mpsc::channel();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let plan = Arc::new(Plan {
            format,
            scripts,
            refuse,
            requests: Mutex::new(0),
            seen: Arc::clone(&seen),
            closes: close_tx,
            cancels: cancel_tx,
        });
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
                    plan.seen.lock().unwrap().push(Seen::default());
                    tokio::spawn(serve(stream, index, Arc::clone(&plan)));
                    index += 1;
                }
            });
        });
        Self {
            url,
            seen,
            closes: Mutex::new(close_rx),
            cancels: Mutex::new(cancel_rx),
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

    /// The context of the next cancel the server read, waiting at most `bound`.
    pub fn next_cancel(&self, bound: Duration) -> Option<String> {
        self.cancels.lock().unwrap().recv_timeout(bound).ok()
    }

    /// An endpoint at this server, with short bounds.
    pub fn endpoint(&self) -> Endpoint {
        Endpoint::new()
            .with_url(&self.url)
            .with_read_timeout(Duration::from_secs(5))
            .with_connect_timeout(Duration::from_secs(5))
    }

    /// A synthesiser of the fixture voice in `format` at this server, with the fixture key.
    pub fn synthesiser(&self, format: OutputFormat) -> CartesiaSynthesiser {
        CartesiaSynthesiser::new(
            self.endpoint(),
            VoiceConfig::new(VOICE, format),
            Arc::new(FixedKey(KEY)),
            "cartesia-api-key",
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
async fn serve(stream: tokio::net::TcpStream, index: usize, plan: Arc<Plan>) {
    let record = |update: &dyn Fn(&mut Seen)| update(&mut plan.seen.lock().unwrap()[index]);
    let head = Arc::new(Mutex::new((String::new(), String::new(), None)));
    let callback_head = Arc::clone(&head);
    let refuse = plan.refuse;
    let callback = move |request: &Request, response: Response| {
        let key = request
            .headers()
            .get("x-api-key")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        *callback_head.lock().unwrap() = (
            request.uri().path().to_owned(),
            request.uri().query().unwrap_or_default().to_owned(),
            key,
        );
        if refuse {
            let mut refusal = ErrorResponse::new(Some("invalid api key".to_owned()));
            *refusal.status_mut() = StatusCode::UNAUTHORIZED;
            return Err(refusal);
        }
        Ok(response)
    };
    let accepted = tokio_tungstenite::accept_hdr_async(stream, callback).await;
    let (path, query, key) = head.lock().unwrap().clone();
    let mut violations = Vec::new();
    if key.as_deref() != Some(KEY) {
        violations.push("the X-API-Key header is not the fixture key".to_owned());
    }
    if path != "/tts/websocket" {
        violations.push(format!("path {path}"));
    }
    if query != "cartesia_version=2026-08-14" {
        violations.push(format!("query {query}"));
    }
    record(&|seen| {
        seen.path.clone_from(&path);
        seen.query.clone_from(&query);
        seen.key.clone_from(&key);
        seen.violations.extend(violations.iter().cloned());
    });
    let Ok(mut socket) = accepted else { return };

    let chunks = tone_chunks(plan.format);
    let mut opened = HashSet::new();
    let mut held: HashMap<String, Vec<String>> = HashMap::new();
    loop {
        let text = match socket.next().await {
            Some(Ok(Message::Text(text))) => text.as_str().to_owned(),
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
            _ => break,
        };
        record(&|seen| seen.messages.push(text.clone()));
        let mut problems = Vec::new();
        let outgoing = match read_client(&text, plan.format, &mut opened, &mut problems) {
            Client::Request(request) => {
                let context = request["context_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                record(&|seen| seen.requests.push(request.clone()));
                let script = {
                    let mut requests = plan.requests.lock().unwrap();
                    let script = plan.scripts[(*requests).min(plan.scripts.len() - 1)];
                    *requests += 1;
                    script
                };
                answer(script, &context, &chunks, &mut held)
            }
            Client::Cancel(context) => {
                record(&|seen| seen.cancels.push(context.clone()));
                let _ = plan.cancels.send(context.clone());
                held.remove(&context).unwrap_or_default()
            }
            Client::Invalid => Vec::new(),
        };
        if !problems.is_empty() {
            record(&|seen| seen.violations.extend(problems.iter().cloned()));
        }
        let mut close = false;
        for line in outgoing {
            if line == CLOSE {
                close = true;
                break;
            }
            if socket.send(Message::text(line)).await.is_err() {
                break;
            }
        }
        if close {
            let _ = socket
                .close(Some(tokio_tungstenite::tungstenite::protocol::CloseFrame {
                    code: 1011.into(),
                    reason: "fixture closes mid-segment".into(),
                }))
                .await;
            return;
        }
    }
    record(&|seen| seen.client_closed = true);
    let _ = plan.closes.send(index);
}

/// The marker in an answer that closes the connection instead of sending.
const CLOSE: &str = "\u{0}close";

/// The messages answering a generation request of `context` under `script`; what a held context
/// still owes goes into `held`.
fn answer(
    script: Script,
    context: &str,
    chunks: &[Vec<u8>],
    held: &mut HashMap<String, Vec<String>>,
) -> Vec<String> {
    let chunk = |data: &[u8], done: bool| {
        json!({
            "type": "chunk",
            "data": STANDARD.encode(data),
            "done": done,
            "status_code": 206,
            "step_time": 12.5,
            "context_id": context,
        })
        .to_string()
    };
    let done = json!({"type": "done", "done": true, "status_code": 206, "context_id": context})
        .to_string();
    let first = chunk(&chunks[0], false);
    let rest = || {
        let mut out: Vec<String> = chunks[1..].iter().map(|data| chunk(data, false)).collect();
        out.push(done.clone());
        out
    };
    match script {
        Script::Serve => {
            let mut out = vec![first];
            out.extend(rest());
            out
        }
        Script::DoneInChunk => chunks
            .iter()
            .enumerate()
            .map(|(index, data)| chunk(data, index + 1 == chunks.len()))
            .collect(),
        Script::HoldAfterFirst => {
            held.insert(context.to_owned(), rest());
            vec![first]
        }
        Script::CloseAfterFirst => vec![first, CLOSE.to_owned()],
        Script::ErrorAfterFirst => vec![
            first,
            json!({
                "type": "error",
                "done": true,
                "status_code": 429,
                "error_code": "concurrency_limited",
                "title": "Too many concurrent requests",
                "message": "The fixture's limit.",
                "request_id": "fixture-request",
                "context_id": context,
            })
            .to_string(),
        ],
        Script::Silent => Vec::new(),
        Script::ForeignAfterFirst => vec![
            first,
            json!({
                "type": "chunk",
                "data": STANDARD.encode(&chunks[1]),
                "done": false,
                "status_code": 206,
                "step_time": 1,
                "context_id": "never-opened",
            })
            .to_string(),
        ],
    }
}

/// A client message, read.
enum Client {
    Request(Value),
    Cancel(String),
    Invalid,
}

/// Checks one client message against the protocol, recording every departure in `problems`.
fn read_client(
    text: &str,
    format: OutputFormat,
    opened: &mut HashSet<String>,
    problems: &mut Vec<String>,
) -> Client {
    if text.contains(KEY) {
        problems.push("a message carries the key".to_owned());
    }
    let Ok(Value::Object(message)) = serde_json::from_str::<Value>(text) else {
        problems.push("a message that is not a JSON object".to_owned());
        return Client::Invalid;
    };
    let Some(Value::String(context)) = message.get("context_id") else {
        problems.push("a message without a string context_id".to_owned());
        return Client::Invalid;
    };
    if message.contains_key("cancel") {
        if message.len() != 2 || message.get("cancel") != Some(&Value::Bool(true)) {
            problems.push(format!(
                "a cancel that is not exactly the cancel of {context}"
            ));
        }
        if !opened.contains(context) {
            problems.push(format!("a cancel of {context}, which was never opened"));
        }
        return Client::Cancel(context.clone());
    }
    if !opened.insert(context.clone()) {
        problems.push(format!("context {context} is used twice"));
    }
    if !message.get("model_id").is_some_and(Value::is_string) {
        problems.push("a request without a model_id".to_owned());
    }
    if !message
        .get("transcript")
        .and_then(Value::as_str)
        .is_some_and(|transcript| !transcript.trim().is_empty())
    {
        problems.push("a request without a transcript".to_owned());
    }
    if message.get("voice") != Some(&Value::from(VOICE)) {
        problems.push("a request not naming the fixture voice".to_owned());
    }
    let expected = json!({
        "container": "raw",
        "encoding": format.encoding_name(),
        "sample_rate": format.sample_rate(),
    });
    if message.get("output_format") != Some(&expected) {
        problems.push(format!(
            "output_format {:?}, expected {expected}",
            message.get("output_format")
        ));
    }
    if message.get("continue") != Some(&Value::Bool(false)) {
        problems.push("a request without continue false".to_owned());
    }
    for field in ["api_key", "x-api-key", "authorization", "access_token"] {
        if message.contains_key(field) {
            problems.push(format!("a request carries {field}"));
        }
    }
    Client::Request(Value::Object(message))
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

/// One Committed segment per text, emitted in order by one fresh generation.
pub fn committed(texts: &[&str]) -> Vec<OutputSegment> {
    let (generation, _) = Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    });
    texts
        .iter()
        .zip(1..)
        .map(|(text, sequence)| {
            let (tentative, _) = OutputSegment::emit(&generation, sequence, (*text).to_owned())
                .expect("a fresh generation is Active");
            tentative.commit().expect("a Tentative segment commits").0
        })
        .collect()
}

/// The tone the server speaks: a 400 Hz triangle wave of amplitude 8000, 200 ms of it at
/// `rate`, in integer arithmetic so that it is the same on every machine.
pub fn tone(rate: u32) -> Vec<i16> {
    let period = i64::from(rate / 400);
    let amplitude = 8_000_i64;
    (0..i64::from(rate / 5))
        .map(|n| {
            let phase = n % period;
            let value = if phase * 2 < period {
                -amplitude + 4 * amplitude * phase / period
            } else {
                3 * amplitude - 4 * amplitude * phase / period
            };
            i16::try_from(value).expect("within the amplitude")
        })
        .collect()
}

/// The bytes of the tone in `format`.
pub fn tone_bytes(format: OutputFormat) -> Vec<u8> {
    let tone = tone(format.sample_rate());
    match format.encoding() {
        AudioEncoding::PcmMulaw => tone.into_iter().map(encode_mulaw).collect(),
        AudioEncoding::PcmAlaw => tone.into_iter().map(encode_alaw).collect(),
        AudioEncoding::PcmS16le => tone.into_iter().flat_map(i16::to_le_bytes).collect(),
        AudioEncoding::PcmF32le => tone
            .into_iter()
            .flat_map(|sample| (f32::from(sample) / 32_768.0).to_le_bytes())
            .collect(),
    }
}

/// The tone in `format` cut into four chunks of a quarter each, the first one byte longer and
/// the last one byte shorter, so that for the PCM encodings two chunk boundaries split a sample.
pub fn tone_chunks(format: OutputFormat) -> Vec<Vec<u8>> {
    let bytes = tone_bytes(format);
    let quarter = bytes.len() / 4;
    let cuts = [
        0,
        quarter + 1,
        2 * quarter + 1,
        3 * quarter + 1,
        bytes.len(),
    ];
    cuts.windows(2)
        .map(|pair| bytes[pair[0]..pair[1]].to_vec())
        .collect()
}

/// Runs `body` on a thread of its own and fails the test when it has not returned within
/// [`DEADLINE`], so a hang fails instead of holding the session.
pub fn within<T: Send + 'static>(body: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let _ = tx.send(body());
    });
    match rx.recv_timeout(DEADLINE) {
        Ok(value) => {
            let _ = thread.join();
            value
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => match thread.join() {
            Err(panic) => std::panic::resume_unwind(panic),
            Ok(()) => panic!("the test body returned nothing"),
        },
        Err(mpsc::RecvTimeoutError::Timeout) => panic!("the test ran past {DEADLINE:?}"),
    }
}
