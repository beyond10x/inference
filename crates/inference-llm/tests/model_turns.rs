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
use inference_core::usage::{Quantity, QuantityKind, UsageRecorded};
use inference_llm::llm_core::{
    BoxFuture, CallId, Cancel, Capabilities, Dispatch, Error, ErrorCode, Id, Item, Model, Protocol,
    Provenance, StopReason, StreamEvent, StreamSink, ToolCall, ToolName, TurnObservation,
    TurnOutcome, TurnRequest, Usage,
};
use inference_llm::{
    CONTRACT_REVISION, CallNotAwaited, CapabilityNotDeclared, GenerationEvent, GenerationHandle,
    GenerationRequest, Generator, OutcomeRefused,
};
use serde_json::json;

/// The upstream model a [`Answer::Text`] turn reports serving.
const UPSTREAM: &str = "scripted-upstream";

/// What the scripted model answers one turn with.
#[derive(Debug, Clone)]
enum Answer {
    /// These deltas, then an outcome ending with these calls, reporting as usage one output
    /// token per delta and the upstream model [`UPSTREAM`].
    Text(Vec<&'static str>, Vec<ToolCall>),
    /// These deltas, then an outcome reporting no usage and no upstream model.
    Unreported(Vec<&'static str>),
    /// A failure, before any delta.
    Fail,
    /// This delta, then nothing until the turn is cancelled.
    Hang(&'static str),
    /// This delta, then nothing until the turn is cancelled, which fails it with `llm`'s
    /// cancelled error carrying this usage as counts not final and the upstream model
    /// [`UPSTREAM`], as `llm`'s clients keep the partial usage of an interrupted stream.
    Cut(&'static str, Usage),
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

    /// The same model, its declaration changed by `declare`.
    fn declaring(mut self, declare: impl FnOnce(Capabilities) -> Capabilities) -> Self {
        self.capabilities = declare(self.capabilities);
        self
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
                Answer::Cut(text, usage) => {
                    sink.emit(StreamEvent::TextDelta {
                        text: text.to_owned(),
                    })
                    .await?;
                    cancel.cancelled().await;
                    Err(Error::cancelled()
                        .with_dispatch(Dispatch::Accepted)
                        .with_observation(TurnObservation {
                            usage: Some(usage),
                            upstream_model: Some(Id::new(UPSTREAM).expect("printable")),
                            final_usage: false,
                            ..TurnObservation::new(self.provenance.clone())
                        }))
                }
                Answer::Unreported(deltas) => {
                    for text in &deltas {
                        sink.emit(StreamEvent::TextDelta {
                            text: (*text).to_owned(),
                        })
                        .await?;
                    }
                    Ok(TurnOutcome {
                        stop_reason: StopReason::EndTurn,
                        items: vec![Item::assistant(deltas.concat())],
                        observation: TurnObservation {
                            final_usage: true,
                            ..TurnObservation::new(self.provenance.clone())
                        },
                    })
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
                            upstream_model: Some(Id::new(UPSTREAM).expect("printable")),
                            final_usage: true,
                            ..TurnObservation::new(self.provenance.clone())
                        },
                    })
                }
            }
        })
    }
}

fn adapter(capabilities: Vec<Capability>) -> Adapter {
    Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: "a scripted llm model".to_owned(),
        contract_revision: CONTRACT_REVISION.to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities,
        max_concurrency: 1,
    })
    .expect("one is positive")
    .0
}

fn generator(capabilities: Vec<Capability>, model: &Scripted) -> Generator {
    Generator::new(adapter(capabilities), Arc::new(model.clone()))
        .expect("the model declares what the adapter claims")
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

/// The kind of each event in `seen`, in order.
fn kinds(seen: &[(GenerationEvent, GenerationState)]) -> Vec<&'static str> {
    seen.iter()
        .map(|(event, _)| match event {
            GenerationEvent::Started(_) => "started",
            GenerationEvent::TextDelta(_) => "delta",
            GenerationEvent::SegmentCommitted(_) => "committed",
            GenerationEvent::Usage(_) => "usage",
            GenerationEvent::Completed(_) => "completed",
            GenerationEvent::Cancelled(..) => "cancelled",
        })
        .collect()
}

