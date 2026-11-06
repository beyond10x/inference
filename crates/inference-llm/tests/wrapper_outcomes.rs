//! What the wrapper keeps of a turn and what it shows of a generation.
//!
//! A turn that fails after the provider counted some of it records those counts, the rest
//! unknown and none final, as a turn its generation's end cut does: tokens billed before a stream
//! failed are not counted unknown. The `Debug` form of a handle carries no text of the
//! conversation: no instructions, no item, no input offered while it runs, no output and no tool
//! call arguments, as the `Debug` form of an admitted input carries none.

use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};

use inference_core::id::{NodeId, RunId};
use inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_core::usage::{Quantity, QuantityKind, UsageRecorded};
use inference_llm::llm_core::{
    BoxFuture, CallId, Cancel, Capabilities, Dispatch, Error, ErrorCode, Id, Item, Model, Protocol,
    Provenance, StopReason, StreamEvent, StreamSink, ToolCall, ToolName, TurnObservation,
    TurnOutcome, TurnRequest, Usage,
};
use inference_llm::{
    CONTRACT_REVISION, GenerationEvent, GenerationHandle, GenerationRequest, Generator,
};
use serde_json::json;

/// The marker every text of the conversation carries in the `Debug` case.
const MARK: &str = "do-not-print";

/// The upstream model the failing turn reports serving.
const UPSTREAM: &str = "partial-upstream";

/// What the model answers its one turn with.
#[derive(Clone)]
enum Answer {
    /// A failure after the provider counted these tokens, its counts marked final as given.
    FailCounted(Usage, bool),
    /// A delta carrying [`MARK`], then an outcome ending with one call whose arguments carry it.
    MarkedCall,
}

struct OneTurn {
    provenance: Provenance,
    capabilities: Capabilities,
    answer: Answer,
    asked: Arc<Mutex<usize>>,
}

impl OneTurn {
    fn new(answer: Answer) -> Self {
        let id = |value: &str| Id::new(value).expect("a printable identifier");
        Self {
            provenance: Provenance {
                protocol: Protocol::Messages,
                provider: id("test"),
                account: id("test"),
                endpoint: id("in-process"),
                model: id("one-turn"),
                binding_revision: id("1"),
            },
            capabilities: Capabilities {
                tools: true,
                ..Capabilities::text(1024, 256)
            },
            answer,
            asked: Arc::default(),
        }
    }
}

impl Model for OneTurn {
    fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    fn turn<'a>(
        &'a self,
        _request: &'a TurnRequest,
        sink: &'a mut dyn StreamSink,
        _cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<TurnOutcome, Error>> {
        Box::pin(async move {
            *self.asked.lock().unwrap_or_else(PoisonError::into_inner) += 1;
            match self.answer.clone() {
                Answer::FailCounted(usage, final_usage) => {
                    Err(Error::new(ErrorCode::Unavailable, "the stream broke")
                        .with_dispatch(Dispatch::Accepted)
                        .with_observation(TurnObservation {
                            usage: Some(usage),
                            upstream_model: Some(Id::new(UPSTREAM).expect("printable")),
                            final_usage,
                            ..TurnObservation::new(self.provenance.clone())
                        }))
                }
                Answer::MarkedCall => {
                    let text = format!("output {MARK}");
                    sink.emit(StreamEvent::TextDelta { text: text.clone() })
                        .await?;
                    let call = ToolCall {
                        call_id: CallId::new("call-1").expect("a printable identifier"),
                        name: ToolName::new("lookup").expect("a printable identifier"),
                        arguments: json!({ "query": format!("arguments {MARK}") }),
                    };
                    Ok(TurnOutcome {
                        stop_reason: StopReason::ToolCalls,
                        items: vec![Item::assistant(text), Item::ToolCall(call)],
                        observation: TurnObservation {
                            final_usage: true,
                            ..TurnObservation::new(self.provenance.clone())
                        },
                    })
                }
            }
        })
    }
}

fn start(answer: Answer, capabilities: Vec<Capability>) -> GenerationHandle {
    let adapter = Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: "a one-turn llm model".to_owned(),
        contract_revision: CONTRACT_REVISION.to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities,
        max_concurrency: 1,
    })
    .expect("one is positive")
    .0;
    let mut turn = TurnRequest::new("one-turn", vec![Item::user(format!("user {MARK}"))]);
    turn.instructions = format!("instructions {MARK}");
    Generator::new(adapter, Arc::new(OneTurn::new(answer)))
        .expect("the model declares what the adapter claims")
        .start(GenerationRequest {
            run_id: RunId::generate(),
            node_id: NodeId::generate(),
            epoch: 1,
            turn,
        })
        .expect("a fresh generator runs nothing else")
}

