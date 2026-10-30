//! The microphone played back through the speaker with a fixed delay: what the `mic-loopback`
//! example runs on the cpal host, and the tests on the [`FakeHost`](crate::fake::FakeHost).
//!
//! [`run`] opens a [`Microphone`] and a [`Speaker`] by selector, queues the delay as frames of
//! silence, then forwards each captured frame to the speaker, renumbered on the speaker's own
//! sample clock so that it continues where the delay ends. It stops when the run has elapsed, when
//! the stop flag is set, or after a given number of captured frames, whichever comes first; then
//! it [finishes](Speaker::finish) the speaker, waits for it to play out, and closes both devices.
//! What happened on the way reaches the caller as [`LoopbackEvent`]s, and the end as a
//! [`LoopbackReport`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use inference_audio::{AudioFrame, SampleClockPosition};

use crate::capture::{CaptureConfig, CaptureStats, Microphone};
use crate::convert::encode;
use crate::host::{AudioHost, DeviceError, DeviceSelector, ResolvedDevice};
use crate::playout::{PlayoutConfig, PlayoutStats, PushError, Speaker};

/// How long the loop waits for the microphone's next frame before it looks at the clock and the
/// stop flag again, and for room in the speaker's queue for a captured frame.
const PATIENCE: Duration = Duration::from_millis(100);

/// How long the loop waits for room in the speaker's queue for a frame of the delay.
const DELAY_PATIENCE: Duration = Duration::from_secs(1);

/// What a loop opens and how long it runs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LoopbackConfig {
    /// The microphone.
    pub input: DeviceSelector,
    /// The speaker.
    pub output: DeviceSelector,
    /// The frames the microphone produces, which the speaker plays.
    pub capture: CaptureConfig,
    /// The speaker's queue.
    pub playout: PlayoutConfig,
    /// The silence queued before the first captured frame, in whole frames of `capture.frame`,
    /// truncated.
    pub delay: Duration,
    /// How long the loop forwards frames at most.
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
    /// The speaker refused a frame, which is not played: one of the delay's silence, or a
    /// captured one.
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
    /// The samples of silence the delay queued, refused ones included.
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
    /// The speaker's played position when the loop ended.
    pub played: Option<SampleClockPosition>,
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
/// [`DeviceError`] when either device does not open, or the capture config makes empty frames.
pub fn run<H: AudioHost>(
    host: &H,
    config: &LoopbackConfig,
    stop: &AtomicBool,
    mut observe: impl FnMut(LoopbackEvent),
) -> Result<LoopbackReport, DeviceError> {
    let frame_samples = config.capture.frame_samples()?;
    let mut microphone = Microphone::open(host, &config.input, config.capture)?;
    let speaker = Speaker::open(host, &config.output, config.playout)?;
    observe(LoopbackEvent::Opened {
        microphone: microphone.device().clone(),
        speaker: speaker.device().clone(),
    });

    let format = microphone.format();
    let delay_frames = config.delay.as_nanos() / config.capture.frame.as_nanos().max(1);
    let silence = encode(&vec![0.0; frame_samples as usize], format.encoding);
    let mut next = 0_u64;
    let mut delay_refused = 0_u64;
    for _ in 0..delay_frames {
        let frame = AudioFrame {
            format,
            position: clock(next),
            sample_count: frame_samples,
            data: silence.clone(),
        };
        observe(match speaker.push(&frame, DELAY_PATIENCE) {
            Ok(()) => LoopbackEvent::Pushed {
                position: frame.position,
                samples: frame_samples,
                delay: true,
            },
            Err(error) => {
                delay_refused += 1;
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
    let started = Instant::now();
    let mut reported = Instant::now();
    let ended = loop {
        if stop.load(Ordering::Relaxed) {
            break Ended::Stopped;
        }
        if config.frames.is_some_and(|limit| captured >= limit) {
            break Ended::Frames;
        }
        if started.elapsed() >= config.run {
            break Ended::Elapsed;
        }
        let Some(mut frame) = microphone.next_frame(PATIENCE) else {
            continue;
        };
        captured += 1;
        frame.position = clock(next);
        next += u64::from(frame.sample_count);
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
                LoopbackEvent::Refused {
                    position: frame.position,
                    error,
                    delay: false,
                }
            }
        });
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
        played: speaker.played(),
        capture: microphone.stats(),
        playout: speaker.stats(),
    })
}

/// The position `index` on a sample clock, saturated.
fn clock(index: u64) -> SampleClockPosition {
    SampleClockPosition::new(index).unwrap_or(SampleClockPosition::MAX)
}
