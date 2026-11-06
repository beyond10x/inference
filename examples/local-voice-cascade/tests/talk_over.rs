//! The caller talks over an answer that is playing, in the local voice cascade on the testkit
//! mocks and the fake audio host (`story:long-talk-over-does-not-resume-the-cut-answer`).
//!
//! The test is the application and the speaker's device, as `barge_in.rs` is: it says a first
//! utterance, plays the first segment of the answer and holds the device, so that the rest of the
//! answer waits unplayed in the speaker's queue. The synthesiser delivers the frames of the
//! answer's last segment only once that segment is discarded, as a backend with frames in flight
//! would. Then the caller speaks over the answer for the case's length, and pauses.
//!
//! Time is the audio frame clock: the talk-over is a number of 10 ms frames sent on the ingress,
//! and the device plays one 10 ms device frame for each frame sent once the talk-over's first
//! 100 ms are sent and the cascade has taken them. Nothing sleeps. The cascade's nodes run on
//! threads of their own, so the test waits for what they record, each wait failing the case at
//! its own deadline.
//!
//! Each case states, from the generation and segment state the nodes record:
//!
//! * when playout of the cut answer stops: at the opening of the talk-over's turn, which the
//!   sink takes while at most the talk-over's first 100 ms were sent;
//! * whether any frame of the cut answer is played after the talk-over starts: none of its
//!   segments is Played or acknowledged after the stop, the sink pushes no frame to the speaker
//!   for the rest of the talk-over, and every late frame is dropped;
//! * what the next answer answers: the generation answering the talk-over's turn, whose replies
//!   the mock LLM is scripted to make distinct, is the next one played.
//!
//! The mock recogniser replays the same script for every turn, so the talk-over's spans read as
//! the first utterance's did; the LLM's reply function counts the spans it answered and answers
//! those of the second turn with a reply of its own.

use std::collections::HashMap;
use std::future::Future;
use std::num::{NonZeroU16, NonZeroU32};
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
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
use inference_runtime::run::RunState;
use inference_testkit::cascade::late::LateSynthesiser;
use inference_testkit::cascade::{Mocks, TurnStore};
use inference_testkit::graph::RunLog;
use inference_testkit::llm::{MockConfiguration as LlmConfiguration, MockGenerator};
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

/// How long the run may take before the case gives up.
const PATIENCE: Duration = Duration::from_secs(60);

/// Device frames of the fake laptop's speaker, at 48 kHz, that play one 10 ms frame.
const DEVICE_FRAME: usize = 480;

/// The frames of the talk-over sent before the device plays on: 100 ms.
const ONSET: usize = 10;

/// The frames of silence after an utterance: 800 ms, past the detector's hold.
const PAUSE: usize = 80;

/// How the first answer to a span begins.
const FIRST: &str = "here is what I can tell you";

/// How the answer to a span of the talk-over begins.
const TALK_OVER: &str = "you spoke over me so now about";

/// The LLM's reply function of one case, counting the spans it answered on its own: the first
/// turn's three spans are answered with [`FIRST`], every later one with [`TALK_OVER`].
macro_rules! replies {
    () => {{
        fn reply(question: &str) -> String {
            static ANSWERED: AtomicUsize = AtomicUsize::new(0);
            if ANSWERED.fetch_add(1, Ordering::SeqCst) < QUESTIONS.len() {
                format!("{FIRST} {question} and a little more")
            } else {
                format!("{TALK_OVER} {question}")
            }
        }
        reply as fn(&str) -> String
    }};
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

/// Waits until `ready` holds, yielding to the cascade's threads, `play` run on every turn of the
/// wait; false after `within`.
fn eventually(within: Duration, mut play: impl FnMut(), ready: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if ready() {
            return true;
        }
        play();
        thread::yield_now();
    }
    ready()
}

