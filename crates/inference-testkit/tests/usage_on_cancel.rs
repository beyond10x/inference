//! A cancel never loses the usage record of a model call the node already holds
//! (`story:usage-session-totals`, finding F7).
//!
//! Each model node of the voice cascade, the LLM, the STT and the TTS node, is driven directly
//! through its edges: its content output stalls on one envelope, as an edge that is full and
//! that nobody reads does, after the node took the call's events and before it sent the usage
//! record that ends them. The node is then cancelled. Its usage output, which takes everything,
//! still receives the usage record of that call: the cancel ends the stalled send, the node sends
//! nothing more of the call's content, and it sends the usage record all the same.

#![cfg(all(feature = "stt", feature = "llm", feature = "tts", feature = "run"))]

use std::collections::VecDeque;
use std::future::{self, Future};
use std::num::{NonZeroU16, NonZeroU32};
use std::pin::pin;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use inference_core::endpoint::{EndpointFuture, InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::generation::GenerationStarted;
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::ModelId;
use inference_core::usage::UsageRecorded;
use inference_pipeline::{
    Declaration, NodeEndpoints, NodeImplementation, NodeInput, NodeOutput, PortId,
};
use inference_testkit::cascade::{CascadePayload, Mocks, VoiceCascade, declare};
use inference_testkit::graph::RunLog;
use inference_testkit::llm::{MockConfiguration as LlmConfiguration, MockGenerator, ModelNode};
use inference_testkit::stream::StreamEvent;
use inference_testkit::stt::inference_stt::inference_audio::{AudioEncoding, AudioFormat};
use inference_testkit::stt::{
    FRAME_SAMPLES, MockConfiguration as SttConfiguration, MockRecogniser, SttNode, SttPorts,
    frames, script,
};
use inference_testkit::tts::{MockConfiguration as TtsConfiguration, MockSynthesiser, TtsNode};
use inference_text::{
    SegmentId, SpanId, TextDelta, TextSegmentCommitted, TranscriptCommitted, TurnCompleted, TurnId,
    TurnOpened,
};

/// How long a case waits for its node to stall.
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

fn format() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).expect("16000"),
        channels: NonZeroU16::new(1).expect("1"),
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
            format(),
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

/// An output edge that takes every envelope once its send is polled, as a queue does, but the
/// first of kind `stall_on`: that send never completes, as on a full edge nobody reads, and
/// `stalled` is told.
struct Edge {
    taken: Taken,
    stall_on: Option<&'static str>,
    stalled: Mutex<Option<Sender<()>>>,
}

impl Edge {
    fn taking() -> (Self, Taken) {
        let taken = Taken::default();
        let edge = Self {
            taken: Arc::clone(&taken),
            stall_on: None,
            stalled: Mutex::new(None),
        };
        (edge, taken)
    }

    fn stalling_on(kind: &'static str, stalled: Sender<()>) -> Self {
        Self {
            taken: Taken::default(),
            stall_on: Some(kind),
            stalled: Mutex::new(Some(stalled)),
        }
    }
}

