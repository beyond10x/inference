//! Adversary, story:synthesiser-drop-inside-runtime: the drop of an idle synthesis where a
//! runtime is current in other ways than the unit's test has it, in a process that goes on
//! living, and many such drops in a row.
//!
//! `src/lib.rs` says of a drop inside an async runtime that its close has been sent when the
//! drop returns, or else runs to its end on the handle's own thread. In a process that goes on
//! living the server so receives `close_socket` and the close frame either way. Each case
//! asserts that through a relay to the fixture server, or for every connection of forty, and
//! the last also that no thread of a synthesis and no descriptor is left.

mod support;

#[allow(dead_code)]
#[path = "support/dropped.rs"]
mod dropped;

use std::sync::Arc;
use std::time::{Duration, Instant};

use dropped::{CLOSE, Relay, TEXT, threads_named};
use inference_elevenlabs::{
    CLOSE_BOUND, ElevenLabsSynthesis, ElevenLabsSynthesiser, Endpoint, OutputFormat, VoiceConfig,
};
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use support::{FixtureServer, Script};

/// The kernel's name of a synthesis's own thread: the first 15 bytes of `inference-elevenlabs`.
const OWN_THREAD: &str = "inference-eleve";

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

/// A synthesis at `url` that spoke one segment through its async form and is idle, its
/// utterance open and its connection with it.
async fn idle(url: &str) -> (ElevenLabsSynthesiser, ElevenLabsSynthesis) {
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
    (synthesiser, handle)
}

/// A synthesis at `url` that spoke one segment through its iterator and is idle.
fn idle_iterated(url: &str) -> (ElevenLabsSynthesiser, ElevenLabsSynthesis) {
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
    (synthesiser, handle)
}

/// The server received `close_socket` and then a close frame on its one connection, and the
/// synthesis's own thread ended.
fn closed_in_order(what: &str, server: &FixtureServer, relay: &Relay) {
    let side = relay
        .next_client_side(Duration::from_secs(5))
        .unwrap_or_else(|| panic!("{what}: the client's side of the connection never ended"));
    assert_eq!(
        side.opcodes.last_chunk(),
        Some(&[TEXT, CLOSE]),
        "{what}: a text message, then a close frame, end what the client sent: {side:?}"
    );
    // Not asserted: whether the socket then ended in order, with the client's FIN. The fixture
    // server answers `close_socket` with a close frame of its own; when that frame reaches the
    // client before the client lets go of its socket, the socket ends with a reset and the
    // relay reads an error. This case met that once in fifty uses over ten runs of the suite,
    // after the opcodes `[1, 1, 1, 1, 8]` and nothing unframed.
    assert_eq!(side.unframed, 0, "{what}: {side:?}");
    assert_eq!(server.next_client_close(Duration::from_secs(5)), Some(0));
    assert!(
        server.seen()[0].close_socket,
        "{what}: the text message is `close_socket`"
    );
    assert_eq!(server.violations(), Vec::<String>::new(), "{what}");
    let left = threads_named(OWN_THREAD, 0, Duration::from_secs(5));
    assert!(
        matches!(left, None | Some(0)),
        "{what}: the synthesis's own thread ends after its close: {left:?}"
    );
}

/// A task that holds an idle synthesis is dropped with its runtime: the server still receives
/// `close_socket` and the close frame, on a current-thread and on a multi-thread runtime.
#[test]
fn a_drop_while_the_outer_runtime_shuts_down_still_closes_the_connection() {
    for multi in [false, true] {
        let what = if multi {
            "a multi-thread runtime dropped"
        } else {
            "a current-thread runtime dropped"
        };
        let server = FixtureServer::start(OutputFormat::Ulaw8000, Script::Serve);
        let relay = Relay::to(&server.url);
        let runtime = if multi {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
        } else {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
        }
        .expect("a runtime");
        let (is_idle, idle_seen) = tokio::sync::oneshot::channel();
        let url = relay.url.clone();
        runtime.spawn(async move {
            let _held = idle(&url).await;
            let _ = is_idle.send(());
            std::future::pending::<()>().await;
        });
        runtime
            .block_on(idle_seen)
            .expect("the session reaches its idle handle");
        let started = Instant::now();
        drop(runtime);
        println!(
            "{what}: the drop of the runtime took {:?}",
            started.elapsed()
        );
        closed_in_order(what, &server, &relay);
    }
}

