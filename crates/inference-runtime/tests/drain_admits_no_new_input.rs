//! Input sent after a drain was requested never reaches a model
//! (`story:drain-admits-no-new-input`).
//!
//! `inference.graph.DrainRun` admits no new input; in-flight events complete. The caller of a run
//! whose pipeline exposes the ingress `user text` sends one text, asks for the run's drain without
//! sending `stop`, and keeps sending texts, then ends its input with `stop`. The text sent before
//! the request was admitted and must be answered; none sent after the request may reach the model,
//! whose answers are read off the egress `reply`. The caller sends the later texts either at once
//! after the request, while the run may still be Active, or once the run is Draining, which it
//! reads off the first drain of the run's top node, since the run moves to Draining before it
//! drains its first instance.
//!
//! The pipelines are those of `subgraph_nested_drain.rs`: the mock model node between the ingress
//! and the egress, flat, and two subgraph levels over it, each exposing its ports under the same
//! names. Each case is run [`RUNS`] times, each run bounded by [`PATIENCE`]; every run in which a
//! later text reached the model, the earlier one did not, the run ended otherwise than Completed or
//! it hung is counted.

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::Arc;
use std::sync::mpsc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId, Capability, ModelId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeEndpoints, NodeError,
    NodeFuture, NodeImplementation, NodeInstanceId, NodeRole, OverloadPolicy, PipelineId,
    PipelineSpec, PlannedPipeline, PortTraits, modality,
};
use inference_runtime::run::RunState;
use inference_runtime::{ChildImplementations, RunController, SubgraphNode};
use inference_testkit::graph::{PipelinePayload, RunLog};
use inference_testkit::llm::{MockConfiguration, MockGenerator, ModelNode};
use inference_testkit::stream::StreamEvent;
use inference_text::{SpanId, TranscriptCommitted};

/// How many times each case is run.
const RUNS: usize = 200;

/// How long one run may take before it counts as hung.
const PATIENCE: Duration = Duration::from_secs(20);

/// The texts the caller sends after its drain request.
const LATER: usize = 3;

/// The capacity of every exposure.
const CAPACITY: usize = 16;

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

fn n(capacity: usize) -> NonZeroUsize {
    NonZeroUsize::new(capacity).expect("not zero")
}

fn reply(input: &str) -> String {
    format!("s{input}")
}

struct Nesting {
    declaration: Arc<Declaration>,
    levels: Arc<Vec<PlannedPipeline>>,
    generator: MockGenerator,
}

/// `depth` subgraph levels over the model, none for the model alone.
fn nesting(depth: usize) -> Nesting {
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
    let (model_id, ()) = declaration
        .build(&catalog, profile_id, spec("model"), |pipeline| {
            let model = pipeline.node("model", NodeRole::Model, vec![Capability::OutputStreaming]);
            let prompt = pipeline.input::<modality::Text>(&model, "prompt", TEXT);
            let answer = pipeline.output::<modality::Text>(&model, "answer", TEXT);
            pipeline.expose(&prompt, "user text", n(CAPACITY), OverloadPolicy::Pause);
            pipeline.expose(&answer, "reply", n(CAPACITY), OverloadPolicy::Pause);
            pipeline.bind(&model, adapter_id).expect("known");
        })
        .expect("known");
    let mut levels = vec![plan(&mut declaration, &catalog, model_id)];
    let mut below = model_id;
    for _ in 1..=depth {
        let (over, ()) = declaration
            .build(&catalog, profile_id, spec("over"), |pipeline| {
                let subgraph = pipeline
                    .subgraph("level", NodeRole::Transform, below)
                    .expect("the level below is Planned");
                let into = pipeline.input::<modality::Text>(&subgraph, "user text", TEXT);
                let out = pipeline.output::<modality::Text>(&subgraph, "reply", TEXT);
                pipeline.expose(&into, "user text", n(CAPACITY), OverloadPolicy::Pause);
                pipeline.expose(&out, "reply", n(CAPACITY), OverloadPolicy::Pause);
            })
            .expect("known");
        levels.push(plan(&mut declaration, &catalog, over));
        below = over;
    }
    Nesting {
        declaration: Arc::new(declaration),
        levels: Arc::new(levels),
        generator,
    }
}

fn plan(declaration: &mut Declaration, catalog: &Fixture, pipeline: PipelineId) -> PlannedPipeline {
    let validated = declaration.validate(pipeline).expect("valid");
    declaration.plan(validated, catalog).expect("planned")
}

