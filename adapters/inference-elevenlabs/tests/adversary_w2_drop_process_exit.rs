//! Adversary, story:synthesiser-drop-inside-runtime: what the crate documentation says of a drop
//! inside a Tokio runtime, against a process that ends right after the drop.
//!
//! `src/lib.rs` says of it: the drop waits for its close at most `CLOSE_GRACE`, and "a close
//! that ended within it has been sent when the drop returns"; a close not written by then goes
//! on without the dropping thread, and a process that ends while it still runs ends the socket
//! without the close frame.
//!
//! Each case runs this test binary ten times more, one after another, as a child process. The
//! child speaks one segment through a relay to a fixture server, is then idle with its
//! connection open, drops the handle with a runtime current, and ends as a program does whose
//! `main` returns: the runtime is dropped and the process exits. It reports how long its drop
//! held the thread. The parent then reads what the relay and the server got of each child's
//! connection, and asserts of every child whose drop returned within `CLOSE_GRACE` that the
//! server got `close_socket` and then a close frame, and that at least one child of the ten is
//! such a child. A drop that held its thread for the whole grace handed its close off: whether
//! that close arrives is a race between the child's exit and the handle's own thread, so the
//! parent counts those children and asserts nothing of them.
//!
//! - awaited: the drop is the last statement of the future a current-thread runtime blocks on,
//!   the shape of `#[tokio::main(flavor = "current_thread")] async fn main`.
//! - entered: the drop is on a thread that entered a runtime with `Runtime::enter` and is not
//!   driven by it, after the handle was iterated there.

mod support;

#[allow(dead_code)]
#[path = "support/dropped.rs"]
mod dropped;

use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dropped::{CLOSE, Relay, TEXT};
use inference_elevenlabs::{
    CLOSE_GRACE, ElevenLabsSynthesiser, Endpoint, OutputFormat, VoiceConfig,
};
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};

/// The variable that makes a test of this file the child: the relay's URL.
const CHILD: &str = "ADVERSARY_W2_DROP_CHILD_URL";

/// What the line of a child's output starts with that tells how many nanoseconds its drop held
/// the thread.
const HELD: &str = "the drop held its thread, in nanoseconds: ";

/// A synthesiser of the fixture voice at `url`.
fn at(url: &str) -> ElevenLabsSynthesiser {
    ElevenLabsSynthesiser::new(
        Endpoint::new()
            .with_url(url)
            .with_read_timeout(Duration::from_secs(5))
            .with_connect_timeout(Duration::from_secs(5)),
        VoiceConfig::new(support::VOICE, OutputFormat::Ulaw8000),
        Arc::new(support::FixedKey(support::KEY)),
        "elevenlabs-api-key",
    )
    .expect("the fixture configuration is valid")
}

/// How many child processes each case runs, one after another.
const CHILDREN: usize = 10;

/// Ends the child: reports how long its drop held the thread, and exits.
fn end(held: Duration) -> ! {
    println!("\n{HELD}{}", held.as_nanos());
    std::process::exit(0)
}

/// How long a child's drop held its thread, as the child's output tells it.
fn held_by(output: &[u8]) -> Duration {
    let output = String::from_utf8_lossy(output);
    let nanos = output
        .lines()
        .find_map(|line| line.strip_prefix(HELD))
        .and_then(|nanos| nanos.trim().parse().ok())
        .unwrap_or_else(|| panic!("the child reports how long its drop held: {output}"));
    Duration::from_nanos(nanos)
}