impl OutputEndpoint<CascadePayload> for Edge {
    fn send(
        &self,
        envelope: EventEnvelope<CascadePayload>,
    ) -> EndpointFuture<'_, Result<(), SendError<CascadePayload>>> {
        Box::pin(async move {
            if self.stall_on == Some(envelope.kind.as_str()) {
                let told = self
                    .stalled
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
                if let Some(stalled) = told {
                    let _ = stalled.send(());
                    return future::pending().await;
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

/// An envelope of one stream of the case, on `port`.
struct Stream {
    run_id: RunId,
    producer: NodeId,
    stream_id: StreamId,
    sequence: i64,
}

impl Stream {
    fn new(run_id: RunId) -> Self {
        Self {
            run_id,
            producer: NodeId::generate(),
            stream_id: StreamId::generate(),
            sequence: 0,
        }
    }

    fn envelope(
        &mut self,
        kind: &str,
        timebase: Timebase,
        timestamp: i64,
        payload: CascadePayload,
    ) -> EventEnvelope<CascadePayload> {
        self.sequence += 1;
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: self.run_id,
            session_id: None,
            stream_id: self.stream_id,
            sequence: self.sequence,
            producer: self.producer,
            port: "in".to_owned(),
            kind: kind.to_owned(),
            generation_id: None,
            timestamp,
            timebase,
            causation_id: None,
            payload,
        }
    }

    fn event(&mut self, event: StreamEvent, at: i64) -> EventEnvelope<CascadePayload> {
        let kind = event.name();
        self.envelope(
            kind,
            Timebase::AudioFrameTime,
            at,
            CascadePayload::Stream(event),
        )
    }

    fn stop(&mut self) -> EventEnvelope<CascadePayload> {
        self.envelope(
            ControlKind::Stop.as_str(),
            Timebase::WallTime,
            0,
            CascadePayload::Control(ControlKind::Stop),
        )
    }
}

/// Starts `node` on `endpoints`, waits until its content output stalls, cancels it and waits
/// until its exchange ended; answers the usage records its usage output took.
fn cancelled_while_stalled(
    node: &mut dyn NodeImplementation<CascadePayload>,
    endpoints: NodeEndpoints<CascadePayload>,
    stalled: &mpsc::Receiver<()>,
    usage: &Taken,
) -> Vec<UsageRecorded> {
    block_on(node.start(endpoints)).expect("the node starts on its edges");
    stalled
        .recv_timeout(PATIENCE)
        .expect("the node's content output stalls");
    block_on(node.cancel());
    block_on(node.stop());
    usage
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter_map(|envelope| match &envelope.payload {
            CascadePayload::Usage(recorded) => {
                assert_eq!(envelope.kind, UsageRecorded::KIND);
                Some(recorded.clone())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn a_cancel_during_an_llm_turn_still_yields_the_turns_usage_record() {
    let (declaration, cascade) = cascade();
    let ids = cascade.ids;
    let run_id = RunId::generate();
    let generator = MockGenerator::new(
        LlmConfiguration::StreamingAndConcurrentInput,
        ModelId::generate(),
        ["one ", "two ", "three"],
    );
    let mut node = ModelNode::<CascadePayload>::carrying(run_id, generator, reply, RunLog::new())
        .with_usage(ids.usage.llm);
    block_on(node.describe(declaration.node(ids.llm).expect("the LLM node")));
    let mut input = Stream::new(run_id);
    let question = input.event(
        StreamEvent::TranscriptCommitted(TranscriptCommitted {
            span_id: SpanId::generate(),
            text: "hello".to_owned(),
        }),
        0,
    );
    let (tell, stalled) = mpsc::channel();
    let (usage, taken) = Edge::taking();
    let endpoints = NodeEndpoints {
        inputs: vec![NodeInput {
            port_id: PortId::generate(),
            endpoint: Box::new(Scripted(VecDeque::from([question]))),
        }],
        outputs: vec![
            NodeOutput {
                port_id: PortId::generate(),
                endpoint: Box::new(Edge::stalling_on("inference.stream.TextDelta", tell)),
            },
            NodeOutput {
                port_id: ids.usage.llm,
                endpoint: Box::new(usage),
            },
        ],
    };
    let records = cancelled_while_stalled(&mut node, endpoints, &stalled, &taken);
    assert_eq!(
        records.len(),
        1,
        "the usage record of the turn the cancel cut: {records:#?}"
    );
    assert_eq!(records[0].run_id, Some(run_id));
    assert_eq!(records[0].node_id, Some(ids.llm));
}

#[test]
fn a_cancel_while_a_recognition_delivers_still_yields_its_usage_record() {
    let (declaration, cascade) = cascade();
    let ids = cascade.ids;
    let run_id = RunId::generate();
    let recogniser = MockRecogniser::new(SttConfiguration::Batch, ModelId::generate(), script());
    let ports = SttPorts {
        transcript: None,
        ..ids.stt_ports
    };
    let mut node = SttNode::new(run_id, recogniser, ports, RunLog::new()).with_usage(ids.usage.stt);
    block_on(NodeImplementation::<CascadePayload>::describe(
        &mut node,
        declaration.node(ids.stt).expect("the STT node"),
    ));
    let clip = frames(6);
    let end = i64::try_from(clip.len()).expect("few") * i64::from(FRAME_SAMPLES);
    let turn_id = TurnId::generate();
    let mut audio = Stream::new(run_id);
    let mut heard: VecDeque<_> = clip
        .into_iter()
        .map(|frame| {
            let at = i64::try_from(frame.position.get()).expect("a short clip");
            audio.envelope(
                "inference.audio.AudioFrame",
                Timebase::AudioFrameTime,
                at,
                CascadePayload::Audio(frame),
            )
        })
        .collect();
    heard.push_back(audio.stop());
    let mut turns = Stream::new(run_id);
    let turn = VecDeque::from([
        turns.event(StreamEvent::TurnOpened(TurnOpened { turn_id, run_id }), 0),
        turns.event(StreamEvent::TurnCompleted(TurnCompleted { turn_id }), end),
        turns.stop(),
    ]);
    let (tell, stalled) = mpsc::channel();
    let (turns_out, _) = Edge::taking();
    let (usage, taken) = Edge::taking();
    let endpoints = NodeEndpoints {
        inputs: vec![
            NodeInput {
                port_id: ports.audio,
                endpoint: Box::new(Scripted(heard)),
            },
            NodeInput {
                port_id: ports.turns,
                endpoint: Box::new(Scripted(turn)),
            },
        ],
        outputs: vec![
            NodeOutput {
                port_id: ports.committed_text,
                endpoint: Box::new(Edge::stalling_on(
                    "inference.stream.TranscriptCommitted",
                    tell,
                )),
            },
            NodeOutput {
                port_id: ports.turns_out,
                endpoint: Box::new(turns_out),
            },
            NodeOutput {
                port_id: ids.usage.stt,
                endpoint: Box::new(usage),
            },
        ],
    };
    let records = cancelled_while_stalled(&mut node, endpoints, &stalled, &taken);
    assert_eq!(
        records.len(),
        1,
        "the usage record of the recognition whose delivery the cancel cut: {records:#?}"
    );
    assert_eq!(records[0].run_id, Some(run_id));
    assert_eq!(records[0].node_id, Some(ids.stt));
}

#[test]
fn a_cancel_while_a_synthesis_delivers_still_yields_its_usage_record() {
    let (declaration, cascade) = cascade();
    let ids = cascade.ids;
    let run_id = RunId::generate();
    let synthesiser =
        MockSynthesiser::new(TtsConfiguration::Neither, ModelId::generate(), format());
    let mut node = TtsNode::new(run_id, synthesiser, RunLog::new()).with_usage(ids.usage.tts);
    block_on(NodeImplementation::<CascadePayload>::describe(
        &mut node,
        declaration.node(ids.tts).expect("the TTS node"),
    ));
    let generation_id = GenerationId::generate();
    let segment_id = SegmentId::generate();
    let words = "one two three four";
    let mut text = Stream::new(run_id);
    let said = VecDeque::from([
        text.event(
            StreamEvent::GenerationStarted(GenerationStarted {
                generation_id,
                run_id,
                epoch: 1,
            }),
            0,
        ),
        text.event(
            StreamEvent::TextDelta(TextDelta {
                segment_id,
                generation_id,
                sequence: 1,
                text: words.to_owned(),
            }),
            0,
        ),
        text.event(
            StreamEvent::TextSegmentCommitted(TextSegmentCommitted {
                segment_id,
                text: words.to_owned(),
            }),
            0,
        ),
        text.stop(),
    ]);
    let (tell, stalled) = mpsc::channel();
    let (usage, taken) = Edge::taking();
    let endpoints = NodeEndpoints {
        inputs: vec![NodeInput {
            port_id: PortId::generate(),
            endpoint: Box::new(Scripted(said)),
        }],
        outputs: vec![
            NodeOutput {
                port_id: PortId::generate(),
                endpoint: Box::new(Edge::stalling_on("inference.audio.AudioFrame", tell)),
            },
            NodeOutput {
                port_id: ids.usage.tts,
                endpoint: Box::new(usage),
            },
        ],
    };
    let records = cancelled_while_stalled(&mut node, endpoints, &stalled, &taken);
    assert_eq!(
        records.len(),
        1,
        "the usage record of the synthesis whose delivery the cancel cut: {records:#?}"
    );
    assert_eq!(records[0].run_id, Some(run_id));
    assert_eq!(records[0].node_id, Some(ids.tts));
}
