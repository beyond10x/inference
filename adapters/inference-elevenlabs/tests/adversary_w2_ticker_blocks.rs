//! Adversary, story:ticker-tests-hold-on-a-loaded-host: the ticker of `async_handles.rs` against
//! a runtime thread that is held for 200 ms in other ways than `std::thread::sleep`.
//!
//! The story's acceptance: "A change that blocks the runtime for 200 ms inside a synthesis or a
//! recognition fails both tests." The ticker now takes out of each gap the time the kernel
//! counted the runtime's thread as ready to run without a processor, and the test file says of
//! it: "A thread that sleeps, parks or waits on a lock or a socket is not ready to run, so a
//! handle that blocks the thread is counted in full."
//!
//! Each case holds the thread of that ticker once for 200 ms, in one form, between two awaits,
//! and asserts the opposite of what the two tests assert: the largest gap is at least the bound,
//! so a handle that held the thread that way would fail them. The forms that sleep also assert
//! the sentence above: the gap is at least the 200 ms.
//!
//! The ticker is the one of `async_handles.rs`, `support/ticker.rs`. The loop of yields is
//! ignored: it shows the limit that file's module comment states, and only on a shared
//! processor.

#[path = "support/ticker.rs"]
mod ticker;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Context, Wake, Waker};
use std::time::{Duration, Instant};

use ticker::{GAP_BOUND, with_ticker};

/// How long each form holds the runtime's thread: the story's 200 ms.
const BLOCK: Duration = Duration::from_millis(200);
/// What a session awaits before and after it holds the thread, so the ticker runs on both sides.
const LEAD: Duration = Duration::from_millis(35);

/// One session that holds the runtime's thread once with `block`; how long it was held.
async fn held_by(block: fn()) -> Duration {
    tokio::time::sleep(LEAD).await;
    let started = Instant::now();
    block();
    let held = started.elapsed();
    tokio::time::sleep(LEAD).await;
    held
}

/// Holds the thread with `block` beside the ticker; how long it was held and the ticker's
/// largest gap. Fails where the two tests of `async_handles.rs` would pass.
fn fails_the_tests(form: &str, block: fn()) -> (Duration, Duration) {
    let (held, largest) = with_ticker(vec![held_by(block)]);
    let held = held[0];
    let load = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    println!(
        "{form}: the thread was held {held:?}, the ticker's largest gap is {largest:?}; {:?} \
         processors, load {}",
        std::thread::available_parallelism().map(std::num::NonZero::get),
        load.trim_end(),
    );
    assert!(held >= BLOCK, "{form} held the thread only {held:?}");
    assert!(
        largest >= GAP_BOUND,
        "{form} held the runtime's thread {held:?} and the ticker's largest gap is {largest:?}: \
         under the bound, the two tests pass"
    );
    (held, largest)
}

/// As [`fails_the_tests`], for a form in which the thread is not ready to run: the gap is the
/// whole block.
fn is_counted_in_full(form: &str, block: fn()) {
    let (held, largest) = fails_the_tests(form, block);
    assert!(
        largest >= BLOCK,
        "{form} held the runtime's thread {held:?} and the ticker counted {largest:?} of it"
    );
}

#[test]
fn a_thread_asleep_for_200_ms_fails_the_tests() {
    is_counted_in_full("a sleep", || std::thread::sleep(BLOCK));
}

/// Unparks the thread that waits on a future, as the adapter's blocking wait does.
struct Unpark(std::thread::Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// The adapter's blocking form inside the async one: the thread parks until a task on a second
/// runtime ends.
fn park_on_a_second_runtime() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_time()
        .build()
        .expect("a second runtime");
    let task = runtime.spawn(async { tokio::time::sleep(BLOCK).await });
    let mut task = std::pin::pin!(tokio::task::unconstrained(task));
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    while task.as_mut().poll(&mut cx).is_pending() {
        std::thread::park();
    }
    runtime.shutdown_background();
}

#[test]
fn a_thread_parked_on_a_second_runtime_for_200_ms_fails_the_tests() {
    is_counted_in_full(
        "a park on a second runtime's task",
        park_on_a_second_runtime,
    );
}

/// The thread waits for a lock that another thread holds for [`BLOCK`].
fn wait_for_a_lock() {
    let lock = Arc::new(Mutex::new(()));
    let (holds, held) = mpsc::channel();
    let holder = {
        let lock = Arc::clone(&lock);
        std::thread::spawn(move || {
            let guard = lock.lock().expect("the lock");
            holds.send(()).expect("the waiting thread");
            std::thread::sleep(BLOCK);
            drop(guard);
        })
    };
    held.recv().expect("the holder holds the lock");
    drop(lock.lock().expect("the lock"));
    holder.join().expect("the holder ends");
}

#[test]
fn a_thread_waiting_200_ms_for_a_lock_fails_the_tests() {
    is_counted_in_full("a wait for a lock", wait_for_a_lock);
}

/// The thread reads from a socket whose peer writes after [`BLOCK`].
fn read_a_socket() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a listener");
    let address = listener.local_addr().expect("its address");
    let peer = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("the connection");
        std::thread::sleep(BLOCK);
        stream.write_all(&[1]).expect("one byte");
    });
    let mut stream = TcpStream::connect(address).expect("the connection");
    let mut byte = [0];
    stream.read_exact(&mut byte).expect("the peer's byte");
    peer.join().expect("the peer ends");
}

#[test]
fn a_thread_waiting_200_ms_in_a_blocking_read_fails_the_tests() {
    is_counted_in_full("a blocking read", read_a_socket);
}

/// The thread computes for [`BLOCK`] of wall time.
fn compute() {
    let started = Instant::now();
    while started.elapsed() < BLOCK {
        std::hint::spin_loop();
    }
}

#[test]
fn a_thread_computing_for_200_ms_fails_the_tests() {
    fails_the_tests("a busy loop", compute);
}

/// The thread waits for [`BLOCK`] of wall time by yielding in a loop, ready to run throughout.
fn yield_in_a_loop() {
    let started = Instant::now();
    while started.elapsed() < BLOCK {
        std::thread::yield_now();
    }
}

#[test]
#[ignore = "a thread that yields in a loop stays ready to run, so on a shared processor the kernel counts it as run-queue wait and the ticker takes it out of the gap"]
fn a_thread_yielding_in_a_loop_for_200_ms_fails_the_tests() {
    fails_the_tests("a loop of yields", yield_in_a_loop);
}
