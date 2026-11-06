//! A local WebSocket fixture server speaking the Opal protocol of `src/protocol.rs` as the
//! documentation describes it, and the server messages it scripts.
//!
//! The server listens on 127.0.0.1 at a port the system picks and answers its n-th connection
//! with the n-th [`Script`] (the last one for every connection past them). On a connection it
//! records the path and the `Authorization` header of the handshake, waits for the first client
//! message, which must be `InitializeSessionRequest`, answers `SessionReady` and then takes the
//! script's steps. It answers every `Ping` with a `Pong`, as the definition says, unless the
//! script turns that off. It records every message the client sends with the instant it arrived
//! and when the client closed the connection. A text frame, a frame that is not a
//! `ServiceBoundMessage`, a first message that is not `InitializeSessionRequest`, a second one,
//! audio before `SessionReady`, empty audio, a frame carrying the key and an `Authorization`
//! header other than `Bearer` and the fixture key are recorded as violations.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use inference_deepslate::protocol::{ClientBound, ServiceBound};
use inference_deepslate::{Credential, CredentialError, CredentialFuture, CredentialSource};
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, mpsc};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

/// The key the fixture server expects; no real key appears in any test.
pub const KEY: &str = "fixture-opal-key-5c0e1b7a92d4";

/// Runs `body` on a thread of its own and fails when it takes longer than 25 seconds, so a hang
/// fails the test instead of holding the build.
pub fn within_deadline(body: impl FnOnce() + Send + 'static) {
    let (done, finished) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        body();
        let _ = done.send(());
    });
    match finished.recv_timeout(Duration::from_secs(25)) {
        Ok(()) => worker.join().expect("the test body"),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            if let Err(panic) = worker.join() {
                std::panic::resume_unwind(panic);
            }
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            panic!("the test did not finish within 25 seconds")
        }
    }
}

