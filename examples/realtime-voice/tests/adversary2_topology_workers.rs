//! Adversary cases on the worker threads of the cascade arm of `compare`: a failure on the arm's
//! loop or in a step, a step that blocks while it speaks, and the wall-clock pacing of
//! `run_cascade_live`.
//!
//! Every case runs the arm on a thread of its own under a wall-clock deadline, so an arm that
//! hangs fails the case instead of holding the test binary.

use std::num::NonZeroU32;
use std::panic::{AssertUnwindSafe, catch_unwind};
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
use inference_stt::inference_text::SegmentId;
use inference_testkit::llm::{MockConfiguration as LlmConfiguration, MockGenerator};
use inference_testkit::stt::ScriptStep;
use inference_testkit::stt::{Cue, MockConfiguration as SttConfiguration, MockRecogniser};
use inference_testkit::tts::{
    MockConfiguration as TtsConfiguration, MockSynthesis, MockSynthesiser,
};
use inference_tts::inference_text::{AudioPlayed, OutputSegment, SegmentDiscarded, SegmentQueued};
use inference_tts::{
    ConcurrencyLimitReached as TtsLimit, DiscardRefused, OfferRefused, PlaybackReceipt,
    PlaybackRefused, SynthesisEvent, SynthesisHandle, SynthesisOutput, Synthesiser, UtteranceEnded,
};
use realtime_voice::compare::{
    Cascade, Clip, Measured, Plan, Playout, run_cascade, run_cascade_live,
};
use realtime_voice::pcm16;

const FRAME: Duration = Duration::from_millis(20);
const ATTACK: Duration = Duration::from_millis(30);
const HOLD: Duration = Duration::from_millis(200);
/// The scripted time from a segment's offer to its first audio, on the caller's clock.
const TTS_DELAY: Duration = Duration::from_millis(90);
/// The wall-clock time a case may take.
const DEADLINE: Duration = Duration::from_secs(20);

/// Runs `work` on a thread of its own, failing once `DEADLINE` passed.
fn within_deadline<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(work());
    });
    match receiver.recv_timeout(DEADLINE) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            panic!("the cascade run did not end within {DEADLINE:?} of wall-clock time: it hangs")
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => panic!("the case's thread panicked"),
    }
}

/// A clock that moves only when it is moved, shared across threads.
#[derive(Debug, Clone)]
struct Fake {
    base: Instant,
    offset: Arc<Mutex<Duration>>,
}

impl Fake {
    fn new() -> Self {
        Self {
            base: Instant::now(),
            offset: Arc::new(Mutex::new(Duration::ZERO)),
        }
    }

