//! The microphone played back through the speaker with a fixed delay: what the `mic-loopback`
//! example runs on the cpal host, and the tests on the [`FakeHost`](crate::fake::FakeHost).
//!
//! [`run`] checks that the delay and a captured frame fit the speaker's queue, opens a
//! [`Microphone`] and a [`Speaker`] by selector, queues the delay as frames of silence, then
//! forwards each captured frame to the speaker, renumbered on one sample clock that continues
//! where the delay ends. A frame the speaker refuses keeps its place on that clock, so the
//! speaker's position less the delay is always the microphone's own position, and the refused
//! samples are a gap the speaker skips. The loop stops when the run has elapsed or the stop flag
//! is set, both looked at between frames of the delay too, or after a given number of captured
//! frames, whichever comes first; then it [finishes](Speaker::finish) the speaker, waits for it to
//! play out, and closes both devices. What happened on the way reaches the caller as
//! [`LoopbackEvent`]s, and the end as a [`LoopbackReport`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use inference_audio::{AudioFrame, SampleClockPosition};

use crate::capture::{CaptureConfig, CaptureStats, Microphone, samples_in};
use crate::convert::encode;
use crate::host::{AudioHost, DeviceError, DeviceSelector, ResolvedDevice};
use crate::playout::{PlayoutConfig, PlayoutStats, PushError, Speaker};

/// How long the loop waits for the microphone's next frame, and for room in the speaker's queue
/// for a frame, before it looks at the clock and the stop flag again.
const PATIENCE: Duration = Duration::from_millis(100);

/// What a loop opens and how long it runs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LoopbackConfig {
    /// The microphone.
    pub input: DeviceSelector,
    /// The speaker.
    pub output: DeviceSelector,
    /// The frames the microphone produces, which the speaker plays.
    pub capture: CaptureConfig,
    /// The speaker's queue; a captured frame must fit it.
    pub playout: PlayoutConfig,
    /// The silence queued before the first captured frame: `delay` at the capture rate, rounded
    /// up to a whole sample, then truncated to whole frames, so it comes out short by less than
    /// one frame. It must be shorter than `playout.buffer`, and its whole frames must fit the
    /// speaker's queue, or the loop would wait on the speaker while nothing reads the microphone.
    pub delay: Duration,
    /// How long the loop runs at most, the delay included.
    pub run: Duration,
    /// The most frames the loop captures, if any limit.
    pub frames: Option<u64>,
    /// How often the loop reports [`LoopbackEvent::Progress`], checked after each captured frame.
    pub progress: Duration,
    /// How long the loop waits, once it stops forwarding, for the speaker to play out.
    pub drain: Duration,
}

impl Default for LoopbackConfig {
    /// The default microphone to the default speaker in the default capture and playout configs,
    /// half a second behind, for twenty seconds, reporting progress every second and waiting a
    /// second at the end for the speaker to play out.
    fn default() -> Self {
        Self {
            input: DeviceSelector::Default,
            output: DeviceSelector::Default,
            capture: CaptureConfig::default(),
            playout: PlayoutConfig::default(),
            delay: Duration::from_millis(500),
            run: Duration::from_secs(20),
            frames: None,
            progress: Duration::from_secs(1),
            drain: Duration::from_secs(1),
        }
    }
}

/// Why a loop stopped forwarding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ended {
    /// The run elapsed.
    Elapsed,
    /// The stop flag was set.
    Stopped,
    /// The loop captured as many frames as its limit.
    Frames,
}

/// What a loop reports while it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopbackEvent {
    /// Both devices are open.
    Opened {
        microphone: ResolvedDevice,
        speaker: ResolvedDevice,
    },
    /// A frame was queued on the speaker: one of the delay's silence, or a captured one.
    Pushed {
        position: SampleClockPosition,
        samples: u32,
        delay: bool,
    },
    /// The speaker refused a frame, which is not played but keeps its place on the clock: one of
    /// the delay's silence, or a captured one.
    Refused {
        position: SampleClockPosition,
        error: PushError,
        delay: bool,
    },
    /// What both devices have done so far, and how many samples the speaker has queued.
    Progress {
        capture: CaptureStats,
        playout: PlayoutStats,
        queued: usize,
    },
    /// The loop stopped forwarding and waits for the speaker to play out the samples queued.
    Draining { ended: Ended, queued: usize },
}

