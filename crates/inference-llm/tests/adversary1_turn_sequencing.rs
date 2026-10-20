//! Adversary pass 1 on `story:llm-over-b10x-llm`: the turn sequencing of `GenerationHandle` over
//! an `llm_core::Model` whose stream the test feeds one event at a time, so that a turn is still
//! running while the caller acts on the generation.
//!
//! Red cases: asked for its next event when no turn runs and calls are unanswered, the handle
//! completes the generation, and the poll that ends a turn asks that question at once unless the
//! same poll queued an event. So a round cannot be answered when the turn ends with calls and no
//! text, when its end arrives in a later poll than its last delta, or when the adapter does not
//! stream; and `tools::tool_id` resolves a name that `tools::tool_name` never gives. The other
//! cases guard behaviour the unit's own cases leave unpinned: a delta fed after a supersede or a
//! cancel is not delivered, call announcements and argument fragments are neither text nor
//! proposals, an empty delta is no segment, a round answered in part runs no turn, and a round
//! answered out of order gives a valid request.

#![cfg(feature = "tools")]

use std::collections::VecDeque;
use std::future;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};

use inference_core::generation::GenerationState;
use inference_core::id::{NodeId, RunId};
use inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_llm::llm_core::{
    BoxFuture, CallId, Cancel, Capabilities, Error, Id, Item, Model, Protocol, Provenance,
    StopReason, StreamEvent, StreamSink, ToolCall, ToolName, TurnObservation, TurnOutcome,
    TurnRequest,
};
use inference_llm::tools::{tool_id, tool_name};
use inference_llm::{
    CONTRACT_REVISION, GenerationEvent, GenerationHandle, GenerationRequest, Generator,
    OutcomeRefused,
};
use inference_tools::ToolId;
use serde_json::json;

/// What the test feeds the running turn.
enum Feed {
    /// A stream event, emitted into the turn's sink.
    Event(StreamEvent),
    /// The end of the turn, with these calls.
    End(Vec<ToolCall>),
}

#[derive(Default)]
struct Fed {
    queue: VecDeque<Feed>,
    waker: Option<Waker>,
    asked: Vec<TurnRequest>,
}

/// A model whose turns stream exactly what the test feeds, waiting while nothing is fed.
#[derive(Clone)]
struct Feeding {
    provenance: Provenance,
    capabilities: Capabilities,
    fed: Arc<Mutex<Fed>>,
}

impl Feeding {
    fn new() -> Self {
        let id = |value: &str| Id::new(value).expect("a printable identifier");
        Self {
            provenance: Provenance {
                protocol: Protocol::Responses,
                provider: id("test"),
                account: id("test"),
                endpoint: id("in-process"),
                model: id("fed"),
                binding_revision: id("1"),
            },
            capabilities: Capabilities {
                tools: true,
                ..Capabilities::text(1024, 256)
            },
            fed: Arc::new(Mutex::new(Fed::default())),
        }
    }

    fn with<T>(&self, act: impl FnOnce(&mut Fed) -> T) -> T {
        act(&mut self.fed.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn feed(&self, feed: Feed) {
        self.with(|fed| {
            fed.queue.push_back(feed);
            if let Some(waker) = fed.waker.take() {
                waker.wake();
            }
        });
    }

    fn delta(&self, text: &str) {
        self.feed(Feed::Event(StreamEvent::TextDelta {
            text: text.to_owned(),
        }));
    }

    fn end(&self, calls: Vec<ToolCall>) {
        self.feed(Feed::End(calls));
    }

    fn asked(&self) -> usize {
        self.with(|fed| fed.asked.len())
    }
}

impl Model for Feeding {
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
            self.with(|fed| fed.asked.push(request.clone()));
            let mut text = String::new();
            loop {
                let next = future::poll_fn(|cx| {
                    self.with(|fed| match fed.queue.pop_front() {
                        Some(feed) => Poll::Ready(feed),
                        None => {
                            fed.waker = Some(cx.waker().clone());
                            Poll::Pending
                        }
                    })
                })
                .await;
                match next {
                    Feed::Event(event) => {
                        if let StreamEvent::TextDelta { text: delta } = &event {
                            text.push_str(delta);
                        }
                        sink.emit(event).await?;
                    }
                    Feed::End(calls) => {
                        let stop_reason = if calls.is_empty() {
                            StopReason::EndTurn
                        } else {
                            StopReason::ToolCalls
                        };
                        let mut items = vec![Item::assistant(text)];
                        items.extend(calls.into_iter().map(Item::ToolCall));
                        return Ok(TurnOutcome {
                            stop_reason,
                            items,
                            observation: TurnObservation {
                                final_usage: true,
                                ..TurnObservation::new(self.provenance.clone())
                            },
                        });
                    }
                }
            }
        })
    }
}

