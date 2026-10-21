//! Adversary pass 1 on `story:subgraph-unconnected-usage-port`: a backlog through the bridge that
//! reads and drops what the child sends out of the egresses of ports with no edge, two sibling
//! subgraph nodes of which one connects its `usage` port, a `usage` port whose edge leads to a
//! receiver that closed, compared with a model node's, a child usage egress read through a cancel,
//! and a child node that keeps its egress end past its stop.
//!
//! Every case runs on a thread of its own and is bounded by [`PATIENCE`], so a bridge that never
//! ends fails the case instead of hanging the suite.

use std::collections::HashMap;
use std::future::{self, Future};
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use inference_core::endpoint::{InputEndpoint, OutputEndpoint};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId, Capability, ModelId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_core::usage::UsageRecorded;
use inference_llm::Generator;
use inference_llm::llm_core::Usage;
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeEndpoints, NodeError,
    NodeFuture, NodeImplementation, NodeRole, OverloadPolicy, PipelineId, PipelineSpec,
    PlannedPipeline, PortId, PortTraits, USAGE_PORT, modality,
};
use inference_runtime::record::Recorder;
use inference_runtime::run::RunState;
use inference_runtime::{ChildImplementations, RunController, SubgraphHandle, SubgraphNode};
use inference_testkit::graph::{PipelinePayload, RunLog};
use inference_testkit::llm::{MockConfiguration, MockGenerator, ModelNode};
use inference_testkit::stream::StreamEvent;
use inference_text::{SpanId, TranscriptCommitted};

const PATIENCE: Duration = Duration::from_secs(20);

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

/// Runs `case` on a thread of its own, failing once it took longer than [`PATIENCE`].
fn bounded(case: impl FnOnce() + Send + 'static) {
    let (done, finished) = mpsc::channel();
    let runner = thread::spawn(move || {
        case();
        let _ = done.send(());
    });
    match finished.recv_timeout(PATIENCE) {
        Ok(()) => runner.join().expect("the case does not panic"),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            if let Err(panic) = runner.join() {
                std::panic::resume_unwind(panic);
            }
        }
        Err(mpsc::RecvTimeoutError::Timeout) => panic!("the case did not end within {PATIENCE:?}"),
    }
}

/// A future ready once `delay` has passed, woken by a thread of its own.
fn after(delay: Duration) -> impl Future<Output = ()> + Send {
    let fired = Arc::new(AtomicBool::new(false));
    let mut spawned = false;
    future::poll_fn(move |cx| {
        if fired.load(Ordering::SeqCst) {
            return Poll::Ready(());
        }
        if !spawned {
            spawned = true;
            let fired = Arc::clone(&fired);
            let waker = cx.waker().clone();
            thread::spawn(move || {
                thread::sleep(delay);
                fired.store(true, Ordering::SeqCst);
                waker.wake();
            });
        }
        Poll::Pending
    })
}

struct Fixture {
    profile: DeploymentProfile,
    adapters: Vec<Adapter>,
}

impl Catalog for Fixture {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.profile.profile_id() == profile_id).then_some(&self.profile)
    }

    fn adapter(&self, adapter_id: AdapterId) -> Option<&Adapter> {
        self.adapters
            .iter()
            .find(|adapter| adapter.adapter_id() == adapter_id)
    }
}

const TEXT: PortTraits = PortTraits {
    essentiality: Essentiality::Essential,
    delivery: Delivery::Reliable,
    incremental: true,
    end_marker_required: true,
    provisional_allowed: false,
};

const RECORDS: PortTraits = PortTraits {
    incremental: false,
    ..TEXT
};

const fn optional(traits: PortTraits) -> PortTraits {
    PortTraits {
        essentiality: Essentiality::Optional,
        ..traits
    }
}

fn spec(name: &str, required_capabilities: Vec<Capability>) -> PipelineSpec {
    PipelineSpec {
        name: name.to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities,
    }
}

fn streaming(name: &str) -> PipelineSpec {
    spec(name, vec![Capability::OutputStreaming])
}

fn capacity() -> NonZeroUsize {
    NonZeroUsize::new(16).expect("16")
}

fn reply(input: &str) -> String {
    format!("s{input}")
}

fn generator() -> MockGenerator {
    MockGenerator::new(
        MockConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        Vec::<String>::new(),
    )
}

fn usage(input: u64) -> Usage {
    Usage {
        input_tokens: Some(input),
        output_tokens: Some(1),
        ..Usage::default()
    }
}

