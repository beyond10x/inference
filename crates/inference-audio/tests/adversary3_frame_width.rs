//! Adversary pass 3 of the frame-length decision (`story:tts-contract`, `## Frame length`):
//! `AudioFormat::data_len` is `sample_count * channels * width(encoding)`, with literal expected
//! values for both encodings, one and several channels, and the largest sample count and channel
//! count the types admit. No case of this crate exercised `width` or `data_len` before.

use std::num::{NonZeroU16, NonZeroU32};

use inference_audio::{AudioEncoding, AudioFormat};

fn format(encoding: AudioEncoding, channels: u16) -> AudioFormat {
    AudioFormat {
        encoding,
        sample_rate: NonZeroU32::new(48_000).expect("not zero"),
        channels: NonZeroU16::new(channels).expect("not zero"),
    }
}

#[test]
fn data_len_is_samples_times_channels_times_encoding_width() {
    let cases: [(AudioEncoding, u16, u32, u64); 7] = [
        (AudioEncoding::PcmS16le, 1, 0, 0),
        (AudioEncoding::PcmS16le, 1, 160, 320),
        (AudioEncoding::PcmS16le, 2, 160, 640),
        (AudioEncoding::PcmF32le, 1, 160, 640),
        (AudioEncoding::PcmF32le, 2, 3, 24),
        (AudioEncoding::PcmF32le, 6, 7, 168),
        (
            AudioEncoding::PcmF32le,
            u16::MAX,
            u32::MAX,
            1_125_882_726_711_300,
        ),
    ];
    for (encoding, channels, samples, expected) in cases {
        assert_eq!(
            format(encoding, channels).data_len(samples),
            expected,
            "{encoding:?}, {channels} channel(s), {samples} samples"
        );
    }
}
