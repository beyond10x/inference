//! Adversarial pass 1 of story:sdk-agent against the shared `LiveSession`: `interrupt` now records
//! `SessionInterrupted` after its `cancel` sends (R178), with no lock held across them, while
//! `close` takes `&self` and can run meanwhile. The interrupt was admitted while the session was
//! Ready, but its event is recorded whenever the sends finish: after `SessionClosed`, which the
//! session promises is the last event delivered (`ess/domains/session.yaml`: "and then
//! SessionClosed last").

#![allow(dead_code)]

mod text;

use std::future::{self, Future};
use std::pin::pin;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::task::{Context, Waker};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use inference_core::envelope::ControlKind;
use inference_core::model::Capability;
use inference_facade::events::SessionEvent;
use inference_facade::live::LiveSessions;
use inference_pipeline::{
    InteractionMode, Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation,
};
use inference_testkit::graph::PipelinePayload;
use inference_testkit::stream::StreamEvent;

use text::{Application, block_on, generator, planned};

/// A node that reads nothing from its input until `gate` opens, then records what each input
/// envelope carries until the end of input, which it passes on (as `interrupt_and_cancel.rs`'s).
struct HeldIngressNode {
    gate: Option<mpsc::Receiver<()>>,
    taken: Arc<Mutex<Vec<String>>>,
    exchange: Option<JoinHandle<()>>,
}

impl NodeImplementation<PipelinePayload> for HeldIngressNode {
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
        let (Some(mut input), Some(output), Some(gate)) = (
            inputs.into_iter().next(),
            outputs.into_iter().next(),
            self.gate.take(),
        ) else {
            return Box::pin(future::ready(Err(NodeError::new(
                "one input, one output and a gate",
            ))));
        };
        let taken = Arc::clone(&self.taken);
        self.exchange = Some(thread::spawn(move || {
            let _ = gate.recv();
            while let Some(envelope) = block_on(input.endpoint.recv()) {
                let what = match &envelope.payload {
                    PipelinePayload::Stream(StreamEvent::TranscriptCommitted(committed)) => {
                        committed.text.clone()
                    }
                    PipelinePayload::Control(kind) => kind.as_str().to_owned(),
                    _ => continue,
                };
                taken.lock().expect("the record").push(what);
                if matches!(
                    envelope.payload,
                    PipelinePayload::Control(ControlKind::Stop)
                ) {
                    let _ = block_on(output.endpoint.send(envelope));
                    return;
                }
            }
        }));
        Box::pin(future::ready(Ok(())))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        if let Some(exchange) = self.exchange.take() {
            exchange.join().expect("the node does not panic");
        }
        Box::pin(future::ready(Ok(())))
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        drop(self.exchange.take());
        Box::pin(future::ready(()))
    }
}

/// An interrupt admitted while Ready whose `cancel` send is still waiting when another task closes
/// the session: once the close has delivered `SessionClosed` and `receive` has answered `None`, the
/// interrupt finishes and records `SessionInterrupted`, which `receive` then delivers after
/// `SessionClosed`.
#[test]
fn adversary1_no_event_is_received_after_session_closed_when_an_interrupt_races_close() {
    let generator = generator(vec!["w0 ".to_owned()]);
    let (declaration, planned) = planned(&generator, &[Capability::OutputStreaming]);
    let taken = Arc::new(Mutex::new(Vec::new()));
    let (open_gate, gate) = mpsc::channel();
    let mut gate = Some(gate);
    let sessions = LiveSessions::new();
    let (session, driver) = {
        let taken = Arc::clone(&taken);
        block_on(sessions.open(
            &declaration,
            &planned,
            InteractionMode::TurnBased,
            move |_, _| {
                Box::new(HeldIngressNode {
                    gate: gate.take(),
                    taken: Arc::clone(&taken),
                    exchange: None,
                }) as Box<dyn NodeImplementation<PipelinePayload>>
            },
        ))
        .expect("opens")
    };
    let drive = thread::spawn(move || block_on(driver.drive()));
    let session = Arc::new(session);
    let mut application = Application::new(session.run_id());
    for prompt in ["p1", "p2", "p3", "p4"] {
        block_on(session.send(application.text(prompt))).expect("the ingress has room for four");
    }

    // The interrupt is admitted (the session is Ready) and waits on the full ingress.
    let mut interrupt = pin!(session.interrupt());
    assert!(
        interrupt
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending(),
        "the interrupt waits on the full ingress"
    );

    // Another task closes the session and reads it to its end.
    let (closed, received) = mpsc::channel();
    let closer = {
        let session = Arc::clone(&session);
        thread::spawn(move || {
            let close = block_on(session.close());
            let mut events = Vec::new();
            while let Some(event) = block_on(session.receive()) {
                events.push(event);
            }
            let _ = closed.send((close, events));
        })
    };
    open_gate.send(()).expect("the node waits on the gate");
    let (close, events) = received
        .recv_timeout(Duration::from_secs(5))
        .expect("the close drains and the session ends");
    closer.join().expect("no panic");
    assert!(close.is_ok(), "{close:?}");
    assert!(
        matches!(events.last(), Some(SessionEvent::SessionClosed(_))),
        "the session ended with SessionClosed: {events:#?}"
    );
    drive.join().expect("no panic").expect("the run ends");

    // The interrupt's task runs again only now.
    let interrupted = block_on(interrupt.as_mut());
    let after = block_on(session.receive());
    assert!(
        after.is_none(),
        "an event was received after SessionClosed (interrupt returned {interrupted:?}): \
         {after:#?}"
    );
}