/// Runs the test `test` of this binary as a child process whose synthesis goes through a relay
/// to a fixture server of its own, [`CHILDREN`] times, and asserts what each server got of the
/// connection of a child whose drop returned within `CLOSE_GRACE`: `close_socket`, then a close
/// frame. At least one child is such a child.
fn the_server_got_the_close_of_every_drop_that_returned_within_the_grace(test: &str) {
    let mut lost = Vec::new();
    let mut within = 0;
    let mut handed_off = 0;
    let mut arrived = 0;
    for nth in 0..CHILDREN {
        let server = support::FixtureServer::start(OutputFormat::Ulaw8000, support::Script::Serve);
        let relay = Relay::to(&server.url);
        let child = Command::new(std::env::current_exe().expect("this test binary"))
            .args(["--exact", test, "--nocapture"])
            .env(CHILD, &relay.url)
            .output()
            .expect("the child runs");
        assert!(
            child.status.success(),
            "child {nth} ended {}: {}",
            child.status,
            String::from_utf8_lossy(&child.stderr)
        );
        let held = held_by(&child.stdout);
        let side = relay
            .next_client_side(Duration::from_secs(5))
            .expect("the child's side of the connection ends");
        assert_eq!(server.next_client_close(Duration::from_secs(5)), Some(0));
        let seen = server.seen();
        println!(
            "child {nth}: its drop held the thread {held:?}; the relay read {side:?}; the server \
             read {} message(s), close_socket {}",
            seen[0].messages.len(),
            seen[0].close_socket
        );
        assert_eq!(
            seen[0].contexts.len(),
            1,
            "child {nth} spoke its segment: {:?}",
            seen[0].messages
        );
        let closed = seen[0].close_socket && side.opcodes.last_chunk() == Some(&[TEXT, CLOSE]);
        if held < CLOSE_GRACE {
            within += 1;
            if !closed {
                lost.push((nth, held, side));
            }
        } else {
            handed_off += 1;
            arrived += usize::from(closed);
        }
    }
    println!(
        "{test}: {within} of {CHILDREN} drops returned within {CLOSE_GRACE:?}; {handed_off} \
         handed their close off, and the close of {arrived} of those arrived"
    );
    assert!(
        lost.is_empty(),
        "the server received no `close_socket` and no close frame of {} of {within} handles whose \
         drop returned within {CLOSE_GRACE:?}; the child, its drop's hold and what the relay read \
         of each: {lost:?}",
        lost.len()
    );
    assert!(
        within >= 1,
        "no drop of {CHILDREN} returned within {CLOSE_GRACE:?}: all handed their close off"
    );
}

/// The child of the awaited case: one segment through the async form on a current-thread
/// runtime, the idle handle dropped as the last statement of the future, the runtime dropped,
/// the process ended.
fn end_after_a_drop_in_the_future(url: &str) -> ! {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime");
    let held = runtime.block_on(async {
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
        let started = Instant::now();
        drop(handle);
        started.elapsed()
    });
    drop(runtime);
    end(held)
}

#[test]
fn a_drop_in_a_runtime_that_returned_within_the_grace_has_sent_close_socket_and_the_close_frame() {
    if let Ok(url) = std::env::var(CHILD) {
        end_after_a_drop_in_the_future(&url);
    }
    the_server_got_the_close_of_every_drop_that_returned_within_the_grace(
        "a_drop_in_a_runtime_that_returned_within_the_grace_has_sent_close_socket_and_the_close_frame",
    );
}

/// The child of the entered case: one segment through the iterator on a thread that entered a
/// runtime and is not driven by it, the idle handle dropped there, the process ended.
fn end_after_a_drop_on_an_entered_thread(url: &str) -> ! {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime");
    let entered = runtime.enter();
    let mut synthesiser = at(url);
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
    let held = started.elapsed();
    drop(entered);
    drop(runtime);
    end(held)
}

#[test]
fn a_drop_on_an_entered_thread_that_returned_within_the_grace_has_sent_close_socket_and_the_close_frame()
 {
    if let Ok(url) = std::env::var(CHILD) {
        end_after_a_drop_on_an_entered_thread(&url);
    }
    the_server_got_the_close_of_every_drop_that_returned_within_the_grace(
        "a_drop_on_an_entered_thread_that_returned_within_the_grace_has_sent_close_socket_and_the_close_frame",
    );
}
