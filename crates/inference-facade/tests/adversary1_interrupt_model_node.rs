//! Adversary: an interrupt over the text pipeline the facade's own tests run, the testkit's
//! `ModelNode`, which names a generation on the envelopes of its deltas and not on those of its
//! committed segments (a `TextSegmentCommitted` names no generation).

#![allow(dead_code)]

mod text;

use std::thread;

use inference_core::model::Capability;
use inference_facade::events::SessionEvent;
use inference_facade::history::HistoryKind;
use inference_facade::live::LiveSessions;
use inference_pipeline::InteractionMode;

use text::{Application, block_on, generator, implement, planned};

/// The cut generation's committed segment is neither delivered after the interrupt nor recorded
/// in the history as an assistant segment.
#[test]
fn after_an_interrupt_the_cut_generations_committed_segment_is_not_delivered_nor_recorded() {
    let generator = generator(Vec::<String>::new());
    let (declaration, planned) = planned(&generator, &[Capability::OutputStreaming]);
    let sessions = LiveSessions::new();
    let (mut session, driver) = block_on(sessions.open(
        &declaration,
        &planned,
        InteractionMode::TurnBased,
        implement(&generator),
    ))
    .expect("opens");
    let session_id = session.session_id();
    let drive = thread::spawn(move || block_on(driver.drive()));
    let mut application = Application::new(session.run_id());
    block_on(session.send(application.text("1"))).expect("sent");
    block_on(session.commit_input()).expect("committed");
    loop {
        let event = block_on(session.receive()).expect("the reply streams");
        if matches!(event, SessionEvent::TextDelta(_)) {
            break;
        }
    }

    block_on(session.interrupt()).expect("a Ready session is interrupted");
    block_on(session.close()).expect("drains");
    let mut after = Vec::new();
    while let Some(event) = block_on(session.receive()) {
        after.push(event);
    }
    drive.join().expect("no panic").expect("the run ends");

    let stale: Vec<_> = after
        .iter()
        .filter(|event| matches!(event, SessionEvent::TextSegmentCommitted(_)))
        .collect();
    assert!(
        stale.is_empty(),
        "the interrupted generation's segment is delivered after the interrupt: {stale:#?}"
    );
    let assistant: Vec<_> = sessions
        .history(session_id)
        .into_iter()
        .filter(|entry| entry.kind() == HistoryKind::AssistantSegment)
        .map(|entry| entry.text().to_owned())
        .collect();
    assert!(
        assistant.is_empty(),
        "the interrupted generation's segment joined the history: {assistant:?}"
    );
}
