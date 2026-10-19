//! Session interrupt and the cancel branch of close (`story:session-interrupt-and-cancel`).
//!
//! The interrupt is cut at the session, the application's irreversible boundary
//! (`docs/design/inference.md` 637): a node that goes on computing the interrupted generation,
//! as the streaming node here does, ignoring the `cancel` the session sends it, has none of that
//! generation's events delivered, while the next input is answered by a new generation that is.
//! Cancelling a Ready session cancels its run and closes it without a drain.

#![allow(dead_code)]

mod text;

use std::future;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::Capability;
use inference_facade::events::SessionEvent;
use inference_facade::live::LiveSessions;
use inference_facade::session::{
    SessionClosed, SessionInterrupted, SessionMoveError, SessionState, SessionStateConflict,
};
use inference_llm::{GenerationEvent, GenerationRequest, Generator};
use inference_pipeline::{
    InteractionMode, Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation,
};
use inference_runtime::run::RunState;
use inference_testkit::graph::PipelinePayload;
use inference_testkit::llm::MockGenerator;
use inference_testkit::stream::StreamEvent;

use text::{Application, block_on, generator, implement, planned, planned_with_egress};

/// The long reply the mock model streams for every input: one segment per piece.
const PIECES: usize = 30;

fn long_reply() -> Vec<String> {
    (0..PIECES).map(|piece| format!("w{piece} ")).collect()
}

/// A streaming model node: for each committed transcript it takes, it starts a new generation on
/// its mock generator and sends every event of it, each envelope naming the generation, until
/// the generation completes. It records the control events it takes and does nothing else with
/// them, so a `cancel` leaves the generation running: computation outlives the cancellation of
/// delivery (641).
struct Streamer {
    generator: MockGenerator,
    run_id: RunId,
    node_id: Option<NodeId>,
    controls: Arc<Mutex<Vec<ControlKind>>>,
    exchange: Option<JoinHandle<()>>,
}

