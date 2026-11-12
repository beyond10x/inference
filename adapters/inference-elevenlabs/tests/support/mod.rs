//! A local WebSocket fixture server speaking the multi-stream protocol of `src/protocol.rs`, the
//! fixtures it serves, and helpers the test targets share.
//!
//! The server listens on 127.0.0.1 at a port the system picks. Each connection is checked
//! against the protocol: the `xi-api-key` header carries the fixture key, the path and query name
//! the voice, model and format, and no message carries a key. Every context opens with a single
//! space and its `context_id` and gets one text ending in exactly one space with `flush` set; a
//! `close_context` comes only after that text and at most once; a context name is used once;
//! `close_socket` is the last message. Every departure is recorded as a violation. Each context,
//! once its text is flushed, is answered with the scripted messages of a recorded fixture under
//! `tests/fixtures/`, each naming the context in `contextId`, as its [`Script`] says; the n-th
//! context the server answers, across its connections, gets the n-th script.
//!
//! The server generates as the hosted endpoint was measured to (`src/lib.rs`,
//! `tests/fixtures/tts-multi-stream-protocol.md`): a `close_context` does not cut a flushed
//! context, whose audio the server sends whole; the final message follows the last audio of a
//! closed context; a flushed context that is never closed gets no final message. A
//! [`FixtureServer::paced`] server waits before the first audio message of each context and
//! between the others, so a test can measure the time to first audio and land a cancel
//! mid-segment.
#![allow(dead_code)]

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use inference_elevenlabs::{
    Credential, CredentialError, CredentialFuture, CredentialSource, ElevenLabsSynthesiser,
    Endpoint, OutputFormat, VoiceConfig,
};
use inference_tts::inference_audio::{AudioEncoding, encode_alaw, encode_mulaw};
use inference_tts::inference_core::generation::{Generation, StartGeneration};
use inference_tts::inference_core::id::{NodeId, RunId};
use inference_tts::inference_text::OutputSegment;
use serde_json::Value;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;

/// The key the fixture server expects; no real key appears in any test.
pub const KEY: &str = "fixture-key-d41d8cd98f00b204";

/// The voice the fixture server expects.
pub const VOICE: &str = "fixturevoice01";

