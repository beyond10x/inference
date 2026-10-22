//! Closing a live session by draining delivers what its run produced before the close, the
//! usage record of a turn included, before `SessionClosed`, which is the last event.

#![allow(dead_code)]

mod text;

use std::thread;
use std::time::Duration;

use inference_core::id::RunId;
use inference_facade::events::SessionEvent;
use inference_facade::live::LiveSessions;
use inference_pipeline::{InteractionMode, Node, NodeImplementation, PortId};
use inference_testkit::graph::{PipelinePayload, RunLog};
use inference_testkit::llm::{MockGenerator, ModelNode};

use text::{Application, block_on, generator, planned_with_usage, reply};

/// The text pipeline's model node given its usage port.
fn with_usage(
    generator: &MockGenerator,
    usage: PortId,
) -> impl FnMut(RunId, &Node) -> Box<dyn NodeImplementation<PipelinePayload>> + use<> {
    let generator = generator.clone();
    move |run_id, _| {
        Box::new(ModelNode::new(run_id, generator.clone(), reply, RunLog::new()).with_usage(usage))
    }
}

#[test]
fn what_the_run_produced_before_the_close_is_delivered_before_session_closed() {
    let generator = generator(Vec::<String>::new());
    let (declaration, planned, usage) = planned_with_usage(&generator);
    let sessions = LiveSessions::new();
    let (mut session, driver) = block_on(sessions.open(
        &declaration,
        &planned,
        InteractionMode::TurnBased,
        with_usage(&generator, usage),
    ))
    .expect("opens");
    let run_id = session.run_id();
    let drive = thread::spawn(move || block_on(driver.drive()));
    let mut application = Application::new(run_id);
    block_on(session.send(application.text("1"))).expect("sent");
    block_on(session.commit_input()).expect("committed");
    let mut before = Vec::new();
    loop {
        let event = block_on(session.receive()).expect("the reply streams");
        let committed = matches!(event, SessionEvent::TextSegmentCommitted(_));
        before.push(event);
        if committed {
            break;
        }
    }
    thread::sleep(Duration::from_millis(100));

    block_on(session.close()).expect("drains");
    let mut after = Vec::new();
    while let Some(event) = block_on(session.receive()) {
        after.push(event);
    }
    drive.join().expect("no panic").expect("the run ends");

    assert!(
        matches!(after.last(), Some(SessionEvent::SessionClosed(_))),
        "{after:#?}"
    );
    let records = before
        .iter()
        .chain(&after)
        .filter(|event| matches!(event, SessionEvent::UsageRecorded(_)))
        .count();
    assert_eq!(
        records, 1,
        "the turn's usage record is delivered once, before SessionClosed: {before:#?} {after:#?}"
    );
    assert_eq!(session.usage().session.records(), 1);
}
