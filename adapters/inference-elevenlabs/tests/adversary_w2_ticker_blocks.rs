//! Adversary, story:ticker-tests-hold-on-a-loaded-host and
//! story:ticker-test-holds-under-contention: the ticker of `async_handles.rs` against a runtime
//! thread that is held for 200 ms in other ways than `std::thread::sleep`, on a processor other
//! threads contend for.
//!
//! The first story's acceptance: "A change that blocks the runtime for 200 ms inside a synthesis
//! or a recognition fails both tests." The second's: the test fails on that block also when the
//! processors are contended. The ticker takes out of each gap the time the kernel counted the
//! runtime's thread as ready to run without a processor, and at most half of the gap
//! (`support/ticker.rs`). Half, because that count is not the host's alone: a thread that spins
//! or yields while it holds the runtime is counted as ready whenever it is preempted, and on the
//! build node at a load average of 86 the count took a 200 ms sleep down to a gap of 11.2 ms. A
//! gap is therefore never counted as less than half of its wall time, so a hold of 200 ms leaves
//! at least 100 ms, twice the 50 ms bound, whatever the kernel counted; a gap of the host alone
//! still passes the bound up to 100 ms of wall time, where the most counted in one gap on a
//! loaded host was 18 ms.
//!
//! Each case pins the test to one processor beside [`LOADERS`] threads that spin there
//! ([`Contended`]), so the runtime's thread gets about a ninth of that processor: a busy loop of
//! 200 ms computes about 22 ms of it and is counted as ready for the rest. Each case holds the
//! runtime's thread once for 200 ms, in one form, between two awaits, and asserts the opposite of
//! what the two tests assert: the largest gap is at least the bound, so a handle that held the
//! thread that way would fail them. The forms that wait also assert that the wait is counted in
//! full: the largest gap is at least the hold less what the kernel counted as ready inside the
//! hold, which for a sleep, a park, a lock or a read is only the wait for a processor once the
//! wait ended. The last case holds nothing beside the same load and passes the bound.
//!
//! The ticker is the one of `async_handles.rs`, `support/ticker.rs`.

#[path = "support/ticker.rs"]
mod ticker;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Context, Wake, Waker};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ticker::{GAP_BOUND, ReadyWait, with_ticker};

/// How long each form holds the runtime's thread: the story's 200 ms.
const BLOCK: Duration = Duration::from_millis(200);
/// What a session awaits before and after it holds the thread, so the ticker runs on both sides.
const LEAD: Duration = Duration::from_millis(35);
/// How many threads spin on the test's processor: with the runtime's thread there are nine, so
/// a busy loop of [`BLOCK`] computes about 22 ms, under the bound, and is counted as ready for
/// the rest.
const LOADERS: usize = 8;

/// The test pinned to one processor beside [`LOADERS`] threads that spin there, until dropped.
struct Contended {
    processor: String,
    stop: Arc<AtomicBool>,
    loaders: Vec<JoinHandle<()>>,
}

impl Contended {
    /// Pins every thread of the test to the processor the calling thread runs on, with
    /// `taskset`, then starts the loaders, which are pinned there with it. Threads started later
    /// are pinned there too.
    fn start() -> Self {
        let stat = std::fs::read_to_string("/proc/thread-self/stat").expect("the thread's stat");
        let processor = stat
            .rsplit_once(')')
            .and_then(|(_, fields)| fields.split_whitespace().nth(36))
            .expect("the processor field of the thread's stat")
            .to_owned();
        let pinned = std::process::Command::new("taskset")
            .args(["--all-tasks", "--cpu-list", "--pid", &processor])
            .arg(std::process::id().to_string())
            .output()
            .expect("taskset runs");
        assert!(
            pinned.status.success(),
            "taskset pins the test to processor {processor}: {pinned:?}"
        );
        let stop = Arc::new(AtomicBool::new(false));
        let loaders = (0..LOADERS)
            .map(|_| {
                let stop = Arc::clone(&stop);
                std::thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        std::hint::spin_loop();
                    }
                })
            })
            .collect();
        Self {
            processor,
            stop,
            loaders,
        }
    }
}

impl Drop for Contended {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for loader in self.loaders.drain(..) {
            let _ = loader.join();
        }
    }
}

/// What one session measured of its hold: how long it held the runtime's thread, and how much
/// of that the kernel counted as ready to run without a processor.
#[derive(Debug, Clone, Copy)]
struct Hold {
    held: Duration,
    ready: Duration,
}

/// One session that holds the runtime's thread once with `block`; what it measured of the hold.
async fn held_by(block: fn()) -> Hold {
    let mut ready = ReadyWait::of_this_thread();
    tokio::time::sleep(LEAD).await;
    let ready_before = ready.so_far();
    let started = Instant::now();
    block();
    let held = started.elapsed();
    let ready = ready.so_far().saturating_sub(ready_before);
    tokio::time::sleep(LEAD).await;
    Hold { held, ready }
}

/// Holds the thread with `block` beside the ticker on a contended processor; what the session
/// measured of the hold and the ticker's largest gap. Fails where the two tests of
/// `async_handles.rs` would pass.
fn fails_the_tests(form: &str, block: fn()) -> (Hold, Duration) {
    let contended = Contended::start();
    let (hold, largest) = with_ticker(vec![held_by(block)]);
    let hold = hold[0];
    let load = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    println!(
        "{form}: the thread was held {:?}, {:?} of it counted as ready; the ticker's largest gap \
         is {largest:?}; processor {} beside {LOADERS} loaders, load {}",
        hold.held,
        hold.ready,
        contended.processor,
        load.trim_end(),
    );
    drop(contended);
    assert!(hold.held >= BLOCK, "{form} held the thread only {hold:?}");
    assert!(
        largest >= GAP_BOUND,
        "{form} held the runtime's thread {hold:?} and the ticker's largest gap is {largest:?}: \
         under the bound, the two tests pass"
    );
    (hold, largest)
}

/// As [`fails_the_tests`], for a form in which the thread waits and is not ready to run: the gap
/// is the whole hold, less only what the kernel counted as ready inside it.
fn is_counted_in_full(form: &str, block: fn()) {
    let (hold, largest) = fails_the_tests(form, block);
    assert!(
        largest >= hold.held.saturating_sub(hold.ready),
        "{form} held the runtime's thread {hold:?} and the ticker counted {largest:?} of it"
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
fn a_thread_yielding_in_a_loop_for_200_ms_fails_the_tests() {
    fails_the_tests("a loop of yields", yield_in_a_loop);
}

/// The control: a session that holds nothing, beside the same load, passes the bound, so the
/// cases above fail on their hold and not on the load.
#[test]
fn a_thread_held_by_nothing_passes_the_tests_beside_the_same_load() {
    let contended = Contended::start();
    let (hold, largest) = with_ticker(vec![held_by(|| {})]);
    println!(
        "nothing: the ticker's largest gap is {largest:?}, {:?} counted as ready in the session; \
         processor {} beside {LOADERS} loaders",
        hold[0].ready, contended.processor,
    );
    drop(contended);
    assert!(
        largest < GAP_BOUND,
        "the ticker's largest gap {largest:?} beside the load, with nothing held"
    );
}
