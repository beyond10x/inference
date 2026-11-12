//! Adversary cases on the live arm's barge-in bound of
//! `adversary2_topology_workers::the_live_arm_paces_the_barge_in_while_the_llm_and_the_synthesiser_block`:
//! barge-in to stop plus `Measured::barge_in_sent_late`, from when the barge-in's first voiced
//! frame was due, within 10 to 80 ms.
//!
//! The live cases hold the arm's loop for a set time at the barge-in's first voiced frame through
//! the caller's playout, which the loop calls: a host that holds the thread, or a step that blocks
//! the loop, at a time the case controls. The grid of the plan's pace is read from the
//! recogniser's first start, which `run_cascade_live` makes right before it starts its loop.

use std::num::NonZeroU32;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::task::{Context, Poll};
use std::thread;
use std::time::{Duration, Instant};

use inference_duplex::inference_audio::AudioFrame;
use inference_llm::Generator;
use inference_llm::llm_core::{
    self, BoxFuture, Cancel, Capabilities, Provenance, StreamEvent, StreamSink, TurnOutcome,
    TurnRequest,
};
use inference_stt::inference_core::model::{Adapter, ModelId};
use inference_stt::{ConcurrencyLimitReached as SttLimit, Recogniser, RecognitionRequest};
use inference_testkit::llm::{MockConfiguration as LlmConfiguration, MockGenerator};
use inference_testkit::stt::{
    Cue, MockConfiguration as SttConfiguration, MockRecogniser, ScriptStep,
};
use inference_testkit::tts::{
    MockConfiguration as TtsConfiguration, MockSynthesis, MockSynthesiser,
};
use inference_tts::inference_text::{
    AudioPlayed, OutputSegment, SegmentDiscarded, SegmentId, SegmentQueued,
};
use inference_tts::{
    ConcurrencyLimitReached as TtsLimit, DiscardRefused, OfferRefused, PlaybackReceipt,
    PlaybackRefused, SynthesisEvent, SynthesisHandle, SynthesisOutput, Synthesiser, UtteranceEnded,
};
use realtime_voice::compare::{Cascade, Clip, Plan, Playout, run_cascade, run_cascade_live};
use realtime_voice::pcm16;

const FRAME: Duration = Duration::from_millis(20);
const ATTACK: Duration = Duration::from_millis(30);
const HOLD: Duration = Duration::from_millis(200);
const DEADLINE: Duration = Duration::from_secs(20);
const BARGE_AFTER: Duration = Duration::from_millis(60);
/// The bound of the case under attack.
const LOW: Duration = Duration::from_millis(10);
const HIGH: Duration = Duration::from_millis(80);

fn within_deadline<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(work());
    });
    match receiver.recv_timeout(DEADLINE) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => panic!("the run did not end within {DEADLINE:?}"),
        Err(mpsc::RecvTimeoutError::Disconnected) => panic!("the case's thread panicked"),
    }
}

/// 200 ms of speech, then 100 ms of silence, at 24 kHz.
fn utterance() -> Clip {
    let mut samples = vec![0.25_f32; 4_800];
    samples.extend([0.0; 2_400]);
    Clip::new(samples, NonZeroU32::new(24_000).expect("non-zero"))
}

fn recogniser() -> MockRecogniser {
    MockRecogniser::new(
        SttConfiguration::Incremental,
        ModelId::generate(),
        vec![
            Cue {
                after_frames: 1,
                step: ScriptStep::Hypothesis("what is the capital of france".to_owned()),
            },
            Cue {
                after_frames: usize::MAX,
                step: ScriptStep::Commit { span: 0 },
            },
        ],
    )
}

const ANSWER: [&str; 2] = [
    "one two three four five six seven eight nine ten \
     one two three four five six seven eight nine ten \
     one two three four five six seven eight nine ten \
     one two three four five six seven eight nine ten",
    " and that is all.",
];

fn generator() -> Generator {
    MockGenerator::new(
        LlmConfiguration::StreamingOnly,
        ModelId::generate(),
        ANSWER.iter().copied(),
    )
    .into()
}

