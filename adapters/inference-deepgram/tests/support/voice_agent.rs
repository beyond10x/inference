//! A local WebSocket fixture server speaking the Voice Agent protocol of `src/agent.rs`, after
//! the message flow the documentation draws, and the server messages it scripts.
//!
//! The server listens on 127.0.0.1 at a port the system picks and answers its n-th connection
//! with the n-th [`Script`] (the last one for every connection past them). On a connection it
//! records the path and the `Authorization` header of the handshake, sends `Welcome`, waits for
//! the first client text, which must be `Settings`, answers `SettingsApplied` and then takes the
//! script's steps. It records every text message the client sends with the instant it arrived,
//! every binary message, when it sent `Welcome` and `SettingsApplied`, and when the client closed
//! the connection. A text before `Welcome`, a first text that is not `Settings` (answered with
//! `Error` `NON_SETTINGS_MESSAGE_BEFORE_SETTINGS` and a close, as the documentation describes), a
//! second `Settings`, audio before `SettingsApplied`, an empty binary message, a text carrying the
//! key and an `Authorization` header other than `Token` and the fixture key are recorded as
//! violations.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use inference_deepgram::{Credential, CredentialError, CredentialFuture, CredentialSource};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

/// The key the fixture server expects; no real key appears in any test.
pub const KEY: &str = "fixture-agent-key-7d1e4a9b03c2";

/// One step the server takes after `SettingsApplied`.
#[derive(Debug, Clone)]
pub enum Step {
    /// Sends this JSON value as a text message.
    Send(Value),
    /// Sends this text as it is.
    Text(String),
    /// Sends a binary message: agent audio.
    Binary(Vec<u8>),
    /// Waits until the client has sent this many binary messages over the connection in all.
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
    /// Accepts the TCP connection and never answers the upgrade.
    Stall,
}

/// How the server answers `Settings`.
#[derive(Debug, Clone, PartialEq)]
pub enum Applied {
    /// `SettingsApplied`, after this delay.
    After(Duration),
    /// Nothing at all.
    Never,
    /// A close frame with this code.
    Close(u16),
    /// This message, then a close frame with code 1011.
    ErrorThenClose(Value),
}

/// What the server does with one connection.
#[derive(Debug, Clone)]
pub struct Script {
    pub handshake: Handshake,
    /// How long the server waits after the handshake before it sends `Welcome`.
    pub welcome_delay: Duration,
    pub applied: Applied,
    pub steps: Vec<Step>,
}

impl Script {
    /// Answers the handshake, `Welcome` and `SettingsApplied` at once, then takes `steps`.
    pub fn new(steps: Vec<Step>) -> Self {
        Self {
            handshake: Handshake::Answer,
            welcome_delay: Duration::ZERO,
            applied: Applied::After(Duration::ZERO),
            steps,
        }
    }
}

/// What the server saw of one connection.
#[derive(Debug, Clone, Default)]
pub struct Seen {
    pub path: String,
    pub authorization: Option<String>,
    /// Every text message the client sent, parsed where it is JSON, with when it arrived.
    pub messages: Vec<(Instant, Value)>,
    /// The length of every binary message the client sent, with when it arrived.
    pub audio: Vec<(Instant, usize)>,
    pub welcome_sent: Option<Instant>,
    pub applied_sent: Option<Instant>,
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

    /// When the messages whose `type` is `kind` arrived.
    pub fn times_of(&self, kind: &str) -> Vec<Instant> {
        self.messages
            .iter()
            .filter(|(_, message)| message["type"] == kind)
            .map(|(at, _)| *at)
            .collect()
    }
}

struct Connection {
    seen: Mutex<Seen>,
    changed: Notify,
}

