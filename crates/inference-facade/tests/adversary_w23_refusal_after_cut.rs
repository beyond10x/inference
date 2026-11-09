//! Adversary pass 1 on `story:facade-session-refusal-event`: a refusal that arrives after the
//! end of its generation, once the session interrupted that generation.
//!
//! `inference-llm` keeps a refusal of a turn that ended through a supersede or a cancel of its
//! generation and delivers it after the generation's end, and the testkit's model node sends its
//! `GenerationSuperseded` or `GenerationCancelled` first and the kept refusal after it. The
//! session releases the bookkeeping of a cut generation at its end, so a refusal arriving after
//! that end is no longer known to be cut. `LiveSession::interrupt` promises that nothing more of
//! a generation the session delivered an event of is delivered, and `SessionEvent::
//! GenerationRefused` that an interrupt cuts it with its generation.
//!
//! The node here is hand-built: the testkit's model node requires a model claiming
//! `concurrent_input_output`, every such mock streams, and a streaming model's refusal is sent
//! before any cancel the node takes, so no pipeline of this repository was shown to send a
//! refusal after its generation's end.

#![allow(dead_code)]

mod text;

use std::future;
use std::panic;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use inference_core::envelope::{EventEnvelope, EventId, Timebase};
use inference_core::generation::{GenerationCancelled, GenerationRefused, GenerationStarted};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_facade::events::SessionEvent;
use inference_facade::live::LiveSessions;
use inference_pipeline::{
    InteractionMode, Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation,
};
use inference_testkit::graph::PipelinePayload;
use inference_testkit::stream::StreamEvent;

use text::{block_on, echo_planned, generator};

const REFUSAL: &str = "I cannot help with that request.";

const DEADLINE: Duration = Duration::from_secs(20);

fn within_deadline(body: impl FnOnce() + Send + 'static) {
    let (done, finished) = mpsc::channel();
    let worker = thread::spawn(move || {
        body();
        let _ = done.send(());
    });
    match finished.recv_timeout(DEADLINE) {
        Ok(()) | Err(RecvTimeoutError::Disconnected) => {
            if let Err(failure) = worker.join() {
                panic::resume_unwind(failure);
            }
        }
        Err(RecvTimeoutError::Timeout) => panic!("the session did not end within {DEADLINE:?}"),
    }
}

/// A node that starts one generation as it starts and, when it drains, sends that generation's
/// `GenerationCancelled` and then its refusal: the order the testkit's model node sends them in
/// when its generation is cancelled with a refusal kept.
struct Refuser {
    run_id: RunId,
    generation_id: GenerationId,
    node_id: Option<NodeId>,
    stream_id: StreamId,
    sequence: i64,
    endpoints: Option<NodeEndpoints<PipelinePayload>>,
}

impl Refuser {
    fn envelope(&mut self, event: StreamEvent) -> EventEnvelope<PipelinePayload> {
        self.sequence += 1;
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: self.run_id,
            session_id: None,
            stream_id: self.stream_id,
            sequence: self.sequence,
            producer: self.node_id.expect("described"),
            port: "out".to_owned(),
            kind: event.name().to_owned(),
            generation_id: None,
            timestamp: 0,
            timebase: Timebase::WallTime,
            causation_id: None,
            payload: PipelinePayload::Stream(event),
        }
    }
}

impl NodeImplementation<PipelinePayload> for Refuser {
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.node_id = Some(node.node_id());
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn start(
        &mut self,
        endpoints: NodeEndpoints<PipelinePayload>,
    ) -> NodeFuture<'_, Result<(), NodeError>> {
        let started = self.envelope(StreamEvent::GenerationStarted(GenerationStarted {
            generation_id: self.generation_id,
            run_id: self.run_id,
            epoch: 1,
        }));
        block_on(endpoints.outputs[0].endpoint.send(started)).expect("the egress has room");
        self.endpoints = Some(endpoints);
        Box::pin(future::ready(Ok(())))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        let cancelled = self.envelope(StreamEvent::GenerationCancelled(GenerationCancelled {
            generation_id: self.generation_id,
        }));
        let refused = self.envelope(StreamEvent::GenerationRefused(GenerationRefused {
            generation_id: self.generation_id,
            text: REFUSAL.to_owned(),
        }));
        if let Some(endpoints) = self.endpoints.take() {
            let output = &endpoints.outputs[0].endpoint;
            block_on(output.send(cancelled)).expect("the egress has room");
            block_on(output.send(refused)).expect("the egress has room");
        }
        Box::pin(future::ready(Ok(())))
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

fn refusals(events: &[SessionEvent]) -> Vec<&GenerationRefused> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::GenerationRefused(refused) => Some(refused),
            _ => None,
        })
        .collect()
}

/// The interrupted generation's refusal, sent after its `GenerationCancelled`, is not delivered.
#[test]
fn a_refusal_after_the_end_of_an_interrupted_generation_is_not_delivered() {
    within_deadline(|| {
        let generator = generator(Vec::<String>::new());
        let (declaration, planned) = echo_planned(&generator);
        let sessions = LiveSessions::new();
        let generation_id = GenerationId::generate();
        let (session, driver) = block_on(sessions.open(
            &declaration,
            &planned,
            InteractionMode::TurnBased,
            move |run_id, _| {
                Box::new(Refuser {
                    run_id,
                    generation_id,
                    node_id: None,
                    stream_id: StreamId::generate(),
                    sequence: 0,
                    endpoints: None,
                })
            },
        ))
        .expect("opens");
        let drive = thread::spawn(move || block_on(driver.drive()));
        loop {
            let event = block_on(session.receive()).expect("the generation starts");
            if matches!(event, SessionEvent::GenerationStarted(_)) {
                break;
            }
        }
        block_on(session.interrupt()).expect("a Ready session is interrupted");
        block_on(session.close()).expect("drains");
        let mut after = Vec::new();
        while let Some(event) = block_on(session.receive()) {
            after.push(event);
        }
        let _ = drive.join().expect("no panic");
        assert!(
            refusals(&after).is_empty(),
            "the session delivered a refusal of the generation it interrupted, once that \
             generation's end had released its cut: {after:#?}"
        );
    });
}
