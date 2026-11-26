//! Adversary, story:ticker-tests-hold-on-a-loaded-host and
//! story:ticker-test-holds-under-contention: the ticker of `async_handles.rs` against a runtime
//! thread that is held in other ways than `std::thread::sleep`, on a processor other threads
//! contend for.
//!
//! The first story's acceptance: "A change that blocks the runtime for 200 ms inside a synthesis
//! or a recognition fails both tests." The second's: the test fails on that block also when the
//! processors are contended. The ticker (`support/ticker.rs`) takes out of each gap all of the time
//! the kernel counted the runtime's thread as ready to run without a processor, and nothing else.
//!
//! Each case pins the test to one processor beside [`LOADERS`] threads that spin there
//! ([`Contended`]), so the runtime's thread gets about a ninth of that processor, and holds the
//! runtime's thread once between two awaits. It asserts the opposite of what the two tests assert:
//! the largest gap is at least the bound, so a handle that held the thread that way would fail
//! them. What the ticker counts of a hold is what the hold is made of, so each form plants what the
//! rule counts:
//!
//! - A wait (a sleep, a park, a lock, a read) is not ready to run, so it is counted in full: the
//!   largest gap is at least the hold less what the kernel counted as ready inside it. A ticker
//!   that took out more than the kernel's count fails that. The kernel counts only the wait for a
//!   processor once the wait ended, so a case whose kernel counted more than a quarter of the hold
//!   as ready fails with a message that says so: the kernel then counted the wait itself, as the
//!   build node's did at a load average of 86 (a 200 ms sleep, a gap of 11.2 ms), and no rule of
//!   the ticker can tell that from a late host.
//! - A busy loop plants 200 ms of its own thread's processor time, not 200 ms of wall time: the
//!   time a thread runs is never counted as ready, so the gap holds all of it however long the
//!   thread waited for a processor between its runs.
//! - A loop of yields stays ready to run and is taken out of the gap: the limit
//!   `support/ticker.rs` states. Its case is ignored.
//!
//! Two controls beside the same load pass the bound: a session that holds nothing, and one that
//! computes 15 ms of its own processor time between two awaits, a third of the bound.

#[path = "support/ticker.rs"]
mod ticker;

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Context, Wake, Waker};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ticker::{GAP_BOUND, with_ticker};

/// How long each wait holds the runtime's thread, and how much processor time the busy loop
/// takes: the story's 200 ms.
const BLOCK: Duration = Duration::from_millis(200);
/// What the computing control takes of its own processor time: a third of [`GAP_BOUND`].
const SHORT: Duration = Duration::from_millis(15);
/// How long a busy loop may wait for its processor time before its case fails: beside the
/// loaders it takes about nine times its processor time.
const COMPUTE_LIMIT: Duration = Duration::from_secs(10);
/// What a session awaits before and after it holds the thread, so the ticker runs on both sides.
const LEAD: Duration = Duration::from_millis(35);
/// How many threads spin on the test's processor: with the runtime's thread there are nine.
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

/// The calling thread's `/proc/thread-self/schedstat`: how long it ran, and how long it was ready
/// to run without a processor. Read it on the thread that opened it only.
struct Schedstat(File);

impl Schedstat {
    fn of_this_thread() -> Self {
        Self(File::open("/proc/thread-self/schedstat").expect("the thread's schedstat"))
    }

    /// The time the thread ran and the time it was ready to run, so far.
    fn read(&mut self) -> (Duration, Duration) {
        let mut text = String::new();
        self.0
            .seek(SeekFrom::Start(0))
            .expect("the start of schedstat");
        self.0.read_to_string(&mut text).expect("schedstat");
        let mut fields = text
            .split_whitespace()
            .map(|nanoseconds| Duration::from_nanos(nanoseconds.parse().expect("nanoseconds")));
        let ran = fields.next().expect("the time the thread ran");
        let ready = fields.next().expect("the time the thread was ready");
        (ran, ready)
    }
}

/// What one session measured of its hold: how long it held the runtime's thread, how much of that
/// the kernel counted as ready to run without a processor, and how long the thread ran in it.
#[derive(Debug, Clone, Copy)]
struct Hold {
    held: Duration,
    ready: Duration,
    ran: Duration,
}

/// One session that holds the runtime's thread once with `block`; what it measured of the hold.
/// The ready time is read outside the hold and the run time inside it, so the first is at least
/// the ready time of the hold and the second at most its run time.
async fn held_by(block: fn()) -> Hold {
    let mut schedstat = Schedstat::of_this_thread();
    tokio::time::sleep(LEAD).await;
    let (_, ready_before) = schedstat.read();
    let started = Instant::now();
    let (ran_first, _) = schedstat.read();
    block();
    let (ran_last, _) = schedstat.read();
    let held = started.elapsed();
    let (_, ready_after) = schedstat.read();
    tokio::time::sleep(LEAD).await;
    Hold {
        held,
        ready: ready_after.saturating_sub(ready_before),
        ran: ran_last.saturating_sub(ran_first),
    }
}

