//! Adversary pass 1 against playout: played-position accounting across resampling ratios, the
//! queue's boundaries, a stop racing a waiting push and a device callback on its own thread, and
//! what a frame's samples become on the device (`story:audio-device-adapter`, Planned scope:
//! playout).

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_audio_device::convert::encode;
use inference_audio_device::fake::{FakeDevice, FakeHost};
use inference_audio_device::{DeviceSelector, PlayoutConfig, PushError, Speaker};

fn hz(rate: u32) -> NonZeroU32 {
    NonZeroU32::new(rate).expect("a rate above 0")
}

fn position(index: u64) -> SampleClockPosition {
    SampleClockPosition::new(index).expect("a position")
}

fn frame_in(encoding: AudioEncoding, rate: u32, at: u64, samples: &[f32]) -> AudioFrame {
    AudioFrame {
        format: AudioFormat {
            encoding,
            sample_rate: hz(rate),
            channels: NonZeroU16::MIN,
        },
        position: position(at),
        sample_count: u32::try_from(samples.len()).expect("a short frame"),
        data: encode(samples, encoding),
    }
}

fn frame_at(rate: u32, at: u64, samples: &[f32]) -> AudioFrame {
    frame_in(AudioEncoding::PcmS16le, rate, at, samples)
}

fn mono(rate: u32) -> FakeHost {
    FakeHost::new(vec![FakeDevice::output(
        "line out",
        hz(rate),
        NonZeroU16::MIN,
        true,
    )])
}

fn open(host: &FakeHost, config: PlayoutConfig) -> Speaker {
    Speaker::open(host, &DeviceSelector::Default, config).expect("the default output opens")
}

/// 16 kHz frames on a 48 kHz device, not finished: once the device was handed the last queued
/// sample, exactly, nothing is left unplayed, and `wait_played` says so.
#[test]
fn upsampled_audio_whose_last_sample_was_handed_to_the_device_is_all_played() {
    let host = mono(48_000);
    let speaker = open(&host, PlayoutConfig::default());
    let ramp: Vec<f32> = (1..=320).map(|n| n as f32 / 1000.0).collect();
    speaker
        .try_push(&frame_at(16_000, 0, &ramp))
        .expect("room for 20 ms");
    let played = host.pull(960).expect("the output runs");
    let last = 319 * 3;
    assert!(
        (played[last] - 0.32).abs() < 1e-4,
        "device sample {last} is queued sample 319 itself: {}",
        played[last]
    );
    assert_eq!(
        speaker.played(),
        Some(position(320)),
        "sample 319 was handed to the device, so 320 is the first not yet handed"
    );
    assert_eq!(speaker.queued(), 0, "nothing queued is unplayed");
    assert!(
        speaker.wait_played(Duration::from_millis(50)),
        "every sample the device can play was played"
    );
}

/// 24 kHz frames on the 48 kHz stereo laptop device, pushed and never finished: once the device
/// was handed every queued sample it can be handed, `wait_played` returns true instead of
/// waiting out its patience with one sample counted as queued.
#[test]
fn wait_played_returns_once_unfinished_upsampled_audio_was_handed_to_the_device() {
    let host = FakeHost::laptop();
    let speaker = open(&host, PlayoutConfig::default());
    speaker
        .try_push(&frame_at(24_000, 0, &[0.3; 480]))
        .expect("room for 20 ms");
    let _ = host.pull(2000);
    assert_eq!(
        (
            speaker.queued(),
            speaker.wait_played(Duration::from_millis(50))
        ),
        (0, true),
        "every queued sample was handed to the device"
    );
}

/// 48 kHz frames on a 16 kHz device: the played position never passes the end of what was
/// pushed, however far the device clock ran.
#[test]
fn downsampled_audio_never_reports_a_position_past_what_was_pushed() {
    let host = mono(16_000);
    let speaker = open(&host, PlayoutConfig::default());
    speaker
        .try_push(&frame_at(48_000, 1000, &[0.25; 320]))
        .expect("room");
    let _ = host.pull(200);
    assert_eq!(speaker.played(), Some(position(1320)));
    assert_eq!(speaker.queued(), 0);
    assert_eq!(speaker.stop(), Some(position(1320)));
}

/// A frame exactly as long as the whole queue fits an empty queue.
#[test]
fn a_frame_exactly_as_long_as_the_queue_is_accepted() {
    let host = mono(16_000);
    let speaker = open(
        &host,
        PlayoutConfig {
            buffer: Duration::from_millis(40),
        },
    );
    speaker
        .try_push(&frame_at(16_000, 0, &[0.1; 640]))
        .expect("640 samples fit a queue of 640");
    assert_eq!(speaker.queued(), 640);
    assert_eq!(
        speaker.try_push(&frame_at(16_000, 640, &[0.1; 1])),
        Err(PushError::Full)
    );
}

