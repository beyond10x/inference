//! Adversary pass 2 against the loopback's correction: the up-front refusals at their boundaries
//! (a delay that fills the speaker's queue exactly, a frame as long as the queue and one sample
//! longer, a zero queue), the stop and run looked at between frames of the delay and not only
//! before the first, the run clock including the delay, the longest accepted delay on devices
//! playing in real time, and `played` with several refused frames and with a refused frame after
//! the speaker's played position (`story:audio-device-adapter`, Planned scope: the `mic-loopback`
//! example).

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use inference_audio::SampleClockPosition;
use inference_audio_device::fake::{FakeDevice, FakeHost};
use inference_audio_device::loopback::{
    self, Ended, LoopbackConfig, LoopbackEvent, LoopbackReport,
};
use inference_audio_device::{CaptureConfig, DeviceError, PlayoutConfig};

/// How long a test waits for the loop's next event before it fails.
const PATIENCE: Duration = Duration::from_secs(10);

fn hz(rate: u32) -> NonZeroU32 {
    NonZeroU32::new(rate).expect("a rate above 0")
}

fn channels(count: u16) -> NonZeroU16 {
    NonZeroU16::new(count).expect("a channel count above 0")
}

fn position(index: u64) -> SampleClockPosition {
    SampleClockPosition::new(index).expect("a position")
}

/// A host whose default input and output are mono devices at 16 kHz.
fn mono() -> FakeHost {
    FakeHost::new(vec![
        FakeDevice::input("line in", hz(16_000), channels(1), true),
        FakeDevice::output("line out", hz(16_000), channels(1), true),
    ])
}

/// A loop at the default 16 kHz, 20 ms frames, ending after `frames` frames, never by its run.
fn config(frames: u64) -> LoopbackConfig {
    LoopbackConfig {
        frames: Some(frames),
        run: Duration::from_secs(60),
        drain: PATIENCE,
        ..LoopbackConfig::default()
    }
}

/// One-sample frames at 16 kHz.
fn one_sample() -> CaptureConfig {
    CaptureConfig {
        frame: Duration::from_nanos(62_500),
        ..CaptureConfig::default()
    }
}

type Running = thread::JoinHandle<Result<LoopbackReport, DeviceError>>;

fn start(
    host: &FakeHost,
    config: LoopbackConfig,
    stop: &Arc<AtomicBool>,
) -> (Running, Receiver<LoopbackEvent>) {
    let (events, received) = mpsc::channel();
    let host = host.clone();
    let stop = Arc::clone(stop);
    let handle = thread::spawn(move || {
        loopback::run(&host, &config, &stop, |event| {
            let _sent = events.send(event);
        })
    });
    (handle, received)
}

fn until(
    received: &Receiver<LoopbackEvent>,
    seen: &mut Vec<LoopbackEvent>,
    wanted: impl Fn(&LoopbackEvent) -> bool,
) {
    loop {
        let event = received
            .recv_timeout(PATIENCE)
            .expect("the loop reports its next event in time");
        let found = wanted(&event);
        seen.push(event);
        if found {
            return;
        }
    }
}

/// `count` samples, each exactly one PCM16 step, none of them zero.
fn signal(count: usize) -> Vec<f32> {
    (0..count)
        .map(|n| {
            let step = i16::try_from((n * 37) % 20_000).expect("a step") - 10_001;
            f32::from(step) / 32_767.0
        })
        .collect()
}

/// The positions of the frames pushed (`delay` picks the delay's or the captured ones).
fn pushed_positions(seen: &[LoopbackEvent], delay: bool) -> Vec<u64> {
    seen.iter()
        .filter_map(|event| match event {
            LoopbackEvent::Pushed {
                position,
                delay: is_delay,
                ..
            } if *is_delay == delay => Some(position.get()),
            _ => None,
        })
        .collect()
}

/// The positions of the captured frames the speaker refused.
fn refused_positions(seen: &[LoopbackEvent]) -> Vec<u64> {
    seen.iter()
        .filter_map(|event| match event {
            LoopbackEvent::Refused {
                position,
                delay: false,
                ..
            } => Some(position.get()),
            _ => None,
        })
        .collect()
}

fn is_refused_at(event: &LoopbackEvent, at: u64) -> bool {
    matches!(event, LoopbackEvent::Refused { position, .. } if position.get() == at)
}

// --- The up-front refusals at their boundaries ---------------------------------------------

