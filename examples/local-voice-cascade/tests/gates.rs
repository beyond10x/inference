//! The local voice cascade on the testkit mocks and the fake audio host, with no network
//! (`story:cascade-commit-gates`; topology B of `docs/design/inference.md` 759-778, the commit
//! rules at 624-628, playback confirmed by the sink alone at 388, section 23.3 tests 3 and 7).
//!
//! The test is the application: it sends a synthetic utterance and the end of input into the
//! ingress, reads both egresses, and plays the fake speaker on, pull by pull, until the run ends.
//! Two nodes are wrapped on the way, as a misbehaving engine would be: the STT node's transcript
//! carries, after each committed span, a revision of that span, and the LLM node's output carries,
//! before its generation's completion, a delta of a segment it never commits. Every node's inputs
//! are recorded, so that what each fan-out received is compared with what each of its consumers
//! received.
//!
//! The scenarios read: `inference.stream.OutputSegment/transition/play/by/inference.stream.
//! AcknowledgePlayback/played` (each answer segment reaches Played, through the sink's receipts
//! only), `inference.stream.Span/state/Committed/refuses/inference.stream.ReviseSpan` (each
//! revision of a committed span is stopped at the input gate) and `inference.stream.OutputSegment/
//! state/Tentative/refuses/inference.stream.QueueForSynthesis` (the Tentative segment is never
//! queued: the output gate holds it and drops it).

use std::collections::HashMap;
use std::future::Future;
use std::num::{NonZeroU16, NonZeroU32};
use std::pin::pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use inference_audio::{AudioEncoding, AudioFormat, AudioFrame};
use inference_audio_device::PlayoutConfig;
use inference_audio_device::fake::FakeHost;
use inference_core::endpoint::{EndpointFuture, InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::ModelId;
use inference_pipeline::{
    Declaration, Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation, NodeInput,
    NodeInstanceId, NodeOutput, PortId,
};
use inference_runtime::RunController;
use inference_runtime::run::{NodeInstanceState, RunState};
use inference_testkit::cascade::{Mocks, TurnStore};
use inference_testkit::graph::RunLog;
use inference_testkit::llm::{MockConfiguration as LlmConfiguration, MockGenerator};
use inference_testkit::stream::StreamEvent;
use inference_testkit::stt::synthetic_audio::{Stretch, synthesize};
use inference_testkit::stt::{
    Cue, MockConfiguration as SttConfiguration, MockRecogniser, ScriptStep,
};
use inference_testkit::tts::{MockConfiguration as TtsConfiguration, MockSynthesiser};
use inference_text::{SegmentId, SegmentState, SpanId, TextDelta, TranscriptRevision};
use local_voice_cascade::{AUDIO_INGRESS, TEXT_EGRESS, TRANSCRIPT_EGRESS, VoicePayload, declare};

/// What the user asks: the recogniser's two scripted spans, in one turn.
const QUESTIONS: [&str; 2] = ["how warm is it outside", "and will it rain"];

/// How long the run may take, pulled on at ten times real time, before the test gives up.
const PATIENCE: Duration = Duration::from_secs(60);

/// The mock LLM's answer to a question: one segment per question.
fn reply(question: &str) -> String {
    format!("I cannot know {question}")
}

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Polls `future` on this thread until it completes: the runtime commits to no executor.
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

/// The scripted mocks: the recogniser emits and commits two spans, the LLM answers each with
/// [`reply`], the synthesiser streams one frame per word.
fn mocks() -> Mocks {
    let cue = |after_frames, step| Cue { after_frames, step };
    Mocks {
        recogniser: MockRecogniser::new(
            SttConfiguration::Incremental,
            ModelId::generate(),
            vec![
                cue(1, ScriptStep::Hypothesis(QUESTIONS[0].to_owned())),
                cue(2, ScriptStep::Commit { span: 0 }),
                cue(3, ScriptStep::Hypothesis(QUESTIONS[1].to_owned())),
                cue(4, ScriptStep::Commit { span: 1 }),
            ],
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
    }
}

/// The synthetic utterance: 300 ms of a 440 Hz tone at half scale, then 800 ms of silence.
fn utterance() -> Vec<AudioFrame> {
    synthesize(&[
        Stretch::Tone {
            frames: 30,
            hertz: 440,
            amplitude: 0.5,
        },
        Stretch::Silence { frames: 80 },
    ])
}

/// What one input port of a node received: the envelope's identity and kind, in arrival order.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Seen {
    port: PortId,
    event_id: EventId,
    kind: String,
}

type Log = Arc<Mutex<Vec<Seen>>>;

fn read(log: &Log) -> Vec<Seen> {
    log.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// What an injector sends before and after the envelope it was shown.
#[derive(Default)]
struct Injection {
    before: Option<EventEnvelope<VoicePayload>>,
    after: Option<EventEnvelope<VoicePayload>>,
}

type Injector = Arc<Mutex<dyn FnMut(&EventEnvelope<VoicePayload>) -> Injection + Send>>;

/// An input edge whose every delivery is recorded.
struct Recording {
    port: PortId,
    inner: Box<dyn InputEndpoint<VoicePayload>>,
    log: Log,
}

impl InputEndpoint<VoicePayload> for Recording {
    fn recv(&mut self) -> EndpointFuture<'_, Option<EventEnvelope<VoicePayload>>> {
        let (port, log) = (self.port, Arc::clone(&self.log));
        let received = self.inner.recv();
        Box::pin(async move {
            let received = received.await;
            if let Some(envelope) = &received {
                log.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(Seen {
                        port,
                        event_id: envelope.event_id,
                        kind: envelope.kind.clone(),
                    });
            }
            received
        })
    }
}

/// An output edge that sends what its injector adds around each envelope.
struct Injecting {
    inner: Box<dyn OutputEndpoint<VoicePayload>>,
    injector: Injector,
}

impl OutputEndpoint<VoicePayload> for Injecting {
    fn send(
        &self,
        envelope: EventEnvelope<VoicePayload>,
    ) -> EndpointFuture<'_, Result<(), SendError<VoicePayload>>> {
        let injection = (self.injector.lock().unwrap_or_else(PoisonError::into_inner))(&envelope);
        Box::pin(async move {
            if let Some(before) = injection.before {
                self.inner.send(before).await?;
            }
            self.inner.send(envelope).await?;
            if let Some(after) = injection.after {
                self.inner.send(after).await?;
            }
            Ok(())
        })
    }
}

