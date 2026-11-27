//! Adversary, story:synthesiser-drop-inside-runtime: what the crate documentation says of a drop
//! inside a Tokio runtime, against a process that ends right after the drop.
//!
//! `src/lib.rs` says of it: the drop waits for its close at most `CLOSE_GRACE`, and "a close
//! that ended within it has been sent when the drop returns"; a close not written by then goes
//! on without the dropping thread, and a process that ends while it still runs ends the socket
//! without the close frame.
//!
//! The case runs this test binary ten times more, one after another, as a child process. The
//! child speaks one segment through a relay to a fixture server, is then idle with its
//! connection open, drops the handle as the last statement of the future a current-thread
//! runtime blocks on, and ends as a program does whose `main` returns: the runtime is dropped
//! and the process exits. It reports how long its drop held the thread. The parent then reads
//! what the relay got of each child's connection, and asserts of every child whose drop returned
//! within `CLOSE_GRACE` that the relay read the request and then a close frame, and that at
//! least one child of the ten is such a child. A drop that held its thread for the whole grace
//! handed its close off: whether that close arrives is a race between the child's exit and the
//! handle's own thread, so the parent counts those children and asserts nothing of them.

mod support;

#[allow(dead_code)]
#[path = "../../inference-elevenlabs/tests/support/dropped.rs"]
mod dropped;

use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dropped::{CLOSE, Relay, TEXT};
use inference_cartesia::{CLOSE_GRACE, CartesiaSynthesiser, Endpoint, OutputFormat, VoiceConfig};
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use support::{FixtureServer, Script, committed, within};

const FORMAT: OutputFormat = OutputFormat::MULAW_8000;

/// The variable that makes the test of this file the child: the relay's URL.
const CHILD: &str = "ADVERSARY_W2_DROP_CHILD_URL";

/// What the line of a child's output starts with that tells how many nanoseconds its drop held
/// the thread.
const HELD: &str = "the drop held its thread, in nanoseconds: ";

/// How many child processes the case runs, one after another.
const CHILDREN: usize = 10;

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

/// The child: one segment through the async form on a current-thread runtime, the idle handle
/// dropped as the last statement of the future, the runtime dropped, how long the drop held the
/// thread reported, the process ended.
fn end_after_a_drop_in_the_future(url: &str) -> ! {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime");
    let held = runtime.block_on(async {
        let mut synthesiser = CartesiaSynthesiser::new(
            Endpoint::new()
                .with_url(url)
                .with_read_timeout(Duration::from_secs(5))
                .with_connect_timeout(Duration::from_secs(5)),
            VoiceConfig::new(support::VOICE, FORMAT),
            Arc::new(support::FixedKey(support::KEY)),
            "cartesia-api-key",
        )
        .expect("the fixture configuration is valid");
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
        let started = Instant::now();
        drop(handle);
        started.elapsed()
    });
    drop(runtime);
    println!("\n{HELD}{}", held.as_nanos());
    std::process::exit(0)
}

#[test]
fn a_drop_in_a_runtime_that_returned_within_the_grace_has_sent_the_close_frame() {
    const TEST: &str =
        "a_drop_in_a_runtime_that_returned_within_the_grace_has_sent_the_close_frame";
    if let Ok(url) = std::env::var(CHILD) {
        end_after_a_drop_in_the_future(&url);
    }
    within(|| {
        let mut lost = Vec::new();
        let mut returned = 0;
        let mut handed_off = 0;
        let mut arrived = 0;
        for nth in 0..CHILDREN {
            let server = FixtureServer::start(FORMAT, Script::Serve);
            let relay = Relay::to(&server.url);
            let child = Command::new(std::env::current_exe().expect("this test binary"))
                .args(["--exact", TEST, "--nocapture"])
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
            println!("child {nth}: its drop held the thread {held:?}; the relay read {side:?}");
            assert_eq!(
                server.seen()[0].requests.len(),
                1,
                "child {nth} spoke its segment"
            );
            let closed = side.opcodes == [TEXT, CLOSE];
            if held < CLOSE_GRACE {
                returned += 1;
                if !closed {
                    lost.push((nth, held, side));
                }
            } else {
                handed_off += 1;
                arrived += usize::from(closed);
            }
        }
        println!(
            "{TEST}: {returned} of {CHILDREN} drops returned within {CLOSE_GRACE:?}; \
             {handed_off} handed their close off, and the close of {arrived} of those arrived"
        );
        assert!(
            lost.is_empty(),
            "the server received no close frame of {} of {returned} handles whose drop returned \
             within {CLOSE_GRACE:?}; the child, its drop's hold and what the relay read of each: \
             {lost:?}",
            lost.len()
        );
        assert!(
            returned >= 1,
            "no drop of {CHILDREN} returned within {CLOSE_GRACE:?}: all handed their close off"
        );
    });
}