/// Where the run is on the wall clock: when the recogniser started each turn, when the
/// barge-in turn's answer first played, and the hold of the loop the playout made.
#[derive(Default)]
struct Timeline {
    starts: Vec<Instant>,
    first: Option<Instant>,
    /// The grid slot the hold was placed over, and when the hold began and ended.
    held: Option<(Instant, Instant, Instant)>,
}

type Shared = Arc<Mutex<Timeline>>;

fn lock(timeline: &Shared) -> std::sync::MutexGuard<'_, Timeline> {
    timeline.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The mock recogniser, noting when each turn starts.
struct Timed {
    inner: MockRecogniser,
    timeline: Shared,
}

impl Recogniser for Timed {
    type Handle = <MockRecogniser as Recogniser>::Handle;

    fn adapter(&self) -> &Adapter {
        self.inner.adapter()
    }

    fn start(&mut self, request: RecognitionRequest) -> Result<Self::Handle, SttLimit> {
        lock(&self.timeline).starts.push(Instant::now());
        self.inner.start(request)
    }
}

/// A playout that, in the barge-in turn, holds the arm's loop once: from a frame it plays in the
/// 19.5 ms before the grid slot the barge-in's first voiced frame is due at, after the slot
/// before it went out, to `lag` past that slot.
struct Holding {
    timeline: Shared,
    lag: Duration,
    stops: usize,
}

impl Playout for Holding {
    fn play(&mut self, _frame: &AudioFrame) {
        let now = Instant::now();
        let mut timeline = lock(&self.timeline);
        if timeline.starts.len() < 2 || timeline.held.is_some() {
            return;
        }
        let first = *timeline.first.get_or_insert(now);
        let grid = timeline.starts[0];
        let since = (first + BARGE_AFTER).duration_since(grid);
        let slots = since.as_nanos().div_ceil(FRAME.as_nanos());
        let slot = grid + FRAME * u32::try_from(slots).expect("a run of few frames");
        if now + Duration::from_micros(19_500) >= slot && now < slot {
            let until = slot + self.lag;
            timeline.held = Some((slot, now, until));
            drop(timeline);
            while Instant::now() < until {
                thread::sleep(until.saturating_duration_since(Instant::now()));
            }
        }
    }

    fn stop(&mut self) {
        self.stops += 1;
    }
}

/// A synthesiser over the mock at 8 kHz that blocks 8 ms before each frame, as the case under
/// attack builds it.
struct Streaming {
    inner: MockSynthesiser,
}

impl Synthesiser for Streaming {
    type Handle = Stream;

    fn adapter(&self) -> &Adapter {
        self.inner.adapter()
    }

    fn start(&mut self) -> Result<Stream, TtsLimit> {
        Ok(Stream {
            inner: self.inner.start()?,
        })
    }
}

struct Stream {
    inner: MockSynthesis,
}

impl Iterator for Stream {
    type Item = SynthesisEvent;

    fn next(&mut self) -> Option<SynthesisEvent> {
        let event = self.inner.next()?;
        if matches!(event, SynthesisEvent::Audio(_)) {
            thread::sleep(Duration::from_millis(8));
        }
        Some(event)
    }
}

impl SynthesisHandle for Stream {
    fn output(&self) -> &SynthesisOutput {
        self.inner.output()
    }

    fn offer_segment(&mut self, segment: OutputSegment) -> Result<SegmentQueued, OfferRefused> {
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

fn streaming() -> Streaming {
    Streaming {
        inner: MockSynthesiser::new(
            TtsConfiguration::IncrementalStreaming,
            ModelId::generate(),
            pcm16(8_000),
        ),
    }
}

struct WallDelay {
    delay: Duration,
    done: Arc<AtomicBool>,
    started: bool,
}

impl std::future::Future for WallDelay {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.done.load(Ordering::SeqCst) {
            return Poll::Ready(());
        }
        if !self.started {
            self.started = true;
            let done = Arc::clone(&self.done);
            let delay = self.delay;
            let waker = cx.waker().clone();
            thread::spawn(move || {
                thread::sleep(delay);
                done.store(true, Ordering::SeqCst);
                waker.wake();
            });
        }
        Poll::Pending
    }
}

struct SlowSink<'s> {
    inner: &'s mut dyn StreamSink,
    deltas: usize,
}

