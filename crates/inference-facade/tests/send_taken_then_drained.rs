//! A send the ingress took is answered `Ok` when the session moved on to Draining while the
//! ingress took it. A send overtaken by a cancel is never answered `Ok`: the cancel closes the
//! ingress, so the window in which the ingress took the envelope and `Refused` answers it cannot
//! be held open from outside the facade.

#![allow(dead_code)]

mod text;

use std::future;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use inference_facade::events::SessionEvent;
use inference_facade::live::{InputError, LiveSession, LiveSessions};
use inference_facade::session::SessionState;
use inference_pipeline::{
    InteractionMode, Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation,
};
use inference_testkit::graph::PipelinePayload;

use text::{Application, block_on, echo_planned_with_ingress, generator};

/// A node that takes nothing from its input until `gate` is released, then passes on what its
/// input delivers.
/// A cancel releases the gate and ends the node without taking anything; the input it held is
/// kept, as a function taking one envelope from it, so the ingress stays open.
struct Held {
    gate: Option<mpsc::Receiver<()>>,
    signal: mpsc::Sender<()>,
    cancelled: Arc<AtomicBool>,
    kept: Arc<Mutex<Vec<Takes>>>,
    exchange: Option<JoinHandle<Option<Takes>>>,
}

type Takes = Box<dyn FnOnce() + Send>;

impl Held {
    fn join(&mut self) {
        if let Some(exchange) = self.exchange.take()
            && let Some(takes) = exchange.join().expect("the node does not panic")
        {
            self.kept.lock().expect("lock").push(takes);
        }
    }
}

impl NodeImplementation<PipelinePayload> for Held {
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
        let NodeEndpoints { inputs, outputs } = endpoints;
        let (Some(mut input), Some(output)) =
            (inputs.into_iter().next(), outputs.into_iter().next())
        else {
            return Box::pin(future::ready(Err(NodeError::new(
                "one input and one output",
            ))));
        };
        let gate = self.gate.take().expect("started once");
        let cancelled = Arc::clone(&self.cancelled);
        self.exchange = Some(thread::spawn(move || {
            let _ = gate.recv();
            if cancelled.load(Ordering::Acquire) {
                return Some(Box::new(move || {
                    let _ = block_on(input.endpoint.recv());
                }) as Takes);
            }
            while let Some(envelope) = block_on(input.endpoint.recv()) {
                if block_on(output.endpoint.send(envelope)).is_err() {
                    break;
                }
            }
            None
        }));
        Box::pin(future::ready(Ok(())))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        self.join();
        Box::pin(future::ready(Ok(())))
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        self.cancelled.store(true, Ordering::Release);
        let _ = self.signal.send(());
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        self.join();
        Box::pin(future::ready(()))
    }
}

struct Opened {
    sessions: Arc<LiveSessions>,
    session: Arc<LiveSession<PipelinePayload>>,
    release: mpsc::Sender<()>,
    kept: Arc<Mutex<Vec<Takes>>>,
    drive: JoinHandle<()>,
}

fn open() -> Opened {
    let generator = generator(Vec::<String>::new());
    let (declaration, planned) =
        echo_planned_with_ingress(&generator, NonZeroUsize::new(1).expect("1"));
    let sessions = Arc::new(LiveSessions::new());
    let (release, gate) = mpsc::channel();
    let kept = Arc::new(Mutex::new(Vec::new()));
    let mut held = Some(Held {
        gate: Some(gate),
        signal: release.clone(),
        cancelled: Arc::new(AtomicBool::new(false)),
        kept: Arc::clone(&kept),
        exchange: None,
    });
    let (session, driver) = block_on(sessions.open(
        &declaration,
        &planned,
        InteractionMode::TurnBased,
        |_, _| {
            Box::new(held.take().expect("one node")) as Box<dyn NodeImplementation<PipelinePayload>>
        },
    ))
    .expect("opens");
    let drive = thread::spawn(move || {
        let _ = block_on(driver.drive());
    });
    Opened {
        sessions,
        session: Arc::new(session),
        release,
        kept,
        drive,
    }
}

fn wait_for(what: &str, mut holds: impl FnMut() -> bool) {
    let limit = Instant::now() + Duration::from_secs(5);
    while !holds() {
        assert!(Instant::now() < limit, "within 5 s: {what}");
        thread::sleep(Duration::from_millis(2));
    }
}

/// Fills the ingress, then starts a send that waits on it; returns the waiting send.
fn waiting_send(opened: &Opened) -> JoinHandle<Result<(), InputError<PipelinePayload>>> {
    let session = Arc::clone(&opened.session);
    let mut application = Application::new(session.run_id());
    block_on(session.send(application.text("first"))).expect("the ingress buffers one");
    let sender = thread::spawn(move || block_on(session.send(application.text("second"))));
    thread::sleep(Duration::from_millis(100));
    sender
}

#[test]
fn a_send_the_ingress_took_after_close_moved_the_session_to_draining_answers_ok() {
    let opened = open();
    let sender = waiting_send(&opened);
    let closer = {
        let session = Arc::clone(&opened.session);
        thread::spawn(move || block_on(session.close()))
    };
    let id = opened.session.session_id();
    wait_for("the session is Draining", || {
        opened
            .sessions
            .session(id)
            .is_some_and(|row| row.state() == SessionState::Draining)
    });
    opened.release.send(()).expect("the node waits");
    let sent = sender.join().expect("the sender does not panic");
    assert!(sent.is_ok(), "the ingress took the input: {sent:?}");
    closer
        .join()
        .expect("the closer does not panic")
        .expect("closes");
    while block_on(opened.session.receive()).is_some() {}
    opened.drive.join().expect("no panic");
    let inputs = opened
        .sessions
        .events()
        .into_iter()
        .filter(|event| matches!(event, SessionEvent::InputSent(_)))
        .count();
    assert_eq!(inputs, 1, "one InputSent, for the input taken while Ready");
    assert_eq!(
        opened.sessions.inputs_sent(id),
        1,
        "no input counted for the one taken while Draining"
    );
}

#[test]
fn a_send_overtaken_by_a_cancel_is_never_answered_ok() {
    let opened = open();
    let sender = waiting_send(&opened);
    opened.session.cancel().expect("cancels");
    let id = opened.session.session_id();
    wait_for("the cancelled session is Closed", || {
        opened
            .sessions
            .session(id)
            .is_some_and(|row| row.state() == SessionState::Closed)
    });
    wait_for("the cancelled node kept its input", || {
        !opened.kept.lock().expect("lock").is_empty()
    });
    let takes = opened.kept.lock().expect("lock").pop().expect("kept");
    takes();
    let sent = sender.join().expect("the sender does not panic");
    assert!(
        matches!(sent, Err(InputError::Refused(_) | InputError::Forward(_))),
        "a cancelled session never answers Ok for a send it overtook: {sent:?}"
    );
    assert_eq!(
        opened.sessions.inputs_sent(id),
        1,
        "the cancelled send is not counted"
    );
    while block_on(opened.session.receive()).is_some() {}
    opened.drive.join().expect("no panic");
}
