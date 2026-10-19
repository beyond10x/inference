//! Adversary: a live session whose application lets go of its handle while Ready, and the order
//! of the history when the reply is received before the input is committed.

#![allow(dead_code)]

mod text;

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use inference_core::model::Capability;
use inference_facade::events::SessionEvent;
use inference_facade::history::HistoryKind;
use inference_facade::live::LiveSessions;
use inference_facade::session::SessionState;
use inference_pipeline::InteractionMode;

use text::{Application, block_on, generator, implement, planned};

/// "An application that lets go of its handle lets go of what was in flight, and the session
/// closes once the run has completed" (`live.rs` module documentation).
#[test]
fn a_ready_session_whose_handle_is_dropped_closes_once_its_run_has_ended() {
    let generator = generator(Vec::<String>::new());
    let (declaration, planned) = planned(&generator, &[Capability::OutputStreaming]);
    let sessions = LiveSessions::new();
    let (session, driver) = block_on(sessions.open(
        &declaration,
        &planned,
        InteractionMode::TurnBased,
        implement(&generator),
    ))
    .expect("opens");
    let session_id = session.session_id();
    let (ended, wait) = mpsc::channel();
    thread::spawn(move || {
        let _ = ended.send(block_on(driver.drive()).map(|run| run.state()));
    });
    drop(session);
    let run = wait.recv_timeout(Duration::from_secs(5));
    let state = sessions.session(session_id).map(|s| s.state());
    assert!(
        run.is_ok() && state == Some(SessionState::Closed),
        "after the handle was dropped: run {run:?}, session {state:?}"
    );
}

/// The user turn is recorded at the commit; a reply the application received before it commits
/// lands in the history before the turn it answers.
#[test]
fn the_user_turn_precedes_the_reply_that_answers_it_when_the_reply_is_received_before_the_commit() {
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
    loop {
        let event = block_on(session.receive()).expect("the reply streams");
        if matches!(event, SessionEvent::TextSegmentCommitted(_)) {
            break;
        }
    }
    block_on(session.commit_input()).expect("committed");
    block_on(session.close()).expect("drains");
    while block_on(session.receive()).is_some() {}
    drive.join().expect("no panic").expect("the run ends");
    let history: Vec<_> = sessions
        .history(session_id)
        .into_iter()
        .map(|entry| (entry.position(), entry.kind(), entry.text().to_owned()))
        .collect();
    assert_eq!(
        history,
        [
            (1, HistoryKind::UserTurn, "1".to_owned()),
            (2, HistoryKind::AssistantSegment, "s1".to_owned()),
        ]
    );
}
