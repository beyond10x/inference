//! Second adversary cases on `LiveSession::send` and a cancel: a send started after the cancel,
//! while the cancelled run still winds down and the session is still Ready, answers `Cancelled`
//! and forwards nothing; a send the cancel overtook answers `Cancelled` while the session still
//! holds Ready; and a send taken while Draining that a cancel then overtook answers `Cancelled`,
//! not the `Ok` of a drain.

#![allow(dead_code)]

mod text;

use std::future::{self, Future};
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use inference_facade::live::{InputError, LiveSession, LiveSessions};
use inference_facade::session::SessionState;
use inference_pipeline::{
    InteractionMode, Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation, NodeInput,
};
use inference_testkit::graph::PipelinePayload;

use text::{Application, block_on, echo_planned_with_ingress, generator};

/// A node that takes nothing from its input until it is cancelled. Its cancel takes `takes`
/// envelopes from the input, says so on `took`, waits on `hold`, then tells on `left` whether
/// its input held one more envelope, and returns: until `hold` is released the run winds down.
struct OnCancel {
    takes: usize,
    input: Option<NodeInput<PipelinePayload>>,
    took: mpsc::Sender<()>,
    hold: Arc<Mutex<mpsc::Receiver<()>>>,
    left: mpsc::Sender<bool>,
}

impl NodeImplementation<PipelinePayload> for OnCancel {
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
        self.input = endpoints.inputs.into_iter().next();
        Box::pin(future::ready(Ok(())))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        if let Some(input) = self.input.as_mut() {
            for _ in 0..self.takes {
                let _ = block_on(input.endpoint.recv());
            }
        }
        let _ = self.took.send(());
        let _ = self.hold.lock().expect("lock").recv();
        let held = self.input.as_mut().is_some_and(|input| {
            let recv = pin!(input.endpoint.recv());
            matches!(
                recv.poll(&mut Context::from_waker(Waker::noop())),
                Poll::Ready(Some(_))
            )
        });
        let _ = self.left.send(held);
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }
}

struct Opened {
    sessions: Arc<LiveSessions>,
    session: Arc<LiveSession<PipelinePayload>>,
    application: Application,
    took: mpsc::Receiver<()>,
    release: mpsc::Sender<()>,
    left: mpsc::Receiver<bool>,
    drive: JoinHandle<()>,
}

fn open(takes: usize) -> Opened {
    let generator = generator(Vec::<String>::new());
    let (declaration, planned) =
        echo_planned_with_ingress(&generator, NonZeroUsize::new(1).expect("1"));
    let sessions = Arc::new(LiveSessions::new());
    let (took_tx, took) = mpsc::channel();
    let (release, hold) = mpsc::channel();
    let (left_tx, left) = mpsc::channel();
    let mut node = Some(OnCancel {
        takes,
        input: None,
        took: took_tx,
        hold: Arc::new(Mutex::new(hold)),
        left: left_tx,
    });
    let (session, driver) = block_on(sessions.open(
        &declaration,
        &planned,
        InteractionMode::TurnBased,
        |_, _| {
            Box::new(node.take().expect("one node")) as Box<dyn NodeImplementation<PipelinePayload>>
        },
    ))
    .expect("opens");
    let drive = thread::spawn(move || {
        let _ = block_on(driver.drive());
    });
    let session = Arc::new(session);
    let application = Application::new(session.run_id());
    Opened {
        sessions,
        session,
        application,
        took,
        release,
        left,
        drive,
    }
}

fn state(opened: &Opened) -> Option<SessionState> {
    opened
        .sessions
        .session(opened.session.session_id())
        .map(|row| row.state())
}

fn wait_for(what: &str, mut holds: impl FnMut() -> bool) {
    let limit = Instant::now() + Duration::from_secs(5);
    while !holds() {
        assert!(Instant::now() < limit, "within 5 s: {what}");
        thread::sleep(Duration::from_millis(2));
    }
}

fn finish(opened: Opened) {
    while block_on(opened.session.receive()).is_some() {}
    opened.drive.join().expect("no panic");
}

