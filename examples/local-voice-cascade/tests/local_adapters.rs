//! The local voice cascade over the local adapters: the sherpa-onnx recogniser, the generator of
//! a loopback `llama-server` and the testkit's mock synthesiser, behind the TTS binding until a
//! local TTS adapter exists (`story:local-voice-cascade-example`; P2's exit at
//! `docs/design/inference.md` 1160, A-04, A-05, A-08 and A-10 at 1140-1146, the measures at
//! 1022-1023).
//!
//! The three cases that need the adapters are ignored. They read the checkpoint directory from
//! `INFERENCE_SHERPA_ONNX_STT_DIR` and the server from `INFERENCE_LLAMA_SERVER_URL` and
//! `INFERENCE_LLAMA_SERVER_MODEL`, and with `--ignored` fail naming the variable that is missing.
//! The spoken question is the checkpoint's own LibriSpeech clip `test_wavs/0.wav` ("after early
//! nightfall the yellow lamps would light up here and there the squalid quarter of the
//! brothels"), followed by a second of silence. The speaker is the fake audio host's, which the
//! test pulls at the real-time rate, 10 ms of device audio every 10 ms, while the run goes:
//!
//! * offline answer: the clip, under the egress watch, yields a committed span containing
//!   "nightfall", committed assistant text, each segment Played through the sink's receipts, and
//!   a run that reaches Completed after `stop` with no failure and the watch not tripped;
//! * A-08: a revision naming a Committed span the recogniser produced, injected at the input
//!   gate, is refused there (`inference.stream.Span/state/Committed/refuses/
//!   inference.stream.ReviseSpan`), and a Tentative delta of the server's generation, injected at
//!   the output gate before the generation completes, is never queued for synthesis
//!   (`inference.stream.OutputSegment/state/Tentative/refuses/inference.stream.QueueForSynthesis`);
//! * A-10: the clip said again while the first answer plays stops the speaker, the first
//!   generation is Superseded, and no frame of it is played after the stop.
//!
//! The cases that need nothing run always: a server URL whose host is not loopback is refused
//! before anything is opened, a missing variable is named, and the cascade answers on a mock
//! generator that, as the server's adapter, claims output streaming only.
//!
//! The watch reads every socket of the process and the server answers one request at a time, so
//! the cases that run the cascade hold one lock and never overlap.

#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::future::Future;
use std::num::{NonZeroU16, NonZeroU32};
use std::path::Path;
use std::pin::pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant};

use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_audio_device::PlayoutConfig;
use inference_audio_device::fake::FakeHost;
use inference_core::endpoint::{EndpointFuture, InputEndpoint, OutputEndpoint, SendError};
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::generation::GenerationState;
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{Locality, ModelId};
use inference_core::policy::Egress;
use inference_pipeline::{
    Declaration, Node, NodeEndpoints, NodeError, NodeFuture, NodeImplementation, NodeInput,
    NodeInstanceId, NodeOutput, PortId,
};
use inference_runtime::RunController;
use inference_runtime::egress::EgressWatch;
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
use local_voice_cascade::local::{
    LocalConfig, LocalModels, LocalRefused, SERVER_MODEL_VARIABLE, SERVER_URL_VARIABLE,
    STT_DIR_VARIABLE, declare_local, loopback,
};
use local_voice_cascade::{
    AUDIO_INGRESS, CascadeHandles, CascadeIds, LocalCascade, TEXT_EGRESS, TRANSCRIPT_EGRESS,
    VoicePayload, declare,
};

/// The checkpoint's LibriSpeech clip, relative to the checkpoint directory.
const CLIP: &str = "test_wavs/0.wav";

/// The word of the clip the committed span is to contain.
const HEARD: &str = "nightfall";

/// How long a run with the real adapters may take on CPU before the case gives up.
const PATIENCE: Duration = Duration::from_secs(180);

/// Device frames of the fake laptop's speaker, at 48 kHz: 10 ms, which play one frame of the
/// mock synthesiser, 160 samples at 16 kHz.
const DEVICE_FRAME: usize = 480;

/// The real-time interval of one [`DEVICE_FRAME`].
const DEVICE_TICK: Duration = Duration::from_millis(10);

static SERIAL: Mutex<()> = Mutex::new(());

/// Holds the lock of the cases that run the cascade.
fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
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

