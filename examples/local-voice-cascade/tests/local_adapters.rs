//! The local voice cascade over the local adapters: the recogniser `INFERENCE_CASCADE_STT`
//! selects, sherpa-onnx's streaming one or whisper.cpp's batch one, the generator of a loopback
//! `llama-server` and the synthesiser of `inference-sherpa-onnx-tts` over its pinned checkpoint
//! (`story:local-voice-cascade-example`, `story:stt-swap-by-configuration`,
//! `story:local-tts-adapter-under-licence-policy`; P2's
//! exit at `docs/design/inference.md` 1160, A-04, A-05, A-08 and A-10 at 1140-1146, demonstrator
//! step 6 at 1175, the measures at 1022-1023).
//!
//! The three cases that need the adapters are ignored, and the same three run under either
//! recogniser: run them once with `INFERENCE_CASCADE_STT` unset or `sherpa-onnx` and once with
//! it `whispercpp`. They read the recogniser's checkpoint from `INFERENCE_SHERPA_ONNX_STT_DIR` or
//! `INFERENCE_WHISPERCPP_STT_MODEL`, the server from `INFERENCE_LLAMA_SERVER_URL` and
//! `INFERENCE_LLAMA_SERVER_MODEL` and the synthesiser's checkpoint from
//! `INFERENCE_SHERPA_ONNX_TTS_DIR`, and with `--ignored` fail naming the variable that is missing.
//! The synthesiser speaks only in a process started on the sherpa-onnx native library built with
//! text-to-speech; the README of `inference-sherpa-onnx-tts` says how.
//! The spoken question is the sherpa-onnx checkpoint's own LibriSpeech clip `test_wavs/0.wav`
//! ("after early nightfall the yellow lamps would light up here and there the squalid quarter of
//! the brothels"), read from `INFERENCE_SHERPA_ONNX_STT_DIR` under either recogniser, followed by
//! a second of silence. No case asserts what only a streaming recogniser shows, a hypothesis
//! before the turn completes, so none is conditional on the recogniser's claims. The speaker is
//! the fake audio host's, which the test pulls at the real-time rate, 10 ms of device audio every
//! 10 ms, while the run goes:
//!
//! * offline answer: the clip, under the egress watch, yields a committed span containing
//!   "nightfall", committed assistant text, each segment Played through the sink's receipts, and
//!   a run that reaches Completed after `stop` with no failure and the watch not tripped; the
//!   cascade's effective capabilities, read from its row of the `Pipelines` view, are
//!   `output_streaming` under sherpa-onnx and none under whisper.cpp;
//! * A-08: a revision naming a Committed span the recogniser produced, injected at the input
//!   gate, is refused there (`inference.stream.Span/state/Committed/refuses/
//!   inference.stream.ReviseSpan`), and a Tentative delta of the server's generation, injected at
//!   the output gate before the generation completes, is never queued for synthesis
//!   (`inference.stream.OutputSegment/state/Tentative/refuses/inference.stream.QueueForSynthesis`);
//! * A-10: the clip said again while the first answer plays stops the speaker, the first
//!   generation is Superseded, and no frame of it is played after the stop.
//!
//! The cases that need nothing run always: a server URL whose host is not loopback is refused
//! before anything is opened, a missing variable is named, `INFERENCE_CASCADE_STT` selects the
//! recogniser and the variable of its checkpoint, the cascade planned over each recogniser's
//! adapter record has under whisper.cpp the effective capabilities it has under sherpa-onnx less
//! `incremental_input` and `output_streaming`, an `stt` node declared requiring both refuses the
//! whisper.cpp record naming the node, and the cascade answers on a mock generator that, as the
//! server's adapter, claims output streaming only, over the mock recogniser claiming both
//! capabilities and over the one claiming neither.
//!
//! One more ignored case needs the synthesiser alone, no recogniser checkpoint and no server: the
//! mock recogniser commits two spans, the mock generator answers each with its text, and the
//! synthesiser speaks both answers into the fake host's recorded speaker, under the egress watch.
//! It prints the frames the speaker took, their rate and their duration. A second such case is a
//! measurement: it supersedes an answer of three sentences while the synthesiser generates and
//! prints how long the thread that drives the run was held and when the supersede took effect.
//!
//! The watch reads every socket of the process and the server answers one request at a time, so
//! the cases that run the cascade hold one lock and never overlap.

#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::future::Future;
use std::num::{NonZeroU16, NonZeroU32};
use std::path::{Path, PathBuf};
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
use inference_core::model::{Capability, Locality, ModelId};
use inference_core::policy::Egress;
use inference_pipeline::{
    CapabilityMismatch, Declaration, Node, NodeEndpoints, NodeError, NodeFuture,
    NodeImplementation, NodeInput, NodeInstanceId, NodeOutput, PlanPipelineError, PortId,
};
use inference_runtime::RunController;
use inference_runtime::egress::EgressWatch;
use inference_runtime::run::{NodeInstanceState, RunState};
use inference_testkit::cascade::{CascadeRefused, Mocks, TurnStore};
use inference_testkit::graph::RunLog;
use inference_testkit::llm::{MockConfiguration as LlmConfiguration, MockGenerator};
use inference_testkit::stream::StreamEvent;
use inference_testkit::stt::inference_stt::Recogniser;
use inference_testkit::stt::synthetic_audio::{Stretch, synthesize};
use inference_testkit::stt::{
    Cue, MockConfiguration as SttConfiguration, MockRecogniser, ScriptStep,
};
use inference_testkit::tts::{MockConfiguration as TtsConfiguration, MockSynthesiser};
use inference_text::{SegmentId, SegmentState, SpanId, TextDelta, TranscriptRevision};
use inference_tts::Synthesiser;
use local_voice_cascade::local::{
    ADMITTED, LocalConfig, LocalModels, LocalRefused, SERVER_MODEL_VARIABLE, SERVER_URL_VARIABLE,
    STT_DIR_VARIABLE, STT_VARIABLE, SttBackend, SttConfig, TTS_DIR_VARIABLE,
    WHISPERCPP_MODEL_VARIABLE, declare_local, declare_local_adapters, loopback, server_adapter,
    synthesiser, synthesiser_adapter,
};
use local_voice_cascade::{
    AUDIO_INGRESS, CascadeHandles, CascadeIds, LocalCascade, STREAMING_STT_REQUIRES, TEXT_EGRESS,
    TRANSCRIPT_EGRESS, VoicePayload, declare, declare_requiring,
};

