//! Adversary cases of `story:sdk-model-nodes`: the model node now lives in `inference-pipeline`,
//! so a host outside the testkit drives it on a payload of its own. The node is taken from
//! `inference_pipeline::nodes`, never through a testkit re-export, and run on a payload that is
//! not the text pipeline's `PipelinePayload`; what it sends and records is held equal to the
//! same run on `PipelinePayload`: the kinds in order, one stream of the node's own numbered from
//! 1, the generation of each envelope, and the end of input as a `stop` on that stream.

#![cfg(all(feature = "llm", feature = "run"))]

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::Arc;
use std::sync::mpsc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use inference_core::envelope::{ControlKind, EventEnvelope};
use inference_core::id::{GenerationId, RunId};
use inference_core::model::{Adapter, AdapterId, Capability, ModelId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_core::usage::UsageRecorded;
use inference_pipeline::nodes::{Carried, ModelNode, ModelPayload, RunLog, StreamEvent};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeImplementation,
    NodeRole, OverloadPolicy, PipelineSpec, PlannedPipeline, PortTraits, modality,
};
use inference_runtime::run::RunState;
use inference_runtime::{NodeId, RunController};
use inference_testkit::graph::{PipelinePayload, TextSink, TextSource};
use inference_testkit::llm::{MockConfiguration, MockGenerator};
use inference_text::{SpanId, TranscriptCommitted};

/// The whole case fails rather than hangs past this.
const DEADLINE: Duration = Duration::from_secs(20);

const INPUTS: [&str; 3] = ["1", "2", "3"];

fn reply(input: &str) -> String {
    format!("s{input}")
}

/// A host's payload: stream events and control events, and no usage.
#[derive(Debug, Clone)]
enum Host {
    Event(StreamEvent),
    Control(ControlKind),
}

impl From<ControlKind> for Host {
    fn from(kind: ControlKind) -> Self {
        Self::Control(kind)
    }
}

impl ModelPayload for Host {
    fn stream(event: StreamEvent) -> Self {
        Self::Event(event)
    }

    fn usage(_recorded: UsageRecorded) -> Option<Self> {
        None
    }

    fn carried(self) -> Carried {
        match self {
            Self::Event(event) => Carried::Stream(event),
            Self::Control(kind) => Carried::Control(kind),
        }
    }
}

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

fn generator() -> MockGenerator {
    MockGenerator::new(
        MockConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        Vec::<String>::new(),
    )
}

fn planned(generator: &MockGenerator) -> (Declaration, PlannedPipeline, [NodeId; 3]) {
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
        name: "a host's text pipeline".to_owned(),
        schema_revision: "1".to_owned(),
        mode: InteractionMode::TurnBased,
        required_capabilities: vec![Capability::OutputStreaming],
    };
    let mut declaration = Declaration::new();
    let (pipeline_id, nodes) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let source = pipeline.node("source", NodeRole::Source, Vec::new());
            let model = pipeline.node("model", NodeRole::Model, vec![Capability::OutputStreaming]);
            let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
            let emitted = pipeline.output::<modality::Text>(&source, "text", text);
            let prompt = pipeline.input::<modality::Text>(&model, "prompt", text);
            let answer = pipeline.output::<modality::Text>(&model, "answer", text);
            let collected = pipeline.input::<modality::Text>(&sink, "text", text);
            pipeline.connect(
                &emitted,
                &prompt,
                NonZeroUsize::new(4).expect("4"),
                OverloadPolicy::Pause,
            );
            pipeline.connect(
                &answer,
                &collected,
                NonZeroUsize::new(64).expect("64"),
                OverloadPolicy::Fail,
            );
            pipeline
                .bind(&model, adapter_id)
                .expect("the catalog holds the mock's adapter");
            [source.id(), model.id(), sink.id()]
        })
        .expect("the profile is declared");
    let validated = declaration.validate(pipeline_id).expect("valid");
    let planned = declaration
        .plan(validated, &catalog)
        .expect("the model streams output");
    (declaration, planned, nodes)
}

/// What one envelope the sink received says, with its payload reduced to what both payloads
/// carry: the kind, the sequence, whether the envelope names a generation and whether that is the
/// payload's, and the text of a delta.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Seen {
    kind: String,
    sequence: i64,
    names_generation: bool,
    generation_agrees: bool,
    delta: Option<String>,
}

fn seen(envelope: &EventEnvelope<Option<StreamEvent>>) -> Seen {
    let payload_generation = envelope
        .payload
        .as_ref()
        .and_then(StreamEvent::generation_id);
    Seen {
        kind: envelope.kind.clone(),
        sequence: envelope.sequence,
        names_generation: envelope.generation_id.is_some(),
        generation_agrees: payload_generation.is_none_or(|g| envelope.generation_id == Some(g)),
        delta: match &envelope.payload {
            Some(StreamEvent::TextDelta(delta)) => Some(delta.text.clone()),
            _ => None,
        },
    }
}

fn reduce<P>(
    envelope: EventEnvelope<P>,
    event: impl FnOnce(P) -> Option<StreamEvent>,
) -> EventEnvelope<Option<StreamEvent>> {
    EventEnvelope {
        schema_version: envelope.schema_version,
        event_id: envelope.event_id,
        run_id: envelope.run_id,
        session_id: envelope.session_id,
        stream_id: envelope.stream_id,
        sequence: envelope.sequence,
        producer: envelope.producer,
        port: envelope.port,
        kind: envelope.kind,
        generation_id: envelope.generation_id,
        timestamp: envelope.timestamp,
        timebase: envelope.timebase,
        causation_id: envelope.causation_id,
        payload: event(envelope.payload),
    }
}

