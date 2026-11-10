//! `AgentNode` reads the turn events and the committed spans from one edge, in the order the edge
//! carries them: two turns queued at once are each answered by a generation of their own, over
//! their own turn's text, however early the spans of the second turn are on the edge.
//!
//! The node is driven directly through its edges, as `agent_keeping_replies.rs` drives it.

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

fn mocks() -> (Mocks, MockGenerator) {
    let generator = MockGenerator::new(
        LlmConfiguration::StreamingOnly,
        ModelId::generate(),
        ["hello ", "there"],
    );
    let mocks = Mocks {
        recogniser: MockRecogniser::new(
            SttConfiguration::Incremental,
            ModelId::generate(),
            script(),
        ),
        generator: generator.clone(),
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
    (mocks, generator)
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
        self.put_as(run_id, event, EventId::generate(), None);
    }

    fn put_as(
        &self,
        run_id: RunId,
        event: StreamEvent,
        event_id: EventId,
        causation_id: Option<EventId>,
    ) {
        let envelope = EventEnvelope {
            schema_version: 1,
            event_id,
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
            causation_id,
            payload: CascadePayload::Stream(event),
        };
        self.with(|queue| queue.envelopes.push_back(envelope));
    }

    fn close(&self) {
        self.with(|queue| queue.closed = true);
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

fn turn(input: &Edge, run_id: RunId, text: &str) -> TurnId {
    let turn_id = TurnId::generate();
    input.put(
        run_id,
        StreamEvent::TurnOpened(TurnOpened { turn_id, run_id }),
    );
    input.put(
        run_id,
        StreamEvent::TranscriptCommitted(TranscriptCommitted {
            span_id: SpanId::generate(),
            text: text.to_owned(),
        }),
    );
    input.put(
        run_id,
        StreamEvent::TurnCompleted(TurnCompleted { turn_id }),
    );
    turn_id
}

#[test]
fn two_turns_queued_at_once_are_each_answered_over_their_own_text() {
    let (mocks, generator) = mocks();
    let mut declaration = Declaration::new();
    let cascade = declare(&mut declaration, &mocks).expect("the cascade plans");
    let node = declaration
        .node(cascade.ids.llm)
        .expect("the cascade has its model node")
        .clone();
    let run_id = RunId::generate();
    let mut agent = AgentNode::new(run_id, mocks.generator, reply, RunLog::new());
    let handle = agent.handle();
    let (input, output) = (Edge::default(), Edge::default());
    let first = turn(&input, run_id, "first");
    let second = turn(&input, run_id, "second");
    let implementation: &mut dyn NodeImplementation<CascadePayload> = &mut agent;
    block_on(implementation.describe(&node));
    block_on(implementation.prepare()).expect("the node prepares");
    block_on(implementation.start(NodeEndpoints {
        inputs: vec![NodeInput {
            port_id: cascade.ids.usage.llm,
            endpoint: Box::new(input.clone()),
        }],
        outputs: vec![NodeOutput {
            port_id: cascade.ids.usage.stt,
            endpoint: Box::new(output.clone()),
        }],
    }))
    .expect("the node starts");

    assert!(
        eventually(|| handle.generations().len() == 2),
        "each turn is answered by a generation of its own: {:?}",
        handle.generations()
    );
    let turns: Vec<TurnId> = handle
        .generations()
        .iter()
        .map(|answered| answered.turn_id)
        .collect();
    assert_eq!(turns, [first, second]);
    input.close();
    block_on(implementation.drain()).expect("the node drains");
    block_on(implementation.stop());
    assert_eq!(
        generator.asked(),
        [vec![reply("first")], vec![reply("second")],],
        "each generation is asked its own turn's text"
    );
}

/// Two turns whose spans both reach the edge before either turn's completion, as they do when the
/// node reads late: each span names the `TurnOpened` of its turn, so each generation is asked its
/// own turn's text.
#[test]
fn spans_naming_their_turn_are_answered_at_that_turns_completion_only() {
    let (mocks, generator) = mocks();
    let mut declaration = Declaration::new();
    let cascade = declare(&mut declaration, &mocks).expect("the cascade plans");
    let node = declaration
        .node(cascade.ids.llm)
        .expect("the cascade has its model node")
        .clone();
    let run_id = RunId::generate();
    let mut agent = AgentNode::new(run_id, mocks.generator, reply, RunLog::new());
    let handle = agent.handle();
    let (input, output) = (Edge::default(), Edge::default());
    let (first, second) = (TurnId::generate(), TurnId::generate());
    let (first_opened, second_opened) = (EventId::generate(), EventId::generate());
    let span = |text: &str, cause| {
        input.put_as(
            run_id,
            StreamEvent::TranscriptCommitted(TranscriptCommitted {
                span_id: SpanId::generate(),
                text: text.to_owned(),
            }),
            EventId::generate(),
            Some(cause),
        );
    };
    input.put_as(
        run_id,
        StreamEvent::TurnOpened(TurnOpened {
            turn_id: first,
            run_id,
        }),
        first_opened,
        None,
    );
    span("first", first_opened);
    span("second", second_opened);
    input.put(
        run_id,
        StreamEvent::TurnCompleted(TurnCompleted { turn_id: first }),
    );
    input.put_as(
        run_id,
        StreamEvent::TurnOpened(TurnOpened {
            turn_id: second,
            run_id,
        }),
        second_opened,
        None,
    );
    input.put(
        run_id,
        StreamEvent::TurnCompleted(TurnCompleted { turn_id: second }),
    );
    let implementation: &mut dyn NodeImplementation<CascadePayload> = &mut agent;
    block_on(implementation.describe(&node));
    block_on(implementation.prepare()).expect("the node prepares");
    block_on(implementation.start(NodeEndpoints {
        inputs: vec![NodeInput {
            port_id: cascade.ids.usage.llm,
            endpoint: Box::new(input.clone()),
        }],
        outputs: vec![NodeOutput {
            port_id: cascade.ids.usage.stt,
            endpoint: Box::new(output.clone()),
        }],
    }))
    .expect("the node starts");

    assert!(
        eventually(|| handle.generations().len() == 2),
        "each turn is answered by a generation of its own: {:?}",
        handle.generations()
    );
    input.close();
    block_on(implementation.drain()).expect("the node drains");
    block_on(implementation.stop());
    assert_eq!(
        generator.asked(),
        [vec![reply("first")], vec![reply("second")]],
        "each generation is asked its own turn's text"
    );
}