/// The sherpa-onnx checkpoint's LibriSpeech clip, relative to the checkpoint directory.
const CLIP: &str = "test_wavs/0.wav";

/// The capabilities the cascade's effective capabilities lose when whisper.cpp replaces
/// sherpa-onnx at `stt`.
const STREAMING: [Capability; 2] = [Capability::IncrementalInput, Capability::OutputStreaming];

/// The word of the clip the committed span is to contain.
const HEARD: &str = "nightfall";

/// The answer length the three cases on the server ask it for, in tokens. The example's own
/// default, [`local_voice_cascade::local::MAX_OUTPUT_TOKENS`], is 64; the cases ask for fewer so
/// that the answers end within the 60 s after which the runner kills a test. The cascade offers
/// one segment per delta of the server, and the synthesiser speaks a delta of one word no faster
/// than it plays: the thirteen deltas measured came to 7.13 s of audio, 0.55 s each on average
/// and 0.84 s the longest, generated in 6.94 s to 8.23 s, a ratio of 0.97 to 1.15. The record's
/// real-time factor of 0.553 is of whole sentences and does not hold here. The sum for the
/// longest of the three cases, the question said twice, from what was measured on CPU, at the
/// slower end of that ratio:
///
/// * the models load before the run: the recogniser in 4.9 s (sherpa-onnx; whisper.cpp 0.6 s)
///   and the synthesiser in 8.1 s, by their capability records: 13.0 s;
/// * two questions of at most [`CLIP_SECONDS`] are heard at a real-time factor of 0.202
///   (whisper.cpp; sherpa-onnx 0.131): 4.0 s;
/// * the first answer plays until its first segment is acknowledged: a delta of at most 0.84 s
///   is generated in 0.97 s and played: 1.8 s;
/// * the first answer is superseded, but not cut at the synthesiser: every delta committed
///   before the supersede is generated to its end before the supersede is read. With a server
///   faster than the synthesiser that is every other delta of the answer, 23. About 3 of them
///   are generated while the second question is heard, which is counted above; the other 20, of
///   0.55 s each, take 12.7 s;
/// * the second answer plays whole: 24 deltas are 13.2 s of audio, generated in 15.2 s, and the
///   last of them is played after it is generated, at most 0.84 s: 16.0 s;
/// * that is 47.5 s, which leaves 12.5 s, in which the server gives the first token of each
///   answer; its later tokens come while the synthesiser works. A slower server commits fewer
///   deltas before the supersede, which shortens the fourth term.
///
/// The sum rests on the average delta of that one measurement. Were every delta as long as the
/// longest measured, 0.84 s, the fourth term would be 20.4 s and the fifth 24.1 s, and the sum
/// 63.3 s: an answer of 24 deltas that are all long words does not fit, and the runner would
/// end the case.
///
/// The other two cases say the question once and are shorter by a question and an answer. With
/// 64 tokens the two answers alone are 70 s of audio at 0.55 s a delta.
const ANSWER_TOKENS: u64 = 24;

/// The longest clip [`Microphone::say`] takes, in seconds: the length the sum at
/// [`ANSWER_TOKENS`] counts for a question.
const CLIP_SECONDS: usize = 10;

/// How long one wait of a case may last before the case gives up naming what it waited for: 60 s,
/// after which the runner kills the test, less the 13.0 s in which the models load and a margin.
const PATIENCE: Duration = Duration::from_secs(45);

/// Device frames of the fake laptop's speaker, at 48 kHz: 10 ms, half of one 20 ms frame of the
/// synthesiser, or one frame of the mock synthesiser, 160 samples at 16 kHz.
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

/// The interval of the [`Ticker`].
const TICK: Duration = Duration::from_millis(10);

/// How long the thread that opened it has been ready to run without a processor, as Linux counts
/// it in the second field of `/proc/thread-self/schedstat`. The file stays open and is read again
/// from its start. Where there is no such file every reading is zero.
struct ReadyWait(Option<std::fs::File>);

impl ReadyWait {
    fn of_this_thread() -> Self {
        Self(std::fs::File::open("/proc/thread-self/schedstat").ok())
    }

    fn so_far(&mut self) -> Duration {
        use std::io::{Read, Seek, SeekFrom};
        let mut text = String::new();
        let read = self.0.as_mut().is_some_and(|file| {
            file.seek(SeekFrom::Start(0)).is_ok() && file.read_to_string(&mut text).is_ok()
        });
        text.split_whitespace()
            .nth(1)
            .filter(|_| read)
            .and_then(|nanoseconds| nanoseconds.parse().ok())
            .map_or(Duration::ZERO, Duration::from_nanos)
    }
}