    fn now(&self) -> Instant {
        self.base + *self.offset.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn advance(&self, by: Duration) {
        *self.offset.lock().unwrap_or_else(PoisonError::into_inner) += by;
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

fn generator(answer: &[&'static str]) -> Generator {
    MockGenerator::new(
        LlmConfiguration::StreamingOnly,
        ModelId::generate(),
        answer.iter().copied(),
    )
    .into()
}

fn mock_synthesiser() -> MockSynthesiser {
    MockSynthesiser::new(
        TtsConfiguration::IncrementalStreaming,
        ModelId::generate(),
        pcm16(16_000),
    )
}

const ANSWER: [&str; 2] = [
    "one two three four five six seven eight nine ten \
     one two three four five six seven eight nine ten \
     one two three four five six seven eight nine ten \
     one two three four five six seven eight nine ten",
    " and that is all.",
];

/// A synthesiser over the testkit's mock, each handle built by `wrap`.
struct Wrapped<F> {
    inner: MockSynthesiser,
    wrap: F,
}

impl<F: FnMut(MockSynthesis) -> Wrapping> Synthesiser for Wrapped<F> {
    type Handle = Wrapping;

    fn adapter(&self) -> &Adapter {
        self.inner.adapter()
    }

    fn start(&mut self) -> Result<Wrapping, TtsLimit> {
        Ok((self.wrap)(self.inner.start()?))
    }
}

/// What a wrapped synthesis does besides its mock.
enum Behaviour {
    /// Panics at its first segment.
    PanicOnOffer,
    /// Blocks its first audio until the caller's clock is `TTS_DELAY` past the first offer, as a
    /// hosted synthesiser blocks while it speaks a segment.
    BlockOnClock {
        clock: Fake,
        offered: Option<Instant>,
    },
    /// Blocks `per_frame` of wall-clock time before each frame, as a hosted synthesiser streams.
    Stream { per_frame: Duration },
}

struct Wrapping {
    inner: MockSynthesis,
    behaviour: Behaviour,
    audio: bool,
}

impl Iterator for Wrapping {
    type Item = SynthesisEvent;

    fn next(&mut self) -> Option<SynthesisEvent> {
        let event = self.inner.next()?;
        if matches!(event, SynthesisEvent::Audio(_)) {
            match &self.behaviour {
                Behaviour::BlockOnClock {
                    clock,
                    offered: Some(offered),
                } if !self.audio => {
                    let due = *offered + TTS_DELAY;
                    let give_up = Instant::now() + DEADLINE + Duration::from_secs(5);
                    while clock.now() < due && Instant::now() < give_up {
                        thread::sleep(Duration::from_millis(1));
                    }
                }
                Behaviour::Stream { per_frame } => thread::sleep(*per_frame),
                _ => {}
            }
            self.audio = true;
        }
        Some(event)
    }
}

impl SynthesisHandle for Wrapping {
    fn output(&self) -> &SynthesisOutput {
        self.inner.output()
    }

    fn offer_segment(&mut self, segment: OutputSegment) -> Result<SegmentQueued, OfferRefused> {
        match &mut self.behaviour {
            Behaviour::PanicOnOffer => panic!("adversary2: the synthesiser failed"),
            Behaviour::BlockOnClock { clock, offered } => {
                offered.get_or_insert_with(|| clock.now());
            }
            Behaviour::Stream { .. } => {}
        }
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

/// A playout that fails at its first frame, as a device playout whose stream broke may.
struct FailingPlayout;

impl Playout for FailingPlayout {
    fn play(&mut self, _frame: &AudioFrame) {
        panic!("adversary2: the playout failed");
    }

    fn stop(&mut self) {}
}

/// A playout that counts its stops.
#[derive(Default)]
struct Counting {
    stops: usize,
}

impl Playout for Counting {
    fn play(&mut self, _frame: &AudioFrame) {}

    fn stop(&mut self) {
        self.stops += 1;
    }
}

fn simulated<S>(synthesiser: S, answer: &[&'static str], playout: &mut dyn Playout) -> Measured
where
    S: Synthesiser,
    S::Handle: Send,
{
    let clock = Fake::new();
    simulated_on(&clock, synthesiser, answer, playout)
}

fn simulated_on<S>(
    clock: &Fake,
    synthesiser: S,
    answer: &[&'static str],
    playout: &mut dyn Playout,
) -> Measured
where
    S: Synthesiser,
    S::Handle: Send,
{
    let mut cascade = Cascade::new(recogniser(), generator(answer), synthesiser, pcm16(16_000));
    cascade.hold = HOLD;
    cascade.attack = ATTACK;
    let now = {
        let clock = clock.clone();
        move || clock.now()
    };
    let wait = {
        let clock = clock.clone();
        move |by: Duration| clock.advance(by)
    };
    run_cascade(&mut cascade, &Plan::new(utterance()), &now, &wait, playout)
}

/// A playout is the caller's code and may fail. Before the worker threads, a panic in it
/// unwound out of `run_cascade` to its caller; it must still do so, not leave the arm's
/// thread scope waiting on a generator worker that nothing tells to stop.
#[test]
fn a_playout_that_panics_fails_the_simulated_run_instead_of_hanging_it() {
    let panicked = within_deadline(|| {
        catch_unwind(AssertUnwindSafe(|| {
            simulated(mock_synthesiser(), &ANSWER, &mut FailingPlayout)
        }))
        .is_err()
    });
    assert!(
        panicked,
        "a playout that panics made run_cascade return instead of panicking"
    );
}

/// A synthesiser that panics on its worker: the run ends and the panic reaches the caller.
#[test]
fn a_synthesiser_that_panics_on_its_worker_fails_the_simulated_run() {
    let panicked = within_deadline(|| {
        catch_unwind(AssertUnwindSafe(|| {
            let synthesiser = Wrapped {
                inner: mock_synthesiser(),
                wrap: |inner| Wrapping {
                    inner,
                    behaviour: Behaviour::PanicOnOffer,
                    audio: false,
                },
            };
            simulated(synthesiser, &ANSWER, &mut Counting::default())
        }))
        .is_err()
    });
    assert!(
        panicked,
        "a synthesiser that panics was swallowed by run_cascade"
    );
}

/// A hosted synthesiser blocks while it speaks a segment (the ElevenLabs adapter's `next`
/// does); a fake of it on the caller's simulated clock blocks until that clock reaches the
/// segment's audio. The simulated arm moves its clock only once no worker owes a report, and the
/// blocked synthesiser owes one, so neither moves: the run must still end, or `run_cascade` must
/// say that a step blocking on the caller's clock is outside what it runs.
#[test]
fn a_synthesiser_that_blocks_on_the_callers_clock_does_not_hang_the_simulated_run() {
    let measured = within_deadline(|| {
        let clock = Fake::new();
        let synthesiser = Wrapped {
            inner: mock_synthesiser(),
            wrap: {
                let clock = clock.clone();
                move |inner| Wrapping {
                    inner,
                    behaviour: Behaviour::BlockOnClock {
                        clock: clock.clone(),
                        offered: None,
                    },
                    audio: false,
                }
            },
        };
        simulated_on(&clock, synthesiser, &ANSWER, &mut Counting::default())
    });
    assert_eq!(measured.notes, Vec::<String>::new(), "{measured:?}");
    let (Some(token), Some(audio)) = (measured.first_token, measured.tts_first_audio) else {
        panic!("first token and TTS first audio measured: {measured:?}");
    };
    let step = audio.saturating_sub(token);
    assert!(
        step >= TTS_DELAY && step <= TTS_DELAY + FRAME,
        "TTS first audio {step:?} after the first token, scripted {TTS_DELAY:?}"
    );
}

/// Pending until `delay` of wall-clock time passed, woken by a thread of its own.
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

/// A sink that holds the second text delta back for `GAP` of wall-clock time.
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

/// On the wall clock, with an LLM working on its second segment and a synthesiser that blocks
/// while it streams each frame, the barge-in's frames go out at the plan's pace: the detector's
/// attack of two frames is met one frame after the barge-in's first voiced frame, so barge-in to
/// stop is about one frame, not the zero of a burst.
#[test]
fn the_live_arm_paces_the_barge_in_while_the_llm_and_the_synthesiser_block() {
    let (measured, stops) = within_deadline(|| {
        let mock = generator(&ANSWER);
        let generator = Generator::new(mock.adapter().clone(), Arc::new(SlowModel { inner: mock }))
            .expect("the wrapper declares what the mock declares");
        let synthesiser = Wrapped {
            inner: mock_synthesiser(),
            wrap: |inner| Wrapping {
                inner,
                behaviour: Behaviour::Stream {
                    per_frame: Duration::from_millis(8),
                },
                audio: false,
            },
        };
        let mut cascade = Cascade::new(recogniser(), generator, synthesiser, pcm16(16_000));
        cascade.hold = HOLD;
        cascade.attack = ATTACK;
        let mut plan = Plan::new(utterance());
        plan.barge_in = Some(utterance());
        plan.barge_after = Duration::from_millis(60);
        plan.pause = Duration::from_millis(200);
        let mut playout = Counting::default();
        let measured = run_cascade_live(&mut cascade, &plan, &mut playout);
        (measured, playout.stops)
    });
    println!("{measured:?}");
    assert_eq!(measured.notes, Vec::<String>::new(), "{measured:?}");
    assert_eq!(stops, 1, "the playout stopped once");
    let stop = measured
        .barge_in_stop
        .expect("barge-in to stop was measured");
    assert!(
        stop >= Duration::from_millis(10) && stop <= Duration::from_millis(80),
        "barge-in to stop {stop:?}: the attack of {ATTACK:?} needs two frames of {FRAME:?} sent \
         at pace"
    );
}
