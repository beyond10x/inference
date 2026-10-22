//! Adversarial cases for `story:drain-admits-no-new-input`: input sent after a drain was asked
//! for never reaches a model, on the paths the unit's own test does not take.
//!
//! The pipeline is the flat one of `drain_admits_no_new_input.rs`: the mock model node between the
//! ingress `user text` and the egress `reply`.

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::Arc;
use std::sync::mpsc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use inference_core::endpoint::SendError;
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId, Capability, ModelId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeEndpoints, NodeError,
    NodeFuture, NodeImplementation, NodeInstanceId, NodeRole, OverloadPolicy, PipelineSpec,
    PlannedPipeline, PortTraits, modality,
};
use inference_runtime::RunController;
use inference_runtime::run::RunState;
use inference_testkit::graph::{PipelinePayload, RunLog};
use inference_testkit::llm::{MockConfiguration, MockGenerator, ModelNode};
use inference_testkit::stream::StreamEvent;
use inference_text::{SpanId, TranscriptCommitted};

const RUNS: usize = 50;
const PATIENCE: Duration = Duration::from_secs(20);
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

fn reply(input: &str) -> String {
    format!("s{input}")
}

struct Flat {
    declaration: Declaration,
    planned: PlannedPipeline,
    generator: MockGenerator,
}

fn flat() -> Flat {
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
    let capacity = NonZeroUsize::new(CAPACITY).expect("not zero");
    let mut declaration = Declaration::new();
    let spec = PipelineSpec {
        name: "model".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: vec![Capability::OutputStreaming],
    };
    let (model_id, ()) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let model = pipeline.node("model", NodeRole::Model, vec![Capability::OutputStreaming]);
            let prompt = pipeline.input::<modality::Text>(&model, "prompt", TEXT);
            let answer = pipeline.output::<modality::Text>(&model, "answer", TEXT);
            pipeline.expose(&prompt, "user text", capacity, OverloadPolicy::Pause);
            pipeline.expose(&answer, "reply", capacity, OverloadPolicy::Pause);
            pipeline.bind(&model, adapter_id).expect("known");
        })
        .expect("known");
    let validated = declaration.validate(model_id).expect("valid");
    let planned = declaration.plan(validated, &catalog).expect("planned");
    Flat {
        declaration,
        planned,
        generator,
    }
}

/// The model node, telling the test when its drain is asked for: by then the run is Draining.
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

/// How the run is drained, and how the caller sends `later` once it is Draining.
#[derive(Debug, Clone, Copy)]
enum Path {
    /// The caller drains through `RunController::drain` itself, with no run loop and no
    /// `RunRequests::drain`; `later` is sent once the run is Draining.
    ControllerDrain,
    /// The run loop drives; the caller builds the send of `later`, asks for the drain, and awaits
    /// the send once the run is Draining.
    SendBuiltBeforeTheRequest,
}

#[derive(Debug)]
struct Ended {
    state: RunState,
    committed: Vec<String>,
    later_taken: bool,
}

fn once(fixture: &Flat, path: Path) -> Ended {
    let run_id = RunId::generate();
    let (draining, drained) = mpsc::channel();
    let generator = fixture.generator.clone();
    let mut controller = RunController::create_as(
        run_id,
        &fixture.planned,
        &fixture.declaration,
        HashMap::<GenerationId, i64>::new(),
        |_: &Node| {
            Box::new(Watched {
                inner: Box::new(ModelNode::new(
                    run_id,
                    generator.clone(),
                    reply,
                    RunLog::new(),
                )),
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
    let (go, earlier_sent) = mpsc::channel::<()>();
    let driving = thread::spawn(move || {
        match path {
            Path::ControllerDrain => {
                let _sent_or_gone = earlier_sent.recv();
                let _drained = block_on(controller.drain());
                let _completed = block_on(controller.complete());
            }
            Path::SendBuiltBeforeTheRequest => {
                let _answered = block_on(controller.drive());
            }
        }
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
    let mut caller = Caller {
        run_id,
        producer: NodeId::generate(),
        stream_id: StreamId::generate(),
        sequence: 0,
    };
    block_on(ingress.send(caller.text("earlier"))).expect("the ingress has room");
    let _drain_may_start = go.send(());
    let later = match path {
        Path::ControllerDrain => {
            drained.recv_timeout(PATIENCE).expect("the run drains");
            block_on(ingress.send(caller.text("later")))
        }
        Path::SendBuiltBeforeTheRequest => {
            let pending = ingress.send(caller.text("later"));
            requests.drain();
            drained.recv_timeout(PATIENCE).expect("the run drains");
            block_on(pending)
        }
    };
    let later_taken = match later {
        Ok(()) => true,
        Err(SendError::Closed(_)) => false,
        Err(other) => panic!("the later send was refused otherwise than as closed: {other:?}"),
    };
    block_on(ingress.send(caller.stop())).expect("the end of input passes");
    let committed = reading.join().expect("the reader does not panic");
    let state = driving.join().expect("the run does not panic");
    Ended {
        state,
        committed,
        later_taken,
    }
}

fn no_later_text_reaches_the_model(path: Path) {
    let fixture = Arc::new(flat());
    let mut missed = Vec::new();
    for at in 0..RUNS {
        let (done, finished) = mpsc::channel();
        let running = Arc::clone(&fixture);
        let runner = thread::spawn(move || {
            let _ = done.send(once(&running, path));
        });
        match finished.recv_timeout(PATIENCE) {
            Ok(ended) => {
                runner.join().expect("the run does not panic");
                if ended.state != RunState::Completed
                    || ended.later_taken
                    || ended.committed.contains(&reply("later"))
                    || !ended.committed.contains(&reply("earlier"))
                {
                    missed.push(format!("{at}: {ended:?}"));
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => match runner.join() {
                Err(panic) => std::panic::resume_unwind(panic),
                Ok(()) => unreachable!("a runner that returned has sent"),
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {
                missed.push(format!("{at}: did not end within {PATIENCE:?}"));
                break;
            }
        }
    }
    assert!(
        missed.is_empty(),
        "{path:?}: {} runs let input sent after the drain reach the model or did not complete: \
         {missed:#?}",
        missed.len()
    );
}

/// Guards `Signals::run_moved` setting `input_closed` on Draining: the unit's test always asks
/// through `RunRequests::drain`, which closes the input on its own, so dropping the Draining
/// branch leaves that test green. Here the run is drained through `RunController::drain`.
#[test]
fn text_sent_into_a_run_drained_by_its_controller_never_reaches_the_model() {
    no_later_text_reaches_the_model(Path::ControllerDrain);
}

/// The guard reads `admits_input` when `send` is called, not when its future is first polled:
/// a send built before the drain request and awaited once the run is Draining is admitted.
#[test]
fn a_send_awaited_only_after_the_drain_request_never_reaches_the_model() {
    no_later_text_reaches_the_model(Path::SendBuiltBeforeTheRequest);
}