fn quantity(recorded: &UsageRecorded, kind: QuantityKind) -> Quantity {
    let found: Vec<&Quantity> = recorded
        .quantities
        .iter()
        .filter(|quantity| quantity.kind() == kind)
        .collect();
    assert_eq!(found.len(), 1, "one {kind:?} quantity: {recorded:?}");
    *found[0]
}

fn failed_turn_record(final_usage: bool) -> UsageRecorded {
    let usage = Usage {
        input_tokens: Some(12),
        output_tokens: Some(3),
        ..Usage::default()
    };
    let mut handle = start(
        Answer::FailCounted(usage, final_usage),
        vec![Capability::OutputStreaming],
    );
    let seen: Vec<GenerationEvent> = handle.by_ref().collect();
    assert!(
        matches!(seen.last(), Some(GenerationEvent::Cancelled(..))),
        "a failed turn cancels its generation: {seen:?}"
    );
    let records: Vec<UsageRecorded> = seen
        .into_iter()
        .filter_map(|event| match event {
            GenerationEvent::Usage(recorded) => Some(recorded),
            _ => None,
        })
        .collect();
    let [record] = records.as_slice() else {
        panic!("one record: {records:?}")
    };
    record.clone()
}

#[test]
fn a_turn_failing_with_partial_usage_records_those_counts_not_final() {
    for final_usage in [false, true] {
        let record = failed_turn_record(final_usage);
        assert_eq!(
            quantity(&record, QuantityKind::InputTextTokens),
            Quantity::provider_reported(QuantityKind::InputTextTokens, 12),
            "the input the provider counted before the turn failed is recorded, not unknown"
        );
        assert_eq!(
            quantity(&record, QuantityKind::OutputTextTokens),
            Quantity::provider_reported(QuantityKind::OutputTextTokens, 3),
            "the output the provider counted before the turn failed is recorded, not unknown"
        );
        for kind in [
            QuantityKind::CachedInputTokens,
            QuantityKind::CacheCreationInputTokens,
        ] {
            assert_eq!(
                quantity(&record, kind),
                Quantity::unknown(kind),
                "a count the provider left out stays unknown"
            );
        }
        assert_eq!(
            quantity(&record, QuantityKind::Requests),
            Quantity::measured(QuantityKind::Requests, 1)
        );
        assert_eq!(record.upstream_model.as_deref(), Some(UPSTREAM));
        assert!(
            !record.r#final,
            "the counts of a failed turn are never final, whatever the observation marked"
        );
    }
}

#[test]
fn the_debug_form_of_a_handle_carries_no_text_of_the_conversation() {
    let mut handle = start(
        Answer::MarkedCall,
        vec![
            Capability::OutputStreaming,
            Capability::ConcurrentInputOutput,
            Capability::ToolCalling,
        ],
    );
    let mut cx = Context::from_waker(Waker::noop());
    let Poll::Ready(Some(GenerationEvent::Started(started))) = handle.poll_turn_event(&mut cx)
    else {
        panic!("the first event is GenerationStarted")
    };
    handle
        .offer_input(format!("input {MARK}"))
        .expect("the adapter claims concurrent input and output");
    let mut shown = vec![format!("{handle:?}"), format!("{handle:#?}")];
    while let Poll::Ready(Some(_)) = handle.poll_turn_event(&mut cx) {}
    shown.push(format!("{handle:?}"));
    let proposal = handle.take_proposal().expect("the turn ended with a call");
    handle
        .integrate(&proposal.call_id, json!({ "result": MARK }), false)
        .expect("the call is awaited");
    shown.push(format!("{handle:?}"));
    for debug in &shown {
        assert!(
            !debug.contains(MARK),
            "the Debug form of a handle carries no instructions, items, input, output or tool \
             arguments: {debug}"
        );
        assert!(
            debug.contains(&started.generation_id.to_string())
                || debug.contains(&format!("{:?}", started.generation_id)),
            "the Debug form still names its generation: {debug}"
        );
    }
}
