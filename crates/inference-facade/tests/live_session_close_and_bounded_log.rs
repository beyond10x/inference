//! `story:live-session-close-and-bounded-log`: a draining session whose run fails closes, an input
//! the ingress refused records no `InputSent`, and the session state machine's event log stays
//! within its bound however many inputs a session sends.
//!
//! `story:live-session-commit-and-close-under-full-ingress`: over a full ingress whose overload
//! policy is `fail`, a refused commit is neither delivered nor recorded, a refused close can be
//! retried, and both succeed once the ingress has room.

#![allow(dead_code)]

mod text;

use std::future::{self, Future};
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::mpsc;
use std::task::{Context, Poll, Waker};
use std::thread;
use std::time::Duration;

use inference_core::endpoint::{InputEndpoint, SendError};
use inference_core::envelope::ControlKind;
use inference_core::id::{RunId, SessionId};
use inference_core::model::Capability;
use inference_facade::events::SessionEvent;
use inference_facade::history::HistoryKind;
use inference_facade::live::{CloseError, InputError, LiveSession, LiveSessions};
use inference_facade::session::{
    AttachRun, Graph, GraphPipeline, OpenSession, SessionClosed, SessionState, Sessions,
};
use inference_pipeline::{
    InteractionMode, Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation,
    OverloadPolicy, PipelineId, PipelineState,
};
use inference_testkit::graph::PipelinePayload;
use uuid::Uuid;

