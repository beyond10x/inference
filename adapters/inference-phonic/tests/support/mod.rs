//! A local WebSocket fixture server speaking the conversation protocol of `src/protocol.rs`, the
//! server messages it scripts, and a wall-clock deadline for each test.
//!
//! The server listens on 127.0.0.1 at a port the system picks and answers its n-th connection
//! with the n-th [`Script`] (the last one for every connection past them). It records the path,
//! query and `Authorization` header of the handshake, every text message the client sends with
//! the instant it arrived, the WebSocket pings it read, when it sent
//! `ready_to_start_conversation`, and when the client closed the connection. A first message that
//! is not `config`, or an `audio_chunk` before `ready_to_start_conversation`, is recorded as a
//! violation.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

/// The key the tests' credential source gives.
pub const KEY: &str = "phonic-fixture-key-0123456789";

/// The longest a test may run before it fails instead of hanging.
pub const DEADLINE: Duration = Duration::from_secs(20);

/// Runs `body` on a thread of its own and fails when it has not returned within [`DEADLINE`];
/// a panic of `body` is the test's.
pub fn bounded(body: impl FnOnce() + Send + 'static) {
    let (done, finished) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        body();
        let _ = done.send(());
    });
    match finished.recv_timeout(DEADLINE) {
        Ok(()) => thread.join().expect("the test body returned"),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            panic!("the test ran past its deadline of {DEADLINE:?}")
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => match thread.join() {
            Ok(()) => {}
            Err(panic) => std::panic::resume_unwind(panic),
        },
    }
}

/// One step the server takes after `ready_to_start_conversation`.
#[derive(Debug, Clone)]
pub enum Step {
    /// Sends this JSON value as a text message.
    Send(Value),
    /// Sends this text as it is.
    Text(String),
    /// Waits until the client has sent this many `audio_chunk` messages in all.
    AwaitAudio(usize),
    /// Waits until the client has sent a message whose `type` is this.
    AwaitType(&'static str),
    /// Waits this long.
    Pause(Duration),
    /// Closes the connection with this code.
    Close(u16),
}

/// How the server answers the opening handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handshake {
    Answer,
    /// Answers the upgrade with this HTTP status.
    Refuse(u16),
}

/// What the server does with one connection.
#[derive(Debug, Clone)]
pub struct Script {
    pub handshake: Handshake,
    /// Whether the server sends `conversation_created` and `ready_to_start_conversation` once
    /// `config` arrived.
    pub ready: bool,
    /// How long the server waits after `config` before it sends them.
    pub ready_delay: Duration,
    /// A deaf server reads nothing, so answers no ping: it sends the ready messages at once,
    /// then takes its steps.
    pub deaf: bool,
    pub steps: Vec<Step>,
}

impl Script {
    /// Answers the handshake and `config` at once, then takes `steps`.
    pub fn new(steps: Vec<Step>) -> Self {
        Self {
            handshake: Handshake::Answer,
            ready: true,
            ready_delay: Duration::ZERO,
            deaf: false,
            steps,
        }
    }
}

/// What the server saw of one connection.
#[derive(Debug, Clone, Default)]
pub struct Seen {
    pub path: String,
    pub query: String,
    pub authorization: Option<String>,
    /// Every text message the client sent, parsed where it is JSON, with when it arrived.
    pub messages: Vec<(Instant, Value)>,
    pub pings: usize,
    pub ready_sent: Option<Instant>,
    /// When the client closed the connection: a close frame, the end of the stream or an error.
    pub closed: Option<Instant>,
    pub violations: Vec<String>,
}

impl Seen {
    /// The messages whose `type` is `kind`.
    pub fn of_type(&self, kind: &str) -> Vec<Value> {
        self.messages
            .iter()
            .filter(|(_, message)| message["type"] == kind)
            .map(|(_, message)| message.clone())
            .collect()
    }

    /// The decoded audio of every `audio_chunk` the client sent.
    pub fn audio(&self) -> Vec<Vec<u8>> {
        self.of_type("audio_chunk")
            .iter()
            .map(|chunk| {
                STANDARD
                    .decode(chunk["audio"].as_str().unwrap_or_default())
                    .unwrap_or_default()
            })
            .collect()
    }
}

struct Connection {
    seen: Mutex<Seen>,
    changed: Notify,
}

