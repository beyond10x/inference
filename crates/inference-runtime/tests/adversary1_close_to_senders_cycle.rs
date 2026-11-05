//! Adversary pass on `story:run-ingress-admitted-send-outlives-the-drain`: a producer inside the
//! run whose edge into a subgraph node the node's ingress bridge closes to its senders while the
//! producer's send still waits for room.
//!
//! The only producer that still sends into a subgraph node once the node drains is one that drains
//! with it: a node on a feedback cycle through the subgraph node, both in the last drain layer
//! (`RunController::drain`). Here a relay node takes the caller's text off the ingress and sends it
//! into the subgraph node, whose reply comes back to the relay over a feedback edge. The relay
//! answers a refused send as every node of the workspace does outside a cancel, as its error
//! (`.map_err(|refused| refused.to_string())`, for example
//! `inference-pipeline/src/nodes/vad.rs:321`, `turn_policy.rs:228`, `commit_gate.rs:435`,
//! `inference-testkit/src/graph/nodes.rs:199`), and its drain answers that error, as
//! `vad.rs:427` does.
//!
//! The relay's `stop` waits for room on its full edge into the subgraph node when the run drains.
//! The ingress bridge, at its first read of the drain, takes the envelope that makes room or
//! closes the edge to its senders first. Every exposure and edge holds one envelope; the child's
//! node hands the test its input, so the test orders every take. No case waits on time.

use std::collections::HashMap;
use std::future::{self, Future};
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use inference_core::endpoint::{InputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId, Capability, ModelId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeEndpoints, NodeError,
    NodeFuture, NodeImplementation, NodeOutput, NodeRole, OverloadPolicy, PipelineId, PipelineSpec,
    PlannedPipeline, PortTraits, modality,
};
use inference_runtime::run::RunState;
use inference_runtime::{RunController, SubgraphNode};
use inference_testkit::graph::PipelinePayload;
use inference_testkit::llm::{MockConfiguration, MockGenerator};
use inference_testkit::stream::StreamEvent;
use inference_text::{SpanId, TranscriptCommitted};

const PATIENCE: Duration = Duration::from_secs(20);

const STOP: &str = "stop";

type Input = Box<dyn InputEndpoint<PipelinePayload>>;

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
        thread::park_timeout(Duration::from_millis(50));
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A value another thread sets once, and the task waiting for it.
struct Slot<T>(Mutex<(Option<T>, Option<Waker>)>);

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self(Mutex::new((None, None)))
    }
}

impl<T: Clone> Slot<T> {
    fn set(&self, value: T) {
        let waker = {
            let mut state = lock(&self.0);
            state.0 = Some(value);
            state.1.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    fn poll_get(&self, cx: &Context<'_>) -> Poll<T> {
        let mut state = lock(&self.0);
        if let Some(value) = state.0.clone() {
            return Poll::Ready(value);
        }
        state.1 = Some(cx.waker().clone());
        Poll::Pending
    }
}

/// The child's node: hands the test the receiving end of its input when it starts, holds its
/// outputs until it stops, and drains once the test released it.
struct Hand {
    input: mpsc::Sender<Input>,
    released: Arc<Slot<()>>,
    outputs: Vec<NodeOutput<PipelinePayload>>,
}

impl NodeImplementation<PipelinePayload> for Hand {
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
        for input in endpoints.inputs {
            let _test_waits_or_gone = self.input.send(input.endpoint);
        }
        self.outputs = endpoints.outputs;
        Box::pin(future::ready(Ok(())))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        let released = Arc::clone(&self.released);
        Box::pin(async move {
            future::poll_fn(|cx| released.poll_get(cx)).await;
            Ok(())
        })
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        self.outputs.clear();
        Box::pin(future::ready(()))
    }
}

/// A node whose drain tells the test once it was first polled: for a subgraph node, once its
/// bridges were told to drain.
struct DrainPolled {
    inner: Box<dyn NodeImplementation<PipelinePayload>>,
    polled: mpsc::Sender<()>,
}

impl NodeImplementation<PipelinePayload> for DrainPolled {
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.inner.describe(node)
    }

    fn described(&mut self, instance_id: inference_pipeline::NodeInstanceId) {
        self.inner.described(instance_id);
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        self.inner.prepare()
    }

