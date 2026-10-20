//! `inference-llm` over `llm_core::Model`: a generation runs one `Model::turn` on the request it
//! was started with and one more for each input or round of tool results, and adds only what the
//! graph needs. A streaming adapter's deltas become committed segments delivered while the turn
//! still runs; a non-streaming adapter's text arrives as one segment at completion. Tool calls
//! are `llm`'s, and the next turn runs once every call of a round is answered. A failing turn
//! cancels the generation and keeps `llm`'s error; a cancel reaches the running turn.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};

use inference_core::generation::GenerationState;
use inference_core::id::{NodeId, RunId};
use inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_llm::llm_core::{
    BoxFuture, CallId, Cancel, Capabilities, Error, ErrorCode, Id, Item, Model, Protocol,
    Provenance, StopReason, StreamEvent, StreamSink, ToolCall, ToolName, TurnObservation,
    TurnOutcome, TurnRequest, Usage,
};
use inference_llm::{
    CONTRACT_REVISION, CallNotAwaited, GenerationEvent, GenerationHandle, GenerationRequest,
    Generator, OutcomeRefused,
};
use serde_json::json;

/// What the scripted model answers one turn with.
#[derive(Debug, Clone)]
enum Answer {
    /// These deltas, then an outcome ending with these calls.
    Text(Vec<&'static str>, Vec<ToolCall>),
    /// A failure, before any delta.
    Fail,
    /// This delta, then nothing until the turn is cancelled.
    Hang(&'static str),
}

#[derive(Debug, Default)]
struct Script {
    answers: VecDeque<Answer>,
    asked: Vec<TurnRequest>,
    cancel: Option<Cancel>,
}

#[derive(Debug, Clone)]
struct Scripted {
    provenance: Provenance,
    capabilities: Capabilities,
    script: Arc<Mutex<Script>>,
}

impl Scripted {
    fn new(answers: impl IntoIterator<Item = Answer>) -> Self {
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
            script: Arc::new(Mutex::new(Script {
                answers: answers.into_iter().collect(),
                ..Script::default()
            })),
        }
    }

    fn with<T>(&self, read: impl FnOnce(&mut Script) -> T) -> T {
        read(&mut self.script.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn asked(&self) -> Vec<TurnRequest> {
        self.with(|script| script.asked.clone())
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
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<TurnOutcome, Error>> {
        Box::pin(async move {
            let answer = self.with(|script| {
                script.asked.push(request.clone());
                script.cancel = Some(cancel.clone());
                script.answers.pop_front()
            });
            match answer.expect("the script answers every turn it is asked") {
                Answer::Fail => Err(Error::new(ErrorCode::Unavailable, "scripted to fail")),
                Answer::Hang(text) => {
                    sink.emit(StreamEvent::TextDelta {
                        text: text.to_owned(),
                    })
                    .await?;
                    cancel.cancelled().await;
                    Err(Error::cancelled())
                }
                Answer::Text(deltas, calls) => {
                    for text in &deltas {
                        sink.emit(StreamEvent::TextDelta {
                            text: (*text).to_owned(),
                        })
                        .await?;
                    }
                    let stop_reason = if calls.is_empty() {
                        StopReason::EndTurn
                    } else {
                        StopReason::ToolCalls
                    };
                    let mut items = vec![Item::assistant(deltas.concat())];
                    items.extend(calls.into_iter().map(Item::ToolCall));
                    Ok(TurnOutcome {
                        stop_reason,
                        items,
                        observation: TurnObservation {
                            usage: Some(Usage {
                                output_tokens: Some(u64::try_from(deltas.len()).unwrap_or(0)),
                                ..Usage::default()
                            }),
                            final_usage: true,
                            ..TurnObservation::new(self.provenance.clone())
                        },
                    })
                }
            }
        })
    }
}

fn generator(capabilities: Vec<Capability>, model: &Scripted) -> Generator {
    let (adapter, _) = Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: "a scripted llm model".to_owned(),
        contract_revision: CONTRACT_REVISION.to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities,
        max_concurrency: 1,
    })
    .expect("one is positive");
    Generator::new(adapter, Arc::new(model.clone()))
}

fn request() -> GenerationRequest {
    GenerationRequest {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
        turn: TurnRequest::new("scripted", vec![Item::user("Greet the user.")]),
    }
}

fn call(id: &str) -> ToolCall {
    ToolCall {
        call_id: CallId::new(id).expect("a printable identifier"),
        name: ToolName::new("lookup").expect("a printable identifier"),
        arguments: json!({"city": id}),
    }
}

/// Every event `handle` yields until it yields none, with the generation's state at each.
fn drain(handle: &mut GenerationHandle) -> Vec<(GenerationEvent, GenerationState)> {
    let mut seen = Vec::new();
    while let Some(event) = handle.next() {
        seen.push((event, handle.output().generation().state()));
    }
    seen
}

fn texts(seen: &[(GenerationEvent, GenerationState)]) -> Vec<(String, GenerationState)> {
    seen.iter()
        .filter_map(|(event, state)| match event {
            GenerationEvent::TextDelta(delta) => Some((delta.text.clone(), *state)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_streaming_adapter_delivers_each_delta_as_a_committed_segment_while_active() {
    let model = Scripted::new([Answer::Text(vec!["Hel", "lo"], Vec::new())]);
    let mut handle = generator(vec![Capability::OutputStreaming], &model)
        .start(request())
        .expect("a fresh generator runs nothing else");
    let seen = drain(&mut handle);
    let kinds: Vec<&str> = seen
        .iter()
        .map(|(event, _)| match event {
            GenerationEvent::Started(_) => "started",
            GenerationEvent::TextDelta(_) => "delta",
            GenerationEvent::SegmentCommitted(_) => "committed",
            GenerationEvent::Completed(_) => "completed",
            GenerationEvent::Cancelled(..) => "cancelled",
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "started",
            "delta",
            "committed",
            "delta",
            "committed",
            "completed"
        ]
    );
    assert_eq!(
        texts(&seen),
        [
            ("Hel".to_owned(), GenerationState::Active),
            ("lo".to_owned(), GenerationState::Active)
        ]
    );
    assert_eq!(
        model.asked(),
        vec![request().turn],
        "the request's own turn, as given"
    );
    assert_eq!(
        handle.conversation().items,
        vec![Item::user("Greet the user."), Item::assistant("Hello")],
        "the model's output items join the conversation"
    );
    assert_eq!(
        handle.outcomes()[0]
            .observation
            .usage
            .as_ref()
            .and_then(|usage| usage.output_tokens),
        Some(2),
        "usage is llm's, kept per turn"
    );
}

#[test]
fn a_non_streaming_adapter_delivers_the_text_as_one_segment_at_completion() {
    let model = Scripted::new([Answer::Text(vec!["Hel", "lo"], Vec::new())]);
    let mut handle = generator(Vec::new(), &model)
        .start(request())
        .expect("a fresh generator runs nothing else");
    let seen = drain(&mut handle);
    assert_eq!(
        texts(&seen),
        [("Hello".to_owned(), GenerationState::Completed)]
    );
}

#[test]
fn input_offered_while_active_is_the_next_turn_as_user_text() {
    let model = Scripted::new([
        Answer::Text(vec!["Hi"], Vec::new()),
        Answer::Text(vec!["Sunny"], Vec::new()),
    ]);
    let mut handle = generator(
        vec![
            Capability::OutputStreaming,
            Capability::ConcurrentInputOutput,
        ],
        &model,
    )
    .start(request())
    .expect("a fresh generator runs nothing else");
    assert!(matches!(handle.next(), Some(GenerationEvent::Started(_))));
    assert_eq!(handle.offer_input("and the weather?".to_owned()), Ok(()));
    let seen = drain(&mut handle);
    assert_eq!(
        texts(&seen),
        [
            ("Hi".to_owned(), GenerationState::Active),
            ("Sunny".to_owned(), GenerationState::Active)
        ]
    );
    assert_eq!(
        model.asked()[1].items,
        vec![
            Item::user("Greet the user."),
            Item::assistant("Hi"),
            Item::user("and the weather?"),
        ]
    );
}

#[test]
fn the_next_turn_runs_once_every_call_of_the_round_is_answered() {
    let model = Scripted::new([
        Answer::Text(vec!["Looking"], vec![call("a"), call("b")]),
        Answer::Text(vec!["Both"], Vec::new()),
    ]);
    let mut handle = generator(
        vec![Capability::OutputStreaming, Capability::ToolCalling],
        &model,
    )
    .start(request())
    .expect("a fresh generator runs nothing else");
    let mut delivered = Vec::new();
    for event in handle.by_ref() {
        let committed = matches!(event, GenerationEvent::SegmentCommitted(_));
        delivered.push(event);
        if committed {
            break;
        }
    }
    assert_eq!(handle.take_proposal(), Some(call("a")));
    assert_eq!(handle.take_proposal(), Some(call("b")));
    assert_eq!(handle.take_proposal(), None);

    let unknown = CallId::new("c").expect("a printable identifier");
    assert_eq!(
        handle.integrate(&unknown, json!(1), false),
        Err(OutcomeRefused::NotAwaited(CallNotAwaited {
            call_id: unknown
        })),
        "a result answers a call of the latest round only"
    );
    assert_eq!(
        handle.integrate(&call("a").call_id, json!({"temp": 20}), false),
        Ok(())
    );
    assert_eq!(
        handle.integrate(&call("a").call_id, json!({"temp": 21}), false),
        Err(OutcomeRefused::NotAwaited(CallNotAwaited {
            call_id: call("a").call_id
        })),
        "a call is answered once"
    );
    assert_eq!(
        handle.integrate(&call("b").call_id, json!("down"), true),
        Ok(())
    );
    let seen = drain(&mut handle);
    assert_eq!(texts(&seen), [("Both".to_owned(), GenerationState::Active)]);
    let asked = model.asked();
    assert_eq!(asked.len(), 2, "one turn answers the whole round");
    assert_eq!(
        asked[1].items[1..],
        [
            Item::assistant("Looking"),
            Item::ToolCall(call("a")),
            Item::ToolCall(call("b")),
            Item::ToolResult {
                call_id: call("a").call_id,
                output: json!({"temp": 20}),
                failed: false,
            },
            Item::ToolResult {
                call_id: call("b").call_id,
                output: json!("down"),
                failed: true,
            },
        ]
    );
    assert!(
        asked[1].validate().is_ok(),
        "the turn after the round is a valid llm request"
    );
}

#[test]
fn a_failing_turn_cancels_the_generation_and_keeps_the_llm_error() {
    let model = Scripted::new([Answer::Fail]);
    let mut handle = generator(vec![Capability::OutputStreaming], &model)
        .start(request())
        .expect("a fresh generator runs nothing else");
    let seen = drain(&mut handle);
    assert!(
        matches!(
            seen.as_slice(),
            [(GenerationEvent::Started(_), _), (GenerationEvent::Cancelled(_, discarded), GenerationState::Cancelled)] if discarded.is_empty()
        ),
        "GenerationStarted, then the cancellation of the failed turn: {seen:?}"
    );
    assert_eq!(
        handle.output().generation().state(),
        GenerationState::Cancelled
    );
    assert_eq!(
        handle.failure().map(|error| error.code),
        Some(ErrorCode::Unavailable)
    );
}

#[test]
fn a_delta_reaches_the_caller_while_the_turn_runs_and_a_cancel_reaches_the_turn() {
    let model = Scripted::new([Answer::Hang("partial")]);
    let mut handle = generator(vec![Capability::OutputStreaming], &model)
        .start(request())
        .expect("a fresh generator runs nothing else");
    let mut cx = Context::from_waker(Waker::noop());
    let mut events = Vec::new();
    while let Poll::Ready(Some(event)) = handle.poll_event(&mut cx) {
        events.push(event);
    }
    assert!(
        events.iter().any(
            |event| matches!(event, GenerationEvent::TextDelta(delta) if delta.text == "partial")
        ),
        "the delta arrived before the turn ended: {events:?}"
    );
    let cancel = model
        .with(|script| script.cancel.clone())
        .expect("the turn was handed a cancel");
    assert!(!cancel.is_cancelled());
    handle.cancel().expect("Active cancels");
    assert!(cancel.is_cancelled(), "the running turn is told to stop");
    let next = pin!(handle.next_event()).poll(&mut cx);
    assert_eq!(next, Poll::Ready(None), "nothing more is delivered");
    assert_eq!(
        handle.output().generation().state(),
        GenerationState::Cancelled
    );
}
