//! Adversary pass 2 on the input gate's copies of turn events and on its holding of the text
//! lane's end (`story:cascade-turn-spans-in-a-burst`).
//!
//! The gate copies every turn event it takes on a lane other than the first onto the first lane's
//! output, and holds the first lane's end until every other lane ended, while another lane is
//! open. These cases drive a gate through its edges: turn events the gate reads after it took the
//! text lane's end, a gate of one lane, and the record of the `TurnCompleted` passed.

use std::collections::VecDeque;
use std::future::{self, Future};
use std::pin::pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use inference_core::endpoint::{EndpointFuture, InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::nodes::commit_gate::{
    CommitGate, CommitGateHandle, GateLane, GatePayload, Gated,
};
use inference_pipeline::nodes::turn_policy::TurnEvent;
use inference_pipeline::{
    Catalog, Declaration, InteractionMode, NodeEndpoints, NodeImplementation, NodeInput,
    NodeOutput, NodeRole, PipelineSpec, PortId,
};
use inference_text::{SpanId, TurnCompleted, TurnId, TurnOpened};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Payload {
    Content(Gated),
    Control(ControlKind),
}

impl GatePayload for Payload {
    fn gated(&self) -> Gated {
        match self {
            Self::Content(gated) => *gated,
            Self::Control(_) => Gated::Other,
        }
    }
}

fn envelope(payload: Payload) -> EventEnvelope<Payload> {
    let kind = match payload {
        Payload::Control(kind) => kind.as_str().to_owned(),
        Payload::Content(_) => "content".to_owned(),
    };
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id: RunId::generate(),
        session_id: None,
        stream_id: StreamId::generate(),
        sequence: 1,
        producer: NodeId::generate(),
        port: "out".to_owned(),
        kind,
        generation_id: None,
        timestamp: 0,
        timebase: Timebase::WallTime,
        causation_id: None,
        payload,
    }
}

#[derive(Default)]
struct Queue {
    envelopes: VecDeque<EventEnvelope<Payload>>,
    waker: Option<Waker>,
    popped: usize,
    /// After this many envelopes were taken from this edge, `envelope` lands on `edge`: what a
    /// producer sends on its other port while the gate is reading this one.
    feed: Option<(usize, Edge, EventEnvelope<Payload>)>,
}

/// An unbounded edge the case writes and reads.
#[derive(Clone, Default)]
struct Edge(Arc<Mutex<Queue>>);

impl Edge {
    fn of(payloads: &[Payload]) -> Self {
        let edge = Self::default();
        for payload in payloads {
            edge.push(envelope(*payload));
        }
        edge
    }

    fn push(&self, envelope: EventEnvelope<Payload>) {
        let mut queue = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        queue.envelopes.push_back(envelope);
        if let Some(waker) = queue.waker.take() {
            waker.wake();
        }
    }

    fn feeding(&self, after: usize, edge: &Self, payload: Payload) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).feed =
            Some((after, edge.clone(), envelope(payload)));
    }

    fn payloads(&self) -> Vec<Payload> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .envelopes
            .iter()
            .map(|envelope| envelope.payload)
            .collect()
    }
}

impl OutputEndpoint<Payload> for Edge {
    fn send(
        &self,
        envelope: EventEnvelope<Payload>,
    ) -> EndpointFuture<'_, Result<(), SendError<Payload>>> {
        self.push(envelope);
        Box::pin(future::ready(Ok(())))
    }
}

impl InputEndpoint<Payload> for Edge {
    fn recv(&mut self) -> EndpointFuture<'_, Option<EventEnvelope<Payload>>> {
        let queue = Arc::clone(&self.0);
        Box::pin(future::poll_fn(move |cx| {
            let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(envelope) = queue.envelopes.pop_front() else {
                queue.waker = Some(cx.waker().clone());
                return Poll::Pending;
            };
            queue.popped += 1;
            let popped = queue.popped;
            if queue
                .feed
                .as_ref()
                .is_some_and(|(after, _, _)| *after == popped)
                && let Some((_, edge, fed)) = queue.feed.take()
            {
                drop(queue);
                edge.push(fed);
            }
            Poll::Ready(Some(envelope))
        }))
    }
}

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
            return output;
        }
        thread::park_timeout(Duration::from_millis(10));
    }
}

struct Fixture {
    profile: DeploymentProfile,
}

impl Catalog for Fixture {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.profile.profile_id() == profile_id).then_some(&self.profile)
    }

    fn adapter(&self, _: AdapterId) -> Option<&Adapter> {
        None
    }
}

fn declared() -> (Declaration, NodeId) {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    let catalog = Fixture { profile };
    let spec = PipelineSpec {
        name: "input gate".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let mut declaration = Declaration::new();
    let (_, node_id) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            pipeline
                .node("input gate", NodeRole::Policy, Vec::new())
                .id()
        })
        .expect("the profile is declared");
    (declaration, node_id)
}

