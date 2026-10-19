//! Adversary cases, pass 1: the wire form `ess/domains/audio.yaml` declares, read through the
//! primitives it names (`Bytes` is a padded base64 string and `Integer` an exact `i64`, in the
//! schema `ess generate --kind schema` emits for these types), and the sample clock conversion at
//! more than one channel.

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

#[test]
fn a_frame_whose_data_is_ess_bytes_deserializes() {
    let wire = json!({
        "format": {"encoding": "pcm_s16le", "sample_rate": 16_000, "channels": 1},
        "position": 32_000,
        "sample_count": 2,
        "data": "AQD/fw==",
    });

    let frame: AudioFrame = serde_json::from_value(wire).expect("an ESS-shaped frame deserializes");
    assert_eq!(frame.data, vec![0x01, 0x00, 0xff, 0x7f]);
}

#[test]
fn a_frame_serializes_its_data_as_ess_bytes() {
    let frame = AudioFrame {
        format: format(AudioEncoding::PcmS16le, 16_000, 1),
        position: position(32_000),
        sample_count: 2,
        data: vec![0x01, 0x00, 0xff, 0x7f],
    };

    let wire = serde_json::to_value(&frame).expect("serializes");
    assert_eq!(wire["data"], json!("AQD/fw=="));
}

#[test]
fn a_position_is_audio_frame_time_whatever_the_channel_count() {
    let mono = format(AudioEncoding::PcmF32le, 48_000, 1);
    let stereo = format(AudioEncoding::PcmF32le, 48_000, 2);
    let six = format(AudioEncoding::PcmF32le, 48_000, 6);

    for at in [stereo, six] {
        assert_eq!(
            position(48_000).frame_time(&at),
            Duration::from_secs(1),
            "{} channels",
            at.channels
        );
        assert_eq!(
            position(7).frame_time(&at),
            position(7).frame_time(&mono),
            "{} channels",
            at.channels
        );
    }
}

#[test]
fn a_position_serializes_within_the_ess_integer_range() {
    assert_eq!(
        SampleClockPosition::new(u64::MAX),
        None,
        "u64::MAX is beyond the ESS Integer range and cannot be constructed"
    );
    assert_eq!(SampleClockPosition::new(i64::MAX as u64 + 1), None);

    let largest = position(i64::MAX as u64);
    let wire = serde_json::to_value(largest).expect("serializes");
    assert!(
        wire.as_i64().is_some(),
        "{wire} is beyond the ESS Integer range"
    );
}

#[test]
fn a_format_beyond_the_declared_u32_rate_and_u16_channel_narrowing_is_refused() {
    for wire in [
        json!({"encoding": "pcm_s16le", "sample_rate": 4_294_967_296_u64, "channels": 1}),
        json!({"encoding": "pcm_s16le", "sample_rate": 16_000, "channels": 65_536}),
    ] {
        assert!(
            serde_json::from_value::<AudioFormat>(wire.clone()).is_err(),
            "{wire} was accepted: story:audio-vocabulary narrows sample_rate to u32 and channels \
             to u16, and the narrowing is declared in ess/domains/audio.yaml"
        );
    }
}

#[test]
fn a_format_with_a_field_the_ess_does_not_declare_is_refused() {
    let wire = json!({
        "encoding": "pcm_s16le",
        "sample_rate": 16_000,
        "channels": 1,
        "bit_depth": 16,
    });
    assert!(
        serde_json::from_value::<AudioFormat>(wire.clone()).is_err(),
        "{wire} was accepted"
    );
}
