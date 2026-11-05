//! An ingress send the run admitted before its drain, still waiting for room under `pause` when
//! the drain runs, is delivered or refused as closed: its envelope reaches the node that reads the
//! ingress, or the send answers [`SendError::Closed`], which says it was not delivered. It never
//! answers `Ok` for an envelope no node receives, and the run never ends with it still waiting
//! (`story:run-ingress-admitted-send-outlives-the-drain`).
//!
//! `inference.graph.DrainRun` admits no new input; in-flight events complete. The sender of an
//! ingress reads whether the run admits input when a send is first polled, so a send first polled
//! before the drain was asked for is admitted, and under `pause` it then waits for room while the
//! run drains.
//!
//! Every exposure holds one envelope, so the caller fills the ingress and its next send waits. The
//! node that reads the ingress hands the test the receiving end of its input when it starts and
//! drains once the test released it, so the test reads in the node's place and orders every take
//! itself. Where another thread moves, a case waits on a waker that thread calls, never on time:
//! a [`Hold`] tells the test it was woken and holds the waking thread until the test lets it go.
//!
//! The cases are the run's own ingress, read by its node, and the ingress of a run whose node is a
//! subgraph node, read by that node's ingress bridge, which forwards it into the child run's
//! ingress and ends once its parent edges are empty at the drain (`subgraph` module). The defect
//! the story measured is the subgraph case whose waiting send lands while the bridge sends `stop`:
//! the send answered `Ok` and its envelope was discarded with the parent edge, the child having
//! received one, two, three and `stop`.

use std::collections::HashMap;
use std::future::{self, Future};
use std::num::NonZeroUsize;
use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use inference_core::endpoint::{InputEndpoint, OutputEndpoint, SendError};
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

/// How long a case waits for another thread before it counts as hung.
const PATIENCE: Duration = Duration::from_secs(20);

/// The label of the caller's end of input among what a node received.
const STOP: &str = "stop";

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

fn poll<F: Future + ?Sized>(future: Pin<&mut F>, waker: &Waker) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(waker))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A waker that records that it was woken.
#[derive(Default)]
struct Flag(AtomicBool);

impl Wake for Flag {
    fn wake(self: Arc<Self>) {
        self.0.store(true, Ordering::Release);
    }
}

/// A waker that, the first time it is woken, tells the test and holds the waking thread until
/// the test lets it go, or [`PATIENCE`] passed.
struct Hold {
    woken: Mutex<Option<mpsc::Sender<()>>>,
    go: Mutex<mpsc::Receiver<()>>,
}

/// A [`Hold`], what it tells the test when it is woken, and how the test lets the thread go.
fn hold() -> (Waker, mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (woken, told) = mpsc::channel();
    let (go, held) = mpsc::channel();
    let waker = Waker::from(Arc::new(Hold {
        woken: Mutex::new(Some(woken)),
        go: Mutex::new(held),
    }));
    (waker, told, go)
}

impl Wake for Hold {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        let Some(woken) = lock(&self.woken).take() else {
            return;
        };
        let _test_waits_or_gone = woken.send(());
        let _let_go_or_gave_up = lock(&self.go).recv_timeout(PATIENCE);
    }
}

/// Whether the node's drain may return, and the task waiting for it.
#[derive(Default)]
struct Released(Mutex<(bool, Option<Waker>)>);

