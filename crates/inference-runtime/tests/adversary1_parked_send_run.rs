//! Adversary cases on a run's ingress around its drain and around a send its caller gives up on
//! (`story:run-ingress-admitted-send-outlives-the-drain`).
//!
//! The race case widens the window between the ingress admitting a send and the send reaching
//! its queue: the parent's ingress is declared `drop_stale_epochs`, and the epoch lookup the run
//! is built with, which the queue reads before it takes the envelope, holds that one send until
//! the test lets it go. Where another thread moves, a case waits on a waker that thread calls or
//! on a channel it sends on, bounded by [`PATIENCE`].

use std::collections::HashMap;
use std::future::{self, Future};
use std::num::NonZeroUsize;
use std::pin::{Pin, pin};
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
use inference_runtime::{EpochLookup, RunController, SubgraphNode};
use inference_testkit::graph::PipelinePayload;
use inference_testkit::llm::{MockConfiguration, MockGenerator};
use inference_testkit::stream::StreamEvent;
use inference_text::{SpanId, TranscriptCommitted};

const PATIENCE: Duration = Duration::from_secs(20);

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
        thread::park_timeout(Duration::from_millis(50));
    }
}

fn poll<F: Future + ?Sized>(future: Pin<&mut F>, waker: &Waker) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(waker))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A waker that, the first time it is woken, tells the test and holds the waking thread until
/// the test lets it go, or [`PATIENCE`] passed.
struct Hold {
    woken: Mutex<Option<mpsc::Sender<()>>>,
    go: Mutex<mpsc::Receiver<()>>,
}

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

/// The epoch lookup of the parent run: the first lookup of `generation` tells the test and holds
/// the looking thread until the test lets it go; every generation it knows is of epoch 1.
struct Gate {
    generation: GenerationId,
    entered: Mutex<Option<mpsc::Sender<()>>>,
    go: Mutex<mpsc::Receiver<()>>,
}

impl EpochLookup for Gate {
    fn epoch(&self, generation_id: GenerationId) -> Option<i64> {
        if generation_id != self.generation {
            return None;
        }
        if let Some(entered) = lock(&self.entered).take() {
            let _test_waits_or_gone = entered.send(());
            let _let_go_or_gave_up = lock(&self.go).recv_timeout(PATIENCE);
        }
        Some(1)
    }
}

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

/// A node that hands the test the receiving end of its input when it starts and drains once the
/// test released it.
struct Hand {
    input: mpsc::Sender<Input>,
    released: Arc<Released>,
    outputs: Vec<NodeOutput<PipelinePayload>>,
}