/// What a loop did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopbackReport {
    /// The device captured from.
    pub microphone: ResolvedDevice,
    /// The device played on.
    pub speaker: ResolvedDevice,
    /// The samples of silence the delay took on the clock, refused ones included.
    pub delay_samples: u64,
    /// The frames of the delay the speaker refused.
    pub delay_refused: u64,
    /// The frames the microphone produced.
    pub captured: u64,
    /// The captured frames the speaker queued.
    pub forwarded: u64,
    /// The captured frames the speaker refused.
    pub refused: u64,
    /// Why the loop stopped forwarding.
    pub ended: Ended,
    /// Whether the speaker played out everything queued within the drain.
    pub played_out: bool,
    /// How much the speaker played, delay included: its played position less the samples of
    /// refused frames before it, which it never played.
    pub played: Option<SampleClockPosition>,
    /// The speaker's own played position on the loop's clock ([`Speaker::played`]), refused
    /// frames' gaps included: the delay plus the microphone's position.
    pub speaker_played: Option<SampleClockPosition>,
    /// What the microphone did.
    pub capture: CaptureStats,
    /// What the speaker did.
    pub playout: PlayoutStats,
}

/// Plays the microphone `config.input` selects on `host` back through the speaker
/// `config.output` selects, `config.delay` behind, until the run elapses, `stop` is set or
/// `config.frames` frames were captured; see the module documentation. Every event goes to
/// `observe` on the calling thread. Both devices are closed when it returns.
///
/// # Errors
///
/// [`DeviceError::InvalidConfig`], before any device opens, when the capture config makes empty
/// frames, a captured frame is longer than the speaker's queue, or the delay is not shorter than
/// the queue or its whole frames do not fit it; otherwise what the host answered when either
/// device does not open.
pub fn run<H: AudioHost>(
    host: &H,
    config: &LoopbackConfig,
    stop: &AtomicBool,
    mut observe: impl FnMut(LoopbackEvent),
) -> Result<LoopbackReport, DeviceError> {
    let frame_samples = config.capture.frame_samples()?;
    let delay_frames = delay_frames(config, frame_samples)?;
    let started = Instant::now();
    let mut microphone = Microphone::open(host, &config.input, config.capture)?;
    let speaker = Speaker::open(host, &config.output, config.playout)?;
    observe(LoopbackEvent::Opened {
        microphone: microphone.device().clone(),
        speaker: speaker.device().clone(),
    });
    let ended_by = |captured: Option<u64>| {
        if stop.load(Ordering::Relaxed) {
            Some(Ended::Stopped)
        } else if captured.is_some_and(|captured| config.frames.is_some_and(|n| captured >= n)) {
            Some(Ended::Frames)
        } else if started.elapsed() >= config.run {
            Some(Ended::Elapsed)
        } else {
            None
        }
    };

    let format = microphone.format();
    let silence = encode(&vec![0.0; frame_samples as usize], format.encoding);
    let mut next = 0_u64;
    let mut gaps: Vec<(u64, u64)> = Vec::new();
    let mut delay_refused = 0_u64;
    let mut ended_in_delay = None;
    for _ in 0..delay_frames {
        ended_in_delay = ended_by(None);
        if ended_in_delay.is_some() {
            break;
        }
        let frame = AudioFrame {
            format,
            position: clock(next),
            sample_count: frame_samples,
            data: silence.clone(),
        };
        observe(match speaker.push(&frame, PATIENCE) {
            Ok(()) => LoopbackEvent::Pushed {
                position: frame.position,
                samples: frame_samples,
                delay: true,
            },
            Err(error) => {
                delay_refused += 1;
                gaps.push((next, u64::from(frame_samples)));
                LoopbackEvent::Refused {
                    position: frame.position,
                    error,
                    delay: true,
                }
            }
        });
        next += u64::from(frame_samples);
    }
    let delay_samples = next;

    let (mut captured, mut forwarded, mut refused) = (0_u64, 0_u64, 0_u64);
    let mut reported = Instant::now();
    let ended = loop {
        if let Some(ended) = ended_in_delay.or_else(|| ended_by(Some(captured))) {
            break ended;
        }
        let Some(mut frame) = microphone.next_frame(PATIENCE) else {
            continue;
        };
        captured += 1;
        frame.position = clock(next);
        observe(match speaker.push(&frame, PATIENCE) {
            Ok(()) => {
                forwarded += 1;
                LoopbackEvent::Pushed {
                    position: frame.position,
                    samples: frame.sample_count,
                    delay: false,
                }
            }
            Err(error) => {
                refused += 1;
                gaps.push((next, u64::from(frame.sample_count)));
                LoopbackEvent::Refused {
                    position: frame.position,
                    error,
                    delay: false,
                }
            }
        });
        next += u64::from(frame.sample_count);
        if reported.elapsed() >= config.progress {
            reported = Instant::now();
            observe(LoopbackEvent::Progress {
                capture: microphone.stats(),
                playout: speaker.stats(),
                queued: speaker.queued(),
            });
        }
    };

    speaker.finish();
    observe(LoopbackEvent::Draining {
        ended,
        queued: speaker.queued(),
    });
    let played_out = speaker.wait_played(config.drain);
    let speaker_played = speaker.played();
    Ok(LoopbackReport {
        microphone: microphone.device().clone(),
        speaker: speaker.device().clone(),
        delay_samples,
        delay_refused,
        captured,
        forwarded,
        refused,
        ended,
        played_out,
        played: speaker_played.map(|position| clock(unskipped(position.get(), &gaps))),
        speaker_played,
        capture: microphone.stats(),
        playout: speaker.stats(),
    })
}

