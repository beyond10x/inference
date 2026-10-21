//! The text-only topology A of `docs/design/inference.md` (749-757) as a session's tests need it:
//! the A-02 model node, its `prompt` exposed as the ingress `user text` and its `answer` as the
//! egress `reply`, declared with the typed builder, validated and planned, and the application's
//! side of the session.

use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

use inference_core::envelope::{EventEnvelope, EventId, Timebase};
use inference_core::id::{NodeId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId, Capability, ModelId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeImplementation,
    NodeRole, OverloadPolicy, PipelineSpec, PlannedPipeline, PortId, PortTraits, modality,
};
use inference_testkit::graph::{PipelinePayload, RunLog};
use inference_testkit::llm::{MockConfiguration, MockGenerator, ModelNode};
use inference_testkit::stream::StreamEvent;
use inference_text::{SpanId, TranscriptCommitted};

pub const INGRESS: &str = "user text";
pub const EGRESS: &str = "reply";
pub const USAGE_EGRESS: &str = "usage";

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Polls `future` on this thread until it completes: the facade commits to no executor.
pub fn block_on<F: Future>(future: F) -> F::Output {
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

/// The model's reply to an input text.
pub fn reply(input: &str) -> String {
    format!("s{input}")
}

/// A mock generator streaming its output, generating `script` piece by piece.
pub fn generator<I, S>(script: I) -> MockGenerator
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    MockGenerator::new(
        MockConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        script,
    )
}

/// The model node alone between the ingress `user text` and the egress `reply`, a turn-based
/// pipeline requiring `required`, declared, validated and planned.
pub fn planned(
    generator: &MockGenerator,
    required: &[Capability],
) -> (Declaration, PlannedPipeline) {
    planned_with_egress(generator, required, NonZeroUsize::new(64).expect("64"))
}

/// As [`planned`], the egress holding at most `egress` envelopes, pausing its producer when full.
pub fn planned_with_egress(
    generator: &MockGenerator,
    required: &[Capability],
    egress: NonZeroUsize,
) -> (Declaration, PlannedPipeline) {
    let (declaration, planned, _) = text_pipeline(generator, required, egress, false);
    (declaration, planned)
}

/// As [`planned`] requiring `output_streaming`, the model node's output port `usage` exposed as
/// the egress [`USAGE_EGRESS`], pausing on overload so that no record is dropped; and that port,
/// which the node is given ([`ModelNode::with_usage`]).
pub fn planned_with_usage(generator: &MockGenerator) -> (Declaration, PlannedPipeline, PortId) {
    let (declaration, planned, usage) = text_pipeline(
        generator,
        &[Capability::OutputStreaming],
        NonZeroUsize::new(64).expect("64"),
        true,
    );
    (
        declaration,
        planned,
        usage.expect("the usage port was declared"),
    )
}

/// The text pipeline, its egress holding at most `egress` envelopes, and, when `usage`, the
/// model node's `usage` port, exposed as [`USAGE_EGRESS`].
fn text_pipeline(
    generator: &MockGenerator,
    required: &[Capability],
    egress: NonZeroUsize,
    usage: bool,
) -> (Declaration, PlannedPipeline, Option<PortId>) {
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
        name: "text session".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: required.to_vec(),
    };
    let records = PortTraits {
        incremental: false,
        ..text
    };
    let mut declaration = Declaration::new();
    let (pipeline_id, usage) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let model = pipeline.node("model", NodeRole::Model, required.to_vec());
            let prompt = pipeline.input::<modality::Text>(&model, "prompt", text);
            let answer = pipeline.output::<modality::Text>(&model, "answer", text);
            pipeline.expose(
                &prompt,
                INGRESS,
                NonZeroUsize::new(4).expect("4"),
                OverloadPolicy::Pause,
            );
            pipeline.expose(&answer, EGRESS, egress, OverloadPolicy::Pause);
            let usage = usage.then(|| {
                let port = pipeline.output::<modality::Structured>(&model, "usage", records);
                pipeline.expose(
                    &port,
                    USAGE_EGRESS,
                    NonZeroUsize::new(16).expect("16"),
                    OverloadPolicy::Pause,
                );
                port.id()
            });
            pipeline
                .bind(&model, adapter_id)
                .expect("the catalog holds the mock LLM's adapter");
            usage
        })
        .expect("the profile is declared");
    let validated = declaration
        .validate(pipeline_id)
        .expect("a model between an ingress and an egress is valid");
    let planned = declaration
        .plan(validated, &catalog)
        .expect("the model is bound to an adapter that streams output");
    (declaration, planned, usage)
}