    fn start(
        &mut self,
        endpoints: NodeEndpoints<PipelinePayload>,
    ) -> NodeFuture<'_, Result<(), NodeError>> {
        self.inner.start(endpoints)
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        let polled = self.polled.clone();
        let mut drain = self.inner.drain();
        Box::pin(async move {
            let mut told = false;
            future::poll_fn(|cx| {
                let answer = drain.as_mut().poll(cx);
                if !std::mem::replace(&mut told, true) {
                    let _test_waits_or_gone = polled.send(());
                }
                answer
            })
            .await
        })
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        self.inner.cancel()
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        self.inner.stop()
    }
}

/// The relay on the cycle: on a thread of its own, takes each envelope of its `user text` input
/// and sends it on its one output, into the subgraph node, telling the test what each send
/// answered; it ends after `stop`. A refused send is its error, and its drain answers it. It
/// holds its `back` input, the subgraph node's reply, without reading it.
struct Relay {
    declaration: Arc<Declaration>,
    answers: mpsc::Sender<String>,
    ended: Arc<Slot<Result<(), String>>>,
    back: Vec<Input>,
}

impl NodeImplementation<PipelinePayload> for Relay {
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
        let mut user = None;
        for input in endpoints.inputs {
            let name = self
                .declaration
                .port(input.port_id)
                .map(|port| port.name().to_owned());
            if name.as_deref() == Some("user text") {
                user = Some(input.endpoint);
            } else {
                self.back.push(input.endpoint);
            }
        }
        let mut user = user.expect("the relay's ingress");
        let output = endpoints
            .outputs
            .into_iter()
            .next()
            .expect("the relay's edge into the subgraph node")
            .endpoint;
        let answers = self.answers.clone();
        let ended = Arc::clone(&self.ended);
        thread::spawn(move || {
            let mut exchange = || -> Result<(), String> {
                while let Some(envelope) = block_on(user.recv()) {
                    let label = label(&envelope);
                    let sent = block_on(output.send(envelope));
                    let _test_waits_or_gone = answers.send(format!("{label}: {}", outcome(&sent)));
                    sent.map_err(|refused| refused.to_string())?;
                    if label == STOP {
                        return Ok(());
                    }
                }
                Ok(())
            };
            ended.set(exchange());
        });
        Box::pin(future::ready(Ok(())))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        let ended = Arc::clone(&self.ended);
        Box::pin(async move {
            future::poll_fn(|cx| ended.poll_get(cx))
                .await
                .map_err(NodeError::new)
        })
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        self.back.clear();
        Box::pin(future::ready(()))
    }
}

struct Fixture {
    profile: DeploymentProfile,
    adapter: Adapter,
}

impl Catalog for Fixture {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.profile.profile_id() == profile_id).then_some(&self.profile)
    }

    fn adapter(&self, adapter_id: AdapterId) -> Option<&Adapter> {
        (self.adapter.adapter_id() == adapter_id).then_some(&self.adapter)
    }
}

const TEXT: PortTraits = PortTraits {
    essentiality: Essentiality::Essential,
    delivery: Delivery::Reliable,
    incremental: true,
    end_marker_required: true,
    provisional_allowed: false,
};

fn spec(name: &str) -> PipelineSpec {
    PipelineSpec {
        name: name.to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: vec![Capability::OutputStreaming],
    }
}

fn one() -> NonZeroUsize {
    NonZeroUsize::MIN
}

fn plan(declaration: &mut Declaration, catalog: &Fixture, pipeline: PipelineId) -> PlannedPipeline {
    let validated = declaration.validate(pipeline).expect("valid");
    declaration.plan(validated, catalog).expect("planned")
}

/// The child: one node between the ingress `user text` and the egress `reply`. The parent: the
/// relay between the ingress `user text` and the subgraph node over the child, whose `reply`
/// comes back to the relay over a feedback edge. Every exposure and edge holds one envelope
/// under `pause`.
struct Pipelines {
    declaration: Arc<Declaration>,
    flat: PlannedPipeline,
    cycle: PlannedPipeline,
}

