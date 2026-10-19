//! Adversary: what `LiveSession::switch` delivers of the run it leaves, and what is left of a
//! session whose switch did not drain.

#![allow(dead_code)]

mod text;

use std::future::{self, Future};
use std::pin::pin;
use std::sync::mpsc;
use std::task::{Context, Poll, Waker};
use std::thread;
use std::time::Duration;

use inference_core::model::Capability;
use inference_facade::events::SessionEvent;
use inference_facade::live::LiveSessions;
use inference_facade::session::SessionState;
use inference_facade::switch::SwitchError;
use inference_pipeline::{
    InteractionMode, Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation,
};
use inference_runtime::run::RunState;
use inference_testkit::graph::PipelinePayload;

use text::{block_on, echo_planned, generator, implement, planned};

/// Polls `future` once.
fn poll_once<F: Future + Unpin>(future: &mut F) -> Poll<F::Output> {
    std::pin::Pin::new(future).poll(&mut Context::from_waker(Waker::noop()))
}

/// The old run's last events are delivered before the old run's end: the switch polls first, then
/// the old run's driver runs to its end, as a single-threaded join of the two polls them.
#[test]
fn the_old_runs_egress_tail_is_delivered_before_its_run_completed() {
    let first = generator(Vec::<String>::new());
    let (declaration, planned_first) = planned(&first, &[Capability::OutputStreaming]);
    let second = generator(Vec::<String>::new());
    let (second_declaration, planned_second) = planned(&second, &[Capability::OutputStreaming]);
    let sessions = LiveSessions::new();
    let (mut session, driver) = block_on(sessions.open(
        &declaration,
        &planned_first,
        InteractionMode::TurnBased,
        implement(&first),
    ))
    .expect("opens");
    let old_run = session.run_id();

    let new_driver = {
        let mut switching = Box::pin(session.switch(
            &second_declaration,
            &planned_second,
            InteractionMode::TurnBased,
            implement(&second),
        ));
        assert!(
            poll_once(&mut switching).is_pending(),
            "the old run has not ended yet"
        );
        let old = block_on(driver.drive()).expect("the old run ends");
        assert_eq!(old.state(), RunState::Completed);
        block_on(switching).expect("the switch completes")
    };
    let new_run = session.run_id();
    let mut received = Vec::new();
    loop {
        let event = block_on(session.receive()).expect("the session's own events");
        let attached =
            matches!(&event, SessionEvent::RunAttached(attached) if attached.run_id == new_run);
        received.push(event);
        if attached {
            break;
        }
    }
    let completed = received
        .iter()
        .position(|event| matches!(event, SessionEvent::RunCompleted(c) if c.run_id == old_run))
        .expect("the old run's end is delivered");
    let generation_completed = received
        .iter()
        .position(|event| matches!(event, SessionEvent::GenerationCompleted(_)))
        .expect("the old run's generation completes on the drain and is delivered");
    assert!(
        generation_completed < completed,
        "the old run's egress tail arrives after its RunCompleted: {received:#?}"
    );
    drop(new_driver);
}

/// A node whose drain either fails or never finishes.
struct Stuck {
    endpoints: Option<NodeEndpoints<PipelinePayload>>,
    fail: bool,
}

impl NodeImplementation<PipelinePayload> for Stuck {
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
        if self.fail {
            self.endpoints = None;
            Box::pin(future::ready(Err(NodeError::new("the drain failed"))))
        } else {
            Box::pin(future::pending())
        }
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

fn stuck(
    fail: bool,
) -> impl FnMut(inference_core::id::RunId, &Node) -> Box<dyn NodeImplementation<PipelinePayload>> {
    move |_, _| {
        Box::new(Stuck {
            endpoints: None,
            fail,
        })
    }
}

/// A switch whose old run fails while it drains answers `SwitchError::Drained` and leaves the
/// session Opened with no run; the application then cancels it, which must close it.
#[test]
fn a_session_whose_switch_did_not_drain_is_closed_by_cancel() {
    let first = generator(Vec::<String>::new());
    let (declaration, planned_first) = echo_planned(&first);
    let second = generator(Vec::<String>::new());
    let (second_declaration, planned_second) = planned(&second, &[Capability::OutputStreaming]);
    let sessions = LiveSessions::new();
    let (mut session, driver) = block_on(sessions.open(
        &declaration,
        &planned_first,
        InteractionMode::TurnBased,
        stuck(true),
    ))
    .expect("opens");
    let session_id = session.session_id();
    let drive = thread::spawn(move || block_on(driver.drive()));
    let switched = block_on(session.switch(
        &second_declaration,
        &planned_second,
        InteractionMode::TurnBased,
        implement(&second),
    ));
    assert!(
        matches!(switched, Err(SwitchError::Drained(RunState::Failed))),
        "{switched:?}"
    );
    let _ = drive.join().expect("no panic");
    assert_eq!(
        sessions.session(session_id).map(|s| s.state()),
        Some(SessionState::Opened)
    );

    session.cancel().expect("an Opened session is cancelled");
    assert_eq!(
        sessions.session(session_id).map(|s| s.state()),
        Some(SessionState::Closed),
        "cancel left a session with no run unclosed"
    );
}

/// A switch whose old run never drains is abandoned by the application (a timeout drops it); the
/// application then cancels the session, which cancels the old run and must close the session.
#[test]
fn a_session_whose_switch_never_drains_is_closed_by_cancel_once_the_switch_is_dropped() {
    let first = generator(Vec::<String>::new());
    let (declaration, planned_first) = echo_planned(&first);
    let second = generator(Vec::<String>::new());
    let (second_declaration, planned_second) = planned(&second, &[Capability::OutputStreaming]);
    let sessions = LiveSessions::new();
    let (mut session, driver) = block_on(sessions.open(
        &declaration,
        &planned_first,
        InteractionMode::TurnBased,
        stuck(false),
    ))
    .expect("opens");
    let session_id = session.session_id();
    let (ended, wait) = mpsc::channel();
    thread::spawn(move || {
        let _ = ended.send(block_on(driver.drive()).map(|run| run.state()));
    });
    {
        let mut switching = pin!(session.switch(
            &second_declaration,
            &planned_second,
            InteractionMode::TurnBased,
            implement(&second),
        ));
        for _ in 0..5 {
            assert!(
                poll_once(&mut switching).is_pending(),
                "the old run never drains"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    session.cancel().expect("an Opened session is cancelled");
    let old = wait
        .recv_timeout(Duration::from_secs(5))
        .expect("the cancel ends the old run");
    assert_eq!(old.ok(), Some(RunState::Cancelled));
    assert_eq!(
        sessions.session(session_id).map(|s| s.state()),
        Some(SessionState::Closed),
        "cancel left the session of an abandoned switch unclosed"
    );
}
