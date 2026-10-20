//! A run hands the outer ends of its pipeline's exposures to its caller
//! (`story:pipeline-exposed-ports`, design line 511): the A-02 model node runs between an exposed
//! ingress and an exposed egress with no source or sink node. The test is the application: it
//! sends text and the end of input, a control event of kind `stop`, into the ingress, and reads
//! the reply from the egress until the node's end is gone; the run loop drains on that `stop` as
//! it does on a source node's, and completes. Every node instance is then Stopped and every queue
//! of the run, the exposures' among them, is dropped. A run cancelled while its caller never took
//! its ends drops those ends with the run.

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

use inference_core::endpoint::{InputEndpoint, OutputEndpoint};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId, Capability, ModelId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeRole, OverloadPolicy,
    PipelineSpec, PlannedPipeline, PortTraits, modality,
};
use inference_runtime::RunController;
use inference_runtime::run::{NodeInstanceState, RunState};
use inference_testkit::graph::{PipelinePayload, RunLog};
use inference_testkit::llm::{MockConfiguration, MockGenerator, ModelNode};
use inference_testkit::stream::StreamEvent;
use inference_text::{SpanId, TranscriptCommitted};

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Polls `future` on this thread until it completes: the runtime commits to no executor.
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

const INGRESS: &str = "user text";
const EGRESS: &str = "reply";

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

/// The model node alone, its `prompt` exposed as the ingress `user text` and its `answer` as the
/// egress `reply`, declared with the typed builder, validated and planned.
fn planned(generator: &MockGenerator) -> (Declaration, PlannedPipeline, NodeId) {
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
    let text = PortTraits {
        essentiality: Essentiality::Essential,
        delivery: Delivery::Reliable,
        incremental: true,
        end_marker_required: true,
        provisional_allowed: false,
    };
    let spec = PipelineSpec {
        name: "exposed model".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: vec![Capability::OutputStreaming],
    };
    let mut declaration = Declaration::new();
    let (pipeline_id, model) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let model = pipeline.node("model", NodeRole::Model, vec![Capability::OutputStreaming]);
            let prompt = pipeline.input::<modality::Text>(&model, "prompt", text);
            let answer = pipeline.output::<modality::Text>(&model, "answer", text);
            pipeline.expose(
                &prompt,
                INGRESS,
                NonZeroUsize::new(4).expect("4"),
                OverloadPolicy::Pause,
            );
            pipeline.expose(
                &answer,
                EGRESS,
                NonZeroUsize::new(16).expect("16"),
                OverloadPolicy::Fail,
            );
            pipeline
                .bind(&model, adapter_id)
                .expect("the catalog holds the mock LLM's adapter");
            model.id()
        })
        .expect("the profile is declared");
    let validated = declaration
        .validate(pipeline_id)
        .expect("a model between an ingress and an egress is valid");
    let planned = declaration
        .plan(validated, &catalog)
        .expect("the model is bound to an adapter that streams output");
    (declaration, planned, model)
}

/// A run of `planned` whose one node is the mock model answering on `generator`, validated and
/// prepared.
fn prepared(
    declaration: &Declaration,
    planned: &PlannedPipeline,
    generator: &MockGenerator,
) -> (RunController<PipelinePayload>, RunId) {
    let run_id = RunId::generate();
    let model = ModelNode::new(run_id, generator.clone(), reply, RunLog::new());
    let mut model = Some(model);
    let mut controller = RunController::create_as(
        run_id,
        planned,
        declaration,
        HashMap::<GenerationId, i64>::new(),
        |_: &Node| Box::new(model.take().expect("the pipeline has one node")),
    )
    .expect("the pipeline is planned and neither exposure cancels or supersedes");
    controller.validate().expect("a Created run validates");
    block_on(controller.prepare()).expect("the model prepares");
    (controller, run_id)
}

/// What the application sends on the ingress: an envelope of its own stream, numbered from 1.
struct Application {
    run_id: RunId,
    producer: NodeId,
    stream_id: StreamId,
    sequence: i64,
}

impl Application {
    fn envelope(
        &mut self,
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
            port: INGRESS.to_owned(),
            kind,
            generation_id: None,
            timestamp: 0,
            timebase: Timebase::WallTime,
            causation_id: None,
            payload,
        }
    }

    fn text(&mut self, text: &str) -> EventEnvelope<PipelinePayload> {
        let event = StreamEvent::TranscriptCommitted(TranscriptCommitted {
            span_id: SpanId::generate(),
            text: text.to_owned(),
        });
        self.envelope(event.name().to_owned(), PipelinePayload::Stream(event))
    }

    fn stop(&mut self) -> EventEnvelope<PipelinePayload> {
        self.envelope(
            ControlKind::Stop.as_str().to_owned(),
            PipelinePayload::Control(ControlKind::Stop),
        )
    }
}