use text::{
    Application, block_on, echo_planned, generator, implement, planned, sink_planned_with_ingress,
};

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
    let (session, driver) = block_on(sessions.open(
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
    let (session, driver) = block_on(sessions.open(
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
    assert_eq!(
        sessions.events_released(),
        0,
        "a log within its bound released nothing"
    );
}

/// The log keeps every event up to exactly its bound; the event one past it releases the oldest
/// half, which the count of released events says.
#[test]
fn the_log_holds_exactly_its_bound_and_releases_its_oldest_half_one_past_it() {
    let mut sessions = Sessions::counted_in(9);
    let session_id = ready(&mut sessions);
    while sessions.events().len() < BOUND {
        let before = sessions.events().len();
        sessions.commit_input(session_id).expect("committed");
        assert_eq!(
            sessions.events().len(),
            before + 1,
            "below its bound the log keeps every event"
        );
    }
    assert_eq!(sessions.events().len(), BOUND, "a full log holds its bound");
    assert_eq!(sessions.events_released(), 0, "a full log released nothing");
    sessions.commit_input(session_id).expect("committed");
    assert_eq!(
        sessions.events().len(),
        BOUND / 2 + 1,
        "one past its bound the log released its oldest half, then took the event"
    );
    assert_eq!(
        sessions.events_released(),
        u64::try_from(BOUND / 2).expect("small")
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

/// A sink that hands the inner end of its one ingress to the test, which reads it when it wants
/// the ingress to have room, and that drains at once.
struct HandsOverItsInput {
    inputs: mpsc::Sender<Box<dyn InputEndpoint<PipelinePayload>>>,
}

impl NodeImplementation<PipelinePayload> for HandsOverItsInput {
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
        for input in endpoints.inputs {
            let _ = self.inputs.send(input.endpoint);
        }
        Box::pin(future::ready(Ok(())))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }
}

/// A Ready session over a sink behind a one-place ingress that fails on overload, its run driven
/// on a thread of its own, and the inner end of that ingress.
fn over_a_one_place_failing_ingress() -> (
    LiveSessions,
    LiveSession<PipelinePayload>,
    thread::JoinHandle<Result<inference_runtime::Run, inference_runtime::RunError>>,
    Box<dyn InputEndpoint<PipelinePayload>>,
) {
    let unused = generator(Vec::<String>::new());
    let (declaration, planned) =
        sink_planned_with_ingress(&unused, NonZeroUsize::MIN, OverloadPolicy::Fail);
    let sessions = LiveSessions::new();
    let (inputs, handed) = mpsc::channel();
    let (session, driver) = block_on(sessions.open(
        &declaration,
        &planned,
        InteractionMode::TurnBased,
        move |_, _| {
            Box::new(HandsOverItsInput {
                inputs: inputs.clone(),
            })
        },
    ))
    .expect("opens");
    let drive = thread::spawn(move || block_on(driver.drive()));
    let input = handed
        .recv_timeout(PATIENCE)
        .expect("the sink started and handed over its input");
    (sessions, session, drive, input)
}

/// The events the session has queued for the application now, without waiting for more.
fn queued(session: &LiveSession<PipelinePayload>) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    loop {
        let mut receive = pin!(session.receive());
        match receive
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(Some(event)) => events.push(event),
            Poll::Ready(None) | Poll::Pending => return events,
        }
    }
}

/// Every event the session delivers until it answers none.
fn the_rest(session: LiveSession<PipelinePayload>) -> Vec<SessionEvent> {
    let (done, wait) = mpsc::channel();
    thread::spawn(move || {
        let mut received = Vec::new();
        while let Some(event) = block_on(session.receive()) {
            received.push(event);
        }
        let _ = done.send(received);
    });
    wait.recv_timeout(PATIENCE)
        .expect("receive answers none once everything was delivered")
}

/// Reads the next envelope off the ingress's inner end, which makes room for one more.
fn make_room(input: &mut Box<dyn InputEndpoint<PipelinePayload>>) -> String {
    block_on(input.recv())
        .expect("the ingress holds an envelope")
        .kind
}

/// Finding 8: a commit the full ingress refused is not delivered as `InputCommitted`, not logged
/// and not recorded as a turn, so the application can tell it must commit again; once there is
/// room, the second commit is delivered and recorded once.
#[test]
fn a_commit_a_full_ingress_refused_is_neither_delivered_nor_recorded_until_it_is_taken() {
    let (sessions, session, drive, mut input) = over_a_one_place_failing_ingress();
    let session_id = session.session_id();
    let mut application = Application::new(session.run_id());
    block_on(session.send(application.text("held text"))).expect("the one place takes the input");

    match block_on(session.commit_input()) {
        Err(InputError::Forward(error)) if matches!(*error, SendError::Full(_)) => {}
        other => panic!("the full ingress refuses the commit as Full, not {other:?}"),
    }
    let delivered = queued(&session);
    assert!(
        !delivered
            .iter()
            .any(|event| matches!(event, SessionEvent::InputCommitted(_))),
        "a refused commit delivers no InputCommitted: {delivered:#?}"
    );
    assert!(
        !sessions
            .events()
            .iter()
            .any(|event| matches!(event, SessionEvent::InputCommitted(_))),
        "a refused commit logs no InputCommitted"
    );
    assert!(
        sessions.history(session_id).is_empty(),
        "a refused commit records no turn: {:#?}",
        sessions.history(session_id)
    );

    assert_ne!(
        make_room(&mut input),
        ControlKind::Commit.as_str(),
        "the queue held the input, not a commit"
    );
    block_on(session.commit_input()).expect("with room, the commit is taken");
    let delivered = queued(&session);
    assert_eq!(
        delivered
            .iter()
            .filter(|event| matches!(event, SessionEvent::InputCommitted(_)))
            .count(),
        1,
        "the commit the run took is delivered once: {delivered:#?}"
    );
    let history = sessions.history(session_id);
    assert_eq!(
        history
            .iter()
            .map(|entry| (entry.kind(), entry.text().to_owned()))
            .collect::<Vec<_>>(),
        vec![(HistoryKind::UserTurn, "held text".to_owned())],
        "the committed turn is recorded once"
    );

    assert_eq!(make_room(&mut input), ControlKind::Commit.as_str());
    block_on(session.close()).expect("drains");
    drop(input);
    let _ = the_rest(session);
    drive.join().expect("no panic").expect("the run ends");
}

/// Finding 9: a close whose end of input the full ingress refused leaves the session Draining,
/// and once the ingress has room a second close delivers the end of input: the run completes and
/// the session closes.
#[test]
fn a_close_a_full_ingress_refused_can_be_retried_once_it_has_room() {
    let (sessions, session, drive, mut input) = over_a_one_place_failing_ingress();
    let session_id = session.session_id();
    let run_id = session.run_id();
    let mut application = Application::new(run_id);
    block_on(session.send(application.text("held text"))).expect("the one place takes the input");

    match block_on(session.close()) {
        Err(CloseError::Forward(error)) if matches!(*error, SendError::Full(_)) => {}
        other => panic!("the full ingress refuses the end of input as Full, not {other:?}"),
    }
    assert_eq!(
        sessions.session(session_id).map(|s| s.state()),
        Some(SessionState::Draining)
    );

    make_room(&mut input);
    block_on(session.close()).expect("with room, a second close delivers the end of input");
    assert_eq!(make_room(&mut input), ControlKind::Stop.as_str());
    drop(input);
    let received = the_rest(session);
    drive.join().expect("no panic").expect("the run completes");

    let completed = received
        .iter()
        .position(|event| {
            matches!(event, SessionEvent::RunCompleted(completed) if completed.run_id == run_id)
        })
        .unwrap_or_else(|| panic!("the run completes: {received:#?}"));
    assert_eq!(
        received.last(),
        Some(&SessionEvent::SessionClosed(SessionClosed { session_id })),
        "SessionClosed comes last: {received:#?}"
    );
    assert!(completed < received.len() - 1);
    assert_eq!(
        sessions.session(session_id).map(|s| s.state()),
        Some(SessionState::Closed)
    );
}