/// What the server does once the client has flushed a context's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Script {
    /// Sends every audio message of the fixture, a close or not, and the context's final message
    /// after the last of them once the client has closed the context: as the endpoint does, a
    /// flushed context ends only when it is closed, and a close cuts none of its audio.
    Serve,
    /// Sends every audio message of the fixture and no final message, closed or not.
    NoFinal,
    /// Sends the first audio message, then closes the connection with code 1011.
    CloseAfterFirst,
    /// Sends nothing and closes the connection with code 1011.
    CloseAtOnce,
    /// Sends the first audio message, then this text, with the context's `contextId` added when
    /// it is a JSON object that names none.
    SendAfterFirst(&'static str),
    /// Sends the first audio message, then this text exactly as it is.
    SendBareAfterFirst(&'static str),
    /// Sends nothing.
    Silent,
    /// Refuses the handshake with HTTP 401, when this is the script of the next context.
    Refuse,
}

/// What the server saw of one connection.
#[derive(Debug, Clone, Default)]
pub struct Seen {
    pub path: String,
    pub query: String,
    pub key: Option<String>,
    pub messages: Vec<String>,
    pub violations: Vec<String>,
    /// The contexts the client flushed, in order, each answered by its script.
    pub contexts: Vec<String>,
    /// The contexts the client closed with `close_context`, in order.
    pub closed: Vec<String>,
    /// The client sent `close_socket`.
    pub close_socket: bool,
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

/// What one connection's task shares with the server: the fixture lines, the scripts, how many
/// contexts have been answered so far, the wait before the first audio message of a context and
/// the wait before each later one.
struct Plan {
    lines: Vec<String>,
    scripts: Vec<Script>,
    answered: Mutex<usize>,
    first_delay: Duration,
    pace: Duration,
}

impl Plan {
    /// The script of the next context to be answered.
    fn next_script(&self) -> Script {
        let answered = *self.answered.lock().unwrap();
        self.scripts[answered.min(self.scripts.len() - 1)]
    }

    /// The script of a context being answered now.
    fn take_script(&self) -> Script {
        let mut answered = self.answered.lock().unwrap();
        let script = self.scripts[(*answered).min(self.scripts.len() - 1)];
        *answered += 1;
        script
    }
}

impl FixtureServer {
    /// A server answering every context with `script`, from the fixture of `format`.
    pub fn start(format: OutputFormat, script: Script) -> Self {
        Self::scripted(format, vec![script])
    }

    /// A server answering its n-th context with `scripts[n]`, the last script for every context
    /// past them.
    pub fn scripted(format: OutputFormat, scripts: Vec<Script>) -> Self {
        Self::paced(format, scripts, Duration::ZERO, Duration::ZERO)
    }

    /// [`FixtureServer::scripted`], waiting `first_delay` after a context's text is flushed
    /// before its first audio message, and `pace` before each later one.
    pub fn paced(
        format: OutputFormat,
        scripts: Vec<Script>,
        first_delay: Duration,
        pace: Duration,
    ) -> Self {
        let plan = Arc::new(Plan {
            lines: fixture(format).lines().map(str::to_owned).collect(),
            scripts,
            answered: Mutex::new(0),
            first_delay,
            pace,
        });
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        std_listener.set_nonblocking(true).expect("non-blocking");
        let url = format!("ws://{}", std_listener.local_addr().expect("an address"));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (close_tx, close_rx) = mpsc::channel();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let thread_seen = Arc::clone(&seen);
        let expected_query = format!("output_format={}", format.as_str());
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
                    thread_seen.lock().unwrap().push(Seen::default());
                    tokio::spawn(serve(
                        stream,
                        index,
                        Arc::clone(&plan),
                        Arc::clone(&thread_seen),
                        close_tx.clone(),
                        expected_query.clone(),
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
            .with_read_timeout(Duration::from_secs(5))
            .with_connect_timeout(Duration::from_secs(5))
    }

    /// A synthesiser of the fixture voice in `format` at this server, with the fixture key.
    pub fn synthesiser(&self, format: OutputFormat) -> ElevenLabsSynthesiser {
        ElevenLabsSynthesiser::new(
            self.endpoint(),
            VoiceConfig::new(VOICE, format),
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

/// One thing the server's writer sends.
enum Out {
    Text(String),
    /// A close frame with this code and reason; the writer stops after it.
    Close(u16, &'static str),
}

/// What the client has done with one context.
#[derive(Debug, Default)]
struct Context {
    flushed: bool,
    closed: bool,
}

/// What a client message asks of the server, once checked.
enum Asked {
    Nothing,
    Answer(String),
    Close(String),
    CloseSocket,
}

/// The handshake callback's error type is tungstenite's `ErrorResponse`, which its `Callback`
/// trait fixes, so the size of that error is not this function's to change.
#[allow(clippy::result_large_err)]
async fn serve(
    stream: tokio::net::TcpStream,
    index: usize,
    plan: Arc<Plan>,
    seen: Arc<Mutex<Vec<Seen>>>,
    closes: mpsc::Sender<usize>,
    expected_query: String,
) {
    let record = |update: &dyn Fn(&mut Seen)| update(&mut seen.lock().unwrap()[index]);
    let head = Arc::new(Mutex::new((String::new(), String::new(), None)));
    let callback_head = Arc::clone(&head);
    let refuse = plan.next_script() == Script::Refuse;
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
        if refuse {
            let mut refusal = ErrorResponse::new(Some("invalid_api_key".to_owned()));
            *refusal.status_mut() = StatusCode::UNAUTHORIZED;
            return Err(refusal);
        }
        Ok(response)
    };
    let accepted = tokio_tungstenite::accept_hdr_async(stream, callback).await;
    let (path, query, key) = head.lock().unwrap().clone();
    record(&|seen| {
        seen.path.clone_from(&path);
        seen.query.clone_from(&query);
        seen.key.clone_from(&key);
    });
    let Ok(socket) = accepted else { return };
    let mut violations = Vec::new();
    if key.as_deref() != Some(KEY) {
        violations.push("the xi-api-key header is not the fixture key".to_owned());
    }
    if path != format!("/v1/text-to-speech/{VOICE}/multi-stream-input") {
        violations.push(format!("path {path}"));
    }
    if !query.split('&').any(|pair| pair == expected_query) || !query.contains("model_id=") {
        violations.push(format!("query {query}"));
    }
    record(&|seen| seen.violations.clone_from(&violations));

    let (mut sink, mut stream) = socket.split();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<Out>();
    let writer = tokio::spawn(async move {
        while let Some(out) = out_rx.recv().await {
            match out {
                Out::Text(text) => {
                    if sink.send(Message::text(text)).await.is_err() {
                        return;
                    }
                }
                Out::Close(code, reason) => {
                    let _ = sink
                        .send(Message::Close(Some(CloseFrame {
                            code: code.into(),
                            reason: reason.into(),
                        })))
                        .await;
                    return;
                }
            }
        }
    });

    let mut contexts: HashMap<String, Context> = HashMap::new();
    let mut answering: HashMap<String, tokio::sync::watch::Sender<bool>> = HashMap::new();
    let mut tasks = Vec::new();
    let mut socket_closed = false;
    let mut position = 0;
    loop {
        match stream.next().await {
            Some(Ok(Message::Text(text))) => {
                let text = text.as_str().to_owned();
                let mut found = Vec::new();
                let asked = if socket_closed {
                    found.push(format!("message {position} after close_socket"));
                    Asked::Nothing
                } else {
                    check(&text, position, &mut contexts, &mut found)
                };
                position += 1;
                record(&|seen| {
                    seen.messages.push(text.clone());
                    seen.violations.extend(found.iter().cloned());
                });
                match asked {
                    Asked::Nothing => {}
                    Asked::Answer(context_id) => {
                        record(&|seen| seen.contexts.push(context_id.clone()));
                        let (closed_tx, closed_rx) = tokio::sync::watch::channel(false);
                        tasks.push(tokio::spawn(answer(
                            plan.take_script(),
                            Arc::clone(&plan),
                            context_id.clone(),
                            out_tx.clone(),
                            closed_rx,
                        )));
                        answering.insert(context_id, closed_tx);
                    }
                    Asked::Close(context_id) => {
                        record(&|seen| seen.closed.push(context_id.clone()));
                        if let Some(closed) = answering.get(&context_id) {
                            let _ = closed.send(true);
                        }
                    }
                    Asked::CloseSocket => {
                        socket_closed = true;
                        record(&|seen| seen.close_socket = true);
                        let _ = out_tx.send(Out::Close(1000, ""));
                    }
                }
            }
            Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
            Some(Ok(Message::Binary(_))) => {
                record(&|seen| seen.violations.push("a binary message".to_owned()));
            }
            Some(Ok(Message::Close(_)) | Err(_)) | None => break,
        }
    }
    for task in tasks {
        task.abort();
    }
    drop(out_tx);
    writer.abort();
    record(&|seen| seen.client_closed = true);
    let _ = closes.send(index);
}

/// Answers the context `context_id` by `script`: its audio messages, paced as the plan says and
/// never stopped by a close, then its final message as the script says.
async fn answer(
    script: Script,
    plan: Arc<Plan>,
    context_id: String,
    out: tokio::sync::mpsc::UnboundedSender<Out>,
    mut closed: tokio::sync::watch::Receiver<bool>,
) {
    let with_context = |line: &str| -> String {
        match serde_json::from_str::<Value>(line) {
            Ok(Value::Object(mut object)) => {
                object
                    .entry("contextId")
                    .or_insert_with(|| Value::String(context_id.clone()));
                Value::Object(object).to_string()
            }
            _ => line.to_owned(),
        }
    };
    let audio: Vec<String> = plan
        .lines
        .iter()
        .filter(|line| !line.contains("isFinal"))
        .map(|line| with_context(line))
        .collect();
    let final_message = serde_json::json!({"isFinal": true, "contextId": context_id}).to_string();
    let first = audio.first().cloned().unwrap_or_default();
    let send = |text: String| {
        let _ = out.send(Out::Text(text));
    };
    match script {
        Script::Serve | Script::NoFinal => {
            for (at, line) in audio.into_iter().enumerate() {
                let wait = if at == 0 { plan.first_delay } else { plan.pace };
                if !wait.is_zero() {
                    tokio::time::sleep(wait).await;
                }
                send(line);
            }
            if script == Script::Serve {
                let _ = closed.wait_for(|closed| *closed).await;
                send(final_message);
            }
        }
        Script::CloseAtOnce => {
            let _ = out.send(Out::Close(1011, "fixture closes before any audio"));
        }
        Script::CloseAfterFirst => {
            send(first);
            let _ = out.send(Out::Close(1011, "fixture closes mid-segment"));
        }
        Script::SendAfterFirst(raw) => {
            send(first);
            send(with_context(raw));
        }
        Script::SendBareAfterFirst(raw) => {
            send(first);
            send(raw.to_owned());
        }
        Script::Silent | Script::Refuse => {}
    }
}

/// Checks the `position`-th message of a connection against the protocol, given the contexts
/// the connection has seen, and says what it asks of the server.
fn check(
    text: &str,
    position: usize,
    contexts: &mut HashMap<String, Context>,
    violations: &mut Vec<String>,
) -> Asked {
    let Ok(Value::Object(message)) = serde_json::from_str::<Value>(text) else {
        violations.push(format!("message {position} is not a JSON object"));
        return Asked::Nothing;
    };
    for key_field in ["xi-api-key", "xi_api_key", "authorization"] {
        if message.contains_key(key_field) {
            violations.push(format!("message {position} carries {key_field}"));
        }
    }
    if text.contains(KEY) {
        violations.push(format!("message {position} carries the key"));
    }
    let only = |allowed: &[&str], violations: &mut Vec<String>| {
        for name in message.keys() {
            if !allowed.contains(&name.as_str()) {
                violations.push(format!("message {position} carries {name}"));
            }
        }
    };
    if message.contains_key("close_socket") {
        only(&["close_socket"], violations);
        if message.get("close_socket") != Some(&Value::Bool(true)) {
            violations.push(format!("message {position}: close_socket is not true"));
        }
        return Asked::CloseSocket;
    }
    let Some(Value::String(context_id)) = message.get("context_id") else {
        violations.push(format!("message {position} names no context_id"));
        return Asked::Nothing;
    };
    if message.contains_key("close_context") {
        only(&["context_id", "close_context"], violations);
        if message.get("close_context") != Some(&Value::Bool(true)) {
            violations.push(format!("message {position}: close_context is not true"));
        }
        let Some(context) = contexts.get_mut(context_id) else {
            violations.push(format!("message {position} closes a context never opened"));
            return Asked::Nothing;
        };
        if context.closed {
            violations.push(format!("message {position} closes a context twice"));
            return Asked::Nothing;
        }
        if !context.flushed {
            violations.push(format!("message {position} closes a context with no text"));
        }
        context.closed = true;
        return Asked::Close(context_id.clone());
    }
    let Some(Value::String(said)) = message.get("text") else {
        violations.push(format!("message {position} has no text"));
        return Asked::Nothing;
    };
    let Some(context) = contexts.get_mut(context_id) else {
        only(&["text", "context_id", "voice_settings"], violations);
        if said != " " {
            violations.push(format!(
                "message {position} opens a context with text other than a single space"
            ));
        }
        contexts.insert(context_id.clone(), Context::default());
        return Asked::Nothing;
    };
    only(&["text", "context_id", "flush"], violations);
    if context.closed {
        violations.push(format!("message {position} is sent to a closed context"));
    }
    if context.flushed {
        violations.push(format!(
            "message {position} is a second text of its context"
        ));
    }
    if said.trim().is_empty() {
        violations.push(format!("message {position} is a blank text"));
    }
    if !said.ends_with(' ') || said.ends_with("  ") {
        violations.push(format!(
            "message {position} does not end in exactly one space"
        ));
    }
    if message.get("flush") != Some(&Value::Bool(true)) {
        violations.push(format!("message {position} does not flush"));
    }
    context.flushed = true;
    Asked::Answer(context_id.clone())
}

/// Runs `work` on a thread of its own and fails the case when it does not return within `bound`.
pub fn within<T: Send + 'static>(bound: Duration, work: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    rx.recv_timeout(bound)
        .unwrap_or_else(|_| panic!("the case did not finish within {bound:?}"))
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

/// The fixture file a format is served from: the μ-law tone, the A-law tone, or the PCM16 tone
/// for every PCM rate.
pub fn fixture_name(format: OutputFormat) -> &'static str {
    match format.audio_format().encoding {
        AudioEncoding::PcmMulaw => "tone-ulaw.jsonl",
        AudioEncoding::PcmAlaw => "tone-alaw.jsonl",
        _ => "tone-pcm16.jsonl",
    }
}

/// The text of the fixture of `format`.
pub fn fixture(format: OutputFormat) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(fixture_name(format));
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The audio bytes of the fixture of `format`, each chunk decoded, in order.
pub fn fixture_chunks(format: OutputFormat) -> Vec<Vec<u8>> {
    use base64::Engine;
    fixture(format)
        .lines()
        .filter_map(|line| {
            let message: Value = serde_json::from_str(line).expect("a fixture line is JSON");
            message["audio"].as_str().map(|audio| {
                base64::engine::general_purpose::STANDARD
                    .decode(audio)
                    .expect("fixture audio is base64")
            })
        })
        .collect()
}

/// The tone the fixtures carry: a 400 Hz triangle wave of amplitude 8000, 200 ms of it at
/// `rate`, in integer arithmetic so that it is the same on every machine. Synthesized; no
/// recording.
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

/// The fixture text of `encoding`: one `audio` message per chunk and the final message. The
/// G.711 tones are 8 kHz in four chunks of 400 bytes; the PCM16 tone is 24 kHz, 9600 bytes in
/// four chunks of 2401, 2400, 2400 and 2399 bytes, so that two chunk boundaries split a sample.
pub fn fixture_text(encoding: AudioEncoding) -> String {
    use base64::Engine;
    let (bytes, sizes): (Vec<u8>, Vec<usize>) = match encoding {
        AudioEncoding::PcmMulaw => (
            tone(8_000).into_iter().map(encode_mulaw).collect(),
            vec![400; 4],
        ),
        AudioEncoding::PcmAlaw => (
            tone(8_000).into_iter().map(encode_alaw).collect(),
            vec![400; 4],
        ),
        _ => (
            tone(24_000)
                .into_iter()
                .flat_map(i16::to_le_bytes)
                .collect(),
            vec![2_401, 2_400, 2_400, 2_399],
        ),
    };
    let mut out = String::new();
    let mut start = 0;
    for size in sizes {
        let chunk = &bytes[start..start + size];
        start += size;
        out.push_str(&format!(
            "{{\"audio\":\"{}\",\"alignment\":null,\"normalizedAlignment\":null}}\n",
            base64::engine::general_purpose::STANDARD.encode(chunk)
        ));
    }
    assert_eq!(start, bytes.len(), "the chunks cover the tone");
    out.push_str("{\"audio\":null,\"isFinal\":true}\n");
    out
}