fn handle(capabilities: Vec<Capability>, model: &Feeding) -> GenerationHandle {
    let (adapter, _) = Adapter::declare(DeclareAdapter {
        model_id: ModelId::generate(),
        backend: "a fed llm model".to_owned(),
        contract_revision: CONTRACT_REVISION.to_owned(),
        execution_form: ExecutionForm::InProcess,
        locality: Locality::SameProcess,
        capabilities,
        max_concurrency: 1,
    })
    .expect("one is positive");
    Generator::new(adapter, Arc::new(model.clone()))
        .start(GenerationRequest {
            run_id: RunId::generate(),
            node_id: NodeId::generate(),
            epoch: 1,
            turn: TurnRequest::new("fed", vec![Item::user("Look it up.")]),
        })
        .expect("a fresh generator runs nothing else")
}

fn call(id: &str) -> ToolCall {
    ToolCall {
        call_id: CallId::new(id).expect("a printable identifier"),
        name: ToolName::new("lookup").expect("a printable identifier"),
        arguments: json!({"city": id}),
    }
}

/// Every event `handle` yields now, without waiting: until it is pending or ended.
fn ready(handle: &mut GenerationHandle) -> Vec<GenerationEvent> {
    let mut cx = Context::from_waker(Waker::noop());
    let mut events = Vec::new();
    while let Poll::Ready(Some(event)) = handle.poll_event(&mut cx) {
        events.push(event);
    }
    events
}

/// The events `handle` yields up to and including its first `TextSegmentCommitted`.
fn until_committed(handle: &mut GenerationHandle) -> Vec<GenerationEvent> {
    let mut events = Vec::new();
    for event in handle.by_ref() {
        let committed = matches!(event, GenerationEvent::SegmentCommitted(_));
        events.push(event);
        if committed {
            break;
        }
    }
    events
}

/// Asks `handle` for its events as a caller of the crate documentation does, trying
/// `take_proposal` after each one, and returns the first proposal with the generation's state
/// when it was taken, and the events seen until then.
fn first_proposal(
    handle: &mut GenerationHandle,
) -> (Option<(ToolCall, GenerationState)>, Vec<GenerationEvent>) {
    let mut seen = Vec::new();
    loop {
        if let Some(proposal) = handle.take_proposal() {
            let state = handle.output().generation().state();
            return (Some((proposal, state)), seen);
        }
        let Some(event) = handle.next() else {
            return (None, seen);
        };
        seen.push(event);
    }
}

fn texts(events: &[GenerationEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            GenerationEvent::TextDelta(delta) => Some(delta.text.clone()),
            _ => None,
        })
        .collect()
}

/// The crate documentation: "A turn ending with tool calls stages them as proposals [...] The
/// caller takes each with `take_proposal` once the turn's text is delivered and answers it with
/// `integrate`; the next turn runs once every call of the round is answered." The capability
/// matrix keeps `tool_calling` apart from `output_streaming`, so an adapter may claim the first
/// alone. Its generation completes in the same poll that ends the turn: the proposal can be taken
/// only once the generation is Completed, and its answer is refused.
#[test]
fn a_tool_calling_adapter_that_does_not_stream_can_answer_its_round() {
    let model = Feeding::new();
    model.delta("Looking");
    model.end(vec![call("a")]);
    model.delta("Sunny");
    model.end(Vec::new());
    let mut handle = handle(vec![Capability::ToolCalling], &model);
    assert_round_answerable(&mut handle);
}

