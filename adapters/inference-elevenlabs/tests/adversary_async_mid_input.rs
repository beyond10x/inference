//! Adversary, story:async-speech-handles: the recogniser's async form while input is still
//! open. The story asks for an async form "woken by the adapter when an event arrives rather
//! than polled". Before the end of input the recognition's `poll_event` takes only what the
//! connection's thread has already queued and returns `Ready(None)` otherwise, registering no
//! waker, so a caller that awaits the next transcript between frames, as a phone agent does
//! while the caller speaks, gets `None` before the server's partial arrives and is never woken
//! for it: it has to poll on a timer.
//!
//! Decided: the contract stays. `RecognitionHandle`'s iterator defines `None` as nothing more
//! until more input is offered or input ends, and `poll_event`'s `Ready(None)` means the same:
//! drained for now. The control below pins that meaning.

#![cfg(feature = "stt")]

mod stt_support;

use std::time::Duration;

use inference_elevenlabs::InputFormat;
use inference_stt::{Recogniser, RecognitionHandle};
use stt_support::{Action, FixtureServer, Script, Step, Trigger, describe, manual, mulaw};

/// `session_started`, then a partial once the first chunk arrived.
fn partial_after_first() -> Script {
    Script::Steps(vec![
        Step {
            on: Trigger::Open,
            action: Action::Send(stt_support::session_started()),
        },
        Step {
            on: Trigger::Audio(1),
            action: Action::Send(stt_support::partial("hallo")),
        },
    ])
}

/// The control: the async form's first `None` is followed, with no more input offered and
/// input still open, by the partial, so that `None` did not mean "nothing until more input".
#[test]
fn control_the_partial_arrives_after_the_async_forms_none_without_more_input() {
    let server = FixtureServer::start(partial_after_first());
    let mut recogniser = server.recogniser(manual(InputFormat::Ulaw8000));
    let mut handle = recogniser
        .start(stt_support::request())
        .expect("a fresh recogniser runs nothing");
    for frame in mulaw(1) {
        handle.offer_frame(frame).expect("input is open");
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime");
    let (first, later) = runtime.block_on(async {
        let first = handle.event().await.as_ref().map(describe);
        let mut later = None;
        for _ in 0..200 {
            tokio::time::sleep(Duration::from_millis(10)).await;
            if let Some(event) = handle.event().await {
                later = Some(describe(&event));
                break;
            }
        }
        (first, later)
    });
    assert_eq!(first, None);
    assert_eq!(later.as_deref(), Some("hypothesis hallo"));
}
