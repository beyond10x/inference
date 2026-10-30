//! Adversary pass 1 against the loopback: a delay longer than the speaker's queue, the delay's
//! rounding, frame limits of 0 and 1, a zero delay, refused frames and what the report then says
//! was played, a microphone that fails or overruns mid-run, and devices at native rates other than
//! the requested one (`story:audio-device-adapter`, Planned scope: the `mic-loopback` example).

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
use inference_audio_device::{CaptureConfig, DeviceError, PlayoutConfig, PushError};

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

/// A host whose default input and output are mono devices at `rate`.
fn mono(rate: u32) -> FakeHost {
    FakeHost::new(vec![
        FakeDevice::input("line in", hz(rate), channels(1), true),
        FakeDevice::output("line out", hz(rate), channels(1), true),
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

// --- A delay longer than the speaker's queue -------------------------------------------------

/// A stop set before the loop starts ends it at once: the stop flag is the deterministic way a
/// caller ends the loop. With a delay longer than the speaker's queue and a speaker that does
/// not drain, the delay phase waits a second for room for each frame that does not fit, and
/// neither the stop flag nor the run is looked at until the delay is queued.
#[test]
fn a_stop_set_before_the_first_frame_ends_the_loop_without_waiting_out_the_delay() {
    let host = mono(16_000);
    let stop = AtomicBool::new(true);
    let config = LoopbackConfig {
        delay: Duration::from_millis(200),
        playout: PlayoutConfig {
            buffer: Duration::from_millis(100),
        },
        run: Duration::from_secs(60),
        drain: Duration::from_millis(10),
        ..LoopbackConfig::default()
    };
    let started = Instant::now();
    let report = loopback::run(&host, &config, &stop, |_| {}).expect("the loop ran");
    let took = started.elapsed();

    assert_eq!(report.ended, Ended::Stopped);
    assert!(
        took < Duration::from_secs(2),
        "a stop set before the first frame took {took:?} to end the loop: the delay phase never \
         looks at the stop flag (or the run) and waited a second for each of the {} delay frames \
         the 100 ms queue refused",
        report.delay_refused
    );
}

/// On a device that plays in real time, a delay longer than the speaker's queue makes the delay
/// phase wait for the speaker while nothing reads the microphone, so its ring overflows and the
/// captured audio is lost before the loop forwards its first frame.
#[test]
fn a_delay_longer_than_the_speaker_queue_loses_no_microphone_audio() {
    let host = mono(16_000);
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
        delay: Duration::from_millis(1_500),
        playout: PlayoutConfig {
            buffer: Duration::from_millis(500),
        },
        frames: Some(30),
        run: Duration::from_secs(30),
        drain: Duration::from_secs(3),
        ..LoopbackConfig::default()
    };
    let (handle, _received) = start(&host, config, &stop);
    let report = handle.join().expect("the loop ends").expect("the loop ran");
    done.store(true, Ordering::Relaxed);
    devices.join().expect("the fake devices stop");

    assert_eq!(
        report.delay_refused, 0,
        "every delay frame found room in time"
    );
    assert_eq!(
        report.capture.dropped_device_frames, 0,
        "the microphone dropped {} samples: a 1.5 s delay does not fit a 0.5 s speaker queue, the \
         delay phase blocked on the speaker for about a second without reading the microphone, \
         and its 0.5 s ring overflowed; the config is accepted without a word",
        report.capture.dropped_device_frames
    );
}

// --- The delay's rounding ------------------------------------------------------------------

/// A 1 ms frame at 44.1 kHz holds 44 samples (44.1, truncated). The loop counts the delay in
/// frame durations and queues 44 samples for each, so a one-second delay becomes 44 000 samples
/// where one second is 44 100: short by more than two frames, not by less than one.
#[test]
fn a_delay_is_short_by_less_than_one_frame_when_the_frame_truncates_its_samples() {
    let host = mono(44_100);
    let stop = AtomicBool::new(false);
    let config = LoopbackConfig {
        capture: CaptureConfig {
            sample_rate: hz(44_100),
            frame: Duration::from_millis(1),
            ..CaptureConfig::default()
        },
        delay: Duration::from_secs(1),
        frames: Some(0),
        drain: Duration::from_millis(10),
        ..LoopbackConfig::default()
    };
    let report = loopback::run(&host, &config, &stop, |_| {}).expect("the loop ran");

    let wanted = 44_100_u64;
    assert!(
        report.delay_samples <= wanted && wanted - report.delay_samples < 44,
        "a 1 s delay at 44.1 kHz in 44-sample frames queued {} samples of silence, {} short of \
         one second: more than one frame short, since the delay counts frames of 1 ms where each \
         holds 0.9977 ms of samples",
        report.delay_samples,
        wanted.saturating_sub(report.delay_samples)
    );
}

#[test]
fn a_delay_that_is_not_a_whole_number_of_frames_is_truncated_to_whole_frames() {
    let host = mono(16_000);
    let stop = AtomicBool::new(false);
    let config = LoopbackConfig {
        delay: Duration::from_millis(519),
        drain: Duration::from_millis(10),
        ..config(0)
    };
    let mut seen = Vec::new();
    let report = loopback::run(&host, &config, &stop, |event| seen.push(event)).expect("ran");

    assert_eq!(report.delay_samples, 25 * 320);
    assert_eq!(
        pushed_positions(&seen, true),
        (0..25).map(|n| n * 320).collect::<Vec<_>>()
    );
}

// --- Frame limits and a zero delay ---------------------------------------------------------

#[test]
fn a_frame_limit_of_zero_forwards_nothing_and_still_closes_both_devices() {
    let host = mono(16_000);
    let stop = AtomicBool::new(false);
    let config = LoopbackConfig {
        drain: Duration::from_millis(10),
        ..config(0)
    };
    let report = loopback::run(&host, &config, &stop, |_| {}).expect("the loop ran");

    assert_eq!(report.ended, Ended::Frames);
    assert_eq!(
        (report.captured, report.forwarded, report.refused),
        (0, 0, 0)
    );
    assert_eq!(report.capture.frames, 0);
    assert!(!host.input_open() && !host.output_open());
}

#[test]
fn a_frame_limit_of_one_forwards_exactly_one_frame_of_two_captured() {
    let host = mono(16_000);
    let stop = Arc::new(AtomicBool::new(false));
    let (handle, received) = start(&host, config(1), &stop);
    let mut seen = Vec::new();
    until(&received, &mut seen, |event| {
        matches!(event, LoopbackEvent::Opened { .. })
    });
    assert!(host.feed(&signal(2 * 320)));
    until(&received, &mut seen, |event| {
        matches!(event, LoopbackEvent::Draining { .. })
    });
    let _played = host.pull(8_000 + 320).expect("the speaker runs");
    let report = handle.join().expect("the loop ends").expect("the loop ran");
    seen.extend(received.try_iter());

    assert_eq!(report.ended, Ended::Frames);
    assert_eq!((report.captured, report.forwarded), (1, 1));
    assert_eq!(pushed_positions(&seen, false), vec![8_000]);
    assert_eq!(report.played, Some(position(8_000 + 320)));
    assert!(report.played_out);
}

#[test]
fn a_zero_delay_plays_the_microphone_from_the_first_sample() {
    let host = mono(16_000);
    let stop = Arc::new(AtomicBool::new(false));
    let config = LoopbackConfig {
        delay: Duration::ZERO,
        ..config(3)
    };
    let (handle, received) = start(&host, config, &stop);
    let mut seen = Vec::new();
    until(&received, &mut seen, |event| {
        matches!(event, LoopbackEvent::Opened { .. })
    });
    let sent = signal(3 * 320);
    assert!(host.feed(&sent));
    until(&received, &mut seen, |event| {
        matches!(event, LoopbackEvent::Draining { .. })
    });
    let played = host.pull(sent.len()).expect("the speaker runs");
    let report = handle.join().expect("the loop ends").expect("the loop ran");
    seen.extend(received.try_iter());

    assert_eq!(report.delay_samples, 0);
    assert!(pushed_positions(&seen, true).is_empty());
    assert_eq!(pushed_positions(&seen, false), vec![0, 320, 640]);
    assert_eq!(played, sent);
    assert_eq!(report.played, Some(position(960)));
}

// --- Refused frames ------------------------------------------------------------------------

/// A loop with no delay and a speaker queue of one frame, fed three frames at once: the first
/// is queued, the second is refused (nobody pulls within the push's patience), the test then
/// pulls the first so the third is queued, and finally pulls the third. Answers the report and
/// every event.
fn one_refused_between_two_forwarded() -> (LoopbackReport, Vec<LoopbackEvent>, Vec<f32>, Vec<f32>) {
    let host = mono(16_000);
    let stop = Arc::new(AtomicBool::new(false));
    let config = LoopbackConfig {
        delay: Duration::ZERO,
        playout: PlayoutConfig {
            buffer: Duration::from_millis(20),
        },
        ..config(3)
    };
    let (handle, received) = start(&host, config, &stop);
    let mut seen = Vec::new();
    until(&received, &mut seen, |event| {
        matches!(event, LoopbackEvent::Opened { .. })
    });
    let sent = signal(3 * 320);
    assert!(host.feed(&sent));
    until(&received, &mut seen, |event| {
        matches!(event, LoopbackEvent::Refused { .. })
    });
    let mut played = host.pull(320).expect("the speaker runs");
    until(&received, &mut seen, |event| {
        matches!(event, LoopbackEvent::Draining { .. })
    });
    played.extend(host.pull(320).expect("the speaker runs"));
    let report = handle.join().expect("the loop ends").expect("the loop ran");
    seen.extend(received.try_iter());
    (report, seen, sent, played)
}

/// The unit's own tests assert `refused` only as 0; this one holds a refusal to its count.
#[test]
fn a_refused_frame_is_counted_and_reported_and_the_rest_play() {
    let (report, seen, sent, played) = one_refused_between_two_forwarded();

    assert_eq!(
        (report.captured, report.forwarded, report.refused),
        (3, 2, 1)
    );
    assert!(seen.iter().any(|event| matches!(
        event,
        LoopbackEvent::Refused {
            error: PushError::Full,
            delay: false,
            ..
        }
    )));
    assert_eq!(&played[..320], &sent[..320]);
    assert_eq!(&played[320..], &sent[640..]);
}

/// The speaker played 640 samples, the first frame and the third, and nothing in between; the
/// report's played position says 960, because the loop numbered the refused frame's samples on
/// the speaker's clock although the speaker never played them.
#[test]
fn the_reported_played_position_counts_only_what_the_speaker_played() {
    let (report, seen, _sent, played) = one_refused_between_two_forwarded();

    assert_eq!(played.len(), 640);
    assert_eq!(
        report.played,
        Some(position(report.delay_samples + report.forwarded * 320)),
        "the speaker played {} samples (forwarded {} frames, refused {}), yet the report's played \
         position is {:?}: the refused frame advanced the clock the loop renumbers frames on, so \
         the speaker's clock has a gap of audio it never played (captured positions {:?})",
        played.len(),
        report.forwarded,
        report.refused,
        report.played,
        pushed_positions(&seen, false)
    );
}

/// The unit's own tests assert `delay_refused` only as 0: a speaker queue of three frames that
/// nobody pulls refuses the delay's other two, each after waiting its patience.
#[test]
fn refused_delay_frames_are_counted_and_still_take_their_place_on_the_clock() {
    let host = mono(16_000);
    let stop = AtomicBool::new(false);
    let config = LoopbackConfig {
        delay: Duration::from_millis(100),
        playout: PlayoutConfig {
            buffer: Duration::from_millis(60),
        },
        drain: Duration::from_millis(10),
        ..config(0)
    };
    let mut seen = Vec::new();
    let report = loopback::run(&host, &config, &stop, |event| seen.push(event)).expect("ran");

    assert_eq!(report.delay_samples, 5 * 320, "refused ones included");
    assert_eq!(report.delay_refused, 2);
    assert_eq!(pushed_positions(&seen, true), vec![0, 320, 640]);
    let refused: Vec<u64> = seen
        .iter()
        .filter_map(|event| match event {
            LoopbackEvent::Refused {
                position,
                delay: true,
                error: PushError::Full,
            } => Some(position.get()),
            _ => None,
        })
        .collect();
    assert_eq!(refused, vec![960, 1_280]);
}

// --- The microphone mid-run ----------------------------------------------------------------

#[test]
fn a_microphone_failing_mid_run_is_reported_in_the_capture_stats() {
    let host = mono(16_000);
    let stop = Arc::new(AtomicBool::new(false));
    let (handle, received) = start(&host, config(2), &stop);
    let mut seen = Vec::new();
    until(&received, &mut seen, |event| {
        matches!(event, LoopbackEvent::Opened { .. })
    });
    assert!(host.feed(&signal(320)));
    until(&received, &mut seen, |event| {
        matches!(event, LoopbackEvent::Pushed { delay: false, .. })
    });
    assert!(host.fail_input("the device was unplugged"));
    assert!(host.feed(&signal(320)));
    until(&received, &mut seen, |event| {
        matches!(event, LoopbackEvent::Draining { .. })
    });
    let _played = host.pull(8_000 + 640);
    let report = handle.join().expect("the loop ends").expect("the loop ran");

    assert_eq!(report.capture.stream_errors, 1);
    assert_eq!(
        report.capture.last_error.as_deref(),
        Some("the device was unplugged")
    );
    assert_eq!(report.forwarded, 2);
}

#[test]
fn a_microphone_overrun_is_counted_and_leaves_no_gap_on_the_clock() {
    let host = mono(16_000);
    let stop = Arc::new(AtomicBool::new(false));
    let (handle, received) = start(&host, config(25), &stop);
    let mut seen = Vec::new();
    until(&received, &mut seen, |event| {
        matches!(event, LoopbackEvent::Opened { .. })
    });
    assert!(
        host.feed(&signal(16_000)),
        "one second into a half-second ring"
    );
    until(&received, &mut seen, |event| {
        matches!(event, LoopbackEvent::Draining { .. })
    });
    let _played = host.pull(8_000 + 8_000);
    let report = handle.join().expect("the loop ends").expect("the loop ran");
    seen.extend(received.try_iter());

    assert_eq!(report.capture.dropped_device_frames, 8_000);
    assert_eq!(
        pushed_positions(&seen, false),
        (0..25).map(|n| 8_000 + n * 320).collect::<Vec<_>>()
    );
    assert_eq!(report.played, Some(position(16_000)));
}

// --- Native rates, progress ----------------------------------------------------------------

#[test]
fn a_microphone_at_44_1_khz_and_a_speaker_at_22_05_khz_keep_the_positions() {
    let host = FakeHost::new(vec![
        FakeDevice::input("usb mic", hz(44_100), channels(2), true),
        FakeDevice::output("old speaker", hz(22_050), channels(1), true),
    ]);
    let stop = Arc::new(AtomicBool::new(false));
    let (handle, received) = start(&host, config(10), &stop);
    let mut seen = Vec::new();
    until(&received, &mut seen, |event| {
        matches!(event, LoopbackEvent::Opened { .. })
    });
    let quarter_second: Vec<f32> = (0..11_025).flat_map(|_| [0.25, 0.25]).collect();
    assert!(host.feed(&quarter_second));
    until(&received, &mut seen, |event| {
        matches!(event, LoopbackEvent::Draining { .. })
    });
    let device_frames = (8_000 + 10 * 320) * 22_050 / 16_000 + 2;
    let played = host.pull(device_frames).expect("the speaker runs");
    let report = handle.join().expect("the loop ends").expect("the loop ran");
    seen.extend(received.try_iter());

    assert!(played.iter().all(|sample| sample.is_finite()));
    assert_eq!(
        pushed_positions(&seen, false),
        (0..10).map(|n| 8_000 + n * 320).collect::<Vec<_>>()
    );
    assert!(report.played_out);
    assert_eq!(report.played, Some(position(8_000 + 10 * 320)));
}

/// The unit's own tests never look at a progress event; the example prints from them.
#[test]
fn progress_is_reported_after_a_captured_frame_once_the_interval_passed() {
    let host = mono(16_000);
    let stop = Arc::new(AtomicBool::new(false));
    let config = LoopbackConfig {
        progress: Duration::ZERO,
        ..config(3)
    };
    let (handle, received) = start(&host, config, &stop);
    let mut seen = Vec::new();
    until(&received, &mut seen, |event| {
        matches!(event, LoopbackEvent::Opened { .. })
    });
    assert!(host.feed(&signal(3 * 320)));
    until(&received, &mut seen, |event| {
        matches!(event, LoopbackEvent::Draining { .. })
    });
    let _played = host.pull(8_000 + 960);
    let _report = handle.join().expect("the loop ends").expect("the loop ran");
    seen.extend(received.try_iter());

    let progress: Vec<u64> = seen
        .iter()
        .filter_map(|event| match event {
            LoopbackEvent::Progress { capture, .. } => Some(capture.frames),
            _ => None,
        })
        .collect();
    assert_eq!(progress, vec![1, 2, 3]);
}