/// The configuration the variables name; a missing one fails the case, naming it.
fn configured() -> LocalConfig {
    LocalConfig::from_env().unwrap_or_else(|refused| panic!("{refused}"))
}

/// The samples of the 16 kHz mono 16-bit PCM WAV file at `path`.
fn pcm16_mono(path: &Path) -> (AudioFormat, Vec<u8>) {
    let bytes = std::fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    assert!(
        bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE",
        "{} is a RIFF WAVE file",
        path.display()
    );
    let field = |at: usize, width: usize| {
        bytes[at..at + width]
            .iter()
            .rev()
            .fold(0_usize, |value, byte| (value << 8) | usize::from(*byte))
    };
    let mut at = 12;
    let mut format = None;
    while at + 8 <= bytes.len() {
        let size = field(at + 4, 4);
        let body = at + 8;
        match &bytes[at..at + 4] {
            b"fmt " => {
                let (tag, channels, rate, bits) = (
                    field(body, 2),
                    field(body + 2, 2),
                    field(body + 4, 4),
                    field(body + 14, 2),
                );
                assert_eq!(
                    (tag, channels, rate, bits),
                    (1, 1, 16_000, 16),
                    "the clip is 16 kHz mono pcm_s16le"
                );
                format = Some(AudioFormat {
                    encoding: AudioEncoding::PcmS16le,
                    sample_rate: NonZeroU32::new(16_000).expect("16000"),
                    channels: NonZeroU16::new(1).expect("1"),
                });
            }
            b"data" => {
                let end = (body + size).min(bytes.len());
                let data = bytes[body..end - (end - body) % 2].to_vec();
                return (format.expect("a fmt chunk before the data"), data);
            }
            _ => {}
        }
        at = body + size + size % 2;
    }
    panic!("{} has no data chunk", path.display());
}

/// What the application sends on the ingress: frames of its own stream, numbered from 1, on the
/// audio frame clock, then the end of input.
struct Microphone {
    run_id: RunId,
    producer: NodeId,
    stream_id: StreamId,
    sequence: i64,
    /// The sample clock position just past the last frame said.
    position: u64,
}

impl Microphone {
    fn new(run_id: RunId) -> Self {
        Self {
            run_id,
            producer: NodeId::generate(),
            stream_id: StreamId::generate(),
            sequence: 0,
            position: 0,
        }
    }

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

    /// The clip at `path` in 10 ms frames, then a second of silence, on the microphone's clock.
    fn say(&mut self, path: &Path) -> Vec<EventEnvelope<VoicePayload>> {
        let (format, mut data) = pcm16_mono(path);
        data.extend(std::iter::repeat_n(0, 16_000 * 2));
        data.chunks(160 * 2)
            .map(|chunk| {
                let sample_count = u32::try_from(chunk.len() / 2).expect("a frame fits u32");
                let frame = AudioFrame {
                    format,
                    position: SampleClockPosition::new(self.position).expect("on the clock"),
                    sample_count,
                    data: chunk.to_vec(),
                };
                let timestamp = i64::try_from(self.position).expect("within i64");
                self.position += u64::from(sample_count);
                self.envelope(
                    "inference.audio.AudioFrame",
                    timestamp,
                    VoicePayload::Audio(frame),
                )
            })
            .collect()
    }

    /// Frames of synthetic audio on the microphone's clock.
    fn say_frames(&mut self, frames: Vec<AudioFrame>) -> Vec<EventEnvelope<VoicePayload>> {
        frames
            .into_iter()
            .map(|mut frame| {
                frame.position = SampleClockPosition::new(self.position).expect("on the clock");
                let timestamp = i64::try_from(self.position).expect("within i64");
                self.position += u64::from(frame.sample_count);
                self.envelope(
                    "inference.audio.AudioFrame",
                    timestamp,
                    VoicePayload::Audio(frame),
                )
            })
            .collect()
    }

