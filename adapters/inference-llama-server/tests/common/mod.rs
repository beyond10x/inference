//! A loopback stand-in for `llama-server` that replays the pinned stream of
//! `fixtures/llama-server-b11752-text.sse`, recorded from a running server (see the `.txt` beside
//! it), and keeps every request it was sent. It is a fixture: it proves the adapter against the
//! recorded bytes, not against a live server.

#![allow(dead_code)]

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// The pinned response body of one streamed request.
pub const RECORDED: &[u8] = include_bytes!("../fixtures/llama-server-b11752-text.sse");

/// The model name the recorded server answered to.
pub const SERVED_MODEL: &str = "qwen2.5-1.5b-instruct";

/// The text deltas of [`RECORDED`], in order.
pub const DELTAS: [&str; 3] = ["Hello", " there", "."];

/// The longest a held response waits for its release.
const PATIENCE: Duration = Duration::from_secs(20);

/// One request the fixture received: its head (request line and headers) and its body.
#[derive(Debug, Clone)]
pub struct Request {
    pub head: String,
    pub body: String,
}

impl Request {
    /// The body as JSON.
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).expect("the request body is JSON")
    }
}

/// A server on a loopback port answering every connection with [`RECORDED`].
pub struct Fixture {
    base_url: String,
    requests: Arc<Mutex<Vec<Request>>>,
    release: Option<Sender<()>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Fixture {
    /// Answers every connection with the whole of [`RECORDED`] at once.
    pub fn replaying() -> Self {
        Self::start(None)
    }

    /// Answers the first connection with [`RECORDED`] up to and including the event of the first
    /// text delta, and holds the rest until [`Fixture::release`]; later connections are answered
    /// at once.
    pub fn holding() -> Self {
        let (release, held) = mpsc::channel();
        let mut fixture = Self::start(Some(held));
        fixture.release = Some(release);
        fixture
    }

    fn start(held: Option<Receiver<()>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        listener
            .set_nonblocking(true)
            .expect("a non-blocking listener");
        let port = listener.local_addr().expect("an address").port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (requests, stop) = (Arc::clone(&requests), Arc::clone(&stop));
            thread::spawn(move || serve(&listener, &requests, &stop, held))
        };
        Self {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            requests,
            release: None,
            stop,
            thread: Some(thread),
        }
    }

    /// The base URL of the fixture's API, `http://127.0.0.1:<port>/v1`.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Sends the rest of the held response.
    pub fn release(&mut self) {
        if let Some(release) = self.release.take() {
            let _receiver_gone = release.send(());
        }
    }

    /// Every request received so far, in order.
    pub fn requests(&self) -> Vec<Request> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.release();
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(
    listener: &TcpListener,
    requests: &Mutex<Vec<Request>>,
    stop: &AtomicBool,
    mut held: Option<Receiver<()>>,
) {
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((mut socket, _)) => {
                socket.set_nonblocking(false).expect("a blocking socket");
                let Some(request) = read_request(&mut socket) else {
                    continue;
                };
                requests
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(request);
                answer(&mut socket, held.take());
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2));
            }
            Err(error) => panic!("the fixture cannot accept: {error}"),
        }
    }
}

/// Reads one request: its head, then as many body bytes as its `content-length` names.
fn read_request(socket: &mut TcpStream) -> Option<Request> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let deadline = Instant::now() + PATIENCE;
    socket
        .set_read_timeout(Some(Duration::from_millis(200)))
        .expect("a read timeout");
    loop {
        if let Some(request) = complete(&bytes) {
            return Some(request);
        }
        if Instant::now() > deadline {
            return None;
        }
        match socket.read(&mut buffer) {
            Ok(0) => return complete(&bytes),
            Ok(read) => bytes.extend_from_slice(&buffer[..read]),
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => return None,
        }
    }
}

fn complete(bytes: &[u8]) -> Option<Request> {
    let text = String::from_utf8_lossy(bytes);
    let (head, body) = text.split_once("\r\n\r\n")?;
    let length = head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse::<usize>().ok())
            .flatten()
    })?;
    (body.len() >= length).then(|| Request {
        head: head.to_owned(),
        body: body[..length].to_owned(),
    })
}

/// Writes the response: all of [`RECORDED`], or, when `held` is given, its first part, then the
/// rest once `held` is released.
fn answer(socket: &mut TcpStream, held: Option<Receiver<()>>) {
    let head = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
    let (first, rest) = match held {
        Some(_) => RECORDED.split_at(first_delta_end()),
        None => (RECORDED, &[][..]),
    };
    let written = socket
        .write_all(head)
        .and_then(|()| socket.write_all(first))
        .and_then(|()| socket.flush());
    if written.is_err() {
        return;
    }
    if let Some(held) = held {
        match held.recv_timeout(PATIENCE) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => {}
            Err(RecvTimeoutError::Timeout) => return,
        }
    }
    let _client_gone = socket.write_all(rest);
    let _ = socket.shutdown(std::net::Shutdown::Both);
}

/// The offset just past the event that carries the first text delta of [`RECORDED`].
fn first_delta_end() -> usize {
    let text = std::str::from_utf8(RECORDED).expect("the recorded stream is UTF-8");
    let mut offset = 0;
    for event in text.split_inclusive("\n\n") {
        offset += event.len();
        if event.contains(&format!("\"delta\":{{\"content\":\"{}\"", DELTAS[0])) {
            return offset;
        }
    }
    panic!("the recorded stream carries no text delta")
}
