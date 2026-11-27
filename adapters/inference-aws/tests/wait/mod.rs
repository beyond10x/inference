//! The waits of the offline Nova Sonic tests for the events of a session: [`until`] the event a
//! test asserts, [`ended`] for the end of the session, and [`drain`] for the quiet after the last
//! event, which only a test that asserts that nothing more comes uses.
//!
//! On a busy host a gap between two events of a replay or a scripted provider can pass one
//! [`WAIT`]: a wait for a quiet time then ends before the events after the gap, and a test that
//! asserted one of them failed (two tests of `nova_sonic_duplex.rs` on a host whose load average
//! was 95 to 122 on 20 processors). The waits for an event let such a gap pass.

use std::time::{Duration, Instant};

use inference_duplex::{SessionEvent, SpeechSessionHandle};

/// How long one wait for the next event lasts.
pub const WAIT: Duration = Duration::from_millis(100);

/// How long [`until`] waits for its event in all, however often events that are not the one
/// come: a third of the runner's 60 s, so the wait fails with its own message first.
pub const UNTIL_LIMIT: Duration = Duration::from_secs(20);

/// Every event until nothing arrives for one [`WAIT`]. This waits for a time: on a busy host a
/// gap between two events passes the wait, and the events after it are left out. Only a test
/// that asserts that nothing more comes uses it, after it waited for the events it needs.
pub fn drain(session: &mut impl SpeechSessionHandle) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    while let Some(event) = session.next_event(WAIT) {
        events.push(event);
        assert!(events.len() < 500, "the session never went quiet");
    }
    events
}

/// Every event up to and including the first `stop` holds for, within [`UNTIL_LIMIT`].
pub fn until(
    session: &mut impl SpeechSessionHandle,
    stop: impl Fn(&SessionEvent) -> bool,
) -> Vec<SessionEvent> {
    until_within(session, stop, UNTIL_LIMIT)
}

/// Every event up to and including the first `stop` holds for. It fails with its own message at
/// either of two ends: once 30 waits in a row bring nothing, 3 s without an event, and once
/// `limit` has passed since it began. Waits that came up empty between events do not count
/// towards the first: on a busy host a gap between two events of the replay can pass one wait.
pub fn until_within(
    session: &mut impl SpeechSessionHandle,
    stop: impl Fn(&SessionEvent) -> bool,
    limit: Duration,
) -> Vec<SessionEvent> {
    let started = Instant::now();
    let mut events = Vec::new();
    let mut idle = 0;
    loop {
        match session.next_event(WAIT) {
            Some(event) => {
                idle = 0;
                let done = stop(&event);
                events.push(event);
                if done {
                    return events;
                }
                assert!(
                    events.len() < 500,
                    "the awaited event never came: {events:#?}"
                );
            }
            None => {
                idle += 1;
                assert!(
                    idle < 30,
                    "the awaited event never came, and no event for 30 waits of {WAIT:?}: \
                     {events:#?}"
                );
            }
        }
        assert!(
            started.elapsed() < limit,
            "the awaited event did not come within {limit:?}: {events:#?}"
        );
    }
}

/// Every event up to and including the session's last, `SpeechSessionClosed` or
/// `SpeechSessionFailed`.
pub fn ended(session: &mut impl SpeechSessionHandle) -> Vec<SessionEvent> {
    until(session, |event| {
        matches!(
            event,
            SessionEvent::SpeechSessionClosed(_) | SessionEvent::SpeechSessionFailed(_)
        )
    })
}

/// Every event up to and including the first `ResponseCompleted`.
pub fn completed(session: &mut impl SpeechSessionHandle) -> Vec<SessionEvent> {
    until(session, |event| {
        matches!(event, SessionEvent::ResponseCompleted(_))
    })
}

/// Returns once the provider has sent `count` frames, as `sent` counts them, and takes no event
/// of the session: for a test whose caller leaves what arrived unread. Fails at [`UNTIL_LIMIT`].
pub fn until_sent(sent: impl Fn() -> usize, count: usize) {
    let started = Instant::now();
    while sent() < count {
        assert!(
            started.elapsed() < UNTIL_LIMIT,
            "the provider sent {} of {count} frames within {UNTIL_LIMIT:?}",
            sent()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Every event until the provider has sent `count` frames, as `sent` counts them, and then until
/// nothing arrives for one [`WAIT`], which takes the last of them in: for a frame that brings no
/// event of its own, such as usage no response holds yet or a frame the binding cannot read.
/// The quiet wait comes after the provider sent the frame, so no gap of the provider passes it.
/// Fails at [`UNTIL_LIMIT`].
pub fn after_sent(
    session: &mut impl SpeechSessionHandle,
    sent: impl Fn() -> usize,
    count: usize,
) -> Vec<SessionEvent> {
    let started = Instant::now();
    let mut events = Vec::new();
    while sent() < count {
        events.extend(session.next_event(WAIT));
        assert!(
            started.elapsed() < UNTIL_LIMIT,
            "the provider sent {} of {count} frames within {UNTIL_LIMIT:?}: {events:#?}",
            sent()
        );
    }
    events.extend(drain(session));
    events
}
