//! Adversary cases for `story:refused-send-before-cancel-keeps-partial-usage`: a model node whose
//! output send is refused as closed while its run is being cancelled ends cancelled, the turn the
//! cancel cut recorded not final.
//!
//! The node is driven directly through its edges, as `model_node_refused_send_on_cancel.rs`
//! drives it. An output edge here refuses the n-th envelope of one kind as closed, as an edge
//! whose receiving side is gone does, and refuses every later send too, as a closed edge stays
//! closed; given a gate, it holds that send pending until the case opens the gate, so that the
//! case orders the node's drain or cancel against the refusal without sleeping.

#![cfg(all(feature = "stt", feature = "llm", feature = "tts", feature = "run"))]

use std::collections::VecDeque;
use std::future::{self, Future};
use std::num::{NonZeroU16, NonZeroU32};
use std::pin::pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use inference_core::endpoint::{EndpointFuture, InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::{EventEnvelope, EventId, Timebase};
use inference_core::generation::GenerationState;
use inference_core::id::{NodeId, RunId, StreamId};
use inference_core::model::ModelId;
use inference_core::usage::{QuantityKind, UsageRecorded};
use inference_llm::llm_core::Usage;
use inference_pipeline::{
    Declaration, NodeEndpoints, NodeImplementation, NodeInput, NodeOutput, PortId,
};
use inference_testkit::cascade::{CascadePayload, Mocks, VoiceCascade, declare};
use inference_testkit::graph::RunLog;
use inference_testkit::llm::{
    MockConfiguration as LlmConfiguration, MockGenerator, ModelHandle, ModelNode,
};
use inference_testkit::stream::StreamEvent;
use inference_testkit::stt::inference_stt::inference_audio::{AudioEncoding, AudioFormat};
use inference_testkit::stt::{MockConfiguration as SttConfiguration, MockRecogniser, script};
use inference_testkit::tts::{MockConfiguration as TtsConfiguration, MockSynthesiser};
use inference_text::{SpanId, TranscriptCommitted};

const PATIENCE: Duration = Duration::from_secs(10);
const COMMITTED: &str = "inference.stream.TextSegmentCommitted";
const RECORDED: &str = "inference.usage.UsageRecorded";

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

fn reply(input: &str) -> String {
    format!("you said {input}")
}

fn cascade() -> (Declaration, VoiceCascade) {
    let mocks = Mocks {
        recogniser: MockRecogniser::new(
            SttConfiguration::Incremental,
            ModelId::generate(),
            script(),
        ),
        generator: MockGenerator::new(
            LlmConfiguration::StreamingAndConcurrentInput,
            ModelId::generate(),
            Vec::<String>::new(),
        ),
        synthesiser: MockSynthesiser::new(
            TtsConfiguration::IncrementalStreaming,
            ModelId::generate(),
            AudioFormat {
                encoding: AudioEncoding::PcmS16le,
                sample_rate: NonZeroU32::new(16_000).expect("16000"),
                channels: NonZeroU16::new(1).expect("1"),
            },
        ),
        reply,
    };
    let mut declaration = Declaration::new();
    let cascade = declare(&mut declaration, &mocks).expect("the cascade plans");
    (declaration, cascade)
}

/// An input edge that delivers its envelopes, then waits for ever; `dropped` is told when the
/// node lets the edge go.
struct Scripted {
    envelopes: VecDeque<EventEnvelope<CascadePayload>>,
    dropped: Mutex<Sender<()>>,
}

impl InputEndpoint<CascadePayload> for Scripted {
    fn recv(&mut self) -> EndpointFuture<'_, Option<EventEnvelope<CascadePayload>>> {
        match self.envelopes.pop_front() {
            Some(envelope) => Box::pin(future::ready(Some(envelope))),
            None => Box::pin(future::pending()),
        }
    }
}

impl Drop for Scripted {
    fn drop(&mut self) {
        let _ = self
            .dropped
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(());
    }
}

type Taken = Arc<Mutex<Vec<EventEnvelope<CascadePayload>>>>;

/// A gate the case opens; a send held at it is pending until then.
#[derive(Default)]
struct Gate {
    open: AtomicBool,
    waker: Mutex<Option<Waker>>,
}