impl Released {
    fn release(&self) {
        let waker = {
            let mut state = lock(&self.0);
            state.0 = true;
            state.1.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    fn poll_released(&self, cx: &Context<'_>) -> Poll<()> {
        let mut state = lock(&self.0);
        if state.0 {
            return Poll::Ready(());
        }
        state.1 = Some(cx.waker().clone());
        Poll::Pending
    }
}

type Input = Box<dyn InputEndpoint<PipelinePayload>>;

/// A node that hands the test the receiving end of its input when it starts, holds its outputs
/// until it stops, and drains once the test released it, telling the test when its drain is
/// asked for.
struct Hand {
    input: mpsc::Sender<Input>,
    released: Arc<Released>,
    draining: Option<mpsc::Sender<()>>,
    outputs: Vec<NodeOutput<PipelinePayload>>,
}

impl Hand {
    fn boxed(
        input: &mpsc::Sender<Input>,
        released: &Arc<Released>,
        draining: Option<&mpsc::Sender<()>>,
    ) -> Box<dyn NodeImplementation<PipelinePayload>> {
        Box::new(Self {
            input: input.clone(),
            released: Arc::clone(released),
            draining: draining.cloned(),
            outputs: Vec::new(),
        })
    }
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
        if let Some(draining) = &self.draining {
            let _test_waits_or_gone = draining.send(());
        }
        let released = Arc::clone(&self.released);
        Box::pin(async move {
            future::poll_fn(|cx| released.poll_released(cx)).await;
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

/// Takes the next envelope of `input`, waiting for it.
fn take(input: &mut Input) -> String {
    let envelope = block_on(input.recv()).expect("the edge holds an envelope or one arrives");
    label(&envelope)
}

/// Takes what `input` holds and what arrives on it until the end of input.
fn take_to_the_end(input: &mut Input, received: &mut Vec<String>) {
    while received.last().map(String::as_str) != Some(STOP) {
        received.push(take(input));
    }
}

type Sent = Poll<Result<(), SendError<PipelinePayload>>>;

/// The send of `waiting` was delivered, its envelope among what the node received, or refused as
/// closed; never taken and not received, nor still waiting.
fn delivered_or_refused(sent: &Sent, received: &[String]) -> bool {
    let delivered = received.iter().any(|label| label == "waiting");
    match sent {
        Poll::Ready(Ok(())) => delivered,
        Poll::Ready(Err(SendError::Closed(envelope))) => !delivered && label(envelope) == "waiting",
        Poll::Ready(Err(_)) | Poll::Pending => false,
    }
}

fn outcome(sent: &Sent) -> String {
    match sent {
        Poll::Ready(Ok(())) => "Ok".to_owned(),
        Poll::Ready(Err(SendError::Closed(_))) => "Closed".to_owned(),
        Poll::Ready(Err(other)) => format!("{other:?}"),
        Poll::Pending => "still waiting".to_owned(),
    }
}

#[test]
fn an_admitted_send_waiting_for_room_at_the_drain_reaches_the_node_that_reads_the_ingress() {
    let pipelines = pipelines();
    let run_id = RunId::generate();
    let (inputs, handed) = mpsc::channel();
    let (draining, drained) = mpsc::channel();
    let released = Arc::new(Released::default());
    let mut controller = RunController::create_as(
        run_id,
        &pipelines.flat,
        &pipelines.declaration,
        HashMap::<GenerationId, i64>::new(),
        |_: &Node| Hand::boxed(&inputs, &released, Some(&draining)),
    )
    .expect("planned");
    controller.validate().expect("Created");
    block_on(controller.prepare()).expect("prepares");
    let mut ends = controller.exposed();
    let ingress = ends.ingress("user text").expect("the ingress");
    let _reply = ends.egress("reply").expect("the egress");
    block_on(controller.activate()).expect("activates");
    let mut input = handed.try_recv().expect("the node handed over its input");
    let requests = controller.requests();
    let mut caller = Caller::new(run_id);
    let noop = Waker::noop();

    assert!(
        matches!(
            poll(ingress.send(caller.text("first")).as_mut(), noop),
            Poll::Ready(Ok(()))
        ),
        "the empty ingress takes the first text"
    );
    let woken = Arc::new(Flag::default());
    let mut waiting = ingress.send(caller.text("waiting"));
    assert!(
        poll(waiting.as_mut(), &Waker::from(Arc::clone(&woken))).is_pending(),
        "the ingress is full, so the send admitted on its first poll waits for room"
    );
    requests.drain();

    let (sent, received) = {
        let mut drive = pin!(controller.drive());
        assert!(
            poll(drive.as_mut(), noop).is_pending(),
            "the run drains its node, which waits to be released"
        );
        drained
            .try_recv()
            .expect("the run is Draining and asked its node for its drain");
        let mut received = vec![take(&mut input)];
        assert!(
            woken.0.load(Ordering::Acquire),
            "the node's take made room, which wakes the waiting send"
        );
        let sent = poll(waiting.as_mut(), noop);
        drop(waiting);
        if let Poll::Ready(Ok(())) = sent {
            received.push(take(&mut input));
        }
        assert!(
            matches!(
                poll(ingress.send(caller.stop()).as_mut(), noop),
                Poll::Ready(Ok(()))
            ),
            "the end of input passes while the run drains"
        );
        take_to_the_end(&mut input, &mut received);
        drop(input);
        released.release();
        assert!(
            matches!(poll(drive.as_mut(), noop), Poll::Ready(Ok(()))),
            "the run loop returns once the node drained"
        );
        (sent, received)
    };

    assert_eq!(controller.run().state(), RunState::Completed);
    assert!(
        matches!(sent, Poll::Ready(Ok(()))) && received == ["first", "waiting", STOP],
        "the send admitted before the drain and waiting for room answered {} and the node \
         received {received:?}",
        outcome(&sent)
    );
}

/// A run whose node is a subgraph node over the flat pipeline, whose node the test reads for;
/// active, its ingress full and the send of `waiting` admitted and waiting for room, and the drain
/// asked for and taken by the subgraph node, whose bridge was told to drain.
struct Nested {
    _reply: Box<dyn InputEndpoint<PipelinePayload>>,
    child_input: Input,
    released: Arc<Released>,
    drive: thread::JoinHandle<(bool, RunState)>,
    polled: mpsc::Receiver<()>,
    requests: inference_runtime::RunRequests,
    caller: Caller,
}

type Ingress = Box<dyn OutputEndpoint<PipelinePayload>>;

/// The run of [`Nested`], active, with the child's ingress holding `one`, the bridge of its
/// subgraph node holding `two` and the parent's ingress, whose caller's end comes with it,
/// holding `three`.
fn nested() -> (Nested, Ingress) {
    let pipelines = pipelines();
    let run_id = RunId::generate();
    let (inputs, handed) = mpsc::channel();
    let (polled_tx, polled) = mpsc::channel();
    let released = Arc::new(Released::default());
    let mut controller = RunController::create_as(
        run_id,
        &pipelines.over,
        &pipelines.declaration,
        HashMap::<GenerationId, i64>::new(),
        |_: &Node| {
            let inputs = inputs.clone();
            let released = Arc::clone(&released);
            Box::new(DrainPolled {
                inner: Box::new(SubgraphNode::new(
                    run_id,
                    &pipelines.declaration,
                    &pipelines.flat,
                    HashMap::<GenerationId, i64>::new(),
                    Box::new(move |_: RunId, _: &Node| Hand::boxed(&inputs, &released, None)),
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
    let reply = ends.egress("reply").expect("the egress");
    block_on(controller.activate()).expect("activates");
    let child_input = handed
        .recv_timeout(PATIENCE)
        .expect("the child's node handed over its input");
    let requests = controller.requests();
    let mut caller = Caller::new(run_id);
    for text in ["one", "two", "three"] {
        block_on(ingress.send(caller.text(text))).expect("the ingress takes it");
    }
    let drive = thread::spawn(move || {
        let answered = block_on(controller.drive()).is_ok();
        (answered, controller.run().state())
    });
    (
        Nested {
            _reply: reply,
            child_input,
            released,
            drive,
            polled,
            requests,
            caller,
        },
        ingress,
    )
}

impl Nested {
    /// Asks for the drain and waits for the subgraph node to take it.
    fn drain(&self) {
        self.requests.drain();
        self.polled
            .recv_timeout(PATIENCE)
            .expect("the run drains its subgraph node");
    }

    /// Releases the child's node and answers how the run ended.
    fn end(self) -> (bool, RunState) {
        drop(self.child_input);
        self.released.release();
        self.drive.join().expect("the run loop does not panic")
    }
}

#[test]
fn an_admitted_send_into_a_subgraph_node_landing_while_its_bridge_sends_stop_is_delivered_or_refused()
 {
    let (mut nested, ingress) = nested();
    let noop = Waker::noop();
    let (waiting_waker, waiting_woken, waiting_go) = hold();
    let waiting_text = nested.caller.text("waiting");
    let mut waiting = ingress.send(waiting_text);
    assert!(
        poll(waiting.as_mut(), &waiting_waker).is_pending(),
        "the ingress is full, so the send admitted on its first poll waits for room"
    );
    nested.drain();

    let mut received = vec![take(&mut nested.child_input)];
    waiting_woken.recv_timeout(PATIENCE).expect(
        "the bridge forwards `two` and, reading the drain, wakes the waiting send, by taking \
         `three` off the parent's ingress or by closing it to its senders, and is held there",
    );
    received.push(take(&mut nested.child_input));
    let (pushed_waker, pushed, pushed_go) = hold();
    assert!(
        poll(nested.child_input.recv().as_mut(), &pushed_waker).is_pending(),
        "the child's ingress is empty until the bridge forwards `three`"
    );
    waiting_go.send(()).expect("the bridge is held");
    pushed
        .recv_timeout(PATIENCE)
        .expect("the bridge forwards `three`, which wakes the child's node and holds the bridge");
    received.push(take(&mut nested.child_input));
    let (next_waker, next, next_go) = hold();
    assert!(
        poll(nested.child_input.recv().as_mut(), &next_waker).is_pending(),
        "the child's ingress is empty until the bridge sends what comes next"
    );
    pushed_go.send(()).expect("the bridge is held");
    next.recv_timeout(PATIENCE).expect(
        "the bridge sends the next envelope into the child's ingress, which holds it there",
    );
    let sent = poll(waiting.as_mut(), noop);
    drop(waiting);
    next_go.send(()).expect("the bridge is held");
    take_to_the_end(&mut nested.child_input, &mut received);
    let (answered, state) = nested.end();

    assert!(
        answered && state == RunState::Completed,
        "the run completes"
    );
    assert!(
        delivered_or_refused(&sent, &received),
        "the send admitted before the drain and waiting for room answered {} once the bridge \
         had sent the child what followed `three`, and the child's node received {received:?}",
        outcome(&sent)
    );
}

#[test]
fn an_admitted_send_into_a_subgraph_node_awaited_once_the_run_ended_is_delivered_or_refused() {
    let (mut nested, ingress) = nested();
    let noop = Waker::noop();
    let woken = Arc::new(Flag::default());
    let waiting_text = nested.caller.text("waiting");
    let mut waiting = ingress.send(waiting_text);
    assert!(
        poll(waiting.as_mut(), &Waker::from(Arc::clone(&woken))).is_pending(),
        "the ingress is full, so the send admitted on its first poll waits for room"
    );
    nested.drain();

    let mut received = Vec::new();
    take_to_the_end(&mut nested.child_input, &mut received);
    let (answered, state) = nested.end();
    let sent = poll(waiting.as_mut(), noop);
    drop(waiting);

    assert!(
        answered && state == RunState::Completed,
        "the run completes"
    );
    assert!(
        delivered_or_refused(&sent, &received),
        "the send admitted before the drain and waiting for room answered {} once the run had \
         ended, and the child's node received {received:?}",
        outcome(&sent)
    );
}
