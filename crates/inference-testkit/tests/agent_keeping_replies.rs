//! `AgentNode::keeping_replies`: at a turn's opening the node completes the generation of the
//! turn before, its reply unplayed, and never supersedes it; a node built without it supersedes
//! that generation, as the voice cascade's barge-in does.
//!
//! The node is driven directly through its edges, as `adversary1_llm_cut_node.rs` drives the
//! model node.

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
use inference_core::generation::GenerationState;
use inference_core::id::{NodeId, RunId, StreamId};
use inference_core::model::ModelId;
use inference_pipeline::nodes::agent::AgentNode;
use inference_pipeline::{Declaration, NodeEndpoints, NodeImplementation, NodeInput, NodeOutput};
use inference_testkit::cascade::{CascadePayload, Mocks, declare};
use inference_testkit::graph::RunLog;
use inference_testkit::llm::{MockConfiguration as LlmConfiguration, MockGenerator};
use inference_testkit::stream::StreamEvent;
use inference_testkit::stt::inference_stt::inference_audio::{AudioEncoding, AudioFormat};
use inference_testkit::stt::{MockConfiguration as SttConfiguration, MockRecogniser, script};
use inference_testkit::tts::{MockConfiguration as TtsConfiguration, MockSynthesiser};
use inference_text::{SpanId, TranscriptCommitted, TurnCompleted, TurnId, TurnOpened};

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
        thread::park_timeout(Duration::from_millis(10));
    }
}

fn eventually(ready: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if ready() {
            return true;
        }
        thread::sleep(Duration::from_millis(5));
    }
    ready()
}

fn reply(input: &str) -> String {
    format!("you said {input}")
}

fn mocks() -> Mocks {
    Mocks {
        recogniser: MockRecogniser::new(
            SttConfiguration::Incremental,
            ModelId::generate(),
            script(),
        ),
        generator: MockGenerator::new(
            LlmConfiguration::StreamingOnly,
            ModelId::generate(),
            ["hello ", "there"],
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
    }
}

#[derive(Default)]
struct Queue {
    envelopes: VecDeque<EventEnvelope<CascadePayload>>,
    closed: bool,
    waker: Option<Waker>,
}

/// An unbounded edge the case writes and reads.
#[derive(Clone, Default)]
struct Edge(Arc<Mutex<Queue>>);

impl Edge {
    fn with<T>(&self, change: impl FnOnce(&mut Queue) -> T) -> T {
        let mut queue = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let answer = change(&mut queue);
        if let Some(waker) = queue.waker.take() {
            waker.wake();
        }
        answer
    }

    fn put(&self, run_id: RunId, event: StreamEvent) {
        let envelope = EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id,
            session_id: None,
            stream_id: StreamId::generate(),
            sequence: 1,
            producer: NodeId::generate(),
            port: "case".to_owned(),
            kind: "inference.stream".to_owned(),
            generation_id: None,
            timestamp: 0,
            timebase: Timebase::WallTime,
            causation_id: None,
            payload: CascadePayload::Stream(event),
        };
        self.with(|queue| queue.envelopes.push_back(envelope));
    }

    fn close(&self) {
        self.with(|queue| queue.closed = true);
    }

    fn completed(&self) -> usize {
        self.with(|queue| {
            queue
                .envelopes
                .iter()
                .filter(|envelope| {
                    matches!(
                        envelope.payload,
                        CascadePayload::Stream(StreamEvent::GenerationCompleted(_))
                    )
                })
                .count()
        })
    }
}

impl OutputEndpoint<CascadePayload> for Edge {
    fn send(
        &self,
        envelope: EventEnvelope<CascadePayload>,
    ) -> EndpointFuture<'_, Result<(), SendError<CascadePayload>>> {
        self.with(|queue| queue.envelopes.push_back(envelope));
        Box::pin(future::ready(Ok(())))
    }
}