impl NodeImplementation<PipelinePayload> for Streamer {
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
        let NodeEndpoints { inputs, outputs } = endpoints;
        let (Some(mut input), Some(output), Some(node_id)) = (
            inputs.into_iter().next(),
            outputs.into_iter().next(),
            self.node_id,
        ) else {
            return Box::pin(future::ready(Err(NodeError::new(
                "one input, one output and a description",
            ))));
        };
        let mut generator = self.generator.clone();
        let run_id = self.run_id;
        let controls = Arc::clone(&self.controls);
        self.exchange = Some(thread::spawn(move || {
            let stream_id = StreamId::generate();
            let mut sequence = 0;
            let mut epoch = 0;
            let mut send = |event: StreamEvent, generation_id: Option<GenerationId>| {
                sequence += 1;
                let envelope = EventEnvelope {
                    schema_version: 1,
                    event_id: EventId::generate(),
                    run_id,
                    session_id: None,
                    stream_id,
                    sequence,
                    producer: node_id,
                    port: "answer".to_owned(),
                    kind: event.name().to_owned(),
                    generation_id,
                    timestamp: 0,
                    timebase: Timebase::WallTime,
                    causation_id: None,
                    payload: PipelinePayload::Stream(event),
                };
                block_on(output.endpoint.send(envelope)).is_ok()
            };
            while let Some(envelope) = block_on(input.endpoint.recv()) {
                match envelope.payload {
                    PipelinePayload::Stream(StreamEvent::TranscriptCommitted(committed)) => {
                        epoch += 1;
                        let generation = generator
                            .start(GenerationRequest {
                                run_id,
                                node_id,
                                epoch,
                                prompt: committed.text,
                            })
                            .expect("one generation at a time");
                        let mut generation_id = None;
                        for event in generation {
                            let event = match event {
                                GenerationEvent::Started(started) => {
                                    generation_id = Some(started.generation_id);
                                    StreamEvent::GenerationStarted(started)
                                }
                                GenerationEvent::TextDelta(delta) => StreamEvent::TextDelta(delta),
                                GenerationEvent::SegmentCommitted(segment) => {
                                    StreamEvent::TextSegmentCommitted(segment)
                                }
                                GenerationEvent::Completed(completed) => {
                                    StreamEvent::GenerationCompleted(completed)
                                }
                            };
                            if !send(event, generation_id) {
                                return;
                            }
                        }
                    }
                    PipelinePayload::Control(ControlKind::Stop) => {
                        let stop = EventEnvelope {
                            schema_version: 1,
                            event_id: EventId::generate(),
                            run_id,
                            session_id: None,
                            stream_id: StreamId::generate(),
                            sequence: 1,
                            producer: node_id,
                            port: "answer".to_owned(),
                            kind: ControlKind::Stop.as_str().to_owned(),
                            generation_id: None,
                            timestamp: 0,
                            timebase: Timebase::WallTime,
                            causation_id: None,
                            payload: PipelinePayload::Control(ControlKind::Stop),
                        };
                        let _ = block_on(output.endpoint.send(stop));
                        return;
                    }
                    PipelinePayload::Control(kind) => {
                        controls.lock().expect("the record").push(kind);
                    }
                    PipelinePayload::Stream(_) => {}
                }
            }
        }));
        Box::pin(future::ready(Ok(())))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        if let Some(exchange) = self.exchange.take() {
            exchange.join().expect("the streamer does not panic");
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

/// The generation an event names, if it names one.
fn generation(event: &SessionEvent) -> Option<GenerationId> {
    match event {
        SessionEvent::GenerationStarted(event) => Some(event.generation_id),
        SessionEvent::GenerationCompleted(event) => Some(event.generation_id),
        SessionEvent::TextDelta(event) => Some(event.generation_id),
        _ => None,
    }
}

#[test]
fn after_an_interrupt_nothing_of_the_cut_generation_is_delivered_and_the_next_input_is_answered_by_a_new_one()
 {
    let generator = generator(long_reply());
    let (declaration, planned) = planned_with_egress(
        &generator,
        &[Capability::OutputStreaming],
        NonZeroUsize::new(2).expect("2"),
    );
    let controls = Arc::new(Mutex::new(Vec::new()));
    let sessions = LiveSessions::new();
    let (mut session, driver) = {
        let controls = Arc::clone(&controls);
        let generator = generator.clone();
        block_on(sessions.open(
            &declaration,
            &planned,
            InteractionMode::TurnBased,
            move |run_id, _| {
                Box::new(Streamer {
                    generator: generator.clone(),
                    run_id,
                    node_id: None,
                    controls: Arc::clone(&controls),
                    exchange: None,
                }) as Box<dyn NodeImplementation<PipelinePayload>>
            },
        ))
        .expect("opens")
    };
    let session_id = session.session_id();
    let drive = thread::spawn(move || block_on(driver.drive()));
    for _ in 0..3 {
        block_on(session.receive()).expect("the session's lifecycle");
    }

    let mut application = Application::new(session.run_id());
    block_on(session.send(application.text("1"))).expect("sent");
    block_on(session.commit_input()).expect("committed");
    let mut first = None;
    loop {
        let event = block_on(session.receive()).expect("the reply streams");
        if let SessionEvent::GenerationStarted(started) = &event {
            first = Some(started.generation_id);
        }
        if matches!(event, SessionEvent::TextDelta(_)) {
            break;
        }
    }
    let first = first.expect("the first generation started");

    block_on(session.interrupt()).expect("a Ready session is interrupted");
    assert_eq!(
        sessions.session(session_id).map(|s| s.state()),
        Some(SessionState::Ready),
        "an interrupted session stays Ready"
    );
    block_on(session.send(application.text("2"))).expect("the next input goes in");
    block_on(session.commit_input()).expect("committed");

    let mut after = Vec::new();
    let second = loop {
        let event = block_on(session.receive()).expect("the next reply streams");
        let completed = match &event {
            SessionEvent::GenerationCompleted(completed) => Some(completed.generation_id),
            _ => None,
        };
        after.push(event);
        if let Some(generation_id) = completed {
            break generation_id;
        }
    };
    assert_ne!(
        second, first,
        "the next input is answered by a new generation"
    );
    assert!(
        after.iter().all(|event| generation(event) != Some(first)),
        "nothing of the interrupted generation is delivered after the interrupt: {after:#?}"
    );
    assert!(
        after
            .iter()
            .any(|event| matches!(event, SessionEvent::GenerationStarted(started) if started.generation_id == second))
    );
    assert_eq!(
        after
            .iter()
            .filter(|event| matches!(event, SessionEvent::TextSegmentCommitted(_)))
            .count(),
        PIECES,
        "only the new generation's segments are delivered"
    );
    assert!(
        after.contains(&SessionEvent::SessionInterrupted(SessionInterrupted {
            session_id
        }))
    );

    block_on(session.close()).expect("drains");
    while block_on(session.receive()).is_some() {}
    let run = drive.join().expect("no panic").expect("the run ends");
    assert_eq!(run.state(), RunState::Completed);
    assert!(
        controls
            .lock()
            .expect("the record")
            .contains(&ControlKind::Cancel),
        "the interrupt signalled cancel into the ingress"
    );
}

#[test]
fn cancelling_a_ready_session_closes_it_without_a_drain_with_the_run_cancelled_and_every_queue_dropped()
 {
    let generator = generator(long_reply());
    let (declaration, planned) = planned(&generator, &[Capability::OutputStreaming]);
    let sessions = LiveSessions::new();
    let (mut session, driver) = block_on(sessions.open(
        &declaration,
        &planned,
        InteractionMode::TurnBased,
        implement(&generator),
    ))
    .expect("opens");
    let session_id = session.session_id();
    let run_id = session.run_id();
    let exposure_queues = session.exposure_queues();
    let drive = thread::spawn(move || block_on(driver.drive()));
    let mut application = Application::new(run_id);
    block_on(session.send(application.text("1"))).expect("sent");

    session.cancel().expect("a Ready session is cancelled");
    let mut events = Vec::new();
    while let Some(event) = block_on(session.receive()) {
        events.push(event);
    }
    let run = drive.join().expect("no panic").expect("the run ends");
    assert_eq!(run.state(), RunState::Cancelled);
    assert_eq!(
        events.last(),
        Some(&SessionEvent::SessionClosed(SessionClosed { session_id })),
        "the close is delivered last: {events:#?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            SessionEvent::RunCancelled(cancelled) if cancelled.run_id == run_id
        )),
        "the run's cancellation is delivered: {events:#?}"
    );
    assert!(
        !sessions
            .events()
            .iter()
            .any(|event| matches!(event, SessionEvent::SessionDraining(_))),
        "a cancelled session never drained"
    );
    assert_eq!(
        sessions.session(session_id).map(|s| s.state()),
        Some(SessionState::Closed)
    );
    assert_eq!(
        session.cancel(),
        Err(SessionMoveError::WrongState(SessionStateConflict {
            current: SessionState::Closed,
        })),
        "a Closed session is not cancelled again"
    );
    drop(session);
    for queue in &exposure_queues {
        assert!(
            queue.is_dropped(),
            "the queue of exposure {} is dropped",
            queue.exposure_id()
        );
    }
}

