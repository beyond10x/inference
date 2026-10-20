//! Adversary cases for `inference-llm` on `llm` 0.8.0: the claims an adapter record makes
//! against the streaming declarations of its `llm` model, in both directions, and the text a
//! turn returns under `llm`'s rule that a message no delta was shown for carries the text the
//! server sent (`llm` design, "Streamed events are final").

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use inference_core::generation::GenerationState;
use inference_core::id::{NodeId, RunId};
use inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_llm::llm_core::{
    BoxFuture, Cancel, Capabilities, Error, Id, Item, Model, Protocol, Provenance, StopReason,
    StreamEvent, StreamSink, TurnObservation, TurnOutcome, TurnRequest,
};
use inference_llm::{
    CONTRACT_REVISION, CapabilityNotClaimed, CapabilityNotDeclared, GenerationEvent,
    GenerationHandle, GenerationRequest, Generator, InputRefused,
};

/// A model that shows `deltas` and returns one assistant message of `text`; it declares
/// `capabilities`, or `later` once `switched` is set.
struct Declared {
    provenance: Provenance,
    capabilities: Capabilities,
    later: Capabilities,
    switched: Arc<AtomicBool>,
    deltas: Vec<&'static str>,
    text: &'static str,
}

impl Declared {
    fn new(capabilities: Capabilities, deltas: Vec<&'static str>, text: &'static str) -> Self {
        let id = |value: &str| Id::new(value).expect("a printable identifier");
        Self {
            provenance: Provenance {
                protocol: Protocol::Responses,
                provider: id("test"),
                account: id("test"),
                endpoint: id("in-process"),
                model: id("declared"),
                binding_revision: id("1"),
            },
            later: capabilities.clone(),
            capabilities,
            switched: Arc::new(AtomicBool::new(false)),
            deltas,
            text,
        }
    }
}

impl Model for Declared {
    fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    fn capabilities(&self) -> &Capabilities {
        if self.switched.load(Ordering::SeqCst) {
            &self.later
        } else {
            &self.capabilities
        }
    }

    fn turn<'a>(
        &'a self,
        _request: &'a TurnRequest,
        sink: &'a mut dyn StreamSink,
        _cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<TurnOutcome, Error>> {
        Box::pin(async move {
            for text in &self.deltas {
                sink.emit(StreamEvent::TextDelta {
                    text: (*text).to_owned(),
                })
                .await?;
            }
            Ok(TurnOutcome {
                stop_reason: StopReason::EndTurn,
                items: vec![Item::assistant(self.text)],
                observation: TurnObservation::new(self.provenance.clone()),
            })
        })
    }
}

fn adapter(capabilities: Vec<Capability>) -> Adapter {
    Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: "a declared llm model".to_owned(),
        contract_revision: CONTRACT_REVISION.to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities,
        max_concurrency: 1,
    })
    .expect("one is positive")
    .0
}

fn request() -> GenerationRequest {
    GenerationRequest {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
        turn: TurnRequest::new("declared", vec![Item::user("Greet the user.")]),
    }
}

fn texts(handle: &mut GenerationHandle) -> Vec<(String, GenerationState)> {
    let mut seen = Vec::new();
    while let Some(event) = handle.next() {
        if let GenerationEvent::TextDelta(delta) = event {
            seen.push((delta.text, handle.output().generation().state()));
        }
    }
    seen
}

fn text_model() -> Capabilities {
    Capabilities::text(1024, 256)
}

#[test]
fn a_model_that_does_not_stream_and_shows_no_delta_still_gives_its_turn_text_as_one_segment() {
    let model = Declared::new(
        text_model().with_output_streaming(false),
        Vec::new(),
        "Hello",
    );
    let mut handle = Generator::new(adapter(Vec::new()), Arc::new(model))
        .expect("the adapter claims nothing")
        .start(request())
        .expect("a fresh generator runs nothing else");
    assert_eq!(
        texts(&mut handle),
        [("Hello".to_owned(), GenerationState::Completed)],
        "llm: a message no delta was shown for carries the text the server sent"
    );
}

#[test]
fn a_streaming_turn_whose_message_no_delta_showed_still_gives_its_text() {
    let model = Declared::new(text_model(), Vec::new(), "Hello");
    let mut handle = Generator::new(adapter(vec![Capability::OutputStreaming]), Arc::new(model))
        .expect("the model streams")
        .start(request())
        .expect("a fresh generator runs nothing else");
    let seen: Vec<String> = texts(&mut handle)
        .into_iter()
        .map(|(text, _)| text)
        .collect();
    assert_eq!(
        seen,
        ["Hello"],
        "llm: a message no delta was shown for carries the text the server sent"
    );
}