/// A task that panics while it holds an idle synthesis: the handle is dropped while the panic
/// unwinds inside the runtime, and the server still receives `close_socket` and the close frame.
#[test]
fn a_drop_while_a_panic_unwinds_inside_a_runtime_still_closes_the_connection() {
    let server = FixtureServer::start(OutputFormat::Ulaw8000, Script::Serve);
    let relay = Relay::to(&server.url);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime");
    let url = relay.url.clone();
    let joined = runtime.block_on(async move {
        tokio::spawn(async move {
            let _held = idle(&url).await;
            panic!("the session panics with its handle idle");
        })
        .await
    });
    assert!(
        joined.as_ref().is_err_and(tokio::task::JoinError::is_panic),
        "the session panicked: {joined:?}"
    );
    closed_in_order("a panic unwinding", &server, &relay);
}

/// A handle iterated and dropped in `spawn_blocking`, and one in `block_in_place`, where a
/// runtime is current and the thread is the caller's to block: the server still receives
/// `close_socket` and the close frame.
#[test]
fn a_drop_on_a_blocking_thread_of_a_runtime_still_closes_the_connection() {
    for in_place in [false, true] {
        let what = if in_place {
            "block_in_place"
        } else {
            "spawn_blocking"
        };
        let server = FixtureServer::start(OutputFormat::Ulaw8000, Script::Serve);
        let relay = Relay::to(&server.url);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("a multi-thread runtime");
        let url = relay.url.clone();
        let speak_and_drop = move || {
            let (_synthesiser, handle) = idle_iterated(&url);
            let started = Instant::now();
            drop(handle);
            started.elapsed()
        };
        let held = runtime.block_on(async move {
            let joined = if in_place {
                tokio::spawn(async move { tokio::task::block_in_place(speak_and_drop) }).await
            } else {
                tokio::task::spawn_blocking(speak_and_drop).await
            };
            joined.expect("the session ends without a panic")
        });
        println!("{what}: the drop held its thread {held:?}");
        closed_in_order(what, &server, &relay);
    }
}

/// How many descriptors this process holds.
fn descriptors() -> usize {
    std::fs::read_dir("/proc/self/fd").map_or(0, Iterator::count)
}

/// Forty syntheses, one after another on a current-thread runtime, each dropped idle with its
/// connection open: the server receives `close_socket` on every connection, and once
/// `CLOSE_BOUND` has passed no thread of a synthesis and no descriptor is left. One synthesis
/// against the fixture server takes about 53 ms, so five hundred took 26.5 s when the case was
/// written and passed the same way; forty keep the case near 2 s.
#[test]
fn forty_drops_inside_a_runtime_leave_no_thread_and_no_descriptor() {
    const DROPS: usize = 40;
    let started = Instant::now();
    let server = FixtureServer::start(OutputFormat::Ulaw8000, Script::Serve);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime");
    let url = server.url.clone();
    let (before, most) = runtime.block_on(async {
        // One synthesis first, over before the descriptors are counted: the server's runtime
        // and this one then hold every descriptor they keep.
        let (_synthesiser, first) = idle(&url).await;
        drop(first);
        assert!(
            server.next_client_close(Duration::from_secs(5)).is_some(),
            "the server saw the first connection end"
        );
        let own = threads_named(OWN_THREAD, 0, Duration::from_secs(5));
        assert!(
            matches!(own, None | Some(0)),
            "the first thread ends: {own:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        let before = descriptors();
        let mut most = 0;
        for _ in 0..DROPS {
            let (_synthesiser, handle) = idle(&url).await;
            drop(handle);
            most = most.max(threads_named(OWN_THREAD, usize::MAX, Duration::ZERO).unwrap_or(0));
        }
        (before, most)
    });
    let spoke = started.elapsed();
    for nth in 0..DROPS {
        assert!(
            server.next_client_close(Duration::from_secs(5)).is_some(),
            "the server saw only {nth} of {DROPS} connections end"
        );
    }
    let left = threads_named(OWN_THREAD, 0, CLOSE_BOUND + Duration::from_secs(3));
    let until = Instant::now() + CLOSE_BOUND + Duration::from_secs(3);
    let mut after = descriptors();
    while after > before && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
        after = descriptors();
    }
    let seen = server.seen();
    let closed = seen.iter().filter(|seen| seen.close_socket).count();
    println!(
        "{DROPS} drops in {spoke:?}, the case took {:?}: at most {most} own thread(s) at once, \
         {left:?} left; {before} descriptor(s) before, {after} after; `close_socket` on {closed} \
         of {} connection(s)",
        started.elapsed(),
        seen.len()
    );
    assert_eq!(seen.len(), DROPS + 1, "one connection per synthesis");
    assert_eq!(closed, DROPS + 1, "`close_socket` on every connection");
    assert_eq!(server.violations(), Vec::<String>::new());
    assert!(
        matches!(left, None | Some(0)),
        "threads of dropped syntheses are left: {left:?}"
    );
    assert!(
        after <= before,
        "descriptors are left: {before} before, {after} after"
    );
}