/// A node of the cascade, its inputs recorded and, given an injector, its output ports among
/// `inject` (every one when `None`) injected into.
struct Watching {
    inner: Box<dyn NodeImplementation<VoicePayload>>,
    log: Log,
    injector: Option<(Option<PortId>, Injector)>,
}

impl NodeImplementation<VoicePayload> for Watching {
    fn describe(&mut self, node: &Node) -> NodeFuture<'_, ()> {
        self.inner.describe(node)
    }

    fn described(&mut self, instance_id: NodeInstanceId) {
        self.inner.described(instance_id);
    }

    fn prepare(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        self.inner.prepare()
    }

    fn start(
        &mut self,
        endpoints: NodeEndpoints<VoicePayload>,
    ) -> NodeFuture<'_, Result<(), NodeError>> {
        let NodeEndpoints { inputs, outputs } = endpoints;
        let inputs = inputs
            .into_iter()
            .map(|input| NodeInput {
                port_id: input.port_id,
                endpoint: Box::new(Recording {
                    port: input.port_id,
                    inner: input.endpoint,
                    log: Arc::clone(&self.log),
                }) as Box<dyn InputEndpoint<VoicePayload>>,
            })
            .collect();
        let outputs = outputs
            .into_iter()
            .map(|output| match &self.injector {
                Some((port, injector)) if port.is_none_or(|port| port == output.port_id) => {
                    NodeOutput {
                        port_id: output.port_id,
                        endpoint: Box::new(Injecting {
                            inner: output.endpoint,
                            injector: Arc::clone(injector),
                        })
                            as Box<dyn OutputEndpoint<VoicePayload>>,
                    }
                }
                _ => output,
            })
            .collect();
        self.inner.start(NodeEndpoints { inputs, outputs })
    }

    fn drain(&mut self) -> NodeFuture<'_, Result<(), NodeError>> {
        self.inner.drain()
    }

    fn cancel(&mut self) -> NodeFuture<'_, ()> {
        self.inner.cancel()
    }

    fn stop(&mut self) -> NodeFuture<'_, ()> {
        self.inner.stop()
    }
}