#[test]
fn a_model_declaring_more_than_its_adapter_claims_is_a_generator_held_to_the_claims() {
    let model = Declared::new(
        text_model().with_input_during_output(true),
        vec!["Hel", "lo"],
        "Hello",
    );
    let mut handle = Generator::new(adapter(Vec::new()), Arc::new(model))
        .expect("claiming less than the model declares is no refusal")
        .start(request())
        .expect("a fresh generator runs nothing else");
    let offered = handle.offer_input("during output".to_owned());
    assert_eq!(
        offered,
        Err(InputRefused::CapabilityNotClaimed(CapabilityNotClaimed {
            capability: Capability::ConcurrentInputOutput,
        }))
    );
    assert_eq!(
        texts(&mut handle),
        [("Hello".to_owned(), GenerationState::Completed)]
    );
}

#[test]
fn a_model_declaring_no_streaming_under_an_adapter_claiming_nothing_is_a_generator() {
    let model = Declared::new(
        text_model().with_output_streaming(false),
        vec!["Hel", "lo"],
        "Hello",
    );
    assert!(Generator::new(adapter(Vec::new()), Arc::new(model)).is_ok());
}

#[test]
fn a_model_that_stops_declaring_output_streaming_after_construction_does_not_stream() {
    let model = Declared {
        later: text_model().with_output_streaming(false),
        ..Declared::new(text_model(), vec!["Hel", "lo"], "Hello")
    };
    let switched = Arc::clone(&model.switched);
    let mut generator = Generator::new(adapter(vec![Capability::OutputStreaming]), Arc::new(model))
        .expect("the model streams when the generator is made");
    switched.store(true, Ordering::SeqCst);
    let mut handle = generator
        .start(request())
        .expect("a fresh generator runs nothing else");
    assert_eq!(
        texts(&mut handle),
        [("Hello".to_owned(), GenerationState::Completed)],
        "the start reads the model's declaration, not only the adapter's claim"
    );
}

#[test]
fn a_refusal_names_the_first_undeclared_claim_of_the_adapter() {
    let model = Declared::new(text_model().with_output_streaming(false), Vec::new(), "Hi");
    let adapter = adapter(vec![
        Capability::OutputStreaming,
        Capability::ConcurrentInputOutput,
    ]);
    let first = adapter.capabilities()[0];
    assert_eq!(
        Generator::new(adapter, Arc::new(model)).err(),
        Some(CapabilityNotDeclared { capability: first })
    );
}

#[test]
fn every_declaration_llm_accepts_for_a_protocol_admits_a_concurrent_input_output_claim() {
    for protocol in [
        Protocol::Responses,
        Protocol::Messages,
        Protocol::ChatCompletions,
    ] {
        assert!(
            text_model()
                .with_input_during_output(true)
                .validate_protocol(protocol)
                .is_err(),
            "{protocol:?}: llm refuses input_during_output at bind"
        );
        let accepted = text_model();
        assert!(accepted.validate_protocol(protocol).is_ok());
        assert!(
            Generator::new(
                adapter(vec![
                    Capability::OutputStreaming,
                    Capability::ConcurrentInputOutput
                ]),
                Arc::new(Declared::new(accepted, Vec::new(), "Hi")),
            )
            .is_ok(),
            "{protocol:?}: the wrapper takes input during output as the next turn's text"
        );
    }
}

#[test]
fn an_adapter_claiming_tool_calling_its_model_does_not_declare_is_refused() {
    let model = Declared::new(text_model(), Vec::new(), "Hi");
    assert!(
        !model.capabilities().tools,
        "llm's text declaration takes no tools"
    );
    assert_eq!(
        Generator::new(adapter(vec![Capability::ToolCalling]), Arc::new(model)).err(),
        Some(CapabilityNotDeclared {
            capability: Capability::ToolCalling
        })
    );
}

#[cfg(feature = "tools")]
#[test]
fn a_rejected_call_is_a_failed_result_carrying_the_reason_under_its_ess_name() {
    use inference_llm::tools::ToolOutcome;
    use inference_tools::RejectionReason;
    let outcome = ToolOutcome::Rejected(RejectionReason::Denied);
    assert_eq!(
        (outcome.output(), outcome.failed()),
        (serde_json::json!({"rejected": "denied"}), true)
    );
}