fn catalog(adapters: Vec<Adapter>) -> (Fixture, ProfileId) {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: vec![inference_core::model::Locality::SameProcess],
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    (Fixture { profile, adapters }, profile_id)
}

/// A child pipeline of one model bound to `adapter`: its prompt exposed as `user text`, its
/// answer as `reply` and its `usage` port as `model usage`. Planned; answers it and the model's
/// node and `usage` port.
fn one_model(
    declaration: &mut Declaration,
    catalog: &Fixture,
    profile_id: ProfileId,
    adapter: AdapterId,
) -> (PipelineId, PlannedPipeline, (NodeId, PortId)) {
    let (pipeline, usage_of) = declaration
        .build(catalog, profile_id, streaming("models"), |pipeline| {
            let model = pipeline.node("model", NodeRole::Model, vec![Capability::OutputStreaming]);
            let prompt = pipeline.input::<modality::Text>(&model, "prompt", TEXT);
            let answer = pipeline.output::<modality::Text>(&model, "answer", TEXT);
            let used = pipeline.output::<modality::Structured>(&model, USAGE_PORT, RECORDS);
            pipeline.expose(&prompt, "user text", capacity(), OverloadPolicy::Pause);
            pipeline.expose(&answer, "reply", capacity(), OverloadPolicy::Pause);
            pipeline.expose(&used, "model usage", capacity(), OverloadPolicy::Pause);
            pipeline
                .bind(&model, adapter)
                .expect("the catalog holds it");
            (model.id(), used.id())
        })
        .expect("the profile is declared");
    let validated = declaration.validate(pipeline).expect("valid");
    let planned = declaration.plan(validated, catalog).expect("planned");
    (pipeline, planned, usage_of)
}

/// The model node of a child run, answering on `answering`, wrapped by `recorder`.
fn recorded_model(
    usage_of: (NodeId, PortId),
    answering: Generator,
    recorder: &Recorder<PipelinePayload>,
) -> ChildImplementations<PipelinePayload> {
    let recorder = recorder.clone();
    Box::new(move |run_id: RunId, node: &Node| {
        assert_eq!(node.node_id(), usage_of.0);
        let model =
            ModelNode::new(run_id, answering.clone(), reply, RunLog::new()).with_usage(usage_of.1);
        recorder.wrap(node, Box::new(model))
    })
}

/// The usage records `recorder` took.
fn recorded_usage(recorder: &Recorder<PipelinePayload>) -> Vec<UsageRecorded> {
    recorder
        .recording()
        .usage()
        .into_iter()
        .filter_map(|envelope| match &envelope.payload {
            PipelinePayload::Usage(recorded) => Some(recorded.clone()),
            _ => None,
        })
        .collect()
}

/// A subgraph node of the run `run_id` over `child`, whose nodes `inner` makes; its handle is
/// kept in `handles` under the node's identity.
fn subgraph_node(
    run_id: RunId,
    node: &Node,
    declaration: &Declaration,
    child: &PlannedPipeline,
    inner: ChildImplementations<PipelinePayload>,
    handles: &Arc<Mutex<HashMap<NodeId, SubgraphHandle>>>,
) -> Box<dyn NodeImplementation<PipelinePayload>> {
    let subgraph = SubgraphNode::new(
        run_id,
        declaration,
        child,
        HashMap::<GenerationId, i64>::new(),
        inner,
    );
    handles
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(node.node_id(), subgraph.handle());
    Box::new(subgraph)
}

fn child_of(handles: &Arc<Mutex<HashMap<NodeId, SubgraphHandle>>>, node: NodeId) -> RunId {
    handles.lock().unwrap_or_else(PoisonError::into_inner)[&node]
        .child_run_id()
        .expect("prepared")
}

struct Application {
    run_id: RunId,
    producer: NodeId,
    stream_id: StreamId,
    sequence: i64,
}

impl Application {
    fn new(run_id: RunId) -> Self {
        Self {
            run_id,
            producer: NodeId::generate(),
            stream_id: StreamId::generate(),
            sequence: 0,
        }
    }

