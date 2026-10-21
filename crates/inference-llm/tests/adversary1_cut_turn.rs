//! Adversary pass 1 on `story:llm-usage-on-cancel`: the turn a supersede, a cancel or a
//! completion cuts, as `GenerationHandle` polls it to its end.
//!
//! What a model may do once its turn is cancelled: stream more text and end all the same, end
//! with its counts final, fail with counts the provider marked final, end before the cut was
//! seen, or never return. Each case states what the documentation of the crate or
//! `ess/domains/usage.yaml` asks of it.

use std::collections::VecDeque;
use std::future::{self, Future};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};
use std::thread;
use std::time::{Duration, Instant};

use inference_core::generation::GenerationState;
use inference_core::id::{NodeId, RunId};
use inference_core::model::{
    Adapter, Capability, DeclareAdapter, ExecutionForm, Locality, ModelId,
};
use inference_core::usage::{Quantity, QuantityKind, UsageRecorded};
use inference_llm::llm_core::{
    BoxFuture, Cancel, Capabilities, Dispatch, Error, Id, Item, Model, Protocol, Provenance,
    StopReason, StreamEvent, StreamSink, TurnObservation, TurnOutcome, TurnRequest, Usage,
};
use inference_llm::{
    CONTRACT_REVISION, GenerationEvent, GenerationHandle, GenerationRequest, Generator,
};

/// How long a case waits for a blocking call before it says the call did not return.
const PATIENCE: Duration = Duration::from_secs(2);

