//! A model node whose send is refused just before its cancel keeps the cut turn's usage
//! (`story:refused-send-before-cancel-keeps-partial-usage`).
//!
//! When a run is cancelled, the node a model node sends to can end before the model node is
//! cancelled itself: the edge between them then refuses the model node's next send as closed,
//! and the cancel arrives right after. The node is driven directly through its edges, as
//! `usage_on_cancel.rs` drives it: its text output refuses the `TextSegmentCommitted` of a turn
//! the model holds, as an edge whose receiving side is gone does, and tells the case, which only
//! then cancels the node. The node is recorded as cancelled, not failed, and its usage output
//! receives the record of the turn the cancel cut, not final. A refusal that no cancel follows,
//! the runtime draining the node instead, still fails it.

#![cfg(all(feature = "stt", feature = "llm", feature = "tts", feature = "run"))]

use std::collections::VecDeque;
use std::future::{self, Future};
use std::num::{NonZeroU16, NonZeroU32};
use std::pin::pin;
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

/// How long the case waits for the node's send to be refused.
const PATIENCE: Duration = Duration::from_secs(10);

/// The kind of the envelope the text output refuses.
const REFUSED: &str = "inference.stream.TextSegmentCommitted";

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

/// The cascade, declared only for its nodes and their ports.
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

/// An input edge that delivers `envelopes`, then waits for ever: its sender never goes.
struct Scripted(VecDeque<EventEnvelope<CascadePayload>>);

impl InputEndpoint<CascadePayload> for Scripted {
    fn recv(&mut self) -> EndpointFuture<'_, Option<EventEnvelope<CascadePayload>>> {
        match self.0.pop_front() {
            Some(envelope) => Box::pin(future::ready(Some(envelope))),
            None => Box::pin(future::pending()),
        }
    }
}

/// What an output edge took, shared with the case.
type Taken = Arc<Mutex<Vec<EventEnvelope<CascadePayload>>>>;

/// An output edge that takes every envelope once its send is polled, as a queue does, but, given
/// `refused`, every envelope of kind [`REFUSED`]: that send is refused as closed, as on an edge
/// whose receiving side is gone, and `refused` is told.
struct Edge {
    taken: Taken,
    refused: Option<Mutex<Sender<()>>>,
}

impl Edge {
    fn taking() -> (Self, Taken) {
        let taken = Taken::default();
        let edge = Self {
            taken: Arc::clone(&taken),
            refused: None,
        };
        (edge, taken)
    }

    fn refusing(refused: Sender<()>) -> Self {
        Self {
            taken: Taken::default(),
            refused: Some(Mutex::new(refused)),
        }
    }
}

impl OutputEndpoint<CascadePayload> for Edge {
    fn send(
        &self,
        envelope: EventEnvelope<CascadePayload>,
    ) -> EndpointFuture<'_, Result<(), SendError<CascadePayload>>> {
        Box::pin(async move {
            if let Some(refused) = &self.refused
                && envelope.kind == REFUSED
            {
                let _ = refused
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .send(());
                return Err(SendError::Closed(envelope));
            }
            self.taken
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(envelope);
            Ok(())
        })
    }
}

/// The input the node answers: one `TranscriptCommitted`.
fn question(run_id: RunId) -> EventEnvelope<CascadePayload> {
    let event = StreamEvent::TranscriptCommitted(TranscriptCommitted {
        span_id: SpanId::generate(),
        text: "hello".to_owned(),
    });
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id,
        session_id: None,
        stream_id: StreamId::generate(),
        sequence: 1,
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

/// The LLM node of the cascade over a model that holds its first turn, reporting 20 input tokens
/// and 1 output token for it, started on a text output that refuses [`REFUSED`] and a usage
/// output that takes everything. Answers the node, its handle, the receiver told of the refusal
/// and what the usage output took.
fn started() -> (
    ModelNode<CascadePayload>,
    ModelHandle,
    Receiver<()>,
    Taken,
    RunId,
) {
    let (declaration, cascade) = cascade();
    let ids = cascade.ids;
    let run_id = RunId::generate();
    let generator = MockGenerator::new(
        LlmConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        Vec::<String>::new(),
    )
    .reporting([Some(Usage {
        input_tokens: Some(20),
        output_tokens: Some(1),
        ..Usage::default()
    })])
    .holding(0);
    let mut node = ModelNode::<CascadePayload>::carrying(run_id, generator, reply, RunLog::new())
        .with_usage(ids.usage.llm);
    block_on(node.describe(declaration.node(ids.llm).expect("the LLM node")));
    let handle = node.handle();
    let (tell, refused) = mpsc::channel();
    let (usage, taken) = Edge::taking();
    let endpoints = NodeEndpoints {
        inputs: vec![NodeInput {
            port_id: PortId::generate(),
            endpoint: Box::new(Scripted(VecDeque::from([question(run_id)]))),
        }],
        outputs: vec![
            NodeOutput {
                port_id: PortId::generate(),
                endpoint: Box::new(Edge::refusing(tell)),
            },
            NodeOutput {
                port_id: ids.usage.llm,
                endpoint: Box::new(usage),
            },
        ],
    };
    block_on(node.start(endpoints)).expect("the node starts on its edges");
    (node, handle, refused, taken, run_id)
}

/// The usage records `taken` holds.
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

#[test]
fn a_send_refused_as_closed_just_before_the_cancel_ends_the_node_cancelled_with_the_cut_turns_usage()
 {
    let (mut node, handle, refused, taken, run_id) = started();
    refused
        .recv_timeout(PATIENCE)
        .expect("the text output refuses the turn's segment commit");
    block_on(node.cancel());
    block_on(node.stop());
    let records = records(&taken);
    assert_eq!(
        (handle.failure(), handle.generation_state()),
        (None, Some(GenerationState::Cancelled)),
        "a refusal the cancel follows ends the node as cancelled, not failed; records: \
         {records:#?}"
    );
    let [recorded] = records.as_slice() else {
        panic!("the record of the turn the cancel cut: {records:#?}")
    };
    assert!(!recorded.r#final, "the cut turn's record: {recorded:?}");
    assert_eq!(recorded.run_id, Some(run_id));
    assert!(
        recorded.quantities.iter().any(|quantity| {
            quantity.kind() == QuantityKind::InputTextTokens && quantity.value() == Some(20)
        }),
        "the counts the model reported for the cut turn: {recorded:?}"
    );
}

#[test]
fn a_send_refused_as_closed_that_no_cancel_follows_still_fails_the_node() {
    let (mut node, handle, refused, _taken, _) = started();
    refused
        .recv_timeout(PATIENCE)
        .expect("the text output refuses the turn's segment commit");
    let drained = block_on(node.drain());
    let error = drained.expect_err("a refusal the drain follows fails the node");
    assert!(
        error
            .detail()
            .contains("the receiving side of the edge is gone"),
        "the node's failure names the refusal: {error:?}"
    );
    assert!(
        handle
            .failure()
            .is_some_and(|failure| failure.contains("the receiving side of the edge is gone")),
        "{:?}",
        handle.failure()
    );
    block_on(node.stop());
}
