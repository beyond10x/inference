//! Adversary case on `story:cascade-turn-spans-in-a-burst`: the STT node names, as the
//! `causation_id` of each transcript envelope of a turn, the envelope of that turn's `TurnOpened`
//! it sent on `turns_out` (the node's module documentation). The agent node tells the turns of a
//! burst apart by that name alone once the gate read late, so the case asks it of two turns, read
//! off the node's own outputs.
//!
//! The node is driven directly through its edges.

#![cfg(all(feature = "stt", feature = "llm", feature = "tts", feature = "run"))]

use std::collections::VecDeque;
use std::future::{self, Future};
use std::num::{NonZeroU16, NonZeroU32};
use std::pin::pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use inference_core::endpoint::{EndpointFuture, InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{NodeId, RunId, StreamId};
use inference_core::model::ModelId;
use inference_pipeline::{Declaration, NodeEndpoints, NodeImplementation, NodeInput, NodeOutput};
use inference_testkit::cascade::{CascadePayload, Mocks, declare};
use inference_testkit::graph::RunLog;
use inference_testkit::llm::{MockConfiguration as LlmConfiguration, MockGenerator};
use inference_testkit::stream::StreamEvent;
use inference_testkit::stt::inference_stt::inference_audio::{AudioEncoding, AudioFormat};
use inference_testkit::stt::{
    Cue, MockConfiguration as SttConfiguration, MockRecogniser, ScriptStep, SttNode, frames,
};
use inference_testkit::tts::{MockConfiguration as TtsConfiguration, MockSynthesiser};
use inference_text::{TurnCompleted, TurnId, TurnOpened};

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

fn envelope(run_id: RunId, kind: &str, payload: CascadePayload) -> EventEnvelope<CascadePayload> {
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id,
        session_id: None,
        stream_id: StreamId::generate(),
        sequence: 1,
        producer: NodeId::generate(),
        port: "case".to_owned(),
        kind: kind.to_owned(),
        generation_id: None,
        timestamp: 0,
        timebase: Timebase::WallTime,
        causation_id: None,
        payload,
    }
}

#[derive(Default)]
struct Queue {
    envelopes: VecDeque<EventEnvelope<CascadePayload>>,
    waker: Option<Waker>,
    popped: usize,
    /// After this many envelopes were taken from this edge, these land on the edge named.
    feed: Option<(usize, Edge, Vec<EventEnvelope<CascadePayload>>)>,
}

#[derive(Clone, Default)]
struct Edge(Arc<Mutex<Queue>>);

impl Edge {
    fn push(&self, envelope: EventEnvelope<CascadePayload>) {
        let mut queue = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        queue.envelopes.push_back(envelope);
        if let Some(waker) = queue.waker.take() {
            waker.wake();
        }
    }

    fn taken(&self) -> Vec<EventEnvelope<CascadePayload>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .envelopes
            .iter()
            .cloned()
            .collect()
    }
}

impl OutputEndpoint<CascadePayload> for Edge {
    fn send(
        &self,
        envelope: EventEnvelope<CascadePayload>,
    ) -> EndpointFuture<'_, Result<(), SendError<CascadePayload>>> {
        self.push(envelope);
        Box::pin(future::ready(Ok(())))
    }
}

impl InputEndpoint<CascadePayload> for Edge {
    fn recv(&mut self) -> EndpointFuture<'_, Option<EventEnvelope<CascadePayload>>> {
        let queue = Arc::clone(&self.0);
        Box::pin(future::poll_fn(move |cx| {
            let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(envelope) = queue.envelopes.pop_front() else {
                queue.waker = Some(cx.waker().clone());
                return Poll::Pending;
            };
            queue.popped += 1;
            let popped = queue.popped;
            if queue
                .feed
                .as_ref()
                .is_some_and(|(after, _, _)| *after == popped)
                && let Some((_, edge, fed)) = queue.feed.take()
            {
                drop(queue);
                for envelope in fed {
                    edge.push(envelope);
                }
            }
            Poll::Ready(Some(envelope))
        }))
    }
}

