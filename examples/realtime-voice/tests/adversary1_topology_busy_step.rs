//! Adversary cases on the cascade arm of `compare`: its steps run one after the other on the
//! caller's thread, and input is paced by the same loop. While a step blocks, no frame is sent;
//! the frames due meanwhile go out afterwards in one burst, each stamped with the same instant.
//!
//! The case scripts an LLM that takes `GAP` to give its second segment, as a model producing the
//! next sentence while the first one plays does. A barge-in due while it works is stamped when
//! the LLM returns, and the detector's attack, which needs two frames of voiced input sent at
//! pace, is met within one instant: barge-in to stop reads as the playout's own stop time.
//!
//! The LLM's `GAP` is a future pending until the caller's clock reached it, woken by the clock's
//! timer when the arm moves the clock on, as a model awaiting its next token is. The LLM never
//! moves the clock itself: an arm that reads the clock while a step jumps it would see every
//! frame of the jump as late, whether or not it paces input during the step.
//!
//! Every time is on a fake clock moved only by the playout's stop and the arm's waits; nothing
//! sleeps, so nothing can hang on the wall clock.

use std::num::NonZeroU32;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use inference_duplex::inference_audio::AudioFrame;
use inference_llm::Generator;
use inference_llm::llm_core::{
    self, BoxFuture, Cancel, Capabilities, Provenance, StreamEvent, StreamSink, TurnOutcome,
    TurnRequest,
};
use inference_stt::inference_core::model::ModelId;
use inference_testkit::llm::{MockConfiguration as LlmConfiguration, MockGenerator};
use inference_testkit::stt::ScriptStep;
use inference_testkit::stt::{Cue, MockConfiguration as SttConfiguration, MockRecogniser};
use inference_testkit::tts::{MockConfiguration as TtsConfiguration, MockSynthesiser};
use realtime_voice::compare::{
    Cascade, Clip, Comparison, Measured, Plan, Playout, Topology, run_cascade,
};
use realtime_voice::pcm16;

const FRAME: Duration = Duration::from_millis(20);
const ATTACK: Duration = Duration::from_millis(30);
const HOLD: Duration = Duration::from_millis(200);
const STOP_DELAY: Duration = Duration::from_millis(10);
/// The time the LLM takes to give its second segment.
const GAP: Duration = Duration::from_millis(200);

/// Forty words: the mock synthesiser speaks one 10 ms frame per word, so 400 ms of answer, still
/// playing when the second segment comes.
const FIRST: &str = "one two three four five six seven eight nine ten \
                     one two three four five six seven eight nine ten \
                     one two three four five six seven eight nine ten \
                     one two three four five six seven eight nine ten";
const SECOND: &str = " and that is all.";

/// A clock that moves only when it is moved, with timers it wakes as it passes them.
#[derive(Debug, Clone)]
struct Fake {
    base: Instant,
    offset: Arc<Mutex<Duration>>,
    timers: Arc<Mutex<Vec<(Instant, Waker)>>>,
}

impl Fake {
    fn new() -> Self {
        Self {
            base: Instant::now(),
            offset: Arc::new(Mutex::new(Duration::ZERO)),
            timers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn now(&self) -> Instant {
        self.base + *self.offset.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn advance(&self, by: Duration) {
        let now = {
            let mut offset = self.offset.lock().unwrap_or_else(PoisonError::into_inner);
            *offset += by;
            self.base + *offset
        };
        let due: Vec<Waker> = {
            let mut timers = self.timers.lock().unwrap_or_else(PoisonError::into_inner);
            let (due, pending): (Vec<_>, Vec<_>) = timers.drain(..).partition(|(at, _)| *at <= now);
            *timers = pending;
            due.into_iter().map(|(_, waker)| waker).collect()
        };
        due.into_iter().for_each(Waker::wake);
    }

    /// Wakes `waker` once the clock reaches `at`.
    fn wake_at(&self, at: Instant, waker: &Waker) {
        self.timers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((at, waker.clone()));
    }
}

/// Pending until the fake clock reaches `at`, woken by its timer.
struct UntilClock {
    clock: Fake,
    at: Instant,
}

impl UntilClock {
    fn after(clock: &Fake, delay: Duration) -> Self {
        Self {
            clock: clock.clone(),
            at: clock.now() + delay,
        }
    }
}

impl std::future::Future for UntilClock {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.clock.now() >= self.at {
            Poll::Ready(())
        } else {
            self.clock.wake_at(self.at, cx.waker());
            Poll::Pending
        }
    }
}

/// Pending, waking itself, until the playout was handed a frame after `base`, or for at most
/// `MAX_YIELDS` polls: the turn yields, so the handle delivers what came before and the arm
/// speaks it. The bound keeps a handle that never delivers from spinning for ever.
struct UntilPlayed {
    played: Arc<AtomicUsize>,
    base: usize,
    polls: usize,
}

const MAX_YIELDS: usize = 64;

impl std::future::Future for UntilPlayed {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        self.polls += 1;
        if self.played.load(Ordering::SeqCst) > self.base || self.polls > MAX_YIELDS {
            Poll::Ready(())
        } else {
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

/// A sink that holds the second text delta back until the first is playing, then for `GAP` on
/// the fake clock.
struct SlowSink<'s> {
    inner: &'s mut dyn StreamSink,
    clock: Fake,
    deltas: usize,
    played: Arc<AtomicUsize>,
    base: usize,
}

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
                UntilPlayed {
                    played: Arc::clone(&self.played),
                    base: self.base,
                    polls: 0,
                }
                .await;
                UntilClock::after(&self.clock, GAP).await;
            }
            self.inner.emit(event).await
        })
    }
}

