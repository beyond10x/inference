//! Adversarial pass 1 of story:sdk-agent against the `FakeHost` tap: `played()` is the log of what
//! the output sink took, so a write the sink refused must not appear in it, and the story's sink
//! takes every write with no pull and is never full.

use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_audio_device::fake::{FakeDevice, FakeHost};
use inference_audio_device::{DeviceSelector, PlayoutConfig, PushError, Speaker};

fn frame(rate: u32, position: u64, level: u8) -> AudioFrame {
    AudioFrame {
        format: AudioFormat {
            encoding: AudioEncoding::PcmS16le,
            sample_rate: NonZeroU32::new(rate).expect("not zero"),
            channels: NonZeroU16::MIN,
        },
        position: SampleClockPosition::new(position).expect("a position"),
        sample_count: 320,
        data: vec![level; 640],
    }
}

fn device() -> FakeDevice {
    FakeDevice::output(
        "fake speaker",
        NonZeroU32::new(48_000).expect("48 kHz"),
        NonZeroU16::new(2).expect("stereo"),
        true,
    )
}

/// A speaker over `host` whose queue holds 20 ms: one 320-sample frame at 16 kHz.
fn small(host: &FakeHost) -> Speaker {
    Speaker::open(
        host,
        &DeviceSelector::Default,
        PlayoutConfig {
            buffer: Duration::from_millis(20),
        },
    )
    .expect("the default output opens")
}

/// The story: "Its sink accepts writes without a pull. `open_output` sets two crate-private hooks
/// on the `PlayoutBuffer` it makes: a tap ... and no capacity bound, so `Speaker::push` never
/// answers `Full`." By decision the bound stays the default and `unbounded()` opts out of it, as
/// the story now says, so the sink that takes every write is `FakeHost::new(..).unbounded()`.
#[test]
fn adversary1_the_fake_hosts_sink_is_never_full_without_a_pull() {
    let host = FakeHost::new(vec![device()]).unbounded();
    let speaker = small(&host);
    speaker
        .try_push(&frame(16_000, 0, 1))
        .expect("the first frame fits");
    let second = speaker.try_push(&frame(16_000, 320, 2));
    assert_eq!(
        second,
        Ok(()),
        "FakeHost::new's sink answered a write with no pull"
    );
}

/// A frame the sink refused as full was not written to it, yet `played()` logs it.
#[test]
fn adversary1_a_write_refused_as_full_is_not_played() {
    let host = FakeHost::new(vec![device()]);
    let speaker = small(&host);
    speaker
        .try_push(&frame(16_000, 0, 1))
        .expect("the first frame fits");
    let refused = speaker.try_push(&frame(16_000, 320, 2));
    assert_eq!(refused, Err(PushError::Full), "the queue holds one frame");
    assert_eq!(
        host.played_frames(),
        [frame(16_000, 0, 1)],
        "played() holds only the frame the sink took"
    );
}

/// `Speaker::push` retries a full queue every 2 ms until its patience runs out; each retry is a
/// fresh `PlayoutBuffer::push`, and so a fresh tap write: one refused frame is logged many times.
#[test]
fn adversary1_a_push_that_waits_for_room_is_played_at_most_once() {
    let host = FakeHost::new(vec![device()]);
    let speaker = small(&host);
    speaker
        .try_push(&frame(16_000, 0, 1))
        .expect("the first frame fits");
    let refused = speaker.push(&frame(16_000, 320, 2), Duration::from_millis(30));
    assert_eq!(refused, Err(PushError::Full), "nothing pulls");
    let copies = host
        .played_frames()
        .iter()
        .filter(|played| **played == frame(16_000, 320, 2))
        .count();
    assert!(
        copies <= 1,
        "one push of one frame is logged {copies} times"
    );
}

/// With `unbounded()` a write refused for its rate is still logged as played.
#[test]
fn adversary1_a_write_refused_for_its_rate_is_not_played() {
    let host = FakeHost::new(vec![device()]).unbounded();
    let speaker = small(&host);
    speaker
        .try_push(&frame(16_000, 0, 1))
        .expect("the first frame fits");
    let refused = speaker.try_push(&frame(24_000, 320, 2));
    assert!(
        matches!(refused, Err(PushError::RateChanged { .. })),
        "{refused:?}"
    );
    assert_eq!(host.played_frames(), [frame(16_000, 0, 1)]);
}

/// "While the returned guard lives, a write to the output sink waits": with two holds taken,
/// dropping one releases the sink while the other still lives.
#[test]
fn adversary1_a_write_waits_while_any_hold_lives() {
    let host = FakeHost::new(vec![device()]).unbounded();
    let speaker = std::sync::Arc::new(small(&host));
    let first = host.hold_sink();
    let second = host.hold_sink();
    drop(first);
    let writer = {
        let speaker = std::sync::Arc::clone(&speaker);
        std::thread::spawn(move || speaker.push(&frame(16_000, 0, 1), Duration::from_secs(5)))
    };
    while second.waiting().is_empty() && host.played().is_empty() {
        std::thread::sleep(Duration::from_millis(2));
    }
    let played = host.played();
    let waiting = second.waiting();
    drop(second);
    writer
        .join()
        .expect("no panic")
        .expect("taken once released");
    assert!(
        played.is_empty(),
        "a write was recorded while a hold lived: {played:?}, waiting {waiting:?}"
    );
}

/// `unbounded()` promises "a write is never refused as full"; the playout queue still refuses a
/// 257th discontinuous segment as `Full` when nothing pulls.
#[test]
fn adversary1_an_unbounded_sink_takes_more_than_256_discontinuous_frames() {
    let host = FakeHost::new(vec![device()]).unbounded();
    let speaker = small(&host);
    for index in 0..300_u64 {
        let pushed = speaker.try_push(&frame(16_000, index * 1_000, 1));
        assert_eq!(pushed, Ok(()), "frame {index} at a fresh position");
    }
}
