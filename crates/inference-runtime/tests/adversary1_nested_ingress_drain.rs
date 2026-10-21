//! Adversary pass 1 of `story:subgraph-nested-ingress-drain`: the drain of a subgraph node asks
//! its child run for a drain only once every ingress bridge has ended, at every nesting level.
//!
//! The pipelines are those of `subgraph_nested_drain.rs`: the mock model node between the ingress
//! `user text` and the egress `reply`, and above it subgraph nodes over the level below, their
//! ports exposed under the same names. These cases reach what the story's case does not:
//!
//! - a backlog: many texts queued at the top, each child's ingress holding one envelope, so the
//!   drain flag is set while a bridge still has envelopes on its edge to forward;
//! - an edge that closes without `stop` and a drain the caller asks for: no `stop` reaches the
//!   child, so only the drain the subgraph node asks for ends the child's input, which it asks
//!   for once the count of open ingress bridges reaches zero;
//! - text sent after `stop` on the same stream: never admitted;
//! - a cancel while the nested drain waits for its bridges: the run ends, it does not hang.

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

/// How many times each case is run.
const RUNS: usize = 20;

/// How long one run may take before it counts as hung.
const PATIENCE: Duration = Duration::from_secs(20);

/// The texts of a backlog.
const BACKLOG: usize = 32;

/// How long a jammed run is left draining before the cancel.
const JAM: Duration = Duration::from_millis(100);

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