fn nodes(nesting: &Nesting, level: usize) -> ChildImplementations<PipelinePayload> {
    let declaration = Arc::clone(&nesting.declaration);
    let levels = Arc::clone(&nesting.levels);
    let generator = nesting.generator.clone();
    Box::new(move |run_id: RunId, _: &Node| {
        if level == 0 {
            return Box::new(ModelNode::new(
                run_id,
                generator.clone(),
                reply,
                RunLog::new(),
            )) as Box<dyn NodeImplementation<PipelinePayload>>;
        }
        let below = Nesting {
            declaration: Arc::clone(&declaration),
            levels: Arc::clone(&levels),
            generator: generator.clone(),
        };
        Box::new(SubgraphNode::new(
            run_id,
            &declaration,
            &levels[level - 1],
            HashMap::<GenerationId, i64>::new(),
            nodes(&below, level - 1),
        ))
    })
}

/// The top run's node, telling the test when its drain is asked for: by then the run is Draining.
struct Watched {
    inner: Box<dyn NodeImplementation<PipelinePayload>>,
    draining: mpsc::Sender<()>,
}

impl NodeImplementation<PipelinePayload> for Watched {
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.inner.describe(node)
    }

    fn described(&mut self, instance_id: NodeInstanceId) {
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
        let _test_waits_or_gone = self.draining.send(());
        self.inner.drain()
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        self.inner.cancel()
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        self.inner.stop()
    }
}

fn envelope(
    run_id: RunId,
    producer: NodeId,
    stream_id: StreamId,
    sequence: i64,
    payload: PipelinePayload,
) -> EventEnvelope<PipelinePayload> {
    let kind = match &payload {
        PipelinePayload::Stream(event) => event.name().to_owned(),
        PipelinePayload::Control(kind) => kind.as_str().to_owned(),
        PipelinePayload::Usage(_) => unreachable!("the caller sends no usage"),
    };
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id,
        session_id: None,
        stream_id,
        sequence,
        producer,
        port: "user text".to_owned(),
        kind,
        generation_id: None,
        timestamp: 0,
        timebase: Timebase::WallTime,
        causation_id: None,
        payload,
    }
}

fn text(content: &str) -> PipelinePayload {
    PipelinePayload::Stream(StreamEvent::TranscriptCommitted(TranscriptCommitted {
        span_id: SpanId::generate(),
        text: content.to_owned(),
    }))
}

/// When the caller sends its later texts.
#[derive(Debug, Clone, Copy)]
enum Later {
    /// At once after its drain request returned.
    Requested,
    /// Once the run is Draining.
    Draining,
}

#[derive(Debug)]
struct Ended {
    state: RunState,
    /// The answers the egress delivered, in order.
    committed: Vec<String>,
    /// How many later texts the ingress took.
    taken: usize,
    /// Whether the end of input, sent after the later texts, was taken. A flat run must take it:
    /// the model's drain ends on it, so a refused `stop` would hold the run. Through subgraph
    /// levels the top bridge may have ended at the drain, sending the child its own `stop`, and
    /// its edge then refuses the caller's as closed.
    stop_taken: bool,
}

impl Ended {
    /// How many later texts the model answered.
    fn answered_later(&self) -> usize {
        (1..=LATER)
            .filter(|at| self.committed.contains(&reply(&format!("later {at}"))))
            .count()
    }
}