/// A running fixture server; it stops when dropped.
pub struct AgentServer {
    /// `ws://127.0.0.1:<port>`, the base URL to connect to.
    pub url: String,
    connections: Arc<Mutex<Vec<Arc<Connection>>>>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl AgentServer {
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

impl Drop for AgentServer {
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
async fn serve(mut stream: TcpStream, script: Script, connection: Arc<Connection>) {
    if script.handshake == Handshake::Stall {
        let mut buffer = [0_u8; 1024];
        loop {
            match tokio::time::timeout(Duration::from_secs(30), stream.read(&mut buffer)).await {
                Ok(Ok(0) | Err(_)) | Err(_) => break,
                Ok(Ok(_)) => {}
            }
        }
        closed(&connection);
        return;
    }
    let refuse = match script.handshake {
        Handshake::Refuse(status) => Some(status),
        _ => None,
    };
    let recorder = Arc::clone(&connection);
    let callback = move |request: &Request, response: Response| {
        {
            let mut seen = recorder.seen.lock().unwrap();
            seen.path = request.uri().path().to_owned();
            seen.authorization = request
                .headers()
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            if seen.authorization.as_deref() != Some(&format!("Token {KEY}")) {
                seen.violations
                    .push("the Authorization header is not `Token` and the fixture key".to_owned());
            }
        }
        match refuse {
            Some(status) => {
                let mut refusal =
                    ErrorResponse::new(Some("{\"err_code\":\"INVALID_AUTH\"}".to_owned()));
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
    let reader_connection = Arc::clone(&connection);
    let reader = tokio::spawn(async move {
        while let Some(message) = source.next().await {
            match message {
                Ok(Message::Text(text)) => {
                    let value = serde_json::from_str::<Value>(text.as_str())
                        .unwrap_or_else(|_| Value::String(text.as_str().to_owned()));
                    let mut seen = reader_connection.seen.lock().unwrap();
                    if seen.welcome_sent.is_none() {
                        seen.violations
                            .push(format!("a message before Welcome: {value}"));
                    }
                    if text.as_str().contains(KEY) {
                        seen.violations
                            .push("a text message carries the key".to_owned());
                    }
                    if seen.messages.is_empty() && value["type"] != "Settings" {
                        seen.violations
                            .push(format!("the first message is not Settings: {value}"));
                    }
                    if !seen.messages.is_empty() && value["type"] == "Settings" {
                        seen.violations.push("a second Settings".to_owned());
                    }
                    seen.messages.push((Instant::now(), value));
                }
                Ok(Message::Binary(data)) => {
                    let mut seen = reader_connection.seen.lock().unwrap();
                    if seen.applied_sent.is_none() {
                        seen.violations
                            .push("audio before SettingsApplied".to_owned());
                    }
                    if data.is_empty() {
                        seen.violations.push("an empty binary message".to_owned());
                    }
                    seen.audio.push((Instant::now(), data.len()));
                }
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(_) => {}
            }
            reader_connection.changed.notify_waiters();
        }
        closed(&reader_connection);
    });

    tokio::time::sleep(script.welcome_delay).await;
    connection.seen.lock().unwrap().welcome_sent = Some(Instant::now());
    let welcome = json!({"type": "Welcome", "request_id": "fixture-request-0001"});
    if sink.send(Message::text(welcome.to_string())).await.is_err() {
        return;
    }
    if !until(&connection, |seen| !seen.messages.is_empty()).await {
        return;
    }
    let first_is_settings = connection.seen.lock().unwrap().messages[0].1["type"] == "Settings";
    if !first_is_settings {
        let _ = sink
            .send(Message::text(
                error(
                    "NON_SETTINGS_MESSAGE_BEFORE_SETTINGS",
                    "Settings come first",
                )
                .to_string(),
            ))
            .await;
        let _ = sink.send(close_frame(1008)).await;
    } else {
        match script.applied {
            Applied::After(delay) => {
                tokio::time::sleep(delay).await;
                connection.seen.lock().unwrap().applied_sent = Some(Instant::now());
                let applied = json!({"type": "SettingsApplied"});
                if sink.send(Message::text(applied.to_string())).await.is_err() {
                    return;
                }
                run(&mut sink, &connection, script.steps).await;
            }
            Applied::Never => {}
            Applied::Close(code) => {
                let _ = sink.send(close_frame(code)).await;
            }
            Applied::ErrorThenClose(message) => {
                let _ = sink.send(Message::text(message.to_string())).await;
                let _ = sink.send(close_frame(1011)).await;
            }
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(30), reader).await;
}

type Sink = futures_util::stream::SplitSink<tokio_tungstenite::WebSocketStream<TcpStream>, Message>;

async fn run(sink: &mut Sink, connection: &Connection, steps: Vec<Step>) {
    for step in steps {
        let sent = match step {
            Step::Send(value) => sink.send(Message::text(value.to_string())).await,
            Step::Text(text) => sink.send(Message::text(text)).await,
            Step::Binary(data) => sink.send(Message::binary(data)).await,
            Step::AwaitAudio(count) => {
                if !until(connection, |seen| seen.audio.len() >= count).await {
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
                let _ = sink.send(close_frame(code)).await;
                return;
            }
        };
        if sent.is_err() {
            return;
        }
    }
}

fn close_frame(code: u16) -> Message {
    Message::Close(Some(CloseFrame {
        code: CloseCode::from(code),
        reason: "fixture close".into(),
    }))
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

pub fn user_started_speaking() -> Value {
    json!({"type": "UserStartedSpeaking"})
}

pub fn conversation_text(role: &str, content: &str) -> Value {
    json!({"type": "ConversationText", "role": role, "content": content})
}

pub fn agent_thinking(content: &str) -> Value {
    json!({"type": "AgentThinking", "content": content})
}

/// `AgentStartedSpeaking` with its latencies as the schema types them: strings.
pub fn agent_started_speaking() -> Value {
    json!({
        "type": "AgentStartedSpeaking",
        "total_latency": "1.2",
        "tts_latency": "0.3",
        "ttt_latency": "0.6",
    })
}

pub fn agent_audio_done() -> Value {
    json!({"type": "AgentAudioDone"})
}

pub fn warning(code: &str, description: &str) -> Value {
    json!({"type": "Warning", "code": code, "description": description})
}

pub fn error(code: &str, description: &str) -> Value {
    json!({"type": "Error", "code": code, "description": description})
}

pub fn function_call(name: &str, id: &str, client_side: bool) -> Value {
    json!({
        "type": "FunctionCallRequest",
        "functions": [{
            "id": id,
            "name": name,
            "arguments": "{\"zone\":\"UTC\"}",
            "client_side": client_side,
        }]
    })
}

/// One scripted turn, as the message flow draws it: the user's words, the agent thinking, its
/// words, the start of its speech, `frames` binary frames of `bytes` bytes and the end of it.
pub fn turn(user: &str, agent: &str, frames: usize, bytes: usize) -> Vec<Step> {
    let mut steps = vec![
        Step::Send(user_started_speaking()),
        Step::Send(conversation_text("user", user)),
        Step::Send(agent_thinking("")),
        Step::Send(conversation_text("assistant", agent)),
        Step::Send(agent_started_speaking()),
    ];
    steps.extend((0..frames).map(|_| Step::Binary(vec![7; bytes])));
    steps.push(Step::Send(agent_audio_done()));
    steps
}