/// `depth` subgraph levels over the model; the ingress of every level below the top holds
/// `inner` envelopes, the top's `top`, every egress `egress`.
fn nesting(depth: usize, inner: usize, top: usize, egress: usize) -> Nesting {
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
    let ingress = |level: usize| n(if level == depth { top } else { inner });
    let mut declaration = Declaration::new();
    let (model_id, ()) = declaration
        .build(&catalog, profile_id, spec("model"), |pipeline| {
            let model = pipeline.node("model", NodeRole::Model, vec![Capability::OutputStreaming]);
            let prompt = pipeline.input::<modality::Text>(&model, "prompt", TEXT);
            let answer = pipeline.output::<modality::Text>(&model, "answer", TEXT);
            pipeline.expose(&prompt, "user text", ingress(0), OverloadPolicy::Pause);
            pipeline.expose(&answer, "reply", n(egress), OverloadPolicy::Pause);
            pipeline.bind(&model, adapter_id).expect("known");
        })
        .expect("known");
    let mut levels = vec![plan(&mut declaration, &catalog, model_id)];
    let mut below = model_id;
    for level in 1..=depth {
        let (over, ()) = declaration
            .build(&catalog, profile_id, spec("over"), |pipeline| {
                let subgraph = pipeline
                    .subgraph("level", NodeRole::Transform, below)
                    .expect("the level below is Planned");
                let into = pipeline.input::<modality::Text>(&subgraph, "user text", TEXT);
                let out = pipeline.output::<modality::Text>(&subgraph, "reply", TEXT);
                pipeline.expose(&into, "user text", ingress(level), OverloadPolicy::Pause);
                pipeline.expose(&out, "reply", n(egress), OverloadPolicy::Pause);
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

/// How the caller ends its input after the texts.
#[derive(Debug, Clone, Copy)]
enum End {
    /// `stop` on the ingress, which ends the top run's input.
    Stop,
    /// `stop` with nobody reading the egress, so back-pressure holds every bridge in a send and
    /// the top run's drain waits on them; after [`JAM`] a cancel, and only then the egress read.
    JamThenCancel,
    /// `stop`, then one more text on the same stream.
    StopThenText,
    /// The ingress dropped without `stop`, then a drain of the top run asked for.
    CloseThenDrain,
}

#[derive(Debug)]
struct Ended {
    state: RunState,
    committed: Vec<String>,
    /// For [`End::JamThenCancel`]: whether the run loop was still running when the cancel was
    /// asked for.
    running_at_cancel: Option<bool>,
}

fn once(nesting: &Nesting, texts: usize, end: End) -> Ended {
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
    let requests = controller.requests();
    let drive = thread::spawn(move || {
        let _answered = block_on(controller.drive());
        controller.run().state()
    });
    let read = move || {
        let mut committed = Vec::new();
        while let Some(envelope) = block_on(egress.recv()) {
            if let PipelinePayload::Stream(StreamEvent::TextSegmentCommitted(segment)) =
                envelope.payload
            {
                committed.push(segment.text);
            }
        }
        committed
    };
    let mut running_at_cancel = None;
    let mut read = Some(read);
    let mut reading = None;
    if !matches!(end, End::JamThenCancel) {
        reading = read.take().map(thread::spawn);
    }
    let (producer, stream_id) = (NodeId::generate(), StreamId::generate());
    let mut sequence = 0;
    let mut next = || {
        sequence += 1;
        sequence
    };
    for at in 1..=texts {
        block_on(ingress.send(envelope(
            run_id,
            producer,
            stream_id,
            next(),
            text(&at.to_string()),
        )))
        .expect("the top ingress has room");
    }
    let stop = PipelinePayload::Control(ControlKind::Stop);
    match end {
        End::Stop => {
            block_on(ingress.send(envelope(run_id, producer, stream_id, next(), stop)))
                .expect("taken");
        }
        End::JamThenCancel => {
            block_on(ingress.send(envelope(run_id, producer, stream_id, next(), stop)))
                .expect("taken");
            thread::sleep(JAM);
            running_at_cancel = Some(!drive.is_finished());
            requests.cancel();
            reading = read.take().map(thread::spawn);
        }
        End::StopThenText => {
            block_on(ingress.send(envelope(run_id, producer, stream_id, next(), stop)))
                .expect("taken");
            let _taken_or_closed = block_on(ingress.send(envelope(
                run_id,
                producer,
                stream_id,
                next(),
                text("after stop"),
            )));
        }
        End::CloseThenDrain => {
            drop(ingress);
            requests.drain();
        }
    }
    let committed = reading
        .expect("the reader runs")
        .join()
        .expect("the reader does not panic");
    let state = drive.join().expect("the loop does not panic");
    Ended {
        state,
        committed,
        running_at_cancel,
    }
}

/// Runs `once` up to [`RUNS`] times, each bounded by [`PATIENCE`], and answers each run's
/// outcome, or none for a run that hung, after which it runs no more.
fn repeated(nesting: &Arc<Nesting>, texts: usize, end: End) -> Vec<Option<Ended>> {
    let mut outcomes = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let (done, finished) = mpsc::channel();
        let running = Arc::clone(nesting);
        let runner = thread::spawn(move || {
            let _ = done.send(once(&running, texts, end));
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

fn every_text_of_a_backlog_reaches_the_model(depth: usize) {
    let nesting = Arc::new(nesting(depth, 1, 64, 64));
    let expected: Vec<String> = (1..=BACKLOG).map(|at| reply(&at.to_string())).collect();
    let missed: Vec<String> = repeated(&nesting, BACKLOG, End::Stop)
        .into_iter()
        .enumerate()
        .filter_map(|(at, ended)| match ended {
            None => Some(format!("{at}: did not end within {PATIENCE:?}")),
            Some(ended) if ended.state != RunState::Completed || ended.committed != expected => {
                Some(format!(
                    "{at}: {:?}, {} of {BACKLOG} answered",
                    ended.state,
                    ended.committed.len()
                ))
            }
            Some(_) => None,
        })
        .collect();
    assert!(
        missed.is_empty(),
        "{depth} levels, a backlog of {BACKLOG} texts through ingresses holding one each, then \
         stop: lost or not completed in {} of {RUNS} runs: {missed:#?}",
        missed.len()
    );
}

/// A drain flag set while the top bridge still holds most of a backlog on its edge: each bridge
/// forwards every envelope its edge holds before it sends `stop`, and the child below drains only
/// after that, so every text is answered, in order.
#[test]
fn a_backlog_behind_full_child_ingresses_is_answered_in_full_two_subgraphs_deep() {
    every_text_of_a_backlog_reaches_the_model(2);
}

#[test]
fn a_backlog_behind_full_child_ingresses_is_answered_in_full_three_subgraphs_deep() {
    every_text_of_a_backlog_reaches_the_model(3);
}

/// The caller drops its ingress without `stop` and asks for the top run's drain: no `stop`
/// crosses any boundary, so each child's input ends only through the drain its subgraph node
/// asks for once its ingress bridge ended. The text still reaches the model and the run
/// completes, at two and three levels.
#[test]
fn an_edge_closed_without_stop_then_drained_completes_at_every_depth() {
    for depth in [2, 3] {
        let nesting = Arc::new(nesting(depth, 16, 16, 16));
        let missed: Vec<String> = repeated(&nesting, 1, End::CloseThenDrain)
            .into_iter()
            .enumerate()
            .filter_map(|(at, ended)| match ended {
                None => Some(format!("{at}: did not end within {PATIENCE:?}")),
                Some(ended)
                    if ended.state != RunState::Completed || ended.committed != [reply("1")] =>
                {
                    Some(format!("{at}: {ended:?}"))
                }
                Some(_) => None,
            })
            .collect();
        assert!(
            missed.is_empty(),
            "{depth} levels, the ingress closed without stop, then a drain: {} of {RUNS} runs \
             hung, lost the text or did not complete: {missed:#?}",
            missed.len()
        );
    }
}

/// Text sent after `stop` on the same stream is not input the run admitted: it never reaches the
/// model, which answers the text before `stop` only.
#[test]
fn text_after_stop_is_never_admitted_two_subgraphs_deep() {
    let nesting = Arc::new(nesting(2, 16, 16, 16));
    let missed: Vec<String> = repeated(&nesting, 1, End::StopThenText)
        .into_iter()
        .enumerate()
        .filter_map(|(at, ended)| match ended {
            None => Some(format!("{at}: did not end within {PATIENCE:?}")),
            Some(ended)
                if ended.state != RunState::Completed || ended.committed != [reply("1")] =>
            {
                Some(format!("{at}: {ended:?}"))
            }
            Some(_) => None,
        })
        .collect();
    assert!(
        missed.is_empty(),
        "{} of {RUNS} runs: {missed:#?}",
        missed.len()
    );
}

/// A cancel of the top run while the subgraph node's drain waits for an ingress bridge that
/// back-pressure holds: every egress holds one envelope and nobody reads the top one, so the
/// model blocks, every ingress below fills and the top bridge cannot end. The loop is still
/// running when the cancel is asked for, and the cancel ends the run Cancelled, three levels deep.
#[test]
fn a_cancel_while_the_nested_drain_waits_for_its_bridges_ends_the_run_three_subgraphs_deep() {
    let nesting = Arc::new(nesting(3, 1, 64, 1));
    let missed: Vec<String> = repeated(&nesting, BACKLOG, End::JamThenCancel)
        .into_iter()
        .enumerate()
        .filter_map(|(at, ended)| match ended {
            None => Some(format!("{at}: did not end within {PATIENCE:?}")),
            Some(ended)
                if ended.state != RunState::Cancelled || ended.running_at_cancel != Some(true) =>
            {
                Some(format!(
                    "{at}: {:?}, running at the cancel: {:?}",
                    ended.state, ended.running_at_cancel
                ))
            }
            Some(_) => None,
        })
        .collect();
    assert!(
        missed.is_empty(),
        "{} of {RUNS} runs: {missed:#?}",
        missed.len()
    );
}
