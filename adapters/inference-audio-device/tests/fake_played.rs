//! What a [`FakeHost`] records of what is played on it: every frame written to its output sink,
//! in order, and every stop as `Cleared`; its sink takes writes with no `pull` and is never full;
//! and a hold keeps writes waiting, recording nothing, until it is dropped, discarding a write
//! that waited through a stop.

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_audio_device::fake::{FakeDevice, FakeHost, Played};
use inference_audio_device::{DeviceSelector, PlayoutConfig, Speaker};

fn frame(index: u64, level: u8) -> AudioFrame {
    AudioFrame {
        format: AudioFormat {
            encoding: AudioEncoding::PcmS16le,
            sample_rate: NonZeroU32::new(16_000).expect("16 kHz"),
            channels: NonZeroU16::MIN,
        },
        position: SampleClockPosition::new(index * 320).expect("a position"),
        sample_count: 320,
        data: vec![level; 640],
    }
}

fn host() -> FakeHost {
    FakeHost::new(vec![FakeDevice::output(
        "fake speaker",
        NonZeroU32::new(48_000).expect("48 kHz"),
        NonZeroU16::new(2).expect("stereo"),
        true,
    )])
    .unbounded()
}

fn open(host: &FakeHost) -> Speaker {
    Speaker::open(host, &DeviceSelector::Default, PlayoutConfig::default())
        .expect("the default output opens")
}

#[test]
fn every_frame_written_and_every_stop_are_played_in_order_with_no_pull_and_no_bound() {
    let host = host();
    let speaker = open(&host);
    for index in 0..200 {
        speaker
            .try_push(&frame(index, 1))
            .expect("the fake sink takes every write");
    }
    speaker.stop();
    speaker
        .push(&frame(200, 2), Duration::from_millis(10))
        .expect("taken after the stop");

    let played = host.played();
    assert_eq!(played.len(), 202, "{} entries", played.len());
    assert_eq!(played[0], Played::Frame(frame(0, 1)));
    assert_eq!(played[199], Played::Frame(frame(199, 1)));
    assert_eq!(played[200], Played::Cleared);
    assert_eq!(played[201], Played::Frame(frame(200, 2)));
    assert_eq!(host.played_frames().len(), 201);
    assert!(
        speaker.queued() > 0,
        "nothing pulled, so the frames stay queued"
    );
}

#[test]
fn a_held_sink_keeps_writes_waiting_and_records_them_in_order_once_released() {
    let host = host();
    let speaker = Arc::new(open(&host));
    let hold = host.hold_sink();
    let writer = {
        let speaker = Arc::clone(&speaker);
        thread::spawn(move || {
            for index in 0..3 {
                speaker
                    .push(&frame(index, 1), Duration::from_secs(5))
                    .expect("taken once released");
            }
        })
    };
    while hold.waiting().is_empty() {
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(hold.waiting(), [frame(0, 1)], "the first write waits");
    assert!(host.played().is_empty(), "nothing is recorded while held");

    drop(hold);
    writer.join().expect("the writer does not panic");
    assert_eq!(
        host.played_frames(),
        [frame(0, 1), frame(1, 1), frame(2, 1)]
    );
}

#[test]
fn a_write_waiting_in_the_hold_through_a_stop_is_discarded() {
    let host = host();
    let speaker = Arc::new(open(&host));
    let hold = host.hold_sink();
    let writer = {
        let speaker = Arc::clone(&speaker);
        thread::spawn(move || speaker.push(&frame(0, 1), Duration::from_secs(5)))
    };
    while hold.waiting().is_empty() {
        thread::sleep(Duration::from_millis(5));
    }
    speaker.stop();
    drop(hold);
    let pushed = writer.join().expect("the writer does not panic");
    assert!(
        pushed.is_err(),
        "the waiting write is discarded: {pushed:?}"
    );
    assert_eq!(host.played(), [Played::Cleared]);
}