/// The doc on `delay` asks that its whole frames fit the speaker's queue; a delay whose frames
/// fill the queue exactly fits. A refusal at `>=` instead of `>` would refuse these, and no case
/// of the unit's suite holds a delay that fills the queue exactly.
#[test]
fn a_delay_whose_whole_frames_fill_the_speaker_queue_exactly_is_accepted() {
    let cases = [
        (
            "25 frames of 320 for a queue of 8000",
            LoopbackConfig {
                delay: Duration::from_nanos(499_999_999),
                playout: PlayoutConfig {
                    buffer: Duration::from_millis(500),
                },
                drain: Duration::from_millis(10),
                ..config(0)
            },
            8_000,
        ),
        (
            "one frame of 320 for a queue of 320",
            LoopbackConfig {
                delay: Duration::from_nanos(19_999_999),
                playout: PlayoutConfig {
                    buffer: Duration::from_millis(20),
                },
                drain: Duration::from_millis(10),
                ..config(0)
            },
            320,
        ),
        (
            "8000 one-sample frames for a queue of 8000",
            LoopbackConfig {
                capture: one_sample(),
                delay: Duration::from_millis(500),
                playout: PlayoutConfig {
                    buffer: Duration::from_micros(500_010),
                },
                drain: Duration::from_millis(10),
                ..config(0)
            },
            8_000,
        ),
    ];
    for (case, accepted, samples) in cases {
        let host = mono();
        let report = loopback::run(&host, &accepted, &AtomicBool::new(false), |_| {})
            .unwrap_or_else(|error| panic!("{case}: a delay that fits was refused: {error:?}"));
        assert_eq!(report.delay_samples, samples, "{case}");
        assert_eq!(report.delay_refused, 0, "{case}");
        assert!(!host.input_open() && !host.output_open(), "{case}");
    }
}

/// A captured frame as long as the queue fits it; one sample longer does not.
#[test]
fn a_frame_as_long_as_the_queue_is_accepted_and_one_sample_longer_is_refused() {
    let fits = LoopbackConfig {
        delay: Duration::ZERO,
        playout: PlayoutConfig {
            buffer: Duration::from_millis(20),
        },
        drain: Duration::from_millis(10),
        ..config(0)
    };
    let host = mono();
    let report = loopback::run(&host, &fits, &AtomicBool::new(false), |_| {})
        .unwrap_or_else(|error| panic!("a 320-sample frame for a 320 queue: {error:?}"));
    assert_eq!((report.ended, report.delay_samples), (Ended::Frames, 0));

    let too_long = LoopbackConfig {
        playout: PlayoutConfig {
            buffer: Duration::from_nanos(19_999_999),
        },
        ..fits
    };
    let host = mono();
    let mut seen = Vec::new();
    let refused = loopback::run(&host, &too_long, &AtomicBool::new(false), |event| {
        seen.push(event);
    });
    assert!(
        matches!(refused, Err(DeviceError::InvalidConfig(_))),
        "a 320-sample frame for a 319 queue: {refused:?}"
    );
    assert!(seen.is_empty(), "no device opened: {seen:?}");
    assert!(!host.input_open() && !host.output_open());
}

/// A zero queue holds nothing: refused whatever the frame, before a device opens, also with a
/// zero delay and frames of one sample (which the queue's floor of one sample would hold).
#[test]
fn a_zero_playout_buffer_is_refused_before_a_device_opens() {
    let zero = PlayoutConfig {
        buffer: Duration::ZERO,
    };
    let cases = [
        (
            "20 ms frames",
            LoopbackConfig {
                delay: Duration::ZERO,
                playout: zero,
                ..config(1)
            },
        ),
        (
            "one-sample frames",
            LoopbackConfig {
                capture: one_sample(),
                delay: Duration::ZERO,
                playout: zero,
                ..config(1)
            },
        ),
    ];
    for (case, refused) in cases {
        let host = mono();
        let mut seen = Vec::new();
        let answer = loopback::run(&host, &refused, &AtomicBool::new(false), |event| {
            seen.push(event);
        });
        assert!(
            matches!(answer, Err(DeviceError::InvalidConfig(_))),
            "{case}: {answer:?}"
        );
        assert!(seen.is_empty(), "{case}: no device opened: {seen:?}");
        assert!(!host.input_open() && !host.output_open(), "{case}");
    }
}

// --- The stop and the run between frames of the delay --------------------------------------

/// The unit's cases set the stop before the loop starts, so a check made once before the delay
/// would pass them; this one sets it after the delay's first frame was queued.
#[test]
fn a_stop_set_after_the_first_delay_frame_ends_the_delay_there() {
    let host = mono();
    let stop = AtomicBool::new(false);
    let config = LoopbackConfig {
        delay: Duration::from_millis(500),
        frames: None,
        drain: Duration::from_millis(50),
        ..config(0)
    };
    let mut seen = Vec::new();
    let started = Instant::now();
    let report = loopback::run(&host, &config, &stop, |event| {
        if matches!(event, LoopbackEvent::Pushed { delay: true, .. }) {
            stop.store(true, Ordering::Relaxed);
        }
        seen.push(event);
    })
    .expect("ran");

    assert_eq!(report.ended, Ended::Stopped);
    assert_eq!(pushed_positions(&seen, true), vec![0]);
    assert_eq!(report.delay_samples, 320);
    assert_eq!((report.captured, report.forwarded), (0, 0));
    assert_eq!(report.speaker_played, Some(position(0)));
    assert_eq!(report.played, Some(position(0)));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(!host.input_open() && !host.output_open());
}

