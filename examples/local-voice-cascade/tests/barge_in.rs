//! Barge-in in the local voice cascade on the testkit mocks and the fake audio host
//! (`story:cascade-barge-in`; interruption and barge-in at `docs/design/inference.md` 632-641,
//! A-10 and section 23.3 test 5).
//!
//! The test is the application and the speaker's device: it says a first utterance, plays the
//! first segment of the answer and then holds the device, so that the rest of the answer waits
//! unplayed in the speaker's queue, and says a second utterance. The synthesiser is scripted to
//! deliver the frames of the first answer's last segment only once that segment is discarded, as
//! a backend with frames in flight would. Then it ends the input and plays the device on until the
//! run ends.
//!
//! The scenarios read: `inference.stream.SupersedeGeneration/outcome/superseded` (the first
//! generation moves to Superseded at the second turn's opening) and
//! `inference.stream.DiscardSegment/outcome/discarded` (its segments not yet Played move to
//! Discarded, and none of them is Played after the speaker stopped); the late frames are dropped
//! at the sink, with no receipt and the played position unchanged; the second answer is played and
//! acknowledged; and the interruption-to-playback-stop measure (1023) is recorded.

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
use inference_core::generation::GenerationState;
use inference_core::id::{GenerationId, NodeId, RunId, StreamId};
use inference_core::model::ModelId;
use inference_pipeline::{Declaration, Node};
use inference_runtime::RunController;
use inference_runtime::run::{NodeInstanceState, RunState};
use inference_testkit::cascade::late::LateSynthesiser;
use inference_testkit::cascade::{Mocks, TurnStore};
use inference_testkit::graph::RunLog;
use inference_testkit::llm::{MockConfiguration as LlmConfiguration, MockGenerator};
use inference_testkit::stream::StreamEvent;
use inference_testkit::stt::synthetic_audio::{Stretch, synthesize};
use inference_testkit::stt::{
    Cue, MockConfiguration as SttConfiguration, MockRecogniser, ScriptStep,
};
use inference_testkit::tts::{MockConfiguration as TtsConfiguration, MockSynthesiser};
use inference_text::{SegmentId, SegmentState};
use local_voice_cascade::{AUDIO_INGRESS, VoicePayload, declare};

/// What the user says in each utterance: the recogniser's three scripted spans, in one turn.
const QUESTIONS: [&str; 3] = [
    "tell me about the sea",
    "and about the ships",
    "and the sailors on them",
];

/// How long the run may take before the test gives up.
const PATIENCE: Duration = Duration::from_secs(60);

/// Device frames of the fake laptop's speaker, at 48 kHz, that play one frame of the mock
/// synthesiser, 160 samples at 16 kHz.
const DEVICE_FRAME: usize = 480;

/// The mock LLM's answer to a question: one segment per question, one frame per word.
fn reply(question: &str) -> String {
    format!("here is what I can tell you {question} and a little more")
}

