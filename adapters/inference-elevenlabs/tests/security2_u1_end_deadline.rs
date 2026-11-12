//! Conformance checks of "never a panic or a hang" for the wait after the end of input: a server
//! that keeps sending messages which deliver nothing to the caller (a `warning`, or the same
//! partial transcript again) and never answers the end-of-input commit must not keep a single
//! `next` call blocked for longer than a bound derived from the read timeout. The Deepgram
//! recogniser holds this with one deadline from the end of input that messages never move.
#![cfg(feature = "stt")]

mod stt_support;

use std::sync::mpsc;
use std::time::{Duration, Instant};

use inference_elevenlabs::{ElevenLabsError, InputFormat};
use inference_stt::{Recogniser, RecognitionHandle};
use stt_support::{
    Action, FixtureServer, Script, Step, Trigger, manual, mulaw, partial, request, session_started,
};

const READ_TIMEOUT: Duration = Duration::from_millis(300);

/// Ten read timeouts: generous for any bound the read timeout is meant to set.
const CALL_BOUND: Duration = Duration::from_secs(3);

/// How long the server keeps sending after the end-of-input commit: far past `CALL_BOUND`.
const STREAM_FOR: Duration = Duration::from_secs(12);

const STREAM_EVERY: Duration = Duration::from_millis(100);

/// `session_started`, the partial "hallo" after the first chunk, and after the end-of-input
/// commit `message` every `STREAM_EVERY` for `STREAM_FOR`, the commit never answered.
fn streams_after_commit(message: &str) -> Script {
    let mut steps = vec![
        Step {
            on: Trigger::Open,
            action: Action::Send(session_started()),
        },
        Step {
            on: Trigger::Audio(1),
            action: Action::Send(partial("hallo")),
        },
    ];
    let count = STREAM_FOR.as_millis() / STREAM_EVERY.as_millis();
    for _ in 0..count {
        steps.push(Step {
            on: Trigger::Commit(1),
            action: Action::Send(message.to_owned()),
        });
        steps.push(Step {
            on: Trigger::Commit(1),
            action: Action::Wait(STREAM_EVERY),
        });
    }
    Script::Steps(steps)
}

/// Runs one recognition against `script` on its own thread and returns the longest single `next`
/// call after the end of input and the failure, or `None` when one call is still blocked after
/// `CALL_BOUND`.
fn longest_call_after_end(script: Script) -> Option<(Duration, Option<ElevenLabsError>)> {
    let server = FixtureServer::start(script);
    let endpoint = server.endpoint().with_read_timeout(READ_TIMEOUT);
    let (progress, calls) = mpsc::channel::<Result<Duration, Option<ElevenLabsError>>>();
    std::thread::spawn(move || {
        let mut recogniser = inference_elevenlabs::ElevenLabsRecogniser::new(
            endpoint,
            manual(InputFormat::Ulaw8000),
            std::sync::Arc::new(stt_support::FixedKey(stt_support::KEY)),
            "elevenlabs-api-key",
        )
        .unwrap();
        let mut handle = recogniser.start(request()).unwrap();
        for frame in mulaw(3) {
            handle.offer_frame(frame).unwrap();
            handle.by_ref().for_each(drop);
        }
        let opened = Instant::now() + Duration::from_secs(3);
        while handle.output().spans().is_empty() && Instant::now() < opened {
            handle.by_ref().for_each(drop);
            std::thread::sleep(Duration::from_millis(10));
        }
        handle.end_input().unwrap();
        loop {
            let called = Instant::now();
            let next = handle.next();
            if progress.send(Ok(called.elapsed())).is_err() || next.is_none() {
                break;
            }
        }
        let _ = progress.send(Err(handle.failure().cloned()));
    });
    let mut longest = Duration::ZERO;
    loop {
        match calls.recv_timeout(CALL_BOUND) {
            Ok(Ok(call)) => longest = longest.max(call),
            Ok(Err(failure)) => {
                drop(server);
                return Some((longest, failure));
            }
            Err(_) => return None,
        }
    }
}

fn assert_bounded(script: Script, what: &str) {
    match longest_call_after_end(script) {
        None => panic!(
            "a single next() after the end of input was still blocked after {CALL_BOUND:?} (read \
             timeout {READ_TIMEOUT:?}) while the server sent only {what} every {STREAM_EVERY:?} \
             and never answered the end-of-input commit"
        ),
        Some((longest, failure)) => {
            assert!(longest <= CALL_BOUND, "{longest:?}");
            assert!(
                matches!(failure, Some(ElevenLabsError::Stalled(_))),
                "the commit was never answered, yet failure {failure:?}"
            );
        }
    }
}

#[test]
fn warnings_streamed_after_the_end_of_input_do_not_block_next_past_the_bound() {
    let warning = serde_json::json!({"message_type": "warning", "warning": "slow"}).to_string();
    assert_bounded(streams_after_commit(&warning), "`warning` messages");
}

#[test]
fn the_same_partial_streamed_after_the_end_of_input_does_not_block_next_past_the_bound() {
    assert_bounded(
        streams_after_commit(&partial("hallo")),
        "the partial transcript it had already sent",
    );
}