fn pipelines() -> Pipelines {
    let generator = MockGenerator::new(
        MockConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        Vec::<String>::new(),
    );
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: vec![inference_core::model::Locality::SameProcess],
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    let adapter_id = generator.adapter().adapter_id();
    let catalog = Fixture {
        profile,
        adapter: generator.adapter().clone(),
    };
    let mut declaration = Declaration::new();
    let (flat_id, ()) = declaration
        .build(&catalog, profile_id, spec("flat"), |pipeline| {
            let node = pipeline.node("model", NodeRole::Model, vec![Capability::OutputStreaming]);
            let prompt = pipeline.input::<modality::Text>(&node, "prompt", TEXT);
            let answer = pipeline.output::<modality::Text>(&node, "answer", TEXT);
            pipeline.expose(&prompt, "user text", one(), OverloadPolicy::Pause);
            pipeline.expose(&answer, "reply", one(), OverloadPolicy::Pause);
            pipeline.bind(&node, adapter_id).expect("known");
        })
        .expect("known");
    let flat = plan(&mut declaration, &catalog, flat_id);
    let (cycle_id, ()) = declaration
        .build(&catalog, profile_id, spec("cycle"), |pipeline| {
            let relay = pipeline.node("relay", NodeRole::Transform, Vec::new());
            let user = pipeline.input::<modality::Text>(&relay, "user text", TEXT);
            let back = pipeline.input::<modality::Text>(&relay, "back", TEXT);
            let out = pipeline.output::<modality::Text>(&relay, "out", TEXT);
            let subgraph = pipeline
                .subgraph("level", NodeRole::Transform, flat_id)
                .expect("the pipeline below is Planned");
            let into = pipeline.input::<modality::Text>(&subgraph, "user text", TEXT);
            let reply = pipeline.output::<modality::Text>(&subgraph, "reply", TEXT);
            pipeline.expose(&user, "user text", one(), OverloadPolicy::Pause);
            pipeline.connect(&out, &into, one(), OverloadPolicy::Pause);
            pipeline.connect_feedback(&reply, &back, one(), OverloadPolicy::Pause, one());
        })
        .expect("known");
    let cycle = plan(&mut declaration, &catalog, cycle_id);
    Pipelines {
        declaration: Arc::new(declaration),
        flat,
        cycle,
    }
}

struct Caller {
    run_id: RunId,
    producer: NodeId,
    stream_id: StreamId,
    sequence: i64,
}

impl Caller {
    fn envelope(&mut self, kind: &str, payload: PipelinePayload) -> EventEnvelope<PipelinePayload> {
        self.sequence += 1;
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: self.run_id,
            session_id: None,
            stream_id: self.stream_id,
            sequence: self.sequence,
            producer: self.producer,
            port: "user text".to_owned(),
            kind: kind.to_owned(),
            generation_id: None,
            timestamp: 0,
            timebase: Timebase::WallTime,
            causation_id: None,
            payload,
        }
    }

    fn text(&mut self, content: &str) -> EventEnvelope<PipelinePayload> {
        let event = StreamEvent::TranscriptCommitted(TranscriptCommitted {
            span_id: SpanId::generate(),
            text: content.to_owned(),
        });
        let kind = event.name().to_owned();
        self.envelope(&kind, PipelinePayload::Stream(event))
    }

    fn stop(&mut self) -> EventEnvelope<PipelinePayload> {
        self.envelope(
            ControlKind::Stop.as_str(),
            PipelinePayload::Control(ControlKind::Stop),
        )
    }
}

fn label(envelope: &EventEnvelope<PipelinePayload>) -> String {
    match &envelope.payload {
        PipelinePayload::Stream(StreamEvent::TranscriptCommitted(committed)) => {
            committed.text.clone()
        }
        PipelinePayload::Control(ControlKind::Stop) => STOP.to_owned(),
        other => format!("{other:?}"),
    }
}

fn outcome(sent: &Result<(), SendError<PipelinePayload>>) -> String {
    match sent {
        Ok(()) => "Ok".to_owned(),
        Err(SendError::Closed(_)) => "Closed".to_owned(),
        Err(other) => format!("{other:?}"),
    }
}

fn take(input: &mut Input) -> String {
    let envelope = block_on(input.recv()).expect("the edge holds an envelope or one arrives");
    label(&envelope)
}

