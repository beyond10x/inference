//! A live session shared between tasks: one task sends while another receives, both through the
//! same `Arc<LiveSession<_>>`, because `send`, `receive` and `close` take `&self`. Over an
//! ingress of capacity 1 under `OverloadPolicy::Pause`, a sender faster than the run waits on
//! the ingress while the receiver keeps the egress drained, so neither waits on a lock the other
//! holds.

#![allow(dead_code)]

mod text;

use std::future;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use inference_core::envelope::ControlKind;
use inference_facade::events::SessionEvent;
use inference_facade::live::LiveSessions;
use inference_pipeline::{
    InteractionMode, Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation,
};
use inference_testkit::graph::PipelinePayload;

use text::{Application, block_on, echo_planned_with_ingress, generator};

const INPUTS: usize = 100;

/// A node passing on every envelope its one input delivers to its one output, until the end of
/// input, which it passes on too.
#[derive(Default)]
struct Echo {
    exchange: Option<JoinHandle<()>>,
}

impl Echo {
    fn join(&mut self) {
        if let Some(exchange) = self.exchange.take() {
            exchange.join().expect("the echo does not panic");
        }
    }
}

impl NodeImplementation<PipelinePayload> for Echo {
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
        self.exchange = Some(thread::spawn(move || {
            while let Some(envelope) = block_on(input.endpoint.recv()) {
                let stop = matches!(
                    envelope.payload,
                    PipelinePayload::Control(ControlKind::Stop)
                );
                if block_on(output.endpoint.send(envelope)).is_err() || stop {
                    break;
                }
            }
        }));
        Box::pin(future::ready(Ok(())))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        self.join();
        Box::pin(future::ready(Ok(())))
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        self.join();
        Box::pin(future::ready(()))
    }
}

#[test]
fn one_task_sends_while_another_receives_on_the_same_session() {
    let generator = generator(Vec::<String>::new());
    let (declaration, planned) =
        echo_planned_with_ingress(&generator, NonZeroUsize::new(1).expect("1"));
    let sessions = LiveSessions::new();
    let mut echo = Some(Echo::default());
    let (session, driver) = block_on(sessions.open(
        &declaration,
        &planned,
        InteractionMode::TurnBased,
        |_, _| {
            Box::new(echo.take().expect("one node")) as Box<dyn NodeImplementation<PipelinePayload>>
        },
    ))
    .expect("opens");
    let drive = thread::spawn(move || block_on(driver.drive()));
    let session = Arc::new(session);
    let run_id = session.run_id();

    let (done, finished) = mpsc::channel();
    let receiver = {
        let session = Arc::clone(&session);
        let done = done.clone();
        thread::spawn(move || {
            let mut received = 0;
            let mut after_close = Vec::new();
            while let Some(event) = block_on(session.receive()) {
                if matches!(event, SessionEvent::TranscriptCommitted(_)) {
                    received += 1;
                }
                after_close.push(event);
            }
            let _ = done.send("received");
            (received, after_close)
        })
    };
    let sender = {
        let session = Arc::clone(&session);
        thread::spawn(move || {
            let mut application = Application::new(run_id);
            for index in 0..INPUTS {
                block_on(session.send(application.text(&index.to_string())))
                    .expect("the ingress takes every input in turn");
            }
            let closed = block_on(session.close());
            let _ = done.send("sent");
            closed
        })
    };

    for _ in 0..2 {
        finished
            .recv_timeout(Duration::from_secs(5))
            .expect("sending and receiving finish within 5 s");
    }
    let closed = sender.join().expect("the sender does not panic");
    assert!(closed.is_ok(), "close from the sending task: {closed:?}");
    let (received, events) = receiver.join().expect("the receiver does not panic");
    assert_eq!(received, INPUTS, "every input is received");
    assert!(
        matches!(events.last(), Some(SessionEvent::SessionClosed(_))),
        "the session closed: {events:#?}"
    );
    assert_eq!(
        block_on(session.receive()),
        None,
        "receive answers None once the session closed"
    );
    drive.join().expect("no panic").expect("the run ends");
}
