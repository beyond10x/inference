//! The usage of a subgraph's child run rolls up into its parent (`story:usage-subgraph-rollup`):
//! a subgraph node forwards on its one `usage` port the usage records of every usage egress of
//! its child, and each record keeps the child run's identity.
//!
//! The child holds two A-02 model nodes, each between an ingress and an egress of its own and
//! each with its `usage` port exposed under a name of its own, `first usage` and `second usage`.
//! The parent is the subgraph node alone, every port of it exposed under the port's name, its
//! `usage` port among them, which no exposure of the child carries the name of. One text into
//! each ingress gives one turn per model: the parent's `usage` egress delivers both records, each
//! once, naming the child run and the child node that made the call, on an envelope that crossed
//! to the parent (the parent run, the subgraph node as producer, the port `usage`), and one `stop`
//! after them, once both child egresses ended.

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
use inference_core::usage::UsageRecorded;
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeImplementation,
    NodeRole, OverloadPolicy, PipelineSpec, PlannedPipeline, PortId, PortTraits, USAGE_PORT,
    modality,
};
use inference_runtime::run::RunState;
use inference_runtime::{RunController, SubgraphHandle, SubgraphNode};
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

/// Each model's ingress, egress and usage egress in the child; the parent's ports and exposures
/// carry the first two names, and its `usage` port forwards the third.
const MODELS: [(&str, &str, &str); 2] = [
    ("user text", "reply", "first usage"),
    ("second text", "second reply", "second usage"),
];