const GAP: Duration = Duration::from_millis(400);

impl StreamSink for SlowSink<'_> {
    fn emit(&mut self, event: StreamEvent) -> BoxFuture<'_, Result<(), llm_core::Error>> {
        let slow = if matches!(event, StreamEvent::TextDelta { .. }) {
            self.deltas += 1;
            self.deltas == 2
        } else {
            false
        };
        Box::pin(async move {
            if slow {
                WallDelay {
                    delay: GAP,
                    done: Arc::new(AtomicBool::new(false)),
                    started: false,
                }
                .await;
            }
            self.inner.emit(event).await
        })
    }
}

#[derive(Debug)]
struct SlowModel {
    inner: Generator,
}

impl llm_core::Model for SlowModel {
    fn provenance(&self) -> &Provenance {
        self.inner.model().provenance()
    }

    fn capabilities(&self) -> &Capabilities {
        self.inner.model().capabilities()
    }

    fn turn<'a>(
        &'a self,
        request: &'a TurnRequest,
        sink: &'a mut dyn StreamSink,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<TurnOutcome, llm_core::Error>> {
        Box::pin(async move {
            let mut slow = SlowSink {
                inner: sink,
                deltas: 0,
            };
            self.inner.model().turn(request, &mut slow, cancel).await
        })
    }
}

/// What a live run with its loop held measured.
struct Held {
    stops: usize,
    stop: Duration,
    late: Duration,
    since_due: Duration,
    hold: (Instant, Instant, Instant),
}

/// The case under attack's run, its loop held `lag` past the due time of the barge-in's first
/// voiced frame, its detector's attack `attack`.
fn held_run(attack: Duration, lag: Duration) -> Held {
    let (measured, stops, held) = within_deadline(move || {
        let timeline = Shared::default();
        let mock = generator();
        let generator = Generator::new(mock.adapter().clone(), Arc::new(SlowModel { inner: mock }))
            .expect("the wrapper declares what the mock declares");
        let timed = Timed {
            inner: recogniser(),
            timeline: Arc::clone(&timeline),
        };
        let mut cascade = Cascade::new(timed, generator, streaming(), pcm16(16_000));
        cascade.hold = HOLD;
        cascade.attack = attack;
        let mut plan = Plan::new(utterance());
        plan.barge_in = Some(utterance());
        plan.barge_after = BARGE_AFTER;
        plan.pause = Duration::from_millis(200);
        let mut playout = Holding {
            timeline: Arc::clone(&timeline),
            lag,
            stops: 0,
        };
        let measured = run_cascade_live(&mut cascade, &plan, &mut playout, |_| None);
        let held = lock(&timeline).held;
        (measured, playout.stops, held)
    });
    println!("{measured:?}");
    assert_eq!(measured.notes, Vec::<String>::new(), "{measured:?}");
    assert_eq!(stops, 1, "the playout stopped once");
    let hold = held.expect("precondition: the playout held the loop before the barge-in's slot");
    let stop = measured
        .barge_in_stop
        .expect("barge-in to stop was measured");
    let late = measured
        .barge_in_sent_late
        .expect("the barge-in's lag behind the plan's pace was measured");
    Held {
        stops,
        stop,
        late,
        since_due: stop + late,
        hold,
    }
}

fn describe(held: &Held) -> String {
    let (slot, from, until) = held.hold;
    format!(
        "{:?} from the due time ({:?} from the send, {:?} behind the pace); the loop was held \
         from {:?} before the slot to {:?} after it; stops {}",
        held.since_due,
        held.stop,
        held.late,
        slot.saturating_duration_since(from),
        until.saturating_duration_since(slot),
        held.stops,
    )
}

