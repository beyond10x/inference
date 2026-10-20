//! A-02 (design 23.3): a text-only streaming pipeline of source, model node and sink, built as
//! `inference-testkit`'s `llm_model_node` test builds it, with the generator of this adapter in
//! place of the in-process model, runs to Completed against the loopback fixture of `common`.
//!
//! The testkit's `ModelNode` offers every input to one running generation, which needs
//! `concurrent_input_output`; this adapter does not claim it, and the generator refuses such
//! input. The model node here answers each input on a generation of its own instead: it starts
//! the generation with the input as the user text of its request and forwards its events, in
//! envelopes, until it completes.

mod common;

use std::collections::{HashMap, VecDeque};
use std::future::{self, Future};
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, JoinHandle, Thread};
use std::time::{SystemTime, UNIX_EPOCH};

use common::{DELTAS, Fixture, SERVED_MODEL};
use inference_core::endpoint::{InputEndpoint, OutputEndpoint};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, RunId, StreamId};
use inference_core::model::{Adapter, AdapterId, Capability, Model};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_llama_server::{Config, LlamaServer, declare_adapter, declare_model};
use inference_llm::llm_core::{Item, TurnRequest};
use inference_llm::{GenerationEvent, GenerationRequest, Generator};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeEndpoints, NodeError,
    NodeFuture, NodeImplementation, NodeRole, OverloadPolicy, PipelineSpec, PlannedPipeline,
    PortTraits, modality,
};
use inference_runtime::run::RunState;
use inference_runtime::{NodeId, RunController};
use inference_testkit::graph::{PipelinePayload, RunLog, TextSink, TextSource};
use inference_testkit::stream::StreamEvent;

const INPUTS: [&str; 2] = ["hello", "world"];

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

/// Whether a node's exchange thread ended, and the waker of the drain waiting for it.
#[derive(Default)]
struct Ended(Mutex<(bool, Option<Waker>)>);

