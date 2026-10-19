//! Adversary pass 1 on the text pipeline of `story:text-pipeline-end-to-end`: the drained run of
//! Check 3 on the same three mocks declared downstream first, and the story's "a repeated
//! `(stream_id, sequence)` from a node is refused" asked of the run rather than of the log.

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase, is_control_kind};
use inference_core::id::{GenerationId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId, Capability, ModelId};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_llm::Generator;
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeEndpoints, NodeError,
    NodeFuture, NodeImplementation, NodeRole, OverloadPolicy, PipelineSpec, PlannedPipeline,
    PortTraits, modality,
};
use inference_runtime::run::RunState;
use inference_runtime::{NodeId, RunController, RunError};
use inference_testkit::graph::{PipelinePayload, RunLog, TextSink, TextSource};
use inference_testkit::llm::{MockConfiguration, MockGenerator, ModelNode};
use inference_testkit::stream::StreamEvent;
use inference_text::{SpanId, TranscriptCommitted};

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
    adapter: Option<Adapter>,
}

impl Catalog for Fixture {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.profile.profile_id() == profile_id).then_some(&self.profile)
    }

    fn adapter(&self, adapter_id: AdapterId) -> Option<&Adapter> {
        self.adapter
            .as_ref()
            .filter(|adapter| adapter.adapter_id() == adapter_id)
    }
}

const TEXT: PortTraits = PortTraits {
    essentiality: Essentiality::Essential,
    delivery: Delivery::Reliable,
    incremental: true,
    end_marker_required: true,
    provisional_allowed: false,
};

fn fixture(adapter: Option<Adapter>) -> (Fixture, ProfileId, PipelineSpec) {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    let required_capabilities = if adapter.is_some() {
        vec![Capability::OutputStreaming]
    } else {
        Vec::new()
    };
    (
        Fixture { profile, adapter },
        profile_id,
        PipelineSpec {
            name: "text only".to_owned(),
            schema_revision: "1".to_owned(),
            mode: InteractionMode::TurnBased,
            required_capabilities,
        },
    )
}

/// The story's pipeline, source to model to sink with the story's edges, with its nodes declared
/// in the reverse order: sink, then model, then source.
fn planned_downstream_first(
    generator: &MockGenerator,
) -> (Declaration, PlannedPipeline, NodeId, NodeId, NodeId) {
    let (catalog, profile_id, spec) = fixture(Some(generator.adapter().clone()));
    let adapter_id = generator.adapter().adapter_id();
    let mut declaration = Declaration::new();
    let (pipeline_id, (source, model, sink)) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
            let model = pipeline.node("model", NodeRole::Model, vec![Capability::OutputStreaming]);
            let source = pipeline.node("source", NodeRole::Source, Vec::new());
            let emitted = pipeline.output::<modality::Text>(&source, "text", TEXT);
            let prompt = pipeline.input::<modality::Text>(&model, "prompt", TEXT);
            let answer = pipeline.output::<modality::Text>(&model, "answer", TEXT);
            let collected = pipeline.input::<modality::Text>(&sink, "text", TEXT);
            pipeline.connect(
                &emitted,
                &prompt,
                NonZeroUsize::new(4).expect("4"),
                OverloadPolicy::Pause,
            );
            pipeline.connect(
                &answer,
                &collected,
                NonZeroUsize::new(16).expect("16"),
                OverloadPolicy::Fail,
            );
            pipeline
                .bind(&model, adapter_id)
                .expect("the catalog holds the mock LLM's adapter");
            (source.id(), model.id(), sink.id())
        })
        .expect("the profile is declared");
    let validated = declaration
        .validate(pipeline_id)
        .expect("source to model to sink is valid in any declaration order");
    let planned = declaration
        .plan(validated, &catalog)
        .expect("the model is bound to an adapter that streams output");
    (declaration, planned, source, model, sink)
}

