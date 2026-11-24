//! Adversary, story:synthesiser-drop-inside-runtime: the story's acceptance against a close that
//! cannot be written, with no change in `src`.
//!
//! The story's acceptance: "Dropping an idle synthesis with an open connection on a
//! current-thread runtime, with a 10 ms ticker task running, keeps the ticker's largest gap under
//! 50 ms." The unit's own test of it meets a server whose buffer has room, where the close is one
//! small write, and so it passes with the drop that waits for its close as well.
//!
//! Here nothing can be written. The server reads the generation request and nothing after it,
//! sends [`PINGS`] pings and then the segment. The client answers each ping with a pong while it
//! reads; the pongs fill the socket's buffers and then wait in the client's WebSocket buffer, in
//! front of everything written later: the close frame of a drop.
//!
//! A drop that waits for its close holds the runtime's thread for `CLOSE_BOUND` there and fails
//! the ticker's bound. The case also asserts that the close was still waiting 300 ms after the
//! drop, and that the synthesis's own thread ends once `CLOSE_BOUND` has passed.

mod support;

#[allow(dead_code)]
#[path = "../../inference-elevenlabs/tests/support/dropped.rs"]
mod dropped;
#[path = "../../inference-elevenlabs/tests/support/ticker.rs"]
mod ticker;

use std::net::TcpListener;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use base64::Engine;
use dropped::threads_named;
use inference_cartesia::{CLOSE_BOUND, CartesiaSynthesiser, Endpoint, OutputFormat, VoiceConfig};
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use serde_json::{Value, json};
use support::{committed, within};
use ticker::{GAP_BOUND, with_ticker};
use tokio_tungstenite::tungstenite::{self, Bytes, Message};

const FORMAT: OutputFormat = OutputFormat::MULAW_8000;

/// How many pings the server sends: 36,000 pongs of 131 bytes are 4.7 MB, more than the socket
/// buffers of both ends hold. The client's send buffer grows to the kernel's largest
/// (`net.ipv4.tcp_wmem`, 4 MiB where this was measured and by Linux's default), and the server
/// that reads nothing holds about 64 kB more: 32,400 pings did not back the close up and 32,600
/// did. This is a tenth above that and no more, because the flood loads a processor beside the
/// suite's other ticker tests. A host with a larger send buffer needs more.
const PINGS: usize = 36_000;

/// What a session awaits before it drops or discards, so the ticker runs before it.
const LEAD: Duration = Duration::from_millis(35);

/// How long after the drop the session looks whether the close is still waiting.
const LOOK: Duration = Duration::from_millis(300);

/// The kernel's name of a synthesis's own thread: the first 15 bytes of `inference-cartesia`.
const OWN_THREAD: &str = "inference-carte";

/// A server that reads one generation request and nothing after it; it holds its connection
/// open until it is dropped.
struct Deaf {
    url: String,
    release: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Deaf {
    /// A server that, after its pings, sends one chunk of the segment and the context's `done`.
    fn start() -> Self {
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
            for _ in 0..PINGS {
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
            let done = json!({
                "type": "done",
                "done": true,
                "status_code": 206,
                "context_id": context,
            });
            socket
                .send(Message::text(done.to_string()))
                .expect("`done` is sent");
            let _ = released.recv_timeout(Duration::from_secs(30));
        });
        Self {
            url,
            release: Some(release),
            thread: Some(thread),
        }
    }
}

