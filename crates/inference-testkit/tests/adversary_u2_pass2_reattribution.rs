//! Adversary pass 2 on `story:cascade-turn-spans-in-a-burst`: the agent's re-attribution of the
//! spans held when a turn opens over one that never completed.
//!
//! `AgentNode`'s module documentation: "When a `TurnOpened` arrives while the turn before it is
//! open and was never completed, the spans held that name the earlier `TurnOpened` are the new
//! turn's: they are re-attributed to it and answered by its generation, in arrival order, with the
//! spans that name it." A span that names a later `TurnOpened`, already on the edge in a burst,
//! belongs to that later turn and is answered at its completion only.
//!
//! The node is driven directly through its edges, as `adversary_u2_agent_reopened_turn.rs` drives
//! it.

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

/// What the case queues on the agent's input edge before it starts.
enum Put {
    Opened(TurnId, EventId),
    Span(&'static str, EventId),
    Completed(TurnId),
}

/// Runs an agent over `puts` until it answered `generations` turns and its input ended; the turn
/// each generation answered, what its model was asked per generation, and the inputs it dropped.
fn answer(puts: &[Put], generations: usize) -> (Vec<TurnId>, Vec<Vec<String>>, Vec<String>) {
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
    for put in puts {
        match *put {
            Put::Opened(turn_id, event_id) => input.put_as(
                run_id,
                StreamEvent::TurnOpened(TurnOpened { turn_id, run_id }),
                event_id,
                None,
            ),
            Put::Span(text, cause) => input.put_as(
                run_id,
                StreamEvent::TranscriptCommitted(TranscriptCommitted {
                    span_id: SpanId::generate(),
                    text: text.to_owned(),
                }),
                EventId::generate(),
                Some(cause),
            ),
            Put::Completed(turn_id) => input.put_as(
                run_id,
                StreamEvent::TurnCompleted(TurnCompleted { turn_id }),
                EventId::generate(),
                None,
            ),
        }
    }
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
    let _ = eventually(|| handle.generations().len() >= generations);
    input.close();
    block_on(implementation.drain()).expect("the node drains");
    block_on(implementation.stop());
    (
        handle
            .generations()
            .iter()
            .map(|answered| answered.turn_id)
            .collect(),
        generator.asked(),
        handle.dropped_inputs(),
    )
}

/// Three turns open, none of the first two completed: each opening re-attributes the spans held
/// for the turn before, so the third turn's generation answers all three spans in arrival order.
#[test]
fn spans_of_two_turns_never_completed_are_answered_by_the_third_turns_generation() {
    let (first, second, third) = (TurnId::generate(), TurnId::generate(), TurnId::generate());
    let (o1, o2, o3) = (
        EventId::generate(),
        EventId::generate(),
        EventId::generate(),
    );
    let (turns, asked, dropped) = answer(
        &[
            Put::Opened(first, o1),
            Put::Span("one", o1),
            Put::Opened(second, o2),
            Put::Span("two", o2),
            Put::Opened(third, o3),
            Put::Span("three", o3),
            Put::Completed(third),
        ],
        1,
    );

    assert_eq!(turns, [third], "one generation, the third turn's");
    assert_eq!(
        asked,
        [vec![reply("one"), reply("two"), reply("three")]],
        "every span re-attributed along the chain, in arrival order"
    );
    assert_eq!(dropped, Vec::<String>::new());
}

/// A turn opens over one that never completed while a span of a later turn, which names that
/// later turn's `TurnOpened`, is already held: only the spans naming the uncompleted turn's
/// opening move to the new turn, and the later span waits for its own turn's completion.
#[test]
fn a_span_of_a_later_turn_held_at_a_reopening_stays_with_its_own_turn() {
    let (first, second, third) = (TurnId::generate(), TurnId::generate(), TurnId::generate());
    let (o1, o2, o3) = (
        EventId::generate(),
        EventId::generate(),
        EventId::generate(),
    );
    let (turns, asked, dropped) = answer(
        &[
            Put::Opened(first, o1),
            Put::Span("one", o1),
            Put::Span("three", o3),
            Put::Opened(second, o2),
            Put::Span("two", o2),
            Put::Completed(second),
            Put::Opened(third, o3),
            Put::Completed(third),
        ],
        2,
    );

    assert_eq!(turns, [second, third], "one generation per completed turn");
    assert_eq!(
        asked,
        [vec![reply("one"), reply("two")], vec![reply("three")]],
        "the third turn's span is answered with the third turn, not re-attributed to the second"
    );
    assert_eq!(dropped, Vec::<String>::new());
}
