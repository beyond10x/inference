//! A send into a run's ingress, admitted and waiting for room under `pause`, held by its caller
//! the way a caller holds one: polled under a timeout that fires while it waits, then the run's
//! drain asked for and awaited, then the send awaited. The drain returns, and the send answers
//! `Ok` with its envelope received by the node that reads the ingress, or
//! [`SendError::Closed`] with its own envelope and nothing of it received
//! (`story:run-ingress-admitted-send-outlives-the-drain`).
//!
//! The drain must not wait for the caller to poll the send it holds: the caller awaits the drain
//! first. Every await of the caller is bounded by [`PATIENCE`], so a drain that waits for the
//! send fails the case instead of hanging it.
//!
//! The cases are the ingress of a run whose node is a subgraph node, read by that node's ingress
//! bridge, and the ingress of a run whose plain node reads it in its drain for as long as the
//! endpoint says the edge holds an envelope ([`InputEndpoint::known_contents`]).

use std::collections::HashMap;
use std::future::{self, Future};
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use inference_core::endpoint::{InputEndpoint, KnownContents, OutputEndpoint, SendError};
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

/// How long the caller waits for the drain, or for the send once the drain returned, before
/// the case counts as stalled.
const PATIENCE: Duration = Duration::from_secs(10);

/// The timeout the caller first polls the waiting send under: it fires while the send waits.
const FIRST_TRY: Duration = Duration::from_millis(50);

/// The label of the caller's end of input among what a node received.
const STOP: &str = "stop";

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Polls `future` on this thread until it completes or `timeout` passed: `None` once the
/// timeout fired with the future still pending, which the caller keeps.
fn block_on_timeout<F: Future>(future: F, timeout: Duration) -> Option<F::Output> {
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let deadline = Instant::now() + timeout;
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
            return Some(output);
        }
        let now = Instant::now();
        if now >= deadline {
            return None;
        }
        thread::park_timeout(deadline - now);
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    block_on_timeout(future, PATIENCE).expect("completes within the patience")
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether a node's reading ended, and the task waiting for it.
#[derive(Default)]
struct Done(Mutex<(bool, Option<Waker>)>);

