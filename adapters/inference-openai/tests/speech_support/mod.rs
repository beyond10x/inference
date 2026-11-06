//! A local HTTP/1.1 fixture server speaking OpenAI's streamed speech endpoint as
//! `src/speech/protocol.rs` describes it, and helpers the speech test targets share.
//!
//! The server listens on 127.0.0.1 at a port the system picks. Each request is checked against
//! the protocol: `POST /v1/audio/speech`, the fixture key as a bearer token, a JSON body naming
//! the model, the voice, the input, `response_format` `pcm` and `stream_format` `audio`, and no
//! key in the body. Every departure is recorded as a violation. It then answers as its [`Script`]
//! says, streaming the fixture tone, 24 kHz 16-bit little-endian mono PCM, with chunked transfer
//! encoding in chunks of 2401, 2400, 2400 and 2399 bytes, so that two chunk boundaries split a
//! sample. Synthesized in integer arithmetic; no recording.
#![allow(dead_code)]

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use inference_openai::speech::{OpenAiSpeech, SpeechConfig, SpeechEndpoint, SpeechVoice};
use inference_openai::{Credential, CredentialError, CredentialFuture, CredentialSource};
use inference_tts::inference_audio::{AudioEncoding, AudioFormat};
use inference_tts::inference_core::generation::{Generation, StartGeneration};
use inference_tts::inference_core::id::{NodeId, RunId};
use inference_tts::inference_text::OutputSegment;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// The key the fixture server expects; no real key appears in any test.
pub const KEY: &str = "fixture-key-0f1e2d3c4b5a6978";

/// The voice the fixture synthesisers use.
pub const VOICE: &str = "marin";

/// The sizes of the chunks the tone is streamed in, 9600 bytes in all.
pub const CHUNKS: [usize; 4] = [2_401, 2_400, 2_400, 2_399];

/// The longest a test may run before it fails instead of hanging.
pub const DEADLINE: Duration = Duration::from_secs(25);

/// What the server answers a request with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Script {
    /// Streams every chunk of the tone and ends the body.
    Serve,
    /// Streams the first chunk, then holds the response open until the client goes.
    Stall,
    /// Streams the first chunk and ends the body there, half a sample short.
    HalfSample,
    /// Answers HTTP 401 with an error body.
    Refuse,
}