impl InputEndpoint<CascadePayload> for Edge {
    fn recv(&mut self) -> EndpointFuture<'_, Option<EventEnvelope<CascadePayload>>> {
        let queue = Arc::clone(&self.0);
        Box::pin(future::poll_fn(move |cx| {
            let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(envelope) = queue.envelopes.pop_front() {
                Poll::Ready(Some(envelope))
            } else if queue.closed {
                Poll::Ready(None)
            } else {
                queue.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }))
    }
}

/// One turn answered, then a second opened while the reply is unplayed: the generation's state
/// once the second opened, how many generations the node superseded and how many
/// `GenerationCompleted` it sent.
fn second_turn_opened(keeping: bool) -> (GenerationState, usize, usize) {
    let mocks = mocks();
    let mut declaration = Declaration::new();
    let cascade = declare(&mut declaration, &mocks).expect("the cascade plans");
    let node = declaration
        .node(cascade.ids.llm)
        .expect("the cascade has its model node")
        .clone();
    let run_id = RunId::generate();
    let agent = AgentNode::new(
        run_id,
        mocks.generator,
        reply,
        cascade.ids.llm_turns,
        RunLog::new(),
    );
    let mut agent = if keeping {
        agent.keeping_replies()
    } else {
        agent
    };
    let handle = agent.handle();
    let (input, turns, output) = (Edge::default(), Edge::default(), Edge::default());
    let implementation: &mut dyn NodeImplementation<CascadePayload> = &mut agent;
    block_on(implementation.describe(&node));
    block_on(implementation.prepare()).expect("the node prepares");
    block_on(implementation.start(NodeEndpoints {
        inputs: vec![
            NodeInput {
                port_id: cascade.ids.llm_turns,
                endpoint: Box::new(turns.clone()),
            },
            NodeInput {
                port_id: cascade.ids.usage.llm,
                endpoint: Box::new(input.clone()),
            },
        ],
        outputs: vec![NodeOutput {
            port_id: cascade.ids.usage.stt,
            endpoint: Box::new(output.clone()),
        }],
    }))
    .expect("the node starts");

    let first = TurnId::generate();
    turns.put(
        run_id,
        StreamEvent::TurnOpened(TurnOpened {
            turn_id: first,
            run_id,
        }),
    );
    input.put(
        run_id,
        StreamEvent::TranscriptCommitted(TranscriptCommitted {
            span_id: SpanId::generate(),
            text: "hi".to_owned(),
        }),
    );
    turns.put(
        run_id,
        StreamEvent::TurnCompleted(TurnCompleted { turn_id: first }),
    );
    assert!(
        eventually(|| handle
            .generations()
            .first()
            .is_some_and(|answered| !answered.segments.is_empty())),
        "the first turn is answered: {:?}",
        handle.generations()
    );
    turns.put(
        run_id,
        StreamEvent::TurnOpened(TurnOpened {
            turn_id: TurnId::generate(),
            run_id,
        }),
    );
    assert!(
        eventually(|| handle
            .generations()
            .first()
            .is_some_and(|answered| answered.state != GenerationState::Active)),
        "the second turn's opening ends the first generation: {:?}",
        handle.generations()
    );
    let state = handle.generations()[0].state;
    let superseded = handle.superseded().len();
    let completed = output.completed();
    input.close();
    turns.close();
    block_on(implementation.drain()).expect("the node drains");
    block_on(implementation.stop());
    (state, superseded, completed)
}

#[test]
fn a_node_keeping_replies_completes_the_generation_a_turn_opening_finds_unplayed() {
    let (state, superseded, completed) = second_turn_opened(true);
    assert_eq!(state, GenerationState::Completed);
    assert_eq!(superseded, 0, "nothing is superseded");
    assert_eq!(completed, 1, "its GenerationCompleted is sent");
}

#[test]
fn a_node_not_keeping_replies_supersedes_that_generation() {
    let (state, superseded, completed) = second_turn_opened(false);
    assert_eq!(state, GenerationState::Superseded);
    assert_eq!(superseded, 1);
    assert_eq!(completed, 0);
}