fn answer(answers: &mpsc::Receiver<String>) -> String {
    answers
        .recv_timeout(PATIENCE)
        .expect("the relay's send answered")
}

/// The relay's `stop` waits for room on its full edge into the subgraph node when the run
/// drains, the edge holding `three` and the bridge waiting to forward `two` into the child. On
/// `85bf603` the bridge takes `three` at the drain, the relay's `stop` lands in that room, the
/// bridge receives it and the run completes. Here the bridge closes the edge to its senders at
/// that read, the relay's `stop` is refused as closed, and the relay fails the run with it.
#[test]
fn a_relay_on_a_cycle_through_a_subgraph_node_whose_stop_waits_at_the_drain_does_not_fail_the_run()
{
    let pipelines = pipelines();
    let run_id = RunId::generate();
    let (inputs, handed) = mpsc::channel();
    let (polled_tx, polled) = mpsc::channel();
    let (answers_tx, answers) = mpsc::channel();
    let released = Arc::new(Slot::default());
    let declaration = Arc::clone(&pipelines.declaration);
    let mut controller = RunController::create_as(
        run_id,
        &pipelines.cycle,
        &pipelines.declaration,
        HashMap::<GenerationId, i64>::new(),
        |node: &Node| {
            if node.name() == "relay" {
                return Box::new(Relay {
                    declaration: Arc::clone(&declaration),
                    answers: answers_tx.clone(),
                    ended: Arc::new(Slot::default()),
                    back: Vec::new(),
                }) as Box<dyn NodeImplementation<PipelinePayload>>;
            }
            let inputs = inputs.clone();
            let released = Arc::clone(&released);
            Box::new(DrainPolled {
                inner: Box::new(SubgraphNode::new(
                    run_id,
                    &pipelines.declaration,
                    &pipelines.flat,
                    HashMap::<GenerationId, i64>::new(),
                    Box::new(move |_: RunId, _: &Node| {
                        Box::new(Hand {
                            input: inputs.clone(),
                            released: Arc::clone(&released),
                            outputs: Vec::new(),
                        }) as Box<dyn NodeImplementation<PipelinePayload>>
                    }),
                )),
                polled: polled_tx.clone(),
            }) as Box<dyn NodeImplementation<PipelinePayload>>
        },
    )
    .expect("planned");
    drop(inputs);
    controller.validate().expect("Created");
    block_on(controller.prepare()).expect("prepares");
    let mut ends = controller.exposed();
    let ingress = ends.ingress("user text").expect("the ingress");
    block_on(controller.activate()).expect("activates");
    let mut child_input = handed
        .recv_timeout(PATIENCE)
        .expect("the child's node handed over its input");
    let requests = controller.requests();
    let mut caller = Caller {
        run_id,
        producer: NodeId::generate(),
        stream_id: StreamId::generate(),
        sequence: 0,
    };

    for text in ["one", "two", "three"] {
        block_on(ingress.send(caller.text(text))).expect("the ingress takes it");
        assert_eq!(
            answer(&answers),
            format!("{text}: Ok"),
            "the relay's edge into the subgraph node takes {text}"
        );
    }
    block_on(ingress.send(caller.stop())).expect("the ingress takes the end of input");
    let drive = thread::spawn(move || {
        let answered = block_on(controller.drive()).map_err(|error| error.to_string());
        (answered, controller.run().state())
    });
    requests.drain();
    polled
        .recv_timeout(PATIENCE)
        .expect("the run drains its subgraph node");

    let mut received = vec![take(&mut child_input)];
    let relay_stop = answer(&answers);
    while received.last().map(String::as_str) != Some(STOP) {
        received.push(take(&mut child_input));
    }
    drop(child_input);
    released.set(());
    let (answered, state) = drive.join().expect("the run loop does not panic");

    eprintln!(
        "relay stop: {relay_stop}; child received {received:?}; drive answered {answered:?}; run \
         {state:?}"
    );
    assert_eq!(received, ["one", "two", "three", STOP]);
    assert_eq!(
        (relay_stop.as_str(), state),
        ("stop: Ok", RunState::Completed),
        "the relay's end of input, waiting for room at the drain, is taken and the run \
         completes; drive answered {answered:?}"
    );
}
