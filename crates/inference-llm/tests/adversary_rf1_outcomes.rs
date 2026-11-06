//! Adversary pass 1 on the wrapper's turn outcomes: the stop reason over several turns, a refusal
//! beside text in one turn, a refusal of a turn that ended before a later turn failed, and the
//! `final` mark of a failed turn's counts against `ess/domains/usage.yaml`.
//!
//! Every case runs on its own thread under a wall-clock deadline, so a handle that never ends
//! fails the case instead of hanging the target.

use std::collections::VecDeque;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};
use std::thread;
use std::time::Duration;

use inference_core::generation::StopReason as Stop;
use inference_core::id::{NodeId, RunId};
use inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_core::usage::UsageRecorded;
use inference_llm::llm_core::{
    BoxFuture, CallId, Cancel, Capabilities, Dispatch, Error, ErrorCode, Id, Item, Model, Protocol,
    Provenance, StopReason, StreamEvent, StreamSink, ToolCall, ToolName, TurnObservation,
    TurnOutcome, TurnRequest, Usage,
};
use inference_llm::{
    CONTRACT_REVISION, GenerationEvent, GenerationHandle, GenerationRequest, Generator,
};
use serde_json::json;

const DEADLINE: Duration = Duration::from_secs(20);

/// Runs `case` on its own thread and fails when it has not returned within [`DEADLINE`].
fn within(name: &str, case: impl FnOnce() + Send + 'static) {
    let (done, finished) = mpsc::channel();
    let runner = thread::spawn(move || {
        case();
        let _ = done.send(());
    });
    match finished.recv_timeout(DEADLINE) {
        Ok(()) => runner.join().expect("the case returned"),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            if let Err(panic) = runner.join() {
                std::panic::resume_unwind(panic);
            }
        }
        Err(mpsc::RecvTimeoutError::Timeout) => panic!("{name} ran past {DEADLINE:?}"),
    }
}

/// One scripted turn.
#[derive(Clone)]
enum Step {
    /// This text as one delta, an outcome carrying it, stopped for this reason.
    Text(&'static str, StopReason),
    /// Optionally a delta of the text, then an outcome carrying the text and a refusal.
    TextAndRefusal {
        text: &'static str,
        refusal: &'static str,
        streamed: bool,
    },
    /// An outcome carrying only this refusal, ending its turn.
    Refusal(&'static str),
    /// One call, ending the turn with tool calls.
    Call,
    /// A failure after the provider counted these tokens, its counts marked final as given.
    Fail(Usage, bool),
}

struct Scripted {
    provenance: Provenance,
    capabilities: Capabilities,
    steps: Mutex<VecDeque<Step>>,
}

impl Scripted {
    fn new(steps: Vec<Step>) -> Self {
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
            capabilities: Capabilities {
                tools: true,
                ..Capabilities::text(1024, 256)
            },
            steps: Mutex::new(steps.into()),
        }
    }

    fn outcome(&self, stop_reason: StopReason, items: Vec<Item>) -> TurnOutcome {
        TurnOutcome {
            stop_reason,
            items,
            observation: TurnObservation {
                final_usage: true,
                ..TurnObservation::new(self.provenance.clone())
            },
        }
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
        _request: &'a TurnRequest,
        sink: &'a mut dyn StreamSink,
        _cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<TurnOutcome, Error>> {
        Box::pin(async move {
            let step = self
                .steps
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pop_front()
                .expect("a scripted turn is left");
            match step {
                Step::Text(text, stop_reason) => {
                    sink.emit(StreamEvent::TextDelta {
                        text: text.to_owned(),
                    })
                    .await?;
                    Ok(self.outcome(stop_reason, vec![Item::assistant(text)]))
                }
                Step::TextAndRefusal {
                    text,
                    refusal,
                    streamed,
                } => {
                    if streamed {
                        sink.emit(StreamEvent::TextDelta {
                            text: text.to_owned(),
                        })
                        .await?;
                        sink.emit(StreamEvent::RefusalDelta {
                            text: refusal.to_owned(),
                        })
                        .await?;
                    }
                    Ok(self.outcome(
                        StopReason::EndTurn,
                        vec![
                            Item::assistant(text),
                            Item::AssistantRefusal {
                                text: refusal.to_owned(),
                            },
                        ],
                    ))
                }
                Step::Refusal(text) => Ok(self.outcome(
                    StopReason::EndTurn,
                    vec![Item::AssistantRefusal {
                        text: text.to_owned(),
                    }],
                )),
                Step::Call => Ok(self.outcome(
                    StopReason::ToolCalls,
                    vec![Item::ToolCall(ToolCall {
                        call_id: CallId::new("call-1").expect("a printable identifier"),
                        name: ToolName::new("lookup").expect("a printable identifier"),
                        arguments: json!({}),
                    })],
                )),
                Step::Fail(usage, final_usage) => {
                    Err(Error::new(ErrorCode::Unavailable, "the stream broke")
                        .with_dispatch(Dispatch::Accepted)
                        .with_observation(TurnObservation {
                            usage: Some(usage),
                            final_usage,
                            ..TurnObservation::new(self.provenance.clone())
                        }))
                }
            }
        })
    }
}

fn start(steps: Vec<Step>, capabilities: Vec<Capability>) -> GenerationHandle {
    let adapter = Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: "a scripted llm model".to_owned(),
        contract_revision: CONTRACT_REVISION.to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities,
        max_concurrency: 1,
    })
    .expect("one is positive")
    .0;
    Generator::new(adapter, Arc::new(Scripted::new(steps)))
        .expect("the model declares what the adapter claims")
        .start(GenerationRequest {
            run_id: RunId::generate(),
            node_id: NodeId::generate(),
            epoch: 1,
            turn: TurnRequest::new("scripted", vec![Item::user("hello")]),
        })
        .expect("a fresh generator runs nothing else")
}