#[derive(Debug)]
struct SlowModel {
    inner: Generator,
    clock: Fake,
    played: Arc<AtomicUsize>,
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
        let clock = self.clock.clone();
        let played = Arc::clone(&self.played);
        let base = played.load(Ordering::SeqCst);
        Box::pin(async move {
            let mut slow = SlowSink {
                inner: sink,
                clock,
                deltas: 0,
                played,
                base,
            };
            self.inner.model().turn(request, &mut slow, cancel).await
        })
    }
}

struct StopPlayout {
    clock: Fake,
    stops: usize,
    played: Arc<AtomicUsize>,
}

impl Playout for StopPlayout {
    fn play(&mut self, _frame: &AudioFrame) {
        self.played.fetch_add(1, Ordering::SeqCst);
    }

    fn stop(&mut self) {
        self.stops += 1;
        self.clock.advance(STOP_DELAY);
    }
}

/// 200 ms of speech, then 100 ms of silence, at 24 kHz.
fn utterance() -> Clip {
    let mut samples = vec![0.25_f32; 4_800];
    samples.extend([0.0; 2_400]);
    Clip::new(samples, NonZeroU32::new(24_000).expect("non-zero"))
}

fn cascade_run(plan: &Plan) -> (Measured, usize) {
    let clock = Fake::new();
    let played = Arc::new(AtomicUsize::new(0));
    let recogniser = MockRecogniser::new(
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
    );
    let mock: Generator = MockGenerator::new(
        LlmConfiguration::StreamingOnly,
        ModelId::generate(),
        [FIRST, SECOND],
    )
    .into();
    let generator = Generator::new(
        mock.adapter().clone(),
        Arc::new(SlowModel {
            inner: mock,
            clock: clock.clone(),
            played: Arc::clone(&played),
        }),
    )
    .expect("the wrapper declares what the mock declares");
    let synthesiser = MockSynthesiser::new(
        TtsConfiguration::IncrementalStreaming,
        ModelId::generate(),
        pcm16(16_000),
    );
    let mut cascade = Cascade::new(recogniser, generator, synthesiser, pcm16(16_000));
    cascade.hold = HOLD;
    cascade.attack = ATTACK;
    let mut playout = StopPlayout {
        clock: clock.clone(),
        stops: 0,
        played,
    };
    let now = {
        let clock = clock.clone();
        move || clock.now()
    };
    let wait = move |by: Duration| clock.advance(by);
    let measured = run_cascade(&mut cascade, plan, &now, &wait, &mut playout, |_| None);
    (measured, playout.stops)
}

/// The wall-clock time a run of the case may take: the run is on a fake clock and takes
/// milliseconds, so an arm that spins or hangs fails the case instead of holding it.
const DEADLINE: std::time::Duration = std::time::Duration::from_secs(20);

/// Runs `work` on a thread of its own and returns its result, failing once `DEADLINE` passed.
fn within_deadline<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(work());
    });
    match receiver.recv_timeout(DEADLINE) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => panic!(
            "the cascade run did not end within {DEADLINE:?} of wall-clock time: the arm spins or \
             hangs while the LLM is pending on the fake clock"
        ),
        Err(mpsc::RecvTimeoutError::Disconnected) => panic!("the cascade run panicked"),
    }
}

/// The same plan as the unit's own barge-in case, which measures `FRAME + STOP_DELAY` with an
/// LLM that gives every segment at once. Whether the LLM is still busy on a later segment when
/// the barge-in comes must not change the figure: the barge-in's frames are sent at the plan's
/// pace, and the detector needs `ATTACK` of them, two frames, before the playout is stopped.
#[test]
fn barge_in_to_stop_does_not_shrink_while_the_llm_is_busy_on_its_next_segment() {
    let (cascade, stops) = within_deadline(|| {
        let mut plan = Plan::new(utterance());
        plan.barge_in = Some(utterance());
        plan.barge_after = Duration::from_millis(20);
        cascade_run(&plan)
    });
    println!("{cascade:?}");
    assert_eq!(cascade.notes, Vec::<String>::new(), "{cascade:?}");
    assert_eq!(stops, 1, "the playout stopped once");
    let stop = cascade
        .barge_in_stop
        .expect("barge-in to stop was measured");
    assert!(
        stop >= FRAME + STOP_DELAY && stop <= FRAME + STOP_DELAY + FRAME,
        "barge-in to stop {stop:?}: the detector's attack of {ATTACK:?} needs two frames sent at \
         the pace of {FRAME:?} before the stop of {STOP_DELAY:?}; with the LLM busy for {GAP:?} \
         the frames due meanwhile went out in one burst and the barge-in was stamped late"
    );
}

/// `--runs 0` is accepted by `compare`, and the cascade arm then hands `Comparison::ran` no run:
/// the row must still say `cascade`, as a skipped cascade row does.
#[test]
fn a_cascade_with_no_run_keeps_its_topology_in_the_table() {
    let mut comparison = Comparison::new();
    comparison.ran("cascade", Vec::new());
    assert_eq!(
        comparison.topology("cascade"),
        Some(Topology::Cascade),
        "the cascade arm with zero runs is labelled {:?}:\n{comparison}",
        comparison.topology("cascade")
    );
}
