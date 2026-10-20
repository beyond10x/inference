//! Playout without audio hardware, against the fake device: resampling to the device, played
//! position accounting on the frames' own clock, stop-and-clear, underruns and the bounded queue
//! (`story:audio-device-adapter`, Planned scope: playout, tests).

use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_audio_device::convert::{decode, encode};
use inference_audio_device::fake::{FakeDevice, FakeHost};
use inference_audio_device::{DeviceSelector, PlayoutConfig, PushError, Speaker};

fn hz(rate: u32) -> NonZeroU32 {
    NonZeroU32::new(rate).expect("a rate above 0")
}

fn channels(count: u16) -> NonZeroU16 {
    NonZeroU16::new(count).expect("a channel count above 0")
}

fn position(index: u64) -> SampleClockPosition {
    SampleClockPosition::new(index).expect("a position")
}

/// A mono PCM16 frame at `rate` of `samples` at `at`.
fn frame_at(rate: u32, at: u64, samples: &[f32]) -> AudioFrame {
    AudioFrame {
        format: AudioFormat {
            encoding: AudioEncoding::PcmS16le,
            sample_rate: hz(rate),
            channels: channels(1),
        },
        position: position(at),
        sample_count: u32::try_from(samples.len()).expect("a short frame"),
        data: encode(samples, AudioEncoding::PcmS16le),
    }
}

/// A 20 ms mono PCM16 frame at 16 kHz at `at`, every sample `level`.
fn frame(at: u64, level: f32) -> AudioFrame {
    frame_at(16_000, at, &[level; 320])
}

/// A host whose default output is a mono device at 16 kHz: frames at 16 kHz pass through.
fn mono_16k() -> FakeHost {
    FakeHost::new(vec![FakeDevice::output(
        "line out",
        hz(16_000),
        channels(1),
        true,
    )])
}

fn open(host: &FakeHost, config: PlayoutConfig) -> Speaker {
    Speaker::open(host, &DeviceSelector::Default, config).expect("the default output opens")
}

#[test]
fn at_the_device_rate_frames_play_sample_for_sample_and_the_position_follows() {
    let host = mono_16k();
    let speaker = open(&host, PlayoutConfig::default());
    assert_eq!(speaker.played(), None, "nothing was pushed");
    let ramp: Vec<f32> = (0..320).map(|n| n as f32 / 400.0).collect();
    let sent = frame_at(16_000, 1000, &ramp);
    speaker.try_push(&sent).expect("room for 20 ms");
    assert_eq!(speaker.played(), Some(position(1000)));
    assert_eq!(speaker.queued(), 320);
    let played = host.pull(100).expect("the output runs");
    assert_eq!(played, decode(&sent).expect("a well-formed frame")[..100]);
    assert_eq!(speaker.played(), Some(position(1100)));
    let played = host.pull(220).expect("the output runs");
    assert_eq!(played, decode(&sent).expect("a well-formed frame")[100..]);
    assert_eq!(speaker.played(), Some(position(1320)));
    assert_eq!(speaker.queued(), 0);
}

#[test]
fn frames_are_resampled_to_a_48k_stereo_device_with_every_channel_alike() {
    let host = FakeHost::laptop();
    let speaker = open(&host, PlayoutConfig::default());
    speaker.try_push(&frame(0, 0.5)).expect("room");
    speaker.finish();
    let played = host.pull(960).expect("the output runs");
    assert_eq!(played.len(), 1920);
    let level = 16384.0 / 32767.0;
    assert!(
        played.iter().all(|sample| (sample - level).abs() < 1e-6),
        "20 ms at 16 kHz fills 20 ms at 48 kHz on both channels"
    );
    assert_eq!(speaker.played(), Some(position(320)));
    let after = host.pull(10).expect("the output runs");
    assert!(after.iter().all(|&sample| sample == 0.0), "silence after");
    assert_eq!(speaker.stats().underruns, 0, "the audio was finished");
}

#[test]
fn a_ramp_is_interpolated_between_its_samples_on_the_device() {
    let host = FakeHost::new(vec![FakeDevice::output(
        "line out",
        hz(48_000),
        channels(1),
        true,
    )]);
    let speaker = open(&host, PlayoutConfig::default());
    let ramp: Vec<f32> = (0..320).map(|n| n as f32 / 1000.0).collect();
    let sent = frame_at(16_000, 0, &ramp);
    let decoded = decode(&sent).expect("a well-formed frame");
    speaker.try_push(&sent).expect("room");
    let played = host.pull(9).expect("the output runs");
    for (k, value) in played.iter().enumerate() {
        let (index, fraction) = (k / 3, (k % 3) as f32 / 3.0);
        let want = decoded[index] + (decoded[index + 1] - decoded[index]) * fraction;
        assert!((value - want).abs() < 1e-6, "device sample {k}");
    }
}

#[test]
fn the_played_position_counts_only_what_was_handed_to_the_device() {
    let host = FakeHost::laptop();
    let speaker = open(&host, PlayoutConfig::default());
    for index in 0..3 {
        speaker
            .try_push(&frame(index * 320, 0.25))
            .expect("room for 60 ms");
    }
    assert_eq!(speaker.played(), Some(position(0)));
    let _ = host.pull(480);
    assert_eq!(
        speaker.played(),
        Some(position(160)),
        "480 at 48 kHz is 160 at 16 kHz"
    );
    let _ = host.pull(1440);
    assert_eq!(speaker.played(), Some(position(640)));
    assert_eq!(speaker.queued(), 320);
}

