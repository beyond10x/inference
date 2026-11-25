//! Adversary, story:synthesiser-drop-inside-runtime: the story's acceptance against a close that
//! cannot be written, with no change in `src`.
//!
//! The story's acceptance: "Dropping an idle synthesis with an open connection on a
//! current-thread runtime, with a 10 ms ticker task running, keeps the ticker's largest gap under
//! 50 ms." The unit's own test of it meets a server whose buffer has room, where the close is one
//! small write, and so it passes with the drop that waits for its close as well.
//!
//! Here the close cannot be written. The server of `support/deaf.rs` reads the segment's three
//! messages and nothing after them, sends a flood of pings and then the segment's audio and its
//! final message. The segment completes, the synthesis is idle with its connection open, and its
//! `close_socket` and close frame wait behind the pongs until `CLOSE_BOUND` has passed. The
//! server tells each case whether its flood backed the connection up, and a flood that did not
//! is doubled, so no case depends on how much the host's socket buffers hold.
//!
//! - A drop that waits for its close holds the runtime's thread for `CLOSE_BOUND` there and
//!   fails the ticker's bound. The first case also asserts that the close was still waiting
//!   300 ms after the drop, so it cannot pass because the scenario built no slow close, and that
//!   the synthesis's own thread ends once `CLOSE_BOUND` has passed.
//! - Outside a Tokio runtime the drop waits `CLOSE_BOUND` and no longer: on a thread with no
//!   executor, and inside an executor that is not Tokio.
//! - The hold of the drop inside a Tokio runtime is bounded on both sides: at least
//!   `CLOSE_GRACE`, because the close cannot end sooner, and at most [`HOLD_SLACK`] more.
//! - The server reads again after a drop handed its close off, while the synthesis's own thread
//!   still waits to write it: `src/lib.rs` says such a close "runs to its end on the handle's
//!   own thread".

mod support;

#[path = "support/deaf.rs"]
mod deaf;
#[allow(dead_code)]
#[path = "support/dropped.rs"]
mod dropped;
#[path = "support/ticker.rs"]
mod ticker;

use std::future::Future;
use std::sync::Arc;
use std::sync::mpsc;
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use deaf::{Deaf, FIXED, OWN_THREAD, on_a_backed_up_connection};
use dropped::{CLOSE, TEXT, threads_named};
use inference_elevenlabs::{
    CLOSE_BOUND, CLOSE_GRACE, ElevenLabsSynthesis, ElevenLabsSynthesiser, Endpoint, OutputFormat,
    VoiceConfig,
};
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use ticker::{GAP_BOUND, with_ticker};

/// The first flood of the case that compares descriptors: a quarter of [`FIXED`], so that a
/// session whose close was written runs before the one whose close is backed up.
const FIRST: usize = FIXED / 4;

/// How long past `CLOSE_GRACE` the drop may hold the runtime's thread: what a parked thread may
/// take to run again on a busy host.
const HOLD_SLACK: Duration = Duration::from_millis(15);

/// How long past `CLOSE_BOUND` a drop that waits for its close may hold its thread inside an
/// executor that is not Tokio, and how long before `CLOSE_BOUND` it may return. Measured there
/// the drop held its thread 1.0001 to 1.0002 s over five runs, and outside a runtime, where it
/// waits the same way, 1.0001 to 1.0054 s over twenty runs of the suite. This is about twenty
/// times the largest overshoot, and the lower bound it leaves, 900 ms, is 180 times
/// `CLOSE_GRACE`.
const BOUND_SLACK: Duration = Duration::from_millis(100);

/// What a session awaits before it drops its handle, so the ticker runs before the drop.
const LEAD: Duration = Duration::from_millis(35);

/// How long after the drop the session looks whether the close is still waiting.
const LOOK: Duration = Duration::from_millis(300);

/// How long after the drop the server of the last case reads again.
const AGAIN: Duration = Duration::from_millis(100);

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

/// A synthesiser of the fixture voice at `url`.
fn at(url: &str) -> ElevenLabsSynthesiser {
    ElevenLabsSynthesiser::new(
        Endpoint::new()
            .with_url(url)
            .with_read_timeout(Duration::from_secs(30))
            .with_connect_timeout(Duration::from_secs(5)),
        VoiceConfig::new(support::VOICE, OutputFormat::Ulaw8000),
        Arc::new(support::FixedKey(support::KEY)),
        "elevenlabs-api-key",
    )
    .expect("the fixture configuration is valid")
}

/// One synthesis at `url` that speaks a segment through the async form and is then idle, its
/// utterance open and its connection with it.
async fn idle(url: &str) -> ElevenLabsSynthesis {
    let mut synthesiser = at(url);
    let mut handle = synthesiser
        .start()
        .expect("a fresh synthesiser runs nothing");
    handle
        .offer_segment(support::committed(&["First."]).remove(0))
        .expect("the utterance is open");
    let mut completed = false;
    while let Some(event) = handle.event().await {
        completed |= matches!(event, SynthesisEvent::Completed(_));
    }
    assert!(completed, "the segment completes");
    assert_eq!(handle.failure().map(ToString::to_string), None);
    handle
}

