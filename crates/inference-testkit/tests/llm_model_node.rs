//! A model node driven by an `llm_core::Model` that is not the testkit's mock: `inference-llm`
//! wraps any implementation of the `llm` contract, and the node of a text-only pipeline answers
//! each input on one generation of it. The model here streams its answer to the latest user text
//! in two deltas; each delta reaches the sink as one segment, committed, and the model is asked
//! each turn with the whole conversation as `llm` items, its own earlier answers among them.

#![cfg(all(feature = "llm", feature = "run"))]

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

use inference_core::generation::GenerationState;
use inference_core::id::{GenerationId, RunId};
use inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_llm::llm_core::{
    BoxFuture, Cancel, Capabilities, Error, Id, Item, Protocol, Provenance, StopReason,
    StreamEvent as LlmEvent, StreamSink, TurnObservation, TurnOutcome, TurnRequest, Usage,
};
use inference_llm::{CONTRACT_REVISION, Generator};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeImplementation,
    NodeRole, OverloadPolicy, PipelineSpec, PlannedPipeline, PortTraits, modality,
};
use inference_runtime::run::RunState;
use inference_runtime::{NodeId, RunController};
use inference_testkit::graph::{PipelinePayload, RunLog, TextSink, TextSource};
use inference_testkit::llm::ModelNode;
use inference_testkit::stream::StreamEvent;

const INPUTS: [&str; 2] = ["hello", "world"];

/// An `llm` model answering the latest user text upper-cased, streamed in two halves, and
/// recording every request it was asked.
#[derive(Debug, Clone)]
struct Shouting {
    provenance: Provenance,
    capabilities: Capabilities,
    asked: Arc<Mutex<Vec<TurnRequest>>>,
}

impl Shouting {
    fn new() -> Self {
        let id = |value: &str| Id::new(value).expect("a printable identifier");
        Self {
            provenance: Provenance {
                protocol: Protocol::Responses,
                provider: id("test"),
                account: id("test"),
                endpoint: id("in-process"),
                model: id("shouting"),
                binding_revision: id("1"),
            },
            capabilities: Capabilities::text(1024, 256),
            asked: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn asked(&self) -> Vec<TurnRequest> {
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl inference_llm::llm_core::Model for Shouting {
    fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn turn<'a>(
        &'a self,
        request: &'a TurnRequest,
        sink: &'a mut dyn StreamSink,
        _cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<TurnOutcome, Error>> {
        Box::pin(async move {
            self.asked
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(request.clone());
            let latest = request.items.iter().rev().find_map(|item| match item {
                Item::UserText { text } => Some(text.to_uppercase()),
                _ => None,
            });
            let mut items = Vec::new();
            if let Some(answer) = latest {
                let (first, second) = answer.split_at(answer.len() / 2);
                for half in [first, second] {
                    sink.emit(LlmEvent::TextDelta {
                        text: half.to_owned(),
                    })
                    .await?;
                }
                items.push(Item::assistant(answer));
            }
            Ok(TurnOutcome {
                stop_reason: StopReason::EndTurn,
                items,
                observation: TurnObservation {
                    usage: Some(Usage {
                        output_tokens: Some(2),
                        ..Usage::default()
                    }),
                    final_usage: true,
                    ..TurnObservation::new(self.provenance.clone())
                },
            })
        })
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

/// Source to model to sink, the model bound to `adapter`.
fn planned(adapter: &Adapter) -> (Declaration, PlannedPipeline, [NodeId; 3]) {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: vec![Locality::SameProcess],
    })
    .expect("a row of the table");
    let profile_id = profile.profile_id();
    let catalog = Fixture {
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
        name: "text over an llm model".to_owned(),
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
                NonZeroUsize::new(16).expect("16"),
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
fn a_model_node_answers_each_input_on_one_generation_of_an_llm_model() {
    let shouting = Shouting::new();
    let (adapter, _) = Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: "an in-process llm model".to_owned(),
        contract_revision: CONTRACT_REVISION.to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities: vec![
            Capability::OutputStreaming,
            Capability::ConcurrentInputOutput,
        ],
        max_concurrency: 1,
    })
    .expect("one is positive");
    let (declaration, planned, [source_id, model_id, sink_id]) = planned(&adapter);
    let generator = Generator::new(adapter, Arc::new(shouting.clone()))
        .expect("the model declares what the adapter claims");

    let run_id = RunId::generate();
    let log = RunLog::new();
    let source = TextSource::new(run_id, INPUTS, log.clone());
    let model = ModelNode::new(run_id, generator, str::to_owned, log.clone());
    let sink = TextSink::new();
    let (source_handle, model_handle, sink_handle) =
        (source.handle(), model.handle(), sink.handle());
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
    assert_eq!(
        model_handle.generation_state(),
        Some(GenerationState::Completed)
    );
    assert_eq!(
        sink_handle.committed(),
        ["HE", "LLO", "WO", "RLD"],
        "each delta of the model is one committed segment"
    );
    let generations: Vec<GenerationId> = sink_handle
        .received()
        .into_iter()
        .filter_map(|envelope| match envelope.payload {
            PipelinePayload::Stream(StreamEvent::TextDelta(delta)) => Some(delta.generation_id),
            _ => None,
        })
        .collect();
    assert_eq!(generations.len(), 4);
    assert!(
        generations.iter().all(|id| *id == generations[0]),
        "one generation answers every input: {generations:?}"
    );

    let asked = shouting.asked();
    let last = asked.last().expect("the model was asked");
    assert_eq!(
        last.items,
        vec![
            Item::user("hello"),
            Item::assistant("HELLO"),
            Item::user("world"),
        ],
        "the last turn carries the conversation as llm items: {asked:?}"
    );
}
