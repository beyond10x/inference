//! The G.711 cases of story `g711-codec-and-anti-aliased-resampler`: every μ-law and A-law code
//! word decodes to the value of ITU-T G.711's tables, every 16-bit sample survives an encode and
//! decode within its segment's quantisation step, known pairs from the standard hold, and a frame
//! converts between `pcm_s16le`, `pcm_mulaw` and `pcm_alaw`, refusing a mismatch with a typed
//! error.
//!
//! The reference values are computed here from the standard's segment rule, in the standard's own
//! units (14-bit linear for μ-law, 13-bit linear for A-law), then scaled to 16 bits. They are not
//! copied from the implementation.

use std::num::{NonZeroU16, NonZeroU32};

use inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, ConversionError, SampleClockPosition, decode_alaw,
    decode_mulaw, encode_alaw, encode_mulaw,
};

/// μ-law, G.711 Table 2a: the code word is transmitted inverted. Bit 7 of the inverted word is the
/// sign (1 negative), bits 4 to 6 the segment `e`, bits 0 to 3 the step `m`. The decoder output in
/// 14-bit units is `(2m + 33) * 2^e - 33`; a 16-bit sample is that value times 4.
fn mulaw_reference(code: u8) -> i16 {
    let word = !code;
    let negative = word & 0x80 != 0;
    let e = i32::from((word >> 4) & 0x07);
    let m = i32::from(word & 0x0F);
    let magnitude = ((2 * m + 33) << e) - 33;
    let value = 4 * magnitude;
    (if negative { -value } else { value }) as i16
}

/// The μ-law quantisation step of the segment a code word lies in, in 16-bit units: `2^(e+1)` in
/// 14-bit units.
fn mulaw_step(code: u8) -> i32 {
    let e = i32::from(((!code) >> 4) & 0x07);
    4 * (2 << e)
}

/// A-law, G.711 Table 1a: the even bits of the code word are inverted on the line (XOR 0x55). Bit 7
/// of the restored word is the sign (1 positive), bits 4 to 6 the segment `e`, bits 0 to 3 the
/// step `m`. The decoder output in 13-bit units is `2m + 1` in segment 0 and
/// `(2m + 33) * 2^(e-1)` above it; a 16-bit sample is that value times 8.
fn alaw_reference(code: u8) -> i16 {
    let word = code ^ 0x55;
    let positive = word & 0x80 != 0;
    let e = i32::from((word >> 4) & 0x07);
    let m = i32::from(word & 0x0F);
    let magnitude = if e == 0 {
        2 * m + 1
    } else {
        (2 * m + 33) << (e - 1)
    };
    let value = 8 * magnitude;
    (if positive { value } else { -value }) as i16
}

/// The A-law quantisation step of the segment a code word lies in, in 16-bit units: 2 in 13-bit
/// units for segments 0 and 1, `2^e` above.
fn alaw_step(code: u8) -> i32 {
    let e = i32::from(((code ^ 0x55) >> 4) & 0x07);
    8 * (1 << e.max(1))
}

/// The largest 16-bit magnitude μ-law represents without clipping: 8159 in 14-bit units.
const MULAW_CLIP: i32 = 8159 * 4;

#[test]
fn every_mulaw_code_word_decodes_to_the_g711_table_value() {
    for code in 0..=u8::MAX {
        assert_eq!(
            decode_mulaw(code),
            mulaw_reference(code),
            "μ-law code word {code:#04x}"
        );
    }
}

#[test]
fn every_alaw_code_word_decodes_to_the_g711_table_value() {
    for code in 0..=u8::MAX {
        assert_eq!(
            decode_alaw(code),
            alaw_reference(code),
            "A-law code word {code:#04x}"
        );
    }
}

#[test]
fn every_16_bit_sample_survives_mulaw_within_its_segments_step() {
    for sample in i16::MIN..=i16::MAX {
        let code = encode_mulaw(sample);
        let decoded = i32::from(mulaw_reference(code));
        let error = (decoded - i32::from(sample)).abs();
        let step = mulaw_step(code);
        let bound = if i32::from(sample).abs() <= MULAW_CLIP {
            step / 2
        } else {
            step
        };
        assert!(
            error <= bound,
            "sample {sample} encodes to {code:#04x}, which decodes to {decoded}: error {error} is \
             over {bound} (step {step})"
        );
        assert_eq!(
            decoded,
            i32::from(decode_mulaw(code)),
            "decode agrees with the table"
        );
    }
}

#[test]
fn every_16_bit_sample_survives_alaw_within_half_its_segments_step() {
    for sample in i16::MIN..=i16::MAX {
        let code = encode_alaw(sample);
        let decoded = i32::from(alaw_reference(code));
        let error = (decoded - i32::from(sample)).abs();
        let step = alaw_step(code);
        assert!(
            error <= step / 2,
            "sample {sample} encodes to {code:#04x}, which decodes to {decoded}: error {error} is \
             over half the step {step}"
        );
        assert_eq!(
            decoded,
            i32::from(decode_alaw(code)),
            "decode agrees with the table"
        );
    }
}