/// What the model answers one turn with.
#[derive(Debug, Clone)]
enum Answer {
    /// This delta, then an outcome reporting `usage`, its counts final.
    Done(&'static str, Usage),
    /// This delta, then nothing until the turn is cancelled; then a late delta, and an outcome
    /// reporting `usage`, its counts final: a turn that ended all the same.
    LateText(&'static str, Usage),
    /// This delta, then nothing until the turn is cancelled; then `llm`'s cancelled error
    /// carrying `usage` with its counts marked final by the provider.
    FinalThenCancelled(&'static str, Usage),
    /// This delta, then nothing until the gate opens; then an outcome reporting `usage`, if
    /// any, its counts final. The cancel is not watched.
    Gated(&'static str, Option<Usage>),
    /// This delta, then nothing until the turn is cancelled, and then never returns.
    Stubborn(&'static str),
}

#[derive(Debug, Clone)]
struct Scripted {
    provenance: Provenance,
    capabilities: Capabilities,
    answers: Arc<Mutex<VecDeque<Answer>>>,
    gate: Arc<AtomicBool>,
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
            capabilities: Capabilities::text(1024, 256),
            answers: Arc::new(Mutex::new(answers.into_iter().collect())),
            gate: Arc::new(AtomicBool::new(false)),
        }
    }

    fn open_gate(&self) {
        self.gate.store(true, Ordering::Release);
    }

    fn outcome(&self, text: &str, usage: Option<Usage>) -> TurnOutcome {
        TurnOutcome {
            stop_reason: StopReason::EndTurn,
            items: vec![Item::assistant(text)],
            observation: TurnObservation {
                usage,
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
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<TurnOutcome, Error>> {
        Box::pin(async move {
            let answer = self
                .answers
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pop_front()
                .expect("the script answers every turn it is asked");
            let (Answer::Done(text, _)
            | Answer::LateText(text, _)
            | Answer::FinalThenCancelled(text, _)
            | Answer::Gated(text, _)
            | Answer::Stubborn(text)) = answer.clone();
            sink.emit(StreamEvent::TextDelta {
                text: text.to_owned(),
            })
            .await?;
            match answer {
                Answer::Done(text, usage) => Ok(self.outcome(text, Some(usage))),
                Answer::LateText(text, usage) => {
                    cancel.cancelled().await;
                    sink.emit(StreamEvent::TextDelta {
                        text: "late".to_owned(),
                    })
                    .await?;
                    Ok(self.outcome(&format!("{text}late"), Some(usage)))
                }
                Answer::FinalThenCancelled(_, usage) => {
                    cancel.cancelled().await;
                    Err(Error::cancelled()
                        .with_dispatch(Dispatch::Accepted)
                        .with_observation(TurnObservation {
                            usage: Some(usage),
                            final_usage: true,
                            ..TurnObservation::new(self.provenance.clone())
                        }))
                }
                Answer::Gated(text, usage) => {
                    let gate = Arc::clone(&self.gate);
                    future::poll_fn(|_| {
                        if gate.load(Ordering::Acquire) {
                            Poll::Ready(())
                        } else {
                            Poll::Pending
                        }
                    })
                    .await;
                    Ok(self.outcome(text, usage))
                }
                Answer::Stubborn(_) => {
                    cancel.cancelled().await;
                    future::pending::<()>().await;
                    unreachable!("a stubborn turn never returns")
                }
            }
        })
    }
}

fn generator(streams: bool, model: &Scripted) -> Generator {
    let mut capabilities = vec![Capability::ConcurrentInputOutput];
    if streams {
        capabilities.insert(0, Capability::OutputStreaming);
    }
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

fn usage(input: u64, output: u64) -> Usage {
    Usage {
        input_tokens: Some(input),
        output_tokens: Some(output),
        ..Usage::default()
    }
}

/// Every event of the turns that can run now, until a turn is pending or none is left; the
/// generation is never completed by it.
fn turn_events(handle: &mut GenerationHandle) -> Vec<GenerationEvent> {
    let mut cx = Context::from_waker(Waker::noop());
    let mut events = Vec::new();
    while let Poll::Ready(Some(event)) = handle.poll_turn_event(&mut cx) {
        events.push(event);
    }
    events
}

/// Starts a generation of `model`, runs its first turn to its end, offers one input and polls
/// the second turn until it is pending; the events seen so far.
fn second_turn_running(
    streams: bool,
    model: &Scripted,
) -> (GenerationHandle, Vec<GenerationEvent>) {
    let mut handle = generator(streams, model)
        .start(request())
        .expect("a fresh generator runs nothing else");
    let mut events = turn_events(&mut handle);
    handle
        .offer_input("again".to_owned())
        .expect("the adapter claims concurrent input");
    events.extend(turn_events(&mut handle));
    assert_eq!(
        handle.output().generation().state(),
        GenerationState::Active,
        "{events:?}"
    );
    (handle, events)
}

fn records(events: &[GenerationEvent]) -> Vec<&UsageRecorded> {
    events
        .iter()
        .filter_map(|event| match event {
            GenerationEvent::Usage(recorded) => Some(recorded),
            _ => None,
        })
        .collect()
}

fn kind(event: &GenerationEvent) -> &'static str {
    match event {
        GenerationEvent::Started(_) => "started",
        GenerationEvent::TextDelta(_) => "delta",
        GenerationEvent::SegmentCommitted(_) => "committed",
        GenerationEvent::Usage(_) => "usage",
        GenerationEvent::Completed(_) => "completed",
        GenerationEvent::Cancelled(..) => "cancelled",
    }
}

fn quantity(recorded: &UsageRecorded, kind: QuantityKind) -> Option<Quantity> {
    recorded
        .quantities
        .iter()
        .find(|quantity| quantity.kind() == kind)
        .copied()
}

/// `ess/domains/stream.yaml` refuses `EmitTextDelta` on a generation no longer Active: a turn that
/// streams more once it was cut delivers none of it, and its record, the turn having ended with
/// its counts final, is final. Supersede and cancel alike.
#[test]
fn a_cut_turn_that_streams_more_and_ends_delivers_no_text_and_a_final_record() {
    for supersede in [true, false] {
        let model = Scripted::new([
            Answer::Done("a", usage(10, 1)),
            Answer::LateText("b", usage(20, 2)),
        ]);
        let (mut handle, before) = second_turn_running(true, &model);
        assert_eq!(
            before.iter().map(kind).collect::<Vec<_>>(),
            [
                "started",
                "delta",
                "committed",
                "usage",
                "delta",
                "committed"
            ]
        );
        if supersede {
            handle.supersede().expect("Active supersedes");
        } else {
            handle.cancel().expect("Active cancels");
        }
        let after: Vec<GenerationEvent> = handle.by_ref().collect();
        let [GenerationEvent::Usage(recorded)] = after.as_slice() else {
            panic!("supersede {supersede}: one usage record and no text: {after:?}")
        };
        assert!(
            recorded.r#final,
            "supersede {supersede}: the turn ended with final counts: {recorded:?}"
        );
        assert_eq!(
            quantity(recorded, QuantityKind::OutputTextTokens),
            Some(Quantity::provider_reported(
                QuantityKind::OutputTextTokens,
                2
            ))
        );
        assert!(handle.next().is_none(), "nothing after the record");
        assert!(handle.supersede().is_err() && handle.cancel().is_err());
        assert!(handle.complete().is_err());
    }
}

/// A completion cuts the running turn: the turn's record, then `Completed`, exactly once and
/// last, and no text the turn streamed once cut.
#[test]
fn a_completion_delivers_the_cut_turns_record_then_completed_last() {
    let model = Scripted::new([
        Answer::Done("a", usage(10, 1)),
        Answer::LateText("b", usage(20, 2)),
    ]);
    let (mut handle, _) = second_turn_running(true, &model);
    handle.complete().expect("Active completes");
    let after: Vec<GenerationEvent> = handle.by_ref().collect();
    assert_eq!(
        after.iter().map(kind).collect::<Vec<_>>(),
        ["usage", "completed"],
        "{after:?}"
    );
    assert!(records(&after)[0].r#final);
    assert!(handle.next().is_none());
}

/// An adapter that does not stream holds the first turn's text and record until the
/// completion: they come first, then the cut turn's record, then `Completed`.
#[test]
fn a_completion_of_an_adapter_that_does_not_stream_keeps_completed_last() {
    let model = Scripted::new([
        Answer::Done("a", usage(10, 1)),
        Answer::LateText("b", usage(20, 2)),
    ]);
    let (mut handle, before) = second_turn_running(false, &model);
    assert_eq!(before.iter().map(kind).collect::<Vec<_>>(), ["started"]);
    handle.complete().expect("Active completes");
    let after: Vec<GenerationEvent> = handle.by_ref().collect();
    assert_eq!(
        after.iter().map(kind).collect::<Vec<_>>(),
        ["delta", "committed", "usage", "usage", "completed"],
        "{after:?}"
    );
    let delivered = records(&after);
    assert_ne!(
        delivered[0].usage_record_id, delivered[1].usage_record_id,
        "two records, not one twice"
    );
}

/// An adapter that does not stream, superseded during its second turn: the first turn's held
/// record survives ahead of the cut turn's, and no held text is delivered.
#[test]
fn a_supersede_of_an_adapter_that_does_not_stream_delivers_both_records_in_order() {
    let model = Scripted::new([
        Answer::Done("a", usage(10, 1)),
        Answer::LateText("b", usage(20, 2)),
    ]);
    let (mut handle, _) = second_turn_running(false, &model);
    handle.supersede().expect("Active supersedes");
    let after: Vec<GenerationEvent> = handle.by_ref().collect();
    let delivered = records(&after);
    assert_eq!(after.len(), delivered.len(), "only records: {after:?}");
    let [first, second] = delivered.as_slice() else {
        panic!("two records: {after:?}")
    };
    assert_eq!(
        (
            quantity(first, QuantityKind::InputTextTokens),
            quantity(second, QuantityKind::InputTextTokens)
        ),
        (
            Some(Quantity::provider_reported(
                QuantityKind::InputTextTokens,
                10
            )),
            Some(Quantity::provider_reported(
                QuantityKind::InputTextTokens,
                20
            ))
        )
    );
}

/// A turn that ended, final counts reported, before the supersede was seen, its outcome not yet
/// polled: its record is final.
#[test]
fn a_turn_that_ended_before_the_supersede_was_seen_delivers_a_final_record() {
    let model = Scripted::new([
        Answer::Done("a", usage(10, 1)),
        Answer::Gated("b", Some(usage(20, 2))),
    ]);
    let (mut handle, _) = second_turn_running(true, &model);
    model.open_gate();
    handle.supersede().expect("Active supersedes");
    let after: Vec<GenerationEvent> = handle.by_ref().collect();
    let [GenerationEvent::Usage(recorded)] = after.as_slice() else {
        panic!("one record and no text: {after:?}")
    };
    assert!(recorded.r#final, "the turn ended: {recorded:?}");
}

/// RED, pins the decided change (a cut turn without counts gets a record, requests 1). A turn
/// that ended before the supersede was seen and reported no usage is a model request all the
/// same: polled before the supersede, it yields a record (`turn_ended`); polled after, none
/// (`cut_turn_ended` filters on `usage.is_some()`).
#[test]
fn a_turn_that_ended_unreported_before_the_supersede_was_seen_still_counts_one_request() {
    let model = Scripted::new([Answer::Done("a", usage(10, 1)), Answer::Gated("b", None)]);
    let (mut handle, _) = second_turn_running(true, &model);
    model.open_gate();
    handle.supersede().expect("Active supersedes");
    let after: Vec<GenerationEvent> = handle.by_ref().collect();
    let delivered = records(&after);
    let [recorded] = delivered.as_slice() else {
        panic!("one record of the request the turn made: {after:?}")
    };
    assert_eq!(
        quantity(recorded, QuantityKind::Requests),
        Some(Quantity::measured(QuantityKind::Requests, 1))
    );
}

/// RED, contract drift. `ess/domains/usage.yaml` on `final`: "False when the provider marked its
/// counts partial, as a turn cut short may." A cut turn whose error carries counts the provider
/// marked final (`final_usage: true`, as when the usage chunk arrived before the cancel) is
/// recorded `final: false` (`turn_ended && observation.final_usage`).
#[test]
fn counts_the_provider_marked_final_stay_final_when_the_cut_turn_fails() {
    let model = Scripted::new([
        Answer::Done("a", usage(10, 1)),
        Answer::FinalThenCancelled("b", usage(20, 2)),
    ]);
    let (mut handle, _) = second_turn_running(true, &model);
    handle.supersede().expect("Active supersedes");
    let after: Vec<GenerationEvent> = handle.by_ref().collect();
    let [GenerationEvent::Usage(recorded)] = after.as_slice() else {
        panic!("one record: {after:?}")
    };
    assert!(
        recorded.r#final,
        "the provider marked these counts final: {recorded:?}"
    );
}

/// Measures what the crate documentation states ("polled until its model returns from the
/// cancel, which may take as long as the model takes"): a model that never returns from the
/// cancel leaves the superseded handle pending, and its blocking iterator does not return within
/// [`PATIENCE`]. Before this unit, `supersede` dropped the turn and the iterator ended at once.
#[test]
fn a_model_that_never_returns_from_the_cancel_holds_a_superseded_handle() {
    let model = Scripted::new([Answer::Done("a", usage(10, 1)), Answer::Stubborn("b")]);
    let (mut handle, _) = second_turn_running(true, &model);
    handle.supersede().expect("Active supersedes");
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..100 {
        assert!(handle.poll_event(&mut cx).is_pending());
    }
    let (sent, next) = mpsc::channel();
    thread::spawn(move || {
        let _ = sent.send(handle.next());
    });
    let waited = Instant::now();
    assert!(
        next.recv_timeout(PATIENCE).is_err(),
        "the iterator returned while the model holds its turn"
    );
    assert!(waited.elapsed() >= PATIENCE);
}

/// Measures the same for a completion: the generation is Completed at once, and
/// `GenerationEvent::Completed` is never delivered while the cut turn does not return.
#[test]
fn a_model_that_never_returns_from_the_cancel_withholds_completed() {
    let model = Scripted::new([Answer::Done("a", usage(10, 1)), Answer::Stubborn("b")]);
    let (mut handle, _) = second_turn_running(true, &model);
    handle.complete().expect("Active completes");
    assert_eq!(
        handle.output().generation().state(),
        GenerationState::Completed
    );
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..100 {
        assert!(
            handle.poll_event(&mut cx).is_pending(),
            "Completed is withheld"
        );
    }
    let mut next = Box::pin(handle.next_event());
    assert!(next.as_mut().poll(&mut cx).is_pending());
}
