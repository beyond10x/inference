//! Adversary cases for barge-in in the local voice cascade (`story:cascade-barge-in`), on the
//! testkit mocks and the fake audio host.
//!
//! * A barge-in before the first frame of the answer reached the sink: the synthesiser computes
//!   the answer's first segment synchronously, inside `offer_segment`, as a backend that renders
//!   a whole segment before it returns does, and the user speaks again meanwhile. Section 23.3
//!   test 5 of `docs/design/inference.md` (1128): a cancelled generation produces no newly played
//!   stale audio; the story: from the `TurnOpened` the sink drops every frame of an older
//!   generation.
//! * A turn opened after the answer of the turn before was played to the end: the story's
//!   trigger is a `TurnOpened` "while output of an earlier turn is not all Played", and a
//!   generation whose output was all played is no interruption's subject.

use std::collections::HashMap;
use std::future::Future;
use std::num::{NonZeroU16, NonZeroU32};
use std::pin::pin;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant};

use inference_audio::{AudioEncoding, AudioFormat, AudioFrame};
use inference_audio_device::PlayoutConfig;
use inference_audio_device::fake::FakeHost;
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::generation::GenerationState;
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::{Adapter, ModelId};
use inference_pipeline::{Declaration, Node};
use inference_runtime::RunController;
use inference_runtime::run::RunState;
use inference_testkit::cascade::{Mocks, TurnStore};
use inference_testkit::graph::RunLog;
use inference_testkit::llm::{MockConfiguration as LlmConfiguration, MockGenerator};
use inference_testkit::stream::StreamEvent;
use inference_testkit::stt::synthetic_audio::{Stretch, synthesize};
use inference_testkit::stt::{
    Cue, MockConfiguration as SttConfiguration, MockRecogniser, ScriptStep,
};
use inference_testkit::tts::{MockConfiguration as TtsConfiguration, MockSynthesiser};
use inference_text::{AudioPlayed, OutputSegment, SegmentDiscarded, SegmentId, SegmentQueued};
use inference_tts::{
    ConcurrencyLimitReached, DiscardRefused, OfferRefused, PlaybackReceipt, PlaybackRefused,
    SynthesisEvent, SynthesisHandle, SynthesisOutput, Synthesiser, UtteranceEnded,
};
use local_voice_cascade::{AUDIO_INGRESS, CascadeHandles, VoicePayload, declare};

const QUESTIONS: [&str; 3] = [
    "tell me about the sea",
    "and about the ships",
    "and the sailors on them",
];

const PATIENCE: Duration = Duration::from_secs(60);

const DEVICE_FRAME: usize = 480;

fn reply(question: &str) -> String {
    format!("here is what I can tell you {question} and a little more")
}

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

fn eventually(ready: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if ready() {
            return true;
        }
        thread::sleep(Duration::from_millis(2));
    }
    ready()
}

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
                cue(5, ScriptStep::Hypothesis(QUESTIONS[2].to_owned())),
                cue(6, ScriptStep::Commit { span: 2 }),
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

fn utterances() -> (Vec<AudioFrame>, Vec<AudioFrame>) {
    let utterance = [
        Stretch::Tone {
            frames: 30,
            hertz: 440,
            amplitude: 0.5,
        },
        Stretch::Silence { frames: 80 },
    ];
    let mut first = synthesize(&[utterance[0], utterance[1], utterance[0], utterance[1]]);
    let second = first.split_off(110);
    (first, second)
}

struct Microphone {
    run_id: RunId,
    producer: NodeId,
    stream_id: StreamId,
    sequence: i64,
    last: i64,
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

    fn say(&mut self, frames: Vec<AudioFrame>) -> Vec<EventEnvelope<VoicePayload>> {
        frames
            .into_iter()
            .map(|frame| {
                let timestamp = i64::try_from(frame.position.get()).expect("within i64");
                self.last = timestamp + i64::from(frame.sample_count);
                self.envelope(
                    "inference.audio.AudioFrame",
                    timestamp,
                    VoicePayload::Audio(frame),
                )
            })
            .collect()
    }

    fn stop(&mut self) -> EventEnvelope<VoicePayload> {
        let last = self.last;
        self.envelope(
            ControlKind::Stop.as_str(),
            last,
            VoicePayload::Control(ControlKind::Stop),
        )
    }
}

/// Where the synthesis of the first offered segment stands: entered and waiting, then released.
#[derive(Debug, Default)]
struct Hold {
    entered: bool,
    released: bool,
}

#[derive(Debug, Clone, Default)]
struct Gate(Arc<(Mutex<Hold>, Condvar)>);

impl Gate {
    fn with<T>(&self, change: impl FnOnce(&mut Hold) -> T) -> T {
        let answer = change(&mut self.0.0.lock().unwrap_or_else(PoisonError::into_inner));
        self.0.1.notify_all();
        answer
    }