#[test]
fn known_pairs_from_the_standard_hold() {
    assert_eq!(encode_mulaw(0), 0xFF, "μ-law positive zero");
    assert_eq!(decode_mulaw(0xFF), 0);
    assert_eq!(decode_mulaw(0x7F), 0, "μ-law negative zero");
    assert_eq!(decode_mulaw(0x80), 8031 * 4, "μ-law largest positive");
    assert_eq!(decode_mulaw(0x00), -8031 * 4, "μ-law largest negative");
    assert_eq!(encode_mulaw(i16::MAX), 0x80);
    assert_eq!(encode_mulaw(i16::MIN), 0x00);
    assert_eq!(decode_mulaw(0xEF), 33 * 4, "μ-law first value of segment 1");

    assert_eq!(encode_alaw(0), 0xD5, "A-law smallest positive");
    assert_eq!(decode_alaw(0xD5), 8);
    assert_eq!(decode_alaw(0x55), -8, "A-law smallest negative");
    assert_eq!(decode_alaw(0xAA), 4032 * 8, "A-law largest positive");
    assert_eq!(decode_alaw(0x2A), -4032 * 8, "A-law largest negative");
    assert_eq!(encode_alaw(i16::MAX), 0xAA);
    assert_eq!(encode_alaw(i16::MIN), 0x2A);

    assert_eq!(encode_mulaw(0), AudioEncoding::PcmMulaw.silence());
    assert_eq!(encode_alaw(0), AudioEncoding::PcmAlaw.silence());
}

fn format(encoding: AudioEncoding, sample_rate: u32, channels: u16) -> AudioFormat {
    AudioFormat {
        encoding,
        sample_rate: NonZeroU32::new(sample_rate).expect("non-zero sample rate"),
        channels: NonZeroU16::new(channels).expect("non-zero channel count"),
    }
}

fn pcm_frame(samples: &[i16], channels: u16) -> AudioFrame {
    AudioFrame {
        format: format(AudioEncoding::PcmS16le, 8_000, channels),
        position: SampleClockPosition::new(160).expect("in range"),
        sample_count: (samples.len() / usize::from(channels)) as u32,
        data: samples.iter().flat_map(|s| s.to_le_bytes()).collect(),
    }
}

fn samples_of(frame: &AudioFrame) -> Vec<i16> {
    frame
        .data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| i16::from_le_bytes(*pair))
        .collect()
}

#[test]
fn a_pcm_frame_converts_to_mulaw_and_back_keeping_rate_channels_and_position() {
    let samples: Vec<i16> = vec![0, 1000, -1000, 32767, -32768, 12345, -54, 8];
    let frame = pcm_frame(&samples, 2);

    let mulaw = frame
        .to_encoding(AudioEncoding::PcmMulaw)
        .expect("s16 to mu-law");
    assert_eq!(mulaw.format, format(AudioEncoding::PcmMulaw, 8_000, 2));
    assert_eq!(mulaw.position, frame.position);
    assert_eq!(mulaw.sample_count, 4);
    assert_eq!(
        mulaw.data,
        samples.iter().map(|&s| encode_mulaw(s)).collect::<Vec<_>>()
    );

    let back = mulaw
        .to_encoding(AudioEncoding::PcmS16le)
        .expect("mu-law to s16");
    assert_eq!(back.format, frame.format);
    assert_eq!(
        samples_of(&back),
        mulaw
            .data
            .iter()
            .map(|&c| decode_mulaw(c))
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_pcm_frame_converts_to_alaw_and_alaw_converts_to_mulaw() {
    let samples: Vec<i16> = vec![0, 1000, -1000, 32767, -32768];
    let frame = pcm_frame(&samples, 1);

    let alaw = frame
        .to_encoding(AudioEncoding::PcmAlaw)
        .expect("s16 to A-law");
    assert_eq!(alaw.format.encoding, AudioEncoding::PcmAlaw);
    assert_eq!(
        alaw.data,
        samples.iter().map(|&s| encode_alaw(s)).collect::<Vec<_>>()
    );

    let mulaw = alaw
        .to_encoding(AudioEncoding::PcmMulaw)
        .expect("A-law to mu-law");
    assert_eq!(
        mulaw.data,
        alaw.data
            .iter()
            .map(|&c| encode_mulaw(decode_alaw(c)))
            .collect::<Vec<_>>(),
        "through the linear sample"
    );

    let same = alaw
        .to_encoding(AudioEncoding::PcmAlaw)
        .expect("same encoding");
    assert_eq!(same, alaw);
}

#[test]
fn a_frame_whose_data_does_not_match_its_sample_count_is_refused() {
    let mut frame = pcm_frame(&[1, 2, 3], 1);
    frame.data.pop();

    assert_eq!(
        frame.to_encoding(AudioEncoding::PcmMulaw),
        Err(ConversionError::DataLength {
            expected: 6,
            actual: 5
        })
    );
    assert_eq!(
        frame.to_encoding(AudioEncoding::PcmS16le),
        Err(ConversionError::DataLength {
            expected: 6,
            actual: 5
        }),
        "the same encoding is checked too"
    );
}

#[test]
fn float_pcm_is_refused_with_a_typed_error() {
    let frame = AudioFrame {
        format: format(AudioEncoding::PcmF32le, 8_000, 1),
        position: SampleClockPosition::new(0).expect("in range"),
        sample_count: 1,
        data: vec![0; 4],
    };

    let refused = frame
        .to_encoding(AudioEncoding::PcmMulaw)
        .expect_err("f32 is not converted");
    assert_eq!(
        refused,
        ConversionError::Unsupported {
            from: AudioEncoding::PcmF32le,
            to: AudioEncoding::PcmMulaw
        }
    );
    assert!(!refused.to_string().is_empty());

    assert_eq!(
        pcm_frame(&[1], 1).to_encoding(AudioEncoding::PcmF32le),
        Err(ConversionError::Unsupported {
            from: AudioEncoding::PcmS16le,
            to: AudioEncoding::PcmF32le
        })
    );
}
