//! The acceptance cases of the audio-vocabulary story: format equality, frame duration arithmetic
//! and the conversion of a sample clock position to audio frame time at the format's sample rate,
//! and the ESS names of `ess/domains/audio.yaml` on the wire.

use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use serde_json::json;

fn format(encoding: AudioEncoding, sample_rate: u32, channels: u16) -> AudioFormat {
    AudioFormat {
        encoding,
        sample_rate: NonZeroU32::new(sample_rate).expect("non-zero sample rate"),
        channels: NonZeroU16::new(channels).expect("non-zero channel count"),
    }
}

fn position(index: u64) -> SampleClockPosition {
    SampleClockPosition::new(index).expect("within the ESS Integer range")
}

fn frame(format: AudioFormat, index: u64, sample_count: u32) -> AudioFrame {
    AudioFrame {
        format,
        position: position(index),
        sample_count,
        data: Vec::new(),
    }
}

#[test]
fn formats_are_equal_exactly_when_encoding_sample_rate_and_channels_are() {
    let base = format(AudioEncoding::PcmS16le, 16_000, 1);

    assert_eq!(base, format(AudioEncoding::PcmS16le, 16_000, 1));
    assert_ne!(base, format(AudioEncoding::PcmF32le, 16_000, 1), "encoding");
    assert_ne!(
        base,
        format(AudioEncoding::PcmS16le, 48_000, 1),
        "sample rate"
    );
    assert_ne!(base, format(AudioEncoding::PcmS16le, 16_000, 2), "channels");
}

#[test]
fn a_frame_lasts_its_sample_count_at_the_format_sample_rate() {
    let mono_16k = format(AudioEncoding::PcmS16le, 16_000, 1);
    let stereo_48k = format(AudioEncoding::PcmF32le, 48_000, 2);

    assert_eq!(
        frame(mono_16k, 0, 320).duration(),
        Duration::from_millis(20)
    );
    assert_eq!(
        frame(stereo_48k, 0, 480).duration(),
        Duration::from_millis(10),
        "sample_count is per channel, so the channel count does not change the duration"
    );
    assert_eq!(
        frame(mono_16k, 16_000, 320).duration(),
        Duration::from_millis(20),
        "the position does not change the duration"
    );
    assert_eq!(frame(mono_16k, 0, 0).duration(), Duration::ZERO);
    assert_eq!(
        frame(stereo_48k, 0, 1).duration(),
        Duration::from_nanos(20_833),
        "one sample at 48 kHz truncates toward zero to whole nanoseconds"
    );
}

#[test]
fn position_16000_at_16_khz_is_one_second() {
    let mono_16k = format(AudioEncoding::PcmS16le, 16_000, 1);

    assert_eq!(
        position(16_000).frame_time(&mono_16k),
        Duration::from_secs(1)
    );
}

#[test]
fn position_0_is_zero() {
    let mono_16k = format(AudioEncoding::PcmS16le, 16_000, 1);
    let stereo_48k = format(AudioEncoding::PcmF32le, 48_000, 2);

    assert_eq!(position(0).frame_time(&mono_16k), Duration::ZERO);
    assert_eq!(position(0).frame_time(&stereo_48k), Duration::ZERO);
}

#[test]
fn the_same_position_at_48_khz_and_at_16_khz_differs_by_the_factor_three() {
    let at_16k = format(AudioEncoding::PcmS16le, 16_000, 1);
    let at_48k = format(AudioEncoding::PcmS16le, 48_000, 1);

    for index in [48_000, 96_000, 4_800, 3] {
        let slow = position(index).frame_time(&at_16k);
        let fast = position(index).frame_time(&at_48k);
        assert_ne!(fast, Duration::ZERO, "position {index}");
        assert_eq!(slow, fast * 3, "position {index}");
    }
    assert_eq!(position(48_000).frame_time(&at_48k), Duration::from_secs(1));
    assert_eq!(position(48_000).frame_time(&at_16k), Duration::from_secs(3));
}

#[test]
fn the_largest_position_at_the_lowest_sample_rate_converts_without_overflow() {
    let one_hertz = format(AudioEncoding::PcmS16le, 1, 1);
    let max_rate = format(AudioEncoding::PcmS16le, u32::MAX, 1);

    assert_eq!(
        position(i64::MAX as u64).frame_time(&one_hertz),
        Duration::from_secs(i64::MAX as u64)
    );
    assert_eq!(
        position(u64::from(u32::MAX) - 1).frame_time(&max_rate),
        Duration::from_nanos(999_999_999)
    );
}