/// The whole frames of silence the delay of `config` takes, frames of `frame_samples`.
///
/// # Errors
///
/// [`DeviceError::InvalidConfig`] when a frame or the delay does not fit the speaker's queue.
fn delay_frames(config: &LoopbackConfig, frame_samples: u32) -> Result<u64, DeviceError> {
    let rate = config.capture.sample_rate.get();
    let capacity = samples_in(config.playout.buffer, rate).max(1);
    let frame_samples = u128::from(frame_samples);
    if frame_samples > capacity {
        return Err(DeviceError::InvalidConfig(format!(
            "a captured frame of {frame_samples} samples is longer than the playout queue of \
             {capacity} ({:?} at {rate} Hz)",
            config.playout.buffer
        )));
    }
    if config.delay >= config.playout.buffer {
        return Err(DeviceError::InvalidConfig(format!(
            "a delay of {:?} is not shorter than the playout queue of {:?}",
            config.delay, config.playout.buffer
        )));
    }
    let samples = (config.delay.as_nanos() * u128::from(rate)).div_ceil(1_000_000_000);
    let frames = samples / frame_samples;
    if frames * frame_samples > capacity {
        return Err(DeviceError::InvalidConfig(format!(
            "a delay of {:?} takes {frames} frames of {frame_samples} samples, more than the \
             playout queue of {capacity} holds",
            config.delay
        )));
    }
    Ok(u64::try_from(frames).unwrap_or(u64::MAX))
}

/// `position` less the samples of every gap, a refused frame's `(position, samples)`, before it.
fn unskipped(position: u64, gaps: &[(u64, u64)]) -> u64 {
    let skipped: u64 = gaps
        .iter()
        .map(|&(start, samples)| samples.min(position.saturating_sub(start)))
        .sum();
    position.saturating_sub(skipped)
}

/// The position `index` on a sample clock, saturated.
fn clock(index: u64) -> SampleClockPosition {
    SampleClockPosition::new(index).unwrap_or(SampleClockPosition::MAX)
}
