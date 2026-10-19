//! A live session over the text-only topology A (`docs/design/inference.md` 749-757,
//! `story:session-live-text`): the A-02 model node between the exposed ingress `user text` and
//! the exposed egress `reply`, with no source or sink node. The test is the application: it opens
//! a session over the planned pipeline, drives the session's run on a thread of its own, sends text
//! and commits it, receives the mock model's reply as session events, and closes by draining.

#![allow(dead_code)]

mod text;

use std::thread;

use inference_core::id::RunId;
use inference_core::model::Capability;
use inference_facade::events::SessionEvent;
use inference_facade::live::{InputError, LiveSessions};
use inference_facade::session::{
    RunAttached, SessionClosed, SessionDraining, SessionMoveError, SessionOpened, SessionReady,
    SessionState, SessionStateConflict,
};
use inference_runtime::run::RunState;

use text::{Application, block_on, generator, implement, planned, sink_planned};

#[test]
fn a_session_over_the_text_pipeline_opens_sends_commits_receives_and_closes_by_draining() {
    let generator = generator(Vec::<String>::new());
    let (declaration, planned) = planned(&generator, &[Capability::OutputStreaming]);
    let sessions = LiveSessions::new();
    let (mut session, driver) = block_on(sessions.open(
        &declaration,
        &planned,
        inference_pipeline::InteractionMode::TurnBased,
        implement(&generator),
    ))
    .expect("a session opens over a planned pipeline of its mode");
    let session_id = session.session_id();
    let run_id: RunId = session.run_id();
    let exposure_queues = session.exposure_queues();
    assert_eq!(exposure_queues.len(), 2, "one queue per exposure");
    let drive = thread::spawn(move || block_on(driver.drive()));

    let first: Vec<SessionEvent> = (0..3)
        .map(|_| block_on(session.receive()).expect("an event"))
        .collect();
    assert_eq!(
        first,
        [
            SessionEvent::SessionOpened(SessionOpened {
                session_id,
                pipeline_id: planned.pipeline_id(),
            }),
            SessionEvent::RunAttached(RunAttached { session_id, run_id }),
            SessionEvent::SessionReady(SessionReady { session_id }),
        ],
        "the session's lifecycle comes before anything else"
    );
    assert_eq!(
        sessions.session(session_id).map(|s| s.state()),
        Some(SessionState::Ready)
    );

    let mut application = Application::new(run_id);
    block_on(session.send(application.text("1"))).expect("a Ready session takes input");
    block_on(session.commit_input()).expect("a Ready session takes a commit");

    let mut received = Vec::new();
    while !received
        .iter()
        .any(|event| matches!(event, SessionEvent::TextSegmentCommitted(_)))
    {
        received.push(block_on(session.receive()).expect("the reply arrives"));
    }
    assert!(
        received
            .iter()
            .any(|event| matches!(event, SessionEvent::GenerationStarted(_))),
        "the model's generation is delivered: {received:#?}"
    );
    assert!(
        received
            .iter()
            .any(|event| matches!(event, SessionEvent::TextDelta(delta) if delta.text == "s1")),
        "the reply's delta is delivered as a text event: {received:#?}"
    );
    assert!(
        received.iter().any(|event| matches!(
            event,
            SessionEvent::TextSegmentCommitted(segment) if segment.text == "s1"
        )),
        "the reply's committed segment is delivered as a text event: {received:#?}"
    );
    assert!(
        received
            .iter()
            .any(|event| matches!(event, SessionEvent::InputSent(_)))
            && received
                .iter()
                .any(|event| matches!(event, SessionEvent::InputCommitted(_))),
        "the session records what it admitted: {received:#?}"
    );

    block_on(session.close()).expect("a Ready session drains");
    match block_on(session.send(application.text("2"))) {
        Err(InputError::Refused(SessionMoveError::WrongState(SessionStateConflict {
            current: SessionState::Draining,
        }))) => {}
        other => panic!("a draining session refuses input, not {other:?}"),
    }

    let mut rest = Vec::new();
    while let Some(event) = block_on(session.receive()) {
        rest.push(event);
    }
    let draining = rest
        .iter()
        .position(|event| *event == SessionEvent::SessionDraining(SessionDraining { session_id }))
        .expect("the drain is delivered");
    assert_eq!(
        rest.last(),
        Some(&SessionEvent::SessionClosed(SessionClosed { session_id })),
        "the close is delivered last: {rest:#?}"
    );
    assert!(
        rest[draining..]
            .iter()
            .any(|event| matches!(event, SessionEvent::RunCompleted(completed) if completed.run_id == run_id)),
        "the run's completion comes between the drain and the close: {rest:#?}"
    );
    assert!(
        rest.iter()
            .any(|event| matches!(event, SessionEvent::GenerationCompleted(_))),
        "the drain completes the generation in flight: {rest:#?}"
    );

    let run = drive
        .join()
        .expect("the driver does not panic")
        .expect("the run ends");
    assert_eq!(run.state(), RunState::Completed);
    assert_eq!(
        sessions.session(session_id).map(|s| s.state()),
        Some(SessionState::Closed)
    );
    drop(session);
    for queue in &exposure_queues {
        assert!(
            queue.is_dropped(),
            "the queue of exposure {} is dropped",
            queue.exposure_id()
        );
    }
}