/// Starts a gate of one lane per edge of `inputs`; its handle and each lane's output edge.
fn start(inputs: Vec<Edge>) -> (CommitGate, CommitGateHandle, Vec<Edge>) {
    let (declaration, node_id) = declared();
    let node = declaration.node(node_id).expect("declared").clone();
    let lanes: Vec<GateLane> = inputs
        .iter()
        .map(|_| GateLane {
            input: PortId::generate(),
            output: PortId::generate(),
        })
        .collect();
    let outputs: Vec<Edge> = inputs.iter().map(|_| Edge::default()).collect();
    let mut gate = CommitGate::new(lanes.clone());
    let handle = gate.handle();
    let implementation: &mut dyn NodeImplementation<Payload> = &mut gate;
    block_on(implementation.describe(&node));
    block_on(implementation.prepare()).expect("the gate prepares");
    block_on(
        implementation.start(NodeEndpoints {
            inputs: lanes
                .iter()
                .zip(inputs)
                .map(|(lane, edge)| NodeInput {
                    port_id: lane.input,
                    endpoint: Box::new(edge),
                })
                .collect(),
            outputs: lanes
                .iter()
                .zip(&outputs)
                .map(|(lane, edge)| NodeOutput {
                    port_id: lane.output,
                    endpoint: Box::new(edge.clone()),
                })
                .collect(),
        }),
    )
    .expect("the gate starts");
    (gate, handle, outputs)
}

/// Runs a gate of one lane per edge of `inputs` until every lane ended; what each lane's output
/// carried, and the turns of every `TurnCompleted` it recorded as passed, in order.
fn run(inputs: Vec<Edge>) -> (Vec<Vec<Payload>>, Vec<TurnId>) {
    let (mut gate, handle, outputs) = start(inputs);
    let implementation: &mut dyn NodeImplementation<Payload> = &mut gate;
    block_on(implementation.drain()).expect("the gate drains once every lane ended");
    block_on(implementation.stop());
    (
        outputs.iter().map(Edge::payloads).collect(),
        handle
            .turns_passed()
            .iter()
            .map(|passed| passed.turn_id)
            .collect(),
    )
}

struct Turn {
    turn_id: TurnId,
    span: Payload,
    opened: Payload,
    completed: Payload,
}

fn turn() -> Turn {
    let turn_id = TurnId::generate();
    Turn {
        turn_id,
        span: Payload::Content(Gated::SpanCommitted {
            span_id: SpanId::generate(),
        }),
        opened: Payload::Content(Gated::Turn(TurnEvent::Opened(TurnOpened {
            turn_id,
            run_id: RunId::generate(),
        }))),
        completed: Payload::Content(Gated::Turn(TurnEvent::Completed(TurnCompleted { turn_id }))),
    }
}

const STOP: Payload = Payload::Control(ControlKind::Stop);

/// The STT node's end of input on the text lane lands while the gate takes the first turn's
/// `TurnCompleted`, and the second turn's events are not yet on the turn lane: the gate takes the
/// text lane's end in its drain and must still pass both later turn events on the text lane ahead
/// of that end, each `TurnCompleted` recorded as passed once.
#[test]
fn turn_events_read_after_the_text_lanes_end_was_taken_reach_the_text_lane_ahead_of_it() {
    let (first, second) = (turn(), turn());
    let text = Edge::of(&[first.span]);
    let turns = Edge::of(&[
        first.opened,
        first.completed,
        second.opened,
        second.completed,
        STOP,
    ]);
    turns.feeding(2, &text, STOP);

    let (outputs, passed) = run(vec![text, turns]);

    assert_eq!(
        outputs[1],
        [
            first.opened,
            first.completed,
            second.opened,
            second.completed,
            STOP
        ],
        "the turn lane passes as it is"
    );
    assert_eq!(
        outputs[0],
        [
            first.span,
            first.opened,
            first.completed,
            second.opened,
            second.completed,
            STOP
        ],
        "every turn event the gate takes on the turn lane reaches the text lane, ahead of its end"
    );
    assert_eq!(
        passed,
        [first.turn_id, second.turn_id],
        "each TurnCompleted is recorded once, on its own lane, in order"
    );
}

/// A gate of one lane passes every turn event once: there is no other lane to copy it from.
#[test]
fn a_gate_of_one_lane_passes_each_turn_event_once() {
    let first = turn();
    let lane = Edge::of(&[first.opened, first.span, first.completed, STOP]);

    let (outputs, passed) = run(vec![lane]);

    assert_eq!(
        outputs[0],
        [first.opened, first.span, first.completed, STOP],
        "each turn event passed once, in arrival order"
    );
    assert_eq!(passed, [first.turn_id]);
}
