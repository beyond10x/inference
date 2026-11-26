//! Adversary, story:synthesiser-drop-inside-runtime: the story's acceptance against a close that
//! cannot be written, with no change in `src`.
//!
//! The story's acceptance: "Dropping an idle synthesis with an open connection on a
//! current-thread runtime, with a 10 ms ticker task running, keeps the ticker's largest gap under
//! 50 ms." The unit's own test of it meets a server whose buffer has room, where the close is one
//! small write, and so it passes with the drop that waits for its close as well.
//!
//! Here nothing can be written. The server of `support/deaf.rs` reads the generation request and
//! nothing after it, sends a flood of pings and then the segment. The segment completes, the
//! synthesis is idle with its connection open, and its close frame waits behind the pongs until
//! `CLOSE_BOUND` has passed. The server tells each case whether its flood backed the connection
//! up, and a flood that did not is doubled, so no case depends on how much the host's socket
//! buffers hold.
//!
//! - A drop that waits for its close holds the runtime's thread for `CLOSE_BOUND` there and
//!   fails the ticker's bound. The first case also asserts that the close was still waiting
//!   300 ms after the drop, and that the synthesis's own thread ends once the bound has passed:
//!   no sooner than 900 ms after the drop and before 2 s.
//! - Outside a Tokio runtime the drop waits for the bound and no longer: at least 900 ms, and at
//!   most 1.1 s once the time its thread was ready to run without a processor is taken out
//!   ([`ReadyWait`]).
//! - The hold of the drop inside a Tokio runtime is bounded on both sides: at least
//!   `CLOSE_GRACE`, because the close cannot end sooner, and at most [`HOLD_SLACK`] more, once
//!   the time its thread was ready to run without a processor is taken out. A drop that yields
//!   or spins instead of parking is counted as ready, and so that limit does not see it.
//! - The server reads again after a drop handed its close off, while the synthesis's own thread
//!   still waits to write it: `src/lib.rs` says such a close "runs to its end on the handle's
//!   own thread".
//! - A synthesis that fails on such a connection waits for its close, and only the bound on
//!   `Connection::close` ends that wait: the one application of the bound of three that a case
//!   reaches alone, held to the stall and 900 ms to 1.2 s more. In a drop that bound starts with
//!   the drop and bounds the same close as each of the other two, no later than they do, so no
//!   case of a drop fails with one of them removed.
//!
//! The limits on a wait for `CLOSE_BOUND` are numbers from [`STATED`], the bound as the crate
//! states it, and not the constant: limits that moved with the constant would pass with any
//! value of it.

mod support;

#[path = "support/deaf.rs"]
mod deaf;
#[allow(dead_code)]
#[path = "../../inference-elevenlabs/tests/support/dropped.rs"]
mod dropped;
#[path = "../../inference-elevenlabs/tests/support/ticker.rs"]
mod ticker;

use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use deaf::{Deaf, FIXED, OWN_THREAD, on_a_backed_up_connection, on_a_backed_up_connection_of};
use dropped::{CLOSE, threads_named};
use inference_cartesia::{
    CLOSE_BOUND, CLOSE_GRACE, CartesiaSynthesis, CartesiaSynthesiser, Endpoint, OutputFormat,
    VoiceConfig,
};
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use support::{committed, within};
use ticker::{GAP_BOUND, ReadyWait, with_ticker};

const FORMAT: OutputFormat = OutputFormat::MULAW_8000;

/// `CLOSE_BOUND` as `src/connection.rs` states it, 1 s, written out. The test
/// `the_time_bound_defaults_are_the_stated_numbers` of `tests/errors.rs` holds the constant to
/// the same number.
const STATED: Duration = Duration::from_secs(1);

/// How long before [`STATED`] a wait for the bound may be seen to end: the lower limits are
/// 900 ms.
const EARLY: Duration = Duration::from_millis(100);

/// How long past [`STATED`] the drop outside a runtime may hold its thread once the time the
/// thread was ready to run without a processor is taken out ([`ReadyWait`]): the margin the
/// ElevenLabs test inside an executor that is not Tokio allows the same wait. A bound of 1.4 s
/// fails it.
const BOUND_SLACK: Duration = Duration::from_millis(100);

