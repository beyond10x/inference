//! Adversary pass 1 on `story:usage-subgraph-rollup`: the usage roll-up of a subgraph node under
//! a cancel of the parent, a child model that fails, nesting, a stop that crosses late, and a
//! `usage` port the parent does not connect, with the cases of
//! `story:subgraph-unconnected-usage-port`: three levels, a cancel and a text port the parent does
//! not connect.
//!
//! Every case runs on a thread of its own and is bounded by [`PATIENCE`], so a bridge that never
//! ends fails the case instead of hanging the suite.

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use inference_core::endpoint::{InputEndpoint, OutputEndpoint};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId, Capability, ModelId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_core::usage::UsageRecorded;
use inference_llm::Generator;
use inference_llm::llm_core::{
    BoxFuture, Cancel, Capabilities, Dispatch, Error, Id, Model, Protocol, Provenance,
    StreamEvent as LlmEvent, StreamSink, TurnObservation, TurnOutcome, TurnRequest, Usage,
};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeImplementation,
    NodeRole, OverloadPolicy, PipelineId, PipelineSpec, PlannedPipeline, PortId, PortTraits,
    USAGE_PORT, modality,
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

fn spec(name: &str) -> PipelineSpec {
    PipelineSpec {
        name: name.to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: vec![Capability::OutputStreaming],
    }
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

/// A model whose every turn streams one delta and then fails after dispatch, carrying the usage
/// the provider reported for it.
#[derive(Debug, Clone)]
struct Failing {
    provenance: Provenance,
    capabilities: Capabilities,
}

impl Failing {
    fn new() -> Self {
        let id = |value: &str| Id::new(value).expect("a printable identifier");
        Self {
            provenance: Provenance {
                protocol: Protocol::Messages,
                provider: id("test"),
                account: id("test"),
                endpoint: id("in-process"),
                model: id("failing"),
                binding_revision: id("1"),
            },
            capabilities: Capabilities::text(1024, 256),
        }
    }
}

impl Model for Failing {
    fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn turn<'a>(
        &'a self,
        _request: &'a TurnRequest,
        sink: &'a mut dyn StreamSink,
        _cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<TurnOutcome, Error>> {
        Box::pin(async move {
            sink.emit(LlmEvent::TextDelta {
                text: "partial".to_owned(),
            })
            .await?;
            Err(Error::protocol("the stream broke")
                .with_dispatch(Dispatch::Accepted)
                .with_observation(TurnObservation {
                    usage: Some(usage(30)),
                    final_usage: false,
                    ..TurnObservation::new(self.provenance.clone())
                }))
        })
    }
}

/// One model of a child: its ingress, its egress and the name its `usage` port is exposed under.
type Names = (&'static str, &'static str, &'static str);

/// What a declaration built here holds: the pipeline, and each model node with its `usage` port.
struct Models {
    pipeline: PipelineId,
    planned: PlannedPipeline,
    usage_of: Vec<(NodeId, PortId)>,
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

/// A pipeline of one model per entry of `names`, each bound to `adapter`, planned.
fn models(
    declaration: &mut Declaration,
    catalog: &Fixture,
    profile_id: ProfileId,
    names: &[Names],
    adapter: AdapterId,
) -> Models {
    let (pipeline, usage_of) = declaration
        .build(catalog, profile_id, spec("models"), |pipeline| {
            let mut usage_of = Vec::new();
            for (index, &(ingress, egress, usage)) in names.iter().enumerate() {
                let model = pipeline.node(
                    format!("model {index}"),
                    NodeRole::Model,
                    vec![Capability::OutputStreaming],
                );
                let prompt = pipeline.input::<modality::Text>(&model, "prompt", TEXT);
                let answer = pipeline.output::<modality::Text>(&model, "answer", TEXT);
                let used = pipeline.output::<modality::Structured>(&model, USAGE_PORT, RECORDS);
                pipeline.expose(&prompt, ingress, capacity(), OverloadPolicy::Pause);
                pipeline.expose(&answer, egress, capacity(), OverloadPolicy::Pause);
                pipeline.expose(&used, usage, capacity(), OverloadPolicy::Pause);
                pipeline
                    .bind(&model, adapter)
                    .expect("the catalog holds it");
                usage_of.push((model.id(), used.id()));
            }
            usage_of
        })
        .expect("the profile is declared");
    let validated = declaration.validate(pipeline).expect("valid");
    let planned = declaration.plan(validated, catalog).expect("planned");
    Models {
        pipeline,
        planned,
        usage_of,
    }
}

/// A pipeline of one subgraph node over `child`, its text ports `texts` exposed under their
/// names and its `usage` port exposed as `usage`, or declared and connected to nothing when
/// `usage` is none; planned. Answers it and the subgraph node.
fn over(
    declaration: &mut Declaration,
    catalog: &Fixture,
    profile_id: ProfileId,
    child: PipelineId,
    texts: &[(&str, &str)],
    usage: Option<&str>,
) -> (PipelineId, PlannedPipeline, NodeId) {
    over_named(
        declaration,
        catalog,
        profile_id,
        child,
        texts,
        (USAGE_PORT, usage),
    )
}

/// As [`over`], the node's structured output port named `usage.0` rather than `usage`.
fn over_named(
    declaration: &mut Declaration,
    catalog: &Fixture,
    profile_id: ProfileId,
    child: PipelineId,
    texts: &[(&str, &str)],
    (port, usage): (&str, Option<&str>),
) -> (PipelineId, PlannedPipeline, NodeId) {
    let (pipeline, node) = declaration
        .build(catalog, profile_id, spec("over"), |pipeline| {
            let subgraph = pipeline
                .subgraph("frontend", NodeRole::Transform, child)
                .expect("the child is Planned");
            for &(ingress, egress) in texts {
                let into = pipeline.input::<modality::Text>(&subgraph, ingress, TEXT);
                let out = pipeline.output::<modality::Text>(&subgraph, egress, TEXT);
                pipeline.expose(&into, ingress, capacity(), OverloadPolicy::Pause);
                pipeline.expose(&out, egress, capacity(), OverloadPolicy::Pause);
            }
            let traits = if usage.is_some() {
                RECORDS
            } else {
                PortTraits {
                    essentiality: Essentiality::Optional,
                    ..RECORDS
                }
            };
            let used = pipeline.output::<modality::Structured>(&subgraph, port, traits);
            if let Some(name) = usage {
                pipeline.expose(&used, name, capacity(), OverloadPolicy::Pause);
            }
            subgraph.id()
        })
        .expect("the profile is declared");
    let validated = declaration.validate(pipeline).expect("valid");
    let planned = declaration
        .plan(validated, catalog)
        .expect("the usage port forwards the child's usage egresses");
    (pipeline, planned, node)
}

/// The model nodes of a child run, all answering on one generator, each given its usage port.
fn model_nodes(
    usage_of: &[(NodeId, PortId)],
    answering: Generator,
) -> ChildImplementations<PipelinePayload> {
    each_on(usage_of, vec![answering; usage_of.len()])
}

/// The model nodes of a child run, the n-th answering on the n-th of `answering`.
fn each_on(
    usage_of: &[(NodeId, PortId)],
    answering: Vec<Generator>,
) -> ChildImplementations<PipelinePayload> {
    let nodes: HashMap<NodeId, (PortId, Generator)> = usage_of
        .iter()
        .zip(answering)
        .map(|((node, usage), generator)| (*node, (*usage, generator)))
        .collect();
    Box::new(move |run_id: RunId, node: &Node| {
        let (usage, generator) = nodes[&node.node_id()].clone();
        Box::new(ModelNode::new(run_id, generator, reply, RunLog::new()).with_usage(usage))
            as Box<dyn NodeImplementation<PipelinePayload>>
    })
}

/// A subgraph node of the run `run_id` over `child`, whose nodes `inner` makes; its handle is
/// kept in `handles`.
fn subgraph_node(
    run_id: RunId,
    declaration: &Declaration,
    child: &PlannedPipeline,
    inner: ChildImplementations<PipelinePayload>,
    handles: &Arc<Mutex<Vec<SubgraphHandle>>>,
) -> Box<dyn NodeImplementation<PipelinePayload>> {
    let node = SubgraphNode::new(
        run_id,
        declaration,
        child,
        HashMap::<GenerationId, i64>::new(),
        inner,
    );
    handles
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(node.handle());
    Box::new(node)
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

fn drain(egress: &mut dyn InputEndpoint<PipelinePayload>) -> Received {
    let mut received = Vec::new();
    while let Some(envelope) = block_on(egress.recv()) {
        received.push(envelope);
    }
    received
}

/// Receives on `egress` until an envelope `wanted` holds of arrives, and answers it; none if the
/// egress ended first.
fn until(
    egress: &mut dyn InputEndpoint<PipelinePayload>,
    wanted: impl Fn(&EventEnvelope<PipelinePayload>) -> bool,
) -> Option<EventEnvelope<PipelinePayload>> {
    while let Some(envelope) = block_on(egress.recv()) {
        if wanted(&envelope) {
            return Some(envelope);
        }
    }
    None
}

fn records(received: &Received) -> Vec<(&EventEnvelope<PipelinePayload>, &UsageRecorded)> {
    received
        .iter()
        .filter_map(|envelope| match &envelope.payload {
            PipelinePayload::Usage(recorded) => Some((envelope, recorded)),
            _ => None,
        })
        .collect()
}

fn is_stop(envelope: &EventEnvelope<PipelinePayload>) -> bool {
    envelope.kind == ControlKind::Stop.as_str()
}

fn is_text(envelope: &EventEnvelope<PipelinePayload>) -> bool {
    matches!(
        envelope.kind.as_str(),
        "inference.stream.TextDelta" | "inference.stream.TextSegmentCommitted"
    )
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

fn child_run(handles: &Arc<Mutex<Vec<SubgraphHandle>>>, at: usize) -> RunId {
    handles.lock().unwrap_or_else(PoisonError::into_inner)[at]
        .child_run_id()
        .expect("prepared")
}

/// A model holding its first turn, reporting usage for it.
fn holding() -> MockGenerator {
    generator().reporting([Some(usage(20))]).holding(0)
}

/// Sends one text to the model behind `user text`, waits until its held turn streamed, cancels
/// the run, waits for its end and answers what the `usage` egress received, the run's state and
/// the run.
fn cancelled_mid_turn(mut running: Running) -> (Received, RunState, RunId) {
    let mut application = Application::new(running.run_id);
    running.send("user text", application.text("user text", "1"));
    until(running.egress("reply"), is_text).expect("the held turn streams");
    running.requests.cancel();
    let state = running.drive.join().expect("the loop does not panic");
    let usage = running.egresses.get_mut(USAGE_PORT).expect("the egress");
    let received = drain(usage.as_mut());
    (received, state, running.run_id)
}

/// Baseline for the next case: the model directly in the run, its `usage` port exposed. The
/// run's cancel cuts the held turn and the node sends that turn's record (`story:llm-usage-on-
/// cancel`), which the run's `usage` egress delivers.
#[test]
fn a_cancel_of_a_run_holding_its_model_still_delivers_the_cut_turns_record() {
    bounded(|| {
        let mock = holding();
        let (catalog, profile_id) = catalog(vec![mock.adapter().clone()]);
        let mut declaration = Declaration::new();
        let flat = models(
            &mut declaration,
            &catalog,
            profile_id,
            &[("user text", "reply", USAGE_PORT)],
            mock.adapter().adapter_id(),
        );
        let mut nodes = model_nodes(&flat.usage_of, mock.into());
        let running = Running::start(
            &declaration,
            &flat.planned,
            (&["user text"], &["reply", USAGE_PORT]),
            move |run_id, node| nodes(run_id, node),
        );
        let (received, state, _) = cancelled_mid_turn(running);
        assert_eq!(state, RunState::Cancelled);
        let records = records(&received);
        let [(_, recorded)] = records.as_slice() else {
            panic!("the cut turn's record: {received:#?}")
        };
        assert!(!recorded.r#final);
    });
}

/// The same model inside a subgraph node: the parent's cancel cuts the child's held turn, and
/// the child's node sends that turn's record on its usage egress as it does in a run of its own.
/// The roll-up must forward it, so that the parent's total counts it; the egress bridge must not
/// give up a usage egress at the cancel before the child's node has sent what it holds.
#[test]
fn a_cancel_of_the_parent_still_rolls_up_the_cut_turns_record_of_the_child() {
    bounded(|| cut_in_child(USAGE_PORT));
}

/// The same through a port named after the child's usage egress, which crossed by name before
/// this story: whether the loss predates the roll-up.
#[test]
fn a_cancel_of_the_parent_still_delivers_the_cut_turns_record_through_a_port_of_its_name() {
    bounded(|| cut_in_child("model usage"));
}

/// The child's model, holding its first turn, behind a subgraph node whose structured output
/// port `port` takes the child's usage egress `model usage` and is exposed under its own name;
/// the parent is cancelled during the held turn and its `port` egress must deliver the cut
/// turn's record, naming the child run.
fn cut_in_child(port: &'static str) {
    let mock = holding();
    let (catalog, profile_id) = catalog(vec![mock.adapter().clone()]);
    let mut declaration = Declaration::new();
    let child = models(
        &mut declaration,
        &catalog,
        profile_id,
        &[("user text", "reply", "model usage")],
        mock.adapter().adapter_id(),
    );
    let (_, parent, _) = over_named(
        &mut declaration,
        &catalog,
        profile_id,
        child.pipeline,
        &[("user text", "reply")],
        (port, Some(port)),
    );
    let handles: Arc<Mutex<Vec<SubgraphHandle>>> = Arc::default();
    let answering: Generator = mock.into();
    let mut running = {
        let declaration_ = declaration.clone();
        let handles = Arc::clone(&handles);
        Running::start(
            &declaration,
            &parent,
            (&["user text"], &["reply", port]),
            move |run_id, _| {
                subgraph_node(
                    run_id,
                    &declaration_,
                    &child.planned,
                    model_nodes(&child.usage_of, answering.clone()),
                    &handles,
                )
            },
        )
    };
    if port != USAGE_PORT {
        let egress = running.egresses.remove(port).expect("the egress");
        running.egresses.insert(USAGE_PORT.to_owned(), egress);
    }
    let (received, state, _) = cancelled_mid_turn(running);
    assert_eq!(state, RunState::Cancelled);
    let child_run = child_run(&handles, 0);
    let records = records(&received);
    let [(_, recorded)] = records.as_slice() else {
        panic!("the child's cut turn's record, through `{port}`: {received:#?}")
    };
    assert!(!recorded.r#final);
    assert_eq!(recorded.run_id, Some(child_run));
}

/// A child's model whose turn fails after dispatch fails the child run; the failed turn's usage
/// was consumed all the same, and the record the node sends for it rolls up to the parent before
/// the parent fails at its drain.
#[test]
fn a_child_model_that_fails_mid_run_still_rolls_up_its_failed_turns_record() {
    bounded(|| {
        let mock = generator();
        let (catalog, profile_id) = catalog(vec![mock.adapter().clone()]);
        let mut declaration = Declaration::new();
        let child = models(
            &mut declaration,
            &catalog,
            profile_id,
            &[("user text", "reply", "model usage")],
            mock.adapter().adapter_id(),
        );
        let (_, parent, _) = over(
            &mut declaration,
            &catalog,
            profile_id,
            child.pipeline,
            &[("user text", "reply")],
            Some(USAGE_PORT),
        );
        let answering = Generator::new(mock.adapter().clone(), Arc::new(Failing::new()))
            .expect("the mock's adapter claims what the model declares");
        let handles: Arc<Mutex<Vec<SubgraphHandle>>> = Arc::default();
        let mut running = {
            let declaration_ = declaration.clone();
            let handles = Arc::clone(&handles);
            Running::start(
                &declaration,
                &parent,
                (&["user text"], &["reply", USAGE_PORT]),
                move |run_id, _| {
                    subgraph_node(
                        run_id,
                        &declaration_,
                        &child.planned,
                        model_nodes(&child.usage_of, answering.clone()),
                        &handles,
                    )
                },
            )
        };
        let mut application = Application::new(running.run_id);
        running.send("user text", application.text("user text", "1"));
        running.send("user text", application.stop("user text"));
        let _replies = drain(running.egress("reply"));
        let received = drain(running.egress(USAGE_PORT));
        let state = running.drive.join().expect("the loop does not panic");
        assert_eq!(
            state,
            RunState::Failed,
            "the child's failure fails the parent"
        );
        let records = records(&received);
        let [(envelope, recorded)] = records.as_slice() else {
            panic!("the failed turn's record: {received:#?}")
        };
        assert_eq!(envelope.run_id, running.run_id);
        assert_eq!(recorded.run_id, Some(child_run(&handles, 0)));
        assert!(!recorded.r#final);
    });
}

/// Two subgraphs deep: the model sits in the inner child, whose usage egress the middle
/// pipeline's subgraph node forwards on its `usage` port, exposed as `inner usage`; the outer
/// subgraph node forwards that egress in turn. The record reaches the top once, on the top run,
/// produced by the outer subgraph node, still naming the innermost run it was made in; one
/// `stop` follows it.
#[test]
fn a_model_two_subgraphs_deep_rolls_up_once_naming_the_innermost_run() {
    bounded(|| {
        let mock = generator().reporting([Some(usage(7))]);
        let (catalog, profile_id) = catalog(vec![mock.adapter().clone()]);
        let mut declaration = Declaration::new();
        let inner = models(
            &mut declaration,
            &catalog,
            profile_id,
            &[("user text", "reply", "model usage")],
            mock.adapter().adapter_id(),
        );
        let (middle_id, middle, _) = over(
            &mut declaration,
            &catalog,
            profile_id,
            inner.pipeline,
            &[("user text", "reply")],
            Some("inner usage"),
        );
        let (_, outer, outer_node) = over(
            &mut declaration,
            &catalog,
            profile_id,
            middle_id,
            &[("user text", "reply")],
            Some(USAGE_PORT),
        );
        let answering: Generator = mock.into();
        let outer_handles: Arc<Mutex<Vec<SubgraphHandle>>> = Arc::default();
        let inner_handles: Arc<Mutex<Vec<SubgraphHandle>>> = Arc::default();
        let mut running = {
            let declaration_ = declaration.clone();
            let outer_handles = Arc::clone(&outer_handles);
            let inner_handles = Arc::clone(&inner_handles);
            Running::start(
                &declaration,
                &outer,
                (&["user text"], &["reply", USAGE_PORT]),
                move |run_id, _| {
                    let declaration__ = declaration_.clone();
                    let inner_planned = inner.planned.clone();
                    let usage_of = inner.usage_of.clone();
                    let answering = answering.clone();
                    let inner_handles = Arc::clone(&inner_handles);
                    subgraph_node(
                        run_id,
                        &declaration_,
                        &middle,
                        Box::new(move |middle_run: RunId, _: &Node| {
                            subgraph_node(
                                middle_run,
                                &declaration__,
                                &inner_planned,
                                model_nodes(&usage_of, answering.clone()),
                                &inner_handles,
                            )
                        }),
                        &outer_handles,
                    )
                },
            )
        };
        let mut application = Application::new(running.run_id);
        running.send("user text", application.text("user text", "1"));
        running.send("user text", application.stop("user text"));
        let replies = drain(running.egress("reply"));
        assert!(
            replies
                .iter()
                .any(|envelope| envelope.kind == "inference.stream.TextSegmentCommitted"),
            "the model answers through two subgraphs: {:?}",
            replies
                .iter()
                .map(|envelope| envelope.kind.as_str())
                .collect::<Vec<_>>()
        );
        let received = drain(running.egress(USAGE_PORT));
        let state = running.drive.join().expect("the loop does not panic");
        assert_eq!(state, RunState::Completed);
        let innermost = child_run(&inner_handles, 0);
        let middle_run = child_run(&outer_handles, 0);
        assert_ne!(innermost, middle_run);
        let records = records(&received);
        let [(envelope, recorded)] = records.as_slice() else {
            panic!(
                "one record, once: {received:#?}; replies: {:?}",
                replies
                    .iter()
                    .map(|envelope| envelope.kind.as_str())
                    .collect::<Vec<_>>()
            )
        };
        assert_eq!(envelope.run_id, running.run_id);
        assert_eq!(envelope.producer, outer_node);
        assert_eq!(envelope.port, USAGE_PORT);
        assert_eq!(recorded.run_id, Some(innermost));
        let stops: Vec<usize> = received
            .iter()
            .enumerate()
            .filter(|(_, envelope)| is_stop(envelope))
            .map(|(at, _)| at)
            .collect();
        assert_eq!(stops, [received.len() - 1], "{received:#?}");
    });
}

/// A child of two models whose usage egresses are `usage`, which the node's port names, and
/// `late usage`, which it forwards. The first model ends its input and its usage egress sends
/// `stop` well before the second model is asked anything: the merged `stop` must wait for the
/// second egress, so that its record still crosses, and cross once, last.
#[test]
fn the_merged_stop_waits_for_a_usage_egress_that_ends_late() {
    bounded(|| {
        let mock = generator().reporting([Some(usage(3))]);
        let (catalog, profile_id) = catalog(vec![mock.adapter().clone()]);
        let mut declaration = Declaration::new();
        let child = models(
            &mut declaration,
            &catalog,
            profile_id,
            &[
                ("user text", "reply", USAGE_PORT),
                ("second text", "second reply", "late usage"),
            ],
            mock.adapter().adapter_id(),
        );
        let (_, parent, _) = over(
            &mut declaration,
            &catalog,
            profile_id,
            child.pipeline,
            &[("user text", "reply"), ("second text", "second reply")],
            Some(USAGE_PORT),
        );
        let second_mock = generator().reporting([Some(usage(4))]);
        let answering: Vec<Generator> = vec![mock.into(), second_mock.into()];
        let handles: Arc<Mutex<Vec<SubgraphHandle>>> = Arc::default();
        let usage_of = child.usage_of.clone();
        let mut running = {
            let declaration_ = declaration.clone();
            let handles = Arc::clone(&handles);
            Running::start(
                &declaration,
                &parent,
                (
                    &["user text", "second text"],
                    &["reply", "second reply", USAGE_PORT],
                ),
                move |run_id, _| {
                    subgraph_node(
                        run_id,
                        &declaration_,
                        &child.planned,
                        each_on(&child.usage_of, answering.clone()),
                        &handles,
                    )
                },
            )
        };
        let mut application = Application::new(running.run_id);
        running.send("user text", application.text("user text", "1"));
        running.send("user text", application.stop("user text"));
        until(running.egress("reply"), is_stop).expect("the first model ends its output");
        let first = until(running.egress(USAGE_PORT), |envelope| !is_stop(envelope))
            .expect("the first record crosses");
        thread::sleep(Duration::from_millis(200));
        let mut second = Application::new(running.run_id);
        running.send("second text", second.text("second text", "2"));
        running.send("second text", second.stop("second text"));
        let _replies = drain(running.egress("second reply"));
        let _first_replies = drain(running.egress("reply"));
        let mut received = vec![first];
        received.extend(drain(running.egress(USAGE_PORT)));
        let state = running.drive.join().expect("the loop does not panic");
        assert_eq!(state, RunState::Completed);
        let made_by: Vec<Option<NodeId>> = records(&received)
            .iter()
            .map(|(_, recorded)| recorded.node_id)
            .collect();
        assert_eq!(
            made_by,
            [Some(usage_of[0].0), Some(usage_of[1].0)],
            "both records, each once: {received:#?}"
        );
        let kinds: Vec<bool> = received.iter().map(is_stop).collect();
        assert_eq!(kinds, [false, false, true], "{received:#?}");
    });
}

/// The model nodes of a child run as [`model_nodes`] makes them, each wrapped by `recorder`, so
/// that every envelope they send is recorded before it reaches the child's edge or egress.
fn recorded_models(
    usage_of: &[(NodeId, PortId)],
    answering: Generator,
    recorder: &Recorder<PipelinePayload>,
) -> ChildImplementations<PipelinePayload> {
    let mut inner = model_nodes(usage_of, answering);
    let recorder = recorder.clone();
    Box::new(move |run_id: RunId, node: &Node| recorder.wrap(node, inner(run_id, node)))
}

/// The usage records `recorder` took, in merge order.
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

/// Runs one turn through a parent whose subgraph node's structured output port `port`, optional,
/// takes the child's usage egress `model usage` and is connected to nothing; answers how the
/// parent run ended, the records the child's model sent, as a recorder on the child's nodes took
/// them, and the child run.
fn unconnected(port: &'static str) -> (RunState, Vec<UsageRecorded>, RunId) {
    let mock = generator().reporting([Some(usage(5))]);
    let (catalog, profile_id) = catalog(vec![mock.adapter().clone()]);
    let mut declaration = Declaration::new();
    let child = models(
        &mut declaration,
        &catalog,
        profile_id,
        &[("user text", "reply", "model usage")],
        mock.adapter().adapter_id(),
    );
    let (_, parent, _) = over_named(
        &mut declaration,
        &catalog,
        profile_id,
        child.pipeline,
        &[("user text", "reply")],
        (port, None),
    );
    let answering: Generator = mock.into();
    let handles: Arc<Mutex<Vec<SubgraphHandle>>> = Arc::default();
    let recorder = Recorder::new("1");
    let mut running = {
        let declaration_ = declaration.clone();
        let handles = Arc::clone(&handles);
        let recorder = recorder.clone();
        Running::start(
            &declaration,
            &parent,
            (&["user text"], &["reply"]),
            move |run_id, _| {
                subgraph_node(
                    run_id,
                    &declaration_,
                    &child.planned,
                    recorded_models(&child.usage_of, answering.clone(), &recorder),
                    &handles,
                )
            },
        )
    };
    let mut application = Application::new(running.run_id);
    running.send("user text", application.text("user text", "1"));
    running.send("user text", application.stop("user text"));
    let _replies = drain(running.egress("reply"));
    let state = running.drive.join().expect("the loop does not panic");
    (state, recorded_usage(&recorder), child_run(&handles, 0))
}

/// The rule of `story:subgraph-unconnected-usage-port`: a subgraph node whose `usage` port has
/// no edge behaves as a model node whose own `usage` port has no edge, whose run completes. The
/// node goes on reading the child's usage egresses, so the child's model sends its record as in
/// a run of its own, where the child's recorder sees it, and nothing in the parent carries it.
#[test]
fn a_usage_port_the_parent_connects_to_nothing_does_not_fail_the_parent() {
    bounded(|| {
        let (state, records, child) = unconnected(USAGE_PORT);
        assert_eq!(state, RunState::Completed);
        let [recorded] = records.as_slice() else {
            panic!("the child's model sends its record once: {records:#?}")
        };
        assert_eq!(recorded.run_id, Some(child));
    });
}

/// The same without the roll-up: the node's port carries the name of the child's usage egress,
/// which it matched by name before this story, and is connected to nothing.
#[test]
fn a_port_named_after_the_childs_usage_egress_connected_to_nothing_does_not_fail_the_parent() {
    bounded(|| {
        let (state, records, child) = unconnected("model usage");
        assert_eq!(state, RunState::Completed);
        let [recorded] = records.as_slice() else {
            panic!("the child's model sends its record once: {records:#?}")
        };
        assert_eq!(recorded.run_id, Some(child));
    });
}

/// A subgraph node's ports: an input port per name of `inputs` and a text output port per entry
/// of `outputs`, exposed under its name when its flag is set and otherwise optional and connected
/// to nothing; and a structured output port `usage`, none when `usage` is none, otherwise exposed
/// under the name it holds or, holding none, optional and connected to nothing. Planned.
fn over_with(
    declaration: &mut Declaration,
    catalog: &Fixture,
    profile_id: ProfileId,
    child: PipelineId,
    (inputs, outputs): (&[&str], &[(&str, bool)]),
    usage: Option<Option<&str>>,
) -> (PipelineId, PlannedPipeline) {
    let optional = |traits: PortTraits| PortTraits {
        essentiality: Essentiality::Optional,
        ..traits
    };
    let (pipeline, ()) = declaration
        .build(catalog, profile_id, spec("over"), |pipeline| {
            let subgraph = pipeline
                .subgraph("frontend", NodeRole::Transform, child)
                .expect("the child is Planned");
            for &name in inputs {
                let into = pipeline.input::<modality::Text>(&subgraph, name, TEXT);
                pipeline.expose(&into, name, capacity(), OverloadPolicy::Pause);
            }
            for &(name, exposed) in outputs {
                let traits = if exposed { TEXT } else { optional(TEXT) };
                let out = pipeline.output::<modality::Text>(&subgraph, name, traits);
                if exposed {
                    pipeline.expose(&out, name, capacity(), OverloadPolicy::Pause);
                }
            }
            if let Some(usage) = usage {
                let traits = if usage.is_some() {
                    RECORDS
                } else {
                    optional(RECORDS)
                };
                let used = pipeline.output::<modality::Structured>(&subgraph, USAGE_PORT, traits);
                if let Some(name) = usage {
                    pipeline.expose(&used, name, capacity(), OverloadPolicy::Pause);
                }
            }
        })
        .expect("the profile is declared");
    let validated = declaration.validate(pipeline).expect("valid");
    let planned = declaration.plan(validated, catalog).expect("planned");
    (pipeline, planned)
}

/// Runs one turn through three levels, a top run over a middle pipeline over a child of one
/// model reporting usage; `middle` is the middle subgraph node's `usage` port and `top` the top
/// one's, as [`over_with`] reads them. Answers how the top run ended, the records the innermost
/// model sent, as a recorder on it took them, and the innermost run.
fn three_levels(
    middle: Option<&'static str>,
    top: Option<Option<&'static str>>,
) -> (RunState, Vec<UsageRecorded>, RunId) {
    let mock = generator().reporting([Some(usage(9))]);
    let (catalog, profile_id) = catalog(vec![mock.adapter().clone()]);
    let mut declaration = Declaration::new();
    let inner = models(
        &mut declaration,
        &catalog,
        profile_id,
        &[("user text", "reply", "model usage")],
        mock.adapter().adapter_id(),
    );
    let texts: (&[&str], &[(&str, bool)]) = (&["user text"], &[("reply", true)]);
    let (middle_id, middle_planned) = over_with(
        &mut declaration,
        &catalog,
        profile_id,
        inner.pipeline,
        texts,
        Some(middle),
    );
    let (_, top_planned) = over_with(
        &mut declaration,
        &catalog,
        profile_id,
        middle_id,
        texts,
        top,
    );
    let answering: Generator = mock.into();
    let recorder = Recorder::new("1");
    let inner_handles: Arc<Mutex<Vec<SubgraphHandle>>> = Arc::default();
    let mut running = {
        let declaration_ = declaration.clone();
        let recorder = recorder.clone();
        let inner_handles = Arc::clone(&inner_handles);
        Running::start(
            &declaration,
            &top_planned,
            (&["user text"], &["reply"]),
            move |run_id, _| {
                let declaration__ = declaration_.clone();
                let inner_planned = inner.planned.clone();
                let usage_of = inner.usage_of.clone();
                let answering = answering.clone();
                let recorder = recorder.clone();
                let inner_handles = Arc::clone(&inner_handles);
                subgraph_node(
                    run_id,
                    &declaration_,
                    &middle_planned,
                    Box::new(move |middle_run: RunId, _: &Node| {
                        subgraph_node(
                            middle_run,
                            &declaration__,
                            &inner_planned,
                            recorded_models(&usage_of, answering.clone(), &recorder),
                            &inner_handles,
                        )
                    }),
                    &Arc::default(),
                )
            },
        )
    };
    let mut application = Application::new(running.run_id);
    running.send("user text", application.text("user text", "1"));
    running.send("user text", application.stop("user text"));
    let replies = drain(running.egress("reply"));
    assert!(
        replies
            .iter()
            .any(|envelope| envelope.kind == "inference.stream.TextSegmentCommitted"),
        "the model answers through two subgraphs: {replies:#?}"
    );
    let state = running.drive.join().expect("the loop does not panic");
    (
        state,
        recorded_usage(&recorder),
        child_run(&inner_handles, 0),
    )
}

/// Three levels, the top subgraph node's `usage` port connected to nothing: the middle node
/// forwards the innermost model's record on its egress `inner usage`, and the top node reads that
/// egress and carries the record nowhere, as a model node with no edge on its `usage` port.
#[test]
fn three_levels_with_the_top_usage_port_connected_to_nothing_complete() {
    bounded(|| {
        let (state, records, innermost) = three_levels(Some("inner usage"), Some(None));
        assert_eq!(state, RunState::Completed);
        let [recorded] = records.as_slice() else {
            panic!("the innermost model sends its record once: {records:#?}")
        };
        assert_eq!(recorded.run_id, Some(innermost));
    });
}

/// Three levels, the middle subgraph node's `usage` port connected to nothing inside the middle
/// pipeline, and the top node with no `usage` port, the middle pipeline exposing no usage.
#[test]
fn three_levels_with_the_middle_usage_port_connected_to_nothing_complete() {
    bounded(|| {
        let (state, records, innermost) = three_levels(None, None);
        assert_eq!(state, RunState::Completed);
        let [recorded] = records.as_slice() else {
            panic!("the innermost model sends its record once: {records:#?}")
        };
        assert_eq!(recorded.run_id, Some(innermost));
    });
}

/// A cancel of the parent whose subgraph node's `usage` port is connected to nothing. The child's
/// model reports usage for a first turn that ends before the cancel and holds the second, which
/// the cancel cuts; it sends both records, the cut one after its own cancel, and the node reads
/// the usage egress through the cancel until it closes, so the child run is cancelled, not
/// failed, and its recorder holds both records.
#[test]
fn a_cancel_of_the_parent_with_its_usage_port_connected_to_nothing_cancels_the_child() {
    bounded(|| {
        let mock = generator()
            .reporting([Some(usage(5)), Some(usage(20))])
            .holding(1);
        let (catalog, profile_id) = catalog(vec![mock.adapter().clone()]);
        let mut declaration = Declaration::new();
        let child = models(
            &mut declaration,
            &catalog,
            profile_id,
            &[("user text", "reply", "model usage")],
            mock.adapter().adapter_id(),
        );
        let (_, parent) = over_with(
            &mut declaration,
            &catalog,
            profile_id,
            child.pipeline,
            (&["user text"], &[("reply", true)]),
            Some(None),
        );
        let answering: Generator = mock.into();
        let handles: Arc<Mutex<Vec<SubgraphHandle>>> = Arc::default();
        let recorder = Recorder::new("1");
        let mut running = {
            let declaration_ = declaration.clone();
            let handles = Arc::clone(&handles);
            let recorder = recorder.clone();
            Running::start(
                &declaration,
                &parent,
                (&["user text"], &["reply"]),
                move |run_id, _| {
                    subgraph_node(
                        run_id,
                        &declaration_,
                        &child.planned,
                        recorded_models(&child.usage_of, answering.clone(), &recorder),
                        &handles,
                    )
                },
            )
        };
        let mut application = Application::new(running.run_id);
        running.send("user text", application.text("user text", "1"));
        let committed = |envelope: &EventEnvelope<PipelinePayload>| {
            envelope.kind == "inference.stream.TextSegmentCommitted"
        };
        until(running.egress("reply"), committed).expect("the first turn ends");
        let deadline = Instant::now() + PATIENCE / 2;
        while recorded_usage(&recorder).is_empty() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        running.send("user text", application.text("user text", "2"));
        let _held = until(running.egress("reply"), is_text);
        running.requests.cancel();
        let state = running.drive.join().expect("the loop does not panic");
        assert_eq!(state, RunState::Cancelled);
        let at_stop = handles.lock().unwrap_or_else(PoisonError::into_inner)[0]
            .at_stop()
            .expect("the node stopped");
        assert_eq!(at_stop.state, RunState::Cancelled, "{at_stop:#?}");
        assert!(at_stop.queues_dropped);
        let finals: Vec<bool> = recorded_usage(&recorder)
            .iter()
            .map(|recorded| recorded.r#final)
            .collect();
        assert_eq!(finals, [true, false]);
    });
}

/// The rest of the class: a text output port of the subgraph node connected to nothing, which
/// validation admits as optional, leaves the child's egress of its name to be read and dropped
/// as well, so the child's model, which sends its reply there, does not fail the parent, and its
/// usage still rolls up on the connected `usage` port.
#[test]
fn a_text_port_the_parent_connects_to_nothing_does_not_fail_the_parent() {
    bounded(|| {
        let mock = generator().reporting([Some(usage(6))]);
        let (catalog, profile_id) = catalog(vec![mock.adapter().clone()]);
        let mut declaration = Declaration::new();
        let child = models(
            &mut declaration,
            &catalog,
            profile_id,
            &[("user text", "reply", "model usage")],
            mock.adapter().adapter_id(),
        );
        let (_, parent) = over_with(
            &mut declaration,
            &catalog,
            profile_id,
            child.pipeline,
            (&["user text"], &[("reply", false)]),
            Some(Some(USAGE_PORT)),
        );
        let answering: Generator = mock.into();
        let handles: Arc<Mutex<Vec<SubgraphHandle>>> = Arc::default();
        let mut running = {
            let declaration_ = declaration.clone();
            let handles = Arc::clone(&handles);
            Running::start(
                &declaration,
                &parent,
                (&["user text"], &[USAGE_PORT]),
                move |run_id, _| {
                    subgraph_node(
                        run_id,
                        &declaration_,
                        &child.planned,
                        model_nodes(&child.usage_of, answering.clone()),
                        &handles,
                    )
                },
            )
        };
        let mut application = Application::new(running.run_id);
        running.send("user text", application.text("user text", "1"));
        running.send("user text", application.stop("user text"));
        let received = drain(running.egress(USAGE_PORT));
        let state = running.drive.join().expect("the loop does not panic");
        assert_eq!(state, RunState::Completed);
        let records = records(&received);
        let [(_, recorded)] = records.as_slice() else {
            panic!("the child's record rolls up: {received:#?}")
        };
        assert_eq!(recorded.run_id, Some(child_run(&handles, 0)));
    });
}