impl Gate {
    fn open(&self) {
        self.open.store(true, Ordering::SeqCst);
        if let Some(waker) = self
            .waker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            waker.wake();
        }
    }
}

/// How an output edge refuses: the `nth` envelope of `kind`, counted from 1, and every send after
/// it; `reached` is told when that send is made; given a gate, the send is held until it opens.
struct Refusal {
    kind: &'static str,
    nth: usize,
    seen: AtomicUsize,
    closed: AtomicBool,
    reached: Mutex<Sender<()>>,
    gate: Option<Arc<Gate>>,
}

struct Edge {
    taken: Taken,
    refusal: Option<Refusal>,
}

impl Edge {
    fn taking() -> (Self, Taken) {
        let taken = Taken::default();
        (
            Self {
                taken: Arc::clone(&taken),
                refusal: None,
            },
            taken,
        )
    }

    fn refusing(
        kind: &'static str,
        nth: usize,
        reached: Sender<()>,
        gate: Option<Arc<Gate>>,
    ) -> (Self, Taken) {
        let taken = Taken::default();
        (
            Self {
                taken: Arc::clone(&taken),
                refusal: Some(Refusal {
                    kind,
                    nth,
                    seen: AtomicUsize::new(0),
                    closed: AtomicBool::new(false),
                    reached: Mutex::new(reached),
                    gate,
                }),
            },
            taken,
        )
    }
}

impl OutputEndpoint<CascadePayload> for Edge {
    fn send(
        &self,
        envelope: EventEnvelope<CascadePayload>,
    ) -> EndpointFuture<'_, Result<(), SendError<CascadePayload>>> {
        Box::pin(async move {
            if let Some(refusal) = &self.refusal {
                if refusal.closed.load(Ordering::SeqCst) {
                    return Err(SendError::Closed(envelope));
                }
                if envelope.kind == refusal.kind
                    && refusal.seen.fetch_add(1, Ordering::SeqCst) + 1 == refusal.nth
                {
                    let _ = refusal
                        .reached
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .send(());
                    if let Some(gate) = &refusal.gate {
                        future::poll_fn(|cx| {
                            *gate.waker.lock().unwrap_or_else(PoisonError::into_inner) =
                                Some(cx.waker().clone());
                            if gate.open.load(Ordering::SeqCst) {
                                Poll::Ready(())
                            } else {
                                Poll::Pending
                            }
                        })
                        .await;
                    }
                    refusal.closed.store(true, Ordering::SeqCst);
                    return Err(SendError::Closed(envelope));
                }
            }
            self.taken
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(envelope);
            Ok(())
        })
    }
}

fn question(run_id: RunId, text: &str, sequence: i64) -> EventEnvelope<CascadePayload> {
    let event = StreamEvent::TranscriptCommitted(TranscriptCommitted {
        span_id: SpanId::generate(),
        text: text.to_owned(),
    });
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id,
        session_id: None,
        stream_id: StreamId::generate(),
        sequence,
        producer: NodeId::generate(),
        port: "in".to_owned(),
        kind: event.name().to_owned(),
        generation_id: None,
        timestamp: 0,
        timebase: Timebase::AudioFrameTime,
        causation_id: None,
        payload: CascadePayload::Stream(event),
    }
}

fn usage(input: u64, output: u64) -> Option<Usage> {
    Some(Usage {
        input_tokens: Some(input),
        output_tokens: Some(output),
        ..Usage::default()
    })
}

struct Started {
    node: ModelNode<CascadePayload>,
    handle: ModelHandle,
    reached: Receiver<()>,
    inputs_dropped: Receiver<()>,
    taken: Taken,
}

/// Which output refuses.
enum Refusing {
    Text,
    Usage,
}