impl Drop for Deaf {
    fn drop(&mut self) {
        drop(self.release.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A synthesiser of the fixture voice at `url`.
fn at(url: &str) -> CartesiaSynthesiser {
    CartesiaSynthesiser::new(
        Endpoint::new()
            .with_url(url)
            .with_read_timeout(Duration::from_secs(20))
            .with_connect_timeout(Duration::from_secs(5)),
        VoiceConfig::new(support::VOICE, FORMAT),
        Arc::new(support::FixedKey(support::KEY)),
        "cartesia-api-key",
    )
    .expect("the fixture configuration is valid")
}

/// What one session saw of its drop.
struct Seen {
    /// How long the drop held the runtime's thread.
    held: Duration,
    /// When the drop began.
    dropped: Instant,
    /// The synthesis's own threads before the drop.
    before: Option<usize>,
    /// The synthesis's own threads [`LOOK`] after the drop.
    waiting: Option<usize>,
}

/// One synthesis at `url` that speaks a segment and is then idle, its utterance open and its
/// connection with it, and is dropped there on the runtime's thread.
async fn drop_idle(url: String) -> Seen {
    let mut synthesiser = at(&url);
    let mut handle = synthesiser
        .start()
        .expect("a fresh synthesiser runs nothing");
    handle
        .offer_segment(committed(&["Speak, then idle."]).remove(0))
        .expect("the utterance is open");
    let mut completed = false;
    while let Some(event) = handle.event().await {
        completed |= matches!(event, SynthesisEvent::Completed(_));
    }
    assert!(completed, "the segment completes");
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
    let before = threads_named(OWN_THREAD, 1, Duration::ZERO);
    tokio::time::sleep(LEAD).await;
    let dropped = Instant::now();
    drop(handle);
    let held = dropped.elapsed();
    tokio::time::sleep(LOOK).await;
    let waiting = threads_named(OWN_THREAD, 1, Duration::ZERO);
    Seen {
        held,
        dropped,
        before,
        waiting,
    }
}

/// The acceptance, where the close cannot be written: the drop of the idle synthesis holds the
/// runtime's thread for less than the ticker's bound, the close goes on waiting on the
/// synthesis's own thread, and that thread ends once `CLOSE_BOUND` has passed.
#[test]
fn dropping_an_idle_synthesis_whose_close_cannot_be_written_does_not_block_the_runtime() {
    within(|| {
        let started = Instant::now();
        let server = Deaf::start();
        let (seen, largest) = with_ticker(vec![drop_idle(server.url.clone())]);
        let seen = &seen[0];
        println!(
            "the drop held the thread {:?}, the ticker's largest gap is {largest:?}, own threads \
             before the drop {:?} and {LOOK:?} after it {:?}",
            seen.held, seen.before, seen.waiting
        );
        assert!(
            largest < GAP_BOUND,
            "the ticker's largest gap {largest:?}; the drop held the runtime's thread {:?}",
            seen.held
        );
        assert_eq!(
            seen.before,
            Some(1),
            "an idle synthesis has one thread of its own"
        );
        assert_eq!(
            seen.waiting,
            Some(1),
            "the close still waits {LOOK:?} after the drop: the server reads nothing"
        );
        let left = threads_named(OWN_THREAD, 0, CLOSE_BOUND + Duration::from_secs(3));
        let ended = seen.dropped.elapsed();
        println!(
            "the synthesis's own thread ended within {ended:?} of the drop; the case took {:?}",
            started.elapsed()
        );
        assert_eq!(
            left,
            Some(0),
            "the synthesis's own thread ends once CLOSE_BOUND has passed, {ended:?} after the drop"
        );
    });
}

/// The other half of what `src/lib.rs` says of the drop: "Outside an async runtime the drop
/// waits for that close", "bounded by `CLOSE_BOUND`". On a thread with no runtime, where the
/// close cannot be written, the drop of the idle synthesis waits `CLOSE_BOUND` and no longer.
#[test]
fn dropping_an_idle_synthesis_whose_close_cannot_be_written_outside_a_runtime_waits_close_bound() {
    within(|| {
        let server = Deaf::start();
        let mut synthesiser = at(&server.url);
        let mut handle = synthesiser
            .start()
            .expect("a fresh synthesiser runs nothing");
        handle
            .offer_segment(committed(&["Speak, then idle."]).remove(0))
            .expect("the utterance is open");
        let events: Vec<SynthesisEvent> = handle.by_ref().collect();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, SynthesisEvent::Completed(_))),
            "the segment completes"
        );
        assert!(handle.failure().is_none(), "{:?}", handle.failure());
        let started = Instant::now();
        drop(handle);
        let held = started.elapsed();
        println!("outside a runtime the drop held its thread {held:?}");
        assert!(
            held + Duration::from_millis(100) >= CLOSE_BOUND,
            "outside a runtime the drop waits for its close, which cannot be written: {held:?}"
        );
        assert!(
            held <= CLOSE_BOUND + Duration::from_millis(500),
            "the drop waits at most CLOSE_BOUND: {held:?}"
        );
    });
}
