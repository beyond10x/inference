//! A local WebSocket fixture server speaking the single-stream protocol of `src/protocol.rs`, the
//! fixtures it serves, and helpers the test targets share.
//!
//! The server listens on 127.0.0.1 at a port the system picks. Each connection is checked
//! against the protocol: the `xi-api-key` header carries the fixture key, the path and query name
//! the voice, model and format, the first message is a single space, no message carries a key,
//! every text message ends in exactly one space and flushes, and the last is the end of input.
//! Every departure is recorded as a violation. It then answers with the scripted messages of a
//! recorded fixture under `tests/fixtures/`, as its [`Script`] says.
#![allow(dead_code)]

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

/// The key the fixture server expects; no real key appears in any test.
pub const KEY: &str = "fixture-key-d41d8cd98f00b204";

/// The voice the fixture server expects.
pub const VOICE: &str = "fixturevoice01";

/// What the server does once a connection's input has ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Script {
    /// Sends every message of the fixture, the final one last, then waits for the close.
    Serve,
    /// Sends every audio message of the fixture and no final one, then waits for the close.
    NoFinal,
    /// Sends the first audio message, then closes the connection with code 1011.
    CloseAfterFirst,
    /// Sends the first audio message, then this text, then waits for the close.
    SendAfterFirst(&'static str),
    /// Sends nothing, and waits for the close.
    Silent,
    /// Refuses the handshake with HTTP 401.
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
    /// A server answering every connection with `script`, from the fixture of `format`.
    pub fn start(format: OutputFormat, script: Script) -> Self {
        Self::scripted(format, vec![script])
    }

    /// A server answering its n-th connection with `scripts[n]`, the last script for every
    /// connection past them.
    pub fn scripted(format: OutputFormat, scripts: Vec<Script>) -> Self {
        let lines: Vec<String> = fixture(format).lines().map(str::to_owned).collect();
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
                let lines = Arc::new(lines);
                let mut stop_rx = stop_rx;
                let mut index = 0;
                loop {
                    let accepted = tokio::select! {
                        _ = &mut stop_rx => break,
                        accepted = listener.accept() => accepted,
                    };
                    let Ok((stream, _)) = accepted else { break };
                    let script = scripts[index.min(scripts.len() - 1)];
                    thread_seen.lock().unwrap().push(Seen::default());
                    tokio::spawn(serve(
                        stream,
                        index,
                        script,
                        Arc::clone(&lines),
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

/// The handshake callback's error type is tungstenite's `ErrorResponse`, which its `Callback`
/// trait fixes, so the size of that error is not this function's to change.
#[allow(clippy::result_large_err)]
async fn serve(
    stream: tokio::net::TcpStream,
    index: usize,
    script: Script,
    lines: Arc<Vec<String>>,
    seen: Arc<Mutex<Vec<Seen>>>,
    closes: mpsc::Sender<usize>,
    expected_query: String,
) {
    let record = |update: &dyn Fn(&mut Seen)| update(&mut seen.lock().unwrap()[index]);
    let head = Arc::new(Mutex::new((String::new(), String::new(), None)));
    let callback_head = Arc::clone(&head);
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
        if script == Script::Refuse {
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
    let Ok(mut socket) = accepted else { return };
    let mut violations = Vec::new();
    if key.as_deref() != Some(KEY) {
        violations.push("the xi-api-key header is not the fixture key".to_owned());
    }
    if path != format!("/v1/text-to-speech/{VOICE}/stream-input") {
        violations.push(format!("path {path}"));
    }
    if !query.split('&').any(|pair| pair == expected_query) || !query.contains("model_id=") {
        violations.push(format!("query {query}"));
    }

    let mut messages = Vec::new();
    let mut ended = false;
    let mut client_closed = false;
    while !ended {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => {
                let text = text.as_str().to_owned();
                check(&text, messages.len(), &mut violations, &mut ended);
                messages.push(text);
            }
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
            _ => {
                client_closed = true;
                break;
            }
        }
    }
    record(&|seen| {
        seen.messages.clone_from(&messages);
        seen.violations.clone_from(&violations);
    });
    if client_closed {
        record(&|seen| seen.client_closed = true);
        let _ = closes.send(index);
        return;
    }

    let audio: Vec<&String> = lines
        .iter()
        .filter(|line| !line.contains("isFinal"))
        .collect();
    let send_all = |count: usize, with_final: bool| {
        let mut out: Vec<String> = audio
            .iter()
            .take(count)
            .map(|line| (*line).clone())
            .collect();
        if with_final {
            out.extend(
                lines
                    .iter()
                    .filter(|line| line.contains("isFinal"))
                    .cloned(),
            );
        }
        out
    };
    let (outgoing, then_close) = match script {
        Script::Serve => (send_all(audio.len(), true), false),
        Script::NoFinal => (send_all(audio.len(), false), false),
        Script::CloseAfterFirst => (send_all(1, false), true),
        Script::SendAfterFirst(raw) => {
            let mut out = send_all(1, false);
            out.push(raw.to_owned());
            (out, false)
        }
        Script::Silent | Script::Refuse => (Vec::new(), false),
    };
    for line in outgoing {
        if socket.send(Message::text(line)).await.is_err() {
            break;
        }
    }
    if then_close {
        let _ = socket
            .close(Some(tokio_tungstenite::tungstenite::protocol::CloseFrame {
                code: 1011.into(),
                reason: "fixture closes mid-segment".into(),
            }))
            .await;
        return;
    }
    loop {
        match socket.next().await {
            Some(Ok(Message::Close(_)) | Err(_)) | None => break,
            Some(Ok(Message::Text(text))) => {
                let text = text.as_str().to_owned();
                record(&|seen| {
                    seen.messages.push(text.clone());
                    seen.violations
                        .push("a message after the end of input".to_owned());
                });
            }
            Some(Ok(_)) => {}
        }
    }
    record(&|seen| seen.client_closed = true);
    let _ = closes.send(index);
}

/// Checks the `position`-th message of a connection against the protocol.
fn check(text: &str, position: usize, violations: &mut Vec<String>, ended: &mut bool) {
    let Ok(Value::Object(message)) = serde_json::from_str::<Value>(text) else {
        violations.push(format!("message {position} is not a JSON object"));
        return;
    };
    for key_field in ["xi-api-key", "xi_api_key", "authorization"] {
        if message.contains_key(key_field) {
            violations.push(format!("message {position} carries {key_field}"));
        }
    }
    if text.contains(KEY) {
        violations.push(format!("message {position} carries the key"));
    }
    let Some(Value::String(said)) = message.get("text") else {
        violations.push(format!("message {position} has no text"));
        return;
    };
    if position == 0 {
        if said != " " {
            violations.push("the first message's text is not a single space".to_owned());
        }
        return;
    }
    if said.is_empty() {
        *ended = true;
        if message.len() != 1 {
            violations.push("the end of input carries more than its text".to_owned());
        }
        return;
    }
    if !said.ends_with(' ') || said.ends_with("  ") {
        violations.push(format!(
            "message {position} does not end in exactly one space"
        ));
    }
    if message.get("flush") != Some(&Value::Bool(true)) {
        violations.push(format!("message {position} does not flush"));
    }
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