/// The cascade's LLM node over a generator answering `questions` with the usage `reported`,
/// holding turn `held` if any; its `refusing` output refuses the `nth` envelope of `kind`.
fn started(
    questions: &[&str],
    reported: Vec<Option<Usage>>,
    held: Option<usize>,
    refusing: &Refusing,
    kind: &'static str,
    nth: usize,
    gate: Option<Arc<Gate>>,
) -> Started {
    let (declaration, cascade) = cascade();
    let ids = cascade.ids;
    let run_id = RunId::generate();
    let mut generator = MockGenerator::new(
        LlmConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        Vec::<String>::new(),
    )
    .reporting(reported);
    if let Some(turn) = held {
        generator = generator.holding(turn);
    }
    let mut node = ModelNode::<CascadePayload>::carrying(run_id, generator, reply, RunLog::new())
        .with_usage(ids.usage.llm);
    block_on(node.describe(declaration.node(ids.llm).expect("the LLM node")));
    let handle = node.handle();
    let (tell, reached) = mpsc::channel();
    let (dropped, inputs_dropped) = mpsc::channel();
    let ((text, _), (usage_edge, taken)) = match refusing {
        Refusing::Text => (Edge::refusing(kind, nth, tell, gate), Edge::taking()),
        Refusing::Usage => (Edge::taking(), Edge::refusing(kind, nth, tell, gate)),
    };
    let envelopes = questions
        .iter()
        .zip(1..)
        .map(|(text, sequence)| question(run_id, text, sequence))
        .collect();
    let endpoints = NodeEndpoints {
        inputs: vec![NodeInput {
            port_id: PortId::generate(),
            endpoint: Box::new(Scripted {
                envelopes,
                dropped: Mutex::new(dropped),
            }),
        }],
        outputs: vec![
            NodeOutput {
                port_id: PortId::generate(),
                endpoint: Box::new(text),
            },
            NodeOutput {
                port_id: ids.usage.llm,
                endpoint: Box::new(usage_edge),
            },
        ],
    };
    block_on(node.start(endpoints)).expect("the node starts on its edges");
    Started {
        node,
        handle,
        reached,
        inputs_dropped,
        taken,
    }
}

fn records(taken: &Taken) -> Vec<UsageRecorded> {
    taken
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter_map(|envelope| match &envelope.payload {
            CascadePayload::Usage(recorded) => Some(recorded.clone()),
            _ => None,
        })
        .collect()
}

fn input_tokens(recorded: &UsageRecorded) -> Option<i64> {
    recorded
        .quantities
        .iter()
        .find(|quantity| quantity.kind() == QuantityKind::InputTextTokens)
        .and_then(|quantity| quantity.value())
}