    fn envelope(
        &mut self,
        port: &str,
        kind: String,
        payload: PipelinePayload,
    ) -> EventEnvelope<PipelinePayload> {
        self.sequence += 1;
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: self.run_id,
            session_id: None,
            stream_id: self.stream_id,
            sequence: self.sequence,
            producer: self.producer,
            port: port.to_owned(),
            kind,
            generation_id: None,
            timestamp: 0,
            timebase: Timebase::WallTime,
            causation_id: None,
            payload,
        }
    }

    fn text(&mut self, port: &str, text: &str) -> EventEnvelope<PipelinePayload> {
        let event = StreamEvent::TranscriptCommitted(TranscriptCommitted {
            span_id: SpanId::generate(),
            text: text.to_owned(),
        });
        self.envelope(
            port,
            event.name().to_owned(),
            PipelinePayload::Stream(event),
        )
    }

    fn stop(&mut self, port: &str) -> EventEnvelope<PipelinePayload> {
        self.envelope(
            port,
            ControlKind::Stop.as_str().to_owned(),
            PipelinePayload::Control(ControlKind::Stop),
        )
    }
}

type Received = Vec<EventEnvelope<PipelinePayload>>;

/// Receives on `egress` until it ends.
fn drain(egress: &mut dyn InputEndpoint<PipelinePayload>) -> Received {
    let mut received = Vec::new();
    while let Some(envelope) = block_on(egress.recv()) {
        received.push(envelope);
    }
    received
}

fn records(received: &Received) -> Vec<&UsageRecorded> {
    received
        .iter()
        .filter_map(|envelope| match &envelope.payload {
            PipelinePayload::Usage(recorded) => Some(recorded),
            _ => None,
        })
        .collect()
}

/// A run of `planned` made by `implement`, prepared and activated, its loop on a thread.
struct Running {
    run_id: RunId,
    requests: inference_runtime::RunRequests,
    ingresses: HashMap<String, Box<dyn OutputEndpoint<PipelinePayload>>>,
    egresses: HashMap<String, Box<dyn InputEndpoint<PipelinePayload>>>,
    drive: thread::JoinHandle<RunState>,
}

impl Running {
    fn start(
        declaration: &Declaration,
        planned: &PlannedPipeline,
        names: (&[&str], &[&str]),
        mut implement: impl FnMut(RunId, &Node) -> Box<dyn NodeImplementation<PipelinePayload>>,
    ) -> Self {
        let run_id = RunId::generate();
        let mut controller = RunController::create_as(
            run_id,
            planned,
            declaration,
            HashMap::<GenerationId, i64>::new(),
            |node: &Node| implement(run_id, node),
        )
        .expect("planned");
        controller.validate().expect("Created");
        block_on(controller.prepare()).expect("prepares");
        let mut ends = controller.exposed();
        let ingresses = names
            .0
            .iter()
            .map(|name| ((*name).to_owned(), ends.ingress(name).expect("the ingress")))
            .collect();
        let egresses = names
            .1
            .iter()
            .map(|name| ((*name).to_owned(), ends.egress(name).expect("the egress")))
            .collect();
        drop(ends);
        block_on(controller.activate()).expect("activates");
        let requests = controller.requests();
        let drive = thread::spawn(move || {
            let _answered = block_on(controller.drive());
            controller.run().state()
        });
        Self {
            run_id,
            requests,
            ingresses,
            egresses,
            drive,
        }
    }

    fn send(&self, port: &str, envelope: EventEnvelope<PipelinePayload>) {
        block_on(self.ingresses[port].send(envelope)).expect("taken");
    }

    fn egress(&mut self, name: &str) -> &mut dyn InputEndpoint<PipelinePayload> {
        self.egresses.get_mut(name).expect("the egress").as_mut()
    }
}

/// A node that drops every endpoint it is handed as it starts, so that whatever is sent to it
/// along an edge is refused, closed; it answers every step at once.
struct Deaf;

impl NodeImplementation<PipelinePayload> for Deaf {
    fn describe(&mut self, _node: &Node) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn start(
        &mut self,
        endpoints: NodeEndpoints<PipelinePayload>,
    ) -> NodeFuture<'_, Result<(), NodeError>> {
        drop(endpoints);
        Box::pin(future::ready(Ok(())))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }
}

/// A child node that keeps every endpoint it is handed and sends nothing. With `release`, its
/// stop drops them, as `NodeImplementation::stop` asks ("releases what the node holds");
/// without, it keeps them until it is dropped. On its cancel, given `sent`, it waits
/// [`CANCEL_DELAY`], long enough for every bridge to have seen the cancel, then sends one
/// envelope on its first output port and records whether the send was taken.
struct Probe {
    run_id: RunId,
    node_id: Option<NodeId>,
    release: bool,
    sent: Option<Arc<Mutex<Option<bool>>>>,
    held: Option<NodeEndpoints<PipelinePayload>>,
}

