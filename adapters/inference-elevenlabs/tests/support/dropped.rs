//! What the fixture servers do not tell of a dropped handle. A relay in front of a fixture
//! server tells whether the client ended its connection with a WebSocket close frame or only let
//! the socket end: it passes every byte on in both directions and reads the frames the client
//! sent after its handshake request. And [`threads_named`] counts the handle's own thread, which
//! ends with its runtime.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How many threads of this process the kernel lists under a name that starts with `name`, once
/// that is `wanted` or `bound` has passed; `None` where the kernel lists no threads
/// (`/proc/self/task`). The kernel keeps the first 15 bytes of a thread's name.
pub fn threads_named(name: &str, wanted: usize, bound: Duration) -> Option<usize> {
    let started = Instant::now();
    loop {
        let named = std::fs::read_dir("/proc/self/task")
            .ok()?
            .flatten()
            .filter(|task| {
                std::fs::read_to_string(task.path().join("comm"))
                    .is_ok_and(|comm| comm.starts_with(name))
            })
            .count();
        if named == wanted || started.elapsed() >= bound {
            return Some(named);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The opcode of a text frame.
pub const TEXT: u8 = 0x1;
/// The opcode of a close frame.
pub const CLOSE: u8 = 0x8;

/// What the relay read of one client until that client's side of the connection ended.
#[derive(Debug, PartialEq, Eq)]
pub struct ClientSide {
    /// The opcode of every whole frame the client sent after its handshake request, in order.
    pub opcodes: Vec<u8>,
    /// How many bytes followed the last whole frame.
    pub unframed: usize,
    /// The stream ended in order, with the client's FIN, not with an error.
    pub ended_in_order: bool,
}

/// A relay to one server, on a loopback port of its own.
pub struct Relay {
    /// The `ws://` URL of the relay.
    pub url: String,
    sides: Mutex<mpsc::Receiver<ClientSide>>,
}

impl Relay {
    /// A relay to the server at `server_url`, a `ws://` URL of a loopback port.
    pub fn to(server_url: &str) -> Self {
        let server = server_url.trim_start_matches("ws://").to_owned();
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let url = format!("ws://{}", listener.local_addr().expect("an address"));
        let (sides_tx, sides_rx) = mpsc::channel();
        std::thread::spawn(move || {
            for client in listener.incoming() {
                let (Ok(client), Ok(upstream)) = (client, TcpStream::connect(&server)) else {
                    return;
                };
                let sides = sides_tx.clone();
                std::thread::spawn(move || {
                    let _ = sides.send(pass_on(client, upstream));
                });
            }
        });
        Self {
            url,
            sides: Mutex::new(sides_rx),
        }
    }

    /// What the relay read of the next client whose side ended, waiting at most `bound`.
    pub fn next_client_side(&self, bound: Duration) -> Option<ClientSide> {
        self.sides.lock().unwrap().recv_timeout(bound).ok()
    }
}

/// Passes the server's bytes on to the client on a thread of its own and the client's bytes on
/// to the server here, until the client's side ends; what the client sent.
fn pass_on(mut client: TcpStream, mut upstream: TcpStream) -> ClientSide {
    if let (Ok(mut from), Ok(mut to)) = (upstream.try_clone(), client.try_clone()) {
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut from, &mut to);
            let _ = to.shutdown(Shutdown::Write);
        });
    }
    let mut side = ClientSide {
        opcodes: Vec::new(),
        unframed: 0,
        ended_in_order: false,
    };
    let mut request = true;
    let mut kept = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let read = match client.read(&mut chunk) {
            Ok(0) => {
                side.ended_in_order = true;
                break;
            }
            Ok(read) => read,
            Err(_) => break,
        };
        let _ = upstream.write_all(&chunk[..read]);
        kept.extend_from_slice(&chunk[..read]);
        if request {
            let Some(end) = kept.windows(4).position(|window| window == b"\r\n\r\n") else {
                continue;
            };
            kept.drain(..end + 4);
            request = false;
        }
        while let Some((opcode, length)) = whole_frame(&kept) {
            side.opcodes.push(opcode);
            kept.drain(..length);
        }
    }
    let _ = upstream.shutdown(Shutdown::Write);
    side.unframed = kept.len();
    side
}

/// The opcode and the length in bytes of the frame `bytes` starts with, when all of it is there.
fn whole_frame(bytes: &[u8]) -> Option<(u8, usize)> {
    let (first, second) = (*bytes.first()?, *bytes.get(1)?);
    let (header, payload) = match second & 0x7f {
        126 => {
            let length: [u8; 2] = bytes.get(2..4)?.try_into().ok()?;
            (4, usize::from(u16::from_be_bytes(length)))
        }
        127 => {
            let length: [u8; 8] = bytes.get(2..10)?.try_into().ok()?;
            (10, usize::try_from(u64::from_be_bytes(length)).ok()?)
        }
        short => (2, usize::from(short)),
    };
    let mask = if second & 0x80 == 0 { 0 } else { 4 };
    let length = header + mask + payload;
    (bytes.len() >= length).then_some((first & 0x0f, length))
}