#[test]
fn encodings_serialize_to_the_ess_names_in_order() {
    assert_eq!(
        serde_json::to_value(AudioEncoding::ALL).expect("serializes"),
        json!(["pcm_s16le", "pcm_f32le", "pcm_mulaw", "pcm_alaw"])
    );
}

#[test]
fn a_frame_serializes_with_the_ess_field_names() {
    let original = AudioFrame {
        format: format(AudioEncoding::PcmS16le, 16_000, 1),
        position: position(32_000),
        sample_count: 2,
        data: vec![0x01, 0x00, 0xff, 0x7f],
    };

    let wire = serde_json::to_value(&original).expect("serializes");
    assert_eq!(
        wire,
        json!({
            "format": {"encoding": "pcm_s16le", "sample_rate": 16_000, "channels": 1},
            "position": 32_000,
            "sample_count": 2,
            "data": "AQD/fw==",
        })
    );

    let back: AudioFrame = serde_json::from_value(wire).expect("deserializes");
    assert_eq!(back, original);
}

#[test]
fn a_format_with_zero_sample_rate_or_zero_channels_is_refused() {
    for wire in [
        json!({"encoding": "pcm_s16le", "sample_rate": 0, "channels": 1}),
        json!({"encoding": "pcm_s16le", "sample_rate": 16_000, "channels": 0}),
    ] {
        assert!(
            serde_json::from_value::<AudioFormat>(wire.clone()).is_err(),
            "{wire} was accepted"
        );
    }
}

#[test]
fn a_position_deserializes_only_within_0_to_i64_max() {
    let largest: SampleClockPosition =
        serde_json::from_value(json!(i64::MAX)).expect("i64::MAX deserializes");
    assert_eq!(largest.get(), i64::MAX as u64);
    for wire in [json!(-1), json!(i64::MAX as u64 + 1), json!(u64::MAX)] {
        assert!(
            serde_json::from_value::<SampleClockPosition>(wire.clone()).is_err(),
            "{wire} was accepted"
        );
    }
}

#[test]
fn a_frame_with_a_field_the_ess_does_not_declare_is_refused() {
    let mut wire = json!({
        "format": {"encoding": "pcm_s16le", "sample_rate": 16_000, "channels": 1},
        "position": 0,
        "sample_count": 0,
        "data": "",
    });
    serde_json::from_value::<AudioFrame>(wire.clone())
        .expect("the same frame without the undeclared field deserializes");

    wire["timestamp"] = json!(0);
    assert!(
        serde_json::from_value::<AudioFrame>(wire.clone()).is_err(),
        "{wire} was accepted"
    );
}

#[test]
fn frame_data_is_padded_standard_base64_both_ways() {
    let empty = frame(format(AudioEncoding::PcmS16le, 16_000, 1), 0, 0);
    assert_eq!(
        serde_json::to_value(&empty).expect("serializes")["data"],
        json!("")
    );

    for data in ["AQD/fw", "AQD_fw==", "not base64", "AQD/fw==="] {
        let wire = json!({
            "format": {"encoding": "pcm_s16le", "sample_rate": 16_000, "channels": 1},
            "position": 0,
            "sample_count": 2,
            "data": data,
        });
        assert!(
            serde_json::from_value::<AudioFrame>(wire.clone()).is_err(),
            "{wire} was accepted"
        );
    }
}

#[test]
fn a_sample_count_beyond_the_declared_u32_narrowing_is_refused() {
    let mut wire = json!({
        "format": {"encoding": "pcm_s16le", "sample_rate": 16_000, "channels": 1},
        "position": 0,
        "sample_count": u32::MAX,
        "data": "",
    });
    let largest: AudioFrame = serde_json::from_value(wire.clone()).expect("u32::MAX deserializes");
    assert_eq!(largest.sample_count, u32::MAX);

    wire["sample_count"] = json!(u64::from(u32::MAX) + 1);
    assert!(
        serde_json::from_value::<AudioFrame>(wire.clone()).is_err(),
        "{wire} was accepted: sample_count fits a u32, as ess/domains/audio.yaml declares"
    );
}