/// The run is draining when its cancel comes: the drain reached the node while its turn was
/// still being delivered, the consumer the node sends to is then ended by the run's cancel
/// before the cancel reaches the node, and the node's send is refused. The run is being
/// cancelled, so the node is to end cancelled with the cut turn's record, not final.
#[test]
fn a_refusal_between_a_drain_and_the_cancel_that_abandons_it_ends_the_node_cancelled() {
    let gate = Arc::new(Gate::default());
    let Started {
        mut node,
        handle,
        reached,
        taken,
        inputs_dropped,
        ..
    } = started(
        &["hello"],
        vec![usage(20, 1)],
        Some(0),
        &Refusing::Text,
        COMMITTED,
        1,
        Some(Arc::clone(&gate)),
    );
    reached
        .recv_timeout(PATIENCE)
        .expect("the node sends the held turn's segment commit");
    drop(node.drain());
    gate.open();
    inputs_dropped
        .recv_timeout(PATIENCE)
        .expect("the node took the refusal and let its input edge go");
    block_on(node.cancel());
    block_on(node.stop());
    let records = records(&taken);
    assert_eq!(
        (handle.failure(), handle.generation_state()),
        (None, Some(GenerationState::Cancelled)),
        "a refusal the run's cancel follows ends the node cancelled even when a drain came \
         first; records: {records:#?}"
    );
    let [recorded] = records.as_slice() else {
        panic!("the record of the turn the cancel cut: {records:#?}")
    };
    assert!(!recorded.r#final, "{recorded:?}");
    assert_eq!(input_tokens(recorded), Some(20), "{recorded:?}");
}

/// While it waits for the runtime's next signal after the refusal, the node has let its input
/// edges go, and has not ended.
#[test]
fn a_node_waiting_after_a_refusal_has_let_its_inputs_go_and_has_not_ended() {
    let Started {
        mut node,
        handle,
        reached,
        inputs_dropped,
        ..
    } = started(
        &["hello"],
        vec![usage(20, 1)],
        Some(0),
        &Refusing::Text,
        COMMITTED,
        1,
        None,
    );
    reached
        .recv_timeout(PATIENCE)
        .expect("the node sends the held turn's segment commit");
    inputs_dropped
        .recv_timeout(PATIENCE)
        .expect("the node lets its input edge go after the refusal, before any signal");
    assert_eq!(
        (handle.failure(), handle.generation_state()),
        (None, None),
        "the node waits for the runtime's next signal"
    );
    block_on(node.cancel());
    block_on(node.stop());
    assert_eq!(handle.failure(), None);
}

/// The cancel is taken while the refused send is still pending: the cancel wins, the node ends
/// cancelled with the cut turn's record.
#[test]
fn a_cancel_taken_while_the_send_to_be_refused_is_pending_ends_the_node_cancelled() {
    let gate = Arc::new(Gate::default());
    let Started {
        mut node,
        handle,
        reached,
        taken,
        ..
    } = started(
        &["hello"],
        vec![usage(20, 1)],
        Some(0),
        &Refusing::Text,
        COMMITTED,
        1,
        Some(Arc::clone(&gate)),
    );
    reached
        .recv_timeout(PATIENCE)
        .expect("the node sends the held turn's segment commit");
    block_on(node.cancel());
    gate.open();
    block_on(node.stop());
    let records = records(&taken);
    assert_eq!(
        (handle.failure(), handle.generation_state()),
        (None, Some(GenerationState::Cancelled)),
        "{records:#?}"
    );
    let [recorded] = records.as_slice() else {
        panic!("{records:#?}")
    };
    assert!(!recorded.r#final, "{recorded:?}");
}

/// The refusal cuts the second turn, after the first completed: the run's records are the first
/// turn's, final, and the second's, not final, each once.
#[test]
fn a_refusal_in_the_second_turn_records_the_first_final_and_the_cut_one_not_final_each_once() {
    let Started {
        mut node,
        handle,
        reached,
        taken,
        ..
    } = started(
        &["hello", "again"],
        vec![usage(20, 1), usage(30, 2)],
        Some(1),
        &Refusing::Text,
        COMMITTED,
        2,
        None,
    );
    reached
        .recv_timeout(PATIENCE)
        .expect("the node sends the second turn's segment commit");
    block_on(node.cancel());
    block_on(node.stop());
    let records = records(&taken);
    assert_eq!(
        (handle.failure(), handle.generation_state()),
        (None, Some(GenerationState::Cancelled)),
        "{records:#?}"
    );
    let finality: Vec<(bool, Option<i64>)> = records
        .iter()
        .map(|recorded| (recorded.r#final, input_tokens(recorded)))
        .collect();
    assert_eq!(
        finality,
        vec![(true, Some(20)), (false, Some(30))],
        "{records:#?}"
    );
}

/// The `usage` port itself refuses the first turn's record as closed and the cancel follows:
/// the node ends cancelled, not failed.
#[test]
fn a_refusal_on_the_usage_port_that_a_cancel_follows_ends_the_node_cancelled() {
    let Started {
        mut node,
        handle,
        reached,
        taken,
        ..
    } = started(
        &["hello"],
        vec![usage(20, 1)],
        None,
        &Refusing::Usage,
        RECORDED,
        1,
        None,
    );
    reached
        .recv_timeout(PATIENCE)
        .expect("the node sends the turn's usage record");
    block_on(node.cancel());
    block_on(node.stop());
    assert_eq!(
        (handle.failure(), handle.generation_state()),
        (None, Some(GenerationState::Cancelled)),
        "{:#?}",
        records(&taken)
    );
    assert!(records(&taken).is_empty(), "the usage edge is closed");
}

/// The `usage` port refuses the record as closed and a drain follows: the node fails naming it.
#[test]
fn a_refusal_on_the_usage_port_that_a_drain_follows_fails_the_node() {
    let Started {
        mut node,
        handle,
        reached,
        ..
    } = started(
        &["hello"],
        vec![usage(20, 1)],
        None,
        &Refusing::Usage,
        RECORDED,
        1,
        None,
    );
    reached
        .recv_timeout(PATIENCE)
        .expect("the node sends the turn's usage record");
    let error = block_on(node.drain()).expect_err("a refusal the drain follows fails the node");
    assert!(
        error
            .detail()
            .contains("the receiving side of the edge is gone"),
        "{error:?}"
    );
    block_on(node.stop());
    assert!(handle.failure().is_some());
}
