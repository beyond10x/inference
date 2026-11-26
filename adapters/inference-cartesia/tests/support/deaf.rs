//! A connection whose close cannot be written, on any host. The server reads the generation
//! request and nothing after it, sends a flood of pings and then the segment. The client answers
//! each ping with a pong while it reads; the pongs fill the socket's buffers and then wait in the
//! client's WebSocket buffer, in front of everything written later: the close frame of a drop.
//!
//! How many pongs the buffers take is the kernel's (`net.ipv4.tcp_wmem`), so the server tells the
//! case whether the connection was backed up. When it is told to, it reads everything the
//! client's socket held and counts the frames. The client answers every ping, so fewer pongs
//! than pings means the rest never left the client: its writes waited, and the close behind
//! them. [`on_a_backed_up_connection`] doubles a flood whose pongs all arrived and runs the
//! session again, up to [`LARGEST`]; past that it fails and names the host's setting.
//!
//! The file that includes this one has the module `dropped`. The ElevenLabs adapter's tests have
//! a file of the same name for its protocol.

use std::io::Read;
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use base64::Engine;
use inference_cartesia::CLOSE_BOUND;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::{self, Bytes, Message};

use crate::dropped::threads_named;

/// The flood a case starts with: 36,000 pongs of 131 bytes are 4.7 MB, a tenth more than the
/// socket buffers hold where the kernel's largest send buffer is Linux's default of 4 MiB. There
/// 32,400 pings did not back the close up and 32,600 did.
pub const FIXED: usize = 36_000;

/// The largest flood: 576,000 pongs of 131 bytes are 75 MB.
pub const LARGEST: usize = 16 * FIXED;

/// The kernel's name of a synthesis's own thread: the first 15 bytes of `inference-cartesia`.
pub const OWN_THREAD: &str = "inference-carte";

/// The opcode of a pong.
const PONG: u8 = 0xa;

/// What the server read of the client, frame by frame, once it was told to read again.
#[derive(Debug, Clone, Default)]
pub struct Drained {
    /// How many whole pongs it read.
    pub pongs: usize,
    /// The opcode of every whole frame that is not a pong, in order.
    pub others: Vec<u8>,
    /// How many pongs it read after the first frame that is not a pong.
    pub late_pongs: usize,
    /// How many bytes followed the last whole frame.
    pub unframed: usize,
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

/// A server that reads one generation request and nothing after it, sends its pings and the
/// segment, and reads again only when it is told to; it then reads the socket's bytes itself and
/// answers nothing.
pub struct Deaf {
    /// The `ws://` URL of the server.
    pub url: String,
    /// Tells the server to read again, when something is sent or every sender is gone.
    pub release: mpsc::Sender<()>,
    thread: std::thread::JoinHandle<Drained>,
}

impl Deaf {
    /// A server that sends `pings` pings, then one chunk of the segment and the context's `done`.
    pub fn start(pings: usize) -> Self {
        Self::serve(pings, true)
    }

    /// A server that sends `pings` pings, then one chunk of the segment and nothing more: the
    /// synthesis waits for the rest of the segment until its read timeout has passed.
    pub fn stalling(pings: usize) -> Self {
        Self::serve(pings, false)
    }

    /// A server that sends `pings` pings, one chunk of the segment and, with `done`, the
    /// context's `done`.
    fn serve(pings: usize, done: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let url = format!("ws://{}", listener.local_addr().expect("an address"));
        let (release, released) = mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("a client");
            let mut socket = tungstenite::accept(stream).expect("the handshake");
            let context = loop {
                let Message::Text(text) = socket.read().expect("the generation request") else {
                    continue;
                };
                let request: Value = serde_json::from_str(text.as_str()).expect("JSON");
                break request["context_id"]
                    .as_str()
                    .expect("the request names its context")
                    .to_owned();
            };
            let payload = Bytes::from(vec![0x2a_u8; 125]);
            for _ in 0..pings {
                socket
                    .write(Message::Ping(payload.clone()))
                    .expect("a ping is written");
            }
            socket.flush().expect("the pings are flushed");
            let chunk = json!({
                "type": "chunk",
                "data": base64::engine::general_purpose::STANDARD.encode([0xff_u8; 400]),
                "done": false,
                "status_code": 206,
                "step_time": 12.5,
                "context_id": context,
            });
            socket
                .send(Message::text(chunk.to_string()))
                .expect("the chunk is sent");
            if done {
                let done = json!({
                    "type": "done",
                    "done": true,
                    "status_code": 206,
                    "context_id": context,
                });
                socket
                    .send(Message::text(done.to_string()))
                    .expect("`done` is sent");
            }
            let _ = released.recv_timeout(Duration::from_secs(30));
            let stream = socket.get_mut();
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let mut drained = Drained::default();
            let mut kept = Vec::new();
            let mut chunk = vec![0; 1 << 16];
            loop {
                let read = match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => read,
                };
                kept.extend_from_slice(&chunk[..read]);
                let mut at = 0;
                while let Some((opcode, length)) = whole_frame(&kept[at..]) {
                    at += length;
                    if opcode == PONG {
                        drained.pongs += 1;
                        drained.late_pongs += usize::from(!drained.others.is_empty());
                    } else {
                        drained.others.push(opcode);
                    }
                }
                kept.drain(..at);
            }
            drained.unframed = kept.len();
            drained
        });
        Self {
            url,
            release,
            thread,
        }
    }

    /// Tells the server to read again, and returns what it read until the client's side ended.
    pub fn finish(self) -> Drained {
        let _ = self.release.send(());
        self.thread.join().expect("the server ends without a panic")
    }
}