const CANCEL_DELAY: Duration = Duration::from_millis(200);

impl NodeImplementation<PipelinePayload> for Probe {
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
        self.held = Some(endpoints);
        Box::pin(future::ready(Ok(())))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(async move {
            let Some(sent) = self.sent.clone() else {
                return;
            };
            after(CANCEL_DELAY).await;
            let Some(output) = self.held.as_ref().and_then(|held| held.outputs.first()) else {
                return;
            };
            let event = StreamEvent::TranscriptCommitted(TranscriptCommitted {
                span_id: SpanId::generate(),
                text: "after the cancel".to_owned(),
            });
            let envelope = EventEnvelope {
                schema_version: 1,
                event_id: EventId::generate(),
                run_id: self.run_id,
                session_id: None,
                stream_id: StreamId::generate(),
                sequence: 1,
                producer: self.node_id.unwrap_or_else(NodeId::generate),
                port: USAGE_PORT.to_owned(),
                kind: event.name().to_owned(),
                generation_id: None,
                timestamp: 0,
                timebase: Timebase::WallTime,
                causation_id: None,
                payload: PipelinePayload::Stream(event),
            };
            let taken = output.endpoint.send(envelope).await.is_ok();
            *sent.lock().unwrap_or_else(PoisonError::into_inner) = Some(taken);
        })
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        if self.release {
            self.held = None;
        }
        Box::pin(future::ready(()))
    }
}

/// A child node that, as it starts, sends one envelope and `stop` on its first output port, both
/// taken before anything reads them, and releases its endpoints at its stop: a node that sent all
/// it had before an edge downstream of it could refuse.
struct Eager {
    run_id: RunId,
    held: Option<NodeEndpoints<PipelinePayload>>,
}

impl NodeImplementation<PipelinePayload> for Eager {
    fn describe(&mut self, _node: &Node) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn start(
        &mut self,
        endpoints: NodeEndpoints<PipelinePayload>,
    ) -> NodeFuture<'_, Result<(), NodeError>> {
        let held = self.held.insert(endpoints);
        let mut application = Application::new(self.run_id);
        Box::pin(async move {
            let output = held.outputs.first().expect("one output port");
            for envelope in [
                application.text(USAGE_PORT, "sent at start"),
                application.stop(USAGE_PORT),
            ] {
                output
                    .endpoint
                    .send(envelope)
                    .await
                    .map_err(|refused| NodeError::new(refused.to_string()))?;
            }
            Ok(())
        })
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        self.held = None;
        Box::pin(future::ready(()))
    }
}

/// A child pipeline of one probe node, needing no binding: its input exposed as `user text` and
/// its one output port, `output` of structured modality, exposed as `exposed`; and a parent of
/// one subgraph node over it, whose input port is exposed as `user text` and whose output port
/// `output`, optional, has no edge. Answers the declaration, the child and the parent.
fn probed(output: &str, exposed: &str) -> (Declaration, PlannedPipeline, PlannedPipeline) {
    probed_with(output, exposed, false)
}

/// As [`probed`], the parent's output port `output` exposed under its name when `connected`.
fn probed_with(
    output: &str,
    exposed: &str,
    connected: bool,
) -> (Declaration, PlannedPipeline, PlannedPipeline) {
    let (catalog, profile_id) = catalog(Vec::new());
    let mut declaration = Declaration::new();
    let (child, ()) = declaration
        .build(
            &catalog,
            profile_id,
            spec("probe", Vec::new()),
            |pipeline| {
                let probe = pipeline.node("probe", NodeRole::Transform, Vec::new());
                let prompt = pipeline.input::<modality::Text>(&probe, "prompt", TEXT);
                let out = pipeline.output::<modality::Structured>(&probe, output, RECORDS);
                pipeline.expose(&prompt, "user text", capacity(), OverloadPolicy::Pause);
                pipeline.expose(&out, exposed, capacity(), OverloadPolicy::Pause);
            },
        )
        .expect("the profile is declared");
    let validated = declaration.validate(child).expect("valid");
    let child_planned = declaration.plan(validated, &catalog).expect("planned");
    let (parent, ()) = declaration
        .build(&catalog, profile_id, spec("over", Vec::new()), |pipeline| {
            let subgraph = pipeline
                .subgraph("frontend", NodeRole::Transform, child)
                .expect("the child is Planned");
            let into = pipeline.input::<modality::Text>(&subgraph, "user text", TEXT);
            pipeline.expose(&into, "user text", capacity(), OverloadPolicy::Pause);
            if connected {
                let out = pipeline.output::<modality::Structured>(&subgraph, output, RECORDS);
                pipeline.expose(&out, output, capacity(), OverloadPolicy::Pause);
            } else {
                let _unconnected =
                    pipeline.output::<modality::Structured>(&subgraph, output, optional(RECORDS));
            }
        })
        .expect("the profile is declared");
    let validated = declaration.validate(parent).expect("valid");
    let parent_planned = declaration.plan(validated, &catalog).expect("planned");
    (declaration, child_planned, parent_planned)
}