/// A sink node alone behind the ingress `user text`, a turn-based pipeline with no egress,
/// declared, validated and planned: what the session forwards, the sink records.
pub fn sink_planned(generator: &MockGenerator) -> (Declaration, PlannedPipeline) {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: vec![inference_core::model::Locality::SameProcess],
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
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
        name: "recorded session".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let mut declaration = Declaration::new();
    let (pipeline_id, ()) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
            let input = pipeline.input::<modality::Text>(&sink, "in", text);
            pipeline.expose(
                &input,
                INGRESS,
                NonZeroUsize::new(8).expect("8"),
                OverloadPolicy::Pause,
            );
        })
        .expect("the profile is declared");
    let validated = declaration
        .validate(pipeline_id)
        .expect("a sink behind an ingress is valid");
    let planned = declaration
        .plan(validated, &catalog)
        .expect("a sink needs no adapter");
    (declaration, planned)
}

/// A transform node alone between the ingress `user text` and the egress `reply`, both
/// Structured, a turn-based pipeline declared, validated and planned: the node the test supplies
/// passes on what it takes.
pub fn echo_planned(generator: &MockGenerator) -> (Declaration, PlannedPipeline) {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: vec![inference_core::model::Locality::SameProcess],
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    let catalog = Fixture {
        profile,
        adapter: generator.adapter().clone(),
    };
    let structured = PortTraits {
        essentiality: Essentiality::Essential,
        delivery: Delivery::Reliable,
        incremental: false,
        end_marker_required: true,
        provisional_allowed: false,
    };
    let spec = PipelineSpec {
        name: "echo session".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: Vec::new(),
    };
    let mut declaration = Declaration::new();
    let (pipeline_id, ()) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let echo = pipeline.node("echo", NodeRole::Transform, Vec::new());
            let input = pipeline.input::<modality::Structured>(&echo, "in", structured);
            let output = pipeline.output::<modality::Structured>(&echo, "out", structured);
            pipeline.expose(
                &input,
                INGRESS,
                NonZeroUsize::new(8).expect("8"),
                OverloadPolicy::Pause,
            );
            pipeline.expose(
                &output,
                EGRESS,
                NonZeroUsize::new(64).expect("64"),
                OverloadPolicy::Pause,
            );
        })
        .expect("the profile is declared");
    let validated = declaration
        .validate(pipeline_id)
        .expect("a transform between an ingress and an egress is valid");
    let planned = declaration
        .plan(validated, &catalog)
        .expect("a transform needs no adapter");
    (declaration, planned)
}

/// The implementation of the pipeline's one node: the mock model answering on `generator`.
pub fn implement(
    generator: &MockGenerator,
) -> impl FnMut(RunId, &Node) -> Box<dyn NodeImplementation<PipelinePayload>> {
    let generator = generator.clone();
    move |run_id, _| {
        Box::new(ModelNode::new(
            run_id,
            generator.clone(),
            reply,
            RunLog::new(),
        ))
    }
}

/// What the application sends: envelopes of its own stream, numbered from 1, for the ingress.
pub struct Application {
    run_id: RunId,
    producer: NodeId,
    stream_id: StreamId,
    sequence: i64,
}

impl Application {
    pub fn new(run_id: RunId) -> Self {
        Self {
            run_id,
            producer: NodeId::generate(),
            stream_id: StreamId::generate(),
            sequence: 0,
        }
    }

    /// The committed transcript `text`, as the application's input to the model.
    pub fn text(&mut self, text: &str) -> EventEnvelope<PipelinePayload> {
        self.sequence += 1;
        let event = StreamEvent::TranscriptCommitted(TranscriptCommitted {
            span_id: SpanId::generate(),
            text: text.to_owned(),
        });
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: self.run_id,
            session_id: None,
            stream_id: self.stream_id,
            sequence: self.sequence,
            producer: self.producer,
            port: INGRESS.to_owned(),
            kind: event.name().to_owned(),
            generation_id: None,
            timestamp: 0,
            timebase: Timebase::WallTime,
            causation_id: None,
            payload: PipelinePayload::Stream(event),
        }
    }
}
