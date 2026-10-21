//! Input sent before `stop` is never overtaken by the drain at a nested subgraph's ingress
//! (`story:subgraph-nested-ingress-drain`).
//!
//! The innermost pipeline is the mock model node alone between the ingress `user text` and the
//! egress `reply`; each level above it is a subgraph node over the level below, its ports `user
//! text` and `reply` exposed under their names. The caller sends one text and `stop` back to back
//! into the top run's ingress, so the top run's input ends at once and its drain starts while the
//! text may still be crossing the boundaries below. `inference.graph.DrainRun` admits no new input
//! but lets in-flight events complete, and the text was admitted before `stop`: it must reach the
//! model, whose answer crosses back to the top egress, and the run must complete. Each depth is
//! run [`RUNS`] times, each run bounded by [`PATIENCE`], and every run that loses the text, ends
//! otherwise than Completed or hangs is counted.

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
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeImplementation,
    NodeRole, OverloadPolicy, PipelineId, PipelineSpec, PlannedPipeline, PortTraits, modality,
};
use inference_runtime::run::RunState;
use inference_runtime::{ChildImplementations, RunController, SubgraphNode};
use inference_testkit::graph::{PipelinePayload, RunLog};
use inference_testkit::llm::{MockConfiguration, MockGenerator, ModelNode};
use inference_testkit::stream::StreamEvent;
use inference_text::{SpanId, TranscriptCommitted};

/// How many times each depth is run.
const RUNS: usize = 200;

/// How long one run may take before it counts as hung.
const PATIENCE: Duration = Duration::from_secs(10);

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

fn capacity() -> NonZeroUsize {
    NonZeroUsize::new(16).expect("16")
}

fn reply(input: &str) -> String {
    format!("s{input}")
}

/// The planned pipelines of one nesting: the model's at index 0, and at index `k` the subgraph
/// node over index `k - 1`.
struct Nesting {
    declaration: Arc<Declaration>,
    levels: Arc<Vec<PlannedPipeline>>,
    generator: MockGenerator,
}

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
            pipeline.expose(&prompt, "user text", capacity(), OverloadPolicy::Pause);
            pipeline.expose(&answer, "reply", capacity(), OverloadPolicy::Pause);
            pipeline.bind(&model, adapter_id).expect("known");
        })
        .expect("known");
    let mut levels = vec![plan(&mut declaration, &catalog, model_id)];
    let mut below = model_id;
    for _ in 0..depth {
        let (over, ()) = declaration
            .build(&catalog, profile_id, spec("over"), |pipeline| {
                let subgraph = pipeline
                    .subgraph("level", NodeRole::Transform, below)
                    .expect("the level below is Planned");
                let into = pipeline.input::<modality::Text>(&subgraph, "user text", TEXT);
                let out = pipeline.output::<modality::Text>(&subgraph, "reply", TEXT);
                pipeline.expose(&into, "user text", capacity(), OverloadPolicy::Pause);
                pipeline.expose(&out, "reply", capacity(), OverloadPolicy::Pause);
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

/// The node of a run of the level `level`: the model at level 0, else a subgraph node over the
/// level below, whose child's node is made the same way.
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

/// What one run came to: the run's end state and the text of every committed segment the top
/// egress delivered.
#[derive(Debug)]
struct Ended {
    state: RunState,
    committed: Vec<String>,
}

/// Runs the top level of `nesting` once: sends the text `1` and `stop` back to back into its
/// ingress, reads its egress until it closes and waits for the run's loop to return.
fn once(nesting: &Nesting) -> Ended {
    let top = nesting.levels.len() - 1;
    let run_id = RunId::generate();
    let mut implement = nodes(nesting, top);
    let mut controller = RunController::create_as(
        run_id,
        &nesting.levels[top],
        &nesting.declaration,
        HashMap::<GenerationId, i64>::new(),
        |node: &Node| implement(run_id, node),
    )
    .expect("planned");
    controller.validate().expect("Created");
    block_on(controller.prepare()).expect("prepares");
    let mut ends = controller.exposed();
    let ingress = ends.ingress("user text").expect("the ingress");
    let mut egress = ends.egress("reply").expect("the egress");
    drop(ends);
    block_on(controller.activate()).expect("activates");
    let drive = thread::spawn(move || {
        let _answered = block_on(controller.drive());
        controller.run().state()
    });
    let (producer, stream_id) = (NodeId::generate(), StreamId::generate());
    let text = PipelinePayload::Stream(StreamEvent::TranscriptCommitted(TranscriptCommitted {
        span_id: SpanId::generate(),
        text: "1".to_owned(),
    }));
    let stop = PipelinePayload::Control(ControlKind::Stop);
    block_on(ingress.send(envelope(run_id, producer, stream_id, 1, text))).expect("taken");
    block_on(ingress.send(envelope(run_id, producer, stream_id, 2, stop))).expect("taken");
    let mut committed = Vec::new();
    while let Some(envelope) = block_on(egress.recv()) {
        if let PipelinePayload::Stream(StreamEvent::TextSegmentCommitted(segment)) =
            envelope.payload
        {
            committed.push(segment.text);
        }
    }
    let state = drive.join().expect("the loop does not panic");
    Ended { state, committed }
}

/// Runs `depth` levels of subgraph nodes over the model [`RUNS`] times and asserts that every run
/// delivered the model's answer to the text and completed.
fn every_run_answers_and_completes(depth: usize) {
    let nesting = Arc::new(nesting(depth));
    let mut missed: Vec<(usize, String)> = Vec::new();
    for at in 0..RUNS {
        let (done, finished) = mpsc::channel();
        let running = Arc::clone(&nesting);
        let runner = thread::spawn(move || {
            let _ = done.send(once(&running));
        });
        match finished.recv_timeout(PATIENCE) {
            Ok(ended) => {
                runner.join().expect("the run does not panic");
                if ended.state != RunState::Completed || ended.committed != [reply("1")] {
                    missed.push((at, format!("{ended:?}")));
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if let Err(panic) = runner.join() {
                    std::panic::resume_unwind(panic);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                missed.push((at, format!("did not end within {PATIENCE:?}")));
            }
        }
    }
    let first: Vec<_> = missed.iter().take(3).collect();
    assert!(
        missed.is_empty(),
        "{depth} levels: the text was lost or the run did not complete in {} of {RUNS} runs; \
         first: {first:#?}",
        missed.len()
    );
}

/// Acceptance: through two subgraph levels, text sent immediately before `stop` reaches the inner
/// model in every run, and the run completes.
#[test]
fn text_sent_just_before_stop_reaches_the_model_two_subgraphs_deep() {
    every_run_answers_and_completes(2);
}

/// The same three subgraph levels deep: each boundary hands on what it took before the drain
/// below it acts.
#[test]
fn text_sent_just_before_stop_reaches_the_model_three_subgraphs_deep() {
    every_run_answers_and_completes(3);
}