/// Runs the parent of [`probed`], its child's probe made as `probe` says, and ends its input
/// with `stop`; answers how the parent run ended.
fn run_probed(probed: (Declaration, PlannedPipeline, PlannedPipeline), release: bool) -> RunState {
    run_probed_holding(probed, release, &[])
}

/// As [`run_probed`], the caller holding the parent's egresses `egresses` until the run ended.
fn run_probed_holding(
    (declaration, child, parent): (Declaration, PlannedPipeline, PlannedPipeline),
    release: bool,
    egresses: &[&str],
) -> RunState {
    let handles: Arc<Mutex<HashMap<NodeId, SubgraphHandle>>> = Arc::default();
    let running = {
        let declaration_ = declaration.clone();
        Running::start(
            &declaration,
            &parent,
            (&["user text"], egresses),
            move |run_id, node| {
                subgraph_node(
                    run_id,
                    node,
                    &declaration_,
                    &child,
                    Box::new(move |child_run: RunId, _: &Node| {
                        Box::new(Probe {
                            run_id: child_run,
                            node_id: None,
                            release,
                            sent: None,
                            held: None,
                        }) as Box<dyn NodeImplementation<PipelinePayload>>
                    }),
                    &handles,
                )
            },
        )
    };
    let mut application = Application::new(running.run_id);
    running.send("user text", application.stop("user text"));
    running.drive.join().expect("the loop does not panic")
}

/// Forty turns through a subgraph node none of whose output ports has an edge: the child's model
/// sends far more than its egress queues hold (16 each), and the bridge that reads and drops
/// what it sends keeps room in them, so the model is never held, the run completes, and every
/// turn's record was sent in the child.
#[test]
fn a_backlog_through_ports_with_no_edge_never_holds_the_child() {
    const TURNS: usize = 40;
    bounded(|| {
        let mock = generator().reporting((0..TURNS).map(|_| Some(usage(3))));
        let (catalog, profile_id) = catalog(vec![mock.adapter().clone()]);
        let mut declaration = Declaration::new();
        let (child, child_planned, usage_of) = one_model(
            &mut declaration,
            &catalog,
            profile_id,
            mock.adapter().adapter_id(),
        );
        let (parent, ()) = declaration
            .build(&catalog, profile_id, streaming("over"), |pipeline| {
                let subgraph = pipeline
                    .subgraph("frontend", NodeRole::Transform, child)
                    .expect("the child is Planned");
                let into = pipeline.input::<modality::Text>(&subgraph, "user text", TEXT);
                pipeline.expose(&into, "user text", capacity(), OverloadPolicy::Pause);
                let _reply = pipeline.output::<modality::Text>(&subgraph, "reply", optional(TEXT));
                let _usage = pipeline.output::<modality::Structured>(
                    &subgraph,
                    USAGE_PORT,
                    optional(RECORDS),
                );
            })
            .expect("the profile is declared");
        let validated = declaration.validate(parent).expect("valid");
        let planned = declaration.plan(validated, &catalog).expect("planned");
        let answering: Generator = mock.into();
        let recorder = Recorder::new("1");
        let handles: Arc<Mutex<HashMap<NodeId, SubgraphHandle>>> = Arc::default();
        let running = {
            let declaration_ = declaration.clone();
            let recorder = recorder.clone();
            let handles = Arc::clone(&handles);
            Running::start(
                &declaration,
                &planned,
                (&["user text"], &[]),
                move |run_id, node| {
                    subgraph_node(
                        run_id,
                        node,
                        &declaration_,
                        &child_planned,
                        recorded_model(usage_of, answering.clone(), &recorder),
                        &handles,
                    )
                },
            )
        };
        let mut application = Application::new(running.run_id);
        for turn in 0..TURNS {
            running.send(
                "user text",
                application.text("user text", &turn.to_string()),
            );
        }
        running.send("user text", application.stop("user text"));
        let state = running.drive.join().expect("the loop does not panic");
        assert_eq!(state, RunState::Completed);
        let sent = recorded_usage(&recorder);
        assert_eq!(sent.len(), TURNS, "{sent:#?}");
        let node = *handles
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .next()
            .expect("one subgraph node");
        let child_run = child_of(&handles, node);
        assert!(
            sent.iter()
                .all(|recorded| recorded.run_id == Some(child_run))
        );
    });
}