fn mocks(recogniser: MockRecogniser) -> Mocks {
    Mocks {
        recogniser,
        generator: MockGenerator::new(
            LlmConfiguration::StreamingOnly,
            ModelId::generate(),
            ["hello"],
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
        reply: str::to_owned,
    }
}

#[test]
fn each_span_of_a_turn_names_the_turn_opened_the_node_sent_for_that_turn() {
    let recogniser = MockRecogniser::new(
        SttConfiguration::Incremental,
        ModelId::generate(),
        vec![
            Cue {
                after_frames: 1,
                step: ScriptStep::Hypothesis("words".to_owned()),
            },
            Cue {
                after_frames: 2,
                step: ScriptStep::Commit { span: 0 },
            },
        ],
    );
    let mocks = mocks(recogniser.clone());
    let mut declaration = Declaration::new();
    let cascade = declare(&mut declaration, &mocks).expect("the cascade plans");
    let node = declaration
        .node(cascade.ids.stt)
        .expect("the cascade has its stt node")
        .clone();
    let ports = cascade.ids.stt_ports;
    let run_id = RunId::generate();
    let mut stt = SttNode::new(run_id, recogniser, ports, RunLog::new());

    let audio = |frame| {
        envelope(
            run_id,
            "inference.audio.AudioChunk",
            CascadePayload::Audio(frame),
        )
    };
    let stop = || {
        envelope(
            run_id,
            ControlKind::Stop.as_str(),
            CascadePayload::Control(ControlKind::Stop),
        )
    };
    let turn = |event: StreamEvent| envelope(run_id, event.name(), CascadePayload::Stream(event));
    let (first, second) = (TurnId::generate(), TurnId::generate());
    let mut heard = frames(6).into_iter();
    let (audio_in, turns_in) = (Edge::default(), Edge::default());
    for frame in heard.by_ref().take(3) {
        audio_in.push(audio(frame));
    }
    for event in [
        StreamEvent::TurnOpened(TurnOpened {
            turn_id: first,
            run_id,
        }),
        StreamEvent::TurnCompleted(TurnCompleted { turn_id: first }),
        StreamEvent::TurnOpened(TurnOpened {
            turn_id: second,
            run_id,
        }),
        StreamEvent::TurnCompleted(TurnCompleted { turn_id: second }),
    ] {
        turns_in.push(turn(event));
    }
    turns_in.push(stop());
    {
        let mut second_audio: Vec<_> = heard.map(audio).collect();
        second_audio.push(stop());
        turns_in
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .feed = Some((3, audio_in.clone(), second_audio));
    }
    let (committed, turns_out, transcript) = (Edge::default(), Edge::default(), Edge::default());
    let mut outputs = vec![
        NodeOutput {
            port_id: ports.committed_text,
            endpoint: Box::new(committed.clone()) as Box<dyn OutputEndpoint<CascadePayload>>,
        },
        NodeOutput {
            port_id: ports.turns_out,
            endpoint: Box::new(turns_out.clone()),
        },
    ];
    if let Some(port_id) = ports.transcript {
        outputs.push(NodeOutput {
            port_id,
            endpoint: Box::new(transcript.clone()),
        });
    }
    let implementation: &mut dyn NodeImplementation<CascadePayload> = &mut stt;
    block_on(implementation.describe(&node));
    block_on(implementation.prepare()).expect("the node prepares");
    block_on(implementation.start(NodeEndpoints {
        inputs: vec![
            NodeInput {
                port_id: ports.audio,
                endpoint: Box::new(audio_in),
            },
            NodeInput {
                port_id: ports.turns,
                endpoint: Box::new(turns_in),
            },
        ],
        outputs,
    }))
    .expect("the node starts");
    block_on(implementation.drain()).expect("the node drains once its inputs ended");
    block_on(implementation.stop());

    let opened: Vec<EventId> = turns_out
        .taken()
        .iter()
        .filter(|envelope| {
            matches!(
                envelope.payload,
                CascadePayload::Stream(StreamEvent::TurnOpened(_))
            )
        })
        .map(|envelope| envelope.event_id)
        .collect();
    assert_eq!(opened.len(), 2, "two turns opened on turns_out");
    let named = |edge: &Edge| -> Vec<Option<EventId>> {
        edge.taken()
            .iter()
            .filter(|envelope| matches!(envelope.payload, CascadePayload::Stream(_)))
            .map(|envelope| envelope.causation_id)
            .collect()
    };
    assert_eq!(
        named(&committed),
        [Some(opened[0]), Some(opened[1])],
        "each committed span names its own turn's TurnOpened"
    );
    let transcript = named(&transcript);
    assert!(
        transcript.len() >= 4
            && transcript[..transcript.len() / 2]
                .iter()
                .all(|named| *named == Some(opened[0]))
            && transcript[transcript.len() / 2..]
                .iter()
                .all(|named| *named == Some(opened[1])),
        "each transcript event names its own turn's TurnOpened: {transcript:?} of {opened:?}"
    );
}