/// One run: what the sink received, reduced, and the kinds the run's log recorded of the model.
struct Outcome {
    received: Vec<EventEnvelope<Option<StreamEvent>>>,
    logged: Vec<String>,
    model: NodeId,
}

/// Drives source, model and sink on payload `P` to the end of input, within the deadline.
fn run<P: ModelPayload + Clone + std::fmt::Debug>(
    events: Vec<(String, Option<GenerationId>, P)>,
    event: fn(P) -> Option<StreamEvent>,
) -> Outcome {
    let generator = generator();
    let (declaration, planned, [source_id, model_id, sink_id]) = planned(&generator);
    let run_id = RunId::generate();
    let log = RunLog::new();
    let source: TextSource<P> = TextSource::scripted(run_id, events);
    let model: ModelNode<P> = ModelNode::new(run_id, generator, reply, log.clone());
    let sink: TextSink<P> = TextSink::default();
    let (source_handle, sink_handle) = (source.handle(), sink.handle());
    let mut implementations: HashMap<NodeId, Box<dyn NodeImplementation<P>>> = HashMap::from([
        (
            source_id,
            Box::new(source) as Box<dyn NodeImplementation<P>>,
        ),
        (model_id, Box::new(model)),
        (sink_id, Box::new(sink)),
    ]);
    let mut controller = RunController::create_as(
        run_id,
        &planned,
        &declaration,
        HashMap::<GenerationId, i64>::new(),
        |node: &Node| {
            implementations
                .remove(&node.node_id())
                .expect("every node has an implementation")
        },
    )
    .expect("the pipeline is planned");
    controller.validate().expect("a Created run validates");
    block_on(controller.prepare()).expect("every node prepares");
    block_on(controller.activate()).expect("every node starts");
    let (done, ended) = mpsc::channel();
    thread::spawn(move || {
        let result = block_on(controller.drive());
        let _ = done.send((
            controller.run().state(),
            result.map_err(|e| format!("{e:?}")),
        ));
    });
    for _ in INPUTS {
        assert!(source_handle.release(), "the source admits each input");
    }
    assert!(source_handle.release_end(), "the source admits its end");
    let (state, result) = ended
        .recv_timeout(DEADLINE)
        .expect("the run ends within the deadline");
    result.expect("the drive loop ends the run");
    assert_eq!(state, RunState::Completed);
    let received = sink_handle
        .received()
        .into_iter()
        .map(|envelope| reduce(envelope, event))
        .collect();
    let logged = log
        .envelopes()
        .into_iter()
        .filter(|envelope| envelope.producer == model_id)
        .map(|envelope| envelope.kind)
        .collect();
    Outcome {
        received,
        logged,
        model: model_id,
    }
}

fn committed(text: &str) -> (String, Option<GenerationId>, StreamEvent) {
    let event = StreamEvent::TranscriptCommitted(TranscriptCommitted {
        span_id: SpanId::generate(),
        text: text.to_owned(),
    });
    (event.name().to_owned(), None, event)
}

#[test]
fn a_host_drives_the_pipelines_model_node_on_its_own_payload_as_on_the_text_payload() {
    let text = run::<PipelinePayload>(
        INPUTS
            .iter()
            .map(|input| {
                let (kind, generation, event) = committed(input);
                (kind, generation, PipelinePayload::Stream(event))
            })
            .collect(),
        |payload| match payload {
            PipelinePayload::Stream(event) => Some(event),
            PipelinePayload::Usage(_) | PipelinePayload::Control(_) => None,
        },
    );
    let host = run::<Host>(
        INPUTS
            .iter()
            .map(|input| {
                let (kind, generation, event) = committed(input);
                (kind, generation, Host::Event(event))
            })
            .collect(),
        |payload| match payload {
            Host::Event(event) => Some(event),
            Host::Control(_) => None,
        },
    );

    for outcome in [&text, &host] {
        let from_model: Vec<_> = outcome
            .received
            .iter()
            .filter(|envelope| envelope.producer == outcome.model)
            .collect();
        assert!(!from_model.is_empty(), "the model sent something");
        let stream = from_model[0].stream_id;
        assert!(
            from_model
                .iter()
                .all(|envelope| envelope.stream_id == stream),
            "one stream of the model's own"
        );
        let sequences: Vec<i64> = from_model
            .iter()
            .map(|envelope| envelope.sequence)
            .collect();
        let expected: Vec<i64> = (1..=i64::try_from(sequences.len()).expect("small")).collect();
        assert_eq!(sequences, expected, "numbered from 1 without a gap");
        let last = from_model.last().expect("one at least");
        assert_eq!(
            last.kind,
            ControlKind::Stop.as_str(),
            "the end of input last"
        );
        assert!(last.payload.is_none(), "the end is a control event");
        let deltas: Vec<&str> = from_model
            .iter()
            .filter_map(|envelope| match &envelope.payload {
                Some(StreamEvent::TextDelta(delta)) => Some(delta.text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(deltas, ["s1", "s2", "s3"], "one answer per input, in order");
        assert!(
            from_model
                .iter()
                .all(|envelope| seen(envelope).generation_agrees),
            "every envelope names the generation its payload names"
        );
    }

    let text_seen: Vec<Seen> = text
        .received
        .iter()
        .filter(|envelope| envelope.producer == text.model)
        .map(seen)
        .collect();
    let host_seen: Vec<Seen> = host
        .received
        .iter()
        .filter(|envelope| envelope.producer == host.model)
        .map(seen)
        .collect();
    assert_eq!(
        host_seen, text_seen,
        "a host payload sees what the text payload sees"
    );
    assert_eq!(
        host.logged, text.logged,
        "the run's log records the same events of the model on either payload"
    );
    assert!(
        !host
            .logged
            .iter()
            .any(|kind| kind == ControlKind::Stop.as_str()),
        "a control event is not recorded"
    );
}