/// Two sibling subgraph nodes over one child pipeline, `a` exposing its `usage` port and `b`
/// leaving its own with no edge: the parent's `a usage` egress carries `a`'s child's record and
/// nothing of `b`'s child, whose model still sent its own, and the run completes.
#[test]
fn a_sibling_with_no_usage_edge_neither_leaks_into_nor_starves_the_connected_one() {
    bounded(|| {
        let mock = generator().reporting([Some(usage(4))]);
        let other = generator().reporting([Some(usage(4))]);
        let (catalog, profile_id) = catalog(vec![mock.adapter().clone()]);
        let mut declaration = Declaration::new();
        let (child, child_planned, usage_of) = one_model(
            &mut declaration,
            &catalog,
            profile_id,
            mock.adapter().adapter_id(),
        );
        let (parent, (a, b)) = declaration
            .build(&catalog, profile_id, streaming("over"), |pipeline| {
                let mut nodes = Vec::new();
                for (name, connected) in [("a", true), ("b", false)] {
                    let subgraph = pipeline
                        .subgraph(name, NodeRole::Transform, child)
                        .expect("the child is Planned");
                    let into = pipeline.input::<modality::Text>(&subgraph, "user text", TEXT);
                    let out = pipeline.output::<modality::Text>(&subgraph, "reply", TEXT);
                    pipeline.expose(
                        &into,
                        format!("{name} text"),
                        capacity(),
                        OverloadPolicy::Pause,
                    );
                    pipeline.expose(
                        &out,
                        format!("{name} reply"),
                        capacity(),
                        OverloadPolicy::Pause,
                    );
                    let traits = if connected {
                        RECORDS
                    } else {
                        optional(RECORDS)
                    };
                    let used =
                        pipeline.output::<modality::Structured>(&subgraph, USAGE_PORT, traits);
                    if connected {
                        pipeline.expose(
                            &used,
                            format!("{name} usage"),
                            capacity(),
                            OverloadPolicy::Pause,
                        );
                    }
                    nodes.push(subgraph.id());
                }
                (nodes[0], nodes[1])
            })
            .expect("the profile is declared");
        let validated = declaration.validate(parent).expect("valid");
        let planned = declaration.plan(validated, &catalog).expect("planned");
        let answering: [Generator; 2] = [mock.into(), other.into()];
        let recorder = Recorder::new("1");
        let handles: Arc<Mutex<HashMap<NodeId, SubgraphHandle>>> = Arc::default();
        let mut running = {
            let declaration_ = declaration.clone();
            let recorder = recorder.clone();
            let handles = Arc::clone(&handles);
            Running::start(
                &declaration,
                &planned,
                (&["a text", "b text"], &["a reply", "b reply", "a usage"]),
                move |run_id, node| {
                    let on = answering[usize::from(node.node_id() == b)].clone();
                    subgraph_node(
                        run_id,
                        node,
                        &declaration_,
                        &child_planned,
                        recorded_model(usage_of, on, &recorder),
                        &handles,
                    )
                },
            )
        };
        let mut application = Application::new(running.run_id);
        for port in ["a text", "b text"] {
            running.send(port, application.text(port, "1"));
            running.send(port, application.stop(port));
        }
        let _a = drain(running.egress("a reply"));
        let _b = drain(running.egress("b reply"));
        let rolled = drain(running.egress("a usage"));
        let state = running.drive.join().expect("the loop does not panic");
        assert_eq!(state, RunState::Completed);
        let (child_a, child_b) = (child_of(&handles, a), child_of(&handles, b));
        let rolled: Vec<Option<RunId>> = records(&rolled)
            .iter()
            .map(|recorded| recorded.run_id)
            .collect();
        assert_eq!(rolled, [Some(child_a)], "only a's child rolls up");
        let sent: Vec<Option<RunId>> = recorded_usage(&recorder)
            .iter()
            .map(|recorded| recorded.run_id)
            .collect();
        assert_eq!(sent.len(), 2, "{sent:#?}");
        assert!(sent.contains(&Some(child_a)) && sent.contains(&Some(child_b)));
    });
}

