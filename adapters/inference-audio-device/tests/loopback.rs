//! The microphone loopback without audio hardware, against the fake device: what the fake
//! microphone captures comes out of the fake speaker after the delay's silence, on one contiguous
//! sample clock, and a loop that ends closes both devices (`story:audio-device-adapter`, Planned
//! scope: the `mic-loopback` example, tests).

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use inference_audio::SampleClockPosition;
use inference_audio_device::fake::{FakeDevice, FakeHost};
use inference_audio_device::loopback::{
    self, Ended, LoopbackConfig, LoopbackEvent, LoopbackReport,
};
use inference_audio_device::{DeviceError, DeviceSelector, Direction};

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

/// A host whose default input and output are mono devices at 16 kHz, the rate of the default
/// capture config, so nothing is resampled on either side.
fn mono_16k() -> FakeHost {
    FakeHost::new(vec![
        FakeDevice::input("line in", hz(16_000), channels(1), true),
        FakeDevice::output("line out", hz(16_000), channels(1), true),
    ])
}

/// A loop that ends after `frames` captured frames, with the example's half-second delay and a
/// run long enough never to end it first.
fn config(frames: u64) -> LoopbackConfig {
    LoopbackConfig {
        frames: Some(frames),
        run: Duration::from_secs(60),
        drain: PATIENCE,
        ..LoopbackConfig::default()
    }
}

/// A loop running on its own thread.
type Running = thread::JoinHandle<Result<LoopbackReport, DeviceError>>;

/// The loop on `host` with `config`, run on its own thread; its events arrive on the receiver.
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

/// Waits for the first event `wanted` picks, keeping every event that came before it.
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

/// A frame pushed to the speaker: its position and its samples.
type Pushed = (u64, u32);

/// The positions and lengths of the frames pushed to the speaker, the delay's and the
/// forwarded ones apart.
fn pushed(seen: &[LoopbackEvent]) -> (Vec<Pushed>, Vec<Pushed>) {
    let mut delay = Vec::new();
    let mut forwarded = Vec::new();
    for event in seen {
        if let LoopbackEvent::Pushed {
            position,
            samples,
            delay: is_delay,
        } = event
        {
            let into = if *is_delay {
                &mut delay
            } else {
                &mut forwarded
            };
            into.push((position.get(), *samples));
        }
    }
    (delay, forwarded)
}

/// `count` frames of `samples` each, starting at `from`, each continuing the last.
fn contiguous(from: u64, count: u64, samples: u32) -> Vec<Pushed> {
    (0..count)
        .map(|index| (from + index * u64::from(samples), samples))
        .collect()
}

/// A recognisable signal of `count` samples, each exactly one PCM16 step value, so it crosses
/// capture's encoding and playout's decoding unchanged.
fn signal(count: usize) -> Vec<f32> {
    (0..count)
        .map(|n| {
            let step = i16::try_from((n * 37) % 20_000).expect("a step") - 10_000;
            f32::from(step) / 32_767.0
        })
        .collect()
}

#[test]
fn what_the_microphone_captures_plays_after_exactly_the_delay_of_silence_sample_for_sample() {
    let host = mono_16k();
    let stop = Arc::new(AtomicBool::new(false));
    let (handle, received) = start(&host, config(20), &stop);
    let mut seen = Vec::new();
    until(&received, &mut seen, |event| {
        matches!(event, LoopbackEvent::Opened { .. })
    });
    let sent = signal(20 * 320);
    assert!(host.feed(&sent), "the loop's microphone runs");
    until(&received, &mut seen, |event| {
        matches!(event, LoopbackEvent::Draining { .. })
    });

    let delay = 8_000;
    let played = host
        .pull(delay + sent.len())
        .expect("the loop's speaker runs");
    let report = handle.join().expect("the loop ends").expect("the loop ran");
    seen.extend(received.try_iter());

    assert!(
        played[..delay].iter().all(|&sample| sample == 0.0),
        "half a second of silence comes first"
    );
    assert_eq!(
        &played[delay..],
        &sent[..],
        "then the captured audio, sample for sample"
    );
    assert_eq!(report.ended, Ended::Frames);
    assert_eq!(report.delay_samples, delay as u64);
    assert_eq!(report.captured, 20);
    assert_eq!(report.forwarded, 20);
    assert_eq!(report.refused, 0);
    assert_eq!(report.delay_refused, 0);
    assert!(report.played_out, "the speaker played everything out");
    assert_eq!(report.played, Some(position((delay + sent.len()) as u64)));
    assert_eq!(report.capture.frames, 20);
    assert_eq!(report.capture.dropped_device_frames, 0);
    assert_eq!(report.playout.stream_errors, 0);
}