fn spec(name: &str) -> PipelineSpec {
    PipelineSpec {
        name: name.to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: vec![Capability::OutputStreaming],
    }
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

struct Planned {
    declaration: Declaration,
    child: PlannedPipeline,
    parent: PlannedPipeline,
    /// Each child model node and its `usage` port, in the order of [`MODELS`].
    models: Vec<(NodeId, PortId)>,
    subgraph: NodeId,
}

fn capacity() -> NonZeroUsize {
    NonZeroUsize::new(16).expect("16")
}

/// The child of two models, one per generator, and the parent of the subgraph node alone.
fn planned(generators: &[MockGenerator; 2]) -> Planned {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: vec![inference_core::model::Locality::SameProcess],
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    let catalog = Fixture {
        profile,
        adapters: generators
            .iter()
            .map(|generator| generator.adapter().clone())
            .collect(),
    };
    let mut declaration = Declaration::new();
    let (child_id, models) = declaration
        .build(&catalog, profile_id, spec("two models"), |pipeline| {
            let mut models = Vec::new();
            for (index, (ingress, egress, usage)) in MODELS.into_iter().enumerate() {
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
                    .bind(&model, generators[index].adapter().adapter_id())
                    .expect("the catalog holds the adapter");
                models.push((model.id(), used.id()));
            }
            models
        })
        .expect("the profile is declared");
    let validated = declaration.validate(child_id).expect("valid");
    let child = declaration.plan(validated, &catalog).expect("planned");
    let (parent_id, subgraph) = declaration
        .build(&catalog, profile_id, spec("parent"), |pipeline| {
            let subgraph = pipeline
                .subgraph("frontend", NodeRole::Transform, child_id)
                .expect("the child is Planned");
            for (ingress, egress, _) in MODELS {
                let into = pipeline.input::<modality::Text>(&subgraph, ingress, TEXT);
                let out = pipeline.output::<modality::Text>(&subgraph, egress, TEXT);
                pipeline.expose(&into, ingress, capacity(), OverloadPolicy::Pause);
                pipeline.expose(&out, egress, capacity(), OverloadPolicy::Pause);
            }
            let usage = pipeline.output::<modality::Structured>(&subgraph, USAGE_PORT, RECORDS);
            pipeline.expose(&usage, USAGE_PORT, capacity(), OverloadPolicy::Pause);
            subgraph.id()
        })
        .expect("the profile is declared");
    let validated = declaration.validate(parent_id).expect("valid");
    let parent = declaration
        .plan(validated, &catalog)
        .expect("the node's `usage` port forwards both usage egresses of the child");
    Planned {
        declaration,
        child,
        parent,
        models,
        subgraph,
    }
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

fn drain(egress: &mut dyn InputEndpoint<PipelinePayload>) -> Vec<EventEnvelope<PipelinePayload>> {
    let mut received = Vec::new();
    while let Some(envelope) = block_on(egress.recv()) {
        received.push(envelope);
    }
    received
}

#[test]
fn the_subgraph_node_forwards_every_usage_egress_of_its_child_once_on_its_usage_port() {
    let generators = [generator(), generator()];
    let planned = planned(&generators);
    let run_id = RunId::generate();
    let usage_of: HashMap<NodeId, (PortId, MockGenerator)> = planned
        .models
        .iter()
        .zip(&generators)
        .map(|((node, usage), generator)| (*node, (*usage, generator.clone())))
        .collect();
    let subgraph = SubgraphNode::new(
        run_id,
        &planned.declaration,
        &planned.child,
        HashMap::<GenerationId, i64>::new(),
        Box::new(move |child_run: RunId, node: &Node| {
            let (usage, generator) = usage_of
                .get(&node.node_id())
                .cloned()
                .expect("every child node is a model");
            Box::new(ModelNode::new(child_run, generator, reply, RunLog::new()).with_usage(usage))
                as Box<dyn NodeImplementation<PipelinePayload>>
        }),
    );
    let handle: SubgraphHandle = subgraph.handle();
    let mut subgraph = Some(subgraph);
    let mut controller = RunController::create_as(
        run_id,
        &planned.parent,
        &planned.declaration,
        HashMap::<GenerationId, i64>::new(),
        |_: &Node| {
            Box::new(subgraph.take().expect("the parent has one node"))
                as Box<dyn NodeImplementation<PipelinePayload>>
        },
    )
    .expect("planned");
    controller.validate().expect("Created");
    block_on(controller.prepare()).expect("the child run prepares");
    let child_run = handle.child_run_id().expect("prepared");
    let mut ends = controller.exposed();
    let ingresses: Vec<Box<dyn OutputEndpoint<PipelinePayload>>> = MODELS
        .iter()
        .map(|(ingress, _, _)| ends.ingress(ingress).expect("the ingress"))
        .collect();
    let mut egresses: Vec<Box<dyn InputEndpoint<PipelinePayload>>> = MODELS
        .iter()
        .map(|(_, egress, _)| ends.egress(egress).expect("the egress"))
        .collect();
    let mut usage = ends.egress(USAGE_PORT).expect("the usage egress");
    block_on(controller.activate()).expect("the subgraph node starts its child");
    let drive = thread::spawn(move || {
        let result = block_on(controller.drive());
        (controller, result)
    });

    let mut application = Application::new(run_id);
    for ((port, _, _), ingress) in MODELS.iter().zip(&ingresses) {
        block_on(ingress.send(application.text(port, "1"))).expect("taken");
        block_on(ingress.send(application.stop(port))).expect("taken");
    }
    for egress in &mut egresses {
        drain(egress.as_mut());
    }
    let received = drain(usage.as_mut());
    drop((ingresses, egresses, usage, ends));
    let (controller, result) = drive.join().expect("the drive loop does not panic");
    result.expect("the drive loop ends the run");
    assert_eq!(controller.run().state(), RunState::Completed);

    let stops: Vec<usize> = received
        .iter()
        .enumerate()
        .filter(|(_, envelope)| envelope.kind == ControlKind::Stop.as_str())
        .map(|(at, _)| at)
        .collect();
    assert_eq!(
        stops,
        [received.len() - 1],
        "one `stop`, after every record, once both usage egresses ended: {received:#?}"
    );
    let records: Vec<(&EventEnvelope<PipelinePayload>, &UsageRecorded)> = received
        .iter()
        .filter_map(|envelope| match &envelope.payload {
            PipelinePayload::Usage(recorded) => Some((envelope, recorded)),
            _ => None,
        })
        .collect();
    for (envelope, recorded) in &records {
        assert_eq!(envelope.kind, UsageRecorded::KIND);
        assert_eq!(
            envelope.run_id, run_id,
            "the envelope crossed to the parent"
        );
        assert_eq!(envelope.producer, planned.subgraph);
        assert_eq!(envelope.port, USAGE_PORT);
        assert!(
            envelope.causation_id.is_some(),
            "it names what it crossed for"
        );
        assert_eq!(
            recorded.run_id,
            Some(child_run),
            "the record keeps the child run, so the child's total stays derivable"
        );
    }
    let mut made_by: Vec<Option<NodeId>> = records.iter().map(|(_, r)| r.node_id).collect();
    made_by.sort();
    let mut models: Vec<Option<NodeId>> =
        planned.models.iter().map(|(node, _)| Some(*node)).collect();
    models.sort();
    assert_eq!(
        made_by, models,
        "one record per model call, each forwarded once: {received:#?}"
    );
}
