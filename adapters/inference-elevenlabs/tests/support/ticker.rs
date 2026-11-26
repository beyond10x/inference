//! A ticker beside the sessions of a test on one current-thread runtime, and its largest gap:
//! what held the runtime's thread, without two waits that are the host's. The table of
//! descriptors is grown before the ticker starts ([`grow_descriptor_table`]), and each gap is
//! counted less the time its thread was ready to run without a processor ([`ReadyWait`]).
//! The ElevenLabs binding's `tests/async_handles.rs` says what was measured of both.
//!
//! The Cartesia adapter's tests include this file by path
//! (`adapters/inference-cartesia/tests/cancel.rs` and its `adversary_w2_drop_backed_up.rs`), and so
//! does `crates/inference-testkit/tests/async_handles.rs`: a change here builds and runs the tests
//! of all three.

use std::cell::Cell;
use std::fs::File;
use std::future::Future;
use std::io::{Read, Seek, SeekFrom};
use std::rc::Rc;
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_millis(10);
/// What the ticker's largest gap stays under while no session holds the runtime's thread.
pub const GAP_BOUND: Duration = Duration::from_millis(50);
/// How long one session may take before the test fails it.
const SESSION_BOUND: Duration = Duration::from_secs(20);

/// How many descriptors [`grow_descriptor_table`] holds at once: the table then has room for
/// 512, where the twenty sessions of one test hold about 130.
const DESCRIPTORS: usize = 300;

/// Grows the process's descriptor table by holding [`DESCRIPTORS`] descriptors at once, fewer
/// where the process may not hold that many. The table does not shrink when they are closed, so
/// no descriptor opened afterwards makes its thread wait for the table to grow.
fn grow_descriptor_table() {
    let Ok(first) = std::env::current_exe().and_then(File::open) else {
        return;
    };
    let held: Vec<File> = (1..DESCRIPTORS)
        .map_while(|_| first.try_clone().ok())
        .collect();
    drop(held);
}

/// How long the thread that opened it has been ready to run without a processor, as Linux counts
/// it in the second field of `/proc/thread-self/schedstat`. The file stays open and is read
/// again from its start, so a reading opens no descriptor. Where there is no such file every
/// reading is zero and nothing is taken out of a gap.
///
/// The limit: the field cannot tell a late host from a thread that holds itself ready to run. A
/// thread that waits in a loop of yields, or spins and is preempted, is counted here while
/// another thread is ready on its processor, and that time is taken out of the gap.
struct ReadyWait(Option<File>);

impl ReadyWait {
    fn of_this_thread() -> Self {
        Self(File::open("/proc/thread-self/schedstat").ok())
    }

    fn so_far(&mut self) -> Duration {
        let mut text = String::new();
        let read = self.0.as_mut().is_some_and(|file| {
            file.seek(SeekFrom::Start(0)).is_ok() && file.read_to_string(&mut text).is_ok()
        });
        text.split_whitespace()
            .nth(1)
            .filter(|_| read)
            .and_then(|nanoseconds| nanoseconds.parse().ok())
            .map_or(Duration::ZERO, Duration::from_nanos)
    }
}

/// Runs `sessions` on a current-thread runtime beside a ticker that ticks every [`TICK`] until
/// they are done; returns what they returned and the ticker's largest gap, each gap less the
/// time the runtime's thread was ready to run in it without a processor ([`ReadyWait`]). The
/// descriptor table is grown first ([`grow_descriptor_table`]).
pub fn with_ticker<F, T>(sessions: Vec<F>) -> (Vec<T>, Duration)
where
    F: Future<Output = T> + 'static,
    T: 'static,
{
    grow_descriptor_table();
    let mut ready = ReadyWait::of_this_thread();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime");
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async move {
        let done = Rc::new(Cell::new(false));
        let ticker = {
            let done = Rc::clone(&done);
            tokio::task::spawn_local(async move {
                let mut largest = Duration::ZERO;
                let mut last = (Instant::now(), ready.so_far());
                while !done.get() {
                    tokio::time::sleep(TICK).await;
                    let now = (Instant::now(), ready.so_far());
                    let late = now.1.saturating_sub(last.1);
                    largest = largest.max((now.0 - last.0).saturating_sub(late));
                    last = now;
                }
                largest
            })
        };
        let running: Vec<_> = sessions
            .into_iter()
            .map(|session| {
                tokio::task::spawn_local(async move {
                    tokio::time::timeout(SESSION_BOUND, session)
                        .await
                        .expect("the session ends within its bound")
                })
            })
            .collect();
        let mut results = Vec::new();
        for session in running {
            results.push(session.await.expect("a session task ends without a panic"));
        }
        done.set(true);
        let largest = ticker.await.expect("the ticker ends without a panic");
        (results, largest)
    })
}