impl Done {
    fn set(&self) {
        let waker = {
            let mut state = lock(&self.0);
            state.0 = true;
            state.1.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    fn poll_done(&self, cx: &Context<'_>) -> Poll<()> {
        let mut state = lock(&self.0);
        if state.0 {
            return Poll::Ready(());
        }
        state.1 = Some(cx.waker().clone());
        Poll::Pending
    }
}

type Input = Box<dyn InputEndpoint<PipelinePayload>>;

/// What a node received, as labels.
type Received = Arc<Mutex<Vec<String>>>;

/// How a [`Reader`] reads its input.
#[derive(Clone)]
enum Reading {
    /// From a thread of its own, once `go` tells it to, until the end of input or the edge's
    /// close; its drain waits for that thread.
    UntilTheEnd(Arc<Mutex<mpsc::Receiver<()>>>),
    /// In its drain, while the endpoint says the edge holds an envelope.
    WhileKnownHolding,
}

/// A node that reads its one input as [`Reading`] says, records what it received, holds its
/// outputs until it stops, and drops its input once it stopped reading.
struct Reader {
    reading: Reading,
    received: Received,
    done: Arc<Done>,
    input: Option<Input>,
    outputs: Vec<NodeOutput<PipelinePayload>>,
}

impl Reader {
    fn boxed(
        reading: Reading,
        received: &Received,
    ) -> Box<dyn NodeImplementation<PipelinePayload>> {
        Box::new(Self {
            reading,
            received: Arc::clone(received),
            done: Arc::new(Done::default()),
            input: None,
            outputs: Vec::new(),
        })
    }
}

impl NodeImplementation<PipelinePayload> for Reader {
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
        self.outputs = endpoints.outputs;
        let mut input = endpoints
            .inputs
            .into_iter()
            .next()
            .map(|input| input.endpoint);
        if let Reading::UntilTheEnd(go) = &self.reading
            && let Some(mut input) = input.take()
        {
            let go = Arc::clone(go);
            let received = Arc::clone(&self.received);
            let done = Arc::clone(&self.done);
            thread::spawn(move || {
                let _told_or_gave_up = lock(&go).recv_timeout(PATIENCE);
                while let Some(Some(envelope)) = block_on_timeout(input.recv(), PATIENCE) {
                    let label = label(&envelope);
                    let end = label == STOP;
                    lock(&received).push(label);
                    if end {
                        break;
                    }
                }
                drop(input);
                done.set();
            });
        }
        self.input = input;
        Box::pin(future::ready(Ok(())))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        let done = Arc::clone(&self.done);
        let received = Arc::clone(&self.received);
        let input = self.input.take();
        Box::pin(async move {
            if let Some(mut input) = input {
                while input.known_contents() != KnownContents::Empty {
                    let Some(envelope) = input.recv().await else {
                        break;
                    };
                    lock(&received).push(label(&envelope));
                }
                drop(input);
                done.set();
            }
            future::poll_fn(|cx| done.poll_done(cx)).await;
            Ok(())
        })
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        self.input = None;
        self.outputs.clear();
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

/// The pipeline of one node between the ingress `user text` and the egress `reply`, and the
/// pipeline of a subgraph node over it exposing its ports under the same names; every exposure
/// holds one envelope under `pause`.
struct Pipelines {
    declaration: Arc<Declaration>,
    flat: PlannedPipeline,
    over: PlannedPipeline,
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
    let (over_id, ()) = declaration
        .build(&catalog, profile_id, spec("over"), |pipeline| {
            let subgraph = pipeline
                .subgraph("level", NodeRole::Transform, flat_id)
                .expect("the pipeline below is Planned");
            let into = pipeline.input::<modality::Text>(&subgraph, "user text", TEXT);
            let out = pipeline.output::<modality::Text>(&subgraph, "reply", TEXT);
            pipeline.expose(&into, "user text", one(), OverloadPolicy::Pause);
            pipeline.expose(&out, "reply", one(), OverloadPolicy::Pause);
        })
        .expect("known");
    let over = plan(&mut declaration, &catalog, over_id);
    Pipelines {
        declaration: Arc::new(declaration),
        flat,
        over,
    }
}

fn plan(declaration: &mut Declaration, catalog: &Fixture, pipeline: PipelineId) -> PlannedPipeline {
    let validated = declaration.validate(pipeline).expect("valid");
    declaration.plan(validated, catalog).expect("planned")
}

/// The caller of a run, sending on one stream of the ingress.
struct Caller {
    run_id: RunId,
    producer: NodeId,
    stream_id: StreamId,
    sequence: i64,
}

impl Caller {
    fn new(run_id: RunId) -> Self {
        Self {
            run_id,
            producer: NodeId::generate(),
            stream_id: StreamId::generate(),
            sequence: 0,
        }
    }

    fn text(&mut self, content: &str) -> EventEnvelope<PipelinePayload> {
        self.sequence += 1;
        let event = StreamEvent::TranscriptCommitted(TranscriptCommitted {
            span_id: SpanId::generate(),
            text: content.to_owned(),
        });
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: self.run_id,
            session_id: None,
            stream_id: self.stream_id,
            sequence: self.sequence,
            producer: self.producer,
            port: "user text".to_owned(),
            kind: event.name().to_owned(),
            generation_id: None,
            timestamp: 0,
            timebase: Timebase::WallTime,
            causation_id: None,
            payload: PipelinePayload::Stream(event),
        }
    }
}

/// The text a node received, or [`STOP`] for the end of input.
fn label(envelope: &EventEnvelope<PipelinePayload>) -> String {
    match &envelope.payload {
        PipelinePayload::Stream(StreamEvent::TranscriptCommitted(committed)) => {
            committed.text.clone()
        }
        PipelinePayload::Control(ControlKind::Stop) => STOP.to_owned(),
        other => format!("{other:?}"),
    }
}

type Sent = Option<Result<(), SendError<PipelinePayload>>>;

fn outcome(sent: &Sent) -> String {
    match sent {
        Some(Ok(())) => "Ok".to_owned(),
        Some(Err(SendError::Closed(envelope))) => format!("Closed({})", label(envelope)),
        Some(Err(other)) => format!("{other:?}"),
        None => "still waiting".to_owned(),
    }
}

/// The send of `waiting` answered `Ok` with its envelope received, or `Closed` with its own
/// envelope and nothing of it received.
fn delivered_or_refused(sent: &Sent, received: &[String]) -> bool {
    let delivered = received.iter().any(|label| label == "waiting");
    match sent {
        Some(Ok(())) => delivered,
        Some(Err(SendError::Closed(envelope))) => !delivered && label(envelope) == "waiting",
        Some(Err(_)) | None => false,
    }
}

/// How a run's drain ended for its caller: whether the run loop answered `Ok`, and the run's
/// state, or `None` when it had not returned within [`PATIENCE`].
type Drained = Option<(bool, RunState)>;

/// The caller's side of the held send, once the run is active and its ingress is full: the send
/// of `waiting` polled under [`FIRST_TRY`], which fires while it waits; the drain asked for and
/// awaited; then the send awaited. `go` runs once the drain was asked for.
fn hold_send_then_drain(
    mut controller: RunController<PipelinePayload>,
    ingress: &dyn OutputEndpoint<PipelinePayload>,
    caller: &mut Caller,
    go: impl FnOnce(),
) -> (Sent, Drained) {
    let requests = controller.requests();
    let (ended, end) = mpsc::channel();
    let drive = thread::spawn(move || {
        let answered = block_on_timeout(controller.drive(), PATIENCE * 2).map(|run| run.is_ok());
        let _caller_waits_or_gone = ended.send(answered.map(|ok| (ok, controller.run().state())));
    });
    let mut waiting = ingress.send(caller.text("waiting"));
    assert!(
        block_on_timeout(waiting.as_mut(), FIRST_TRY).is_none(),
        "the ingress is full, so the send admitted on its first poll waits for room past the \
         caller's timeout"
    );
    requests.drain();
    go();
    let drained = end.recv_timeout(PATIENCE).ok().flatten();
    let sent = block_on_timeout(waiting.as_mut(), PATIENCE);
    drop(waiting);
    if drained.is_some() {
        drive.join().expect("the run loop does not panic");
    }
    (sent, drained)
}

fn report(case: &str, sent: &Sent, drained: &Drained, received: &[String]) {
    let drain = match drained {
        Some((answered, state)) => format!("returned (ok: {answered}, {state:?})"),
        None => format!("timed out after {PATIENCE:?}"),
    };
    eprintln!(
        "{case}: drain {drain}; held send {}; node received {received:?}",
        outcome(sent)
    );
}

#[test]
fn a_held_send_into_a_subgraph_node_lets_the_drain_return_and_is_delivered_or_refused() {
    let pipelines = pipelines();
    let run_id = RunId::generate();
    let received = Received::default();
    let (go, gone) = mpsc::channel();
    let gone = Arc::new(Mutex::new(gone));
    let mut controller = RunController::create_as(
        run_id,
        &pipelines.over,
        &pipelines.declaration,
        HashMap::<GenerationId, i64>::new(),
        |_: &Node| {
            let reading = Reading::UntilTheEnd(Arc::clone(&gone));
            let received = Arc::clone(&received);
            Box::new(SubgraphNode::new(
                run_id,
                &pipelines.declaration,
                &pipelines.flat,
                HashMap::<GenerationId, i64>::new(),
                Box::new(move |_: RunId, _: &Node| Reader::boxed(reading.clone(), &received)),
            )) as Box<dyn NodeImplementation<PipelinePayload>>
        },
    )
    .expect("planned");
    controller.validate().expect("Created");
    block_on(controller.prepare()).expect("prepares");
    let mut ends = controller.exposed();
    let ingress = ends.ingress("user text").expect("the ingress");
    let _reply = ends.egress("reply").expect("the egress");
    block_on(controller.activate()).expect("activates");
    let mut caller = Caller::new(run_id);
    for text in ["one", "two", "three"] {
        block_on(ingress.send(caller.text(text))).expect("the ingress takes it");
    }

    let (sent, drained) = hold_send_then_drain(controller, ingress.as_ref(), &mut caller, || {
        go.send(()).expect("the child's node waits to read");
    });
    let received = lock(&received).clone();
    report("subgraph node", &sent, &drained, &received);

    assert!(
        matches!(drained, Some((true, RunState::Completed))),
        "the run's drain returns while its caller holds the send unpolled, and the run completes"
    );
    assert!(
        delivered_or_refused(&sent, &received),
        "the held send answered {} and the child's node received {received:?}",
        outcome(&sent)
    );
}

#[test]
fn a_held_send_into_a_plain_node_lets_the_drain_return_and_is_delivered_or_refused() {
    let pipelines = pipelines();
    let run_id = RunId::generate();
    let received = Received::default();
    let mut controller = RunController::create_as(
        run_id,
        &pipelines.flat,
        &pipelines.declaration,
        HashMap::<GenerationId, i64>::new(),
        |_: &Node| Reader::boxed(Reading::WhileKnownHolding, &received),
    )
    .expect("planned");
    controller.validate().expect("Created");
    block_on(controller.prepare()).expect("prepares");
    let mut ends = controller.exposed();
    let ingress = ends.ingress("user text").expect("the ingress");
    let _reply = ends.egress("reply").expect("the egress");
    block_on(controller.activate()).expect("activates");
    let mut caller = Caller::new(run_id);
    block_on(ingress.send(caller.text("first"))).expect("the empty ingress takes it");

    let (sent, drained) = hold_send_then_drain(controller, ingress.as_ref(), &mut caller, || {});
    let received = lock(&received).clone();
    report("plain node", &sent, &drained, &received);

    assert!(
        matches!(drained, Some((true, RunState::Completed))),
        "the run's drain returns while its caller holds the send unpolled, and the run completes"
    );
    assert!(
        delivered_or_refused(&sent, &received),
        "the held send answered {} and the node received {received:?}",
        outcome(&sent)
    );
}
