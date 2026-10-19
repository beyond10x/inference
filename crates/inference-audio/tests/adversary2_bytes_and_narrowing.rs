//! Adversary cases, pass 2: the corrected wire form read against what `ess/domains/audio.yaml`
//! declares. `data` is ESS `Bytes`, which `ess generate --kind schema` emits as
//! `{"type": "string", "contentEncoding": "base64", "pattern":
//! "^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$"}`; and the declared
//! narrowings of `sample_rate` (a `u32`, at least one) and `channels` (a `u16`, at least one) admit
//! their upper bounds.

use inference_audio::{AudioFormat, AudioFrame};
use serde_json::json;

fn frame_with_data(data: &str) -> serde_json::Value {
    json!({
        "format": {"encoding": "pcm_s16le", "sample_rate": 16_000, "channels": 1},
        "position": 0,
        "sample_count": 2,
        "data": data,
    })
}

#[test]
fn frame_data_with_non_zero_trailing_bits_is_refused_as_non_canonical() {
    for (data, canonical, bytes) in [
        ("AQD/fx==", "AQD/fw==", vec![0x01, 0x00, 0xff, 0x7f]),
        ("AB==", "AA==", vec![0x00]),
        ("AAB=", "AAA=", vec![0x00, 0x00]),
    ] {
        let wire = frame_with_data(data);
        assert!(
            serde_json::from_value::<AudioFrame>(wire.clone()).is_err(),
            "{wire} was accepted: {data:?} matches the ESS Bytes pattern but is not canonical; \
             story:audio-vocabulary decided that only canonical base64 is admitted (pass 2, F-A). \
             If that decision changes, flip this case to expect {bytes:?}"
        );
        let frame = serde_json::from_value::<AudioFrame>(frame_with_data(canonical))
            .unwrap_or_else(|error| panic!("{canonical:?} was refused ({error})"));
        assert_eq!(frame.data, bytes, "{canonical:?}");
    }
}

#[test]
fn a_format_at_the_top_of_the_declared_u32_rate_and_u16_channel_narrowing_deserializes() {
    let wire = json!({
        "encoding": "pcm_f32le",
        "sample_rate": u32::MAX,
        "channels": u16::MAX,
    });

    let format: AudioFormat =
        serde_json::from_value(wire.clone()).expect("the largest declared rate and channel count");
    assert_eq!(format.sample_rate.get(), u32::MAX);
    assert_eq!(format.channels.get(), u16::MAX);
    assert_eq!(serde_json::to_value(format).expect("serializes"), wire);
}
