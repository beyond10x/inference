//! `story:live-session-close-and-bounded-log`: a draining session whose run fails closes, an input
//! the ingress refused records no `InputSent`, and the session state machine's event log stays
//! within its bound however many inputs a session sends.

#![allow(dead_code)]

mod text;

use std::future;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use inference_core::id::{RunId, SessionId};
use inference_core::model::Capability;
use inference_facade::events::SessionEvent;
use inference_facade::live::{InputError, LiveSessions};
use inference_facade::session::{
    AttachRun, Graph, GraphPipeline, OpenSession, SessionClosed, SessionState, Sessions,
};
use inference_pipeline::{
    InteractionMode, Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation, PipelineId,
    PipelineState,
};
use inference_testkit::graph::PipelinePayload;
use uuid::Uuid;

use text::{Application, block_on, echo_planned, generator, implement, planned};

/// How long a case waits for a session to deliver its last event.
const PATIENCE: Duration = Duration::from_secs(20);

/// A node that holds its endpoints and fails its drain.
struct FailsItsDrain {
    endpoints: Option<NodeEndpoints<PipelinePayload>>,
}

impl NodeImplementation<PipelinePayload> for FailsItsDrain {
    fn describe(&mut self, _: &Node) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn start(
        &mut self,
        endpoints: NodeEndpoints<PipelinePayload>,
    ) -> NodeFuture<'_, Result<(), NodeError>> {
        self.endpoints = Some(endpoints);
        Box::pin(future::ready(Ok(())))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        self.endpoints = None;
        Box::pin(future::ready(Err(NodeError::new("the drain failed"))))
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        self.endpoints = None;
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        self.endpoints = None;
        Box::pin(future::ready(()))
    }
}

/// `close()` then a node failure during the drain: the session delivers `RunFailed`, then
/// `SessionClosed` last, and is Closed.
#[test]
fn a_draining_session_whose_run_fails_closes_and_delivers_session_closed_last() {
    let unused = generator(Vec::<String>::new());
    let (declaration, planned) = echo_planned(&unused);
    let sessions = LiveSessions::new();
    let (mut session, driver) = block_on(sessions.open(
        &declaration,
        &planned,
        InteractionMode::TurnBased,
        |_, _| Box::new(FailsItsDrain { endpoints: None }),
    ))
    .expect("opens");
    let session_id = session.session_id();
    let run_id = session.run_id();
    let drive = thread::spawn(move || block_on(driver.drive()));
    block_on(session.close()).expect("drains");

    let (done, wait) = mpsc::channel();
    thread::spawn(move || {
        let mut received = Vec::new();
        while let Some(event) = block_on(session.receive()) {
            received.push(event);
        }
        let _ = done.send(received);
    });
    let received = wait
        .recv_timeout(PATIENCE)
        .expect("receive answers none once everything was delivered");
    let driven = drive.join().expect("no panic");
    assert!(
        driven.is_err(),
        "the run loop answers the node's failure: {driven:?}"
    );

    let failed = received
        .iter()
        .position(
            |event| matches!(event, SessionEvent::RunFailed(failed) if failed.run_id == run_id),
        )
        .unwrap_or_else(|| panic!("the run's failure is delivered: {received:#?}"));
    assert_eq!(
        received.last(),
        Some(&SessionEvent::SessionClosed(SessionClosed { session_id })),
        "a draining session whose run failed delivers SessionClosed last: {received:#?}"
    );
    assert!(
        failed < received.len() - 1,
        "RunFailed comes before SessionClosed: {received:#?}"
    );
    assert_eq!(
        sessions.session(session_id).map(|s| s.state()),
        Some(SessionState::Closed),
        "a draining session whose run failed is Closed, not left Draining"
    );
    assert_eq!(
        sessions
            .events()
            .iter()
            .filter(|event| matches!(event, SessionEvent::SessionClosed(_)))
            .count(),
        1,
        "closed once"
    );
}