/// A flood that backed the connection up.
pub struct Flood {
    /// How many pings the server sent.
    pub pings: usize,
    /// What the server read once the synthesis's own thread had ended.
    pub drained: Drained,
    /// The descriptors the process held after the first session, whose close was written, and
    /// after this one; `None` when this session is the first or the kernel lists none.
    pub descriptors: Option<(usize, usize)>,
}

/// How many descriptors the process holds, once that is `wanted` or `bound` has passed; `None`
/// where the kernel lists none (`/proc/self/fd`).
fn descriptors(wanted: Option<usize>, bound: Duration) -> Option<usize> {
    let started = Instant::now();
    loop {
        let held = std::fs::read_dir("/proc/self/fd").ok()?.count() - 1;
        if wanted.is_none_or(|wanted| wanted == held) || started.elapsed() >= bound {
            return Some(held);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// What the kernel says of `limit` under `net.ipv4`, or why it cannot be read.
fn setting(limit: &str) -> String {
    match std::fs::read_to_string(format!("/proc/sys/net/ipv4/{limit}")) {
        Ok(value) => value.split_whitespace().collect::<Vec<_>>().join(" "),
        Err(error) => format!("unreadable ({error})"),
    }
}

/// Runs `session` against a server whose flood, `first` pings at first, is doubled until the
/// connection was backed up: until the server, reading again after the synthesis's own thread
/// has ended, finds fewer pongs than it sent pings. What that session returned, and the flood.
pub fn on_a_backed_up_connection<T>(first: usize, session: impl FnMut(String) -> T) -> (T, Flood) {
    on_a_backed_up_connection_of(Deaf::start, first, session)
}

/// [`on_a_backed_up_connection`] with the server `serve` starts for a flood of the pings it is
/// given: [`Deaf::start`] or [`Deaf::stalling`].
pub fn on_a_backed_up_connection_of<T>(
    serve: fn(usize) -> Deaf,
    first: usize,
    mut session: impl FnMut(String) -> T,
) -> (T, Flood) {
    let wmem = setting("tcp_wmem");
    println!(
        "net.ipv4.tcp_wmem: {wmem}; net.ipv4.tcp_rmem: {}",
        setting("tcp_rmem")
    );
    let mut pings = first;
    let mut written = None;
    loop {
        let server = serve(pings);
        let seen = session(server.url.clone());
        let left = threads_named(OWN_THREAD, 0, CLOSE_BOUND + Duration::from_secs(3));
        assert_eq!(left, Some(0), "the synthesis's own thread ends");
        let drained = server.finish();
        let held = descriptors(written, Duration::from_secs(2));
        println!(
            "a flood of {pings} pings: the server then read {drained:?}; the process holds \
             {held:?} descriptors"
        );
        if drained.pongs < pings {
            let flood = Flood {
                pings,
                drained,
                descriptors: written.zip(held),
            };
            return (seen, flood);
        }
        written = written.or(held);
        assert!(
            pings < LARGEST,
            "the precondition of this case does not hold on this host: its socket buffers took \
             every pong of a flood of {pings} pings, {} bytes, so no close was backed up and \
             nothing was asserted of the drop. The host's `net.ipv4.tcp_wmem` is `{wmem}`; its \
             largest send buffer decides how many pongs the buffers take",
            pings * 131
        );
        pings *= 2;
    }
}
