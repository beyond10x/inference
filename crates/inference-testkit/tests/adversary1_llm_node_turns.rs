//! Adversary pass 1 on `story:llm-over-b10x-llm`: the model node of the testkit over an
//! `llm_core::Model` that is not the mock, which its documentation admits ("It answers on any
//! `inference-llm` `Generator`, the mock's among them").
//!
//! Two red cases. The node's first turn asks the model a request with no items before any input
//! arrived; a model that answers it, as a model given only its instructions does, has that answer
//! sent downstream as the reply to the first input, and the first input is never answered on its
//! own. And a turn that fails after streaming leaves the run Completed: the generation is
//! cancelled inside the handle, no `GenerationCancelled` is recorded, the node does not fail, and
//! later inputs are dropped.

#![cfg(all(feature = "llm", feature = "run"))]

use std::collections::HashMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

use inference_core::id::{GenerationId, RunId};
use inference_core::model::{
    Adapter, AdapterId, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_core::policy::{DeclareProfile, DeploymentProfile, Egress, ProfileId, ProfileKind};
use inference_llm::llm_core::{
    BoxFuture, Cancel, Capabilities, Error, ErrorCode, Id, Item, Model, Protocol, Provenance,
    StopReason, StreamEvent as LlmEvent, StreamSink, TurnObservation, TurnOutcome, TurnRequest,
};
use inference_llm::{CONTRACT_REVISION, Generator};
use inference_pipeline::{
    Catalog, Declaration, Delivery, Essentiality, InteractionMode, Node, NodeImplementation,
    NodeRole, OverloadPolicy, PipelineSpec, PlannedPipeline, PortTraits, modality,
};
use inference_runtime::run::RunState;
use inference_runtime::{NodeId, RunController};
use inference_testkit::graph::{PipelinePayload, RunLog, TextSink, TextSource};
use inference_testkit::llm::{ModelHandle, ModelNode};
use inference_testkit::stream::StreamEvent;

/// How a scripted model answers a turn.
#[derive(Debug, Clone, Copy)]
enum Behaviour {
    /// "re:" and the latest user text, or "unprompted" when the request carries none.
    Echo,
    /// Nothing for a request without user text; otherwise the delta "partial", then a failure.
    FailAfterDelta,
}

#[derive(Debug, Clone)]
struct Scripted {
    provenance: Provenance,
    capabilities: Capabilities,
    behaviour: Behaviour,
    asked: Arc<Mutex<Vec<TurnRequest>>>,
}

impl Scripted {
    fn new(behaviour: Behaviour) -> Self {
        let id = |value: &str| Id::new(value).expect("a printable identifier");
        Self {
            provenance: Provenance {
                protocol: Protocol::Messages,
                provider: id("test"),
                account: id("test"),
                endpoint: id("in-process"),
                model: id("scripted"),
                binding_revision: id("1"),
            },
            capabilities: Capabilities::text(1024, 256),
            behaviour,
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

impl Model for Scripted {
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
                Item::UserText { text } => Some(text.clone()),
                _ => None,
            });
            let answer = match (self.behaviour, latest) {
                (Behaviour::Echo, Some(text)) => format!("re:{text}"),
                (Behaviour::Echo, None) => "unprompted".to_owned(),
                (Behaviour::FailAfterDelta, None) => String::new(),
                (Behaviour::FailAfterDelta, Some(_)) => {
                    sink.emit(LlmEvent::TextDelta {
                        text: "partial".to_owned(),
                    })
                    .await?;
                    return Err(Error::new(ErrorCode::Unavailable, "the stream was cut"));
                }
            };
            if !answer.is_empty() {
                sink.emit(LlmEvent::TextDelta {
                    text: answer.clone(),
                })
                .await?;
            }
            Ok(TurnOutcome {
                stop_reason: StopReason::EndTurn,
                items: vec![Item::assistant(answer)],
                observation: TurnObservation {
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

fn planned(adapter: &Adapter) -> (Declaration, PlannedPipeline, [NodeId; 3]) {
    let (profile, _) = DeploymentProfile::declare(DeclareProfile {
        kind: ProfileKind::OfflineStrict,
        egress: Egress::Denied,
        admitted: Vec::new(),
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

/// What a run of source, model node and sink left.
struct Ran {
    state: RunState,
    model: ModelHandle,
    committed: Vec<String>,
    log: RunLog,
}

/// Runs `inputs` through a model node answering on `scripted` until the run ends.
fn run(scripted: &Scripted, inputs: &[&str]) -> Ran {
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
    let generator = Generator::new(adapter, Arc::new(scripted.clone()))
        .expect("the model declares what the adapter claims");
    let run_id = RunId::generate();
    let log = RunLog::new();
    let source = TextSource::new(run_id, inputs.iter().copied(), log.clone());
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
    for _ in inputs {
        assert!(source_handle.release(), "the source admits each input");
    }
    assert!(source_handle.release_end(), "the source admits its end");
    let (controller, result) = drive.join().expect("the drive loop does not panic");
    let _ = result;
    Ran {
        state: controller.run().state(),
        model: model_handle,
        committed: sink_handle.committed(),
        log,
    }
}

/// The node's documentation: "For each `inference.stream.TranscriptCommitted` its input edge
/// delivers it offers the generation its reply to the text, which the next turn asks as user
/// text, and emits [...] the first segment the model answers with." The model is asked once per
/// input, and each committed segment answers an input.
#[test]
fn the_model_node_asks_its_model_once_per_input_and_answers_each() {
    let scripted = Scripted::new(Behaviour::Echo);
    let ran = run(&scripted, &["hello", "world"]);
    assert_eq!(ran.state, RunState::Completed);
    let asked: Vec<Vec<Item>> = scripted
        .asked()
        .into_iter()
        .map(|request| request.items)
        .collect();
    assert_eq!(
        ran.committed,
        ["re:hello", "re:world"],
        "the model was asked: {asked:?}"
    );
    assert!(
        asked.iter().all(|items| !items.is_empty()),
        "a turn with no items was asked: {asked:?}"
    );
}

/// A turn failing after its delta cancels the generation inside the handle (`turn_failed`). The
/// run must not end Completed as if nothing happened: the node fails, or its log records the
/// generation's cancellation ("The framework MUST preserve generation IDs and cancellation
/// status", design §7.2).
#[test]
fn a_failing_turn_is_not_a_completed_run_with_the_failure_unseen() {
    let scripted = Scripted::new(Behaviour::FailAfterDelta);
    let ran = run(&scripted, &["hello", "world"]);
    let cancelled = ran
        .log
        .envelopes()
        .iter()
        .any(|envelope| matches!(envelope.payload, StreamEvent::GenerationCancelled(_)));
    let completed = ran
        .log
        .envelopes()
        .iter()
        .any(|envelope| matches!(envelope.payload, StreamEvent::GenerationCompleted(_)));
    assert!(
        ran.state != RunState::Completed || cancelled || ran.model.failure().is_some(),
        "run {:?}, committed downstream {:?}, GenerationCancelled recorded {cancelled}, \
         GenerationCompleted recorded {completed}, node failure {:?}, dropped inputs {:?}",
        ran.state,
        ran.committed,
        ran.model.failure(),
        ran.model.dropped_inputs(),
    );
}