/// Polls the first turn to its end and takes `GenerationStarted`, leaving the rest queued.
fn run_first_turn(handle: &mut GenerationHandle) {
    let mut cx = Context::from_waker(Waker::noop());
    let Poll::Ready(Some(GenerationEvent::Started(_))) = handle.poll_turn_event(&mut cx) else {
        panic!("the first event is GenerationStarted")
    };
}

fn completed_stop_reason(seen: &[GenerationEvent]) -> Option<Stop> {
    let Some(GenerationEvent::Completed(completed)) = seen.last() else {
        panic!("the generation completes: {seen:?}")
    };
    completed.stop_reason
}

fn position(seen: &[GenerationEvent], wanted: fn(&GenerationEvent) -> bool) -> usize {
    seen.iter()
        .position(wanted)
        .unwrap_or_else(|| panic!("the event is delivered: {seen:?}"))
}

#[test]
fn adversary_rf1_a_tool_round_then_a_truncated_turn_completes_truncated() {
    within("tool round then truncation", || {
        let mut handle = start(
            vec![
                Step::Call,
                Step::Text("cut sho", StopReason::MaxOutputTokens),
            ],
            vec![Capability::OutputStreaming, Capability::ToolCalling],
        );
        let mut cx = Context::from_waker(Waker::noop());
        while let Poll::Ready(Some(_)) = handle.poll_turn_event(&mut cx) {}
        let call = handle
            .take_proposal()
            .expect("the first turn ended with a call");
        handle
            .integrate(&call.call_id, json!({ "ok": true }), false)
            .expect("the call is awaited");
        let seen: Vec<GenerationEvent> = handle.by_ref().collect();
        assert_eq!(
            completed_stop_reason(&seen),
            Some(Stop::MaxOutputTokens),
            "the stop reason is the last turn's, not the tool round's: {seen:?}"
        );
    });
}

#[test]
fn adversary_rf1_a_truncated_turn_then_an_input_turn_completes_with_the_later_reason() {
    within("truncation then input turn", || {
        let mut handle = start(
            vec![
                Step::Text("first, cut", StopReason::MaxOutputTokens),
                Step::Text("second, whole", StopReason::EndTurn),
            ],
            vec![
                Capability::OutputStreaming,
                Capability::ConcurrentInputOutput,
            ],
        );
        run_first_turn(&mut handle);
        handle
            .offer_input("go on".to_owned())
            .expect("the adapter claims concurrent input and output");
        let seen: Vec<GenerationEvent> = handle.by_ref().collect();
        let usages = seen
            .iter()
            .filter(|event| matches!(event, GenerationEvent::Usage(_)))
            .count();
        assert_eq!(usages, 2, "two turns ran: {seen:?}");
        assert_eq!(
            completed_stop_reason(&seen),
            Some(Stop::EndTurn),
            "the earlier turn's truncation does not stick to the generation: {seen:?}"
        );
    });
}

