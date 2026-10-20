//! A loopback stand-in for `llama-server` that misbehaves on purpose, one script per connection,
//! and records how each connection ended. It answers from the recorded bytes of `fixtures/`: the
//! stream of `llama-server-b11752-text.sse` (the `common` fixture's [`RECORDED`]), and the 503 of
//! `llama-server-b11752-loading.http`, recorded from a server still loading its model.
//!
//! It serves one connection at a time, as a server with one slot computes one request at a time:
//! a connection the client leaves open holds back every later one. So a later request answered
//! in time is evidence that the earlier connection was let go.

#![allow(dead_code)]

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::common::{DELTAS, RECORDED};

/// The recorded answer of a server still loading its model: status line, headers and body.
pub const LOADING: &[u8] = include_bytes!("../fixtures/llama-server-b11752-loading.http");

/// The longest a stalled connection waits for its client to close it.
const PATIENCE: Duration = Duration::from_secs(20);

/// What the fixture does with one connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Script {
    /// The whole of [`RECORDED`], then the connection closed.
    Replay,
    /// [`RECORDED`] up to and including the event of the first text delta, then nothing until the
    /// client closes the connection, the rest held back.
    Stall,
    /// [`LOADING`], then the connection closed.
    Loading,
    /// [`RECORDED`] up to and including the event of the first text delta, then the connection
    /// closed: no finish reason and no `[DONE]`.
    Cut,
}

/// How one connection ended, as the fixture saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// The fixture wrote all its script names and closed the connection.
    Served,
    /// The client closed the connection while the fixture still held the rest of the stream,
    /// which it never sent.
    ClosedByClient,
    /// The client kept the connection open for [`PATIENCE`]; the fixture gave up on it.
    Abandoned,
}

/// What the serving thread records: the request bodies and the ending of each connection, in
/// order of connection.
#[derive(Default)]
struct Record {
    bodies: Vec<String>,
    endings: Vec<Ending>,
}

/// A server on a loopback port running `scripts` on its connections in order, and
/// [`Script::Replay`] on every connection after them.
pub struct Faults {
    base_url: String,
    record: Arc<(Mutex<Record>, Condvar)>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Faults {
    pub fn new(scripts: &[Script]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        listener
            .set_nonblocking(true)
            .expect("a non-blocking listener");
        let port = listener.local_addr().expect("an address").port();
        let record = Arc::new((Mutex::new(Record::default()), Condvar::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (record, stop, scripts) =
                (Arc::clone(&record), Arc::clone(&stop), scripts.to_vec());
            thread::spawn(move || serve(&listener, &record, &stop, &scripts))
        };
        Self {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            record,
            stop,
            thread: Some(thread),
        }
    }

    /// The base URL of the fixture's API, `http://127.0.0.1:<port>/v1`.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The bodies of the requests received so far, in order.
    pub fn bodies(&self) -> Vec<String> {
        self.lock().bodies.clone()
    }

    /// The ending of connection `index` (from 0), waiting for it at most `within`; none if the
    /// connection had not ended by then.
    pub fn ending(&self, index: usize, within: Duration) -> Option<Ending> {
        let deadline = Instant::now() + within;
        let (_, ended) = &*self.record;
        let mut record = self.lock();
        loop {
            if let Some(&ending) = record.endings.get(index) {
                return Some(ending);
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            record = ended
                .wait_timeout(record, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Record> {
        self.record.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Faults {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(
    listener: &TcpListener,
    record: &(Mutex<Record>, Condvar),
    stop: &AtomicBool,
    scripts: &[Script],
) {
    let mut served = 0;
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((mut socket, _)) => {
                socket.set_nonblocking(false).expect("a blocking socket");
                let Some(body) = read_body(&mut socket) else {
                    continue;
                };
                let (lock, ended) = record;
                lock.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .bodies
                    .push(body);
                let script = scripts.get(served).copied().unwrap_or(Script::Replay);
                served += 1;
                let ending = answer(&mut socket, script, stop);
                lock.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .endings
                    .push(ending);
                ended.notify_all();
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2));
            }
            Err(error) => panic!("the fixture cannot accept: {error}"),
        }
    }
}

/// Runs `script` on one connection whose request was read.
fn answer(socket: &mut TcpStream, script: Script, stop: &AtomicBool) -> Ending {
    let head = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
    let first = &RECORDED[..first_delta_end()];
    let written = match script {
        Script::Replay => socket
            .write_all(head)
            .and_then(|()| socket.write_all(RECORDED)),
        Script::Loading => socket.write_all(LOADING),
        Script::Stall | Script::Cut => socket
            .write_all(head)
            .and_then(|()| socket.write_all(first)),
    }
    .and_then(|()| socket.flush());
    if written.is_err() {
        return Ending::ClosedByClient;
    }
    if script == Script::Stall {
        return until_closed(socket, stop);
    }
    let _ = socket.shutdown(Shutdown::Both);
    Ending::Served
}

/// Holds the connection, the rest of the stream unsent, until its client closes it, the fixture
/// stops, or [`PATIENCE`] passes; the last two are [`Ending::Abandoned`].
fn until_closed(socket: &mut TcpStream, stop: &AtomicBool) -> Ending {
    let deadline = Instant::now() + PATIENCE;
    socket
        .set_read_timeout(Some(Duration::from_millis(10)))
        .expect("a read timeout");
    let mut buffer = [0_u8; 256];
    loop {
        match socket.read(&mut buffer) {
            Ok(0) => return Ending::ClosedByClient,
            Ok(_) => {}
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => return Ending::ClosedByClient,
        }
        if stop.load(Ordering::SeqCst) || Instant::now() > deadline {
            return Ending::Abandoned;
        }
    }
}

/// Reads one request and returns its body: the head, then as many body bytes as its
/// `content-length` names.
fn read_body(socket: &mut TcpStream) -> Option<String> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let deadline = Instant::now() + PATIENCE;
    socket
        .set_read_timeout(Some(Duration::from_millis(200)))
        .expect("a read timeout");
    loop {
        let text = String::from_utf8_lossy(&bytes);
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let length = head.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })?;
            if body.len() >= length {
                return Some(body[..length].to_owned());
            }
        }
        if Instant::now() > deadline {
            return None;
        }
        match socket.read(&mut buffer) {
            Ok(0) => return None,
            Ok(read) => bytes.extend_from_slice(&buffer[..read]),
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => return None,
        }
    }
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
