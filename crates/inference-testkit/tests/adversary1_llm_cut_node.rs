//! Adversary pass 1 on `story:llm-usage-on-cancel`: the model node now ends its wait on a running
//! turn when a supersede is asked (`deliver_turn`). A node given a turns port answers the inputs
//! of a completed turn one by one (`deferred`); while the supersede it was asked is not yet
//! taken, every later input of the turn is offered to the generation and counted as a model
//! request, its turn never runs, and the supersede then drops it unrecorded.
//!
//! The node is driven directly through its edges, as `usage_on_cancel.rs` drives it.

#![cfg(all(feature = "stt", feature = "llm", feature = "tts", feature = "run"))]

use std::collections::VecDeque;
use std::future::{self, Future};
use std::num::{NonZeroU16, NonZeroU32};
use std::pin::pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use inference_core::endpoint::{EndpointFuture, InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::{EventEnvelope, EventId, Timebase};
use inference_core::id::{NodeId, RunId, StreamId};
use inference_core::model::ModelId;
use inference_llm::llm_core::Usage;
use inference_pipeline::{
    Declaration, NodeEndpoints, NodeImplementation, NodeInput, NodeOutput, PortId,
};
use inference_testkit::cascade::{CascadePayload, Mocks, VoiceCascade, declare};
use inference_testkit::graph::RunLog;
use inference_testkit::llm::{MockConfiguration as LlmConfiguration, MockGenerator, ModelNode};
use inference_testkit::stream::StreamEvent;
use inference_testkit::stt::inference_stt::inference_audio::{AudioEncoding, AudioFormat};
use inference_testkit::stt::{MockConfiguration as SttConfiguration, MockRecogniser, script};
use inference_testkit::tts::{MockConfiguration as TtsConfiguration, MockSynthesiser};
use inference_text::{SpanId, TranscriptCommitted, TurnCompleted, TurnId};

/// How long the case waits for what it expects of the node.
const PATIENCE: Duration = Duration::from_secs(10);

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

/// An input edge that delivers `envelopes`, then waits for ever.
struct Scripted(VecDeque<EventEnvelope<CascadePayload>>);

impl InputEndpoint<CascadePayload> for Scripted {
    fn recv(&mut self) -> EndpointFuture<'_, Option<EventEnvelope<CascadePayload>>> {
        match self.0.pop_front() {
            Some(envelope) => Box::pin(future::ready(Some(envelope))),
            None => Box::pin(future::pending()),
        }
    }
}

type Taken = Arc<Mutex<Vec<EventEnvelope<CascadePayload>>>>;

/// An output edge that takes every envelope.
struct Edge(Taken);

impl OutputEndpoint<CascadePayload> for Edge {
    fn send(
        &self,
        envelope: EventEnvelope<CascadePayload>,
    ) -> EndpointFuture<'_, Result<(), SendError<CascadePayload>>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(envelope);
        Box::pin(future::ready(Ok(())))
    }
}

fn envelope(run_id: RunId, sequence: i64, event: StreamEvent) -> EventEnvelope<CascadePayload> {
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
        timebase: Timebase::WallTime,
        causation_id: None,
        payload: CascadePayload::Stream(event),
    }
}

fn committed(run_id: RunId, sequence: i64, text: &str) -> EventEnvelope<CascadePayload> {
    envelope(
        run_id,
        sequence,
        StreamEvent::TranscriptCommitted(TranscriptCommitted {
            span_id: SpanId::generate(),
            text: text.to_owned(),
        }),
    )
}

/// Waits until `taken` holds an envelope `wanted` matches.
fn wait_for(taken: &Taken, wanted: fn(&EventEnvelope<CascadePayload>) -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < PATIENCE {
        if taken
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(wanted)
        {
            return true;
        }
        thread::sleep(Duration::from_millis(5));
    }
    false
}

/// RED. The turn of a node given a turns port completes with two inputs; the first one's turn
/// streams and is held, and the host supersedes during it. The second input never runs a turn
/// (the generation is superseded), yet `ModelHandle::model_requests` counts it ("each input
/// offered [...] its turn run") and `ModelHandle::dropped_inputs` does not record it. Before
/// this unit the node waited for the first turn to end and ran the second before it took the
/// supersede.
#[test]
fn an_input_offered_while_a_supersede_waits_is_neither_run_nor_recorded_dropped() {
    let (declaration, cascade) = cascade();
    let ids = cascade.ids;
    let run_id = RunId::generate();
    let generator = MockGenerator::new(
        LlmConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        ["held ", "answer"],
    )
    .reporting([Some(Usage {
        input_tokens: Some(5),
        output_tokens: Some(1),
        ..Usage::default()
    })])
    .holding(0);
    let turns = PortId::generate();
    let mut node = ModelNode::<CascadePayload>::carrying(run_id, generator, reply, RunLog::new())
        .with_turns(turns)
        .with_usage(ids.usage.llm);
    let handle = node.handle();
    block_on(node.describe(declaration.node(ids.llm).expect("the LLM node")));
    let inputs = Scripted(VecDeque::from([
        committed(run_id, 1, "one"),
        committed(run_id, 2, "two"),
    ]));
    let completed = Scripted(VecDeque::from([envelope(
        run_id,
        1,
        StreamEvent::TurnCompleted(TurnCompleted {
            turn_id: TurnId::generate(),
        }),
    )]));
    let (text, usage) = (Taken::default(), Taken::default());
    let endpoints = NodeEndpoints {
        inputs: vec![
            NodeInput {
                port_id: PortId::generate(),
                endpoint: Box::new(inputs),
            },
            NodeInput {
                port_id: turns,
                endpoint: Box::new(completed),
            },
        ],
        outputs: vec![
            NodeOutput {
                port_id: PortId::generate(),
                endpoint: Box::new(Edge(Arc::clone(&text))),
            },
            NodeOutput {
                port_id: ids.usage.llm,
                endpoint: Box::new(Edge(Arc::clone(&usage))),
            },
        ],
    };
    block_on(node.start(endpoints)).expect("the node starts on its edges");
    assert!(
        wait_for(&text, |envelope| matches!(
            envelope.payload,
            CascadePayload::Stream(StreamEvent::TextSegmentCommitted(_))
        )),
        "the first input's turn streams"
    );
    assert!(handle.supersede(), "the supersede is asked");
    assert!(
        wait_for(&usage, |envelope| matches!(
            envelope.payload,
            CascadePayload::Usage(_)
        )),
        "the cut turn's record is sent"
    );
    block_on(node.cancel());
    block_on(node.stop());

    let records = usage
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter(|envelope| matches!(envelope.payload, CascadePayload::Usage(_)))
        .count();
    assert_eq!(records, 1, "one turn ran, one record");
    let (requests, dropped) = (handle.model_requests(), handle.dropped_inputs());
    assert!(
        requests == 1 || dropped.iter().any(|input| input == "two"),
        "one turn ran, yet the node counts {requests} model requests and dropped {dropped:?}: \
         the second input vanished"
    );
}