/// The run counts from before the delay; one that elapses while the delay is queued ends it
/// between frames, as an elapsed run.
#[test]
fn a_run_that_elapses_after_the_first_delay_frame_ends_the_delay_there() {
    let host = mono();
    let stop = AtomicBool::new(false);
    let config = LoopbackConfig {
        delay: Duration::from_millis(500),
        frames: None,
        run: Duration::from_millis(100),
        drain: Duration::from_millis(50),
        ..config(0)
    };
    let mut seen = Vec::new();
    let report = loopback::run(&host, &config, &stop, |event| {
        if matches!(event, LoopbackEvent::Pushed { delay: true, .. }) {
            thread::sleep(Duration::from_millis(150));
        }
        seen.push(event);
    })
    .expect("ran");

    assert_eq!(report.ended, Ended::Elapsed);
    assert_eq!(pushed_positions(&seen, true), vec![0]);
    assert_eq!(report.delay_samples, 320);
    assert_eq!(report.captured, 0);
    assert!(!host.input_open() && !host.output_open());
}

// --- The longest accepted delay on devices in real time -----------------------------------

/// `config` run against devices fed and played one 20 ms frame per 20 ms, progress reported after
/// every captured frame. Answers the report and every event.
fn in_real_time(config: LoopbackConfig) -> (LoopbackReport, Vec<LoopbackEvent>) {
    let host = mono();
    let stop = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let devices = {
        let host = host.clone();
        let done = Arc::clone(&done);
        thread::spawn(move || {
            let tick = Duration::from_millis(20);
            let mut next = Instant::now();
            let mut sent = 0_usize;
            while !done.load(Ordering::Relaxed) {
                let chunk = signal(sent + 320);
                if host.feed(&chunk[sent..]) {
                    sent += 320;
                }
                let _played = host.pull(320);
                next += tick;
                thread::sleep(next.saturating_duration_since(Instant::now()));
            }
        })
    };
    let config = LoopbackConfig {
        progress: Duration::ZERO,
        ..config
    };
    let (handle, received) = start(&host, config, &stop);
    let report = handle.join().expect("the loop ends").expect("the loop ran");
    done.store(true, Ordering::Relaxed);
    devices.join().expect("the fake devices stop");
    (report, received.try_iter().collect())
}

/// The microphone's dropped device frames as the last progress event, after the last captured
/// frame was forwarded, reported them.
fn dropped_while_forwarding(seen: &[LoopbackEvent]) -> u64 {
    seen.iter()
        .rev()
        .find_map(|event| match event {
            LoopbackEvent::Progress { capture, .. } => Some(capture.dropped_device_frames),
            _ => None,
        })
        .expect("progress was reported")
}

/// The report's capture stats are what the example prints at the end as the loop's lost audio.
/// They should count what the microphone dropped while the loop read it; the microphone stays
/// open, unread, through the drain, and its ring overflows there whenever the drain outlasts it.
fn assert_no_audio_dropped_after_forwarding(report: &LoopbackReport, seen: &[LoopbackEvent]) {
    let while_forwarding = dropped_while_forwarding(seen);
    assert_eq!(
        while_forwarding, 0,
        "the microphone dropped audio while forwarding"
    );
    assert_eq!(
        report.capture.dropped_device_frames, while_forwarding,
        "story:audio-device-adapter: the report says the microphone dropped {} device frames, \
         every one of them after the loop stopped forwarding: the microphone stays open and \
         unread while the speaker drains, so its ring overflows during the drain and the report \
         counts that as lost loopback audio",
        report.capture.dropped_device_frames
    );
}

/// Pass 1's case for a delay longer than the queue asserted that the microphone lost nothing;
/// its rewrite asserts the refusal and dropped that assertion. Here it is again for the longest
/// delay still accepted, one that fills the speaker's queue exactly, on devices in real time.
#[test]
fn the_longest_accepted_delay_on_devices_in_real_time_loses_no_microphone_audio() {
    let (report, seen) = in_real_time(LoopbackConfig {
        delay: Duration::from_nanos(499_999_999),
        playout: PlayoutConfig {
            buffer: Duration::from_millis(500),
        },
        frames: Some(30),
        drain: Duration::from_secs(3),
        ..LoopbackConfig::default()
    });

    assert_eq!(report.delay_samples, 8_000);
    assert_eq!((report.forwarded, report.refused), (30, 0));
    assert!(report.played_out);
    assert_eq!(report.played, Some(position(8_000 + 30 * 320)));
    assert_eq!(
        dropped_while_forwarding(&seen),
        0,
        "the microphone dropped audio while forwarding"
    );
}