/// A ticker beside a run on the thread that drives it. The run commits to no executor, so its
/// runtime is the loop that polls its drive future: the ticker is a second task of that loop,
/// due every [`TICK`], and it can tick only between two polls of the run. A gap between two ticks
/// far over [`TICK`] is a poll of the run that held the thread that long. Each gap is counted
/// less the time the thread was ready to run without a processor ([`ReadyWait`]), which is the
/// host's and not the run's. A node that runs its exchange on a thread of its own, as the speech
/// node does, is not seen by the ticker.
#[derive(Default)]
struct Ticker {
    /// When each tick came and the gap since the tick before it.
    ticks: Mutex<Vec<(Instant, Duration)>>,
}

impl Ticker {
    /// Polls `future` on this thread until it completes, ticking beside it.
    fn block_on<F: Future>(&self, future: F) -> F::Output {
        let waker = Waker::from(Arc::new(Unpark(thread::current())));
        let mut future = pin!(future);
        let mut ready = ReadyWait::of_this_thread();
        let mut last = (Instant::now(), ready.so_far());
        loop {
            let now = Instant::now();
            if now.duration_since(last.0) >= TICK {
                let waited = ready.so_far();
                let gap = now
                    .duration_since(last.0)
                    .saturating_sub(waited.saturating_sub(last.1));
                self.ticks
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push((now, gap));
                last = (now, waited);
            }
            if let Poll::Ready(output) = future.as_mut().poll(&mut Context::from_waker(&waker)) {
                return output;
            }
            thread::park_timeout(TICK.saturating_sub(last.0.elapsed()));
        }
    }

    /// The ticks from `from` on that came before `until`.
    fn between(&self, from: Instant, until: Instant) -> Vec<(Instant, Duration)> {
        self.ticks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(at, _)| *at >= from && *at < until)
            .copied()
            .collect()
    }
}

/// The configuration the variables name, with answers of [`ANSWER_TOKENS`]; a missing variable
/// fails the case, naming it.
fn configured() -> LocalConfig {
    let mut config = LocalConfig::from_env().unwrap_or_else(|refused| panic!("{refused}"));
    config.max_output_tokens = ANSWER_TOKENS;
    println!(
        "recogniser: {}; answers of at most {ANSWER_TOKENS} tokens",
        config.stt.backend()
    );
    config
}

/// The clip in the sherpa-onnx checkpoint directory `INFERENCE_SHERPA_ONNX_STT_DIR` names, under
/// either recogniser; a missing variable fails the case, naming it.
fn clip() -> PathBuf {
    std::env::var_os(STT_DIR_VARIABLE)
        .map(|dir| PathBuf::from(dir).join(CLIP))
        .unwrap_or_else(|| panic!("{STT_DIR_VARIABLE} is not set: the clip is its {CLIP}"))
}

/// The effective capabilities of `cascade`, read from its row of the `Pipelines` view.
fn effective(declaration: &Declaration, cascade: &LocalCascade) -> Vec<Capability> {
    declaration
        .pipeline(cascade.pipeline_id)
        .and_then(|pipeline| pipeline.effective_capabilities())
        .expect("the cascade is planned")
        .to_vec()
}