/// The usage record of each event in `seen` that carries one, in order.
fn usage(seen: &[(GenerationEvent, GenerationState)]) -> Vec<UsageRecorded> {
    seen.iter()
        .filter_map(|(event, _)| match event {
            GenerationEvent::Usage(recorded) => Some(recorded.clone()),
            _ => None,
        })
        .collect()
}

/// The quantity of `kind` in `recorded`; panics unless it carries exactly one.
fn quantity(recorded: &UsageRecorded, kind: QuantityKind) -> Quantity {
    let found: Vec<&Quantity> = recorded
        .quantities
        .iter()
        .filter(|quantity| quantity.kind() == kind)
        .collect();
    assert_eq!(found.len(), 1, "one {kind:?} quantity: {recorded:?}");
    *found[0]
}

/// The token kinds an LLM record carries.
const TOKEN_KINDS: [QuantityKind; 4] = [
    QuantityKind::InputTextTokens,
    QuantityKind::OutputTextTokens,
    QuantityKind::CachedInputTokens,
    QuantityKind::CacheCreationInputTokens,
];

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
    assert_eq!(
        kinds(&seen),
        [
            "started",
            "delta",
            "committed",
            "delta",
            "committed",
            "usage",
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
fn an_adapter_claiming_output_streaming_its_model_does_not_declare_is_refused() {
    let model = Scripted::new([Answer::Text(vec!["Hel", "lo"], Vec::new())])
        .declaring(|declared| declared.with_output_streaming(false));
    assert_eq!(
        Generator::new(adapter(vec![Capability::OutputStreaming]), Arc::new(model)).err(),
        Some(CapabilityNotDeclared {
            capability: Capability::OutputStreaming
        })
    );
}

#[test]
fn a_concurrent_input_output_claim_needs_no_input_during_output_declaration() {
    let model = Scripted::new([Answer::Text(vec!["Hi"], Vec::new())]);
    assert!(
        !model.capabilities().input_during_output,
        "llm's text declaration takes no input during output"
    );
    let claims = vec![
        Capability::OutputStreaming,
        Capability::ConcurrentInputOutput,
        Capability::ToolCalling,
    ];
    let generator = Generator::new(adapter(claims.clone()), Arc::new(model))
        .expect("the wrapper takes input during output as the next turn's text");
    assert_eq!(generator.adapter().capabilities(), claims);
}

#[test]
fn an_adapter_claiming_native_duplex_its_model_does_not_declare_is_refused() {
    let model = Scripted::new([Answer::Text(vec!["Hi"], Vec::new())]);
    assert_eq!(
        Generator::new(adapter(vec![Capability::NativeDuplex]), Arc::new(model)).err(),
        Some(CapabilityNotDeclared {
            capability: Capability::NativeDuplex
        })
    );
}

#[test]
fn a_model_that_does_not_stream_gives_one_segment_per_turn() {
    let model = Scripted::new([Answer::Text(vec!["Hel", "lo"], Vec::new())])
        .declaring(|declared| declared.with_output_streaming(false));
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
            [(GenerationEvent::Started(_), _), (GenerationEvent::Usage(_), _), (GenerationEvent::Cancelled(_, discarded), GenerationState::Cancelled)] if discarded.is_empty()
        ),
        "GenerationStarted, the usage of the failed turn, then its cancellation: {seen:?}"
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
    let Poll::Ready(Some(GenerationEvent::Usage(recorded))) = next else {
        panic!("the cut turn's record and no text: {next:?}")
    };
    assert!(!recorded.r#final, "no counts were reported: {recorded:?}");
    assert_eq!(
        quantity(&recorded, QuantityKind::Requests),
        Quantity::measured(QuantityKind::Requests, 1),
        "the cut call is still counted"
    );
    for kind in TOKEN_KINDS {
        assert_eq!(quantity(&recorded, kind), Quantity::unknown(kind));
    }
    let next = pin!(handle.next_event()).poll(&mut cx);
    assert_eq!(next, Poll::Ready(None), "nothing more is delivered");
    assert_eq!(
        handle.output().generation().state(),
        GenerationState::Cancelled
    );
}

/// Every event `handle` yields at once, polled without waiting.
fn ready(handle: &mut GenerationHandle) -> Vec<GenerationEvent> {
    let mut cx = Context::from_waker(Waker::noop());
    let mut events = Vec::new();
    while let Poll::Ready(Some(event)) = handle.poll_turn_event(&mut cx) {
        events.push(event);
    }
    events
}

/// How a test ends a generation whose turn runs.
#[derive(Debug, Clone, Copy)]
enum Cut {
    Supersede,
    Cancel,
}

impl Cut {
    const ALL: [Self; 2] = [Self::Supersede, Self::Cancel];

    fn apply(self, handle: &mut GenerationHandle) {
        let generation_id = handle.output().generation().generation_id();
        let cut = match self {
            Self::Supersede => handle.supersede().expect("Active supersedes").generation_id,
            Self::Cancel => handle.cancel().expect("Active cancels").generation_id,
        };
        assert_eq!(cut, generation_id);
    }
}

#[test]
fn a_generation_cut_during_its_second_turn_delivers_that_turns_usage_not_final() {
    for cut in Cut::ALL {
        let model = Scripted::new([
            Answer::Text(vec!["Hel", "lo"], Vec::new()),
            Answer::Cut(
                "Sun",
                Usage {
                    input_tokens: Some(30),
                    output_tokens: Some(1),
                    ..Usage::default()
                },
            ),
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
        let mut seen = Vec::new();
        for event in handle.by_ref() {
            let ended = matches!(event, GenerationEvent::Usage(_));
            seen.push((event, GenerationState::Active));
            if ended {
                break;
            }
        }
        handle
            .offer_input("and the weather?".to_owned())
            .expect("Active, and the adapter claims concurrent input");
        seen.extend(
            ready(&mut handle)
                .into_iter()
                .map(|event| (event, GenerationState::Active)),
        );
        let cancel = model
            .with(|script| script.cancel.clone())
            .expect("the turn was handed a cancel");
        cut.apply(&mut handle);
        assert!(
            cancel.is_cancelled(),
            "{cut:?}: the running turn is told to stop"
        );
        let after = drain(&mut handle);
        assert_eq!(
            kinds(&after),
            ["usage"],
            "{cut:?}: the usage of the turn it cut and nothing else: {after:?}"
        );
        seen.extend(after);

        assert_eq!(
            kinds(&seen),
            [
                "started",
                "delta",
                "committed",
                "delta",
                "committed",
                "usage",
                "delta",
                "committed",
                "usage"
            ],
            "{cut:?}"
        );
        let records = usage(&seen);
        let [ended, partial] = records.as_slice() else {
            panic!("{cut:?}: two records: {records:?}")
        };
        assert!(ended.r#final);
        assert!(!partial.r#final, "{cut:?}: the cut turn did not end");
        assert_eq!(partial.upstream_model.as_deref(), Some(UPSTREAM));
        assert_eq!(
            TOKEN_KINDS.map(|kind| quantity(partial, kind)),
            [
                Quantity::provider_reported(QuantityKind::InputTextTokens, 30),
                Quantity::provider_reported(QuantityKind::OutputTextTokens, 1),
                Quantity::unknown(QuantityKind::CachedInputTokens),
                Quantity::unknown(QuantityKind::CacheCreationInputTokens),
            ],
            "{cut:?}: the counts llm reported for the cut turn"
        );
        assert_eq!(
            quantity(partial, QuantityKind::Requests),
            Quantity::measured(QuantityKind::Requests, 1)
        );
        assert!(partial.started_at_ms <= partial.ended_at_ms);
        assert_ne!(ended.usage_record_id, partial.usage_record_id);
    }
}

#[test]
fn a_usage_record_not_yet_taken_survives_a_supersede_or_a_cancel() {
    for cut in Cut::ALL {
        for streams in [true, false] {
            let model = Scripted::new([Answer::Text(vec!["Hi"], Vec::new())]);
            let capabilities = if streams {
                vec![Capability::OutputStreaming]
            } else {
                Vec::new()
            };
            let mut handle = generator(capabilities, &model)
                .start(request())
                .expect("a fresh generator runs nothing else");
            let mut cx = Context::from_waker(Waker::noop());
            let mut taken = Vec::new();
            loop {
                match handle.poll_turn_event(&mut cx) {
                    Poll::Ready(Some(GenerationEvent::Usage(recorded))) => {
                        panic!("{cut:?}, streams {streams}: taken too early: {recorded:?}")
                    }
                    Poll::Ready(Some(event)) => {
                        let last = matches!(event, GenerationEvent::SegmentCommitted(_));
                        taken.push(event);
                        if last {
                            break;
                        }
                    }
                    Poll::Ready(None) | Poll::Pending => break,
                }
            }
            assert!(
                handle.outcomes().len() == 1,
                "{cut:?}, streams {streams}: the turn ended: {taken:?}"
            );
            cut.apply(&mut handle);
            let after = drain(&mut handle);
            assert_eq!(
                kinds(&after),
                ["usage"],
                "{cut:?}, streams {streams}: the record of the turn that ended is delivered, \
                 and no text: {after:?}"
            );
            assert!(usage(&after)[0].r#final);
        }
    }
}

#[test]
fn completing_a_generation_cuts_its_running_turn_and_delivers_that_turns_usage_before_completed() {
    let model = Scripted::new([Answer::Cut(
        "partial",
        Usage {
            output_tokens: Some(1),
            ..Usage::default()
        },
    )]);
    let mut handle = generator(vec![Capability::OutputStreaming], &model)
        .start(request())
        .expect("a fresh generator runs nothing else");
    let before: Vec<_> = ready(&mut handle)
        .into_iter()
        .map(|event| (event, GenerationState::Active))
        .collect();
    assert_eq!(kinds(&before), ["started", "delta", "committed"]);
    handle.complete().expect("Active completes");
    let after = drain(&mut handle);
    assert_eq!(
        kinds(&after),
        ["usage", "completed"],
        "Completed stays the last event: {after:?}"
    );
    assert!(!usage(&after)[0].r#final);
}

#[test]
fn each_turn_that_ended_yields_one_usage_record_after_its_events() {
    let model = Scripted::new([
        Answer::Text(vec!["Hel", "lo"], Vec::new()),
        Answer::Unreported(vec!["Sunny"]),
    ]);
    let generator = generator(
        vec![
            Capability::OutputStreaming,
            Capability::ConcurrentInputOutput,
        ],
        &model,
    );
    let adapter = generator.adapter().clone();
    let mut handle = generator
        .clone()
        .start(request())
        .expect("a fresh generator runs nothing else");
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after the epoch")
        .as_millis();
    let mut seen = Vec::new();
    for event in handle.by_ref() {
        let ended = matches!(event, GenerationEvent::Usage(_));
        seen.push((event, GenerationState::Active));
        if ended {
            break;
        }
    }
    handle
        .offer_input("and the weather?".to_owned())
        .expect("the generation is Active and claims concurrent input");
    seen.extend(drain(&mut handle));

    assert_eq!(
        kinds(&seen),
        [
            "started",
            "delta",
            "committed",
            "delta",
            "committed",
            "usage",
            "delta",
            "committed",
            "usage",
            "completed"
        ],
        "one record per turn, after the events of that turn"
    );
    let records = usage(&seen);
    for record in &records {
        assert_eq!(record.adapter_id, adapter.adapter_id());
        assert_eq!(record.model_id, adapter.model_id());
        assert_eq!(
            (record.run_id, record.node_id, record.session_id),
            (None, None, None),
            "the contract crate places the record on no run, node or session"
        );
        assert_eq!((record.speech_session_id, record.response_id), (None, None));
        assert!(record.r#final, "the turn reported final counts");
        assert!(
            i128::from(record.started_at_ms) >= i128::try_from(before).expect("fits") - 1_000
                && record.started_at_ms <= record.ended_at_ms,
            "measured around the turn on the wall clock: {record:?}"
        );
        assert_eq!(
            quantity(record, QuantityKind::Requests),
            Quantity::measured(QuantityKind::Requests, 1)
        );
        assert_eq!(
            quantity(record, QuantityKind::LocalComputeMs),
            Quantity::unknown(QuantityKind::LocalComputeMs),
            "no local backend computed it as far as the contract knows"
        );
        assert_eq!(record.quantities.len(), 6, "{record:?}");
    }
    let [reported, unreported] = records.as_slice() else {
        panic!("two records: {records:?}")
    };
    assert_ne!(reported.usage_record_id, unreported.usage_record_id);
    assert_eq!(reported.upstream_model.as_deref(), Some(UPSTREAM));
    assert_eq!(
        quantity(reported, QuantityKind::OutputTextTokens),
        Quantity::provider_reported(QuantityKind::OutputTextTokens, 2)
    );
    for kind in [
        QuantityKind::InputTextTokens,
        QuantityKind::CachedInputTokens,
        QuantityKind::CacheCreationInputTokens,
    ] {
        assert_eq!(
            quantity(reported, kind),
            Quantity::unknown(kind),
            "a count the provider left out is unknown, never zero"
        );
    }
    assert_eq!(unreported.upstream_model, None);
    for kind in TOKEN_KINDS {
        assert_eq!(
            quantity(unreported, kind),
            Quantity::unknown(kind),
            "a turn reporting no usage counts every token unknown"
        );
    }
}

#[test]
fn every_reported_count_maps_to_its_kind_and_reasoning_stays_inside_output() {
    let usage = Usage {
        input_tokens: Some(35),
        output_tokens: Some(9),
        cached_input_tokens: Some(20),
        cache_creation_input_tokens: Some(5),
        reasoning_output_tokens: Some(4),
    };
    let record = inference_llm::usage_of_turn(
        &adapter(Vec::new()),
        Some(&usage),
        Some("served"),
        true,
        (10, 20),
    );
    assert_eq!(
        TOKEN_KINDS.map(|kind| quantity(&record, kind)),
        [
            Quantity::provider_reported(QuantityKind::InputTextTokens, 35),
            Quantity::provider_reported(QuantityKind::OutputTextTokens, 9),
            Quantity::provider_reported(QuantityKind::CachedInputTokens, 20),
            Quantity::provider_reported(QuantityKind::CacheCreationInputTokens, 5),
        ]
    );
    assert_eq!(
        record.quantities.len(),
        6,
        "reasoning is no kind of its own"
    );
    assert_eq!((record.started_at_ms, record.ended_at_ms), (10, 20));
    assert_eq!(record.upstream_model.as_deref(), Some("served"));
    assert!(record.r#final);
}

#[test]
fn a_non_streaming_adapter_delivers_the_usage_after_the_text_it_held() {
    let model = Scripted::new([Answer::Text(vec!["Hel", "lo"], Vec::new())]);
    let mut handle = generator(Vec::new(), &model)
        .start(request())
        .expect("a fresh generator runs nothing else");
    let seen = drain(&mut handle);
    assert_eq!(
        kinds(&seen),
        ["started", "delta", "committed", "usage", "completed"]
    );
}

#[test]
fn a_failed_turn_yields_a_record_of_one_request_with_every_token_count_unknown() {
    let model = Scripted::new([Answer::Fail]);
    let generator = generator(vec![Capability::OutputStreaming], &model);
    let adapter = generator.adapter().clone();
    let mut handle = generator
        .clone()
        .start(request())
        .expect("a fresh generator runs nothing else");
    let seen = drain(&mut handle);
    assert_eq!(kinds(&seen), ["started", "usage", "cancelled"]);
    let records = usage(&seen);
    let [record] = records.as_slice() else {
        panic!("one record: {records:?}")
    };
    assert_eq!(record.adapter_id, adapter.adapter_id());
    assert_eq!(record.model_id, adapter.model_id());
    assert_eq!(
        quantity(record, QuantityKind::Requests),
        Quantity::measured(QuantityKind::Requests, 1),
        "a failed call is still counted as a request"
    );
    for kind in TOKEN_KINDS {
        assert_eq!(quantity(record, kind), Quantity::unknown(kind));
    }
    assert!(
        !record.r#final,
        "the counts of a failed turn are never final"
    );
    assert!(record.started_at_ms <= record.ended_at_ms);
}