/// An envelope of an injector's own stream, numbered from `sequence`, otherwise as `like`.
fn envelope_like(
    like: &EventEnvelope<VoicePayload>,
    stream_id: StreamId,
    sequence: i64,
    kind: &str,
    generation_id: Option<GenerationId>,
    event: StreamEvent,
) -> EventEnvelope<VoicePayload> {
    EventEnvelope {
        schema_version: 1,
        event_id: EventId::generate(),
        run_id: like.run_id,
        session_id: None,
        stream_id,
        sequence,
        producer: like.producer,
        port: like.port.clone(),
        kind: kind.to_owned(),
        generation_id,
        timestamp: like.timestamp,
        timebase: like.timebase,
        causation_id: Some(like.event_id),
        payload: VoicePayload::Stream(event),
    }
}

/// What the injectors added: the event of each revision of a committed span, and the segment
/// and event of the delta never committed.
#[derive(Debug, Default)]
struct Injected {
    revisions: Vec<(SpanId, EventId)>,
    tentative: Option<(SegmentId, EventId)>,
}

/// Revises each committed span right after its commit, on a stream of the injector's own.
fn revising(injected: Arc<Mutex<Injected>>) -> Injector {
    let stream_id = StreamId::generate();
    let mut sequence = 0;
    Arc::new(Mutex::new(move |envelope: &EventEnvelope<VoicePayload>| {
        let VoicePayload::Stream(StreamEvent::TranscriptCommitted(committed)) = &envelope.payload
        else {
            return Injection::default();
        };
        sequence += 1;
        let revision = envelope_like(
            envelope,
            stream_id,
            sequence,
            "inference.stream.TranscriptRevision",
            None,
            StreamEvent::TranscriptRevision(TranscriptRevision {
                span_id: committed.span_id,
                text: "something else entirely".to_owned(),
            }),
        );
        injected
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .revisions
            .push((committed.span_id, revision.event_id));
        Injection {
            before: None,
            after: Some(revision),
        }
    }))
}

/// Emits, right before the generation's completion, a delta of a segment never committed.
fn trailing(injected: Arc<Mutex<Injected>>) -> Injector {
    let stream_id = StreamId::generate();
    Arc::new(Mutex::new(move |envelope: &EventEnvelope<VoicePayload>| {
        let VoicePayload::Stream(StreamEvent::GenerationCompleted(completed)) = &envelope.payload
        else {
            return Injection::default();
        };
        let segment_id = SegmentId::generate();
        let delta = envelope_like(
            envelope,
            stream_id,
            1,
            "inference.stream.TextDelta",
            Some(completed.generation_id),
            StreamEvent::TextDelta(TextDelta {
                segment_id,
                generation_id: completed.generation_id,
                sequence: 1_000,
                text: "and one more thing".to_owned(),
            }),
        );
        injected
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .tentative = Some((segment_id, delta.event_id));
        Injection {
            before: Some(delta),
            after: None,
        }
    }))
}

/// What the application sends on the ingress: frames of its own stream, numbered from 1, on the
/// audio frame clock, then the end of input.
struct Microphone {
    run_id: RunId,
    producer: NodeId,
    stream_id: StreamId,
    sequence: i64,
}

impl Microphone {
    fn envelope(
        &mut self,
        kind: &str,
        timestamp: i64,
        payload: VoicePayload,
    ) -> EventEnvelope<VoicePayload> {
        self.sequence += 1;
        EventEnvelope {
            schema_version: 1,
            event_id: EventId::generate(),
            run_id: self.run_id,
            session_id: None,
            stream_id: self.stream_id,
            sequence: self.sequence,
            producer: self.producer,
            port: AUDIO_INGRESS.to_owned(),
            kind: kind.to_owned(),
            generation_id: None,
            timestamp,
            timebase: Timebase::AudioFrameTime,
            causation_id: None,
            payload,
        }
    }
}