#[test]
fn a_send_started_after_the_cancel_answers_cancelled_and_forwards_nothing() {
    let mut opened = open(1);
    block_on(opened.session.send(opened.application.text("first")))
        .expect("the ingress buffers one");
    opened.session.cancel().expect("cancels");
    opened
        .took
        .recv_timeout(Duration::from_secs(5))
        .expect("the cancel reached the node, which took the first envelope");
    let before = state(&opened);
    let late = block_on(opened.session.send(opened.application.text("late")));
    let counted = opened.sessions.inputs_sent(opened.session.session_id());
    opened.release.send(()).expect("the node holds its cancel");
    let forwarded = opened
        .left
        .recv_timeout(Duration::from_secs(5))
        .expect("the node tells what its input held");
    finish(opened);

    assert_eq!(
        before,
        Some(SessionState::Ready),
        "the cancelled run is still winding down when the late send starts"
    );
    assert!(
        matches!(late, Err(InputError::Cancelled)),
        "a send started after the application cancelled answers Cancelled: {late:?}"
    );
    assert_eq!(counted, 1, "the late send is not counted");
    assert!(
        !forwarded,
        "InputError::Refused promises nothing was forwarded (live.rs:630), yet the late send's \
         envelope reached the cancelled run's node"
    );
}

#[test]
fn a_send_the_cancel_overtook_answers_cancelled_while_the_session_holds_ready() {
    let mut opened = open(1);
    block_on(opened.session.send(opened.application.text("first")))
        .expect("the ingress buffers one");
    let sender = {
        let session = Arc::clone(&opened.session);
        let second = opened.application.text("second");
        thread::spawn(move || block_on(session.send(second)))
    };
    thread::sleep(Duration::from_millis(100));
    opened.session.cancel().expect("cancels");
    opened
        .took
        .recv_timeout(Duration::from_secs(5))
        .expect("the cancel reached the node, which took the first envelope");
    let sent = sender.join().expect("the sender does not panic");
    let held = state(&opened);
    opened.release.send(()).expect("the node holds its cancel");
    let _ = opened.left.recv_timeout(Duration::from_secs(5));
    finish(opened);

    assert_eq!(
        held,
        Some(SessionState::Ready),
        "the cancelled run is still winding down when the send answers"
    );
    assert!(
        matches!(sent, Err(InputError::Cancelled)),
        "a send the cancel overtook answers Cancelled: {sent:?}"
    );
}

#[test]
fn a_send_taken_while_draining_that_a_cancel_overtook_answers_cancelled() {
    let mut opened = open(3);
    block_on(opened.session.send(opened.application.text("first")))
        .expect("the ingress buffers one");
    let sender = {
        let session = Arc::clone(&opened.session);
        let second = opened.application.text("second");
        thread::spawn(move || block_on(session.send(second)))
    };
    thread::sleep(Duration::from_millis(100));
    let closer = {
        let session = Arc::clone(&opened.session);
        thread::spawn(move || block_on(session.close()))
    };
    wait_for("the session is Draining", || {
        state(&opened) == Some(SessionState::Draining)
    });
    opened.session.cancel().expect("cancels a draining session");
    opened
        .took
        .recv_timeout(Duration::from_secs(5))
        .expect("the cancel reached the node, which took the first, the second and the stop");
    let sent = sender.join().expect("the sender does not panic");
    let held = state(&opened);
    let counted = opened.sessions.inputs_sent(opened.session.session_id());
    opened.release.send(()).expect("the node holds its cancel");
    let _ = opened.left.recv_timeout(Duration::from_secs(5));
    let _ = closer.join().expect("the closer does not panic");
    finish(opened);

    assert_eq!(
        held,
        Some(SessionState::Draining),
        "the cancelled run is still winding down when the send answers"
    );
    assert!(
        matches!(sent, Err(InputError::Cancelled)),
        "a draining session the application cancelled while the ingress took the send answers \
         Cancelled, not the Ok of a drain: {sent:?}"
    );
    assert_eq!(counted, 1, "the send the cancel overtook is not counted");
}