fn once(nesting: &Nesting, later: Later) -> Ended {
    let top = nesting.levels.len() - 1;
    let run_id = RunId::generate();
    let mut implement = nodes(nesting, top);
    let (draining, drained) = mpsc::channel();
    let mut controller = RunController::create_as(
        run_id,
        &nesting.levels[top],
        &nesting.declaration,
        HashMap::<GenerationId, i64>::new(),
        |node: &Node| {
            Box::new(Watched {
                inner: implement(run_id, node),
                draining: draining.clone(),
            }) as Box<dyn NodeImplementation<PipelinePayload>>
        },
    )
    .expect("planned");
    drop(draining);
    controller.validate().expect("Created");
    block_on(controller.prepare()).expect("prepares");
    let mut ends = controller.exposed();
    let ingress = ends.ingress("user text").expect("the ingress");
    let mut egress = ends.egress("reply").expect("the egress");
    drop(ends);
    block_on(controller.activate()).expect("activates");
    let requests = controller.requests();
    let drive = thread::spawn(move || {
        let _answered = block_on(controller.drive());
        controller.run().state()
    });
    let reading = thread::spawn(move || {
        let mut committed = Vec::new();
        while let Some(envelope) = block_on(egress.recv()) {
            if let PipelinePayload::Stream(StreamEvent::TextSegmentCommitted(segment)) =
                envelope.payload
            {
                committed.push(segment.text);
            }
        }
        committed
    });
    let (producer, stream_id) = (NodeId::generate(), StreamId::generate());
    let mut sequence = 0;
    let mut next = || {
        sequence += 1;
        sequence
    };
    block_on(ingress.send(envelope(
        run_id,
        producer,
        stream_id,
        next(),
        text("earlier"),
    )))
    .expect("the ingress has room");
    requests.drain();
    if matches!(later, Later::Draining) {
        drained
            .recv_timeout(PATIENCE)
            .expect("the run drains its top node");
    }
    let mut taken = 0;
    for at in 1..=LATER {
        let sent = block_on(ingress.send(envelope(
            run_id,
            producer,
            stream_id,
            next(),
            text(&format!("later {at}")),
        )));
        taken += usize::from(sent.is_ok());
    }
    let stop_taken = block_on(ingress.send(envelope(
        run_id,
        producer,
        stream_id,
        next(),
        PipelinePayload::Control(ControlKind::Stop),
    )))
    .is_ok();
    let committed = reading.join().expect("the reader does not panic");
    let state = drive.join().expect("the loop does not panic");
    Ended {
        state,
        committed,
        taken,
        stop_taken,
    }
}

/// Runs `once` [`RUNS`] times, each bounded by [`PATIENCE`], and answers each run's outcome, or
/// none for a run that hung, after which it runs no more.
fn repeated(nesting: &Arc<Nesting>, later: Later) -> Vec<Option<Ended>> {
    let mut outcomes = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let (done, finished) = mpsc::channel();
        let running = Arc::clone(nesting);
        let runner = thread::spawn(move || {
            let _ = done.send(once(&running, later));
        });
        match finished.recv_timeout(PATIENCE) {
            Ok(ended) => {
                runner.join().expect("the run does not panic");
                outcomes.push(Some(ended));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => match runner.join() {
                Err(panic) => std::panic::resume_unwind(panic),
                Ok(()) => unreachable!("a runner that returned has sent"),
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {
                outcomes.push(None);
                break;
            }
        }
    }
    outcomes
}

fn no_later_text_reaches_the_model(depth: usize, later: Later) {
    let nesting = Arc::new(nesting(depth));
    let outcomes = repeated(&nesting, later);
    let reached: usize = outcomes.iter().flatten().map(Ended::answered_later).sum();
    let taken: usize = outcomes.iter().flatten().map(|ended| ended.taken).sum();
    let missed: Vec<String> = outcomes
        .iter()
        .enumerate()
        .filter_map(|(at, ended)| match ended {
            None => Some(format!("{at}: did not end within {PATIENCE:?}")),
            Some(ended)
                if ended.state != RunState::Completed
                    || ended.answered_later() != 0
                    || ended.taken != 0
                    || !ended.committed.contains(&reply("earlier"))
                    || (depth == 0 && !ended.stop_taken) =>
            {
                Some(format!("{at}: {ended:?}"))
            }
            Some(_) => None,
        })
        .collect();
    assert!(
        missed.is_empty(),
        "{depth} subgraph levels, {LATER} texts sent {later:?}: {reached} of {} reached the model \
         ({taken} taken by the ingress); {} of {} runs failed: {missed:#?}",
        LATER * outcomes.len(),
        missed.len(),
        outcomes.len()
    );
}

#[test]
fn text_sent_after_the_drain_request_never_reaches_the_model_flat() {
    no_later_text_reaches_the_model(0, Later::Requested);
}

#[test]
fn text_sent_while_draining_never_reaches_the_model_flat() {
    no_later_text_reaches_the_model(0, Later::Draining);
}

#[test]
fn text_sent_after_the_drain_request_never_reaches_the_model_two_subgraphs_deep() {
    no_later_text_reaches_the_model(2, Later::Requested);
}

#[test]
fn text_sent_while_draining_never_reaches_the_model_two_subgraphs_deep() {
    no_later_text_reaches_the_model(2, Later::Draining);
}