    fn stop(&mut self) -> EventEnvelope<VoicePayload> {
        let last = i64::try_from(self.position).expect("within i64");
        self.envelope(
            ControlKind::Stop.as_str(),
            last,
            VoicePayload::Control(ControlKind::Stop),
        )
    }
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

fn event_ids(seen: &[Seen]) -> Vec<EventId> {
    seen.iter().map(|seen| seen.event_id).collect()
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

/// An envelope of an injector's own stream, numbered `sequence`, otherwise as `like`.
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

/// Everything `egress` delivers until the node's end of it is gone, read on a thread of its own.
fn drained(
    mut egress: Box<dyn InputEndpoint<VoicePayload>>,
) -> JoinHandle<Vec<EventEnvelope<VoicePayload>>> {
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

/// The text of every span committed among `received`, with its span.
fn committed_spans(received: &[EventEnvelope<VoicePayload>]) -> Vec<(SpanId, String)> {
    events(received)
        .into_iter()
        .filter_map(|event| match event {
            StreamEvent::TranscriptCommitted(committed) => {
                Some((committed.span_id, committed.text))
            }
            _ => None,
        })
        .collect()
}

/// The segment and text of every segment committed among `received`.
fn committed_segments(received: &[EventEnvelope<VoicePayload>]) -> Vec<(SegmentId, String)> {
    events(received)
        .into_iter()
        .filter_map(|event| match event {
            StreamEvent::TextSegmentCommitted(committed) => {
                Some((committed.segment_id, committed.text))
            }
            _ => None,
        })
        .collect()
}

/// A run of the cascade, Active and driven on a thread of its own, its ingress, its egresses
/// read to their end and the egress watch over it.
struct Running {
    run_id: RunId,
    ingress: Box<dyn OutputEndpoint<VoicePayload>>,
    transcript: JoinHandle<Vec<EventEnvelope<VoicePayload>>>,
    text: JoinHandle<Vec<EventEnvelope<VoicePayload>>>,
    drive: JoinHandle<(
        RunController<VoicePayload>,
        Result<(), inference_runtime::RunError>,
    )>,
    watch: EgressWatch,
}

impl Running {
    /// Creates, starts and drives the run `run_id` of `cascade` on `implementations`.
    fn start(
        run_id: RunId,
        cascade: &LocalCascade,
        declaration: &Declaration,
        mut implementations: HashMap<NodeId, Box<dyn NodeImplementation<VoicePayload>>>,
    ) -> Self {
        let mut controller = RunController::create_as(
            run_id,
            &cascade.planned,
            declaration,
            HashMap::<GenerationId, i64>::new(),
            |node: &Node| {
                implementations
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
        let watch = EgressWatch::start(Egress::Denied, controller.requests())
            .expect("the watch thread starts")
            .expect("a denied egress is watched");
        let drive = thread::spawn(move || {
            let result = block_on(controller.drive());
            (controller, result)
        });
        let deadline = Instant::now() + PATIENCE;
        while watch.polls() < 2 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        Self {
            run_id,
            ingress,
            transcript,
            text,
            drive,
            watch,
        }
    }

    fn send(&self, envelopes: Vec<EventEnvelope<VoicePayload>>) {
        for envelope in envelopes {
            block_on(self.ingress.send(envelope)).expect("the ingress takes every frame");
        }
    }

    /// Plays the speaker at the real-time rate until the run ends, then reads what it left.
    fn finish(self, host: &FakeHost, handles: &CascadeHandles) -> Finished {
        let deadline = Instant::now() + PATIENCE;
        while !self.drive.is_finished() {
            assert!(
                Instant::now() < deadline,
                "the run ends once the speaker played everything; stt {:?}, llm {:?}, tts {:?}, \
                 sink {:?}",
                handles.stt.failure(),
                handles.llm.failure(),
                handles.tts.played(),
                handles.sink.receipts()
            );
            let _ = host.pull(DEVICE_FRAME);
            thread::sleep(DEVICE_TICK);
        }
        let transcript = self.transcript.join().expect("the transcript reader");
        let text = self.text.join().expect("the text reader");
        drop(self.ingress);
        let (controller, result) = self.drive.join().expect("the drive loop does not panic");
        Finished {
            controller,
            result,
            transcript,
            text,
            tripped: self.watch.tripped(),
            polls: self.watch.polls(),
        }
    }
}

/// What a run left.
struct Finished {
    controller: RunController<VoicePayload>,
    result: Result<(), inference_runtime::RunError>,
    transcript: Vec<EventEnvelope<VoicePayload>>,
    text: Vec<EventEnvelope<VoicePayload>>,
    tripped: Option<std::net::SocketAddr>,
    polls: u64,
}

impl Finished {
    /// The run reached Completed under the watch, with no node failed and every instance
    /// Stopped.
    fn assert_completed(&self, ids: &CascadeIds, handles: &CascadeHandles) {
        let failures = [
            ("stt", handles.stt.failure()),
            ("input gate", handles.input_gate.failure()),
            ("agent", handles.llm.failure()),
            ("output gate", handles.output_gate.failure()),
            ("tts", handles.tts.failure()),
            ("speaker", handles.sink.failure()),
        ];
        assert!(
            failures.iter().all(|(_, failure)| failure.is_none()),
            "no node failed: {failures:?}"
        );
        assert!(
            self.result.is_ok(),
            "the drive loop ends the run: {:?}",
            self.result
        );
        assert_eq!(self.controller.run().state(), RunState::Completed);
        for node_id in ids.nodes() {
            assert_eq!(
                self.controller
                    .instance(node_id)
                    .map(|instance| instance.state()),
                Some(NodeInstanceState::Stopped),
                "the instance of node {node_id}"
            );
        }
        assert_eq!(self.tripped, None, "the loopback server is not egress");
        assert!(self.polls >= 2, "the watch polled while the run was Active");
    }
}

/// Prints the two measures of section 20 the run recorded.
fn print_measures(handles: &CascadeHandles) {
    for measure in handles.committed_input_to_first_audio() {
        println!(
            "committed input to first playable audio, turn {}: {:?}",
            measure.turn_id, measure.first_audio
        );
    }
    for measure in handles.interruption_to_playback_stop() {
        println!(
            "interruption to playback stop, turn {}: {:?}",
            measure.turn_id, measure.stop
        );
    }
}

#[test]
#[ignore = "needs the STT checkpoint in INFERENCE_SHERPA_ONNX_STT_DIR and a loopback llama-server at INFERENCE_LLAMA_SERVER_URL serving INFERENCE_LLAMA_SERVER_MODEL"]
fn the_clip_is_answered_offline_and_every_segment_is_played_through_receipts() {
    let _serial = serial();
    let config = configured();
    let models = LocalModels::open(&config).unwrap_or_else(|refused| panic!("{refused}"));
    let host = FakeHost::laptop();
    let mut declaration = Declaration::new();
    let cascade =
        declare_local(&mut declaration, &models).expect("the cascade validates and plans");
    let run_id = RunId::generate();
    let turns = TurnStore::default();
    let log = RunLog::new();
    let run = cascade
        .local_nodes(
            run_id,
            models,
            turns.clone(),
            &log,
            &host,
            &Default::default(),
            PlayoutConfig::default(),
        )
        .expect("the fake speaker opens");
    let handles = run.handles.clone();
    let running = Running::start(run_id, &cascade, &declaration, run.implementations);

    let mut microphone = Microphone::new(running.run_id);
    running.send(microphone.say(&config.stt_dir.join(CLIP)));
    running.send(vec![microphone.stop()]);
    let finished = running.finish(&host, &handles);
    print_measures(&handles);

    finished.assert_completed(&cascade.ids, &handles);
    assert!(!host.output_open(), "the speaker is closed");
    let spans = committed_spans(&finished.transcript);
    println!("heard: {spans:?}");
    assert!(
        spans
            .iter()
            .any(|(_, text)| text.to_lowercase().contains(HEARD)),
        "a committed span contains {HEARD:?}: {spans:?}"
    );
    let answer = committed_segments(&finished.text);
    let said: String = answer.iter().map(|(_, text)| text.as_str()).collect();
    println!("answered: {said:?}");
    assert!(!said.trim().is_empty(), "the server's answer was committed");

    let generations = handles.llm.generations();
    assert_eq!(generations.len(), 1, "one turn, one generation");
    assert_eq!(generations[0].state, GenerationState::Completed);
    let queued = handles.tts.queued();
    assert_eq!(
        queued,
        answer
            .iter()
            .map(|(segment_id, _)| *segment_id)
            .collect::<Vec<_>>(),
        "every committed segment was queued for synthesis"
    );
    assert_eq!(
        handles
            .tts
            .played()
            .iter()
            .map(|segment| (segment.segment_id(), segment.state()))
            .collect::<Vec<_>>(),
        queued
            .iter()
            .map(|segment_id| (*segment_id, SegmentState::Played))
            .collect::<Vec<_>>(),
        "every segment reached Played"
    );
    assert_eq!(
        handles
            .sink
            .receipts()
            .iter()
            .map(|receipt| receipt.segment_id)
            .collect::<Vec<_>>(),
        queued,
        "each by the receipt of the frames the sink played"
    );
    assert_eq!(
        handles
            .tts
            .acknowledged()
            .iter()
            .map(|acknowledged| acknowledged.segment_id)
            .collect::<Vec<_>>(),
        queued,
        "the TTS node acknowledged with those receipts and nothing else"
    );
    let measures = handles.committed_input_to_first_audio();
    assert_eq!(measures.len(), 1);
    assert!(measures[0].first_audio.is_some(), "the answer played");
}

#[test]
#[ignore = "needs the STT checkpoint in INFERENCE_SHERPA_ONNX_STT_DIR and a loopback llama-server at INFERENCE_LLAMA_SERVER_URL serving INFERENCE_LLAMA_SERVER_MODEL"]
fn a_revised_committed_span_and_a_tentative_delta_of_the_server_are_stopped_at_the_gates() {
    let _serial = serial();
    let config = configured();
    let models = LocalModels::open(&config).unwrap_or_else(|refused| panic!("{refused}"));
    let host = FakeHost::laptop();
    let mut declaration = Declaration::new();
    let cascade =
        declare_local(&mut declaration, &models).expect("the cascade validates and plans");
    let ids = cascade.ids;
    let run_id = RunId::generate();
    let log = RunLog::new();
    let mut run = cascade
        .local_nodes(
            run_id,
            models,
            TurnStore::default(),
            &log,
            &host,
            &Default::default(),
            PlayoutConfig::default(),
        )
        .expect("the fake speaker opens");
    let handles = run.handles.clone();

    let injected = Arc::new(Mutex::new(Injected::default()));
    let mut logs: HashMap<NodeId, Log> = HashMap::new();
    let mut watched: HashMap<NodeId, Box<dyn NodeImplementation<VoicePayload>>> = HashMap::new();
    for node_id in ids.nodes() {
        let inner = run
            .implementations
            .remove(&node_id)
            .expect("an implementation for every node");
        let injector = if node_id == ids.stt {
            Some((ids.stt_ports.transcript, revising(Arc::clone(&injected))))
        } else if node_id == ids.llm {
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
    let running = Running::start(run_id, &cascade, &declaration, watched);
    let mut microphone = Microphone::new(running.run_id);
    running.send(microphone.say(&config.stt_dir.join(CLIP)));
    running.send(vec![microphone.stop()]);
    let finished = running.finish(&host, &handles);
    print_measures(&handles);
    finished.assert_completed(&ids, &handles);

    let injected = injected.lock().unwrap_or_else(PoisonError::into_inner);
    let seen = |node_id: NodeId| read(&logs[&node_id]);

    // Span/state/Committed/refuses/ReviseSpan: a revision followed each span the recogniser
    // committed, and each is refused at the input gate and never reaches the LLM.
    let spans = committed_spans(&finished.transcript);
    assert!(!spans.is_empty(), "the recogniser committed a span");
    assert_eq!(
        injected
            .revisions
            .iter()
            .map(|(span_id, _)| *span_id)
            .collect::<Vec<_>>(),
        spans
            .iter()
            .map(|(span_id, _)| *span_id)
            .collect::<Vec<_>>(),
        "a revision followed each committed span"
    );
    assert_eq!(
        handles
            .input_gate
            .refused_revisions()
            .iter()
            .map(|refused| (refused.span_id, refused.event_id))
            .collect::<Vec<_>>(),
        injected.revisions
    );
    let llm_heard = seen(ids.llm);
    for (_, revision) in &injected.revisions {
        assert!(!event_ids(&llm_heard).contains(revision));
    }
    assert!(
        llm_heard.iter().all(|seen| {
            seen.kind != "inference.stream.TranscriptRevision"
                && seen.kind != "inference.stream.TranscriptHypothesis"
        }),
        "the LLM is handed committed spans and turn events only: {llm_heard:?}"
    );

    // OutputSegment/state/Tentative/refuses/QueueForSynthesis: the delta of the server's
    // generation that was never committed is held at the output gate and dropped; the TTS node
    // never sees it and queues only the committed segments.
    let generations = handles.llm.generations();
    assert_eq!(generations.len(), 1);
    let (tentative, tentative_event) = injected.tentative.expect("the generation completed");
    let answered = events(&finished.text);
    assert!(
        answered.iter().any(|event| matches!(
            event,
            StreamEvent::TextDelta(delta)
                if delta.segment_id == tentative
                    && delta.generation_id == generations[0].generation_id
        )),
        "the egress, before the gate, carries the Tentative delta of the server's generation"
    );
    let answer: Vec<SegmentId> = committed_segments(&finished.text)
        .into_iter()
        .map(|(segment_id, _)| segment_id)
        .collect();
    assert!(!answer.is_empty(), "the server's answer was committed");
    assert_eq!(handles.output_gate.dropped_segments(), [tentative]);
    assert_eq!(handles.output_gate.record().committed, answer);
    assert!(!event_ids(&seen(ids.tts)).contains(&tentative_event));
    assert_eq!(handles.tts.queued(), answer);
}

#[test]
#[ignore = "needs the STT checkpoint in INFERENCE_SHERPA_ONNX_STT_DIR and a loopback llama-server at INFERENCE_LLAMA_SERVER_URL serving INFERENCE_LLAMA_SERVER_MODEL"]
#[allow(clippy::too_many_lines)]
fn the_clip_said_again_while_the_answer_plays_stops_it_and_supersedes_its_generation() {
    let _serial = serial();
    let config = configured();
    let models = LocalModels::open(&config).unwrap_or_else(|refused| panic!("{refused}"));
    let host = FakeHost::laptop();
    let mut declaration = Declaration::new();
    let cascade =
        declare_local(&mut declaration, &models).expect("the cascade validates and plans");
    let run_id = RunId::generate();
    let turns = TurnStore::default();
    let log = RunLog::new();
    let run = cascade
        .local_nodes(
            run_id,
            models,
            turns.clone(),
            &log,
            &host,
            &Default::default(),
            PlayoutConfig::default(),
        )
        .expect("the fake speaker opens");
    let handles = run.handles.clone();
    let running = Running::start(run_id, &cascade, &declaration, run.implementations);
    let clip = config.stt_dir.join(CLIP);
    let mut microphone = Microphone::new(running.run_id);

    // The first question; the device plays at the real-time rate until the first segment of the
    // answer is acknowledged and more of it waits in the speaker's queue, then holds.
    running.send(microphone.say(&clip));
    let deadline = Instant::now() + PATIENCE;
    loop {
        assert!(
            Instant::now() < deadline,
            "the first answer plays: llm {:?}, sink pushed {}, receipts {:?}",
            handles.llm.failure(),
            handles.sink.pushed(),
            handles.sink.receipts()
        );
        let receipts = handles.sink.receipts();
        let played: usize = receipts.iter().map(|receipt| receipt.frames).sum();
        if !receipts.is_empty() && handles.sink.pushed() > played as u64 {
            break;
        }
        if receipts.is_empty() {
            let _ = host.pull(DEVICE_FRAME);
        }
        thread::sleep(DEVICE_TICK);
    }
    let receipts_before = handles.sink.receipts().len();

    // The question again, while the rest of the answer waits unplayed.
    running.send(microphone.say(&clip));
    let deadline = Instant::now() + PATIENCE;
    while handles.sink.interruptions().is_empty() {
        assert!(
            Instant::now() < deadline,
            "the sink stops the speaker at the second turn's opening"
        );
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        handles.sink.receipts().len(),
        receipts_before,
        "no receipt while the device held"
    );
    let deadline = Instant::now() + PATIENCE;
    while handles.llm.superseded().is_empty() {
        assert!(
            Instant::now() < deadline,
            "the first generation is superseded: llm {:?}",
            handles.llm.failure()
        );
        thread::sleep(Duration::from_millis(2));
    }
    running.send(vec![microphone.stop()]);
    let finished = running.finish(&host, &handles);
    print_measures(&handles);
    finished.assert_completed(&cascade.ids, &handles);

    let turn_rows = turns.with(|stream| stream.turns_view());
    assert_eq!(turn_rows.len(), 2, "two questions, two turns");
    let spans = committed_spans(&finished.transcript);
    println!("heard: {spans:?}");
    assert_eq!(
        spans
            .iter()
            .filter(|(_, text)| text.to_lowercase().contains(HEARD))
            .count(),
        2,
        "each question was heard: {spans:?}"
    );

    // SupersedeGeneration: the first generation, answering the first turn, was superseded at
    // the second turn's opening; the second answers the second turn and completed.
    let generations = handles.llm.generations();
    assert_eq!(
        generations
            .iter()
            .map(|generation| (generation.turn_id, generation.state))
            .collect::<Vec<_>>(),
        [
            (turn_rows[0].turn_id, GenerationState::Superseded),
            (turn_rows[1].turn_id, GenerationState::Completed),
        ]
    );
    let (old, new) = (&generations[0], &generations[1]);
    let superseded = handles.llm.superseded();
    assert_eq!(superseded.len(), 1);
    assert_eq!(superseded[0].generation_id, old.generation_id);
    assert_eq!(superseded[0].turn_id, turn_rows[1].turn_id);

    // The speaker stopped at the second turn's opening, its queue holding unplayed audio of the
    // first answer, and the measure was recorded.
    let interruptions = handles.sink.interruptions();
    assert_eq!(interruptions.len(), 1);
    let interruption = interruptions[0];
    assert_eq!(interruption.turn_id, turn_rows[1].turn_id);
    assert!(interruption.cleared > 0, "unplayed audio was in the queue");
    let measures = handles.interruption_to_playback_stop();
    assert_eq!(measures.len(), 1);
    assert_eq!(measures[0].stop, interruption.stop);

    // No frame of the first generation was played after the stop: every receipt of its
    // segments was made before it, only those segments are Played, and every frame of it that
    // reached the sink after the stop was dropped with the played position unchanged.
    let old_segments: Vec<SegmentId> = old
        .segments
        .iter()
        .map(|segment| segment.segment_id())
        .collect();
    let receipts = handles.sink.receipts();
    let old_receipts: Vec<SegmentId> = receipts
        .iter()
        .filter(|receipt| old_segments.contains(&receipt.segment_id))
        .map(|receipt| {
            assert!(
                receipt.at < interruption.at,
                "a receipt made after the stop"
            );
            receipt.segment_id
        })
        .collect();
    assert_eq!(old_receipts.len(), receipts_before);
    let played = handles.tts.played();
    assert_eq!(
        played
            .iter()
            .filter(|segment| old_segments.contains(&segment.segment_id()))
            .map(|segment| segment.segment_id())
            .collect::<Vec<_>>(),
        old_receipts,
        "only the segments acknowledged before the stop are Played"
    );
    for frame in handles.sink.stale_frames() {
        assert_eq!(frame.generation_id, Some(old.generation_id));
        assert_eq!(frame.played, interruption.played_after);
    }
    assert!(
        old.segments.len() > old_receipts.len(),
        "part of the first answer was cut"
    );
    for segment in &old.segments {
        if !old_receipts.contains(&segment.segment_id()) {
            assert_eq!(segment.state(), SegmentState::Discarded);
        }
    }

    // The second answer was played and acknowledged, every segment by the sink's receipt.
    let new_segments: Vec<SegmentId> = new
        .segments
        .iter()
        .map(|segment| segment.segment_id())
        .collect();
    assert!(!new_segments.is_empty(), "the second question was answered");
    assert_eq!(
        played
            .iter()
            .filter(|segment| new_segments.contains(&segment.segment_id()))
            .map(|segment| (segment.segment_id(), segment.state()))
            .collect::<Vec<_>>(),
        new_segments
            .iter()
            .map(|segment_id| (*segment_id, SegmentState::Played))
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_server_whose_host_is_not_loopback_is_refused_before_anything_opens() {
    for url in [
        "http://192.0.2.1:8080/v1",
        "http://example.com:8080/v1",
        "https://10.0.0.1/v1",
        "http://[2001:db8::1]:8080/v1",
        "http://127.0.0.1.example.com/v1",
        "127.0.0.1:8080",
    ] {
        assert!(
            matches!(loopback(url), Err(LocalRefused::NotLoopback { .. })),
            "{url} is refused"
        );
        let config = LocalConfig {
            stt_dir: "no such directory".into(),
            server_url: url.to_owned(),
            server_model: "qwen".to_owned(),
            max_output_tokens: 32,
        };
        let refused = LocalModels::open(&config).expect_err("refused");
        assert!(
            matches!(refused, LocalRefused::NotLoopback { .. }),
            "{url} is refused before the checkpoint is read: {refused}"
        );
        assert!(refused.to_string().contains(url), "{refused}");
    }
    for url in [
        "http://127.0.0.1:8080/v1",
        "http://127.8.9.10:8080/v1",
        "http://[::1]:8080/v1",
        "http://localhost:8080/v1",
        "HTTP://LOCALHOST/v1",
    ] {
        assert_eq!(loopback(url).ok(), Some(()), "{url} is loopback");
    }
}

#[test]
fn a_missing_variable_is_named() {
    let all = |name: &str| Some(format!("value of {name}"));
    let config = LocalConfig::from_lookup(all).expect("every variable is set");
    assert_eq!(
        config.stt_dir,
        Path::new("value of INFERENCE_SHERPA_ONNX_STT_DIR")
    );
    assert_eq!(config.server_url, "value of INFERENCE_LLAMA_SERVER_URL");
    assert_eq!(config.server_model, "value of INFERENCE_LLAMA_SERVER_MODEL");
    for missing in [STT_DIR_VARIABLE, SERVER_URL_VARIABLE, SERVER_MODEL_VARIABLE] {
        let refused =
            LocalConfig::from_lookup(|name: &str| (name != missing).then(|| all(name))?)
                .expect_err("a variable is missing");
        assert_eq!(refused, LocalRefused::Missing(missing));
        assert!(refused.to_string().contains(missing), "{refused}");
    }
}

#[test]
fn the_local_profile_admits_the_server_s_locality_and_the_mock_profile_does_not() {
    let mocks = streaming_only_mocks(Vec::new());
    let mut declaration = Declaration::new();
    let cascade = declare(&mut declaration, &mocks).expect("the mocks run in the process");
    assert_eq!(cascade.admitted(), [Locality::SameProcess]);
    assert_eq!(
        local_voice_cascade::local::ADMITTED,
        [Locality::SameProcess, Locality::LocalWorker]
    );
}

/// The mocks of a cascade whose generator claims output streaming only, as the adapter of
/// `llama-server` does; the recogniser commits `questions`, one span each.
fn streaming_only_mocks(questions: Vec<&str>) -> Mocks {
    let cue = |after_frames, step| Cue { after_frames, step };
    let mut script = Vec::new();
    for (span, question) in questions.into_iter().enumerate() {
        let frame = 2 * span + 1;
        script.push(cue(frame, ScriptStep::Hypothesis(question.to_owned())));
        script.push(cue(frame + 1, ScriptStep::Commit { span }));
    }
    Mocks {
        recogniser: MockRecogniser::new(SttConfiguration::Incremental, ModelId::generate(), script),
        generator: MockGenerator::new(
            LlmConfiguration::StreamingOnly,
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
        reply: local_voice_cascade::local::identity,
    }
}

#[test]
fn a_generator_claiming_output_streaming_only_answers_each_turn_from_its_request() {
    let questions = ["how warm is it outside", "and will it rain"];
    let mocks = streaming_only_mocks(questions.to_vec());
    let host = FakeHost::laptop();
    let mut declaration = Declaration::new();
    let cascade = declare(&mut declaration, &mocks).expect("the cascade validates and plans");
    let run_id = RunId::generate();
    let log = RunLog::new();
    let run = cascade
        .nodes(
            run_id,
            mocks,
            TurnStore::default(),
            &log,
            &host,
            PlayoutConfig::default(),
        )
        .expect("the fake speaker opens");
    let handles = run.handles.clone();
    let _serial = serial();
    let running = Running::start(run_id, &cascade, &declaration, run.implementations);
    let mut microphone = Microphone::new(running.run_id);
    running.send(microphone.say_frames(synthesize(&[
        Stretch::Tone {
            frames: 30,
            hertz: 440,
            amplitude: 0.5,
        },
        Stretch::Silence { frames: 80 },
    ])));
    running.send(vec![microphone.stop()]);
    let finished = running.finish(&host, &handles);

    finished.assert_completed(&cascade.ids, &handles);
    let answers: Vec<String> = committed_segments(&finished.text)
        .into_iter()
        .map(|(_, text)| text)
        .collect();
    assert_eq!(answers, questions, "one segment per span, the span's text");
    let generations = handles.llm.generations();
    assert_eq!(generations.len(), 1, "one turn, one generation");
    assert_eq!(generations[0].state, GenerationState::Completed);
    assert_eq!(
        handles.tts.played().len(),
        questions.len(),
        "each answer segment was played"
    );
}
