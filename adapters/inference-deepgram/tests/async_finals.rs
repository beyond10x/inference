//! The recognition's async form after the end of input: the server holds the final result and the
//! summary back, so the async form returns `Pending`, polled a bounded handful of times, and is
//! woken for the finals when they arrive, with the usage record last.

mod support;

use std::time::Duration;

use inference_stt::{Recogniser, RecognitionHandle};
use support::{Action, FixtureServer, Script, Step, Trigger, describe, request, tone};

/// How long the server holds the finals back after `CloseStream`.
const HELD: Duration = Duration::from_millis(300);

/// The most polls one await takes while the finals are held back; a form that wakes itself on
/// every poll takes thousands.
const MOST_POLLS: u32 = 10;

/// Nothing before the end of input; after `CloseStream`, [`HELD`], then a final result, the
/// summary and the close.
fn held_finals() -> Script {
    let on_close = |action| Step {
        on: Trigger::CloseStream,
        action,
    };
    Script::Steps(vec![
        on_close(Action::Wait(HELD)),
        on_close(Action::Send(support::results("hallo", true, true))),
        on_close(Action::Send(
            serde_json::json!({
                "type": "Metadata",
                "request_id": "fixture-request",
                "sha256": "fixture",
                "created": "2026-11-05T00:00:00.000Z",
                "duration": 0.16,
                "channels": 1
            })
            .to_string(),
        )),
        on_close(Action::Close(1000, String::new())),
    ])
}

#[test]
fn after_the_end_of_input_the_async_form_waits_and_is_woken_for_the_finals() {
    let server = FixtureServer::start(held_finals());
    let mut handle = server
        .recogniser()
        .start(request())
        .expect("a slot is free");
    for frame in tone(8) {
        handle.offer_frame(frame).expect("input is open");
    }
    handle.end_input().expect("input ends once");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("a current-thread runtime");
    let (polls, events) = runtime.block_on(async {
        let mut polls = 0_u32;
        let first = tokio::time::timeout(
            Duration::from_secs(5),
            std::future::poll_fn(|cx| {
                polls += 1;
                handle.poll_event(cx)
            }),
        )
        .await
        .expect("the finals arrive within five seconds");
        let mut events: Vec<String> = first.iter().map(describe).collect();
        while let Some(event) = handle.event().await {
            events.push(describe(&event));
        }
        (polls, events)
    });
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
    assert!(
        polls <= MOST_POLLS,
        "one await polled the recognition {polls} times while the finals were held back"
    );
    assert_eq!(events, ["hypothesis hallo", "commit", "usage"]);
    assert_eq!(server.violations(), Vec::<String>::new());
}
