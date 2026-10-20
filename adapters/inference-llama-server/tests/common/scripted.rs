//! A loopback stand-in for `llama-server` that answers its connections in order from a script of
//! pinned response bodies, one body per request, and keeps every request it was sent. Like the
//! fixture of the parent module, it proves the adapter against recorded bytes, not against a live
//! server.

use std::io::{ErrorKind, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::{Request, read_request};

/// The streamed body of a turn that writes prose, then proposes one call; see
/// `fixtures/llama-server-b11752-tool.txt`.
pub const TOOL_CALL: &[u8] = include_bytes!("../fixtures/llama-server-b11752-tool-call.sse");

/// The streamed body of the turn that answers after the result of the call of [`TOOL_CALL`].
pub const TOOL_RESULT: &[u8] = include_bytes!("../fixtures/llama-server-b11752-tool-result.sse");

/// [`TOOL_CALL`] with the call's name removed from every fragment.
pub const TOOL_NAMELESS: &[u8] =
    include_bytes!("../fixtures/llama-server-b11752-tool-nameless.sse");

/// The recorded text stream of the parent module without its usage chunk; see
/// `fixtures/llama-server-b11752-text.txt`.
pub const TEXT_NO_USAGE: &[u8] =
    include_bytes!("../fixtures/llama-server-b11752-text-no-usage.sse");

/// The streamed body of the request of the parent module's recording sent without
/// `stream_options.include_usage`: no usage chunk, its counters in the `timings` object of the
/// finish chunk; see `fixtures/llama-server-b11752-text.txt`.
pub const TEXT_TIMINGS_ONLY: &[u8] =
    include_bytes!("../fixtures/llama-server-b11752-text-timings-only.sse");

/// A server on a loopback port answering its n-th connection with the n-th body of its script,
/// and every connection past the script with an HTTP 500.
pub struct Scripted {
    base_url: String,
    requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Scripted {
    /// Answers the connections in order from `script`.
    pub fn new(script: Vec<&'static [u8]>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        listener
            .set_nonblocking(true)
            .expect("a non-blocking listener");
        let port = listener.local_addr().expect("an address").port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (requests, stop) = (Arc::clone(&requests), Arc::clone(&stop));
            thread::spawn(move || serve(&listener, &requests, &stop, &script))
        };
        Self {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            requests,
            stop,
            thread: Some(thread),
        }
    }

    /// The base URL of the fixture's API, `http://127.0.0.1:<port>/v1`.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Every request received so far, in order.
    pub fn requests(&self) -> Vec<Request> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Drop for Scripted {
    fn drop(&mut self) {
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
    script: &[&'static [u8]],
) {
    let mut answered = 0;
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
                let written = match script.get(answered) {
                    Some(body) => socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                        )
                        .and_then(|()| socket.write_all(body)),
                    None => socket.write_all(
                        b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    ),
                };
                answered += 1;
                let _client_gone = written.and_then(|()| socket.flush());
                let _ = socket.shutdown(std::net::Shutdown::Both);
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2));
            }
            Err(error) => panic!("the fixture cannot accept: {error}"),
        }
    }
}