/// The effective capabilities of the local cascade with `backend` bound at `stt`: what the
/// recogniser, the server's generator and the synthesiser all claim.
fn expected_effective(backend: SttBackend) -> Vec<Capability> {
    match backend {
        SttBackend::SherpaOnnx => vec![Capability::OutputStreaming],
        SttBackend::WhisperCpp => Vec::new(),
    }
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
        assert!(
            data.len() <= CLIP_SECONDS * 16_000 * 2,
            "the clip is at most {CLIP_SECONDS} s: {} samples at 16 kHz",
            data.len() / 2
        );
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
                    "inference.audio.AudioChunk",
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
                    "inference.audio.AudioChunk",
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
        implementations: HashMap<NodeId, Box<dyn NodeImplementation<VoicePayload>>>,
    ) -> Self {
        Self::start_beside(run_id, cascade, declaration, implementations, None)
    }

    /// [`Running::start`], the thread that drives the run ticking `ticker` beside it.
    fn start_beside(
        run_id: RunId,
        cascade: &LocalCascade,
        declaration: &Declaration,
        mut implementations: HashMap<NodeId, Box<dyn NodeImplementation<VoicePayload>>>,
        ticker: Option<Arc<Ticker>>,
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
            let result = match ticker {
                Some(ticker) => ticker.block_on(controller.drive()),
                None => block_on(controller.drive()),
            };
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
#[ignore = "needs the clip in INFERENCE_SHERPA_ONNX_STT_DIR, the checkpoint of the recogniser INFERENCE_CASCADE_STT selects, the synthesiser's checkpoint in INFERENCE_SHERPA_ONNX_TTS_DIR in a process started on the sherpa-onnx library built with text-to-speech, and a loopback llama-server at INFERENCE_LLAMA_SERVER_URL serving INFERENCE_LLAMA_SERVER_MODEL at 3 tokens a second or more; asks for answers of 24 tokens so that the run ends within the 60 s of the runner"]
fn the_clip_is_answered_offline_and_every_segment_is_played_through_receipts() {
    let _serial = serial();
    let config = configured();
    let models = LocalModels::open(&config).unwrap_or_else(|refused| panic!("{refused}"));
    let speaking = synthesiser(&tts_dir()).unwrap_or_else(|refused| panic!("{refused}"));
    assert_eq!(models.recogniser.backend(), config.stt.backend());
    let host = FakeHost::laptop();
    let mut declaration = Declaration::new();
    let cascade = declare_local(&mut declaration, &models, &speaking)
        .expect("the cascade validates and plans");
    let effective = effective(&declaration, &cascade);
    println!("effective capabilities: {effective:?}");
    assert_eq!(effective, expected_effective(config.stt.backend()));
    let run_id = RunId::generate();
    let turns = TurnStore::default();
    let log = RunLog::new();
    let run = cascade
        .local_nodes(
            run_id,
            models,
            speaking,
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
    running.send(microphone.say(&clip()));
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
#[ignore = "needs the clip in INFERENCE_SHERPA_ONNX_STT_DIR, the checkpoint of the recogniser INFERENCE_CASCADE_STT selects, the synthesiser's checkpoint in INFERENCE_SHERPA_ONNX_TTS_DIR in a process started on the sherpa-onnx library built with text-to-speech, and a loopback llama-server at INFERENCE_LLAMA_SERVER_URL serving INFERENCE_LLAMA_SERVER_MODEL at 3 tokens a second or more; asks for answers of 24 tokens so that the run ends within the 60 s of the runner"]
fn a_revised_committed_span_and_a_tentative_delta_of_the_server_are_stopped_at_the_gates() {
    let _serial = serial();
    let config = configured();
    let models = LocalModels::open(&config).unwrap_or_else(|refused| panic!("{refused}"));
    let speaking = synthesiser(&tts_dir()).unwrap_or_else(|refused| panic!("{refused}"));
    let host = FakeHost::laptop();
    let mut declaration = Declaration::new();
    let cascade = declare_local(&mut declaration, &models, &speaking)
        .expect("the cascade validates and plans");
    let ids = cascade.ids;
    let run_id = RunId::generate();
    let log = RunLog::new();
    let mut run = cascade
        .local_nodes(
            run_id,
            models,
            speaking,
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
    running.send(microphone.say(&clip()));
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
#[ignore = "needs the clip in INFERENCE_SHERPA_ONNX_STT_DIR, the checkpoint of the recogniser INFERENCE_CASCADE_STT selects, the synthesiser's checkpoint in INFERENCE_SHERPA_ONNX_TTS_DIR in a process started on the sherpa-onnx library built with text-to-speech, and a loopback llama-server at INFERENCE_LLAMA_SERVER_URL serving INFERENCE_LLAMA_SERVER_MODEL at 3 tokens a second or more; asks for answers of 24 tokens so that the run ends within the 60 s of the runner"]
#[allow(clippy::too_many_lines)]
fn the_clip_said_again_while_the_answer_plays_stops_it_and_supersedes_its_generation() {
    let _serial = serial();
    let config = configured();
    let models = LocalModels::open(&config).unwrap_or_else(|refused| panic!("{refused}"));
    let speaking = synthesiser(&tts_dir()).unwrap_or_else(|refused| panic!("{refused}"));
    let host = FakeHost::laptop();
    let mut declaration = Declaration::new();
    let cascade = declare_local(&mut declaration, &models, &speaking)
        .expect("the cascade validates and plans");
    let run_id = RunId::generate();
    let turns = TurnStore::default();
    let log = RunLog::new();
    let run = cascade
        .local_nodes(
            run_id,
            models,
            speaking,
            turns.clone(),
            &log,
            &host,
            &Default::default(),
            PlayoutConfig::default(),
        )
        .expect("the fake speaker opens");
    let handles = run.handles.clone();
    let running = Running::start(run_id, &cascade, &declaration, run.implementations);
    let clip = clip();
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
            stt: SttConfig::SherpaOnnx {
                dir: "no such directory".into(),
            },
            server_url: url.to_owned(),
            server_model: "qwen".to_owned(),
            max_output_tokens: 32,
        };
        let refused = LocalModels::open(&config).expect_err("refused");
        assert!(
            matches!(refused, LocalRefused::NotLoopback { .. }),
            "{url} is refused before the checkpoint is read: {refused}"
        );
        let origin = url.split("/v1").next().unwrap_or(url);
        assert!(
            refused.to_string().contains(origin),
            "story:llama-server-loopback-only: the refusal names the URL's scheme, host and port, \
             not its path: {refused}"
        );
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
fn a_refused_server_url_is_printed_without_its_userinfo() {
    let url = "http://user:pass@10.0.0.5:8080/v1";
    let config = LocalConfig {
        stt: SttConfig::SherpaOnnx {
            dir: "no such directory".into(),
        },
        server_url: url.to_owned(),
        server_model: "qwen".to_owned(),
        max_output_tokens: 32,
    };
    for refused in [
        loopback(url).expect_err("refused"),
        LocalModels::open(&config).expect_err("refused"),
    ] {
        assert!(
            matches!(refused, LocalRefused::NotLoopback { .. }),
            "{refused:?}"
        );
        for text in [refused.to_string(), format!("{refused:?}")] {
            assert!(!text.contains("user"), "the user is not printed: {text}");
            assert!(
                !text.contains("pass"),
                "the password is not printed: {text}"
            );
            assert!(text.contains("http://10.0.0.5:8080"), "{text}");
        }
    }
}

#[test]
fn a_missing_variable_is_named() {
    let all = |name: &str| Some(format!("value of {name}"));
    let unset = |name: &str| (name != STT_VARIABLE).then(|| all(name))?;
    let config = LocalConfig::from_lookup(unset).expect("every variable is set");
    assert_eq!(
        config.stt,
        SttConfig::SherpaOnnx {
            dir: PathBuf::from("value of INFERENCE_SHERPA_ONNX_STT_DIR")
        }
    );
    assert_eq!(config.server_url, "value of INFERENCE_LLAMA_SERVER_URL");
    assert_eq!(config.server_model, "value of INFERENCE_LLAMA_SERVER_MODEL");
    for missing in [STT_DIR_VARIABLE, SERVER_URL_VARIABLE, SERVER_MODEL_VARIABLE] {
        let refused =
            LocalConfig::from_lookup(|name: &str| (name != missing).then(|| unset(name))?)
                .expect_err("a variable is missing");
        assert_eq!(refused, LocalRefused::Missing(missing));
        assert!(refused.to_string().contains(missing), "{refused}");
    }
}

#[test]
fn the_configuration_of_the_listening_models_does_not_ask_for_the_synthesisers_checkpoint() {
    let asked = std::cell::RefCell::new(Vec::new());
    let config = LocalConfig::from_lookup(|name: &str| {
        asked.borrow_mut().push(name.to_owned());
        (name != STT_VARIABLE && name != TTS_DIR_VARIABLE).then(|| format!("value of {name}"))
    })
    .expect("the recogniser's and the server's variables are set");
    assert_eq!(config.server_url, "value of INFERENCE_LLAMA_SERVER_URL");
    assert!(
        !asked.borrow().iter().any(|name| name == TTS_DIR_VARIABLE),
        "{asked:?}"
    );
}

/// `lookup` answering `value` for [`STT_VARIABLE`] and every other variable but `unset`.
fn selecting(value: &str, unset: &str) -> impl Fn(&str) -> Option<String> {
    let (value, unset) = (value.to_owned(), unset.to_owned());
    move |name: &str| {
        if name == STT_VARIABLE {
            Some(value.clone())
        } else {
            (name != unset).then(|| format!("value of {name}"))
        }
    }
}

#[test]
fn the_stt_variable_selects_the_recogniser_and_the_variable_of_its_checkpoint() {
    let sherpa = LocalConfig::from_lookup(selecting("sherpa-onnx", WHISPERCPP_MODEL_VARIABLE))
        .expect("sherpa-onnx needs no whisper.cpp checkpoint");
    assert_eq!(
        sherpa.stt,
        SttConfig::SherpaOnnx {
            dir: PathBuf::from("value of INFERENCE_SHERPA_ONNX_STT_DIR")
        }
    );
    let whisper = LocalConfig::from_lookup(selecting("whispercpp", STT_DIR_VARIABLE))
        .expect("whisper.cpp needs no sherpa-onnx checkpoint");
    assert_eq!(
        whisper.stt,
        SttConfig::WhisperCpp {
            model: PathBuf::from("value of INFERENCE_WHISPERCPP_STT_MODEL")
        }
    );
    assert_eq!(
        (whisper.server_url, whisper.server_model),
        (sherpa.server_url, sherpa.server_model),
        "nothing else changes with the recogniser"
    );
    for backend in SttBackend::ALL {
        let refused = LocalConfig::from_lookup(selecting(backend.name(), backend.variable()))
            .expect_err("the checkpoint variable is missing");
        assert_eq!(refused, LocalRefused::Missing(backend.variable()));
        assert_eq!(backend.name().parse::<SttBackend>(), Ok(backend));
    }
    let refused = LocalConfig::from_lookup(selecting("whisper", ""))
        .expect_err("whisper names no recogniser");
    assert_eq!(
        refused,
        LocalRefused::UnknownBackend {
            value: "whisper".to_owned()
        }
    );
    assert!(
        refused.to_string().contains(STT_VARIABLE) && refused.to_string().contains("whispercpp"),
        "{refused}"
    );
}

/// The cascade planned with the adapter records of the configuration `lookup` answers: the
/// recogniser's, the server's and the synthesiser's; no checkpoint is opened and no request sent.
fn planned_records(lookup: impl Fn(&str) -> Option<String>) -> (SttBackend, Vec<Capability>) {
    let config = LocalConfig::from_lookup(lookup).expect("every variable is set");
    let backend = config.stt.backend();
    let mut declaration = Declaration::new();
    let cascade = declare_local_adapters(
        &mut declaration,
        [
            backend.adapter(),
            server_adapter(&config.server()).expect("the server's adapter is declared"),
            synthesiser_adapter(),
        ],
    )
    .unwrap_or_else(|refused| panic!("the cascade plans over {backend}: {refused}"));
    (backend, effective(&declaration, &cascade))
}

#[test]
fn under_whispercpp_the_effective_capabilities_lose_incremental_input_and_output_streaming() {
    let localhost = |name: &str| match name {
        SERVER_URL_VARIABLE => Some("http://127.0.0.1:8080/v1".to_owned()),
        _ => Some(format!("value of {name}")),
    };
    let (sherpa, streaming) = planned_records(|name: &str| match name {
        STT_VARIABLE => Some("sherpa-onnx".to_owned()),
        _ => localhost(name),
    });
    let (whisper, batch) = planned_records(|name: &str| match name {
        STT_VARIABLE => Some("whispercpp".to_owned()),
        _ => localhost(name),
    });
    println!("effective capabilities, {sherpa}: {streaming:?}");
    println!("effective capabilities, {whisper}: {batch:?}");
    assert_eq!(
        (sherpa, whisper),
        (SttBackend::SherpaOnnx, SttBackend::WhisperCpp)
    );
    assert_eq!(
        batch,
        streaming
            .iter()
            .copied()
            .filter(|capability| !STREAMING.contains(capability))
            .collect::<Vec<_>>(),
        "whisper.cpp's set is sherpa-onnx's less incremental_input and output_streaming"
    );
    assert!(
        streaming
            .iter()
            .any(|capability| STREAMING.contains(capability)),
        "the difference shows in the view: {streaming:?}"
    );
    assert_eq!(streaming, expected_effective(sherpa));
    assert_eq!(batch, expected_effective(whisper));
}

#[test]
fn an_stt_node_requiring_streaming_refuses_the_whispercpp_recogniser_naming_the_node() {
    let server = server_adapter(
        &LocalConfig {
            stt: SttConfig::WhisperCpp {
                model: PathBuf::new(),
            },
            server_url: "http://127.0.0.1:8080/v1".to_owned(),
            server_model: "qwen".to_owned(),
            max_output_tokens: 32,
        }
        .server(),
    )
    .expect("the server's adapter is declared");
    let strict = |backend: SttBackend, declaration: &mut Declaration| {
        declare_requiring(
            declaration,
            [backend.adapter(), server.clone(), synthesiser_adapter()],
            ADMITTED.to_vec(),
            &STREAMING_STT_REQUIRES,
        )
    };
    strict(SttBackend::SherpaOnnx, &mut Declaration::new())
        .expect("sherpa-onnx claims what the stricter node requires");
    let mut declaration = Declaration::new();
    let refused =
        strict(SttBackend::WhisperCpp, &mut declaration).expect_err("whisper.cpp claims neither");
    let stt = declaration
        .nodes()
        .find(|node| node.name() == "stt")
        .expect("the stt node is declared")
        .node_id();
    assert!(
        matches!(
            refused,
            CascadeRefused::Unplannable(PlanPipelineError::Mismatch(CapabilityMismatch {
                node_id: Some(node_id),
                capability,
            })) if node_id == stt && STREAMING.contains(&capability)
        ),
        "the stt node and a capability it requires and whisper.cpp lacks: {refused:?}"
    );
}

#[test]
fn the_local_profile_admits_the_server_s_locality_and_the_mock_profile_does_not() {
    let mocks = streaming_only_mocks(SttConfiguration::Incremental, Vec::new());
    let mut declaration = Declaration::new();
    let cascade = declare(&mut declaration, &mocks).expect("the mocks run in the process");
    assert_eq!(cascade.admitted(), [Locality::SameProcess]);
    assert_eq!(
        local_voice_cascade::local::ADMITTED,
        [Locality::SameProcess, Locality::LocalWorker]
    );
}

/// The mocks of a cascade whose generator claims output streaming only, as the adapter of
/// `llama-server` does; the recogniser, claiming what `recogniser` claims, commits `questions`,
/// one span each.
fn streaming_only_mocks(recogniser: SttConfiguration, questions: Vec<&str>) -> Mocks {
    let cue = |after_frames, step| Cue { after_frames, step };
    let mut script = Vec::new();
    for (span, question) in questions.into_iter().enumerate() {
        let frame = 2 * span + 1;
        script.push(cue(frame, ScriptStep::Hypothesis(question.to_owned())));
        script.push(cue(frame + 1, ScriptStep::Commit { span }));
    }
    Mocks {
        recogniser: MockRecogniser::new(recogniser, ModelId::generate(), script),
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
    for recogniser in SttConfiguration::ALL {
        answers_each_turn_over(recogniser);
    }
}

/// The cascade over the recogniser claiming what `recogniser` claims, both capabilities or
/// neither, answers a turn of two committed spans with one segment each.
fn answers_each_turn_over(recogniser: SttConfiguration) {
    let questions = ["how warm is it outside", "and will it rain"];
    let mocks = streaming_only_mocks(recogniser, questions.to_vec());
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
    assert_eq!(
        answers, questions,
        "one segment per span, the span's text, over {recogniser:?}"
    );
    let generations = handles.llm.generations();
    assert_eq!(generations.len(), 1, "one turn, one generation");
    assert_eq!(generations[0].state, GenerationState::Completed);
    assert_eq!(
        handles.tts.played().len(),
        questions.len(),
        "each answer segment was played"
    );
}

#[test]
fn the_synthesiser_refuses_a_directory_without_its_checkpoint_naming_the_file() {
    let dir = Path::new("no such directory");
    let refused = synthesiser(dir).expect_err("the directory holds no checkpoint");
    assert!(
        matches!(
            &refused,
            inference_sherpa_onnx_tts::OpenRefused::FileMissing { path, .. }
                if path.starts_with(dir)
        ),
        "{refused:?}"
    );
    assert!(
        refused.to_string().contains("no such directory"),
        "{refused}"
    );
}

#[test]
fn the_record_of_the_synthesiser_claims_what_the_tts_node_requires_in_the_process() {
    let record = synthesiser_adapter();
    assert_eq!(record.capabilities(), STREAMING);
    assert_eq!(record.locality(), Locality::SameProcess);
}

/// The directory `INFERENCE_SHERPA_ONNX_TTS_DIR` names; a missing variable fails the case,
/// naming it.
fn tts_dir() -> PathBuf {
    std::env::var_os(TTS_DIR_VARIABLE)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("{TTS_DIR_VARIABLE} is not set"))
}

#[test]
#[ignore = "needs the synthesiser's checkpoint in INFERENCE_SHERPA_ONNX_TTS_DIR in a process started on the sherpa-onnx library built with text-to-speech"]
fn a_reply_is_spoken_through_the_synthesiser_into_the_recorded_speaker_with_no_socket_opened() {
    let questions = ["how warm is it outside", "and will it rain"];
    let speaking = synthesiser(&tts_dir()).unwrap_or_else(|refused| panic!("{refused}"));
    let format = speaking.format();
    let mocks = streaming_only_mocks(SttConfiguration::Incremental, questions.to_vec());
    let host = FakeHost::laptop();
    let mut declaration = Declaration::new();
    let cascade = declare_local_adapters(
        &mut declaration,
        [
            mocks.recogniser.adapter().clone(),
            mocks.generator.adapter().clone(),
            speaking.adapter().clone(),
        ],
    )
    .expect("the cascade validates and plans over the synthesiser's adapter");
    let run_id = RunId::generate();
    let log = RunLog::new();
    let run = cascade
        .nodes_with(
            run_id,
            mocks,
            move |_mock| speaking,
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
    assert_eq!(
        handles
            .tts
            .played()
            .iter()
            .map(|segment| segment.state())
            .collect::<Vec<_>>(),
        [SegmentState::Played, SegmentState::Played],
        "each answer segment was played"
    );
    let receipts = handles.sink.receipts();
    assert_eq!(receipts.len(), questions.len());

    let frames = host.played_frames();
    assert!(
        frames.iter().all(|frame| frame.format == format),
        "every frame the speaker took is the synthesiser's {format:?}"
    );
    let samples: u64 = frames
        .iter()
        .map(|frame| u64::from(frame.sample_count))
        .sum();
    let rate = u64::from(format.sample_rate.get());
    let peak = frames
        .iter()
        .flat_map(|frame| frame.data.as_chunks::<2>().0.iter())
        .map(|sample| i16::from_le_bytes(*sample).unsigned_abs())
        .max()
        .unwrap_or_default();
    println!(
        "spoken: {answers:?}; the speaker took {} frames, {samples} samples of {:?} at {rate} Hz, \
         {} channel, {} ms; peak {peak} of 32767; receipts of {:?} frames; sockets outside \
         loopback: {:?} over {} polls of the egress watch",
        frames.len(),
        format.encoding,
        format.channels,
        samples * 1_000 / rate,
        receipts
            .iter()
            .map(|receipt| receipt.frames)
            .collect::<Vec<_>>(),
        finished.tripped,
        finished.polls,
    );
    assert_eq!(
        receipts.iter().map(|receipt| receipt.frames).sum::<usize>(),
        frames.len(),
        "every frame the speaker took is in a receipt"
    );
    assert!(
        samples > rate,
        "two phrases last longer than a second: {samples} samples"
    );
    assert!(peak > 1_000, "the speaker took speech, not silence: {peak}");
}

/// What the user says in each utterance of the measurement below: three spans in one turn. The
/// scripted generator answers each span with the span's text, so the answer is three segments of
/// one sentence each.
const SENTENCES: [&str; 3] = [
    "Tell me how warm it is outside this afternoon.",
    "And tell me whether it will rain before the evening.",
    "And say when the sun goes down behind the hills.",
];

/// The fake speaker played at the real-time rate, and when the frames the sink pushed to it or
/// dropped as stale were seen to change, read every millisecond.
struct Playing<'a> {
    host: &'a FakeHost,
    handles: &'a CascadeHandles,
    pulled: Instant,
    seen: (u64, usize),
    /// When a change was seen, and the frames pushed and the stale frames dropped by then.
    arrivals: Vec<(Instant, u64, usize)>,
}

impl<'a> Playing<'a> {
    fn new(host: &'a FakeHost, handles: &'a CascadeHandles) -> Self {
        Self {
            host,
            handles,
            pulled: Instant::now(),
            seen: (0, 0),
            arrivals: Vec::new(),
        }
    }

    /// Plays until `done` holds; fails the case naming `what` after [`PATIENCE`].
    fn until(&mut self, what: &str, done: impl Fn() -> bool) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let now = (
                self.handles.sink.pushed(),
                self.handles.sink.stale_frames().len(),
            );
            if now != self.seen {
                self.seen = now;
                self.arrivals.push((Instant::now(), now.0, now.1));
            }
            if done() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{what}: llm {:?}, tts {:?}, sink pushed {}",
                self.handles.llm.failure(),
                self.handles.tts.failure(),
                self.handles.sink.pushed()
            );
            if self.pulled.elapsed() >= DEVICE_TICK {
                let _ = self.host.pull(DEVICE_FRAME);
                self.pulled = Instant::now();
            }
            thread::sleep(Duration::from_millis(1));
        }
    }
}

/// Milliseconds from `from` to `to`, negative when `to` came first.
fn millis_between(from: Instant, to: Instant) -> i128 {
    let forward = i128::try_from(to.saturating_duration_since(from).as_millis()).unwrap_or(0);
    let backward = i128::try_from(from.saturating_duration_since(to).as_millis()).unwrap_or(0);
    forward - backward
}

/// A measurement, not a rule: what the thread that drives the run and a supersede see while the
/// synthesiser of `inference-sherpa-onnx-tts` generates. The scripted recogniser commits three spans, the scripted
/// generator answers them with three segments of one sentence, and the same utterance is said
/// again as soon as the three segments are committed, which supersedes the answer. The case
/// prints
///
/// * the time from the supersede to the last frame of the superseded generation that reached
///   the sink,
/// * the time from the second utterance to the supersede,
/// * the largest gap of a 10 ms ticker on the thread that drives the run ([`Ticker`]), over the
///   whole run and while the supersede was pending,
///
/// and asserts only that the run ends as a superseded and a completed generation.
///
/// What two runs at adoption printed, on a shared machine: the ticker's largest gap was 10.7 ms
/// over 1145 ticks and 12.0 ms over 1491, so the thread that drives the run is not held. The speech node of `inference-pipeline` runs its
/// exchange on a thread of its own and, after it offers a committed segment, reads the synthesis
/// with the blocking iterator until that segment is generated; it takes the next event on its
/// text port only then. The generation was superseded within 3 ms of the commit of its three
/// segments, and the speaker stopped then; the node still generated all three sentences, 399
/// frames that the speaker dropped as stale, the last of them 2275 ms and 5671 ms after the
/// supersede, and the first frame of the next answer came 760 ms and 1072 ms after that. The
/// limitations page states this limit.
#[test]
#[ignore = "a measurement: needs the synthesiser's checkpoint in INFERENCE_SHERPA_ONNX_TTS_DIR in a process started on the sherpa-onnx library built with text-to-speech"]
#[allow(clippy::too_many_lines)]
fn three_committed_segments_superseded_while_the_synthesiser_generates_measured() {
    let speaking = synthesiser(&tts_dir()).unwrap_or_else(|refused| panic!("{refused}"));
    let mocks = streaming_only_mocks(SttConfiguration::Incremental, SENTENCES.to_vec());
    let host = FakeHost::laptop();
    let mut declaration = Declaration::new();
    let cascade = declare_local_adapters(
        &mut declaration,
        [
            mocks.recogniser.adapter().clone(),
            mocks.generator.adapter().clone(),
            speaking.adapter().clone(),
        ],
    )
    .expect("the cascade validates and plans over the synthesiser's adapter");
    let run_id = RunId::generate();
    let log = RunLog::new();
    let run = cascade
        .nodes_with(
            run_id,
            mocks,
            move |_mock| speaking,
            TurnStore::default(),
            &log,
            &host,
            PlayoutConfig::default(),
        )
        .expect("the fake speaker opens");
    let handles = run.handles.clone();
    let _serial = serial();
    let ticker = Arc::new(Ticker::default());
    let running = Running::start_beside(
        run_id,
        &cascade,
        &declaration,
        run.implementations,
        Some(Arc::clone(&ticker)),
    );
    let mut microphone = Microphone::new(running.run_id);
    let utterance = || {
        synthesize(&[
            Stretch::Tone {
                frames: 30,
                hertz: 440,
                amplitude: 0.5,
            },
            Stretch::Silence { frames: 80 },
        ])
    };
    let mut playing = Playing::new(&host, &handles);

    let started = Instant::now();
    running.send(microphone.say_frames(utterance()));
    playing.until(
        "the three segments of the first answer are committed",
        || {
            handles
                .llm
                .generations()
                .first()
                .is_some_and(|generation| generation.segments.len() == SENTENCES.len())
        },
    );
    let committed = Instant::now();
    running.send(microphone.say_frames(utterance()));
    let said = Instant::now();
    playing.until(
        "the first answer is superseded and the speaker stopped",
        || !handles.llm.superseded().is_empty() && !handles.sink.interruptions().is_empty(),
    );
    running.send(vec![microphone.stop()]);
    playing.until("the run ends", || running.drive.is_finished());
    let ended = Instant::now();
    let arrivals = std::mem::take(&mut playing.arrivals);
    let finished = running.finish(&host, &handles);

    finished.assert_completed(&cascade.ids, &handles);
    let generations = handles.llm.generations();
    assert_eq!(
        generations
            .iter()
            .map(|generation| generation.state)
            .collect::<Vec<_>>(),
        [GenerationState::Superseded, GenerationState::Completed],
        "the first answer is superseded and the second completed"
    );
    let old: Vec<SegmentId> = generations[0]
        .segments
        .iter()
        .map(|segment| segment.segment_id())
        .collect();
    let superseded = handles.llm.superseded()[0].delivery;
    let interruption = handles.sink.interruptions()[0];

    let first_frames = handles.sink.first_frames();
    let old_first_frames: Vec<i128> = first_frames
        .iter()
        .filter(|frame| old.contains(&frame.segment_id))
        .map(|frame| millis_between(started, frame.at))
        .collect();
    let new_first_frames: Vec<i128> = first_frames
        .iter()
        .filter(|frame| !old.contains(&frame.segment_id))
        .map(|frame| millis_between(started, frame.at))
        .collect();
    // A frame of the superseded generation is one pushed before the speaker stopped, or one
    // dropped as stale after it.
    let last_old_frame = arrivals
        .iter()
        .scan((0, 0), |seen, (at, pushed, stale)| {
            let old_frame = *stale > seen.1 || (*pushed > seen.0 && *at < interruption.at);
            *seen = (*pushed, *stale);
            Some(old_frame.then_some(*at))
        })
        .flatten()
        .last();
    let largest = |from: Instant, until: Instant| {
        ticker
            .between(from, until)
            .into_iter()
            .max_by_key(|(_, gap)| *gap)
            .map(|(at, gap)| (gap, millis_between(started, at)))
    };
    let over_50: Vec<(i128, u128)> = ticker
        .between(started, ended)
        .into_iter()
        .filter(|(_, gap)| *gap > Duration::from_millis(50))
        .map(|(at, gap)| (millis_between(started, at), gap.as_millis()))
        .collect();

    println!(
        "measured, in ms after the first utterance was said: the three segments committed at {}; \
         the second utterance said at {}; the generation superseded at {}; the speaker stopped \
         at {}; the last frame of the superseded generation at the sink at {:?}; the run ended \
         at {}",
        millis_between(started, committed),
        millis_between(started, said),
        millis_between(started, superseded),
        millis_between(started, interruption.at),
        last_old_frame.map(|at| millis_between(started, at)),
        millis_between(started, ended),
    );
    println!(
        "from the supersede to the last frame of the superseded generation: {:?} ms; from the \
         second utterance to the supersede: {} ms; segments of the superseded answer: {} \
         discarded, {} played, {} stale frames dropped; first frames of its segments at {:?}, of \
         the second answer's at {:?}",
        last_old_frame.map(|at| millis_between(superseded, at)),
        millis_between(said, superseded),
        handles.tts.discarded().len(),
        handles
            .tts
            .played()
            .iter()
            .filter(|segment| old.contains(&segment.segment_id()))
            .count(),
        handles.sink.stale_frames().len(),
        old_first_frames,
        new_first_frames,
    );
    println!(
        "the 10 ms ticker on the thread that drives the run: {} ticks; largest gap {:?} (gap, \
         ms after the start); while the supersede was pending {:?}; after the supersede {:?}; \
         gaps over 50 ms as (ms after the start, ms): {:?}",
        ticker.between(started, ended).len(),
        largest(started, ended),
        largest(said, superseded),
        largest(superseded, ended),
        over_50,
    );
}
