//! Adversary pass 1 against the capture ring: a writer on its own thread against a slow reader
//! and a ring far smaller than a callback, with every sample unique, so a lost, duplicated or
//! reordered sample and a drop count that does not match what was dropped all show
//! (`story:audio-device-adapter`, Planned scope: the bounded buffer and its drop count).

use std::num::{NonZeroU16, NonZeroU32};
use std::thread;
use std::time::Duration;

use inference_audio::{AudioEncoding, AudioFrame};
use inference_audio_device::convert::decode;
use inference_audio_device::fake::{FakeDevice, FakeHost};
use inference_audio_device::{CaptureConfig, DeviceSelector, Microphone};

const FED: usize = 200_000;

fn host() -> FakeHost {
    FakeHost::new(vec![FakeDevice::input(
        "mono mic",
        NonZeroU32::new(16_000).expect("a rate"),
        NonZeroU16::MIN,
        true,
    )])
}

fn config(buffer: Duration, frame: Duration) -> CaptureConfig {
    CaptureConfig {
        encoding: AudioEncoding::PcmF32le,
        buffer,
        frame,
        ..CaptureConfig::default()
    }
}

/// Feeds `FED` unique samples in callbacks of varying size on a thread of its own while the
/// reader polls with `pause` between frames; answers the samples read and the drop count.
fn run(buffer: Duration, frame: Duration, pause: Duration) -> (Vec<f32>, u64, usize) {
    let host = host();
    let mut microphone = Microphone::open(&host, &DeviceSelector::Default, config(buffer, frame))
        .expect("the input opens");
    let frame_samples = config(buffer, frame).frame_samples().expect("a frame") as usize;
    let device = host.clone();
    let writer = thread::spawn(move || {
        let ramp: Vec<f32> = (1..=FED).map(|n| n as f32).collect();
        let mut rest = ramp.as_slice();
        for size in [1_usize, 17, 160, 3, 480, 64].into_iter().cycle() {
            if rest.is_empty() {
                break;
            }
            let (chunk, after) = rest.split_at(size.min(rest.len()));
            assert!(device.feed(chunk));
            rest = after;
            thread::yield_now();
        }
    });
    let mut frames: Vec<AudioFrame> = Vec::new();
    while !writer.is_finished() {
        if let Some(frame) = microphone.try_frame() {
            frames.push(frame);
            if !pause.is_zero() {
                thread::sleep(pause);
            }
        }
    }
    writer.join().expect("the writer ran");
    frames.extend(std::iter::from_fn(|| microphone.try_frame()));
    for (index, frame) in frames.iter().enumerate() {
        assert_eq!(
            frame.position.get(),
            (index * frame_samples) as u64,
            "frame {index} is not on the clock"
        );
    }
    let samples = frames
        .iter()
        .flat_map(|frame| decode(frame).expect("a well-formed frame"))
        .collect();
    (
        samples,
        microphone.stats().dropped_device_frames,
        frame_samples,
    )
}

fn check(samples: &[f32], dropped: u64, frame_samples: usize) {
    for pair in samples.windows(2) {
        assert!(
            pair[0] < pair[1],
            "sample {} then {}: the ring duplicated or reordered audio",
            pair[0],
            pair[1]
        );
    }
    let kept = samples.len() as u64;
    let accounted = kept + dropped;
    assert!(
        accounted <= FED as u64 && FED as u64 - accounted < frame_samples as u64,
        "kept {kept} and dropped {dropped} of {FED}: the drop count is not what was lost"
    );
}

#[test]
fn a_tiny_ring_under_a_fast_writer_keeps_order_and_counts_every_drop() {
    let (samples, dropped, frame_samples) = run(
        Duration::from_millis(1),
        Duration::from_millis(1),
        Duration::ZERO,
    );
    check(&samples, dropped, frame_samples);
}

#[test]
fn a_slow_reader_loses_audio_only_as_counted_drops() {
    let (samples, dropped, frame_samples) = run(
        Duration::from_millis(20),
        Duration::from_millis(5),
        Duration::from_micros(200),
    );
    check(&samples, dropped, frame_samples);
}