/// The idle synthesis at `url`, dropped on the thread of a Tokio runtime.
async fn drop_idle(url: String) -> Seen {
    let handle = idle(&url).await;
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
    let started = Instant::now();
    let ((seen, largest, left, ended), flood) = on_a_backed_up_connection(FIXED, |url| {
        let (seen, largest) = with_ticker(vec![drop_idle(url)]);
        let left = threads_named(OWN_THREAD, 0, CLOSE_BOUND + Duration::from_secs(3));
        let ended = seen[0].dropped.elapsed();
        (seen, largest, left, ended)
    });
    let seen = &seen[0];
    println!(
        "backed up by {} pings: the drop held the thread {:?}, the ticker's largest gap is \
         {largest:?}, own threads before the drop {:?} and {LOOK:?} after it {:?}",
        flood.pings, seen.held, seen.before, seen.waiting
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
    println!(
        "the synthesis's own thread ended within {ended:?} of the drop; the case took {:?}",
        started.elapsed()
    );
    assert_eq!(
        left,
        Some(0),
        "the synthesis's own thread ends once CLOSE_BOUND has passed, {ended:?} after the drop"
    );
}

/// The other half of what `src/lib.rs` says of the drop: "Outside a Tokio runtime the drop
/// waits for that close", "bounded by `CLOSE_BOUND`". On a thread with no runtime, where the
/// close cannot be written, the drop of the idle synthesis waits `CLOSE_BOUND` and no longer.
#[test]
fn dropping_an_idle_synthesis_whose_close_cannot_be_written_outside_a_runtime_waits_close_bound() {
    let (held, flood) = on_a_backed_up_connection(FIXED, |url| {
        let mut synthesiser = at(&url);
        let mut handle = synthesiser
            .start()
            .expect("a fresh synthesiser runs nothing");
        handle
            .offer_segment(support::committed(&["First."]).remove(0))
            .expect("the utterance is open");
        let events: Vec<SynthesisEvent> = handle.by_ref().collect();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, SynthesisEvent::Completed(_))),
            "the segment completes"
        );
        assert_eq!(handle.failure().map(ToString::to_string), None);
        let started = Instant::now();
        drop(handle);
        started.elapsed()
    });
    println!(
        "backed up by {} pings: outside a runtime the drop held its thread {held:?}",
        flood.pings
    );
    assert!(
        held + Duration::from_millis(100) >= CLOSE_BOUND,
        "outside a runtime the drop waits for its close, which cannot be written: {held:?}"
    );
    assert!(
        held <= CLOSE_BOUND + Duration::from_millis(500),
        "the drop waits at most CLOSE_BOUND: {held:?}"
    );
}

/// The story's acceptance on a connection the case knows to be backed up, on any host: the
/// ticker's bound, and the hold of the drop bounded by `CLOSE_GRACE` from below and by
/// `CLOSE_GRACE` and [`HOLD_SLACK`] from above. Its flood starts at [`FIRST`], so that the
/// descriptors a handed-off drop leaves are compared with those of a drop whose close was
/// written.
#[test]
fn a_drop_whose_close_cannot_be_written_holds_the_runtime_for_the_grace_and_no_longer() {
    let started = Instant::now();
    let ((seen, largest), flood) =
        on_a_backed_up_connection(FIRST, |url| with_ticker(vec![drop_idle(url)]));
    let seen = &seen[0];
    println!(
        "backed up by {} pings: the drop held the thread {:?}, the ticker's largest gap is \
         {largest:?}, own threads {LOOK:?} after the drop {:?}; the case took {:?}",
        flood.pings,
        seen.held,
        seen.waiting,
        started.elapsed()
    );
    assert_eq!(
        flood.drained.others,
        Vec::<u8>::new(),
        "a close that could not be written did not arrive: {:?}",
        flood.drained
    );
    assert!(
        largest < GAP_BOUND,
        "the ticker's largest gap {largest:?}; the drop held the runtime's thread {:?}",
        seen.held
    );
    assert_eq!(
        seen.waiting,
        Some(1),
        "the close still waits {LOOK:?} after the drop"
    );
    if let Some((first, last)) = flood.descriptors {
        assert_eq!(
            last, first,
            "a drop that handed its close off leaves no descriptor: the process held {first} \
             after a session whose close was written and {last} after this one"
        );
    }
    assert!(
        seen.held >= CLOSE_GRACE,
        "the drop waits `CLOSE_GRACE` for a close that cannot be written, and held its thread \
         {:?}",
        seen.held
    );
    assert!(
        seen.held < CLOSE_GRACE + HOLD_SLACK,
        "inside a runtime the drop waits at most `CLOSE_GRACE`, {CLOSE_GRACE:?}, and held its \
         thread {:?}",
        seen.held
    );
}

/// Unparks the thread that runs a future when the future is woken.
struct Unpark(std::thread::Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// An executor that is not Tokio: polls `future` on the calling thread until it is ready,
/// parking the thread between polls. No Tokio runtime is current on that thread.
fn run<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        std::thread::park_timeout(Duration::from_secs(1));
    }
}