fn words(text: &str) -> usize {
    text.split_whitespace().count()
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

/// Waits until `ready` holds, without playing the device; false after `PATIENCE`.
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

/// The scripted mocks: the recogniser emits and commits three spans per turn, the LLM answers
/// each with [`reply`], the synthesiser streams one frame per word.
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

/// Two utterances on one clock, each 300 ms of a 440 Hz tone at half scale and then 800 ms of
/// silence.
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

/// What the application sends on the ingress: frames of its own stream, numbered from 1, on the
/// audio frame clock, then the end of input.
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

#[test]
#[allow(clippy::too_many_lines)]
fn a_second_utterance_supersedes_the_first_answer_stops_the_speaker_and_drops_its_late_frames() {
    let host = FakeHost::laptop();
    let mocks = mocks();
    let mut declaration = Declaration::new();
    let cascade = declare(&mut declaration, &mocks).expect("the cascade validates and plans");
    let ids = cascade.ids;
    let run_id = RunId::generate();
    let turns = TurnStore::default();
    let log = RunLog::new();
    let late = LateSynthesiser::new(mocks.synthesiser.clone(), 2);
    let late_handle = late.handle();
    let mut run = cascade
        .nodes_with(
            run_id,
            mocks,
            |_mock| late,
            turns.clone(),
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
                .expect("the cascade has an implementation for every node")
        },
    )
    .expect("the cascade is planned and no edge cancels or supersedes");
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
        egresses.push(thread::spawn(move || {
            let mut received = Vec::new();
            while let Some(envelope) = block_on(egress.recv()) {
                received.push(envelope);
            }
            received
        }));
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
    let answer_words: Vec<usize> = QUESTIONS
        .iter()
        .map(|question| words(&reply(question)))
        .collect();

    // The first utterance; the whole first answer reaches the speaker but its last segment, held
    // back by the synthesiser. The device is not played yet.
    for envelope in microphone.say(first) {
        block_on(ingress.send(envelope)).expect("the ingress takes every frame");
    }
    assert!(
        eventually(|| handles.tts.queued().len() == 3
            && late_handle.withheld().is_some()
            && handles.sink.pushed() >= (answer_words[0] + answer_words[1]) as u64),
        "the first answer is queued: tts {:?}, sink pushed {}",
        handles.tts.queued(),
        handles.sink.pushed()
    );
    let withheld = late_handle
        .withheld()
        .expect("the third segment was offered");

    // The device plays the first segment, then holds. A callback that finds the queue's lock
    // taken writes silence and plays nothing, so the device is pulled one frame more only when
    // no receipt followed for a while.
    for _ in 0..answer_words[0] {
        let _ = host.pull(DEVICE_FRAME);
    }
    let deadline = Instant::now() + PATIENCE;
    while handles.sink.receipts().is_empty() {
        assert!(
            Instant::now() < deadline,
            "the first segment is acknowledged"
        );
        let waited = Instant::now() + Duration::from_millis(200);
        while handles.sink.receipts().is_empty() && Instant::now() < waited {
            thread::sleep(Duration::from_millis(2));
        }
        if handles.sink.receipts().is_empty() {
            let _ = host.pull(DEVICE_FRAME);
        }
    }
    assert_eq!(
        handles.sink.receipts().len(),
        1,
        "only the first segment is acknowledged"
    );

    // The second utterance, while the rest of the first answer waits in the speaker's queue.
    for envelope in microphone.say(second) {
        block_on(ingress.send(envelope)).expect("the ingress takes every frame");
    }
    assert!(
        eventually(|| handles.sink.interruptions().len() == 1),
        "the sink stops the speaker at the second turn's opening"
    );
    assert!(
        eventually(|| handles.sink.stale_frames().len() == answer_words[2]),
        "every late frame reaches the sink and is dropped: {:?}, late {}",
        handles.sink.stale_frames(),
        late_handle.late_frames()
    );
    assert_eq!(
        handles.sink.receipts().len(),
        1,
        "no receipt after the stop while the device held"
    );

    block_on(ingress.send(microphone.stop())).expect("the ingress takes the end of input");
    let deadline = Instant::now() + PATIENCE;
    while !drive.is_finished() {
        assert!(
            Instant::now() < deadline,
            "the run ends once the speaker played the second answer; sink {:?}, tts {:?}",
            handles.sink.receipts(),
            handles.tts.played()
        );
        let _ = host.pull(DEVICE_FRAME);
        thread::sleep(Duration::from_millis(1));
    }
    for egress in egresses {
        egress.join().expect("an egress reader");
    }
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
    assert_eq!(controller.run().state(), RunState::Completed);
    for node_id in ids.nodes() {
        assert_eq!(
            controller
                .instance(node_id)
                .map(|instance| instance.state()),
            Some(NodeInstanceState::Stopped),
            "no instance changes state on the interruption: node {node_id}"
        );
    }
    drop(controller);
    assert!(!host.output_open(), "the speaker is closed");

    let turn_rows = turns.with(|stream| stream.turns_view());
    assert_eq!(turn_rows.len(), 2, "two utterances, two turns");
    let second_turn = turn_rows[1].turn_id;

    // SupersedeGeneration/outcome/superseded: the first generation, answering the first turn, was
    // superseded at the second turn's opening; the second answers the second turn and completed.
    let generations = handles.llm.generations();
    assert_eq!(
        generations
            .iter()
            .map(|generation| (generation.turn_id, generation.epoch, generation.state))
            .collect::<Vec<_>>(),
        [
            (turn_rows[0].turn_id, 1, GenerationState::Superseded),
            (second_turn, 2, GenerationState::Completed),
        ]
    );
    let (old, new) = (&generations[0], &generations[1]);
    let superseded = handles.llm.superseded();
    assert_eq!(superseded.len(), 1);
    assert_eq!(superseded[0].generation_id, old.generation_id);
    assert_eq!(superseded[0].turn_id, second_turn);
    assert!(
        superseded[0].computation >= superseded[0].delivery,
        "the generation's handle was dropped, its computation released apart from its delivery"
    );
    let envelopes = log.envelopes();
    assert!(envelopes.iter().any(|envelope| matches!(
        &envelope.payload,
        StreamEvent::GenerationSuperseded(event) if event.generation_id == old.generation_id
    )));

    let old_segments: Vec<SegmentId> = old
        .segments
        .iter()
        .map(|segment| segment.segment_id())
        .collect();
    let new_segments: Vec<SegmentId> = new
        .segments
        .iter()
        .map(|segment| segment.segment_id())
        .collect();
    assert_eq!(old_segments.len(), 3);
    assert_eq!(new_segments.len(), 3);
    assert_eq!(withheld, old_segments[2]);

    // The speaker stopped at once: its queue cleared, the played position read, the measure
    // recorded.
    let interruption = handles.sink.interruptions()[0];
    assert_eq!(interruption.turn_id, second_turn);
    assert!(interruption.cleared > 0, "unplayed audio was in the queue");
    let measures = handles.interruption_to_playback_stop();
    assert_eq!(measures.len(), 1);
    assert_eq!(measures[0].turn_id, second_turn);
    assert_eq!(measures[0].stop, interruption.stop);

    // DiscardSegment/outcome/discarded: the first generation's segments not yet Played were
    // discarded by the TTS node's synthesis, and none of them was Played after the stop; only
    // the first segment, acknowledged before it, is Played.
    let played = handles.tts.played();
    let played_old: Vec<SegmentId> = played
        .iter()
        .filter(|segment| old_segments.contains(&segment.segment_id()))
        .map(|segment| segment.segment_id())
        .collect();
    assert_eq!(played_old, [old_segments[0]]);
    let receipts = handles.sink.receipts();
    for receipt in receipts
        .iter()
        .filter(|receipt| old_segments.contains(&receipt.segment_id))
    {
        assert!(
            receipt.at < interruption.at,
            "a receipt made after the stop"
        );
    }
    let discarded = handles.tts.discarded();
    assert_eq!(
        discarded
            .iter()
            .map(|segment| (segment.segment_id(), segment.state()))
            .collect::<Vec<_>>(),
        [
            (old_segments[1], SegmentState::Discarded),
            (old_segments[2], SegmentState::Discarded),
        ]
    );
    let discarded_logged: Vec<SegmentId> = envelopes
        .iter()
        .filter_map(|envelope| match &envelope.payload {
            StreamEvent::SegmentDiscarded(event) => Some(event.segment_id),
            _ => None,
        })
        .collect();
    assert_eq!(discarded_logged, [old_segments[1], old_segments[2]]);

    // The late frames: delivered by the synthesis after the discard, every one dropped at the
    // sink with no receipt, the played position unchanged.
    assert_eq!(late_handle.late_frames(), answer_words[2]);
    let stale = handles.sink.stale_frames();
    assert_eq!(stale.len(), answer_words[2]);
    for frame in &stale {
        assert_eq!(frame.segment_id, withheld);
        assert_eq!(frame.generation_id, Some(old.generation_id));
        assert_eq!(
            frame.played, interruption.played_after,
            "the played position is unchanged"
        );
    }
    assert!(
        receipts
            .iter()
            .all(|receipt| receipt.segment_id != withheld),
        "no receipt of a dropped frame"
    );

    // The second answer was played and acknowledged, every segment by the sink's receipt.
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
    assert_eq!(
        receipts
            .iter()
            .map(|receipt| (receipt.segment_id, receipt.frames))
            .collect::<Vec<_>>(),
        [(old_segments[0], answer_words[0])]
            .into_iter()
            .chain(
                new_segments
                    .iter()
                    .copied()
                    .zip(answer_words.iter().copied())
            )
            .collect::<Vec<_>>()
    );
    let first_audio = handles.committed_input_to_first_audio();
    assert_eq!(first_audio.len(), 2, "both turns' completions were passed");
    assert!(
        first_audio[1].first_audio.is_some(),
        "the second answer played"
    );
}
