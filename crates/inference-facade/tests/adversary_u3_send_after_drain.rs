//! Adversary cases on `LiveSession::send` once the ingress took the envelope: a session the
//! application cancelled while the ingress took it answers `Refused` and records no `InputSent`
//! (the doc of `send` and the story's decision), also while the cancelled run is still winding
//! down and the session is still Ready.

#![allow(dead_code)]

mod text;

use std::future;
use std::num::NonZeroUsize;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use inference_facade::events::SessionEvent;
use inference_facade::live::{InputError, LiveSession, LiveSessions};
use inference_facade::session::SessionState;
use inference_pipeline::{
    InteractionMode, Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation, NodeInput,
    NodeOutput,
};
use inference_testkit::graph::PipelinePayload;

use text::{Application, block_on, echo_planned_with_ingress, generator};

/// A node that takes nothing from its input until it is cancelled. Its cancel takes one envelope
/// from the input, as a node still reading when the cancel reaches it does, says so on `took`,
/// then waits on `hold` before it returns, so the run is still winding down and the session is
/// still Ready while the send that envelope made room for completes.
struct TakesOnCancel {
    input: Option<NodeInput<PipelinePayload>>,
    output: Option<NodeOutput<PipelinePayload>>,
    took: mpsc::Sender<()>,
    hold: Arc<Mutex<mpsc::Receiver<()>>>,
}

impl NodeImplementation<PipelinePayload> for TakesOnCancel {
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
        self.input = inputs.into_iter().next();
        self.output = outputs.into_iter().next();
        Box::pin(future::ready(Ok(())))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        if let Some(input) = self.input.as_mut() {
            let _ = block_on(input.endpoint.recv());
        }
        let _ = self.took.send(());
        let _ = self.hold.lock().expect("lock").recv();
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }
}

#[test]
fn a_send_the_ingress_took_while_the_cancelled_run_winds_down_answers_refused() {
    let generator = generator(Vec::<String>::new());
    let (declaration, planned) =
        echo_planned_with_ingress(&generator, NonZeroUsize::new(1).expect("1"));
    let sessions = Arc::new(LiveSessions::new());
    let (took, cancel_took) = mpsc::channel();
    let (release, hold) = mpsc::channel();
    let mut node = Some(TakesOnCancel {
        input: None,
        output: None,
        took,
        hold: Arc::new(Mutex::new(hold)),
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
    let session: Arc<LiveSession<PipelinePayload>> = Arc::new(session);
    let id = session.session_id();
    let mut application = Application::new(session.run_id());
    block_on(session.send(application.text("first"))).expect("the ingress buffers one");
    let sender: JoinHandle<Result<(), InputError<PipelinePayload>>> = {
        let session = Arc::clone(&session);
        thread::spawn(move || block_on(session.send(application.text("second"))))
    };
    thread::sleep(Duration::from_millis(100));

    session.cancel().expect("cancels");
    cancel_took
        .recv_timeout(Duration::from_secs(5))
        .expect("the cancel reached the node, which took one envelope");
    let sent = sender.join().expect("the sender does not panic");
    let state = sessions.session(id).map(|row| row.state());
    let counted = sessions.inputs_sent(id);
    let input_sent = sessions
        .events()
        .into_iter()
        .filter(|event| matches!(event, SessionEvent::InputSent(_)))
        .count();
    release.send(()).expect("the node holds its cancel");
    while block_on(session.receive()).is_some() {}
    drive.join().expect("no panic");

    assert_eq!(
        state,
        Some(SessionState::Ready),
        "the cancelled run is still winding down when the send completes"
    );
    assert!(
        matches!(sent, Err(InputError::Refused(_))),
        "a session cancelled while the ingress took the envelope answers Refused: {sent:?}"
    );
    assert_eq!(
        input_sent, 1,
        "no InputSent for the send the cancel overtook"
    );
    assert_eq!(counted, 1, "the send the cancel overtook is not counted");
}