/// A running fixture server; it stops when dropped.
pub struct PhonicServer {
    /// `ws://127.0.0.1:<port>`, the base URL to connect to.
    pub url: String,
    connections: Arc<Mutex<Vec<Arc<Connection>>>>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl PhonicServer {
    /// A server answering every connection with `script`.
    pub fn start(script: Script) -> Self {
        Self::scripted(vec![script])
    }

    /// A server answering its n-th connection with `scripts[n]`, the last for every one past.
    pub fn scripted(scripts: Vec<Script>) -> Self {
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        std_listener.set_nonblocking(true).expect("non-blocking");
        let url = format!("ws://{}", std_listener.local_addr().expect("an address"));
        let connections = Arc::new(Mutex::new(Vec::new()));
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let thread_connections = Arc::clone(&connections);
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
                    let connection = Arc::new(Connection {
                        seen: Mutex::new(Seen::default()),
                        changed: Notify::new(),
                    });
                    thread_connections
                        .lock()
                        .unwrap()
                        .push(Arc::clone(&connection));
                    tokio::spawn(serve(stream, script, connection));
                    index += 1;
                }
            });
        });
        Self {
            url,
            connections,
            stop: Some(stop_tx),
            thread: Some(thread),
        }
    }

    /// How many connections the server accepted.
    pub fn connections(&self) -> usize {
        self.connections.lock().unwrap().len()
    }

    /// What the server saw of connection `index` so far.
    pub fn seen(&self, index: usize) -> Seen {
        self.connections.lock().unwrap()[index]
            .seen
            .lock()
            .unwrap()
            .clone()
    }

    /// Waits at most `within` for connection `index` to exist and satisfy `done`, and returns
    /// what was seen of it then, satisfied or not.
    pub fn wait(&self, index: usize, within: Duration, done: impl Fn(&Seen) -> bool) -> Seen {
        let deadline = Instant::now() + within;
        loop {
            let seen = (self.connections() > index).then(|| self.seen(index));
            match seen {
                Some(seen) if done(&seen) || Instant::now() >= deadline => return seen,
                None if Instant::now() >= deadline => return Seen::default(),
                _ => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    }
}

impl Drop for PhonicServer {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn closed(connection: &Connection) {
    connection.seen.lock().unwrap().closed = Some(Instant::now());
    connection.changed.notify_waiters();
}

/// The handshake callback's error type is tungstenite's `ErrorResponse`, which its `Callback`
/// trait fixes, so the size of that error is not this function's to change.
#[allow(clippy::result_large_err)]
async fn serve(stream: TcpStream, script: Script, connection: Arc<Connection>) {
    let refuse = match script.handshake {
        Handshake::Refuse(status) => Some(status),
        Handshake::Answer => None,
    };
    let recorder = Arc::clone(&connection);
    let callback = move |request: &Request, response: Response| {
        {
            let mut seen = recorder.seen.lock().unwrap();
            seen.path = request.uri().path().to_owned();
            seen.query = request.uri().query().unwrap_or_default().to_owned();
            seen.authorization = request
                .headers()
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
        }
        match refuse {
            Some(status) => {
                let mut refusal = ErrorResponse::new(Some("refused".to_owned()));
                *refusal.status_mut() = StatusCode::from_u16(status).expect("a status");
                Err(refusal)
            }
            None => Ok(response),
        }
    };
    let Ok(socket) = tokio_tungstenite::accept_hdr_async(stream, callback).await else {
        closed(&connection);
        return;
    };
    let (mut sink, mut source) = socket.split();
    if script.deaf {
        for message in ready_messages() {
            if sink.send(Message::text(message.to_string())).await.is_err() {
                return;
            }
        }
        connection.seen.lock().unwrap().ready_sent = Some(Instant::now());
        run(&mut sink, &connection, script.steps).await;
        tokio::time::sleep(Duration::from_secs(10)).await;
        drop(source);
        return;
    }
    let reader_connection = Arc::clone(&connection);
    let reader = tokio::spawn(async move {
        while let Some(message) = source.next().await {
            match message {
                Ok(Message::Text(text)) => {
                    let value = serde_json::from_str::<Value>(text.as_str())
                        .unwrap_or_else(|_| Value::String(text.as_str().to_owned()));
                    let mut seen = reader_connection.seen.lock().unwrap();
                    if seen.messages.is_empty() && value["type"] != "config" {
                        seen.violations
                            .push(format!("the first message is not config: {value}"));
                    }
                    if value["type"] == "audio_chunk" && seen.ready_sent.is_none() {
                        seen.violations
                            .push("an audio_chunk before ready_to_start_conversation".to_owned());
                    }
                    seen.messages.push((Instant::now(), value));
                }
                Ok(Message::Ping(_)) => reader_connection.seen.lock().unwrap().pings += 1,
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(_) => {}
            }
            reader_connection.changed.notify_waiters();
        }
        closed(&reader_connection);
    });

    if !until(&connection, |seen| !seen.messages.is_empty()).await {
        return;
    }
    if script.ready {
        tokio::time::sleep(script.ready_delay).await;
        for message in ready_messages() {
            if message["type"] == "ready_to_start_conversation" {
                connection.seen.lock().unwrap().ready_sent = Some(Instant::now());
            }
            if sink.send(Message::text(message.to_string())).await.is_err() {
                return;
            }
        }
        run(&mut sink, &connection, script.steps).await;
    }
    let _ = tokio::time::timeout(Duration::from_secs(15), reader).await;
}

type Sink = futures_util::stream::SplitSink<tokio_tungstenite::WebSocketStream<TcpStream>, Message>;

/// Takes `steps` in order; stops at a close, a failed send or a wait that came to nothing.
async fn run(sink: &mut Sink, connection: &Connection, steps: Vec<Step>) {
    for step in steps {
        let sent = match step {
            Step::Send(value) => sink.send(Message::text(value.to_string())).await,
            Step::Text(text) => sink.send(Message::text(text)).await,
            Step::AwaitAudio(count) => {
                if !until(connection, |seen| {
                    seen.of_type("audio_chunk").len() >= count
                })
                .await
                {
                    return;
                }
                Ok(())
            }
            Step::AwaitType(kind) => {
                if !until(connection, |seen| !seen.of_type(kind).is_empty()).await {
                    return;
                }
                Ok(())
            }
            Step::Pause(duration) => {
                tokio::time::sleep(duration).await;
                Ok(())
            }
            Step::Close(code) => {
                let _ = sink
                    .send(Message::Close(Some(CloseFrame {
                        code: CloseCode::from(code),
                        reason: "fixture close".into(),
                    })))
                    .await;
                return;
            }
        };
        if sent.is_err() {
            return;
        }
    }
}

/// What the server sends once `config` arrived: the conversation, then readiness.
fn ready_messages() -> [Value; 2] {
    [
        json!({"type": "conversation_created", "conversation_id": "conv_fixture"}),
        json!({"type": "ready_to_start_conversation"}),
    ]
}

/// Waits at most five seconds for `done`; false when the client closed first or time ran out.
async fn until(connection: &Connection, done: impl Fn(&Seen) -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let notified = connection.changed.notified();
        {
            let seen = connection.seen.lock().unwrap();
            if done(&seen) {
                return true;
            }
            if seen.closed.is_some() {
                return false;
            }
        }
        if tokio::time::timeout_at(deadline, notified).await.is_err() {
            return false;
        }
    }
}

/// A message of `kind` and no other field.
pub fn bare(kind: &str) -> Value {
    json!({"type": kind})
}

/// An `audio_chunk` of `bytes` bytes of assistant audio saying `text`.
pub fn audio(bytes: usize, text: &str) -> Value {
    json!({
        "type": "audio_chunk",
        "audio": STANDARD.encode(vec![7_u8; bytes]),
        "text": text,
    })
}

pub fn input_text(text: &str) -> Value {
    json!({"type": "input_text", "language": "en", "text": text})
}

pub fn interrupted(text: &str) -> Value {
    json!({"type": "interrupted_response", "text": text})
}

pub fn error(message: &str, code: &str) -> Value {
    json!({"type": "error", "error": {"message": message, "code": code}})
}

pub fn warning(message: &str) -> Value {
    json!({"type": "warning", "warning": {"message": message}})
}

pub fn tool_call(name: &str, call_id: &str) -> Value {
    json!({
        "type": "tool_call",
        "tool_call_id": call_id,
        "tool_name": name,
        "parameters": {"zone": "UTC"},
    })
}