/// Everything `egress` delivers until the node's end of it is gone, read on a thread of its own.
fn drained(
    mut egress: Box<dyn InputEndpoint<VoicePayload>>,
) -> thread::JoinHandle<Vec<EventEnvelope<VoicePayload>>> {
    thread::spawn(move || {
        let mut received = Vec::new();
        while let Some(envelope) = block_on(egress.recv()) {
            received.push(envelope);
        }
        received
    })
}

/// The stream events among `received`.
fn events(received: &[EventEnvelope<VoicePayload>]) -> Vec<StreamEvent> {
    received
        .iter()
        .filter_map(|envelope| match &envelope.payload {
            VoicePayload::Stream(event) => Some(event.clone()),
            _ => None,
        })
        .collect()
}

fn ids(seen: &[Seen]) -> Vec<EventId> {
    seen.iter().map(|seen| seen.event_id).collect()
}

#[test]
fn the_cascade_speaks_committed_answers_through_both_gates_and_plays_them_through_receipts() {
    let host = FakeHost::laptop();
    let mocks = mocks();
    let mut declaration = Declaration::new();
    let cascade = declare(&mut declaration, &mocks).expect("the cascade validates and plans");
    let cascade_ids = cascade.ids;
    let run_id = RunId::generate();
    let turns = TurnStore::default();
    let log = RunLog::new();
    let mut run = cascade
        .nodes(
            run_id,
            mocks,
            turns.clone(),
            &log,
            &host,
            PlayoutConfig::default(),
        )
        .expect("the fake speaker opens");
    assert!(host.output_open());
    let handles = run.handles.clone();

    let injected = Arc::new(Mutex::new(Injected::default()));
    let mut logs: HashMap<NodeId, Log> = HashMap::new();
    let mut watched: HashMap<NodeId, Box<dyn NodeImplementation<VoicePayload>>> = HashMap::new();
    for node_id in cascade_ids.nodes() {
        let inner = run
            .implementations
            .remove(&node_id)
            .expect("an implementation for every node");
        let injector = if node_id == cascade_ids.stt {
            Some((
                cascade_ids.stt_ports.transcript,
                revising(Arc::clone(&injected)),
            ))
        } else if node_id == cascade_ids.llm {
            Some((None, trailing(Arc::clone(&injected))))
        } else {
            None
        };
        let log = Log::default();
        logs.insert(node_id, Arc::clone(&log));
        watched.insert(
            node_id,
            Box::new(Watching {
                inner,
                log,
                injector,
            }),
        );
    }

    let mut controller = RunController::create_as(
        run_id,
        &cascade.planned,
        &declaration,
        HashMap::<GenerationId, i64>::new(),
        |node: &Node| {
            watched
                .remove(&node.node_id())
                .expect("the cascade has an implementation for every node")
        },
    )
    .expect("the cascade is planned and no edge cancels or supersedes");
    controller.validate().expect("a Created run validates");
    block_on(controller.prepare()).expect("every node prepares");
    let mut ends = controller.exposed();
    let ingress = ends.ingress(AUDIO_INGRESS).expect("the audio ingress");
    let transcript = drained(
        ends.egress(TRANSCRIPT_EGRESS)
            .expect("the transcript egress"),
    );
    let text = drained(ends.egress(TEXT_EGRESS).expect("the assistant text egress"));
    block_on(controller.activate()).expect("every node starts");
    assert_eq!(controller.run().state(), RunState::Active);
    let queues = controller.queues();
    let exposure_queues = controller.exposure_queues();
    let drive = thread::spawn(move || {
        let result = block_on(controller.drive());
        (controller, result)
    });

    let mut microphone = Microphone {
        run_id,
        producer: NodeId::generate(),
        stream_id: StreamId::generate(),
        sequence: 0,
    };
    let mut last = 0;
    for frame in utterance() {
        let timestamp = i64::try_from(frame.position.get()).expect("within i64");
        last = timestamp + i64::from(frame.sample_count);
        let envelope = microphone.envelope(
            "inference.audio.AudioFrame",
            timestamp,
            VoicePayload::Audio(frame),
        );
        block_on(ingress.send(envelope)).expect("the ingress takes every frame");
    }
    let stop = microphone.envelope(
        ControlKind::Stop.as_str(),
        last,
        VoicePayload::Control(ControlKind::Stop),
    );
    block_on(ingress.send(stop)).expect("the ingress takes the end of input");

    let deadline = Instant::now() + PATIENCE;
    while !drive.is_finished() {
        assert!(
            Instant::now() < deadline,
            "the run ends once the speaker played everything; sink {:?}, tts {:?}",
            handles.sink.receipts(),
            handles.tts.played()
        );
        let _ = host.pull(480);
        thread::sleep(Duration::from_millis(1));
    }
    let transcript = transcript.join().expect("the transcript reader");
    let text = text.join().expect("the text reader");
    drop(ingress);
    drop(ends);
    let (controller, result) = drive.join().expect("the drive loop does not panic");

    assert_eq!(handles.stt.failure(), None);
    assert_eq!(handles.input_gate.failure(), None);
    assert_eq!(handles.llm.failure(), None);
    assert_eq!(handles.output_gate.failure(), None);
    assert_eq!(handles.tts.failure(), None);
    assert_eq!(handles.sink.failure(), None);
    result.expect("the drive loop ends the run");

    // Test 7 of section 23.3: after the stop the run is Completed, every node instance Stopped,
    // every queue dropped and the speaker closed.
    assert_eq!(controller.run().state(), RunState::Completed);
    for node_id in cascade_ids.nodes() {
        assert_eq!(
            controller
                .instance(node_id)
                .map(|instance| instance.state()),
            Some(NodeInstanceState::Stopped),
            "the instance of node {node_id}"
        );
    }
    assert_eq!(controller.refusals(), []);
    drop(controller);
    for queue in &queues {
        assert!(queue.is_dropped(), "the queue of edge {}", queue.edge_id());
    }
    for queue in &exposure_queues {
        assert!(
            queue.is_dropped(),
            "the queue of exposure {}",
            queue.exposure_id()
        );
    }
    assert!(!host.output_open(), "the speaker is closed");
    assert!(handles.sink.ended());

    let injected = injected.lock().unwrap_or_else(PoisonError::into_inner);
    let seen = |node_id: NodeId| read(&logs[&node_id]);

    // The transcript: both spans committed, in one turn.
    let turn_rows = turns.with(|stream| stream.turns_view());
    assert_eq!(turn_rows.len(), 1, "one utterance, one turn");
    let committed_spans: Vec<(SpanId, String)> = events(&transcript)
        .into_iter()
        .filter_map(|event| match event {
            StreamEvent::TranscriptCommitted(committed) => {
                Some((committed.span_id, committed.text))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        committed_spans
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>(),
        QUESTIONS
    );

    // Span/state/Committed/refuses/ReviseSpan: each revision of a committed span is refused at
    // the input gate and never reaches the LLM.
    assert_eq!(
        injected
            .revisions
            .iter()
            .map(|(span_id, _)| *span_id)
            .collect::<Vec<_>>(),
        committed_spans
            .iter()
            .map(|(span_id, _)| *span_id)
            .collect::<Vec<_>>(),
        "a revision followed each committed span"
    );
    let refused = handles.input_gate.refused_revisions();
    assert_eq!(
        refused
            .iter()
            .map(|refused| (refused.span_id, refused.event_id))
            .collect::<Vec<_>>(),
        injected.revisions
    );
    let llm_heard = seen(cascade_ids.llm);
    for (_, revision) in &injected.revisions {
        assert!(!ids(&llm_heard).contains(revision));
    }
    assert!(
        llm_heard.iter().all(|seen| {
            seen.kind != "inference.stream.TranscriptRevision"
                && seen.kind != "inference.stream.TranscriptHypothesis"
        }),
        "the LLM is handed committed spans and turn events only: {llm_heard:?}"
    );
    assert_eq!(
        handles.input_gate.record().held_back,
        2,
        "both hypotheses held back"
    );

    // The answers: one committed segment per question, and the tentative tail the LLM's output
    // carried, never committed.
    let answered = events(&text);
    let answers: Vec<(SegmentId, String)> = answered
        .iter()
        .filter_map(|event| match event {
            StreamEvent::TextSegmentCommitted(committed) => {
                Some((committed.segment_id, committed.text.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        answers
            .iter()
            .map(|(_, text)| text.clone())
            .collect::<Vec<_>>(),
        QUESTIONS.map(reply)
    );
    let answer_ids: Vec<SegmentId> = answers.iter().map(|(segment_id, _)| *segment_id).collect();
    let (tentative, tentative_event) = injected.tentative.expect("the LLM's output ended");
    assert!(
        answered.iter().any(
            |event| matches!(event, StreamEvent::TextDelta(delta) if delta.segment_id == tentative)
        ),
        "the egress, before the gate, carries the Tentative delta"
    );

    // OutputSegment/state/Tentative/refuses/QueueForSynthesis: the output gate held the
    // Tentative segment and dropped it; the TTS node never saw it and queued only the answers.
    assert_eq!(handles.output_gate.dropped_segments(), [tentative]);
    assert_eq!(handles.output_gate.record().committed, answer_ids);
    let tts_heard = seen(cascade_ids.tts);
    assert!(!ids(&tts_heard).contains(&tentative_event));
    assert_eq!(handles.tts.queued(), answer_ids);

    // OutputSegment/transition/play/by/AcknowledgePlayback/played: every answer segment reached
    // Played, each by the receipt the sink made of the frames it played, one frame per word.
    let played = handles.tts.played();
    assert_eq!(
        played
            .iter()
            .map(|segment| (segment.segment_id(), segment.state()))
            .collect::<Vec<_>>(),
        answer_ids
            .iter()
            .map(|segment_id| (*segment_id, SegmentState::Played))
            .collect::<Vec<_>>()
    );
    let receipts: Vec<(SegmentId, usize)> = handles
        .sink
        .receipts()
        .iter()
        .map(|receipt| (receipt.segment_id, receipt.frames))
        .collect();
    let words: Vec<(SegmentId, usize)> = answers
        .iter()
        .map(|(segment_id, text)| (*segment_id, text.split_whitespace().count()))
        .collect();
    assert_eq!(
        receipts, words,
        "the sink's receipts, of every frame it played"
    );
    assert_eq!(
        handles
            .tts
            .acknowledged()
            .iter()
            .map(|acknowledged| (acknowledged.segment_id, acknowledged.frames))
            .collect::<Vec<_>>(),
        receipts,
        "the TTS node acknowledged with those receipts and nothing else"
    );
    let receipts_heard = tts_heard
        .iter()
        .filter(|seen| seen.port == cascade_ids.speech_ports.played)
        .filter(|seen| seen.kind == "inference.stream.AcknowledgePlayback")
        .count();
    assert_eq!(receipts_heard, answers.len(), "over the feedback edge");
    let played_logged: Vec<SegmentId> = log
        .envelopes()
        .into_iter()
        .filter_map(|envelope| match envelope.payload {
            StreamEvent::AudioPlayed(played) => Some(played.segment_id),
            _ => None,
        })
        .collect();
    assert_eq!(played_logged, answer_ids);

    // Test 3 of section 23.3: each fan-out delivered to every output in the order it received.
    let split = ids(&seen(cascade_ids.split));
    assert_eq!(
        split.len(),
        utterance().len() + 1,
        "every frame and the end"
    );
    assert_eq!(
        ids(&seen(cascade_ids.vad)),
        split,
        "the detector heard it all, in order"
    );
    let stt_audio: Vec<Seen> = seen(cascade_ids.stt)
        .into_iter()
        .filter(|seen| seen.port == cascade_ids.stt_ports.audio)
        .collect();
    assert_eq!(ids(&stt_audio), split, "and so did the STT node");
    let text_split = ids(&seen(cascade_ids.text_split));
    assert!(text_split.contains(&tentative_event));
    assert_eq!(
        ids(&seen(cascade_ids.output_gate)),
        text_split,
        "the output gate was handed the LLM's output in order"
    );
    assert_eq!(
        text.iter()
            .map(|envelope| envelope.event_id)
            .collect::<Vec<_>>(),
        text_split,
        "and so was the egress"
    );

    // The measure: committed input to first playable audio, recorded for the turn.
    let measures = handles.committed_input_to_first_audio();
    assert_eq!(measures.len(), 1);
    assert_eq!(measures[0].turn_id, turn_rows[0].turn_id);
    assert!(
        measures[0].first_audio.is_some(),
        "the first frame after the turn's completion was handed to the speaker"
    );
}
