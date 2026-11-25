//! Cancel: a cancel mid-recognition closes the socket, and no event follows it, though the server
//! had more to send; a dropped handle closes its socket the same way; and a cancel whose close
//! frame cannot be written ends its connection thread at the close bound.

mod support;

use std::time::{Duration, Instant};

use inference_deepgram::ListenConfig;
use inference_stt::{Recogniser, RecognitionHandle};
use support::{Action, FixtureServer, Script, Step, Trigger, describe, request, results, tone};

#[test]
fn a_cancel_closes_the_socket_and_nothing_follows() {
    let server = FixtureServer::start(Script::fixture("conversation.jsonl"));
    let mut recogniser = server.recogniser();
    let mut handle = recogniser.start(request()).unwrap();
    for frame in tone(3) {
        handle.offer_frame(frame).unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let first = loop {
        if let Some(event) = handle.next() {
            break event;
        }
        assert!(Instant::now() < deadline, "no hypothesis arrived");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(describe(&first), "hypothesis hello");
    handle.cancel();
    assert_eq!(
        server.next_client_close(Duration::from_secs(5)),
        Some(0),
        "the server sees the connection close"
    );
    assert!(server.seen()[0].client_closed);
    assert!(handle.next().is_none());
    handle.end_input().unwrap();
    assert!(handle.next().is_none(), "no usage record after a cancel");
    assert!(handle.failure().is_none());
    assert_eq!(server.seen()[0].close_streams, 0);
}

#[test]
fn a_dropped_handle_closes_its_socket_and_frees_its_slot() {
    let server = FixtureServer::start(Script::fixture("conversation.jsonl"));
    let mut recogniser = server.recogniser();
    let mut handle = recogniser.start(request()).unwrap();
    handle.offer_frame(tone(1).remove(0)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while handle.next().is_none() {
        assert!(
            Instant::now() < deadline,
            "no hypothesis arrived, so the socket never opened"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(recogniser.start(request()).is_err(), "one at a time");
    drop(handle);
    assert_eq!(server.next_client_close(Duration::from_secs(5)), Some(0));
    assert!(recogniser.start(request()).is_ok());
}

/// The pings of the first flood of the backed-up cancel. The connection answers each with a
/// pong of 131 bytes; 36,000 of them are 4.7 MB, more than the socket buffers hold where the
/// kernel's largest send buffer is 4 MiB.
const FLOOD: usize = 36_000;

/// The largest flood: a host whose buffers took every pong gets the flood doubled up to here.
/// It is one the connection reads within the test's wait for the hypothesis behind it, so a
/// host that took this one too fails on that and not on the wait.
const LARGEST_FLOOD: usize = 8 * FLOOD;

/// The test's own deadline for a connection thread that nothing ends: past it the test lets the
/// server read again, which ends the thread, and fails.
const THREAD_DEADLINE: Duration = Duration::from_secs(4);

/// The kernel's name of a recognition's connection thread, the first 15 bytes of
/// `inference-deepgram`. The kernel keeps no more of a name, so the crate's clock thread,
/// `inference-deepgram-clock`, is listed under the same 15 bytes.
const THREAD: &str = "inference-deepg";

/// How many threads of this process the kernel lists under a name that starts with `name`. It
/// reads `/proc/self/task`, which Linux has; the gate runs on Linux.
fn threads_named(name: &str) -> usize {
    std::fs::read_dir("/proc/self/task")
        .expect("the kernel lists the threads of this process")
        .flatten()
        .filter(|task| {
            std::fs::read_to_string(task.path().join("comm"))
                .is_ok_and(|comm| comm.starts_with(name))
        })
        .count()
}

/// `cancel` returns at once and the close runs on the recognition's connection thread, so what
/// this test times is the end of that thread. After the first audio message the server sends a
/// flood of pings and one `Results`, and reads nothing more. The connection answers each ping
/// with a pong as it reads; the pongs fill the socket buffers and then wait in its WebSocket
/// buffer, in front of the close frame, so that frame cannot be written and only the close
/// bound ends the thread. Whether the connection was backed up is the server's to tell: it had
/// read no pong when the test told it to read again, and reading again once the thread is gone,
/// it finds the stream ended with no close frame and fewer pongs than it sent pings. A server
/// that read on and was only slow would count fewer pongs too, which is why the first count is
/// asserted. A flood whose pongs all arrived is doubled and the recognition run again. The
/// keep-alive interval is set long, so no `KeepAlive` is written first.
///
/// The thread must be gone no sooner than one second after the cancel and before two. Both
/// numbers are written out and not taken from `CLOSE_BOUND`: limits that moved with the
/// constant would pass with any value of it.
///
/// The thread is found by its name, so the test must see no other recognition's thread: it
/// relies on cargo-nextest, which runs each test in a process of its own, and fails where it
/// finds more than its one thread. The name it matches is also the start of the name of the
/// crate's clock thread, which starts when an async handle waits for a deadline and never ends.
/// This test starts none: it takes its events with the blocking `next` and does not end the
/// input. If the clock ran, the count before the cancel would be two and the test would fail
/// there.
#[test]
fn a_cancel_on_a_backed_up_connection_ends_its_thread_at_the_close_bound() {
    let mut pings = FLOOD;
    loop {
        let after_first = |action| Step {
            on: Trigger::Audio(1),
            action,
        };
        let server = FixtureServer::start(Script::Steps(vec![
            after_first(Action::Pings(pings)),
            after_first(Action::Send(results("hello", false, false))),
            after_first(Action::Hold),
        ]));
        let mut config = ListenConfig::new();
        config.keepalive_interval = Duration::from_secs(30);
        let mut recogniser = server.recogniser_with(config);
        let started = Instant::now();
        let mut handle = recogniser.start(request()).unwrap();
        handle.offer_frame(tone(1).remove(0)).unwrap();
        while handle.next().is_none() {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the hypothesis behind a flood of {pings} pings did not arrive"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let flooded = started.elapsed();
        assert_eq!(
            threads_named(THREAD),
            1,
            "the connection threads of this process; another test's would be counted with it"
        );
        let cancelled = Instant::now();
        handle.cancel();
        let returned = cancelled.elapsed();
        let gone = loop {
            if threads_named(THREAD) == 0 {
                break Some(cancelled.elapsed());
            }
            if cancelled.elapsed() >= THREAD_DEADLINE {
                break None;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        server.read_again();
        let ended = server.next_client_close(Duration::from_secs(5));
        while threads_named(THREAD) != 0 {
            assert!(
                cancelled.elapsed() < THREAD_DEADLINE + Duration::from_secs(5),
                "the connection thread outlived the server's reading again"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let seen = server.seen().remove(0);
        println!(
            "a flood of {pings} pings, read within {flooded:?}: the cancel returned after \
             {returned:?} and the connection thread was gone after {gone:?}; the server had \
             read {} pongs when it was told to read again and {} at the end, close frame: {}, \
             end of the stream: {ended:?}",
            seen.early_pongs, seen.pongs, seen.close_frame
        );
        let gone = gone.unwrap_or_else(|| {
            panic!("the connection thread did not end within {THREAD_DEADLINE:?} of the cancel")
        });
        assert_eq!(ended, Some(0), "the server read no end of the stream");
        if seen.pongs < pings {
            assert_eq!(
                seen.early_pongs, 0,
                "the server read pongs before it was told to read again, so fewer pongs than \
                 pings does not say that the connection was backed up"
            );
            assert!(
                !seen.close_frame,
                "a close frame behind pongs never written"
            );
            assert!(
                gone >= Duration::from_secs(1),
                "the thread of a backed-up connection was gone {gone:?} after the cancel, \
                 before the close bound"
            );
            assert!(
                gone < Duration::from_secs(2),
                "the thread of a backed-up connection was gone {gone:?} after the cancel, at \
                 twice the close bound or later"
            );
            return;
        }
        assert!(
            pings < LARGEST_FLOOD,
            "the socket buffers of this host took every pong of a flood of {pings} pings, {} of \
             them before the server was told to read again, so no close was backed up and \
             nothing was asserted of its bound",
            seen.early_pongs
        );
        pings *= 2;
    }
}