/// The scripted mocks: the recogniser emits and commits three spans per turn, the LLM answers
/// each with `reply`, the synthesiser streams one frame per word.
fn mocks(reply: fn(&str) -> String) -> Mocks {
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

    fn frame(&mut self, frame: AudioFrame) -> EventEnvelope<VoicePayload> {
        let timestamp = i64::try_from(frame.position.get()).expect("within i64");
        self.last = timestamp + i64::from(frame.sample_count);
        self.envelope(
            "inference.audio.AudioChunk",
            timestamp,
            VoicePayload::Audio(frame),
        )
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

/// The first utterance, 300 ms of a 440 Hz tone at half scale and [`PAUSE`] of silence, then
/// the talk-over, `talk` frames of the same tone and [`PAUSE`] of silence, on one clock.
fn utterances(talk: usize) -> (Vec<AudioFrame>, Vec<AudioFrame>) {
    let tone = |frames| Stretch::Tone {
        frames,
        hertz: 440,
        amplitude: 0.5,
    };
    let pause = Stretch::Silence { frames: PAUSE };
    let mut first = synthesize(&[tone(30), pause, tone(talk), pause]);
    let second = first.split_off(30 + PAUSE);
    (first, second)
}

/// Says a first utterance, lets the first segment of its answer play, holds the device and
/// speaks over the rest of the answer for `talk` frames of 10 ms, then pauses; checks what the
/// case states.
#[allow(clippy::too_many_lines)]
fn talk_over(talk: usize, reply: fn(&str) -> String) {
    assert!(talk > ONSET, "the talk-over outlasts its onset");
    let host = FakeHost::laptop();
    let mocks = mocks(reply);
    let mut declaration = Declaration::new();
    let cascade = declare(&mut declaration, &mocks).expect("the cascade validates and plans");
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

    let (first, second) = utterances(talk);
    let mut microphone = Microphone {
        run_id,
        producer: NodeId::generate(),
        stream_id: StreamId::generate(),
        sequence: 0,
        last: 0,
    };
    let first_words: Vec<usize> = QUESTIONS
        .iter()
        .map(|question| words(&format!("{FIRST} {question} and a little more")))
        .collect();
    let next_words: Vec<usize> = QUESTIONS
        .iter()
        .map(|question| words(&format!("{TALK_OVER} {question}")))
        .collect();
    let pull = || {
        let _ = host.pull(DEVICE_FRAME);
    };

    // The first utterance; its answer reaches the speaker but its last segment, held back by the
    // synthesiser. The device is not played yet.
    for frame in first {
        block_on(ingress.send(microphone.frame(frame))).expect("the ingress takes every frame");
    }
    assert!(
        eventually(
            PATIENCE,
            || {},
            || handles.tts.queued().len() == 3
                && late_handle.withheld().is_some()
                && handles.sink.pushed() >= (first_words[0] + first_words[1]) as u64
        ),
        "the first answer is queued: tts {:?}, sink pushed {}",
        handles.tts.queued(),
        handles.sink.pushed()
    );

    // The device plays the first segment, then holds. A callback that finds the queue's lock
    // taken plays nothing, so the device is pulled one frame more only when no receipt followed
    // for a while.
    for _ in 0..first_words[0] {
        pull();
    }
    let deadline = Instant::now() + PATIENCE;
    while handles.sink.receipts().is_empty() {
        assert!(
            Instant::now() < deadline,
            "the first segment is acknowledged"
        );
        if !eventually(
            Duration::from_millis(200),
            || {},
            || !handles.sink.receipts().is_empty(),
        ) {
            pull();
        }
    }
    assert_eq!(
        handles.sink.receipts().len(),
        1,
        "only the first segment is acknowledged before the talk-over"
    );

    // The talk-over's first 100 ms, the device holding: the sink stops the speaker at the opening
    // of the talk-over's turn.
    let mut second = second.into_iter();
    for frame in second.by_ref().take(ONSET) {
        block_on(ingress.send(microphone.frame(frame))).expect("the ingress takes every frame");
    }
    assert!(
        eventually(PATIENCE, || {}, || handles.sink.interruptions().len() == 1),
        "the sink stops the speaker within the talk-over's first {} ms: turns {:?}, policy {:?}, \
         generations {:?}, sink pushed {} receipts {:?}, tts queued {:?}, failures stt {:?} llm \
         {:?} tts {:?} sink {:?}",
        ONSET * 10,
        turns.with(|stream| stream.turns_view()),
        handles.policy,
        handles.llm.generations(),
        handles.sink.pushed(),
        handles.sink.receipts(),
        handles.tts.queued(),
        handles.stt.failure(),
        handles.llm.failure(),
        handles.tts.failure(),
        handles.sink.failure()
    );
    let pushed_at_stop = handles.sink.pushed();

    // The rest of the talk-over, the device playing one frame for each frame said: the sink
    // pushes nothing more until the talk-over's turn is answered.
    for frame in second.by_ref().take(talk - ONSET) {
        block_on(ingress.send(microphone.frame(frame))).expect("the ingress takes every frame");
        pull();
    }
    assert!(
        eventually(PATIENCE, pull, || handles.sink.stale_frames().len()
            == first_words[2]),
        "every late frame of the cut answer reaches the sink and is dropped: {:?}, late {}",
        handles.sink.stale_frames(),
        late_handle.late_frames()
    );
    assert_eq!(
        handles.sink.pushed(),
        pushed_at_stop,
        "no frame reaches the speaker during the talk-over"
    );

    // The pause after the talk-over, and the end of input; the device plays on until the run
    // ends.
    for frame in second {
        block_on(ingress.send(microphone.frame(frame))).expect("the ingress takes every frame");
        pull();
    }
    block_on(ingress.send(microphone.stop())).expect("the ingress takes the end of input");
    assert!(
        eventually(PATIENCE, pull, || drive.is_finished()),
        "the run ends once the speaker played the next answer; sink {:?}, tts {:?}",
        handles.sink.receipts(),
        handles.tts.played()
    );
    for egress in egresses {
        egress.join().expect("an egress reader");
    }
    drop(ingress);
    drop(ends);
    let (controller, result) = drive.join().expect("the drive loop does not panic");
    assert_eq!(handles.stt.failure(), None);
    assert_eq!(handles.llm.failure(), None);
    assert_eq!(handles.tts.failure(), None);
    assert_eq!(handles.sink.failure(), None);
    result.expect("the drive loop ends the run");
    assert_eq!(controller.run().state(), RunState::Completed);
    drop(controller);

    let turn_rows = turns.with(|stream| stream.turns_view());
    assert_eq!(
        turn_rows.len(),
        2,
        "the talk-over of {talk} frames is one turn after the first"
    );
    let talk_over_turn = turn_rows[1].turn_id;

    // Playout stops at the opening of the talk-over's turn: the queue cleared and the played
    // position read; after the stop no frame is pushed, so none is read.
    let interruptions = handles.sink.interruptions();
    assert_eq!(
        interruptions.len(),
        1,
        "one interruption: {interruptions:?}"
    );
    let interruption = interruptions[0];
    assert_eq!(interruption.turn_id, talk_over_turn);
    assert!(interruption.cleared > 0, "the cut answer was still queued");
    assert!(
        interruption.position.is_some(),
        "the stop answered where the cut answer was"
    );
    assert_eq!(interruption.played_after, None);

    // The cut answer is superseded at that opening; the next generation answers the talk-over's
    // turn and completes.
    let generations = handles.llm.generations();
    assert_eq!(
        generations
            .iter()
            .map(|generation| (generation.turn_id, generation.epoch, generation.state))
            .collect::<Vec<_>>(),
        [
            (turn_rows[0].turn_id, 1, GenerationState::Superseded),
            (talk_over_turn, 2, GenerationState::Completed),
        ]
    );
    let (cut, next) = (&generations[0], &generations[1]);
    let cut_segments: Vec<SegmentId> = cut
        .segments
        .iter()
        .map(|segment| segment.segment_id())
        .collect();
    let next_segments: Vec<SegmentId> = next
        .segments
        .iter()
        .map(|segment| segment.segment_id())
        .collect();
    assert_eq!(cut_segments.len(), 3);

    // No frame of the cut answer plays after the talk-over starts: only its first segment,
    // acknowledged before, is Played; the others are Discarded; every receipt of it predates the
    // stop; every late frame was dropped at the sink with the played position unchanged.
    let played = handles.tts.played();
    assert_eq!(
        played
            .iter()
            .filter(|segment| cut_segments.contains(&segment.segment_id()))
            .map(|segment| segment.segment_id())
            .collect::<Vec<_>>(),
        [cut_segments[0]]
    );
    assert_eq!(
        handles
            .tts
            .discarded()
            .iter()
            .map(|segment| (segment.segment_id(), segment.state()))
            .collect::<Vec<_>>(),
        [
            (cut_segments[1], SegmentState::Discarded),
            (cut_segments[2], SegmentState::Discarded),
        ]
    );
    let receipts = handles.sink.receipts();
    for receipt in receipts
        .iter()
        .filter(|receipt| cut_segments.contains(&receipt.segment_id))
    {
        assert!(
            receipt.at < interruption.at,
            "a receipt of the cut answer after the stop"
        );
    }
    for frame in handles.sink.stale_frames() {
        assert_eq!(frame.generation_id, Some(cut.generation_id));
        assert_eq!(frame.played, interruption.played_after);
    }
    let after_stop: Vec<SegmentId> = handles
        .sink
        .first_frames()
        .iter()
        .filter(|first| first.at >= interruption.at)
        .map(|first| first.segment_id)
        .collect();
    assert_eq!(
        after_stop, next_segments,
        "after the stop the speaker is handed the next answer's segments only, in order"
    );

    // The next answer answers the talk-over: its segments are the replies to the talk-over's
    // spans, each played and acknowledged.
    assert_eq!(
        next.segments
            .iter()
            .map(|segment| segment.text().to_owned())
            .collect::<Vec<_>>(),
        QUESTIONS
            .iter()
            .map(|question| format!("{TALK_OVER} {question}"))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        played
            .iter()
            .filter(|segment| next_segments.contains(&segment.segment_id()))
            .map(|segment| (segment.segment_id(), segment.state()))
            .collect::<Vec<_>>(),
        next_segments
            .iter()
            .map(|segment_id| (*segment_id, SegmentState::Played))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        receipts
            .iter()
            .map(|receipt| (receipt.segment_id, receipt.frames))
            .collect::<Vec<_>>(),
        [(cut_segments[0], first_words[0])]
            .into_iter()
            .chain(next_segments.iter().copied().zip(next_words))
            .collect::<Vec<_>>()
    );
}

/// Half a second of speech over the answer: playout stops at the turn's opening, nothing of the
/// cut answer plays again, and the next answer answers the interruption. Today's behaviour,
/// pinned.
#[test]
fn a_short_interruption_of_half_a_second_stops_the_answer_and_is_answered() {
    talk_over(50, replies!());
}

#[test]
fn a_long_talk_over_of_6_s_never_resumes_the_cut_answer_and_the_next_answer_answers_it() {
    talk_over(600, replies!());
}

#[test]
fn a_long_talk_over_of_12_s_never_resumes_the_cut_answer_and_the_next_answer_answers_it() {
    talk_over(1_200, replies!());
}

/// A "mm-hm" of 0.3 s opens a turn like any other speech: it cuts the answer, which does not
/// come back, and the next answer answers the backchannel. Today's behaviour, pinned.
#[test]
fn a_backchannel_cuts_the_answer_today_and_is_answered_as_a_turn() {
    talk_over(30, replies!());
}