/// A frame pushed after `finish` makes audio due again: running dry before its end is an
/// underrun.
#[test]
fn audio_pushed_after_finish_is_due_again_and_running_dry_on_it_is_an_underrun() {
    let host = mono(16_000);
    let speaker = open(&host, PlayoutConfig::default());
    speaker
        .try_push(&frame_at(16_000, 0, &[0.2; 320]))
        .expect("room");
    speaker.finish();
    let _ = host.pull(640);
    assert_eq!(speaker.stats().underruns, 0, "the finished audio ran out");
    speaker
        .try_push(&frame_at(16_000, 320, &[0.2; 320]))
        .expect("room");
    let _ = host.pull(640);
    assert_eq!(
        speaker.stats().underruns,
        1,
        "the second frame was not finished and ran dry"
    );
    assert_eq!(speaker.played(), Some(position(640)));
}

/// A stop while a push waits for room ends the push with `Stopped`, and the frame it waited with
/// is not queued on the new clock.
#[test]
fn a_stop_while_a_push_waits_for_room_answers_stopped_and_queues_nothing() {
    let host = mono(16_000);
    let speaker = Arc::new(open(
        &host,
        PlayoutConfig {
            buffer: Duration::from_millis(20),
        },
    ));
    speaker
        .try_push(&frame_at(16_000, 0, &[0.1; 320]))
        .expect("room for one frame");
    let waiting = Arc::clone(&speaker);
    let pusher = thread::spawn(move || {
        waiting.push(&frame_at(16_000, 320, &[0.1; 320]), Duration::from_secs(5))
    });
    thread::sleep(Duration::from_millis(300));
    assert_eq!(speaker.stop(), Some(position(0)));
    assert_eq!(
        pusher.join().expect("the pusher ran"),
        Err(PushError::Stopped)
    );
    assert_eq!(speaker.queued(), 0);
    assert_eq!(speaker.played(), None);
}

/// Small frames each continuing the last are one segment: more of them than the segment limit
/// fit as long as the queue has room.
#[test]
fn many_small_continuing_frames_fit_while_the_queue_has_room() {
    let host = mono(16_000);
    let speaker = open(&host, PlayoutConfig::default());
    for index in 0..1000_u64 {
        speaker
            .try_push(&frame_at(16_000, 5000 + index * 10, &[0.1; 10]))
            .unwrap_or_else(|refused| panic!("frame {index}: {refused}"));
    }
    assert_eq!(speaker.queued(), 10_000);
    let _ = host.pull(2500);
    assert_eq!(speaker.played(), Some(position(7500)));
}

/// A stop racing a device callback on its own thread answers exactly the position of the first
/// sample the device was not handed.
#[test]
fn a_stop_racing_the_callback_answers_exactly_what_the_device_was_handed() {
    for round in 0..200_u64 {
        let host = mono(16_000);
        let speaker = open(&host, PlayoutConfig::default());
        let ramp: Vec<f32> = (1..=3200).map(|n| n as f32 / 4000.0).collect();
        let start = 10_000 + round;
        speaker
            .try_push(&frame_in(AudioEncoding::PcmF32le, 16_000, start, &ramp))
            .expect("room");
        let running = Arc::new(AtomicBool::new(true));
        let device = host.clone();
        let flag = Arc::clone(&running);
        let callbacks = thread::spawn(move || {
            let mut handed = Vec::new();
            while flag.load(Ordering::Relaxed) {
                handed.extend(device.pull(7).expect("the output runs"));
            }
            handed
        });
        thread::sleep(Duration::from_micros(50 + round * 7));
        let reached = speaker.stop().expect("frames were pushed");
        thread::sleep(Duration::from_millis(1));
        running.store(false, Ordering::Relaxed);
        let handed = callbacks.join().expect("the callbacks ran");
        let audible: Vec<f32> = handed.into_iter().filter(|&s| s != 0.0).collect();
        for (k, sample) in audible.iter().enumerate() {
            assert_eq!(*sample, ramp[k], "round {round}: device sample {k}");
        }
        assert_eq!(
            reached.get(),
            start + audible.len() as u64,
            "round {round}: the stop answered a position other than what was handed"
        );
    }
}

/// What a frame carries reaches the device as a sample a device can play: finite and within
/// full scale.
#[test]
fn non_finite_and_out_of_range_f32_samples_never_reach_the_device_as_such() {
    let host = mono(16_000);
    let speaker = open(&host, PlayoutConfig::default());
    let samples = [0.5, f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1.5, -3.0];
    speaker
        .try_push(&frame_in(AudioEncoding::PcmF32le, 16_000, 0, &samples))
        .expect("room");
    speaker.finish();
    let played = host.pull(6).expect("the output runs");
    assert!(
        played
            .iter()
            .all(|sample| sample.is_finite() && (-1.0..=1.0).contains(sample)),
        "the device was handed {played:?}"
    );
}