/// The same defect on a streaming adapter, with the commonest shape of a tool turn: the model
/// streams no prose and ends the turn with a call. Nothing is delivered between the turn's end
/// and the generation's completion, so the proposal appears only once the generation is
/// Completed.
#[test]
fn a_turn_ending_in_a_call_and_no_text_leaves_its_round_answerable() {
    let model = Feeding::new();
    model.end(vec![call("a")]);
    model.delta("Sunny");
    model.end(Vec::new());
    let mut handle = handle(
        vec![Capability::OutputStreaming, Capability::ToolCalling],
        &model,
    );
    assert_round_answerable(&mut handle);
}

/// And with prose, when the turn's end arrives in a later poll than its last delta, as it does
/// for a model reading a network stream (the last delta, then the stop and usage events). The
/// caller sees the committed segment while the turn still runs, so `take_proposal` is None; asked
/// for the next event, the handle ends the turn and completes the generation in the same poll.
#[test]
fn a_turn_whose_end_arrives_after_its_last_delta_leaves_its_round_answerable() {
    let model = Feeding::new();
    let mut handle = handle(
        vec![Capability::OutputStreaming, Capability::ToolCalling],
        &model,
    );
    model.delta("Looking");
    let first = ready(&mut handle);
    assert_eq!(texts(&first), ["Looking"]);
    assert_eq!(handle.take_proposal(), None, "the turn still runs");
    model.end(vec![call("a")]);
    model.delta("Sunny");
    model.end(Vec::new());
    assert_round_answerable(&mut handle);
}

fn assert_round_answerable(handle: &mut GenerationHandle) {
    let (taken, seen) = first_proposal(handle);
    let (proposal, state) = taken.expect("the turn's call is staged as a proposal");
    assert_eq!(
        state,
        GenerationState::Active,
        "the proposal was first available from a generation already ended: {seen:?}"
    );
    assert_eq!(
        handle.integrate(&proposal.call_id, json!({"temp": 20}), false),
        Ok(()),
        "the answer to the proposal is refused"
    );
}

/// "Iteration ends [...] once the generation is superseded": a delta the running turn streams
/// after the supersede is not delivered, and the turn is not asked again.
#[test]
fn a_delta_fed_after_a_supersede_is_not_delivered() {
    let model = Feeding::new();
    let mut handle = handle(vec![Capability::OutputStreaming], &model);
    model.delta("first");
    let before = ready(&mut handle);
    assert_eq!(texts(&before), ["first"]);
    model.delta("late");
    handle.supersede().expect("Active supersedes");
    model.end(Vec::new());
    let after = ready(&mut handle);
    assert_eq!(after, Vec::new(), "nothing after the supersede");
    assert_eq!(
        handle.output().generation().state(),
        GenerationState::Superseded
    );
    assert_eq!(model.asked(), 1);
}

/// As for a supersede, for a cancel: a delta already accepted by the sink but not yet taken is
/// dropped with the turn.
#[test]
fn a_delta_accepted_before_a_cancel_and_not_yet_taken_is_not_delivered() {
    let model = Feeding::new();
    let mut handle = handle(vec![Capability::OutputStreaming], &model);
    assert!(matches!(
        ready(&mut handle).as_slice(),
        [GenerationEvent::Started(_)]
    ));
    model.delta("accepted, untaken");
    handle.cancel().expect("Active cancels");
    assert_eq!(ready(&mut handle), Vec::new());
    assert!(
        handle.output().segments().is_empty(),
        "no segment was emitted for the untaken delta"
    );
}

/// Call announcements, argument fragments and reasoning are not prose: no `TextDelta`, and no
/// proposal is available while the turn that announces the call still runs.
#[test]
fn call_fragments_are_neither_text_nor_proposals_before_the_turn_ends() {
    let model = Feeding::new();
    let mut handle = handle(
        vec![Capability::OutputStreaming, Capability::ToolCalling],
        &model,
    );
    let lookup = call("a");
    model.feed(Feed::Event(StreamEvent::ToolCallStarted {
        call_id: lookup.call_id.clone(),
        name: lookup.name.clone(),
    }));
    model.feed(Feed::Event(StreamEvent::ToolArgumentsDelta {
        call_id: lookup.call_id.clone(),
        delta: "{\"city\":".to_owned(),
    }));
    model.feed(Feed::Event(StreamEvent::ReasoningDelta {
        text: "thinking".to_owned(),
    }));
    model.delta("Checking.");
    let running = ready(&mut handle);
    assert_eq!(texts(&running), ["Checking."]);
    assert_eq!(handle.take_proposal(), None, "the turn is still running");
    model.feed(Feed::Event(StreamEvent::ToolArgumentsDelta {
        call_id: lookup.call_id.clone(),
        delta: "\"a\"}".to_owned(),
    }));
    model.end(vec![lookup.clone()]);
    let mut cx = Context::from_waker(Waker::noop());
    let ended = handle.poll_event(&mut cx);
    assert!(
        !matches!(ended, Poll::Ready(Some(GenerationEvent::TextDelta(_)))),
        "{ended:?}"
    );
    assert_eq!(handle.take_proposal(), Some(lookup));
}