/// How a parent run ends whose `usage` port, a model node's directly (`subgraph` false) or a
/// subgraph node's over a child of that model, has an edge to a node that dropped its receiving
/// end as it started.
fn usage_edge_to_a_closed_receiver(subgraph: bool) -> RunState {
    let mock = generator().reporting([Some(usage(7))]);
    let (catalog, profile_id) = catalog(vec![mock.adapter().clone()]);
    let adapter = mock.adapter().adapter_id();
    let mut declaration = Declaration::new();
    let (child, child_planned, child_usage) =
        one_model(&mut declaration, &catalog, profile_id, adapter);
    let (parent, (node, usage_port)) = declaration
        .build(&catalog, profile_id, streaming("over"), |pipeline| {
            let (node, into, out, used) = if subgraph {
                let node = pipeline
                    .subgraph("frontend", NodeRole::Transform, child)
                    .expect("the child is Planned");
                let into = pipeline.input::<modality::Text>(&node, "user text", TEXT);
                let out = pipeline.output::<modality::Text>(&node, "reply", TEXT);
                let used = pipeline.output::<modality::Structured>(&node, USAGE_PORT, RECORDS);
                (node, into, out, used)
            } else {
                let node =
                    pipeline.node("model", NodeRole::Model, vec![Capability::OutputStreaming]);
                let into = pipeline.input::<modality::Text>(&node, "prompt", TEXT);
                let out = pipeline.output::<modality::Text>(&node, "answer", TEXT);
                let used = pipeline.output::<modality::Structured>(&node, USAGE_PORT, RECORDS);
                pipeline.bind(&node, adapter).expect("the catalog holds it");
                (node, into, out, used)
            };
            pipeline.expose(&into, "user text", capacity(), OverloadPolicy::Pause);
            pipeline.expose(&out, "reply", capacity(), OverloadPolicy::Pause);
            let deaf = pipeline.node("deaf", NodeRole::Sink, Vec::new());
            let heard = pipeline.input::<modality::Structured>(&deaf, "records", RECORDS);
            pipeline.connect(&used, &heard, capacity(), OverloadPolicy::Pause);
            (node.id(), used.id())
        })
        .expect("the profile is declared");
    let validated = declaration.validate(parent).expect("valid");
    let planned = declaration.plan(validated, &catalog).expect("planned");
    let answering: Generator = mock.into();
    let recorder = Recorder::new("1");
    let handles: Arc<Mutex<HashMap<NodeId, SubgraphHandle>>> = Arc::default();
    let mut running = {
        let declaration_ = declaration.clone();
        Running::start(
            &declaration,
            &planned,
            (&["user text"], &["reply"]),
            move |run_id, declared| {
                if declared.node_id() != node {
                    return Box::new(Deaf);
                }
                if subgraph {
                    subgraph_node(
                        run_id,
                        declared,
                        &declaration_,
                        &child_planned,
                        recorded_model(child_usage, answering.clone(), &recorder),
                        &handles,
                    )
                } else {
                    Box::new(
                        ModelNode::new(run_id, answering.clone(), reply, RunLog::new())
                            .with_usage(usage_port),
                    )
                }
            },
        )
    };
    let mut application = Application::new(running.run_id);
    running.send("user text", application.text("user text", "1"));
    running.send("user text", application.stop("user text"));
    let _replies = drain(running.egress("reply"));
    running.drive.join().expect("the loop does not panic")
}

/// The rule of the story asks a subgraph node's `usage` port to behave as a model node's. With
/// an edge whose receiver closed, rather than no edge, the two end the parent run the same way.
#[test]
fn a_usage_edge_to_a_closed_receiver_ends_the_run_as_it_does_for_a_model_node() {
    bounded(|| {
        let model = usage_edge_to_a_closed_receiver(false);
        let subgraph = usage_edge_to_a_closed_receiver(true);
        assert_eq!(
            subgraph, model,
            "model node: {model:?}, subgraph node: {subgraph:?}"
        );
    });
}

