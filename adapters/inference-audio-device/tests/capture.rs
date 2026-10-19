//! Capture without audio hardware, against the fake device: device listing and selection, frame
//! slicing in the requested format, the bounded buffer and its drop count, and stream errors
//! (`story:audio-device-adapter`, Planned scope: device listing, capture, tests).

use std::f32::consts::TAU;
use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use inference_audio::{AudioEncoding, AudioFormat, AudioFrame};
use inference_audio_device::convert::decode;
use inference_audio_device::fake::{FakeDevice, FakeHost};
use inference_audio_device::{
    AudioHost, CaptureConfig, DeviceError, DeviceSelector, Direction, Microphone,
};

fn hz(rate: u32) -> NonZeroU32 {
    NonZeroU32::new(rate).expect("a rate above 0")
}

fn channels(count: u16) -> NonZeroU16 {
    NonZeroU16::new(count).expect("a channel count above 0")
}

/// `frames` stereo device frames, the left channel at `left` and the right at `right`.
fn stereo(frames: usize, left: f32, right: f32) -> Vec<f32> {
    (0..frames).flat_map(|_| [left, right]).collect()
}

/// Every frame the microphone has ready.
fn ready(microphone: &mut Microphone) -> Vec<AudioFrame> {
    std::iter::from_fn(|| microphone.try_frame()).collect()
}

fn pcm16(frame: &AudioFrame) -> Vec<i16> {
    frame
        .data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| i16::from_le_bytes(*pair))
        .collect()
}

#[test]
fn devices_are_listed_with_the_default_input_and_output_marked() {
    let host = FakeHost::new(vec![
        FakeDevice::input("usb headset mic", hz(48_000), channels(1), false),
        FakeDevice::input("built-in mic", hz(44_100), channels(2), true),
        FakeDevice::output("built-in speaker", hz(48_000), channels(2), true),
    ]);
    let devices = host.devices().expect("a fake host lists its devices");
    let defaults: Vec<(&str, bool, bool)> = devices
        .iter()
        .map(|d| (d.name.as_str(), d.default_input, d.default_output))
        .collect();
    assert_eq!(
        defaults,
        [
            ("usb headset mic", false, false),
            ("built-in mic", true, false),
            ("built-in speaker", false, true)
        ]
    );
}

#[test]
fn a_microphone_opens_the_default_input_or_the_input_of_the_name_given() {
    let host = FakeHost::new(vec![
        FakeDevice::input("usb headset mic", hz(48_000), channels(1), false),
        FakeDevice::input("built-in mic", hz(44_100), channels(2), true),
        FakeDevice::output("built-in speaker", hz(48_000), channels(2), true),
    ]);
    let default = Microphone::open(&host, &DeviceSelector::Default, CaptureConfig::default())
        .expect("the default input opens");
    assert_eq!(default.device().name, "built-in mic");
    assert_eq!(default.device().direction, Direction::Input);
    assert_eq!(default.device().config.sample_rate, hz(44_100));
    drop(default);
    assert!(
        !host.input_open(),
        "dropping the microphone closes the stream"
    );
    let named = Microphone::open(
        &host,
        &DeviceSelector::Name("usb headset mic".to_owned()),
        CaptureConfig::default(),
    )
    .expect("the named input opens");
    assert_eq!(named.device().name, "usb headset mic");
    assert!(host.input_open());
    drop(named);
    let speaker_as_mic = Microphone::open(
        &host,
        &DeviceSelector::Name("built-in speaker".to_owned()),
        CaptureConfig::default(),
    );
    assert_eq!(
        speaker_as_mic.err(),
        Some(DeviceError::NotFound {
            name: "built-in speaker".to_owned(),
            direction: Direction::Input
        }),
        "an output device is no input"
    );
    let no_default = FakeHost::new(vec![FakeDevice::input(
        "usb headset mic",
        hz(48_000),
        channels(1),
        false,
    )]);
    assert_eq!(
        Microphone::open(
            &no_default,
            &DeviceSelector::Default,
            CaptureConfig::default()
        )
        .err(),
        Some(DeviceError::NoDefault(Direction::Input))
    );
}

#[test]
fn a_48k_stereo_device_yields_20ms_mono_pcm16_frames_at_16k_numbered_from_zero() {
    let host = FakeHost::laptop();
    let mut microphone =
        Microphone::open(&host, &DeviceSelector::Default, CaptureConfig::default())
            .expect("the default input opens");
    assert_eq!(
        microphone.format(),
        AudioFormat {
            encoding: AudioEncoding::PcmS16le,
            sample_rate: hz(16_000),
            channels: channels(1)
        }
    );
    let audio = stereo(4800, 0.5, 0.25);
    for chunk in audio.chunks(2 * 7) {
        assert!(host.feed(chunk));
    }
    let frames = ready(&mut microphone);
    assert_eq!(
        frames.len(),
        5,
        "100 ms of device audio is five 20 ms frames"
    );
    for (index, frame) in frames.iter().enumerate() {
        assert_eq!(frame.format, microphone.format());
        assert_eq!(frame.position.get(), index as u64 * 320);
        assert_eq!(frame.sample_count, 320);
        assert_eq!(frame.data.len(), 640);
        assert!(
            pcm16(frame).iter().all(|&sample| sample == 12288),
            "both channels down-mixed to 0.375"
        );
    }
    assert!(
        microphone.try_frame().is_none(),
        "nothing more was captured"
    );
    assert_eq!(microphone.stats().frames, 5);
    assert_eq!(microphone.stats().dropped_device_frames, 0);
}

