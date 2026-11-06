//! Review fix N18 (`story:pipeline-tool-and-node-cancel-findings`): the voice activity detector,
//! the turn policy and the commit gate send nothing more once cancelled, and a cancel taken while
//! one of their sends waits for room on a full edge ends the wait, as the tool nodes' sends do. Each
//! node is handed the end of input, which it passes on, and an output edge that never has room;
//! it is cancelled while it waits there, and its stop must then end.

use std::future::{self, Future};
use std::pin::pin;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use inference_audio::AudioFrame;
use inference_core::endpoint::{EndpointFuture, InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::nodes::commit_gate::{CommitGate, GateLane, GatePayload, Gated};
use inference_pipeline::nodes::turn_policy::{
    CompleteTurnRefused, TurnCommands, TurnEvent, TurnPayload, TurnPolicy,
};
use inference_pipeline::nodes::vad::{
    AudioPayload, SpeechActivity, VadConfig, VoiceActivityDetector,
};
use inference_pipeline::{
    Catalog, Declaration, InteractionMode, NodeEndpoints, NodeImplementation, NodeInput,
    NodeOutput, NodeRole, PipelineSpec, PortId,
};
use inference_text::{OpenTurn, TurnCompleted, TurnId, TurnOpened};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Payload {
    Control(ControlKind),
    Activity(SpeechActivity),
    Turn(TurnEvent),
}

impl AudioPayload for Payload {
    fn into_audio(self) -> Result<AudioFrame, Self> {
        Err(self)
    }

    fn from_activity(activity: SpeechActivity) -> Self {
        Self::Activity(activity)
    }

    fn control(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

impl TurnPayload for Payload {
    fn into_activity(self) -> Result<SpeechActivity, Self> {
        match self {
            Self::Activity(activity) => Ok(activity),
            other => Err(other),
        }
    }

    fn from_turn(event: TurnEvent) -> Self {
        Self::Turn(event)
    }

    fn control(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

impl GatePayload for Payload {
    fn gated(&self) -> Gated {
        Gated::Other
    }
}

/// A store of turns no command reaches: the cases send no speech activity.
struct NoTurns;

impl TurnCommands for NoTurns {
    fn open_turn(&mut self, _: OpenTurn) -> Result<TurnOpened, String> {
        Err("no turn is opened in this case".to_owned())
    }

    fn complete_turn(&mut self, _: TurnId) -> Result<TurnCompleted, CompleteTurnRefused> {
        unreachable!("no turn is opened in this case")
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
        thread::park();
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

/// A declaration of one policy node, to describe the node under test with.
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
        name: "cancel sends".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let mut declaration = Declaration::new();
    let (_, node_id) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            pipeline.node("policy", NodeRole::Policy, Vec::new()).id()
        })
        .expect("the profile is declared");
    (declaration, node_id)
}

/// The input edge: the end of input, then nothing, never closed.
struct EndOfInput(Option<EventEnvelope<Payload>>);

impl InputEndpoint<Payload> for EndOfInput {
    fn recv(&mut self) -> EndpointFuture<'_, Option<EventEnvelope<Payload>>> {
        let next = self.0.take();
        Box::pin(future::poll_fn(move |_| {
            next.clone()
                .map_or(Poll::Pending, |envelope| Poll::Ready(Some(envelope)))
        }))
    }
}

fn stop() -> EventEnvelope<Payload> {
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id: RunId::generate(),
        session_id: None,
        stream_id: StreamId::generate(),
        sequence: 1,
        producer: NodeId::generate(),
        port: "out".to_owned(),
        kind: ControlKind::Stop.as_str().to_owned(),
        generation_id: None,
        timestamp: 0,
        timebase: Timebase::WallTime,
        causation_id: None,
        payload: Payload::Control(ControlKind::Stop),
    }
}

/// The output edge: full for good. It tells the test when a send first waits on it, and never
/// takes anything.
struct Full {
    waiting: Mutex<Option<Sender<()>>>,
}

impl OutputEndpoint<Payload> for Full {
    fn send(
        &self,
        _: EventEnvelope<Payload>,
    ) -> EndpointFuture<'_, Result<(), SendError<Payload>>> {
        Box::pin(future::poll_fn(move |_| {
            let waiting = self
                .waiting
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            if let Some(waiting) = waiting {
                let _ = waiting.send(());
            }
            Poll::Pending
        }))
    }
}

/// Starts `node` on the end of input and a full output edge, cancels it once its send waits there
/// and answers whether its stop then ends.
fn stops_once_cancelled<N>(mut node: N, input: PortId, output: PortId) -> bool
where
    N: NodeImplementation<Payload> + Send + 'static,
{
    let (declaration, node_id) = declared();
    let (waiting, waited) = mpsc::channel();
    let endpoints = NodeEndpoints {
        inputs: vec![NodeInput {
            port_id: input,
            endpoint: Box::new(EndOfInput(Some(stop()))),
        }],
        outputs: vec![NodeOutput {
            port_id: output,
            endpoint: Box::new(Full {
                waiting: Mutex::new(Some(waiting)),
            }),
        }],
    };
    block_on(node.describe(declaration.node(node_id).expect("declared")));
    block_on(node.prepare()).expect("prepares");
    block_on(node.start(endpoints)).expect("starts");
    waited
        .recv_timeout(Duration::from_secs(10))
        .expect("the node's send of the end of input waits on the full edge");
    block_on(node.cancel());
    let (stopped, ended) = mpsc::channel();
    thread::spawn(move || {
        block_on(node.stop());
        let _ = stopped.send(());
    });
    ended.recv_timeout(Duration::from_secs(5)).is_ok()
}

#[test]
fn a_cancel_ends_a_voice_activity_detector_send_waiting_on_a_full_edge() {
    let detector = VoiceActivityDetector::new(
        RunId::generate(),
        VadConfig {
            threshold: 0.1,
            attack: Duration::from_millis(10),
            hold: Duration::from_millis(100),
        },
    );
    assert!(
        stops_once_cancelled(detector, PortId::generate(), PortId::generate()),
        "the cancel ends the detector's wait for room, and its stop ends"
    );
}

#[test]
fn a_cancel_ends_a_turn_policy_send_waiting_on_a_full_edge() {
    let policy = TurnPolicy::new(RunId::generate(), NoTurns);
    assert!(
        stops_once_cancelled(policy, PortId::generate(), PortId::generate()),
        "the cancel ends the turn policy's wait for room, and its stop ends"
    );
}

#[test]
fn a_cancel_ends_a_commit_gate_send_waiting_on_a_full_edge() {
    let lane = GateLane {
        input: PortId::generate(),
        output: PortId::generate(),
    };
    let gate = CommitGate::new([lane]);
    assert!(
        stops_once_cancelled(gate, lane.input, lane.output),
        "the cancel ends the commit gate's wait for room, and its stop ends"
    );
}