#[test]
fn the_delay_and_the_forwarded_frames_lie_on_one_contiguous_clock() {
    let host = mono_16k();
    let stop = Arc::new(AtomicBool::new(false));
    let (handle, received) = start(&host, config(12), &stop);
    let mut seen = Vec::new();
    until(&received, &mut seen, |event| {
        matches!(event, LoopbackEvent::Opened { .. })
    });
    assert!(host.feed(&signal(12 * 320)), "the loop's microphone runs");
    until(&received, &mut seen, |event| {
        matches!(event, LoopbackEvent::Draining { .. })
    });
    let _played = host
        .pull(8_000 + 12 * 320)
        .expect("the loop's speaker runs");
    let report = handle.join().expect("the loop ends").expect("the loop ran");
    seen.extend(received.try_iter());

    let (delay, forwarded) = pushed(&seen);
    assert_eq!(
        delay,
        contiguous(0, 25, 320),
        "25 frames of 20 ms make the delay"
    );
    assert_eq!(
        forwarded,
        contiguous(8_000, 12, 320),
        "the captured frames continue where the delay ends"
    );
    assert_eq!(report.played, Some(position(8_000 + 12 * 320)));
}

#[test]
fn at_a_different_native_rate_the_loop_keeps_the_length_and_the_positions() {
    let host = FakeHost::laptop();
    let stop = Arc::new(AtomicBool::new(false));
    let (handle, received) = start(&host, config(20), &stop);
    let mut seen = Vec::new();
    until(&received, &mut seen, |event| {
        matches!(event, LoopbackEvent::Opened { .. })
    });
    let half_second: Vec<f32> = (0..24_000).flat_map(|_| [0.25, 0.25]).collect();
    assert!(host.feed(&half_second), "the loop's microphone runs");
    until(&received, &mut seen, |event| {
        matches!(event, LoopbackEvent::Draining { .. })
    });

    let device_frames = (8_000 + 20 * 320) * 3;
    let played = host.pull(device_frames).expect("the loop's speaker runs");
    let report = handle.join().expect("the loop ends").expect("the loop ran");
    seen.extend(received.try_iter());

    assert_eq!(played.len(), device_frames * 2);
    assert!(played.iter().all(|sample| sample.is_finite()));
    let (delay, forwarded) = pushed(&seen);
    assert_eq!(delay, contiguous(0, 25, 320));
    assert_eq!(forwarded, contiguous(8_000, 20, 320));
    assert!(
        report.played_out,
        "48 kHz of device frames played 16 kHz of audio out"
    );
    assert_eq!(report.played, Some(position(8_000 + 20 * 320)));
    assert_eq!(report.forwarded, 20);
}

#[test]
fn a_stopped_loop_closes_both_devices() {
    let host = mono_16k();
    let stop = Arc::new(AtomicBool::new(false));
    let unlimited = LoopbackConfig {
        run: Duration::from_secs(60),
        drain: Duration::from_millis(10),
        ..LoopbackConfig::default()
    };
    let (handle, received) = start(&host, unlimited, &stop);
    let mut seen = Vec::new();
    until(&received, &mut seen, |event| {
        matches!(event, LoopbackEvent::Opened { .. })
    });
    assert!(
        host.input_open() && host.output_open(),
        "the loop runs both devices"
    );
    assert!(host.feed(&signal(5 * 320)), "the loop's microphone runs");
    stop.store(true, Ordering::Relaxed);
    let report = handle.join().expect("the loop ends").expect("the loop ran");

    assert_eq!(report.ended, Ended::Stopped);
    assert!(!host.input_open(), "the microphone is closed");
    assert!(!host.output_open(), "the speaker is closed");
    assert!(!report.played_out, "nobody pulled the speaker");
}

#[test]
fn a_loop_whose_run_elapses_ends_and_closes_both_devices() {
    let host = mono_16k();
    let stop = Arc::new(AtomicBool::new(false));
    let short = LoopbackConfig {
        run: Duration::from_millis(50),
        drain: Duration::from_millis(10),
        ..LoopbackConfig::default()
    };
    let (handle, _received) = start(&host, short, &stop);
    let report = handle.join().expect("the loop ends").expect("the loop ran");

    assert_eq!(report.ended, Ended::Elapsed);
    assert_eq!(report.captured, 0);
    assert!(!host.input_open());
    assert!(!host.output_open());
}

#[test]
fn a_speaker_that_does_not_open_leaves_the_microphone_closed() {
    let host = mono_16k();
    let stop = AtomicBool::new(false);
    let absent = LoopbackConfig {
        output: DeviceSelector::Name("absent".to_owned()),
        ..config(1)
    };
    let refused = loopback::run(&host, &absent, &stop, |_| {});

    assert_eq!(
        refused.err(),
        Some(DeviceError::NotFound {
            name: "absent".to_owned(),
            direction: Direction::Output,
        })
    );
    assert!(!host.input_open());
    assert!(!host.output_open());
}