/// The default capture ring holds half a second and the default speaker queue two; a one-second
/// delay is accepted, and draining it takes a second during which nobody reads the microphone.
#[test]
fn a_one_second_delay_on_the_default_buffers_reports_no_audio_dropped_after_forwarding() {
    let (report, seen) = in_real_time(LoopbackConfig {
        delay: Duration::from_secs(1),
        frames: Some(10),
        drain: Duration::from_secs(3),
        ..LoopbackConfig::default()
    });

    assert_eq!(report.delay_samples, 16_000);
    assert_eq!((report.forwarded, report.refused), (10, 0));
    assert!(report.played_out);
    assert_no_audio_dropped_after_forwarding(&report, &seen);
}

// --- `played` with refused frames ------------------------------------------------------------

/// A queue of one frame, no delay, five frames fed at once. The first is queued; the second and
/// third are refused (nobody pulls); the test then plays the first, so the fourth is queued; the
/// fifth is refused; once the loop drains, the test plays the fourth. The speaker played two
/// frames, 640 samples, and its own position is 1280: three gaps, two before it and one at it.
#[test]
fn several_refused_frames_are_not_counted_as_played() {
    let host = mono();
    let stop = Arc::new(AtomicBool::new(false));
    let config = LoopbackConfig {
        delay: Duration::ZERO,
        playout: PlayoutConfig {
            buffer: Duration::from_millis(20),
        },
        ..config(5)
    };
    let (handle, received) = start(&host, config, &stop);
    let mut seen = Vec::new();
    until(&received, &mut seen, |event| {
        matches!(event, LoopbackEvent::Opened { .. })
    });
    let sent = signal(5 * 320);
    assert!(host.feed(&sent));
    until(&received, &mut seen, |event| is_refused_at(event, 640));
    let mut played = host.pull(320).expect("the speaker runs");
    until(&received, &mut seen, |event| {
        matches!(event, LoopbackEvent::Draining { .. })
    });
    played.extend(host.pull(320).expect("the speaker runs"));
    let report = handle.join().expect("the loop ends").expect("the loop ran");
    seen.extend(received.try_iter());

    assert_eq!(pushed_positions(&seen, false), vec![0, 960]);
    assert_eq!(refused_positions(&seen), vec![320, 640, 1_280]);
    assert_eq!(
        (report.captured, report.forwarded, report.refused),
        (5, 2, 3)
    );
    assert_eq!(&played[..320], &sent[..320]);
    assert_eq!(&played[320..], &sent[960..1_280]);
    assert!(report.played_out);
    assert_eq!(report.speaker_played, Some(position(1_280)));
    assert_eq!(report.played, Some(position(640)));
}

/// A frame refused just before the stop leaves a gap after the speaker's played position, which
/// the speaker has not reached: it takes nothing from `played`. The speaker plays half of the
/// one frame it queued within the drain.
#[test]
fn a_refused_frame_after_the_played_position_takes_nothing_from_played() {
    let host = mono();
    let stop = Arc::new(AtomicBool::new(false));
    let config = LoopbackConfig {
        delay: Duration::ZERO,
        playout: PlayoutConfig {
            buffer: Duration::from_millis(20),
        },
        frames: None,
        drain: Duration::from_millis(300),
        ..config(0)
    };
    let (handle, received) = start(&host, config, &stop);
    let mut seen = Vec::new();
    until(&received, &mut seen, |event| {
        matches!(event, LoopbackEvent::Opened { .. })
    });
    let sent = signal(2 * 320);
    assert!(host.feed(&sent));
    until(&received, &mut seen, |event| is_refused_at(event, 320));
    stop.store(true, Ordering::Relaxed);
    until(&received, &mut seen, |event| {
        matches!(event, LoopbackEvent::Draining { .. })
    });
    let played = host.pull(160).expect("the speaker runs");
    let report = handle.join().expect("the loop ends").expect("the loop ran");

    assert_eq!(report.ended, Ended::Stopped);
    assert_eq!((report.forwarded, report.refused), (1, 1));
    assert_eq!(&played[..], &sent[..160]);
    assert!(!report.played_out, "half of the queued frame is left");
    assert_eq!(report.speaker_played, Some(position(160)));
    assert_eq!(
        report.played,
        Some(position(160)),
        "the refused frame at 320 lies after the speaker's position 160 and was subtracted"
    );
    assert!(!host.input_open() && !host.output_open());
}