/// `src/lib.rs`: "The drop asks Tokio: inside another executor it waits as it does outside a
/// runtime, at most [`CLOSE_BOUND`]." The async form needs no Tokio runtime, and an executor
/// that is not Tokio runs it; the idle handle dropped inside that executor's future, where the
/// close cannot be written, holds the executor's thread for `CLOSE_BOUND`, [`BOUND_SLACK`] more
/// or less, and so not for `CLOSE_GRACE` alone.
#[test]
fn a_drop_inside_an_executor_that_is_not_tokio_waits_close_bound_and_not_the_grace() {
    let (held, flood) = on_a_backed_up_connection(FIXED, |url| {
        support::within(Duration::from_secs(30), move || {
            run(async move {
                let handle = idle(&url).await;
                let dropped = Instant::now();
                drop(handle);
                dropped.elapsed()
            })
        })
    });
    println!(
        "backed up by {} pings: inside an executor that is not Tokio the drop held the thread \
         {held:?}",
        flood.pings
    );
    assert!(
        held <= CLOSE_BOUND + BOUND_SLACK,
        "inside an executor that is not Tokio the drop waits at most `CLOSE_BOUND`, and held \
         the executor's thread {held:?}"
    );
    assert!(
        held + BOUND_SLACK >= CLOSE_BOUND,
        "inside an executor that is not Tokio the drop waits for its close as it does outside \
         a runtime, not for {CLOSE_GRACE:?}, and held the executor's thread {held:?}"
    );
}

/// What a session saw of a drop whose server reads again after it.
struct Handed {
    /// How long the drop held the runtime's thread.
    held: Duration,
    /// The synthesis's own threads [`AGAIN`] after the drop, before the server reads again.
    waiting: Option<usize>,
    /// How long after the drop the synthesis's own thread had ended.
    ended: Duration,
}

/// The idle synthesis at `url`, dropped on the thread of a Tokio runtime; [`AGAIN`] later the
/// server is told to read again through `release`, and the session waits for the synthesis's
/// own thread to end.
async fn drop_idle_then_read_again(url: String, release: mpsc::Sender<()>) -> Handed {
    let handle = idle(&url).await;
    let dropped = Instant::now();
    drop(handle);
    let held = dropped.elapsed();
    tokio::time::sleep(AGAIN).await;
    let waiting = threads_named(OWN_THREAD, 1, Duration::ZERO);
    let _ = release.send(());
    while threads_named(OWN_THREAD, 0, Duration::ZERO) != Some(0)
        && dropped.elapsed() < CLOSE_BOUND + Duration::from_secs(3)
    {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    Handed {
        held,
        waiting,
        ended: dropped.elapsed(),
    }
}

/// `src/lib.rs`: "a close not written by then runs to its end on the handle's own thread,
/// bounded by [`CLOSE_BOUND`]". The close of the drop cannot be written and is handed off; the
/// server reads again [`AGAIN`] later, inside `CLOSE_BOUND`. It then reads every pong, one
/// `close_socket` and one close frame after them and nothing else, and the synthesis's own
/// thread ends before `CLOSE_BOUND` has passed.
///
/// The case knows its close was handed off by the synthesis's own thread still being there
/// [`AGAIN`] after the drop; where it is not, the flood is doubled, twice at most.
#[test]
fn a_close_handed_off_is_written_once_when_the_server_reads_again() {
    let mut pings = FIXED;
    loop {
        let server = Deaf::start(pings);
        let release = server.release.clone();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a current-thread runtime");
        let handed = runtime.block_on(drop_idle_then_read_again(server.url.clone(), release));
        drop(runtime);
        let drained = server.finish();
        println!(
            "a flood of {pings} pings: the drop held the thread {:?}, own threads {AGAIN:?} \
             after it {:?}, the own thread ended {:?} after the drop; the server then read \
             {drained:?}",
            handed.held, handed.waiting, handed.ended
        );
        if handed.waiting != Some(1) {
            assert!(
                pings < 4 * FIXED,
                "the precondition of this case does not hold: {AGAIN:?} after the drop no \
                 thread of the synthesis waited to write its close, after a flood of {pings} \
                 pings. Either the socket buffers of this host took every pong, or the drop \
                 left no close waiting"
            );
            pings *= 2;
            continue;
        }
        assert_eq!(
            (drained.pongs, drained.late_pongs),
            (pings, 0),
            "every pong arrives, in front of the close: {drained:?}"
        );
        assert_eq!(
            (drained.others.as_slice(), drained.close_sockets),
            ([TEXT, CLOSE].as_slice(), 1),
            "one `close_socket` and one close frame follow the pongs: {drained:?}"
        );
        assert_eq!(
            drained.unframed, 0,
            "nothing follows the close frame: {drained:?}"
        );
        assert!(
            handed.ended < CLOSE_BOUND,
            "the synthesis's own thread ends when its close is written, {:?} after the drop",
            handed.ended
        );
        return;
    }
}