    fn entered(&self) -> bool {
        self.with(|hold| hold.entered)
    }

    fn release(&self) {
        self.with(|hold| hold.released = true);
    }

    /// Marks the gate entered and waits until it is released.
    fn hold(&self) {
        let mut hold = self.0.0.lock().unwrap_or_else(PoisonError::into_inner);
        hold.entered = true;
        self.0.1.notify_all();
        while !hold.released {
            hold = self.0.1.wait(hold).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// A synthesiser that renders its first offered segment synchronously, holding the caller of
/// `offer_segment` until the test releases it.
#[derive(Debug, Clone)]
struct Rendering<S> {
    inner: S,
    gate: Gate,
}

impl<S: Synthesiser> Synthesiser for Rendering<S> {
    type Handle = RenderingSynthesis<S::Handle>;

    fn adapter(&self) -> &Adapter {
        self.inner.adapter()
    }

    fn start(&mut self) -> Result<Self::Handle, ConcurrencyLimitReached> {
        Ok(RenderingSynthesis {
            inner: self.inner.start()?,
            gate: self.gate.clone(),
            offered: 0,
        })
    }
}

#[derive(Debug)]
struct RenderingSynthesis<H> {
    inner: H,
    gate: Gate,
    offered: usize,
}

impl<H: SynthesisHandle> Iterator for RenderingSynthesis<H> {
    type Item = SynthesisEvent;

    fn next(&mut self) -> Option<SynthesisEvent> {
        self.inner.next()
    }
}

impl<H: SynthesisHandle> SynthesisHandle for RenderingSynthesis<H> {
    fn output(&self) -> &SynthesisOutput {
        self.inner.output()
    }

    fn offer_segment(&mut self, segment: OutputSegment) -> Result<SegmentQueued, OfferRefused> {
        if self.offered == 0 {
            self.gate.hold();
        }
        self.offered += 1;
        self.inner.offer_segment(segment)
    }

    fn end_utterance(&mut self) -> Result<(), UtteranceEnded> {
        self.inner.end_utterance()
    }

    fn acknowledge_playback(
        &mut self,
        segment_id: SegmentId,
        receipt: PlaybackReceipt,
    ) -> Result<AudioPlayed, PlaybackRefused> {
        self.inner.acknowledge_playback(segment_id, receipt)
    }

    fn discard(&mut self, segment_id: SegmentId) -> Result<SegmentDiscarded, DiscardRefused> {
        self.inner.discard(segment_id)
    }
}

/// One running cascade: its handles, the turn store, the log, the ingress and the drive loop.
struct Running {
    host: FakeHost,
    handles: CascadeHandles,
    turns: TurnStore,
    log: RunLog,
    microphone: Microphone,
    ingress: Box<dyn inference_core::endpoint::OutputEndpoint<VoicePayload>>,
    egresses: Vec<JoinHandle<()>>,
    drive: JoinHandle<RunState>,
}

fn run(gate: Option<Gate>) -> Running {
    let host = FakeHost::laptop();
    let mocks = mocks();
    let mut declaration = Declaration::new();
    let cascade = declare(&mut declaration, &mocks).expect("the cascade validates and plans");
    let run_id = RunId::generate();
    let turns = TurnStore::default();
    let log = RunLog::new();
    let mut nodes = match gate {
        Some(gate) => cascade.nodes_with(
            run_id,
            mocks,
            |inner| Rendering { inner, gate },
            turns.clone(),
            &log,
            &host,
            PlayoutConfig::default(),
        ),
        None => cascade.nodes(
            run_id,
            mocks,
            turns.clone(),
            &log,
            &host,
            PlayoutConfig::default(),
        ),
    }
    .expect("the fake speaker opens");
    let handles = nodes.handles.clone();
    let mut controller = RunController::create_as(
        run_id,
        &cascade.planned,
        &declaration,
        HashMap::<GenerationId, i64>::new(),
        |node: &Node| {
            nodes
                .implementations
                .remove(&node.node_id())
                .expect("an implementation for every node")
        },
    )
    .expect("the cascade is planned");
    controller.validate().expect("a Created run validates");
    block_on(controller.prepare()).expect("every node prepares");
    let mut ends = controller.exposed();
    let ingress = ends.ingress(AUDIO_INGRESS).expect("the audio ingress");
    let mut egresses = Vec::new();
    for name in [
        local_voice_cascade::TRANSCRIPT_EGRESS,
        local_voice_cascade::TEXT_EGRESS,
    ] {
        let mut egress = ends.egress(name).expect("the egress");
        egresses.push(thread::spawn(
            move || {
                while block_on(egress.recv()).is_some() {}
            },
        ));
    }
    block_on(controller.activate()).expect("every node starts");
    let drive = thread::spawn(move || {
        let _ = block_on(controller.drive());
        let state = controller.run().state();
        drop(ends);
        state
    });
    Running {
        host,
        handles,
        turns,
        log,
        microphone: Microphone {
            run_id,
            producer: NodeId::generate(),
            stream_id: StreamId::generate(),
            sequence: 0,
            last: 0,
        },
        ingress,
        egresses,
        drive,
    }
}

impl Running {
    fn say(&mut self, frames: Vec<AudioFrame>) {
        for envelope in self.microphone.say(frames) {
            block_on(self.ingress.send(envelope)).expect("the ingress takes every frame");
        }
    }

    /// Ends the input and plays the device until the run ends; the run's final state.
    fn finish(mut self) -> RunState {
        let stop = self.microphone.stop();
        block_on(self.ingress.send(stop)).expect("the ingress takes the end of input");
        let deadline = Instant::now() + PATIENCE;
        while !self.drive.is_finished() {
            assert!(Instant::now() < deadline, "the run ends");
            let _ = self.host.pull(DEVICE_FRAME);
            thread::sleep(Duration::from_millis(1));
        }
        for egress in self.egresses {
            egress.join().expect("an egress reader");
        }
        drop(self.ingress);
        self.drive.join().expect("the drive loop does not panic")
    }
}

#[test]
fn a_turn_opened_before_the_first_frame_of_the_answer_lets_no_frame_of_that_answer_play() {
    let gate = Gate::default();
    let mut running = run(Some(gate.clone()));
    let (first, second) = utterances();

    // The first answer's first segment is being rendered; no frame of it reached the sink.
    running.say(first);
    assert!(
        eventually(|| gate.entered()),
        "the synthesiser renders the first segment"
    );
    assert_eq!(
        running.handles.sink.pushed(),
        0,
        "no frame reached the sink"
    );

    // The user speaks again: the second turn opens and the LLM supersedes the first answer.
    running.say(second);
    let handles = running.handles.clone();
    let turns = running.turns.clone();
    assert!(
        eventually(|| handles.llm.superseded().len() == 1
            && turns.with(|stream| stream.turns_view().len()) == 2),
        "the second turn opened and the first generation was superseded"
    );
    // The turn split hands the `TurnOpened` to the LLM and the sink alike; the sink reads its
    // turns edge every 2 ms.
    thread::sleep(Duration::from_millis(300));
    assert_eq!(handles.sink.pushed(), 0, "still no frame reached the sink");
    let old = handles.llm.generations()[0].clone();
    let old_segments: Vec<SegmentId> = old
        .segments
        .iter()
        .map(|segment| segment.segment_id())
        .collect();

    // The rendering returns; the TTS node sends what it rendered.
    gate.release();
    assert!(
        eventually(|| !handles.tts.discarded().is_empty()),
        "the TTS node took the supersede"
    );
    thread::sleep(Duration::from_millis(300));
    let stale: Vec<SegmentId> = handles
        .sink
        .first_frames()
        .iter()
        .map(|first| first.segment_id)
        .filter(|segment_id| old_segments.contains(segment_id))
        .collect();
    let pushed = handles.sink.pushed();
    let state = running.finish();
    assert_eq!(state, RunState::Completed);
    assert!(
        stale.is_empty(),
        "frames of the superseded generation {} were handed to the speaker after the second turn \
         opened: segments {stale:?} of {old_segments:?}; {pushed} frames pushed, stale frames \
         dropped {:?}, interruptions {:?}",
        old.generation_id,
        handles.sink.stale_frames(),
        handles.sink.interruptions()
    );
}

#[test]
fn an_answer_played_to_the_end_is_not_superseded_by_the_next_turn() {
    let mut running = run(None);
    let (first, second) = utterances();

    running.say(first);
    let handles = running.handles.clone();
    let deadline = Instant::now() + PATIENCE;
    while handles.tts.played().len() < QUESTIONS.len() {
        assert!(
            Instant::now() < deadline,
            "the first answer is played to the end: {:?}",
            handles.tts.played()
        );
        let _ = running.host.pull(DEVICE_FRAME);
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(handles.sink.receipts().len(), QUESTIONS.len());

    running.say(second);
    let log = running.log.clone();
    let state = running.finish();
    assert_eq!(state, RunState::Completed);

    let generations = handles.llm.generations();
    assert_eq!(generations.len(), 2, "one generation per turn");
    let superseded: Vec<GenerationId> = log
        .envelopes()
        .iter()
        .filter_map(|envelope| match &envelope.payload {
            StreamEvent::GenerationSuperseded(event) => Some(event.generation_id),
            _ => None,
        })
        .collect();
    assert_ne!(
        generations[0].state,
        GenerationState::Superseded,
        "the first answer was played to the end before the second turn opened, and its \
         generation ended Superseded; GenerationSuperseded sent for {superseded:?}"
    );
}