/// Holds the thread with `block` beside the ticker on a contended processor; what the session
/// measured of the hold and the ticker's largest gap.
fn beside_the_load(form: &str, block: fn()) -> (Hold, Duration) {
    let contended = Contended::start();
    let (hold, largest) = with_ticker(vec![held_by(block)]);
    let hold = hold[0];
    let load = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    println!(
        "{form}: {hold:?}; the ticker's largest gap is {largest:?}; processor {} beside {LOADERS} \
         loaders, load {}",
        contended.processor,
        load.trim_end(),
    );
    drop(contended);
    (hold, largest)
}

/// As [`beside_the_load`], and fails where the two tests of `async_handles.rs` would pass.
fn fails_the_tests(form: &str, block: fn()) -> (Hold, Duration) {
    let (hold, largest) = beside_the_load(form, block);
    assert!(
        largest >= GAP_BOUND,
        "{form} held the runtime's thread {hold:?} and the ticker's largest gap is {largest:?}: \
         under the bound, the two tests pass"
    );
    (hold, largest)
}

/// As [`fails_the_tests`], for a wait of [`BLOCK`]: the gap is the whole hold less what the kernel
/// counted as ready inside it, and the kernel counted no more than a quarter of the hold.
fn is_counted_in_full(form: &str, block: fn()) {
    let (hold, largest) = beside_the_load(form, block);
    assert!(hold.held >= BLOCK, "{form} held the thread only {hold:?}");
    assert!(
        hold.ready <= hold.held / 4,
        "the kernel counted {:?} of a hold of {:?} by {form} as ready to run: more than a quarter, \
         so it counted the wait itself as ready, and the ticker takes that count as it is \
         (largest gap {largest:?})",
        hold.ready,
        hold.held,
    );
    assert!(
        largest >= hold.held.saturating_sub(hold.ready),
        "{form} held the runtime's thread {hold:?} and the ticker counted {largest:?} of it: it \
         took out more than the kernel counted as ready"
    );
    assert!(
        largest >= GAP_BOUND,
        "{form} held the runtime's thread {hold:?} and the ticker's largest gap is {largest:?}: \
         under the bound, the two tests pass"
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

/// The thread computes until it has run for `time` of its own processor time, however long it
/// waits for a processor between its runs; it fails once [`COMPUTE_LIMIT`] has passed.
fn compute_for(time: Duration) {
    let mut schedstat = Schedstat::of_this_thread();
    let started = Instant::now();
    let (first, _) = schedstat.read();
    loop {
        for _ in 0..10_000 {
            std::hint::spin_loop();
        }
        let ran = schedstat.read().0.saturating_sub(first);
        if ran >= time {
            return;
        }
        assert!(
            started.elapsed() < COMPUTE_LIMIT,
            "the thread ran {ran:?} of its {time:?} of processor time in {COMPUTE_LIMIT:?}"
        );
    }
}

/// The thread computes for [`BLOCK`] of its own processor time.
fn compute() {
    compute_for(BLOCK);
}

#[test]
fn a_thread_computing_for_200_ms_fails_the_tests() {
    let (hold, largest) = fails_the_tests("a busy loop", compute);
    assert!(
        largest >= hold.ran,
        "a busy loop ran {:?} and the ticker counted {largest:?} of it",
        hold.ran
    );
}

/// The thread waits for [`BLOCK`] of wall time by yielding in a loop, ready to run throughout.
fn yield_in_a_loop() {
    let started = Instant::now();
    while started.elapsed() < BLOCK {
        std::thread::yield_now();
    }
}

#[test]
#[ignore = "limit: adapters/inference-elevenlabs/tests/support/ticker.rs states it: a thread that yields in a loop stays ready to run, so on a shared processor the kernel counts it as run-queue wait and the ticker takes it out of the gap"]
fn a_thread_yielding_in_a_loop_for_200_ms_fails_the_tests() {
    fails_the_tests("a loop of yields", yield_in_a_loop);
}

/// A control: a session that holds nothing, beside the same load, passes the bound, so the cases
/// above fail on their hold and not on the load.
#[test]
fn a_thread_held_by_nothing_passes_the_tests_beside_the_same_load() {
    let (hold, largest) = beside_the_load("nothing", || {});
    assert!(
        largest < GAP_BOUND,
        "the ticker's largest gap {largest:?} beside the load, with nothing held: {hold:?}"
    );
}

/// The thread computes for [`SHORT`] of its own processor time.
fn compute_short() {
    compute_for(SHORT);
}

/// A control: a session that computes 15 ms of its own processor time between two awaits, beside
/// the same load, passes the bound, so the busy loop fails on its 200 ms and not on the time it
/// waited for a processor between its runs.
#[test]
fn a_thread_computing_for_15_ms_passes_the_tests_beside_the_same_load() {
    let (hold, largest) = beside_the_load("a short busy loop", compute_short);
    assert!(
        largest < GAP_BOUND,
        "a session that ran {:?} of its own processor time beside the load, and the ticker's \
         largest gap is {largest:?}: {hold:?}",
        hold.ran
    );
}
