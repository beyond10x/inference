//! A local WebSocket fixture server speaking the agent conversation protocol of `src/agent.rs`,
//! and the server messages it scripts.
//!
//! The server listens on 127.0.0.1 at a port the system picks and answers its n-th connection
//! with the n-th [`Script`] (the last one for every connection past them). It records the path
//! and query of the handshake, every text message the client sends with the instant it arrived,
//! when it sent the initiation metadata, and when the client closed the connection. A user audio
//! chunk before the metadata, or a first message that is not the initiation, is recorded as a
//! violation.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

/// The public agent the tests connect to.
pub const AGENT: &str = "fixture-agent-01";

/// One step the server takes after the metadata.
#[derive(Debug, Clone)]
pub enum Step {
    /// Sends this JSON value as a text message.
    Send(Value),
    /// Sends this text as it is.
    Text(String),
    /// Sends a binary message.
    Binary(Vec<u8>),
    /// Waits until the client has sent this many user audio chunks over the connection in all.
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

/// What the server does with one connection.
#[derive(Debug, Clone)]
pub struct Script {
    pub handshake: Handshake,
    /// The formats the metadata reports, the user's input then the agent's output; `None`
    /// sends no metadata.
    pub metadata: Option<(&'static str, &'static str)>,
    /// How long the server waits after the initiation before it sends the metadata.
    pub metadata_delay: Duration,
    pub steps: Vec<Step>,
}

impl Script {
    /// Answers the handshake, reports `pcm_16000` both ways at once, then takes `steps`.
    pub fn new(steps: Vec<Step>) -> Self {
        Self {
            handshake: Handshake::Answer,
            metadata: Some(("pcm_16000", "pcm_16000")),
            metadata_delay: Duration::ZERO,
            steps,
        }
    }
}

/// What the server saw of one connection.
#[derive(Debug, Clone, Default)]
pub struct Seen {
    pub path: String,
    pub query: String,
    /// Every text message the client sent, parsed where it is JSON, with when it arrived.
    pub messages: Vec<(Instant, Value)>,
    pub metadata_sent: Option<Instant>,
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

    /// The user audio chunks the client sent.
    pub fn audio_chunks(&self) -> usize {
        self.messages
            .iter()
            .filter(|(_, message)| message.get("user_audio_chunk").is_some())
            .count()
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
        connection.seen.lock().unwrap().closed = Some(Instant::now());
        connection.changed.notify_waiters();
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
            seen.query = request.uri().query().unwrap_or_default().to_owned();
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
        connection.seen.lock().unwrap().closed = Some(Instant::now());
        connection.changed.notify_waiters();
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
                    if seen.messages.is_empty()
                        && value["type"] != "conversation_initiation_client_data"
                    {
                        seen.violations
                            .push(format!("the first message is not the initiation: {value}"));
                    }
                    if value.get("user_audio_chunk").is_some() && seen.metadata_sent.is_none() {
                        seen.violations
                            .push("a user audio chunk before the metadata".to_owned());
                    }
                    seen.messages.push((Instant::now(), value));
                }
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(_) => {}
            }
            reader_connection.changed.notify_waiters();
        }
        reader_connection.seen.lock().unwrap().closed = Some(Instant::now());
        reader_connection.changed.notify_waiters();
    });

    if !until(&connection, |seen| !seen.messages.is_empty()).await {
        return;
    }
    if let Some((input, output)) = script.metadata {
        tokio::time::sleep(script.metadata_delay).await;
        let metadata = json!({
            "type": "conversation_initiation_metadata",
            "conversation_initiation_metadata_event": {
                "conversation_id": "conv_fixture",
                "agent_output_audio_format": output,
                "user_input_audio_format": input,
            }
        });
        connection.seen.lock().unwrap().metadata_sent = Some(Instant::now());
        if sink
            .send(Message::text(metadata.to_string()))
            .await
            .is_err()
        {
            return;
        }
        for step in script.steps {
            let sent = match step {
                Step::Send(value) => sink.send(Message::text(value.to_string())).await,
                Step::Text(text) => sink.send(Message::text(text)).await,
                Step::Binary(data) => sink.send(Message::binary(data)).await,
                Step::AwaitAudio(count) => {
                    if !until(&connection, |seen| seen.audio_chunks() >= count).await {
                        return;
                    }
                    Ok(())
                }
                Step::AwaitType(kind) => {
                    if !until(&connection, |seen| !seen.of_type(kind).is_empty()).await {
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
                    break;
                }
            };
            if sent.is_err() {
                return;
            }
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(30), reader).await;
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

/// An `audio` message of `bytes` bytes of agent audio under `event_id`.
pub fn audio(event_id: u64, bytes: usize) -> Value {
    json!({
        "type": "audio",
        "audio_event": {
            "audio_base_64": STANDARD.encode(vec![7_u8; bytes]),
            "event_id": event_id,
            "is_final": false,
        }
    })
}

pub fn user_transcript(text: &str, event_id: u64) -> Value {
    json!({
        "type": "user_transcript",
        "user_transcription_event": {"user_transcript": text, "event_id": event_id}
    })
}

pub fn agent_response(text: &str, event_id: u64, response_id: Option<&str>) -> Value {
    let mut event = json!({"agent_response": text, "event_id": event_id});
    if let Some(response_id) = response_id {
        event["response_id"] = json!(response_id);
    }
    json!({"type": "agent_response", "agent_response_event": event})
}

pub fn correction(
    original: &str,
    corrected: &str,
    event_id: u64,
    response_id: Option<&str>,
) -> Value {
    let mut event = json!({
        "original_agent_response": original,
        "corrected_agent_response": corrected,
        "event_id": event_id,
    });
    if let Some(response_id) = response_id {
        event["response_id"] = json!(response_id);
    }
    json!({"type": "agent_response_correction", "agent_response_correction_event": event})
}

pub fn interruption(event_id: u64) -> Value {
    json!({"type": "interruption", "interruption_event": {"event_id": event_id}})
}

pub fn ping(event_id: u64) -> Value {
    json!({"type": "ping", "ping_event": {"event_id": event_id, "ping_ms": 50}})
}

pub fn client_error(code: u64, name: &str, message: &str) -> Value {
    json!({
        "type": "client_error",
        "error_event": {"code": code, "error_name": name, "message": message}
    })
}

pub fn tool_call(name: &str, call_id: &str, expects_response: bool) -> Value {
    json!({
        "type": "client_tool_call",
        "client_tool_call": {
            "tool_name": name,
            "tool_call_id": call_id,
            "parameters": {"zone": "UTC"},
            "event_id": 90,
            "expects_response": expects_response,
        }
    })
}