impl Ended {
    fn end(&self) {
        let waker = {
            let mut ended = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            ended.0 = true;
            ended.1.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    fn poll_ended(&self, cx: &mut Context<'_>) -> Poll<()> {
        let mut ended = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if ended.0 {
            Poll::Ready(())
        } else {
            ended.1 = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

/// Marks the exchange ended when its thread leaves its work, returning or unwinding.
struct Ending(Arc<Ended>);

impl Drop for Ending {
    fn drop(&mut self) {
        self.0.end();
    }
}

/// Wraps what the node sends on its one output port in envelopes of one stream of its own.
struct Envelopes {
    run_id: RunId,
    producer: NodeId,
    port: String,
    stream_id: StreamId,
    sequence: i64,
}

impl Envelopes {
    fn envelope(
        &mut self,
        kind: String,
        generation_id: Option<GenerationId>,
        payload: PipelinePayload,
    ) -> EventEnvelope<PipelinePayload> {
        self.sequence += 1;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| {
                i64::try_from(since.as_micros()).unwrap_or(i64::MAX)
            });
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: self.run_id,
            session_id: None,
            stream_id: self.stream_id,
            sequence: self.sequence,
            producer: self.producer,
            port: self.port.clone(),
            kind,
            generation_id,
            timestamp,
            timebase: Timebase::WallTime,
            causation_id: None,
            payload,
        }
    }
}

/// A model node answering each `TranscriptCommitted` on a generation of its own.
struct TurnNode {
    run_id: RunId,
    node_id: Option<NodeId>,
    generator: Generator,
    exchange: Option<JoinHandle<Result<(), String>>>,
    ended: Arc<Ended>,
}

impl TurnNode {
    fn new(run_id: RunId, generator: Generator) -> Self {
        Self {
            run_id,
            node_id: None,
            generator,
            exchange: None,
            ended: Arc::new(Ended::default()),
        }
    }

    async fn exchange(
        mut generator: Generator,
        mut input: Box<dyn InputEndpoint<PipelinePayload>>,
        output: Box<dyn OutputEndpoint<PipelinePayload>>,
        mut envelopes: Envelopes,
    ) -> Result<(), String> {
        while let Some(envelope) = input.recv().await {
            match envelope.payload {
                PipelinePayload::Stream(StreamEvent::TranscriptCommitted(committed)) => {
                    let mut handle = generator
                        .start(GenerationRequest {
                            run_id: envelopes.run_id,
                            node_id: envelopes.producer,
                            epoch: 1,
                            turn: TurnRequest::new(SERVED_MODEL, vec![Item::user(committed.text)]),
                        })
                        .map_err(|refused| refused.to_string())?;
                    let generation_id = handle.output().generation().generation_id();
                    while let Some(event) = handle.next_event().await {
                        let event = match event {
                            GenerationEvent::Started(started) => {
                                StreamEvent::GenerationStarted(started)
                            }
                            GenerationEvent::TextDelta(delta) => StreamEvent::TextDelta(delta),
                            GenerationEvent::SegmentCommitted(committed) => {
                                StreamEvent::TextSegmentCommitted(committed)
                            }
                            GenerationEvent::Completed(completed) => {
                                StreamEvent::GenerationCompleted(completed)
                            }
                            GenerationEvent::Cancelled(..) => {
                                return Err(format!(
                                    "the generation was cancelled: {:?}",
                                    handle.failure()
                                ));
                            }
                        };
                        let envelope = envelopes.envelope(
                            event.name().to_owned(),
                            Some(generation_id),
                            PipelinePayload::Stream(event),
                        );
                        output
                            .send(envelope)
                            .await
                            .map_err(|refused| refused.to_string())?;
                    }
                }
                PipelinePayload::Control(ControlKind::Stop) => {
                    let stop = envelopes.envelope(
                        ControlKind::Stop.as_str().to_owned(),
                        None,
                        PipelinePayload::Control(ControlKind::Stop),
                    );
                    return output
                        .send(stop)
                        .await
                        .map_err(|refused| refused.to_string());
                }
                PipelinePayload::Stream(_) | PipelinePayload::Control(_) => {}
            }
        }
        Ok(())
    }
}

impl NodeImplementation<PipelinePayload> for TurnNode {
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.node_id = Some(node.node_id());
        Box::pin(future::ready(()))
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(future::ready(Ok(())))
    }

    fn start(
        &mut self,
        mut endpoints: NodeEndpoints<PipelinePayload>,
    ) -> NodeFuture<'_, Result<(), NodeError>> {
        let started = match (
            self.node_id,
            endpoints.inputs.len(),
            endpoints.outputs.len(),
        ) {
            (Some(node_id), 1, 1) => {
                let input = endpoints.inputs.remove(0);
                let output = endpoints.outputs.remove(0);
                let envelopes = Envelopes {
                    run_id: self.run_id,
                    producer: node_id,
                    port: output.port_id.to_string(),
                    stream_id: StreamId::generate(),
                    sequence: 0,
                };
                let generator = self.generator.clone();
                let ending = Ending(Arc::clone(&self.ended));
                self.exchange = Some(thread::spawn(move || {
                    let _ending = ending;
                    block_on(Self::exchange(
                        generator,
                        input.endpoint,
                        output.endpoint,
                        envelopes,
                    ))
                }));
                Ok(())
            }
            _ => Err(NodeError::new(
                "the model node takes one input and one output edge, once described",
            )),
        };
        Box::pin(future::ready(started))
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        Box::pin(async move {
            let ended = Arc::clone(&self.ended);
            future::poll_fn(|cx| ended.poll_ended(cx)).await;
            match self.exchange.take().map(JoinHandle::join) {
                None | Some(Ok(Ok(()))) => Ok(()),
                Some(Ok(Err(failure))) => Err(NodeError::new(failure)),
                Some(Err(_)) => Err(NodeError::new("the model node's exchange panicked")),
            }
        })
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        Box::pin(future::ready(()))
    }
}

struct Catalogue {
    profile: DeploymentProfile,
    adapter: Adapter,
}

impl Catalog for Catalogue {
    fn profile(&self, profile_id: ProfileId) -> Option<&DeploymentProfile> {
        (self.profile.profile_id() == profile_id).then_some(&self.profile)
    }

