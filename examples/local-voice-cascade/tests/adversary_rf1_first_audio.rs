//! Adversary pass 1 on `story:llama-server-loopback-only` (finding N38): "committed input to first
//! playable audio" pairs each passed turn with the first frame after it that no earlier turn took.
//! The sink records one first frame per segment, not per turn, and a turn superseded before any
//! of its audio reached the speaker has none. Then the earlier turn takes the later turn's frame,
//! and the later turn is left with its own second segment's frame or with none.
//!
//! The case: two turns of one span each, so each answer is one segment. The synthesiser holds the
//! first answer's only segment back until it is discarded, so no frame of the first answer is ever
//! handed to the speaker. The second utterance opens the second turn, which supersedes the first
//! generation; the second answer plays. Its turn's measure is the time to the second answer's first
//! frame, the only first frame the sink recorded.

use std::collections::HashMap;
use std::future::Future;
use std::num::{NonZeroU16, NonZeroU32};
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use inference_audio::{AudioEncoding, AudioFormat, AudioFrame};
use inference_audio_device::PlayoutConfig;
use inference_audio_device::fake::FakeHost;
use inference_core::envelope::{ControlKind, EventEnvelope, EventId, Timebase};
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::ModelId;
use inference_pipeline::{Declaration, Node};
use inference_runtime::RunController;
use inference_testkit::cascade::late::LateSynthesiser;
use inference_testkit::cascade::{Mocks, TurnStore};
use inference_testkit::graph::RunLog;
use inference_testkit::llm::{MockConfiguration as LlmConfiguration, MockGenerator};
use inference_testkit::stt::synthetic_audio::{Stretch, synthesize};
use inference_testkit::stt::{
    Cue, MockConfiguration as SttConfiguration, MockRecogniser, ScriptStep,
};
use inference_testkit::tts::{MockConfiguration as TtsConfiguration, MockSynthesiser};
use local_voice_cascade::{AUDIO_INGRESS, VoicePayload, declare};

const PATIENCE: Duration = Duration::from_secs(60);
const DEVICE_FRAME: usize = 480;

fn reply(question: &str) -> String {
    format!("here is what I can tell you {question}")
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

/// One span per turn, so each answer is one segment.
fn mocks() -> Mocks {
    let cue = |after_frames, step| Cue { after_frames, step };
    Mocks {
        recogniser: MockRecogniser::new(
            SttConfiguration::Incremental,
            ModelId::generate(),
            vec![
                cue(
                    1,
                    ScriptStep::Hypothesis("tell me about the sea".to_owned()),
                ),
                cue(2, ScriptStep::Commit { span: 0 }),
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
                    "inference.audio.AudioChunk",
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

#[test]
fn a_turn_whose_answer_never_played_does_not_take_the_next_turns_first_audio() {
    let host = FakeHost::laptop();
    let mocks = mocks();
    let mut declaration = Declaration::new();
    let cascade = declare(&mut declaration, &mocks).expect("the cascade validates and plans");
    let run_id = RunId::generate();
    let log = RunLog::new();
    let late = LateSynthesiser::new(mocks.synthesiser.clone(), 0);
    let late_handle = late.handle();
    let mut run = cascade
        .nodes_with(
            run_id,
            mocks,
            |_mock| late,
            TurnStore::default(),
            &log,
            &host,
            PlayoutConfig::default(),
        )
        .expect("the fake speaker opens");
    let handles = run.handles.clone();
    let mut controller = RunController::create_as(
        run_id,
        &cascade.planned,
        &declaration,
        HashMap::<GenerationId, i64>::new(),
        |node: &Node| {
            run.implementations
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
        let result = block_on(controller.drive());
        (controller, result)
    });

    let (first, second) = utterances();
    let mut microphone = Microphone {
        run_id,
        producer: NodeId::generate(),
        stream_id: StreamId::generate(),
        sequence: 0,
        last: 0,
    };

    for envelope in microphone.say(first) {
        block_on(ingress.send(envelope)).expect("the ingress takes every frame");
    }
    assert!(
        eventually(
            || late_handle.withheld().is_some() && handles.input_gate.turns_passed().len() == 1
        ),
        "the first turn passed and its answer's only segment is held back"
    );
    assert!(
        handles.sink.first_frames().is_empty(),
        "no frame of the first answer reached the speaker: {:?}",
        handles.sink.first_frames()
    );

    for envelope in microphone.say(second) {
        block_on(ingress.send(envelope)).expect("the ingress takes every frame");
    }
    block_on(ingress.send(microphone.stop())).expect("the ingress takes the end of input");
    let deadline = Instant::now() + PATIENCE;
    while !drive.is_finished() {
        assert!(Instant::now() < deadline, "the run ends");
        let _ = host.pull(DEVICE_FRAME);
        thread::sleep(Duration::from_millis(1));
    }
    for egress in egresses {
        egress.join().expect("an egress reader");
    }
    drop(ingress);
    drop(ends);
    let (_controller, result) = drive.join().expect("the drive loop does not panic");
    result.expect("the drive loop ends the run");

    let passed = handles.input_gate.turns_passed();
    let first_frames = handles.sink.first_frames();
    let withheld = late_handle.withheld().expect("a segment was held back");
    assert_eq!(passed.len(), 2, "both turns' completions were passed");
    assert!(
        !first_frames.is_empty(),
        "the second answer reached the speaker"
    );
    assert!(
        first_frames
            .iter()
            .all(|first| first.segment_id != withheld),
        "the first answer never reached the speaker: {first_frames:?}"
    );

    let measures = handles.committed_input_to_first_audio();
    assert_eq!(measures.len(), 2);
    assert_eq!(
        measures[1].first_audio,
        Some(first_frames[0].at.duration_since(passed[1].at)),
        "the second turn's first playable audio is its own answer's first frame, the only answer \
         that played; the first turn, whose answer never played, took it: {measures:?}"
    );
}
