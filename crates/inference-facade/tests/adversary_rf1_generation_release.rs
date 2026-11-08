//! Adversary pass 1 on `story:live-session-close-and-bounded-log`, finding N24: the session
//! releases the bookkeeping of a generation once eight later generations ended after it. An
//! interrupt then no longer cuts the generation whose playback is still in flight, though
//! `LiveSession::interrupt` promises that nothing more of a generation the session delivered an
//! event of is delivered, whatever the nodes still compute.

#![allow(dead_code)]

mod text;

use std::future;
use std::thread;

use inference_core::envelope::{EventEnvelope, EventId, Timebase};
use inference_core::generation::{GenerationCompleted, GenerationStarted};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_facade::events::SessionEvent;
use inference_facade::live::LiveSessions;
use inference_pipeline::{
    InteractionMode, Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation,
};
use inference_testkit::graph::PipelinePayload;
use inference_testkit::stream::StreamEvent;
use inference_text::{AudioPlayed, SegmentId, TextDelta};

use text::{block_on, echo_planned, generator};

/// A node that, as it starts, sends a first generation with one segment and its completion,
/// then `later` generations each started and completed, then the first segment's playback, as a
/// speaker that plays the first answer while the model goes on answering does. It holds its
/// output until it drains.
struct Talker {
    run_id: RunId,
    later: usize,
    node_id: Option<NodeId>,
    endpoints: Option<NodeEndpoints<PipelinePayload>>,
}

impl Talker {
    fn script(&self) -> Vec<StreamEvent> {
        let first = GenerationId::generate();
        let segment_id = SegmentId::generate();
        let mut events = vec![
            StreamEvent::GenerationStarted(GenerationStarted {
                generation_id: first,
                run_id: self.run_id,
                epoch: 1,
            }),
            StreamEvent::TextDelta(TextDelta {
                segment_id,
                generation_id: first,
                sequence: 1,
                text: "a long first answer".to_owned(),
            }),
            StreamEvent::GenerationCompleted(GenerationCompleted {
                generation_id: first,
                stop_reason: None,
            }),
        ];
        for epoch in 0..self.later {
            let generation_id = GenerationId::generate();
            events.push(StreamEvent::GenerationStarted(GenerationStarted {
                generation_id,
                run_id: self.run_id,
                epoch: i64::try_from(epoch).expect("small") + 2,
            }));
            events.push(StreamEvent::GenerationCompleted(GenerationCompleted {
                generation_id,
                stop_reason: None,
            }));
        }
        events.push(StreamEvent::AudioPlayed(AudioPlayed { segment_id }));
        events
    }
}

impl NodeImplementation<PipelinePayload> for Talker {
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
        let stream_id = StreamId::generate();
        {
            let output = &endpoints.outputs[0];
            for (index, event) in self.script().into_iter().enumerate() {
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

/// Receives the first generation and `later` generations after it, interrupts, closes, and
/// answers every event delivered after the interrupt.
fn after_interrupt(later: usize) -> Vec<SessionEvent> {
    let generator = generator(Vec::<String>::new());
    let (declaration, planned) = echo_planned(&generator);
    let sessions = LiveSessions::new();
    let (session, driver) = block_on(sessions.open(
        &declaration,
        &planned,
        InteractionMode::TurnBased,
        move |run_id, _| {
            Box::new(Talker {
                run_id,
                later,
                node_id: None,
                endpoints: None,
            })
        },
    ))
    .expect("opens");
    let drive = thread::spawn(move || block_on(driver.drive()));
    let mut completed = 0;
    while completed < later + 1 {
        let event = block_on(session.receive()).expect("the script");
        if matches!(event, SessionEvent::GenerationCompleted(_)) {
            completed += 1;
        }
    }
    block_on(session.interrupt()).expect("a Ready session is interrupted");
    block_on(session.close()).expect("drains");
    let mut events = Vec::new();
    while let Some(event) = block_on(session.receive()) {
        events.push(event);
    }
    let _ = drive.join().expect("no panic");
    events
}

fn played(events: &[SessionEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, SessionEvent::AudioPlayed(_)))
        .count()
}

/// Seven later generations: the interrupt cuts the first generation's playback.
#[test]
fn an_interrupt_cuts_the_playback_of_a_generation_seven_generations_back() {
    let events = after_interrupt(7);
    assert_eq!(played(&events), 0, "{events:#?}");
}

/// Eight later generations: the interrupt must cut the first generation's playback as it does
/// seven back; the session released its bookkeeping and delivers it.
#[test]
fn an_interrupt_cuts_the_playback_of_a_generation_eight_generations_back() {
    let events = after_interrupt(8);
    assert_eq!(
        played(&events),
        0,
        "the first generation was delivered, so an interrupt cuts what is left of it, its \
         playback among it; eight later generations released its bookkeeping: {events:#?}"
    );
}