    fn adapter(&self, adapter_id: AdapterId) -> Option<&Adapter> {
        (self.adapter.adapter_id() == adapter_id).then_some(&self.adapter)
    }
}

/// Source to model to sink, the model bound to `adapter`, as `llm_model_node` builds it.
fn planned(adapter: &Adapter) -> (Declaration, PlannedPipeline, [NodeId; 3]) {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    let catalog = Catalogue {
        profile,
        adapter: adapter.clone(),
    };
    let text = PortTraits {
        essentiality: Essentiality::Essential,
        delivery: Delivery::Reliable,
        incremental: true,
        end_marker_required: true,
        provisional_allowed: false,
    };
    let spec = PipelineSpec {
        name: "text over a llama-server".to_owned(),
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
                .bind(&model, adapter.adapter_id())
                .expect("the catalog holds the adapter");
            [source.id(), model.id(), sink.id()]
        })
        .expect("the profile is declared");
    let validated = declaration
        .validate(pipeline_id)
        .expect("source to model to sink is valid");
    let planned = declaration
        .plan(validated, &catalog)
        .expect("the model is bound to an adapter that streams output");
    (declaration, planned, nodes)
}

#[test]
fn a_text_only_pipeline_over_the_adapter_runs_to_completed() {
    let fixture = Fixture::replaying();
    let config = Config::new(fixture.base_url(), SERVED_MODEL);
    let (model, _) = Model::declare(declare_model());
    let (adapter, _) =
        Adapter::declare(declare_adapter(model.model_id(), &config)).expect("one slot");
    let (declaration, planned, [source_id, model_id, sink_id]) = planned(&adapter);
    let generator = LlamaServer::new(&config)
        .expect("a binding")
        .generator(adapter)
        .expect("the binding declares what the adapter claims");

    let run_id = RunId::generate();
    let log = RunLog::new();
    let source = TextSource::new(run_id, INPUTS, log);
    let model = TurnNode::new(run_id, generator);
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
                .expect("every node of the pipeline has an implementation")
        },
    )
    .expect("the pipeline is planned");
    controller.validate().expect("a Created run validates");
    block_on(controller.prepare()).expect("every node prepares");
    block_on(controller.activate()).expect("every node starts");
    let drive = thread::spawn(move || {
        let result = block_on(controller.drive());
        (controller, result)
    });
    for _ in INPUTS {
        assert!(source_handle.release(), "the source admits each input");
    }
    assert!(source_handle.release_end(), "the source admits its end");
    let (controller, result) = drive.join().expect("the drive loop does not panic");
    result.expect("the drive loop ends the run");

    assert_eq!(controller.run().state(), RunState::Completed);
    let expected: Vec<&str> = INPUTS.iter().flat_map(|_| DELTAS).collect();
    assert_eq!(
        sink_handle.committed(),
        expected,
        "each delta of the server is one committed segment"
    );

    let mut generations: VecDeque<(GenerationId, &'static str)> = sink_handle
        .received()
        .into_iter()
        .filter_map(|envelope| match envelope.payload {
            PipelinePayload::Stream(StreamEvent::GenerationStarted(started)) => {
                Some((started.generation_id, "started"))
            }
            PipelinePayload::Stream(StreamEvent::GenerationCompleted(completed)) => {
                Some((completed.generation_id, "completed"))
            }
            _ => None,
        })
        .collect();
    assert_eq!(generations.len(), 4, "{generations:?}");
    let first = generations.pop_front().expect("four");
    assert_eq!(generations.pop_front().map(|(id, _)| id), Some(first.0));
    let second = generations.pop_front().expect("two left");
    assert_ne!(
        first.0, second.0,
        "each input is answered on a generation of its own"
    );
    assert_eq!(generations.pop_front(), Some((second.0, "completed")));

    let requests = fixture.requests();
    let asked: Vec<serde_json::Value> = requests
        .iter()
        .map(|request| request.json()["messages"].clone())
        .collect();
    assert_eq!(
        asked,
        INPUTS
            .iter()
            .map(|input| serde_json::json!([{"role": "user", "content": input}]))
            .collect::<Vec<_>>(),
        "one request per input, carrying it as user text"
    );
}