/// An input the ingress refused went to no run: the session records no `InputSent` for it,
/// neither among what it delivers nor in the state machine's log.
#[test]
fn an_input_the_ingress_refused_records_no_input_sent() {
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
    let input = application.text("1");
    block_on(session.send(input.clone())).expect("the first send is taken");
    match block_on(session.send(input)) {
        Err(InputError::Forward(_)) => {}
        other => panic!(
            "the ingress refuses a sequence that does not increase on its stream, not {other:?}"
        ),
    }
    block_on(session.commit_input()).expect("committed");
    block_on(session.close()).expect("drains");
    let mut received = Vec::new();
    while let Some(event) = block_on(session.receive()) {
        received.push(event);
    }
    drive.join().expect("no panic").expect("the run ends");

    let delivered = received
        .iter()
        .filter(|event| matches!(event, SessionEvent::InputSent(_)))
        .count();
    assert_eq!(
        delivered, 1,
        "one InputSent for the one input the run took: {received:#?}"
    );
    let logged = sessions
        .events()
        .iter()
        .filter(|event| matches!(event, SessionEvent::InputSent(_)))
        .count();
    assert_eq!(logged, 1, "the refused input left no InputSent in the log");
    assert_eq!(
        sessions.inputs_sent(session_id),
        1,
        "the session counts the one input its run took"
    );
}

/// The bound the event log of a [`Sessions`] states.
const BOUND: usize = 4096;

/// One Planned pipeline and a run of it.
struct Host;

const PIPELINE: u128 = 1;
const RUN: u128 = 2;

impl Graph for Host {
    fn pipeline(&self, pipeline_id: PipelineId) -> Option<GraphPipeline> {
        (pipeline_id == PipelineId::from(Uuid::from_u128(PIPELINE))).then_some(GraphPipeline {
            state: PipelineState::Planned,
            mode: InteractionMode::Duplex,
        })
    }

    fn run_pipeline(&self, run_id: RunId) -> Option<PipelineId> {
        (run_id == RunId::from(Uuid::from_u128(RUN)))
            .then_some(PipelineId::from(Uuid::from_u128(PIPELINE)))
    }
}

/// A Ready session of `sessions`.
fn ready(sessions: &mut Sessions) -> SessionId {
    let session_id = sessions
        .open_session(
            &Host,
            OpenSession {
                pipeline_id: PipelineId::from(Uuid::from_u128(PIPELINE)),
                mode: InteractionMode::Duplex,
            },
        )
        .expect("opens")
        .session_id;
    sessions
        .attach_run(
            &Host,
            AttachRun {
                session_id,
                run_id: RunId::from(Uuid::from_u128(RUN)),
            },
        )
        .expect("attached");
    sessions.mark_session_ready(session_id).expect("ready");
    session_id
}

/// Ten thousand audio sends, one `SendInput` per frame, leave the event log within its bound.
#[test]
fn ten_thousand_sends_leave_the_event_log_within_its_bound() {
    let mut sessions = Sessions::counted_in(7);
    let session_id = ready(&mut sessions);
    for _ in 0..10_000 {
        sessions
            .send_input(session_id)
            .expect("a Ready session takes input");
    }
    assert_eq!(Sessions::EVENT_LOG_BOUND, BOUND, "the bound the log states");
    assert_eq!(
        sessions.inputs_sent(session_id),
        10_000,
        "every input is counted"
    );
    assert!(
        sessions.events().len() <= BOUND,
        "the event log holds {} events, past its bound of {BOUND}",
        sessions.events().len()
    );
}

/// The bound holds for events that are not inputs as well: sends and commits interleaved, across
/// two sessions, leave the log within its bound, its newest event the last one recorded.
#[test]
fn interleaved_events_of_two_sessions_leave_the_event_log_within_its_bound() {
    let mut sessions = Sessions::counted_in(8);
    let first = ready(&mut sessions);
    let second = ready(&mut sessions);
    for _ in 0..10_000 {
        sessions.send_input(first).expect("sent");
        sessions.commit_input(second).expect("committed");
        sessions.send_input(second).expect("sent");
        sessions.commit_input(first).expect("committed");
    }
    assert!(
        sessions.events().len() <= BOUND,
        "the event log holds {} events, past its bound of {BOUND}",
        sessions.events().len()
    );
    assert!(
        matches!(
            sessions.events().last(),
            Some(SessionEvent::InputCommitted(committed)) if committed.session_id == first
        ),
        "the log keeps the newest events"
    );
}