#[test]
fn the_model_runs_between_an_exposed_ingress_and_egress_to_completed_and_every_queue_is_dropped() {
    let generator = generator();
    let (declaration, planned, model) = planned(&generator);
    let (mut controller, run_id) = prepared(&declaration, &planned, &generator);
    let mut ends = controller.exposed();
    let ingress: Box<dyn OutputEndpoint<PipelinePayload>> = ends
        .ingress(INGRESS)
        .expect("the run hands the caller the ingress");
    let mut egress: Box<dyn InputEndpoint<PipelinePayload>> = ends
        .egress(EGRESS)
        .expect("the run hands the caller the egress");
    assert!(
        controller.exposed().ingresses.is_empty(),
        "the ends are handed out once"
    );
    block_on(controller.activate()).expect("the model starts with the inner ends");
    assert_eq!(controller.run().state(), RunState::Active);
    let queues = controller.queues();
    let exposure_queues = controller.exposure_queues();
    assert!(queues.is_empty(), "the pipeline has no edge");
    assert_eq!(exposure_queues.len(), 2, "one queue per exposure");
    let drive = thread::spawn(move || {
        let result = block_on(controller.drive());
        (controller, result)
    });

    let mut application = Application {
        run_id,
        producer: NodeId::generate(),
        stream_id: StreamId::generate(),
        sequence: 0,
    };
    for input in ["1", "2"] {
        block_on(ingress.send(application.text(input))).expect("the ingress takes the text");
    }
    block_on(ingress.send(application.stop())).expect("the ingress takes the end of input");
    let mut received = Vec::new();
    while let Some(envelope) = block_on(egress.recv()) {
        received.push(envelope);
    }
    drop(ingress);
    drop(egress);
    drop(ends);
    let (controller, result) = drive.join().expect("the drive loop does not panic");
    result.expect("the drive loop ends the run");

    assert_eq!(
        controller.run().state(),
        RunState::Completed,
        "the caller's stop on the ingress ended the run's input"
    );
    let committed: Vec<String> = received
        .iter()
        .filter_map(|envelope| match &envelope.payload {
            PipelinePayload::Stream(StreamEvent::TextSegmentCommitted(committed)) => {
                Some(committed.text.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(committed, ["s1", "s2"], "the reply read from the egress");
    assert!(
        received
            .last()
            .is_some_and(|envelope| envelope.kind == ControlKind::Stop.as_str()),
        "the end of input reaches the egress last"
    );
    assert!(
        received
            .iter()
            .all(|envelope| envelope.run_id == run_id && envelope.producer == model),
        "the egress carries what the model emitted in the run"
    );
    assert_eq!(
        controller
            .instance(model)
            .map(inference_runtime::NodeInstance::state),
        Some(NodeInstanceState::Stopped)
    );
    assert_eq!(controller.refusals(), []);
    for queue in &exposure_queues {
        assert!(
            queue.is_dropped(),
            "the queue of exposure {} is dropped",
            queue.exposure_id()
        );
    }
}

#[test]
fn a_run_cancelled_while_its_caller_never_took_its_ends_drops_every_exposure_queue() {
    let generator = generator();
    let (declaration, planned, model) = planned(&generator);
    let (mut controller, _) = prepared(&declaration, &planned, &generator);
    block_on(controller.activate()).expect("the model starts");
    let exposure_queues = controller.exposure_queues();
    assert_eq!(exposure_queues.len(), 2);
    assert!(
        exposure_queues.iter().all(|queue| !queue.is_dropped()),
        "the controller holds the ends the caller has not taken"
    );
    controller.requests().cancel();
    block_on(controller.drive()).expect("the loop cancels the run");

    assert_eq!(controller.run().state(), RunState::Cancelled);
    assert_eq!(
        controller
            .instance(model)
            .map(inference_runtime::NodeInstance::state),
        Some(NodeInstanceState::Stopped)
    );
    assert!(
        controller.exposed().ingresses.is_empty() && controller.exposed().egresses.is_empty(),
        "an ended run hands out no end"
    );
    for queue in &exposure_queues {
        assert!(
            queue.is_dropped(),
            "the queue of exposure {} is dropped with the run",
            queue.exposure_id()
        );
    }
}