#[test]
fn cancelling_a_draining_session_cuts_a_drain_that_has_not_finished() {
    let generator = generator(long_reply());
    let (declaration, planned) = planned_with_egress(
        &generator,
        &[Capability::OutputStreaming],
        NonZeroUsize::new(2).expect("2"),
    );
    let sessions = LiveSessions::new();
    let (mut session, driver) = block_on(sessions.open(
        &declaration,
        &planned,
        InteractionMode::TurnBased,
        implement(&generator),
    ))
    .expect("opens");
    let session_id = session.session_id();
    let drive = thread::spawn(move || block_on(driver.drive()));
    let mut application = Application::new(session.run_id());
    block_on(session.send(application.text("1"))).expect("sent");
    block_on(session.close()).expect("drains");
    assert_eq!(
        sessions.session(session_id).map(|s| s.state()),
        Some(SessionState::Draining)
    );
    session.cancel().expect("a draining session is cancelled");
    while block_on(session.receive()).is_some() {}
    let run = drive.join().expect("no panic").expect("the run ends");
    assert_eq!(
        sessions.session(session_id).map(|s| s.state()),
        Some(SessionState::Closed)
    );
    assert!(
        matches!(run.state(), RunState::Cancelled | RunState::Completed),
        "the run ends either way: {:?}",
        run.state()
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