/// A child usage egress whose port has no edge in the parent is read through a cancel until it
/// closes, as the module documentation says: a child node that sends on its `usage` port after
/// its cancel, once every bridge has seen the cancel, has its send taken, not refused.
#[test]
fn a_usage_egress_with_no_edge_is_still_read_after_the_cancel() {
    bounded(|| {
        let (declaration, child, parent) = probed(USAGE_PORT, "probe usage");
        let sent: Arc<Mutex<Option<bool>>> = Arc::default();
        let handles: Arc<Mutex<HashMap<NodeId, SubgraphHandle>>> = Arc::default();
        let running = {
            let declaration_ = declaration.clone();
            let sent = Arc::clone(&sent);
            Running::start(
                &declaration,
                &parent,
                (&["user text"], &[]),
                move |run_id, node| {
                    let sent = Arc::clone(&sent);
                    subgraph_node(
                        run_id,
                        node,
                        &declaration_,
                        &child,
                        Box::new(move |child_run: RunId, _: &Node| {
                            Box::new(Probe {
                                run_id: child_run,
                                node_id: None,
                                release: true,
                                sent: Some(Arc::clone(&sent)),
                                held: None,
                            })
                                as Box<dyn NodeImplementation<PipelinePayload>>
                        }),
                        &handles,
                    )
                },
            )
        };
        running.requests.cancel();
        let state = running.drive.join().expect("the loop does not panic");
        assert_eq!(state, RunState::Cancelled);
        let taken = *sent.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(taken, Some(true), "the send after the cancel was taken");
    });
}

/// Baseline for the next case: a child node that releases its endpoints at its stop, its output
/// port's egress with no edge in the parent; the run completes.
#[test]
fn a_child_node_releasing_its_egress_at_stop_completes_the_parent() {
    bounded(|| {
        assert_eq!(
            run_probed(probed("kept", "kept"), true),
            RunState::Completed
        )
    });
}

/// A child node that keeps its endpoints past its stop, which `NodeImplementation::stop` does
/// not allow ("releases what the node holds") but a flat run tolerates, and sends nothing on an
/// output port the parent left with no edge. Before this story the egress end was dropped and the
/// run completed; the bridge reading it ends once the child run has returned, so the run still
/// completes.
#[test]
fn a_child_node_keeping_its_egress_past_stop_still_completes_the_parent() {
    bounded(|| {
        assert_eq!(
            run_probed(probed("kept", "kept"), false),
            RunState::Completed
        )
    });
}

/// The same behind a port with an edge, the parent's caller holding the egress it leads to. The
/// bridge forwarding the child's egress ends once the child run has returned, as the one with no
/// edge does, so a child node that keeps its egress end past its stop holds no drain.
#[test]
fn a_child_node_keeping_its_egress_past_stop_behind_a_port_with_an_edge_completes_the_parent() {
    bounded(|| {
        assert_eq!(
            run_probed_holding(probed_with("kept", "kept", true), false, &["kept"]),
            RunState::Completed
        )
    });
}

/// The case above it with no race: a child node that sent all it had, `stop` included, before
/// the subgraph node's only edge on that port refused, closed, so the child run completes. A
/// model node whose edge refuses fails, so the subgraph node fails the parent as well, the same
/// end as when the child's node sends after the refusal and fails the child run.
#[test]
fn a_child_that_sent_all_it_had_before_every_edge_refused_fails_the_parent_as_a_model_node_does() {
    bounded(|| {
        let (declaration, child, parent) = probed_with(USAGE_PORT, "probe usage", true);
        let handles: Arc<Mutex<HashMap<NodeId, SubgraphHandle>>> = Arc::default();
        let running = {
            let declaration_ = declaration.clone();
            Running::start(
                &declaration,
                &parent,
                (&["user text"], &[]),
                move |run_id, node| {
                    subgraph_node(
                        run_id,
                        node,
                        &declaration_,
                        &child,
                        Box::new(move |child_run: RunId, _: &Node| {
                            Box::new(Eager {
                                run_id: child_run,
                                held: None,
                            })
                                as Box<dyn NodeImplementation<PipelinePayload>>
                        }),
                        &handles,
                    )
                },
            )
        };
        let mut application = Application::new(running.run_id);
        running.send("user text", application.stop("user text"));
        assert_eq!(
            running.drive.join().expect("the loop does not panic"),
            RunState::Failed
        );
    });
}
