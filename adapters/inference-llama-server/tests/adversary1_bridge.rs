//! Adversary cases of the runtime bridge: a turn future dropped by a caller of
//! `llm_core::Model` directly, with its `Cancel` never fired, and two generations of a
//! two-slot generator running at once.

mod common;
mod faults;

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant};

use common::{DELTAS, RECORDED, SERVED_MODEL};
use faults::{Ending, Faults, Script};
use inference_core::id::{NodeId, RunId};
use inference_core::model::{Adapter, Model};
use inference_llama_server::{Config, LlamaServer, declare_adapter, declare_model};
use inference_llm::llm_core::{
    BoxFuture, Cancel, Error, Item, Model as _, StreamEvent, StreamSink, TurnRequest,
};
use inference_llm::{GenerationEvent, GenerationHandle, GenerationRequest, Generator};

const WITHIN: Duration = Duration::from_secs(5);

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

struct Recording(Arc<Mutex<Vec<StreamEvent>>>);

impl StreamSink for Recording {
    fn emit(&mut self, event: StreamEvent) -> BoxFuture<'_, Result<(), Error>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
        Box::pin(async { Ok(()) })
    }
}

/// The crate documents `LlamaServer::turn`: "dropping the turn, cancels the request still
/// running". A caller of `llm_core::Model` that drops the turn future and keeps its `Cancel`
/// unfired must see the connection closed and the turn's thread gone, not a request left open
/// until the server or an idle limit gives up.
#[test]
fn dropping_a_turn_future_with_its_cancel_unfired_closes_the_connection() {
    let faults = Faults::new(&[Script::Stall]);
    let config = Config::new(faults.base_url(), SERVED_MODEL);
    let server = LlamaServer::new(&config).expect("the fixture's base URL binds");
    let request = TurnRequest::new(SERVED_MODEL, vec![Item::user("Say hello in three words.")]);
    let cancel = Cancel::new();
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut sink = Recording(Arc::clone(&events));
    {
        let mut turn = server.turn(&request, &mut sink, &cancel);
        let waker = Waker::from(Arc::new(Unpark(thread::current())));
        let mut cx = Context::from_waker(&waker);
        let deadline = Instant::now() + WITHIN;
        while events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
        {
            assert!(Instant::now() < deadline, "no event of the stalled stream");
            if let Poll::Ready(ended) = turn.as_mut().poll(&mut cx) {
                panic!("the stalled turn ended: ok = {}", ended.is_ok());
            }
            thread::park_timeout(Duration::from_millis(50));
        }
        assert_eq!(server.running_turns(), 1, "the turn runs, the stream open");
        drop(turn);
    }
    assert!(
        !cancel.is_cancelled(),
        "the caller's cancel was never fired"
    );
    assert_eq!(
        faults.ending(0, WITHIN),
        Some(Ending::ClosedByClient),
        "dropping the turn closed the connection"
    );
    let deadline = Instant::now() + WITHIN;
    while server.running_turns() > 0 {
        assert!(Instant::now() < deadline, "the turn's thread still runs");
        thread::sleep(Duration::from_millis(5));
    }
}

/// A loopback server answering each connection on a thread of its own with [`RECORDED`]; the
/// first connection is held after the first delta until `release` fires.
struct Parallel {
    base_url: String,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Parallel {
    fn new(release: Receiver<()>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        listener.set_nonblocking(true).expect("non-blocking");
        let port = listener.local_addr().expect("an address").port();
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                let mut release = Some(release);
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((socket, _)) => {
                            let held = release.take();
                            thread::spawn(move || answer(socket, held));
                        }
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(error) => panic!("accept: {error}"),
                    }
                }
            })
        };
        Self {
            base_url: format!("http://127.0.0.1:{port}/v1"),
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Parallel {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn answer(mut socket: TcpStream, held: Option<Receiver<()>>) {
    socket.set_nonblocking(false).expect("blocking");
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("a timeout");
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let text = String::from_utf8_lossy(&bytes).into_owned();
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let length = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if body.len() >= length {
                break;
            }
        }
        match socket.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(read) => bytes.extend_from_slice(&buffer[..read]),
        }
    }
    let head = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
    let split = first_delta_end();
    let (first, rest) = match held {
        Some(_) => RECORDED.split_at(split),
        None => (RECORDED, &[][..]),
    };
    if socket
        .write_all(head)
        .and_then(|()| socket.write_all(first))
        .and_then(|()| socket.flush())
        .is_err()
    {
        return;
    }
    if let Some(held) = held {
        let _ = held.recv_timeout(Duration::from_secs(15));
    }
    let _ = socket.write_all(rest);
    let _ = socket.shutdown(Shutdown::Both);
}

fn first_delta_end() -> usize {
    let text = std::str::from_utf8(RECORDED).expect("UTF-8");
    let mut offset = 0;
    for event in text.split_inclusive("\n\n") {
        offset += event.len();
        if event.contains(&format!("\"delta\":{{\"content\":\"{}\"", DELTAS[0])) {
            return offset;
        }
    }
    panic!("no text delta")
}

fn start(generator: &mut Generator) -> GenerationHandle {
    generator
        .start(GenerationRequest {
            run_id: RunId::generate(),
            node_id: NodeId::generate(),
            epoch: 1,
            turn: TurnRequest::new(SERVED_MODEL, vec![Item::user("Say hello in three words.")]),
        })
        .expect("a free slot")
}

fn texts(events: &[GenerationEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            GenerationEvent::TextDelta(delta) => Some(delta.text.as_str()),
            _ => None,
        })
        .collect()
}

/// `Config::max_concurrency` is the server's parallel slots, declared as the adapter's maximum
/// concurrency. With two slots, a second generation runs to completion while the first is still
/// streaming: the bridge does not serialize turns.
#[test]
fn two_slots_run_two_generations_at_once() {
    let (release, held) = mpsc::channel();
    let fixture = Parallel::new(held);
    let mut config = Config::new(fixture.base_url.clone(), SERVED_MODEL);
    config.max_concurrency = 2;
    let (model, _) = Model::declare(declare_model());
    let (adapter, _) =
        Adapter::declare(declare_adapter(model.model_id(), &config)).expect("two slots");
    let server = Arc::new(LlamaServer::new(&config).expect("a binding"));
    let mut generator = Generator::new(adapter, Arc::clone(&server) as _).expect("declared");

    let mut first = start(&mut generator);
    let mut taken = Vec::new();
    for event in first.by_ref() {
        let delta = matches!(event, GenerationEvent::TextDelta(_));
        taken.push(event);
        if delta {
            break;
        }
    }
    assert_eq!(texts(&taken), [DELTAS[0]]);

    let began = Instant::now();
    let second: Vec<GenerationEvent> = start(&mut generator).collect();
    assert!(
        began.elapsed() < WITHIN,
        "the second generation waited for the first: {:?}",
        began.elapsed()
    );
    assert!(
        matches!(second.last(), Some(GenerationEvent::Completed(_))),
        "{second:?}"
    );
    assert_eq!(texts(&second), DELTAS);
    assert!(server.running_turns() >= 1, "the first turn still runs");

    release.send(()).expect("the held connection waits");
    let rest: Vec<GenerationEvent> = first.by_ref().collect();
    assert!(
        matches!(rest.last(), Some(GenerationEvent::Completed(_))),
        "{rest:?}"
    );
    assert_eq!(texts(&rest), &DELTAS[1..]);
}