/// How long past [`STALL`] and [`STATED`] the iterator of a synthesis that failed on a backed-up
/// connection may end: its wait spans two timers, the stall and the close, and the wake of a
/// parked thread after each, and nothing in it is taken out. A bound on `Connection::close` of
/// 1.4 s fails it.
const STALL_SLACK: Duration = Duration::from_millis(200);

/// The first flood of the case that compares descriptors: a quarter of [`FIXED`], so that a
/// session whose close was written runs before the one whose close is backed up.
const FIRST: usize = FIXED / 4;

/// How long past `CLOSE_GRACE` the drop may hold the runtime's thread once the time the thread
/// was ready to run without a processor is taken out of the hold ([`ReadyWait`]): the drop's own
/// work on a processor and the wake of a parked thread at its deadline. The time taken out is
/// the host's: on a host whose load average was 95 to 122 on 20 processors the drop held its
/// thread 20.4 ms and 31.7 ms, and this limit, counted on the whole hold, failed the test.
///
/// What this does not see: the kernel counts a thread that yields or spins as ready to run, not
/// as waiting, so a drop that yields or spins for its close instead of parking has that time
/// taken out of its hold and passes this limit. The ticker's bound, [`GAP_BOUND`], still holds
/// such a drop to its gap.
const HOLD_SLACK: Duration = Duration::from_millis(15);

/// What a session awaits before it drops its handle, so the ticker runs before the drop.
const LEAD: Duration = Duration::from_millis(35);

/// How long after the drop the session looks whether the close is still waiting.
const LOOK: Duration = Duration::from_millis(300);

/// How long after the drop the server of the last case reads again.
const AGAIN: Duration = Duration::from_millis(100);

/// How long the synthesis of the last case waits for the rest of its segment before it fails as
/// stalled: longer than the flood takes to read, which comes first.
const STALL: Duration = Duration::from_secs(1);

/// A synthesiser of the fixture voice at `url`.
fn at(url: &str) -> CartesiaSynthesiser {
    at_with(url, Duration::from_secs(20))
}