/// What the server saw of one request.
#[derive(Debug, Clone, Default)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub authorization: Option<String>,
    pub body: Option<Value>,
    pub violations: Vec<String>,
    /// The client closed the connection while the server held the response open.
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
    /// A server answering every request with `script`.
    pub fn start(script: Script) -> Self {
        Self::scripted(vec![script])
    }

    /// A server answering its n-th request with `scripts[n]`, the last script for every request
    /// past them.
    pub fn scripted(scripts: Vec<Script>) -> Self {
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        std_listener.set_nonblocking(true).expect("non-blocking");
        let url = format!(
            "http://{}/v1/audio/speech",
            std_listener.local_addr().expect("an address")
        );
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
                let index = Arc::new(Mutex::new(0_usize));
                loop {
                    let accepted = tokio::select! {
                        _ = &mut stop_rx => break,
                        accepted = listener.accept() => accepted,
                    };
                    let Ok((stream, _)) = accepted else { break };
                    tokio::spawn(serve(
                        stream,
                        scripts.clone(),
                        Arc::clone(&index),
                        Arc::clone(&thread_seen),
                        close_tx.clone(),
                    ));
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

    /// What the server saw of every request so far.
    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    /// Every violation of every request so far.
    pub fn violations(&self) -> Vec<String> {
        self.seen()
            .into_iter()
            .flat_map(|seen| seen.violations)
            .collect()
    }

    /// The `input` of every request so far, in order.
    pub fn inputs(&self) -> Vec<String> {
        self.seen()
            .into_iter()
            .filter_map(|seen| {
                seen.body
                    .and_then(|body| body["input"].as_str().map(str::to_owned))
            })
            .collect()
    }

    /// The index of the next request whose connection the client closed while the server held
    /// the response open, waiting at most `bound`.
    pub fn next_client_close(&self, bound: Duration) -> Option<usize> {
        self.closes.lock().unwrap().recv_timeout(bound).ok()
    }

    /// An endpoint at this server, with short bounds.
    pub fn endpoint(&self) -> SpeechEndpoint {
        SpeechEndpoint::new()
            .with_url(&self.url)
            .with_read_timeout(Duration::from_secs(5))
            .with_connect_timeout(Duration::from_secs(5))
    }

    /// A synthesiser of the fixture voice in `format` at this server, with the fixture key.
    pub fn synthesiser(&self, format: AudioFormat) -> OpenAiSpeech {
        self.synthesiser_with(SpeechConfig::new(
            SpeechVoice::BuiltIn(VOICE.to_owned()),
            format,
        ))
    }

    /// A synthesiser of `config` at this server, with the fixture key.
    pub fn synthesiser_with(&self, config: SpeechConfig) -> OpenAiSpeech {
        OpenAiSpeech::new(
            self.endpoint(),
            config,
            Arc::new(FixedKey(KEY)),
            "openai-api-key",
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

/// Serves the requests of one connection until the client closes it.
async fn serve(
    mut stream: TcpStream,
    scripts: Vec<Script>,
    next: Arc<Mutex<usize>>,
    seen: Arc<Mutex<Vec<Seen>>>,
    closes: mpsc::Sender<usize>,
) {
    let mut buffer = Vec::new();
    loop {
        let Some((head, body)) = read_request(&mut stream, &mut buffer).await else {
            return;
        };
        let index = {
            let mut next = next.lock().unwrap();
            let index = *next;
            *next += 1;
            index
        };
        let script = scripts[index.min(scripts.len() - 1)];
        let observed = check(&head, &body);
        seen.lock().unwrap().push(observed);
        let tone = tone_bytes();
        let ok_head = "HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\n\
                       transfer-encoding: chunked\r\n\r\n";
        match script {
            Script::Serve => {
                if stream.write_all(ok_head.as_bytes()).await.is_err() {
                    return;
                }
                let mut start = 0;
                for size in CHUNKS {
                    if write_chunk(&mut stream, &tone[start..start + size])
                        .await
                        .is_err()
                    {
                        return;
                    }
                    start += size;
                }
                if stream.write_all(b"0\r\n\r\n").await.is_err() {
                    return;
                }
            }
            Script::HalfSample => {
                let _ = stream.write_all(ok_head.as_bytes()).await;
                let _ = write_chunk(&mut stream, &tone[..CHUNKS[0]]).await;
                let _ = stream.write_all(b"0\r\n\r\n").await;
            }
            Script::Refuse => {
                let body = "{\"error\":{\"message\":\"Incorrect API key provided\",\
                            \"type\":\"invalid_request_error\",\"code\":\"invalid_api_key\"}}";
                let response = format!(
                    "HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\n\
                     content-length: {}\r\n\r\n{body}",
                    body.len()
                );
                if stream.write_all(response.as_bytes()).await.is_err() {
                    return;
                }
            }
            Script::Stall => {
                let _ = stream.write_all(ok_head.as_bytes()).await;
                let _ = write_chunk(&mut stream, &tone[..CHUNKS[0]]).await;
                let mut sink = [0_u8; 1024];
                loop {
                    match stream.read(&mut sink).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
                seen.lock().unwrap()[index].client_closed = true;
                let _ = closes.send(index);
                return;
            }
        }
        let _ = stream.flush().await;
    }
}

/// Writes one chunk of a chunked body.
async fn write_chunk(stream: &mut TcpStream, data: &[u8]) -> std::io::Result<()> {
    stream
        .write_all(format!("{:x}\r\n", data.len()).as_bytes())
        .await?;
    stream.write_all(data).await?;
    stream.write_all(b"\r\n").await?;
    stream.flush().await
}

/// The head and body of the next request on the connection, `None` once the client closed it.
async fn read_request(stream: &mut TcpStream, buffer: &mut Vec<u8>) -> Option<(String, Vec<u8>)> {
    let mut chunk = [0_u8; 4096];
    let end = loop {
        if let Some(at) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break at;
        }
        if buffer.len() > 1 << 20 {
            return None;
        }
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return None,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
        }
    };
    let head = String::from_utf8_lossy(&buffer[..end]).into_owned();
    buffer.drain(..end + 4);
    let length = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while buffer.len() < length {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return None,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
        }
    }
    let body = buffer.drain(..length).collect();
    Some((head, body))
}

/// What the server makes of one request: its method, path, key and body, and every departure
/// from the protocol.
fn check(head: &str, body: &[u8]) -> Seen {
    let mut lines = head.lines();
    let mut request_line = lines.next().unwrap_or_default().split(' ');
    let method = request_line.next().unwrap_or_default().to_owned();
    let path = request_line.next().unwrap_or_default().to_owned();
    let header = |wanted: &str| {
        head.lines()
            .skip(1)
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.trim().eq_ignore_ascii_case(wanted))
            .map(|(_, value)| value.trim().to_owned())
    };
    let authorization = header("authorization");
    let mut violations = Vec::new();
    if method != "POST" {
        violations.push(format!("method {method}"));
    }
    if path != "/v1/audio/speech" {
        violations.push(format!("path {path}"));
    }
    if authorization.as_deref() != Some(&format!("Bearer {KEY}")) {
        violations.push("the authorization header is not the fixture key".to_owned());
    }
    if !header("content-type").is_some_and(|value| value.starts_with("application/json")) {
        violations.push("the body is not declared application/json".to_owned());
    }
    if String::from_utf8_lossy(body).contains(KEY) {
        violations.push("the body carries the key".to_owned());
    }
    let parsed = serde_json::from_slice::<Value>(body).ok();
    match parsed.as_ref().and_then(Value::as_object) {
        None => violations.push("the body is not a JSON object".to_owned()),
        Some(object) => {
            for field in ["model", "input"] {
                if !object.get(field).is_some_and(Value::is_string) {
                    violations.push(format!("{field} is not a string"));
                }
            }
            if !object
                .get("voice")
                .is_some_and(|voice| voice.is_string() || voice["id"].is_string())
            {
                violations.push("voice is neither a name nor an object with an id".to_owned());
            }
            if object.get("response_format") != Some(&Value::from("pcm")) {
                violations.push("response_format is not pcm".to_owned());
            }
            if object.get("stream_format") != Some(&Value::from("audio")) {
                violations.push("stream_format is not audio".to_owned());
            }
            for field in object.keys() {
                if ![
                    "model",
                    "input",
                    "voice",
                    "response_format",
                    "stream_format",
                    "instructions",
                    "speed",
                ]
                .contains(&field.as_str())
                {
                    violations.push(format!("an undocumented field {field}"));
                }
            }
            if object
                .get("input")
                .and_then(Value::as_str)
                .is_some_and(|input| input.chars().count() > 4_096)
            {
                violations.push("the input is longer than 4096 characters".to_owned());
            }
        }
    }
    Seen {
        method,
        path,
        authorization,
        body: parsed,
        violations,
        client_closed: false,
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

/// Mono audio of `encoding` at `rate` hertz.
pub fn format(encoding: AudioEncoding, rate: u32) -> AudioFormat {
    AudioFormat {
        encoding,
        sample_rate: NonZeroU32::new(rate).expect("a rate"),
        channels: NonZeroU16::MIN,
    }
}

/// The tone the server streams: a 400 Hz triangle wave of amplitude 8000, 200 ms of it at
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

/// The bytes the server streams for every request: the 24 kHz tone, little-endian.
pub fn tone_bytes() -> Vec<u8> {
    tone(24_000)
        .into_iter()
        .flat_map(i16::to_le_bytes)
        .collect()
}

/// Runs `body` on a thread of its own and returns what it returns, failing the test when it has
/// not finished within [`DEADLINE`], so a hang fails instead of holding the build.
pub fn within_deadline<T: Send + 'static>(body: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)));
    });
    match rx.recv_timeout(DEADLINE) {
        Ok(Ok(value)) => value,
        Ok(Err(panic)) => std::panic::resume_unwind(panic),
        Err(_) => panic!("the test did not finish within {DEADLINE:?}"),
    }
}