#[test]
fn a_session_refuses_input_named_for_an_ingress_the_pipeline_does_not_expose() {
    let generator = generator(Vec::<String>::new());
    let (declaration, planned) = planned(&generator, &[Capability::OutputStreaming]);
    let sessions = LiveSessions::new();
    let (mut session, driver) = block_on(sessions.open(
        &declaration,
        &planned,
        inference_pipeline::InteractionMode::TurnBased,
        implement(&generator),
    ))
    .expect("opens");
    let drive = thread::spawn(move || block_on(driver.drive()));
    let mut application = Application::new(session.run_id());
    let mut stray = application.text("1");
    stray.port = "elsewhere".to_owned();
    match block_on(session.send(stray)) {
        Err(InputError::UnknownIngress(name)) => assert_eq!(name, "elsewhere"),
        other => panic!("an unknown ingress is refused, not {other:?}"),
    }
    assert!(
        !sessions
            .events()
            .iter()
            .any(|event| matches!(event, SessionEvent::InputSent(_))),
        "nothing was admitted"
    );
    block_on(session.close()).expect("drains");
    while block_on(session.receive()).is_some() {}
    let run = drive.join().expect("no panic").expect("the run ends");
    assert_eq!(run.state(), RunState::Completed);
}

#[test]
fn a_session_over_a_pipeline_of_another_mode_is_not_opened() {
    let generator = generator(Vec::<String>::new());
    let (declaration, planned) = planned(&generator, &[Capability::OutputStreaming]);
    let sessions = LiveSessions::new();
    let opened = block_on(sessions.open(
        &declaration,
        &planned,
        inference_pipeline::InteractionMode::Duplex,
        implement(&generator),
    ));
    assert!(
        matches!(
            opened,
            Err(inference_facade::live::OpenError::Open(
                inference_facade::session::OpenSessionError::ModeMismatch(_)
            ))
        ),
        "the session's mode is the pipeline's"
    );
    assert!(sessions.sessions_view().is_empty(), "nothing was opened");
}

#[test]
fn the_session_forwards_input_stamped_with_itself_and_its_run_then_its_commit_and_its_stop() {
    use inference_core::envelope::ControlKind;
    use inference_testkit::graph::{PipelinePayload, TextSink};

    let generator = generator(Vec::<String>::new());
    let (declaration, planned) = sink_planned(&generator);
    let sink = TextSink::new();
    let recorded = sink.handle();
    let mut sink = Some(sink);
    let sessions = LiveSessions::new();
    let (mut session, driver) = block_on(sessions.open(
        &declaration,
        &planned,
        inference_pipeline::InteractionMode::TurnBased,
        |_, _| {
            Box::new(sink.take().expect("one node"))
                as Box<dyn inference_pipeline::NodeImplementation<PipelinePayload>>
        },
    ))
    .expect("opens");
    let session_id = session.session_id();
    let run_id = session.run_id();
    let drive = thread::spawn(move || block_on(driver.drive()));

    let mut application = Application::new(RunId::generate());
    let sent = application.text("1");
    let sent_id = sent.event_id;
    block_on(session.send(sent)).expect("sent");
    block_on(session.commit_input()).expect("committed");
    block_on(session.close()).expect("drains");
    while block_on(session.receive()).is_some() {}
    let run = drive.join().expect("no panic").expect("the run ends");
    assert_eq!(run.state(), RunState::Completed);

    let received = recorded.received();
    assert_eq!(
        received
            .iter()
            .map(|envelope| envelope.kind.as_str())
            .collect::<Vec<_>>(),
        [
            "inference.stream.TranscriptCommitted",
            ControlKind::Commit.as_str(),
            ControlKind::Stop.as_str(),
        ],
        "the input, the commit, then the end of input"
    );
    assert_eq!(received[0].event_id, sent_id, "the application's envelope");
    assert!(
        received
            .iter()
            .all(|envelope| envelope.session_id == Some(session_id) && envelope.run_id == run_id),
        "every envelope names the session and its current run, whatever run the application named"
    );
    assert_eq!(
        received[1].stream_id, received[2].stream_id,
        "the session's control events travel on a stream of its own"
    );
    assert_ne!(received[0].stream_id, received[1].stream_id);
    assert_eq!(
        (received[1].sequence, received[2].sequence),
        (1, 2),
        "numbered from 1 on that stream"
    );
}

#[test]
fn a_session_stays_draining_while_its_completed_run_has_output_in_flight_and_closes_once_let_go() {
    let generator = generator(Vec::<String>::new());
    let (declaration, planned) = planned(&generator, &[Capability::OutputStreaming]);
    let sessions = LiveSessions::new();
    let (mut session, driver) = block_on(sessions.open(
        &declaration,
        &planned,
        inference_pipeline::InteractionMode::TurnBased,
        implement(&generator),
    ))
    .expect("opens");
    let session_id = session.session_id();
    let exposure_queues = session.exposure_queues();
    let mut application = Application::new(session.run_id());
    block_on(session.send(application.text("1"))).expect("sent");
    block_on(session.close()).expect("drains");
    let run = block_on(driver.drive()).expect("the run ends");
    assert_eq!(run.state(), RunState::Completed);
    assert_eq!(
        sessions.session(session_id).map(|s| s.state()),
        Some(SessionState::Draining),
        "the reply is still in flight on the egress"
    );
    drop(session);
    assert_eq!(
        sessions.session(session_id).map(|s| s.state()),
        Some(SessionState::Closed),
        "an application that let go has nothing in flight"
    );
    assert!(
        sessions
            .events()
            .contains(&SessionEvent::SessionClosed(SessionClosed { session_id }))
    );
    assert!(exposure_queues.iter().all(|queue| queue.is_dropped()));
}
