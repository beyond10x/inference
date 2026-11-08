//! Adversarial pass 1 of story:sdk-agent against `Drop` of an unclosed `Agent` (R115): an `Events`
//! a reader took before the drop is the stream `Event::Closed` promises to end ("The session
//! closed; the last event of every stream"; `Events`: "Every stream ends after `Event::Closed`").

#![cfg(feature = "testing")]

use std::time::Duration;

use futures_util::StreamExt;
use inference::{Agent, Binding, Event, Stack};
use inference_testkit::duplex::mock::{MockSpeechModel, Reply, ScriptedTurn};

const LIMIT: Duration = Duration::from_secs(5);

fn turn(text: &str) -> ScriptedTurn {
    ScriptedTurn {
        heard: "hello".to_owned(),
        reply: Reply::Say(text.to_owned()),
    }
}

/// Reads `events` until it ends; `None` when it does not end within the limit.
async fn read_all(events: &mut inference::Events) -> Option<Vec<Event>> {
    tokio::time::timeout(LIMIT, async {
        let mut seen = Vec::new();
        while let Some(event) = events.next().await {
            seen.push(event.expect("no terminal error"));
        }
        seen
    })
    .await
    .ok()
}

/// The stream a reader holds ends once the Agent is dropped: the pump's and every other task's
/// `Arc<Inner>` is gone, so the `Sender` it holds is dropped (the observable side of R115's
/// `Weak` that no longer upgrades).
#[tokio::test(flavor = "multi_thread")]
async fn adversary1_a_dropped_agents_stream_ends() {
    let model = MockSpeechModel::native(vec![turn("hi")]);
    let agent = Agent::voice_with(Stack::new().bind(Binding::speech(model)))
        .start()
        .await
        .expect("starts");
    let mut events = agent.events();
    drop(agent);
    let seen = read_all(&mut events).await;
    assert!(seen.is_some(), "the stream did not end within 5 s");
}

/// The stream a reader holds across the drop ends with `Closed`: the session was closed (cancel),
/// and `Closed` is documented as the last event of every stream.
#[tokio::test(flavor = "multi_thread")]
async fn adversary1_a_dropped_agents_stream_ends_with_closed() {
    let model = MockSpeechModel::native(vec![turn("hi")]);
    let agent = Agent::voice_with(Stack::new().bind(Binding::speech(model)))
        .start()
        .await
        .expect("starts");
    let mut events = agent.events();
    drop(agent);
    let seen = read_all(&mut events).await.expect("the stream ends");
    assert!(
        matches!(seen.last(), Some(Event::Closed { .. })),
        "the stream ended without Closed: {seen:#?}"
    );
}