#[test]
fn a_run_of_the_pipeline_declared_downstream_first_drains_to_completed() {
    let generator = MockGenerator::new(
        MockConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        Vec::<String>::new(),
    );
    let (declaration, planned, source_id, model_id, sink_id) = planned_downstream_first(&generator);
    let run_id = RunId::generate();
    let log = RunLog::new();
    let source = TextSource::new(run_id, ["1", "2", "3", "4"], log.clone());
    let model = ModelNode::new(run_id, generator.clone(), |input| format!("s{input}"), log);
    let sink = TextSink::new();
    let (source_handle, sink_handle) = (source.handle(), sink.handle());
    let mut implementations: HashMap<NodeId, Box<dyn NodeImplementation<PipelinePayload>>> =
        HashMap::from([
            (
                source_id,
                Box::new(source) as Box<dyn NodeImplementation<PipelinePayload>>,
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
                .expect("every node of the pipeline has a mock")
        },
    )
    .expect("neither edge cancels or supersedes");
    controller.validate().expect("a Created run validates");
    block_on(controller.prepare()).expect("every node prepares");
    block_on(controller.activate()).expect("every node starts");
    let requests = controller.requests();
    let (ended, outcome) = mpsc::channel();
    thread::spawn(move || {
        let result = block_on(controller.drive());
        let _ = ended.send((controller.run().state(), result));
    });
    for _ in 0..3 {
        assert!(source_handle.release());
    }
    assert!(
        source_handle.wait_emitted(3, PATIENCE),
        "input 3 is emitted"
    );
    requests.drain();

    let drained: Result<(RunState, Result<(), RunError>), _> = outcome.recv_timeout(PATIENCE);
    if drained.is_err() {
        requests.cancel();
        let cancelled = outcome.recv_timeout(Duration::from_secs(2));
        panic!(
            "Check 3 on the same pipeline declared sink, model, source: the drained run never \
             ended within {PATIENCE:?} (a cancel asked for afterwards ended it: {:?}); the sink \
             held {:?}",
            cancelled.is_ok(),
            sink_handle.committed()
        );
    }
    assert_eq!(drained, Ok((RunState::Completed, Ok(()))));
    assert_eq!(sink_handle.committed(), ["s1", "s2", "s3"]);
}

/// A source that, when it starts, sends `texts` as `TranscriptCommitted` envelopes all carrying
/// one stream and the sequence 1, then its end of input, and drops its endpoints.
struct RepeatingSource {
    run_id: RunId,
    texts: Vec<&'static str>,
    node_id: Option<NodeId>,
}

impl NodeImplementation<PipelinePayload> for RepeatingSource {
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.node_id = Some(node.node_id());
        Box::pin(std::future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn start(
        &mut self,
        endpoints: NodeEndpoints<PipelinePayload>,
    ) -> NodeFuture<'_, Result<(), NodeError>> {
        let producer = self.node_id.expect("described before started");
        let run_id = self.run_id;
        let texts = self.texts.clone();
        Box::pin(async move {
            let stream_id = StreamId::generate();
            let envelope = |kind: &str, payload| EventEnvelope {
                schema_version: 1,
                event_id: EventId::generate(),
                run_id,
                session_id: None,
                stream_id,
                sequence: 1,
                producer,
                port: "text".to_owned(),
                kind: kind.to_owned(),
                generation_id: None,
                timestamp: 0,
                timebase: Timebase::WallTime,
                causation_id: None,
                payload,
            };
            let output = &endpoints.outputs[0].endpoint;
            for text in texts {
                let event = StreamEvent::TranscriptCommitted(TranscriptCommitted {
                    span_id: SpanId::generate(),
                    text: text.to_owned(),
                });
                let _ = output
                    .send(envelope(event.name(), PipelinePayload::Stream(event)))
                    .await;
            }
            let stop = ControlKind::Stop;
            let _ = output
                .send(envelope(stop.as_str(), PipelinePayload::Control(stop)))
                .await;
            drop(endpoints);
            Ok(())
        })
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(std::future::ready(Ok(())))
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(std::future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(std::future::ready(()))
    }
}

#[test]
fn a_repeated_stream_sequence_a_node_emits_on_a_run_is_refused_before_the_sink() {
    let (catalog, profile_id, spec) = fixture(None);
    let mut declaration = Declaration::new();
    let (pipeline_id, (source_id, sink_id)) = declaration
        .build(&catalog, profile_id, spec, |pipeline| {
            let source = pipeline.node("source", NodeRole::Source, Vec::new());
            let sink = pipeline.node("sink", NodeRole::Sink, Vec::new());
            let out = pipeline.output::<modality::Text>(&source, "text", TEXT);
            let into = pipeline.input::<modality::Text>(&sink, "text", TEXT);
            pipeline.connect(
                &out,
                &into,
                NonZeroUsize::new(4).expect("4"),
                OverloadPolicy::Pause,
            );
            (source.id(), sink.id())
        })
        .expect("the profile is declared");
    let validated = declaration.validate(pipeline_id).expect("valid");
    let planned = declaration.plan(validated, &catalog).expect("planned");
    let run_id = RunId::generate();
    let sink = TextSink::new();
    let sink_handle = sink.handle();
    let mut implementations: HashMap<NodeId, Box<dyn NodeImplementation<PipelinePayload>>> =
        HashMap::from([
            (
                source_id,
                Box::new(RepeatingSource {
                    run_id,
                    texts: vec!["1", "1 again"],
                    node_id: None,
                }) as Box<dyn NodeImplementation<PipelinePayload>>,
            ),
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
    .expect("the edge pauses");
    controller.validate().expect("a Created run validates");
    block_on(controller.prepare()).expect("every node prepares");
    block_on(controller.activate()).expect("every node starts");
    let (ended, outcome) = mpsc::channel();
    thread::spawn(move || {
        let result = block_on(controller.drive());
        let _ = ended.send((controller.run().state(), result));
    });
    let _ = outcome.recv_timeout(PATIENCE).expect("the run ends");

    let content: Vec<i64> = sink_handle
        .received()
        .iter()
        .filter(|envelope| !is_control_kind(&envelope.kind))
        .map(|envelope| envelope.sequence)
        .collect();
    assert_eq!(
        content,
        [1],
        "every event a node instance emits on a run passes through the event log, so a repeated \
         (stream_id, sequence) from a node is refused (story section 'Envelopes on emitted \
         events')"
    );
}
