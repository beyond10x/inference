//! The usage of a turn cut short in a session's total (`story:llm-usage-on-cancel`).
//!
//! A session over the text pipeline, its model node's `usage` port exposed as an egress, answers
//! two inputs on the mock LLM, whose second turn streams its text and then holds as a call cut
//! short in the middle of its output does. The host supersedes the node's generation during that
//! turn. The session delivers two usage records, the second not final, and its usage view counts
//! both, the second as partial: neither the run's total nor the session's reads as complete.

#![allow(dead_code)]

mod text;

use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use inference_core::usage::{QuantityKind, UsageRecorded, UsageTotal};
use inference_facade::events::SessionEvent;
use inference_facade::live::{LiveSession, LiveSessions};
use inference_llm::llm_core::Usage;
use inference_pipeline::InteractionMode;
use inference_testkit::graph::{PipelinePayload, RunLog};
use inference_testkit::llm::{ModelHandle, ModelNode};

use text::{Application, block_on, generator, planned_with_usage, reply};

/// How long the case waits for the session to end once it closed it.
const PATIENCE: Duration = Duration::from_secs(20);

fn records(events: &[SessionEvent]) -> Vec<&UsageRecorded> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::UsageRecorded(recorded) => Some(recorded),
            _ => None,
        })
        .collect()
}

fn count(events: &[SessionEvent], matches: fn(&SessionEvent) -> bool) -> usize {
    events.iter().filter(|event| matches(event)).count()
}

/// Sends `text` and commits it, then receives until `segments` segments were committed and
/// `usage` usage records delivered in all.
fn turn(
    session: &mut LiveSession<PipelinePayload>,
    application: &mut Application,
    text: &str,
    (segments, usage): (usize, usize),
    received: &mut Vec<SessionEvent>,
) {
    block_on(session.send(application.text(text))).expect("sent");
    block_on(session.commit_input()).expect("committed");
    while count(received, |event| {
        matches!(event, SessionEvent::TextSegmentCommitted(_))
    }) < segments
        || count(received, |event| {
            matches!(event, SessionEvent::UsageRecorded(_))
        }) < usage
    {
        received.push(block_on(session.receive()).expect("the reply streams"));
    }
}

#[test]
fn a_generation_superseded_during_its_second_turn_counts_both_records_the_second_as_partial() {
    let first = Usage {
        input_tokens: Some(12),
        output_tokens: Some(3),
        ..Usage::default()
    };
    let cut = Usage {
        input_tokens: Some(20),
        output_tokens: Some(1),
        ..Usage::default()
    };
    let generator = generator(Vec::<String>::new())
        .reporting([Some(first), Some(cut)])
        .holding(1);
    let (declaration, planned, usage) = planned_with_usage(&generator);
    let handles: Arc<Mutex<Vec<ModelHandle>>> = Arc::default();
    let nodes = {
        let generator = generator.clone();
        let handles = Arc::clone(&handles);
        move |run_id, _: &inference_pipeline::Node| {
            let node =
                ModelNode::new(run_id, generator.clone(), reply, RunLog::new()).with_usage(usage);
            handles
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(node.handle());
            Box::new(node) as Box<dyn inference_pipeline::NodeImplementation<PipelinePayload>>
        }
    };
    let sessions = LiveSessions::new();
    let (mut session, driver) =
        block_on(sessions.open(&declaration, &planned, InteractionMode::TurnBased, nodes))
            .expect("opens");
    let run_id = session.run_id();
    let drive = thread::spawn(move || block_on(driver.drive()));
    let mut application = Application::new(run_id);
    let mut received = Vec::new();
    turn(&mut session, &mut application, "1", (1, 1), &mut received);
    turn(&mut session, &mut application, "2", (2, 1), &mut received);

    let node = handles
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .first()
        .cloned()
        .expect("the run made its model node");
    assert!(node.supersede(), "the node's generation is superseded");
    block_on(session.close()).expect("drains");
    let (ended, ending) = mpsc::channel();
    thread::spawn(move || {
        while let Some(event) = block_on(session.receive()) {
            received.push(event);
        }
        let _ = ended.send((session, received));
    });
    let (session, received) = ending
        .recv_timeout(PATIENCE)
        .expect("the session ends once the cut turn's usage is delivered");
    drive.join().expect("no panic").expect("the run ends");

    let delivered = records(&received);
    let [ended, partial] = delivered.as_slice() else {
        panic!("two usage records, one per turn: {received:#?}")
    };
    assert!(ended.r#final, "the first turn ended");
    assert!(!partial.r#final, "the superseded turn did not end");
    assert_eq!(partial.run_id, Some(run_id));

    let usage = session.usage();
    let mut expected = UsageTotal::new();
    for recorded in &delivered {
        expected.add(&recorded.quantities, recorded.r#final);
    }
    assert_eq!(usage.session, expected);
    assert_eq!(usage.run(run_id), Some(&usage.session));
    assert_eq!(usage.session.records(), 2);
    let input = usage
        .session
        .kind(QuantityKind::InputTextTokens)
        .expect("both records carry input tokens");
    assert_eq!(
        (input.known, input.unknown, input.records, input.partial),
        (32, 0, 2, 1),
        "both counted, the second as partial"
    );
    let requests = usage
        .session
        .kind(QuantityKind::Requests)
        .expect("both records carry a request");
    assert_eq!((requests.known, requests.partial), (2, 1));
    assert!(
        !usage.session.complete(),
        "a total over a partial record is not complete"
    );
}
