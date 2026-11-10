//! Adversary cases on the input gate's copies of turn events (`story:cascade-turn-spans-in-a-burst`).
//!
//! The commit gate's module documentation: a turn event, other than the end of input, that arrives
//! on a lane other than the first is passed on that lane's output and, once the first lane's spans
//! were passed, on the first lane's output too, so that a consumer of the text lane alone, the
//! agent node, reads each turn's spans and its turn events in one order.
//!
//! The STT node ends a session by sending the last turn's `TurnCompleted` on `turns_out`, then the
//! end of input on `committed_text`, then on `turns_out`. These cases hand the gate those
//! envelopes in the two orders it can read them in and ask whether the first lane's output still
//! carries the last `TurnCompleted` before its end of input. Without it the agent never starts the
//! last turn's generation and drops its spans.

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
use inference_pipeline::nodes::commit_gate::{CommitGate, GateLane, GatePayload, Gated};
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

/// Runs an input gate of two lanes, the transcript on `text` and the turn events on `turns`,
/// until both lanes ended; what it passed on the text lane's output and on the turn lane's, and how
/// many `TurnCompleted` it recorded as passed.
fn run(text: Edge, turns: Edge) -> (Vec<Payload>, Vec<Payload>, usize) {
    let (declaration, node_id) = declared();
    let node = declaration.node(node_id).expect("declared").clone();
    let lanes = [
        GateLane {
            input: PortId::generate(),
            output: PortId::generate(),
        },
        GateLane {
            input: PortId::generate(),
            output: PortId::generate(),
        },
    ];
    let (text_out, turns_out) = (Edge::default(), Edge::default());
    let mut gate = CommitGate::new(lanes);
    let handle = gate.handle();
    let implementation: &mut dyn NodeImplementation<Payload> = &mut gate;
    block_on(implementation.describe(&node));
    block_on(implementation.prepare()).expect("the gate prepares");
    block_on(implementation.start(NodeEndpoints {
        inputs: vec![
            NodeInput {
                port_id: lanes[0].input,
                endpoint: Box::new(text),
            },
            NodeInput {
                port_id: lanes[1].input,
                endpoint: Box::new(turns),
            },
        ],
        outputs: vec![
            NodeOutput {
                port_id: lanes[0].output,
                endpoint: Box::new(text_out.clone()),
            },
            NodeOutput {
                port_id: lanes[1].output,
                endpoint: Box::new(turns_out.clone()),
            },
        ],
    }))
    .expect("the gate starts");
    block_on(implementation.drain()).expect("the gate drains once both lanes ended");
    block_on(implementation.stop());
    (
        text_out.payloads(),
        turns_out.payloads(),
        handle.turns_passed().len(),
    )
}

fn last_turn() -> (Payload, Payload, Payload) {
    let turn_id = TurnId::generate();
    let span = Payload::Content(Gated::SpanCommitted {
        span_id: SpanId::generate(),
    });
    let opened = Payload::Content(Gated::Turn(TurnEvent::Opened(TurnOpened {
        turn_id,
        run_id: RunId::generate(),
    })));
    let completed = Payload::Content(Gated::Turn(TurnEvent::Completed(TurnCompleted { turn_id })));
    (span, opened, completed)
}

const STOP: Payload = Payload::Control(ControlKind::Stop);

/// The gate reads late: the STT node already sent the last turn and both ends of input. The gate
/// asks its first lane first, meets the text lane's end of input, and takes the turn lane's
/// events in its drain before that end.
#[test]
fn a_turn_completed_taken_in_the_drain_before_the_text_lanes_end_reaches_the_text_lane() {
    let (span, opened, completed) = last_turn();
    let text = Edge::of(&[span, STOP]);
    let turns = Edge::of(&[opened, completed, STOP]);

    let (text_out, turns_out, _) = run(text, turns);

    assert_eq!(
        turns_out,
        [opened, completed, STOP],
        "the turn lane passes as it is"
    );
    assert_eq!(
        text_out,
        [span, opened, completed, STOP],
        "the text lane carries the last turn's events behind its span and before its end of \
         input, or the agent never answers that turn"
    );
}

/// The gate reads the last `TurnCompleted` first, and the STT node's end of input on the text lane
/// lands while the gate takes it: the gate drains the text lane, which ends, and the copy of the
/// `TurnCompleted` then has no lane to go on.
#[test]
fn a_turn_completed_read_while_the_text_lane_ends_reaches_the_text_lane() {
    let (span, opened, completed) = last_turn();
    let text = Edge::of(&[span]);
    let turns = Edge::of(&[opened, completed, STOP]);
    turns.feeding(2, &text, STOP);

    let (text_out, turns_out, _) = run(text, turns);

    assert_eq!(
        turns_out,
        [opened, completed, STOP],
        "the turn lane passes as it is"
    );
    assert_eq!(
        text_out,
        [span, opened, completed, STOP],
        "the text lane carries the last turn's completion before its end of input"
    );
}

/// The gate reads in time: the copies reach the text lane, and the gate records the passing of the
/// `TurnCompleted` once, on its own lane, not once more for its copy.
#[test]
fn a_turn_completed_passed_on_both_outputs_is_recorded_as_passed_once() {
    let (span, opened, completed) = last_turn();
    let text = Edge::of(&[span]);
    let turns = Edge::of(&[opened, completed, STOP]);
    turns.feeding(3, &text, STOP);

    let (text_out, turns_out, passed) = run(text, turns);

    assert_eq!(turns_out, [opened, completed, STOP]);
    assert_eq!(text_out, [span, opened, completed, STOP]);
    assert_eq!(passed, 1, "one TurnCompleted passed, recorded once");
}