/// One step the server takes after `SessionReady`.
#[derive(Debug, Clone)]
pub enum Step {
    /// Sends this message as one binary frame.
    Send(ClientBound),
    /// Sends these bytes as one binary frame.
    Raw(Vec<u8>),
    /// Sends this text frame.
    Text(String),
    /// Waits until the client has sent this many audio messages over the connection in all.
    AwaitAudio(usize),
    /// Waits until the client has answered the tool call with this id.
    AwaitToolResponse(&'static str),
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

/// How the server answers `InitializeSessionRequest`.
#[derive(Debug, Clone, PartialEq)]
pub enum Ready {
    /// `SessionReady`, after this delay.
    After(Duration),
    /// Nothing at all.
    Never,
    /// A close frame with this code.
    Close(u16),
    /// This message, then a close frame with code 1011.
    ErrorThenClose(ClientBound),
}

/// What the server does with one connection.
#[derive(Debug, Clone)]
pub struct Script {
    pub handshake: Handshake,
    pub ready: Ready,
    /// Whether the server answers `Ping` with `Pong`.
    pub pong: bool,
    pub steps: Vec<Step>,
}

impl Script {
    /// Answers the handshake and `SessionReady` at once, then takes `steps`.
    pub fn new(steps: Vec<Step>) -> Self {
        Self {
            handshake: Handshake::Answer,
            ready: Ready::After(Duration::ZERO),
            pong: true,
            steps,
        }
    }
}

/// What the server saw of one connection.
#[derive(Debug, Clone, Default)]
pub struct Seen {
    pub path: String,
    pub authorization: Option<String>,
    /// Every message the client sent, with when it arrived.
    pub messages: Vec<(Instant, ServiceBound)>,
    pub ready_sent: Option<Instant>,
    /// When the client closed the connection: a close frame, the end of the stream or an error.
    pub closed: Option<Instant>,
    pub violations: Vec<String>,
}

impl Seen {
    /// The audio messages: packet id and length.
    pub fn audio(&self) -> Vec<(u64, usize)> {
        self.messages
            .iter()
            .filter_map(|(_, message)| match message {
                ServiceBound::Audio {
                    packet_id, data, ..
                } => Some((*packet_id, data.len())),
                _ => None,
            })
            .collect()
    }

    /// The `ToolCallResponse` messages: id and result.
    pub fn tool_responses(&self) -> Vec<(String, String)> {
        self.messages
            .iter()
            .filter_map(|(_, message)| match message {
                ServiceBound::ToolCallResponse { id, result } => Some((id.clone(), result.clone())),
                _ => None,
            })
            .collect()
    }

    /// When each `Ping` arrived.
    pub fn pings(&self) -> Vec<Instant> {
        self.messages
            .iter()
            .filter(|(_, message)| *message == ServiceBound::Ping)
            .map(|(at, _)| *at)
            .collect()
    }
}

struct Connection {
    seen: Mutex<Seen>,
    changed: Notify,
}

/// A running fixture server; it stops when dropped.
pub struct OpalServer {
    /// `ws://127.0.0.1:<port>`, the base URL to connect to.
    pub url: String,
    connections: Arc<Mutex<Vec<Arc<Connection>>>>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl OpalServer {
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

impl Drop for OpalServer {
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
            if seen.authorization.as_deref() != Some(&format!("Bearer {KEY}")) {
                seen.violations.push(
                    "the Authorization header is not `Bearer` and the fixture key".to_owned(),
                );
            }
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
    let (out, mut outgoing) = mpsc::unbounded_channel::<Message>();
    let writer = tokio::spawn(async move {
        while let Some(message) = outgoing.recv().await {
            let close = matches!(message, Message::Close(_));
            if sink.send(message).await.is_err() || close {
                break;
            }
        }
    });
    let reader_connection = Arc::clone(&connection);
    let pong = script.pong;
    let reader_out = out.clone();
    let reader = tokio::spawn(async move {
        while let Some(message) = source.next().await {
            match message {
                Ok(Message::Binary(data)) => {
                    let mut seen = reader_connection.seen.lock().unwrap();
                    if data
                        .windows(KEY.len())
                        .any(|window| window == KEY.as_bytes())
                    {
                        seen.violations.push("a frame carries the key".to_owned());
                    }
                    match ServiceBound::decode(&data) {
                        Ok(message) => {
                            let initialize = matches!(message, ServiceBound::Initialize(_));
                            if seen.messages.is_empty() && !initialize {
                                seen.violations.push(format!(
                                    "the first message is not InitializeSessionRequest: {message:?}"
                                ));
                            }
                            if !seen.messages.is_empty() && initialize {
                                seen.violations
                                    .push("a second InitializeSessionRequest".to_owned());
                            }
                            if let ServiceBound::Audio { data, .. } = &message {
                                if seen.ready_sent.is_none() {
                                    seen.violations.push("audio before SessionReady".to_owned());
                                }
                                if data.is_empty() {
                                    seen.violations.push("an empty audio message".to_owned());
                                }
                            }
                            if message == ServiceBound::Ping && pong {
                                let _ =
                                    reader_out.send(Message::binary(ClientBound::Pong.encode()));
                            }
                            seen.messages.push((Instant::now(), message));
                        }
                        Err(error) => seen.violations.push(format!(
                            "a frame that is not a ServiceBoundMessage: {error}"
                        )),
                    }
                }
                Ok(Message::Text(text)) => {
                    reader_connection
                        .seen
                        .lock()
                        .unwrap()
                        .violations
                        .push(format!("a text frame: {}", text.as_str()));
                }
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(_) => {}
            }
            reader_connection.changed.notify_waiters();
        }
        closed(&reader_connection);
    });

    if until(&connection, |seen| !seen.messages.is_empty()).await {
        let first_is_initialize = matches!(
            connection.seen.lock().unwrap().messages[0].1,
            ServiceBound::Initialize(_)
        );
        if first_is_initialize {
            match script.ready {
                Ready::After(delay) => {
                    tokio::time::sleep(delay).await;
                    connection.seen.lock().unwrap().ready_sent = Some(Instant::now());
                    if out
                        .send(Message::binary(ClientBound::SessionReady.encode()))
                        .is_ok()
                    {
                        run(&out, &connection, script.steps).await;
                    }
                }
                Ready::Never => {}
                Ready::Close(code) => {
                    let _ = out.send(close_frame(code));
                }
                Ready::ErrorThenClose(message) => {
                    let _ = out.send(Message::binary(message.encode()));
                    let _ = out.send(close_frame(1011));
                }
            }
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(30), reader).await;
    drop(out);
    let _ = tokio::time::timeout(Duration::from_secs(1), writer).await;
}

async fn run(out: &mpsc::UnboundedSender<Message>, connection: &Connection, steps: Vec<Step>) {
    for step in steps {
        let sent = match step {
            Step::Send(message) => out.send(Message::binary(message.encode())).is_ok(),
            Step::Raw(bytes) => out.send(Message::binary(bytes)).is_ok(),
            Step::Text(text) => out.send(Message::text(text)).is_ok(),
            Step::AwaitAudio(count) => until(connection, |seen| seen.audio().len() >= count).await,
            Step::AwaitToolResponse(id) => {
                until(connection, |seen| {
                    seen.tool_responses()
                        .iter()
                        .any(|(answered, _)| answered == id)
                })
                .await
            }
            Step::Pause(duration) => {
                tokio::time::sleep(duration).await;
                true
            }
            Step::Close(code) => {
                let _ = out.send(close_frame(code));
                return;
            }
        };
        if !sent {
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

/// `ResponseBegin` of `turn`.
pub fn begin(turn: u32) -> Step {
    Step::Send(ClientBound::ResponseBegin { turn_id: turn })
}

/// `ResponseEnd` of `turn`.
pub fn end(turn: u32) -> Step {
    Step::Send(ClientBound::ResponseEnd { turn_id: turn })
}

/// `ModelTextFragment` of `turn`.
pub fn text(turn: u32, text: &str) -> Step {
    Step::Send(ClientBound::ModelTextFragment {
        text: text.to_owned(),
        turn_id: turn,
    })
}

/// `ModelAudioChunk` of `bytes` bytes of `byte`, of `turn` or of none.
pub fn audio(turn: Option<u32>, bytes: usize, byte: u8) -> Step {
    Step::Send(ClientBound::ModelAudioChunk {
        audio: vec![byte; bytes],
        turn_id: turn,
    })
}

/// `VadStateEvent` from `from` to `to`, triggered by `packet_id`.
pub fn vad(
    from: inference_deepslate::protocol::VadState,
    to: inference_deepslate::protocol::VadState,
    packet_id: u64,
) -> Step {
    Step::Send(ClientBound::VadStateEvent {
        from,
        to,
        packet_id,
    })
}

/// The user's speech as the VAD reports it: starting, confirmed at `start`, ending, and silence
/// confirmed at `stop`.
pub fn utterance(start: u64, stop: u64) -> Vec<Step> {
    use inference_deepslate::protocol::VadState::{Silence, Speech, SpeechEnding, SpeechStarting};
    vec![
        vad(Silence, SpeechStarting, start),
        vad(SpeechStarting, Speech, start),
        vad(Speech, SpeechEnding, stop),
        vad(SpeechEnding, Silence, stop),
    ]
}

/// One reply as the documentation orders it: `ResponseBegin`, its text, `chunks` audio chunks of
/// `bytes` bytes, `InferenceComplete` and `ResponseEnd`.
pub fn reply(turn: u32, said: &str, chunks: usize, bytes: usize) -> Vec<Step> {
    let mut steps = vec![begin(turn), text(turn, said)];
    steps.extend((0..chunks).map(|_| audio(Some(turn), bytes, 7)));
    steps.push(Step::Send(ClientBound::InferenceComplete { turn_id: turn }));
    steps.push(end(turn));
    steps
}