#[test]
fn a_frame_spanning_two_callbacks_continues_the_sample_clock() {
    let host = FakeHost::laptop();
    let mut microphone =
        Microphone::open(&host, &DeviceSelector::Default, CaptureConfig::default())
            .expect("the default input opens");
    assert!(host.feed(&stereo(1500, 0.1, 0.1)));
    assert_eq!(ready(&mut microphone).len(), 1, "500 samples at 16 kHz");
    assert!(host.feed(&stereo(1500, 0.1, 0.1)));
    let frames = ready(&mut microphone);
    assert_eq!(frames.len(), 2, "1000 samples at 16 kHz");
    assert_eq!(frames[0].position.get(), 320);
    assert_eq!(frames[1].position.get(), 640);
}

#[test]
fn frames_at_8_and_24_khz_hold_20ms_each() {
    for (rate, samples) in [(8_000, 160), (24_000, 480)] {
        let host = FakeHost::laptop();
        let config = CaptureConfig {
            sample_rate: hz(rate),
            ..CaptureConfig::default()
        };
        let mut microphone =
            Microphone::open(&host, &DeviceSelector::Default, config).expect("the input opens");
        let mut frames = Vec::new();
        for _ in 0..10 {
            assert!(host.feed(&stereo(4800, 0.2, 0.2)));
            frames.extend(ready(&mut microphone));
        }
        assert_eq!(microphone.stats().dropped_device_frames, 0);
        assert_eq!(frames.len(), 50, "one second at {rate} Hz");
        assert!(frames.iter().all(|frame| frame.sample_count == samples));
        assert_eq!(frames[49].position.get(), 49 * u64::from(samples));
    }
}

#[test]
fn a_pcm_f32_capture_keeps_the_down_mixed_level() {
    let host = FakeHost::laptop();
    let config = CaptureConfig {
        encoding: AudioEncoding::PcmF32le,
        ..CaptureConfig::default()
    };
    let mut microphone =
        Microphone::open(&host, &DeviceSelector::Default, config).expect("the input opens");
    assert!(host.feed(&stereo(960, -0.5, 0.25)));
    let frame = microphone.try_frame().expect("one 20 ms frame");
    assert_eq!(frame.format.encoding, AudioEncoding::PcmF32le);
    assert_eq!(frame.data.len(), 320 * 4);
    assert!(
        decode(&frame)
            .expect("a well-formed frame")
            .iter()
            .all(|&s| s == -0.125)
    );
}

#[test]
fn a_tone_keeps_its_frequency_through_capture() {
    let host = FakeHost::laptop();
    let mut microphone =
        Microphone::open(&host, &DeviceSelector::Default, CaptureConfig::default())
            .expect("the default input opens");
    let tone: Vec<f32> = (0..48_000)
        .flat_map(|n| {
            let sample = (TAU * 440.0 * n as f32 / 48_000.0).sin() * 0.5;
            [sample, sample]
        })
        .collect();
    let mut samples: Vec<f32> = Vec::new();
    for chunk in tone.chunks(2 * 480) {
        assert!(host.feed(chunk));
        for frame in ready(&mut microphone) {
            samples.extend(decode(&frame).expect("a well-formed frame"));
        }
    }
    assert_eq!(microphone.stats().dropped_device_frames, 0);
    assert_eq!(samples.len(), 16_000);
    let rising = samples
        .windows(2)
        .filter(|pair| pair[0] < 0.0 && pair[1] >= 0.0)
        .count();
    assert!(
        (rising as i64 - 440).abs() <= 1,
        "{rising} rising crossings"
    );
}