/// A synthesiser of the fixture voice at `url` whose read timeout is `read_timeout`.
fn at_with(url: &str, read_timeout: Duration) -> CartesiaSynthesiser {
    CartesiaSynthesiser::new(
        Endpoint::new()
            .with_url(url)
            .with_read_timeout(read_timeout)
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
    /// How long of the drop the runtime's thread was ready to run without a processor.
    ready: Duration,
    /// When the drop began.
    dropped: Instant,
    /// The synthesis's own threads before the drop.
    before: Option<usize>,
    /// The synthesis's own threads [`LOOK`] after the drop.
    waiting: Option<usize>,
}

impl Seen {
    /// The hold of the drop less the time its thread was ready to run without a processor.
    fn own_hold(&self) -> Duration {
        self.held.saturating_sub(self.ready)
    }
}

/// One synthesis at `url` that speaks a segment through the async form and is then idle, its
/// utterance open and its connection with it.
async fn idle(url: &str) -> CartesiaSynthesis {
    let mut synthesiser = at(url);
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
    handle
}

/// The idle synthesis at `url`, dropped on the thread of a Tokio runtime.
async fn drop_idle(url: String) -> Seen {
    let handle = idle(&url).await;
    let before = threads_named(OWN_THREAD, 1, Duration::ZERO);
    let mut ready = ReadyWait::of_this_thread();
    tokio::time::sleep(LEAD).await;
    let ready_before = ready.so_far();
    let dropped = Instant::now();
    drop(handle);
    let held = dropped.elapsed();
    let ready = ready.so_far().saturating_sub(ready_before);
    tokio::time::sleep(LOOK).await;
    let waiting = threads_named(OWN_THREAD, 1, Duration::ZERO);
    Seen {
        held,
        ready,
        dropped,
        before,
        waiting,
    }
}

/// The acceptance, where the close cannot be written: the drop of the idle synthesis holds the
/// runtime's thread for less than the ticker's bound, the close goes on waiting on the
/// synthesis's own thread, and that thread ends once the bound has passed: no sooner than
/// 900 ms after the drop and before 2 s. The second past [`STATED`] is what the thread takes to
/// end its runtime once the close gave up, and the polls of 5 ms that see it gone, on a busy
/// host; a bound of 2 s fails it.
#[test]
fn dropping_an_idle_synthesis_whose_close_cannot_be_written_does_not_block_the_runtime() {
    within(|| {
        let started = Instant::now();
        let ((seen, largest, left, ended), flood) = on_a_backed_up_connection(FIXED, |url| {
            let (seen, largest) = with_ticker(vec![drop_idle(url)]);
            let left = threads_named(OWN_THREAD, 0, 3 * STATED);
            let ended = seen[0].dropped.elapsed();
            (seen, largest, left, ended)
        });
        let seen = &seen[0];
        println!(
            "backed up by {} pings: the drop held the thread {:?}, {:?} of it ready to run, the \
             ticker's largest gap is {largest:?}, own threads before the drop {:?} and {LOOK:?} \
             after it {:?}",
            flood.pings, seen.held, seen.ready, seen.before, seen.waiting
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
            "the synthesis's own thread ends once the bound has passed; {ended:?} after the drop \
             it was still there"
        );
        assert!(
            ended + EARLY >= STATED,
            "the close cannot be written, so the synthesis's own thread ends no sooner than the \
             bound, {STATED:?}; it ended within {ended:?} of the drop"
        );
        assert!(
            ended < 2 * STATED,
            "the synthesis's own thread ends once the bound, {STATED:?}, has passed, and ended \
             {ended:?} after the drop"
        );
    });
}

/// The other half of what `src/lib.rs` says of the drop: "Outside a Tokio runtime the drop
/// waits for that close", "bounded by `CLOSE_BOUND`". On a thread with no runtime, where the
/// close cannot be written, the drop of the idle synthesis waits for the bound and no longer:
/// at least 900 ms, and at most [`STATED`] and [`BOUND_SLACK`] once the time its thread was ready
/// to run without a processor is taken out ([`ReadyWait`]). On a busy host a thread parked for
/// 1 s may wait longer than that margin to run again; that wait is what is taken out.
#[test]
fn dropping_an_idle_synthesis_whose_close_cannot_be_written_outside_a_runtime_waits_close_bound() {
    within(|| {
        let ((held, ready), flood) = on_a_backed_up_connection(FIXED, |url| {
            let mut synthesiser = at(&url);
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
            let mut ready = ReadyWait::of_this_thread();
            let ready_before = ready.so_far();
            let started = Instant::now();
            drop(handle);
            let held = started.elapsed();
            (held, ready.so_far().saturating_sub(ready_before))
        });
        println!(
            "backed up by {} pings: outside a runtime the drop held its thread {held:?}, \
             {ready:?} of it ready to run",
            flood.pings
        );
        assert!(
            held + EARLY >= STATED,
            "outside a runtime the drop waits for its close, which cannot be written: {held:?}"
        );
        assert!(
            held.saturating_sub(ready) <= STATED + BOUND_SLACK,
            "the drop waits at most the bound, {STATED:?}, and {BOUND_SLACK:?}: held {held:?}, \
             {ready:?} of it ready to run"
        );
    });
}

/// The story's acceptance on a connection the case knows to be backed up, on any host: the
/// ticker's bound, and the hold of the drop bounded by `CLOSE_GRACE` from below and by
/// `CLOSE_GRACE` and [`HOLD_SLACK`] from above, the upper limit on the hold less the time its
/// thread was ready to run without a processor. Its flood starts at [`FIRST`], so that the
/// descriptors a handed-off drop leaves are compared with those of a drop whose close was
/// written.
#[test]
fn a_drop_whose_close_cannot_be_written_holds_the_runtime_for_the_grace_and_no_longer() {
    within(|| {
        let started = Instant::now();
        let ((seen, largest), flood) =
            on_a_backed_up_connection(FIRST, |url| with_ticker(vec![drop_idle(url)]));
        let seen = &seen[0];
        println!(
            "backed up by {} pings: the drop held the thread {:?}, {:?} of it ready to run, the \
             ticker's largest gap is {largest:?}, own threads {LOOK:?} after the drop {:?}; the \
             case took {:?}",
            flood.pings,
            seen.held,
            seen.ready,
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
                "a drop that handed its close off leaves no descriptor: the process held \
                 {first} after a session whose close was written and {last} after this one"
            );
        }
        assert!(
            seen.held >= CLOSE_GRACE,
            "the drop waits `CLOSE_GRACE` for a close that cannot be written, and held its \
             thread {:?}",
            seen.held
        );
        assert!(
            seen.own_hold() < CLOSE_GRACE + HOLD_SLACK,
            "inside a runtime the drop waits at most `CLOSE_GRACE`, {CLOSE_GRACE:?}, and held \
             its thread {:?}, {:?} of it ready to run without a processor",
            seen.held,
            seen.ready
        );
    });
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
    while threads_named(OWN_THREAD, 0, Duration::ZERO) != Some(0) && dropped.elapsed() < 4 * STATED
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
/// server reads again [`AGAIN`] later, inside `CLOSE_BOUND`. It then reads every pong, one close
/// frame after them and nothing else, and the synthesis's own thread ends before `CLOSE_BOUND`
/// has passed.
///
/// The case knows its close was handed off by the synthesis's own thread still being there
/// [`AGAIN`] after the drop; where it is not, the flood is doubled, twice at most.
#[test]
fn a_close_handed_off_is_written_once_when_the_server_reads_again() {
    within(|| {
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
                drained.others,
                [CLOSE],
                "one close frame follows the pongs: {drained:?}"
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
    });
}

/// The bound on `Connection::close` (`src/connection.rs`) alone, where no drop is: a synthesis
/// that fails closes its connection as an exchange that its next step waits for with no
/// deadline of its own (`fail` in `src/synthesiser.rs`), so where the close cannot be written
/// only that bound ends the wait. The server floods pings, sends one chunk of the segment and
/// then nothing. The synthesis fails as stalled [`STALL`] after the chunk, and the blocking
/// iterator ends once the close gave up: no sooner than [`STALL`] and [`STATED`] less [`EARLY`]
/// after the chunk's audio, and no later than [`STALL`], [`STATED`] and [`STALL_SLACK`]. A bound
/// on `Connection::close` of 1.4 s or of 600 ms fails it, while every case of a drop passes. The
/// connection is gone before the handle is
/// dropped, so the drop waits for nothing. With that bound removed the iterator does not end
/// and the case fails at the limit of its fixture, `support::DEADLINE`.
#[test]
fn a_synthesis_that_fails_on_a_backed_up_connection_ends_at_the_close_bound() {
    within(|| {
        let ((ended, failure), flood) =
            on_a_backed_up_connection_of(Deaf::stalling, FIXED, |url| {
                let mut synthesiser = at_with(&url, STALL);
                let mut handle = synthesiser
                    .start()
                    .expect("a fresh synthesiser runs nothing");
                handle
                    .offer_segment(committed(&["Speak, then stall."]).remove(0))
                    .expect("the utterance is open");
                let mut audio = None;
                for event in handle.by_ref() {
                    if matches!(event, SynthesisEvent::Audio(_)) && audio.is_none() {
                        audio = Some(Instant::now());
                    }
                }
                let ended = audio.map(|audio| audio.elapsed());
                (ended, handle.failure().map(ToString::to_string))
            });
        println!(
            "backed up by {} pings: the iterator ended {ended:?} after the chunk's audio, the \
             synthesis failed with {failure:?}; the server then read {:?}",
            flood.pings, flood.drained
        );
        assert_eq!(
            failure,
            Some(format!("no server message within {STALL:?}")),
            "the synthesis fails as stalled after the chunk"
        );
        let ended = ended.expect("the chunk's audio arrives before the stall");
        assert_eq!(
            flood.drained.others,
            Vec::<u8>::new(),
            "a close that could not be written did not arrive: {:?}",
            flood.drained
        );
        assert!(
            ended + EARLY >= STALL + STATED,
            "the close cannot be written, so the iterator waits for the bound, {STATED:?}, after \
             the stall, {STALL:?}; it ended {ended:?} after the chunk's audio"
        );
        assert!(
            ended <= STALL + STATED + STALL_SLACK,
            "the close of a failed synthesis waits at most the bound, {STATED:?}, after the \
             stall, {STALL:?}, and {STALL_SLACK:?}; the iterator ended {ended:?} after the chunk's \
             audio"
        );
    });
}