/// The mutant "the cut comes at the barge-in's first voiced frame, the attack ignored", expressed
/// as an attack of 1 ms that one frame meets, on a loop held 15 ms past that frame's due time, as
/// a loaded host holds it (the unit's own probe saw lags up to 31 ms alone). The case's old bound
/// took barge-in to stop from the send and rejected this mutant on any host (about 0 ms). The new
/// bound adds the lag and takes it: 15 ms is inside 10 to 80 ms. A runner that waits for the
/// attack cannot stop before the second voiced frame's due time, 20 ms after the first's, so a
/// lower bound of one frame rejects this mutant while the correct runner passes it (see the next
/// case).
#[test]
fn the_bound_rejects_a_cut_at_the_first_voiced_frame_on_a_loop_held_15ms() {
    let held = held_run(Duration::from_millis(1), Duration::from_millis(15));
    assert!(
        held.stop < Duration::from_millis(5),
        "precondition: the cut came at the first voiced frame: {}",
        describe(&held)
    );
    assert!(
        !(LOW..=HIGH).contains(&held.since_due),
        "a cut at the barge-in's first voiced frame, the attack of {ATTACK:?} ignored, passes the \
         case's bound of {LOW:?} to {HIGH:?}: {}",
        describe(&held)
    );
}

/// The control of the case above: the correct runner, its attack 30 ms, on the same 15 ms hold,
/// stops no sooner than one frame after the first voiced frame's due time.
#[test]
fn the_correct_runner_on_a_loop_held_15ms_stops_a_frame_after_the_due_time() {
    let held = held_run(ATTACK, Duration::from_millis(15));
    assert!(
        held.since_due >= FRAME && held.since_due <= HIGH,
        "the attack of {ATTACK:?} needs a second frame, due {FRAME:?} after the first: {}",
        describe(&held)
    );
}

/// A step that blocks the arm's loop for 40 ms at the barge-in, the correct attack: both voiced
/// frames go out in one burst, barge-in to stop is about 0 from the send, which the case's old
/// bound rejected. The new bound takes it: the hold is counted as the host's lag.
#[test]
#[ignore = "infeasible by timestamps: a step holding the loop 20-60 ms cannot be told from host lag (adversary pass 1, F2)"]
fn the_bound_rejects_a_step_that_holds_the_loop_40ms_at_the_barge_in() {
    let held = held_run(ATTACK, Duration::from_millis(40));
    assert!(
        !(LOW..=HIGH).contains(&held.since_due),
        "a loop held 40 ms at the barge-in sent both voiced frames in one burst and passes the \
         case's bound of {LOW:?} to {HIGH:?}: {}",
        describe(&held)
    );
}

/// On a simulated clock whose wait overshoots each due time by 7 ms, every frame goes out 7 ms
/// behind the plan's pace: the barge-in's lag is exactly 7 ms, and barge-in to stop from the send
/// exactly one frame.
#[test]
fn on_a_clock_that_overshoots_each_due_time_the_lag_is_the_overshoot() {
    const OVERSHOOT: Duration = Duration::from_millis(7);
    let measured = within_deadline(|| {
        let base = Instant::now();
        let offset = Arc::new(Mutex::new(Duration::ZERO));
        let now = {
            let offset = Arc::clone(&offset);
            move || base + *offset.lock().unwrap_or_else(PoisonError::into_inner)
        };
        let wait = {
            let offset = Arc::clone(&offset);
            move |by: Duration| {
                *offset.lock().unwrap_or_else(PoisonError::into_inner) += by + OVERSHOOT;
            }
        };
        let synthesiser = MockSynthesiser::new(
            TtsConfiguration::IncrementalStreaming,
            ModelId::generate(),
            pcm16(16_000),
        );
        let mut cascade = Cascade::new(recogniser(), generator(), synthesiser, pcm16(16_000));
        cascade.hold = HOLD;
        cascade.attack = ATTACK;
        let mut plan = Plan::new(utterance());
        plan.barge_in = Some(utterance());
        plan.barge_after = BARGE_AFTER;
        plan.pause = Duration::from_millis(200);
        let mut playout = Holding {
            timeline: Shared::default(),
            lag: Duration::ZERO,
            stops: 0,
        };
        run_cascade(&mut cascade, &plan, &now, &wait, &mut playout, |_| None)
    });
    assert_eq!(measured.notes, Vec::<String>::new(), "{measured:?}");
    assert_eq!(
        (measured.barge_in_sent_late, measured.barge_in_stop),
        (Some(OVERSHOOT), Some(FRAME)),
        "{measured:?}"
    );
}