#[test]
fn stop_clears_the_queue_at_once_and_answers_the_position_reached() {
    let host = FakeHost::laptop();
    let speaker = open(&host, PlayoutConfig::default());
    for index in 0..3 {
        speaker
            .try_push(&frame(8000 + index * 320, 0.25))
            .expect("room");
    }
    let _ = host.pull(480);
    assert_eq!(speaker.stop(), Some(position(8160)));
    assert_eq!(speaker.queued(), 0);
    assert_eq!(speaker.played(), None, "a stop forgets the clock");
    let after = host.pull(480).expect("the output runs");
    assert!(
        after.iter().all(|&sample| sample == 0.0),
        "nothing unplayed is heard"
    );
    assert_eq!(speaker.stats().underruns, 0, "a stopped sink owes no audio");
    assert_eq!(speaker.stop(), None, "nothing was pushed since");
    speaker
        .try_push(&frame_at(24_000, 0, &[0.1; 480]))
        .expect("a stop admits a new clock at any rate");
    assert_eq!(speaker.played(), Some(position(0)));
}

#[test]
fn running_dry_while_audio_is_due_is_an_underrun_and_after_finish_it_is_not() {
    let host = FakeHost::laptop();
    let speaker = open(&host, PlayoutConfig::default());
    let _ = host.pull(960);
    assert_eq!(speaker.stats().underruns, 0, "an idle sink owes no audio");
    speaker.try_push(&frame(0, 0.5)).expect("room");
    let played = host.pull(1920).expect("the output runs");
    assert_eq!(speaker.stats().underruns, 1);
    assert!(played[2000..].iter().all(|&sample| sample == 0.0));
    speaker.try_push(&frame(320, 0.5)).expect("room");
    speaker.finish();
    let _ = host.pull(1920);
    let _ = host.pull(1920);
    assert_eq!(speaker.stats().underruns, 1, "the finished audio ran out");
    assert_eq!(speaker.played(), Some(position(640)));
}

#[test]
fn the_queue_is_bounded_and_a_refused_frame_queues_nothing() {
    let host = FakeHost::laptop();
    let speaker = open(
        &host,
        PlayoutConfig {
            buffer: Duration::from_millis(40),
        },
    );
    speaker.try_push(&frame(0, 0.1)).expect("room");
    speaker.try_push(&frame(320, 0.1)).expect("room");
    assert_eq!(speaker.try_push(&frame(640, 0.1)), Err(PushError::Full));
    assert_eq!(speaker.queued(), 640);
    assert_eq!(
        speaker.try_push(&frame_at(16_000, 640, &[0.1; 800])),
        Err(PushError::TooLarge {
            samples: 800,
            capacity: 640
        })
    );
    assert_eq!(
        speaker.push(&frame(640, 0.1), Duration::from_millis(5)),
        Err(PushError::Full),
        "no room came while nothing played"
    );
    let _ = host.pull(480);
    assert_eq!(speaker.try_push(&frame(640, 0.1)), Err(PushError::Full));
    let _ = host.pull(480);
    speaker
        .push(&frame(640, 0.1), Duration::from_millis(5))
        .expect("320 samples played make room");
    assert_eq!(speaker.queued(), 640);
}

#[test]
fn a_frame_at_another_rate_is_refused_until_a_stop() {
    let host = FakeHost::laptop();
    let speaker = open(&host, PlayoutConfig::default());
    speaker.try_push(&frame(0, 0.1)).expect("room");
    assert_eq!(
        speaker.try_push(&frame_at(24_000, 480, &[0.1; 480])),
        Err(PushError::RateChanged {
            expected: hz(16_000),
            got: hz(24_000)
        })
    );
    assert_eq!(speaker.queued(), 320);
}

#[test]
fn a_malformed_frame_is_refused() {
    let host = FakeHost::laptop();
    let speaker = open(&host, PlayoutConfig::default());
    let mut broken = frame(0, 0.1);
    broken.data.pop();
    assert!(matches!(
        speaker.try_push(&broken),
        Err(PushError::Malformed(_))
    ));
    assert_eq!(speaker.queued(), 0);
    assert_eq!(speaker.played(), None);
}

#[test]
fn a_frame_that_does_not_continue_the_last_keeps_the_position_on_its_own_clock() {
    let host = mono_16k();
    let speaker = open(&host, PlayoutConfig::default());
    speaker.try_push(&frame(0, 0.1)).expect("room");
    speaker.try_push(&frame(10_000, 0.2)).expect("room");
    let _ = host.pull(300);
    assert_eq!(speaker.played(), Some(position(300)));
    let _ = host.pull(20);
    assert_eq!(speaker.played(), Some(position(10_000)));
    let _ = host.pull(100);
    assert_eq!(speaker.stop(), Some(position(10_100)));
}

#[test]
fn a_stereo_frame_is_down_mixed_before_it_plays() {
    let host = mono_16k();
    let speaker = open(&host, PlayoutConfig::default());
    let stereo = AudioFrame {
        format: AudioFormat {
            encoding: AudioEncoding::PcmF32le,
            sample_rate: hz(16_000),
            channels: channels(2),
        },
        position: position(0),
        sample_count: 2,
        data: encode(&[0.5, 0.25, -0.5, 0.0], AudioEncoding::PcmF32le),
    };
    speaker.try_push(&stereo).expect("room");
    speaker.finish();
    assert_eq!(host.pull(2).expect("the output runs"), [0.375, -0.25]);
    assert_eq!(speaker.played(), Some(position(2)));
}

#[test]
fn dropping_the_speaker_closes_the_output_stream() {
    let host = FakeHost::laptop();
    let speaker = open(&host, PlayoutConfig::default());
    assert!(host.output_open());
    drop(speaker);
    assert!(!host.output_open());
    assert!(host.pull(10).is_none());
}