#[test]
fn a_full_buffer_drops_and_counts_device_frames_and_the_clock_runs_on_without_a_gap() {
    let host = FakeHost::laptop();
    let config = CaptureConfig {
        buffer: Duration::from_millis(10),
        ..CaptureConfig::default()
    };
    let mut microphone =
        Microphone::open(&host, &DeviceSelector::Default, config).expect("the input opens");
    assert!(host.feed(&stereo(1000, 0.3, 0.3)));
    assert_eq!(
        microphone.stats().dropped_device_frames,
        520,
        "10 ms at 48 kHz is 480 kept of 1000"
    );
    assert!(
        microphone.try_frame().is_none(),
        "480 device samples make 160 at 16 kHz"
    );
    assert!(host.feed(&stereo(480, 0.3, 0.3)));
    let frame = microphone.try_frame().expect("320 samples at 16 kHz");
    assert_eq!(frame.position.get(), 0, "dropped audio leaves no gap");
    assert!(host.feed(&stereo(480, 0.3, 0.3)));
    assert!(
        microphone.try_frame().is_none(),
        "160 samples of the next frame"
    );
    assert!(host.feed(&stereo(480, 0.3, 0.3)));
    let frame = microphone.try_frame().expect("another 20 ms");
    assert_eq!(frame.position.get(), 320);
    assert_eq!(microphone.stats().dropped_device_frames, 520);
}

#[test]
fn a_callback_on_its_own_thread_loses_nothing_while_the_reader_polls_flat_out() {
    let host = FakeHost::laptop();
    let mut microphone =
        Microphone::open(&host, &DeviceSelector::Default, CaptureConfig::default())
            .expect("the default input opens");
    let device = host.clone();
    let callbacks = std::thread::spawn(move || {
        let ramp: Vec<f32> = (0..48_000)
            .flat_map(|n| {
                let sample = (n % 3_000) as f32 / 3_000.0;
                [sample, sample]
            })
            .collect();
        for chunk in ramp.chunks(2 * 480) {
            assert!(device.feed(chunk));
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let mut frames = Vec::new();
    while !callbacks.is_finished() {
        frames.extend(microphone.try_frame());
    }
    callbacks.join().expect("the callbacks ran");
    frames.extend(ready(&mut microphone));
    assert_eq!(microphone.stats().dropped_device_frames, 0);
    assert_eq!(frames.len(), 50, "one second of 20 ms frames");
    let samples: Vec<f32> = frames
        .iter()
        .flat_map(|frame| decode(frame).expect("a well-formed frame"))
        .collect();
    for (k, sample) in samples.iter().enumerate() {
        let want = ((3 * k) % 3_000) as f32 / 3_000.0;
        assert!(
            (sample - want).abs() <= 1.0 / 32_767.0,
            "sample {k} is {sample}, not {want}: the ring lost or reordered audio"
        );
    }
}

#[test]
fn next_frame_waits_no_longer_than_its_patience() {
    let host = FakeHost::laptop();
    let mut microphone =
        Microphone::open(&host, &DeviceSelector::Default, CaptureConfig::default())
            .expect("the default input opens");
    assert!(microphone.next_frame(Duration::from_millis(5)).is_none());
    assert!(host.feed(&stereo(960, 0.0, 0.0)));
    assert!(microphone.next_frame(Duration::from_millis(5)).is_some());
}

#[test]
fn a_config_whose_frame_holds_no_sample_is_refused_before_the_device_opens() {
    let host = FakeHost::laptop();
    let config = CaptureConfig {
        frame: Duration::from_micros(10),
        ..CaptureConfig::default()
    };
    assert!(matches!(
        Microphone::open(&host, &DeviceSelector::Default, config),
        Err(DeviceError::InvalidConfig(_))
    ));
    assert!(!host.input_open());
}

#[test]
fn errors_the_device_stream_reports_are_counted_with_the_last_message() {
    let host = FakeHost::laptop();
    let microphone = Microphone::open(&host, &DeviceSelector::Default, CaptureConfig::default())
        .expect("the default input opens");
    assert!(host.fail_input("first"));
    assert!(host.fail_input("device unplugged"));
    let stats = microphone.stats();
    assert_eq!(stats.stream_errors, 2);
    assert_eq!(stats.last_error.as_deref(), Some("device unplugged"));
}

#[test]
fn non_finite_device_samples_never_reach_a_frame_and_finite_headroom_is_kept() {
    let host = FakeHost::laptop();
    let config = CaptureConfig {
        encoding: AudioEncoding::PcmF32le,
        ..CaptureConfig::default()
    };
    let mut microphone =
        Microphone::open(&host, &DeviceSelector::Default, config).expect("the input opens");
    let wild = [
        f32::NAN,
        0.2,
        f32::INFINITY,
        f32::INFINITY,
        3.0,
        2.0,
        -5.0,
        f32::NEG_INFINITY,
    ];
    let audio: Vec<f32> = wild.iter().copied().cycle().take(2 * 960).collect();
    assert!(host.feed(&audio));
    let frame = microphone.try_frame().expect("one 20 ms frame");
    let samples = decode(&frame).expect("a well-formed frame");
    assert!(
        samples.iter().all(|sample| sample.is_finite()),
        "the frame carries {:?}",
        &samples[..8]
    );
    assert_eq!(
        samples[..4],
        [0.0, -1.0, 2.5, 1.0],
        "NaN is silence, an infinity full scale, and finite headroom stays"
    );
}