#[test]
fn adversary_rf1_text_and_a_refusal_in_one_turn_keep_their_order_and_stay_apart() {
    within("text and refusal in one turn", || {
        for streamed in [true, false] {
            for capabilities in [vec![Capability::OutputStreaming], Vec::new()] {
                let mut handle = start(
                    vec![Step::TextAndRefusal {
                        text: "Partial answer.",
                        refusal: "I will not go on.",
                        streamed,
                    }],
                    capabilities.clone(),
                );
                let seen: Vec<GenerationEvent> = handle.by_ref().collect();
                let segments: Vec<&str> = seen
                    .iter()
                    .filter_map(|event| match event {
                        GenerationEvent::TextDelta(delta) => Some(delta.text.as_str()),
                        _ => None,
                    })
                    .collect();
                assert_eq!(
                    segments,
                    ["Partial answer."],
                    "the text is one segment, the refusal no part of it \
                     (streamed {streamed}, {capabilities:?}): {seen:?}"
                );
                let delta = position(&seen, |event| {
                    matches!(event, GenerationEvent::TextDelta(_))
                });
                let committed = position(&seen, |event| {
                    matches!(event, GenerationEvent::SegmentCommitted(_))
                });
                let refused = position(&seen, |event| matches!(event, GenerationEvent::Refused(_)));
                let usage = position(&seen, |event| matches!(event, GenerationEvent::Usage(_)));
                assert!(
                    delta < committed && committed < refused && refused < usage,
                    "the refusal comes after the turn's text and before its usage record \
                     (streamed {streamed}, {capabilities:?}): {seen:?}"
                );
                assert_eq!(completed_stop_reason(&seen), Some(Stop::EndTurn));
            }
        }
    });
}

#[test]
fn adversary_rf1_the_refusal_of_an_ended_turn_survives_a_later_failed_turn() {
    within("refusal then failure", || {
        for capabilities in [
            vec![
                Capability::OutputStreaming,
                Capability::ConcurrentInputOutput,
            ],
            vec![Capability::ConcurrentInputOutput],
        ] {
            let mut handle = start(
                vec![Step::Refusal("No."), Step::Fail(Usage::default(), false)],
                capabilities.clone(),
            );
            run_first_turn(&mut handle);
            handle
                .offer_input("please".to_owned())
                .expect("the adapter claims concurrent input and output");
            let seen: Vec<GenerationEvent> = handle.by_ref().collect();
            assert!(
                matches!(seen.last(), Some(GenerationEvent::Cancelled(..))),
                "the failed second turn cancels the generation: {seen:?}"
            );
            assert!(
                seen.iter().any(|event| matches!(
                    event,
                    GenerationEvent::Refused(refused) if refused.text == "No."
                )),
                "the refusal of the first turn, which ended, is delivered whether or not the \
                 adapter streams ({capabilities:?}); the streaming handle delivers it: {seen:?}"
            );
        }
    });
}

#[test]
fn adversary_rf1_a_failed_turn_keeps_the_final_mark_the_provider_gave_its_counts() {
    within("failed turn final counts", || {
        let usage = Usage {
            input_tokens: Some(40),
            output_tokens: Some(9),
            ..Usage::default()
        };
        let mut handle = start(
            vec![Step::Fail(usage, true)],
            vec![Capability::OutputStreaming],
        );
        let records: Vec<UsageRecorded> = handle
            .by_ref()
            .filter_map(|event| match event {
                GenerationEvent::Usage(recorded) => Some(recorded),
                _ => None,
            })
            .collect();
        let [record] = records.as_slice() else {
            panic!("one record: {records:?}")
        };
        assert!(
            record.r#final,
            "ess/domains/usage.yaml declares `final` false only when the provider marked its \
             counts partial, and the cut-turn path keeps a final mark on an error; a failed \
             turn whose provider marked its counts final records them as not final: {record:?}"
        );
    });
}