impl Hand {
    fn boxed(
        input: &mpsc::Sender<Input>,
        released: &Arc<Released>,
    ) -> Box<dyn NodeImplementation<PipelinePayload>> {
        Box::new(Self {
            input: input.clone(),
            released: Arc::clone(released),
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

/// A node whose drain tells the test once it was first polled.
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

struct Pipelines {
    declaration: Arc<Declaration>,
    flat: PlannedPipeline,
    over: PlannedPipeline,
}

/// One node between the ingress `user text` and the egress `reply`, every exposure holding one
/// under `pause`, and a subgraph node over it whose own ingress holds one under `ingress`.
fn pipelines(ingress: OverloadPolicy) -> Pipelines {
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
            pipeline.expose(&into, "user text", one(), ingress);
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

    fn text_in(
        &mut self,
        content: &str,
        generation_id: Option<GenerationId>,
    ) -> EventEnvelope<PipelinePayload> {
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
            generation_id,
            timestamp: 0,
            timebase: Timebase::WallTime,
            causation_id: None,
            payload: PipelinePayload::Stream(event),
        }
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

fn take(input: &mut Input) -> String {
    let envelope = block_on(input.recv()).expect("the edge holds an envelope or one arrives");
    label(&envelope)
}

fn take_to_the_end(input: &mut Input, received: &mut Vec<String>) {
    while received.last().map(String::as_str) != Some(STOP) {
        received.push(take(input));
    }
}

fn outcome(sent: &Result<(), SendError<PipelinePayload>>) -> String {
    match sent {
        Ok(()) => "Ok".to_owned(),
        Err(SendError::Closed(_)) => "Closed".to_owned(),
        Err(other) => format!("{other:?}"),
    }
}

/// A send the parent's ingress admitted before the drain, still on its way into the ingress's
/// queue when the subgraph node's bridge finds that queue empty and sends `stop` into the child,
/// reaches the queue while the bridge still holds it: it must be delivered to the child or
/// refused as closed, never answered `Ok` and dropped with the bridge's end.
#[test]
fn a_send_admitted_before_the_drain_and_reaching_the_queue_after_the_bridge_found_it_empty_is_delivered_or_refused()
 {
    let generation = GenerationId::generate();
    let (entered_tx, entered) = mpsc::channel();
    let (go, held) = mpsc::channel();
    let gate = Gate {
        generation,
        entered: Mutex::new(Some(entered_tx)),
        go: Mutex::new(held),
    };
    let pipelines = pipelines(OverloadPolicy::DropStaleEpochs);
    let run_id = RunId::generate();
    let (inputs, handed) = mpsc::channel();
    let (polled_tx, polled) = mpsc::channel();
    let released = Arc::new(Released::default());
    let mut controller = RunController::create_as(
        run_id,
        &pipelines.over,
        &pipelines.declaration,
        gate,
        |_: &Node| {
            let inputs = inputs.clone();
            let released = Arc::clone(&released);
            Box::new(DrainPolled {
                inner: Box::new(SubgraphNode::new(
                    run_id,
                    &pipelines.declaration,
                    &pipelines.flat,
                    HashMap::<GenerationId, i64>::new(),
                    Box::new(move |_: RunId, _: &Node| Hand::boxed(&inputs, &released)),
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
    let _reply = ends.egress("reply").expect("the egress");
    block_on(controller.activate()).expect("activates");
    let mut child_input = handed
        .recv_timeout(PATIENCE)
        .expect("the child's node handed over its input");
    let requests = controller.requests();
    let mut caller = Caller::new(run_id);

    let late = caller.text_in("late", Some(generation));
    let sending = thread::spawn(move || {
        let sent = block_on(ingress.send(late));
        (sent, ingress)
    });
    entered
        .recv_timeout(PATIENCE)
        .expect("the ingress admitted the send, which reads its epoch before the queue takes it");
    let (pushed_waker, pushed, pushed_go) = hold();
    assert!(
        poll(child_input.recv().as_mut(), &pushed_waker).is_pending(),
        "nothing crossed into the child yet"
    );
    let drive = thread::spawn(move || {
        let answered = block_on(controller.drive()).is_ok();
        (answered, controller.run().state())
    });
    requests.drain();
    let parent_waited = polled.recv_timeout(Duration::from_secs(5)).is_err();
    if parent_waited {
        go.send(()).expect("the send is held");
        polled
            .recv_timeout(PATIENCE)
            .expect("the run drains its subgraph node");
    }
    pushed
        .recv_timeout(PATIENCE)
        .expect("the bridge sends into the child's ingress, which holds the bridge there");
    if !parent_waited {
        go.send(()).expect("the send is held");
    }
    let (sent, _ingress) = sending.join().expect("the sending thread does not panic");
    pushed_go.send(()).expect("the bridge is held");
    let mut received = Vec::new();
    take_to_the_end(&mut child_input, &mut received);
    drop(child_input);
    released.release();
    let (answered, state) = drive.join().expect("the run loop does not panic");

    assert!(
        answered && state == RunState::Completed,
        "the run completes"
    );
    let delivered = received.iter().any(|label| label == "late");
    assert!(
        match &sent {
            Ok(()) => delivered,
            Err(SendError::Closed(envelope)) => !delivered && label(envelope) == "late",
            Err(_) => false,
        },
        "the send admitted before the drain answered {} (the parent waited for it: \
         {parent_waited}) and the child's node received {received:?}",
        outcome(&sent)
    );
}

/// A send its caller dropped while it waited for room, its envelope taken back, sends nothing:
/// the edge never took that sequence, so the caller can send the same envelope again.
#[test]
fn an_ingress_send_dropped_while_it_waited_can_be_sent_again() {
    let pipelines = pipelines(OverloadPolicy::Pause);
    let run_id = RunId::generate();
    let (inputs, handed) = mpsc::channel();
    let released = Arc::new(Released::default());
    let mut controller = RunController::create_as(
        run_id,
        &pipelines.flat,
        &pipelines.declaration,
        HashMap::<GenerationId, i64>::new(),
        |_: &Node| Hand::boxed(&inputs, &released),
    )
    .expect("planned");
    controller.validate().expect("Created");
    block_on(controller.prepare()).expect("prepares");
    let mut ends = controller.exposed();
    let ingress = ends.ingress("user text").expect("the ingress");
    let _reply = ends.egress("reply").expect("the egress");
    block_on(controller.activate()).expect("activates");
    let mut input = handed.try_recv().expect("the node handed over its input");
    let mut caller = Caller::new(run_id);
    let noop = Waker::noop();

    assert!(matches!(
        poll(ingress.send(caller.text_in("first", None)).as_mut(), noop),
        Poll::Ready(Ok(()))
    ));
    let second = caller.text_in("second", None);
    {
        let mut waiting = ingress.send(second.clone());
        assert!(
            poll(waiting.as_mut(), noop).is_pending(),
            "the ingress is full, so the send waits"
        );
    }
    assert_eq!(take(&mut input), "first");
    assert!(
        input.known_contents() == inference_core::endpoint::KnownContents::Empty,
        "the dropped send took its envelope back"
    );
    let retried = poll(ingress.send(second).as_mut(), noop);
    assert!(
        matches!(retried, Poll::Ready(Ok(()))),
        "the edge never took the dropped send's envelope, and the same envelope sent again \
         answered {retried:?}"
    );
    assert_eq!(take(&mut input), "second");
}
