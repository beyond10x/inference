//! Adversary guards: what a live session delivers after `cancel`, and after an interrupt when the
//! envelopes name no generation and only the events do. Each pins a line of `live.rs` the
//! facade's own suite leaves unobserved.

#![allow(dead_code)]

mod text;

use std::future;
use std::thread;

use inference_core::envelope::{EventEnvelope, EventId, Timebase};
use inference_core::generation::GenerationStarted;
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_facade::events::SessionEvent;
use inference_facade::live::LiveSessions;
use inference_pipeline::{
    InteractionMode, Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation,
};
use inference_testkit::graph::PipelinePayload;
use inference_testkit::stream::StreamEvent;
use inference_text::{SegmentId, TextDelta};

use text::{block_on, echo_planned, generator};

const DELTAS: i64 = 3;

/// A node that, as it starts, sends a generation's start and three of its deltas, every envelope
/// naming no generation, then holds its output until it drains.
struct Burst {
    run_id: RunId,
    node_id: Option<NodeId>,
    endpoints: Option<NodeEndpoints<PipelinePayload>>,
}

impl NodeImplementation<PipelinePayload> for Burst {
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
        let node_id = self.node_id.expect("described");
        let generation_id = GenerationId::generate();
        let segment_id = SegmentId::generate();
        let stream_id = StreamId::generate();
        let mut events = vec![StreamEvent::GenerationStarted(GenerationStarted {
            generation_id,
            run_id: self.run_id,
            epoch: 1,
        })];
        for sequence in 1..=DELTAS {
            events.push(StreamEvent::TextDelta(TextDelta {
                segment_id,
                generation_id,
                sequence,
                text: format!("d{sequence}"),
            }));
        }
        {
            let output = &endpoints.outputs[0];
            for (index, event) in events.into_iter().enumerate() {
                let envelope = EventEnvelope {
                    schema_version: 1,
                    event_id: EventId::generate(),
                    run_id: self.run_id,
                    session_id: None,
                    stream_id,
                    sequence: i64::try_from(index).expect("small") + 1,
                    producer: node_id,
                    port: "out".to_owned(),
                    kind: event.name().to_owned(),
                    generation_id: None,
                    timestamp: 0,
                    timebase: Timebase::WallTime,
                    causation_id: None,
                    payload: PipelinePayload::Stream(event),
                };
                block_on(output.endpoint.send(envelope)).expect("the egress has room");
            }
        }
        self.endpoints = Some(endpoints);
        Box::pin(future::ready(Ok(())))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        self.endpoints = None;
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

fn burst() -> impl FnMut(RunId, &Node) -> Box<dyn NodeImplementation<PipelinePayload>> {
    |run_id, _| {
        Box::new(Burst {
            run_id,
            node_id: None,
            endpoints: None,
        })
    }
}

fn deltas(events: &[SessionEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, SessionEvent::TextDelta(_)))
        .count()
}

/// `cancel` "delivers nothing more of what the run sends": the deltas the egress already holds
/// are not delivered.
#[test]
fn after_cancel_nothing_the_egress_still_holds_is_delivered() {
    let generator = generator(Vec::<String>::new());
    let (declaration, planned) = echo_planned(&generator);
    let sessions = LiveSessions::new();
    let (mut session, driver) =
        block_on(sessions.open(&declaration, &planned, InteractionMode::TurnBased, burst()))
            .expect("opens");
    session.cancel().expect("a Ready session is cancelled");
    let drive = thread::spawn(move || block_on(driver.drive()));
    let mut events = Vec::new();
    while let Some(event) = block_on(session.receive()) {
        events.push(event);
    }
    let _ = drive.join().expect("no panic");
    assert_eq!(deltas(&events), 0, "{events:#?}");
    assert!(matches!(
        events.last(),
        Some(SessionEvent::SessionClosed(_))
    ));
}

/// An interrupt cuts a generation delivered through an event that names it, when the envelope
/// names none: the deltas after the interrupt are not delivered.
#[test]
fn an_interrupt_cuts_a_generation_only_its_events_name() {
    let generator = generator(Vec::<String>::new());
    let (declaration, planned) = echo_planned(&generator);
    let sessions = LiveSessions::new();
    let (mut session, driver) =
        block_on(sessions.open(&declaration, &planned, InteractionMode::TurnBased, burst()))
            .expect("opens");
    let drive = thread::spawn(move || block_on(driver.drive()));
    loop {
        let event = block_on(session.receive()).expect("the burst");
        if matches!(event, SessionEvent::GenerationStarted(_)) {
            break;
        }
    }
    block_on(session.interrupt()).expect("a Ready session is interrupted");
    block_on(session.close()).expect("drains");
    let mut events = Vec::new();
    while let Some(event) = block_on(session.receive()) {
        events.push(event);
    }
    let _ = drive.join().expect("no panic");
    assert_eq!(deltas(&events), 0, "{events:#?}");
}