/// "empty deltas make no segment" (the unit's own deviation list); no case of the unit pins it.
#[test]
fn an_empty_delta_is_no_segment() {
    let model = Feeding::new();
    model.delta("");
    model.delta("x");
    model.delta("");
    model.end(Vec::new());
    let mut handle = handle(vec![Capability::OutputStreaming], &model);
    let events: Vec<GenerationEvent> = handle.by_ref().collect();
    assert_eq!(texts(&events), ["x"]);
    assert_eq!(handle.output().segments().len(), 1);
}

/// "the next turn runs once every call of the round is answered": the unit's case answers both
/// calls before it polls again, so a handle starting the next turn after the first answer would
/// pass it. Polled with one call of two answered, no turn is asked.
#[test]
fn a_round_answered_in_part_asks_no_turn() {
    let model = Feeding::new();
    model.delta("Looking");
    model.end(vec![call("a"), call("b")]);
    let mut handle = handle(
        vec![Capability::OutputStreaming, Capability::ToolCalling],
        &model,
    );
    let _ = until_committed(&mut handle);
    assert_eq!(handle.take_proposal(), Some(call("a")));
    assert_eq!(handle.take_proposal(), Some(call("b")));
    assert_eq!(
        handle.integrate(&call("b").call_id, json!(2), false),
        Ok(()),
        "the second call answered first"
    );
    let _ = ready(&mut handle);
    assert_eq!(model.asked(), 1, "a turn ran with call a unanswered");
}

/// Two calls answered out of order give a request `llm` accepts, results in answer order.
#[test]
fn a_round_answered_out_of_order_gives_a_valid_request() {
    let model = Feeding::new();
    model.delta("Looking");
    model.end(vec![call("a"), call("b")]);
    let mut handle = handle(
        vec![Capability::OutputStreaming, Capability::ToolCalling],
        &model,
    );
    let _ = until_committed(&mut handle);
    while handle.take_proposal().is_some() {}
    assert_eq!(handle.integrate(&call("b").call_id, json!(2), true), Ok(()));
    assert_eq!(
        handle.integrate(&call("a").call_id, json!(1), false),
        Ok(())
    );
    assert!(matches!(
        handle.integrate(&call("a").call_id, json!(1), false),
        Err(OutcomeRefused::NotAwaited(_))
    ));
    let _ = ready(&mut handle);
    let asked = model.with(|fed| fed.asked.clone());
    assert_eq!(asked.len(), 2);
    assert_eq!(asked[1].validate(), Ok(()));
    assert_eq!(
        asked[1].items[asked[1].items.len() - 2..],
        [
            Item::ToolResult {
                call_id: call("b").call_id,
                output: json!(2),
                failed: true,
            },
            Item::ToolResult {
                call_id: call("a").call_id,
                output: json!(1),
                failed: false,
            },
        ]
    );
}

/// `tool_id`: "The tool a call named `name` proposes to call, when `name` is one `tool_name`
/// gives; none otherwise". `tool_name` gives the lower-case hyphenated form only.
#[test]
fn tool_id_resolves_only_names_tool_name_gives() {
    let id = ToolId::generate();
    assert_eq!(tool_id(&tool_name(id)), Some(id));
    let upper = ToolName::new(id.to_string().to_uppercase()).expect("printable ASCII");
    assert_ne!(upper, tool_name(id), "a name tool_name does not give");
    assert_eq!(tool_id(&upper), None, "a name no tool was offered under");
}
