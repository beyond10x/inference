//! Adversary cases, pass 1, story `g711-codec-and-anti-aliased-resampler`: the G.711 codec read
//! against the standard's own tables, built here from the segment start values, step sizes and
//! decision values ITU-T G.711 lists (Tables 1a and 2a), with no shared helper, constant or
//! bit trick of the code under test; and `AudioFrame::to_encoding` over every pair of encodings,
//! several channel counts and lengths, and frames whose data disagrees with their sample count.

use std::num::{NonZeroU16, NonZeroU32};

use inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, ConversionError, SampleClockPosition, decode_alaw,
    decode_mulaw, encode_alaw, encode_mulaw,
};

/// μ-law, Table 2a, in 14-bit units: the decoder output of step 0 of each of the eight segments
/// and the step size within it.
const MU_OUTPUT_START: [i32; 8] = [0, 33, 99, 231, 495, 1023, 2079, 4191];
const MU_STEP: [i32; 8] = [2, 4, 8, 16, 32, 64, 128, 256];
/// μ-law, Table 2a: the decision value where each segment starts; segment 0 starts at 0 with its
/// first decision value at 1. 8159 is the overload point.
const MU_DECISION_START: [i32; 8] = [1, 31, 95, 223, 479, 991, 2015, 4063];
const MU_OVERLOAD: i32 = 8159;

/// A-law, Table 1a, in 13-bit units: as above.
const A_OUTPUT_START: [i32; 8] = [1, 33, 66, 132, 264, 528, 1056, 2112];
const A_STEP: [i32; 8] = [2, 2, 4, 8, 16, 32, 64, 128];
const A_DECISION_START: [i32; 8] = [0, 32, 64, 128, 256, 512, 1024, 2048];
const A_OVERLOAD: i32 = 4096;

/// The μ-law character signal of a sign, segment and step: bit 1 (the MSB) is 1 for a positive
/// value, and the segment and step bits are transmitted complemented, all-ones for the smallest.
fn mu_character(positive: bool, segment: usize, step: usize) -> u8 {
    let body = ((7 - segment) << 4) | (15 - step);
    (if positive { 0x80 } else { 0 }) | body as u8
}

/// The A-law character signal: bit 1 is 1 for a positive value, and bits 2, 4, 6 and 8 (counted
/// from the MSB as the standard counts) are inverted on the line.
fn a_character(positive: bool, segment: usize, step: usize) -> u8 {
    let plain = (if positive { 0x80 } else { 0 }) | ((segment << 4) | step) as u8;
    let mut line = plain;
    for bit_from_msb in [2, 4, 6, 8] {
        line ^= 1 << (8 - bit_from_msb);
    }
    line
}

/// Every code word with its 16-bit decoder output, from the tables above.
fn mu_table() -> Vec<(u8, i32)> {
    let mut table = Vec::new();
    for positive in [true, false] {
        for segment in 0..8 {
            for step in 0..16 {
                let value = 4 * (MU_OUTPUT_START[segment] + MU_STEP[segment] * step as i32);
                let signed = if positive { value } else { -value };
                table.push((mu_character(positive, segment, step), signed));
            }
        }
    }
    table
}

fn a_table() -> Vec<(u8, i32)> {
    let mut table = Vec::new();
    for positive in [true, false] {
        for segment in 0..8 {
            for step in 0..16 {
                let value = 8 * (A_OUTPUT_START[segment] + A_STEP[segment] * step as i32);
                let signed = if positive { value } else { -value };
                table.push((a_character(positive, segment, step), signed));
            }
        }
    }
    table
}

/// The decision interval lower bounds, in 16-bit units, of the 128 magnitudes of a law, in order.
fn lower_bounds(start: &[i32; 8], step: &[i32; 8], first_is_zero: bool, scale: i32) -> Vec<i32> {
    let mut bounds = Vec::new();
    for segment in 0..8 {
        for m in 0..16 {
            let bound = if segment == 0 && first_is_zero && m == 0 {
                0
            } else if segment == 0 && first_is_zero {
                start[0] + step[0] * (m - 1)
            } else {
                start[segment] + step[segment] * m
            };
            bounds.push(scale * bound);
        }
    }
    bounds
}

/// The magnitudes (index `segment * 16 + step`) a 16-bit magnitude may encode to: the one whose
/// decision interval holds it, or either neighbour where it lies exactly on a decision value.
fn admissible(magnitude: i32, bounds: &[i32], overload: i32) -> Vec<usize> {
    if magnitude >= overload {
        return vec![127];
    }
    let index = bounds.iter().rposition(|&b| b <= magnitude).unwrap_or(0);
    if index > 0 && bounds[index] == magnitude {
        vec![index - 1, index]
    } else {
        vec![index]
    }
}

#[test]
fn mu_law_tables_built_from_the_standard_hold_for_all_256_code_words() {
    let table = mu_table();
    let mut seen = [false; 256];
    for &(code, value) in &table {
        seen[usize::from(code)] = true;
        assert_eq!(
            i32::from(decode_mulaw(code)),
            value,
            "μ-law {code:#04x} decodes to G.711's {value}"
        );
    }
    assert!(
        seen.iter().all(|&s| s),
        "the table covers all 256 code words"
    );
    assert_eq!(
        decode_mulaw(0xFF),
        0,
        "0xFF, the silence word of ess/domains/audio.yaml"
    );
}

#[test]
fn a_law_tables_built_from_the_standard_hold_for_all_256_code_words() {
    let table = a_table();
    let mut seen = [false; 256];
    for &(code, value) in &table {
        seen[usize::from(code)] = true;
        assert_eq!(
            i32::from(decode_alaw(code)),
            value,
            "A-law {code:#04x} decodes to G.711's {value}"
        );
    }
    assert!(
        seen.iter().all(|&s| s),
        "the table covers all 256 code words"
    );
    assert_eq!(a_character(true, 0, 0), 0xD5, "the silence word");
    assert_eq!(a_character(false, 0, 0), 0x55);
}

#[test]
fn every_16_bit_sample_mu_law_encodes_into_the_decision_interval_g711_gives_it() {
    let bounds = lower_bounds(&MU_DECISION_START, &MU_STEP, true, 4);
    assert_eq!(bounds[16], 4 * 31);
    assert_eq!(bounds[127], 4 * 7903);
    for sample in i16::MIN..=i16::MAX {
        let positive = sample >= 0;
        let magnitude = i32::from(sample).abs();
        let allowed: Vec<u8> = admissible(magnitude, &bounds, 4 * MU_OVERLOAD)
            .into_iter()
            .map(|i| mu_character(positive, i / 16, i % 16))
            .collect();
        let code = encode_mulaw(sample);
        assert!(
            allowed.contains(&code),
            "μ-law {sample} encodes to {code:#04x}, G.711 allows {allowed:02x?}"
        );
    }
}

#[test]
fn every_16_bit_sample_a_law_encodes_into_the_decision_interval_g711_gives_it() {
    let bounds = lower_bounds(&A_DECISION_START, &A_STEP, false, 8);
    assert_eq!(bounds[16], 8 * 32);
    assert_eq!(bounds[127], 8 * 3968);
    for sample in i16::MIN..=i16::MAX {
        let positive = sample >= 0;
        let magnitude = i32::from(sample).abs();
        let allowed: Vec<u8> = admissible(magnitude, &bounds, 8 * A_OVERLOAD)
            .into_iter()
            .map(|i| a_character(positive, i / 16, i % 16))
            .collect();
        let code = encode_alaw(sample);
        assert!(
            allowed.contains(&code),
            "A-law {sample} encodes to {code:#04x}, G.711 allows {allowed:02x?}"
        );
    }
}

#[test]
fn every_decoder_output_re_encodes_to_its_own_code_word() {
    for (code, value) in mu_table() {
        let expected = if value == 0 { 0xFF } else { code };
        assert_eq!(
            encode_mulaw(value as i16),
            expected,
            "μ-law {code:#04x} ({value})"
        );
    }
    for (code, value) in a_table() {
        assert_eq!(
            encode_alaw(value as i16),
            code,
            "A-law {code:#04x} ({value})"
        );
    }
}

fn format(encoding: AudioEncoding, channels: u16) -> AudioFormat {
    AudioFormat {
        encoding,
        sample_rate: NonZeroU32::new(8_000).expect("non-zero"),
        channels: NonZeroU16::new(channels).expect("non-zero"),
    }
}

/// A frame of `samples` (interleaved) in `encoding`, written independently of `to_encoding`.
fn frame_of(encoding: AudioEncoding, channels: u16, samples: &[i16]) -> AudioFrame {
    let data: Vec<u8> = match encoding {
        AudioEncoding::PcmS16le => samples.iter().flat_map(|s| s.to_le_bytes()).collect(),
        AudioEncoding::PcmF32le => samples
            .iter()
            .flat_map(|&s| (f32::from(s) / 32768.0).to_le_bytes())
            .collect(),
        AudioEncoding::PcmMulaw => samples.iter().map(|&s| encode_mulaw(s)).collect(),
        AudioEncoding::PcmAlaw => samples.iter().map(|&s| encode_alaw(s)).collect(),
    };
    AudioFrame {
        format: format(encoding, channels),
        position: SampleClockPosition::new(12_345).expect("in range"),
        sample_count: (samples.len() / usize::from(channels)) as u32,
        data,
    }
}

/// The 16-bit samples a frame in a G.711 or s16 encoding carries, read through the tables above.
fn linear_of(frame: &AudioFrame) -> Vec<i32> {
    let mu = mu_table();
    let a = a_table();
    let look = |table: &[(u8, i32)], code: u8| {
        table
            .iter()
            .find(|(c, _)| *c == code)
            .map(|&(_, v)| v)
            .expect("every code word is in the table")
    };
    match frame.format.encoding {
        AudioEncoding::PcmS16le => frame
            .data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| i32::from(i16::from_le_bytes(*p)))
            .collect(),
        AudioEncoding::PcmMulaw => frame.data.iter().map(|&c| look(&mu, c)).collect(),
        AudioEncoding::PcmAlaw => frame.data.iter().map(|&c| look(&a, c)).collect(),
        AudioEncoding::PcmF32le => unreachable!("not a converted encoding"),
    }
}

#[test]
fn every_pair_of_encodings_at_every_channel_count_and_length_converts_or_refuses_typed() {
    let pool: Vec<i16> = vec![
        0,
        1,
        -1,
        7,
        -8,
        127,
        -129,
        1_000,
        -1_000,
        8_158 * 4,
        -32_768,
        32_767,
        12_345,
        -54,
        4_064 * 4,
        -2_048 * 8,
    ];
    for channels in [1u16, 2, 3, 5] {
        for frames in [0usize, 1, 3] {
            let samples: Vec<i16> = pool
                .iter()
                .cycle()
                .take(frames * usize::from(channels))
                .copied()
                .collect();
            for from in AudioEncoding::ALL {
                let input = frame_of(from, channels, &samples);
                for to in AudioEncoding::ALL {
                    let result = input.to_encoding(to);
                    if from == AudioEncoding::PcmF32le || to == AudioEncoding::PcmF32le {
                        assert_eq!(
                            result,
                            Err(ConversionError::Unsupported { from, to }),
                            "{from:?} -> {to:?}"
                        );
                        continue;
                    }
                    let out = result.unwrap_or_else(|e| panic!("{from:?} -> {to:?}: {e}"));
                    assert_eq!(out.format, format(to, channels), "{from:?} -> {to:?}");
                    assert_eq!(out.position, input.position);
                    assert_eq!(out.sample_count, input.sample_count);
                    assert_eq!(out.data.len() as u64, out.format.data_len(out.sample_count));
                    if from == to {
                        assert_eq!(out, input, "the identity is a copy");
                        continue;
                    }
                    let source = linear_of(&input);
                    let converted = linear_of(&out);
                    for (n, (&s, &c)) in source.iter().zip(&converted).enumerate() {
                        let expected = match to {
                            AudioEncoding::PcmS16le => s,
                            AudioEncoding::PcmMulaw => i32::from(decode_mulaw(encode_mulaw(
                                s.clamp(-32_768, 32_767) as i16,
                            ))),
                            AudioEncoding::PcmAlaw => {
                                i32::from(decode_alaw(encode_alaw(s.clamp(-32_768, 32_767) as i16)))
                            }
                            AudioEncoding::PcmF32le => unreachable!(),
                        };
                        assert_eq!(c, expected, "{from:?} -> {to:?}, sample {n}");
                    }
                }
            }
        }
    }
}

#[test]
fn a_frame_whose_data_disagrees_with_its_sample_count_is_refused_in_every_direction() {
    for from in AudioEncoding::ALL {
        for channels in [1u16, 2] {
            let good = frame_of(from, channels, &[1, 2, 3, 4]);
            let mut cases = Vec::new();
            let mut short = good.clone();
            short.data.pop();
            cases.push(short);
            let mut long = good.clone();
            long.data.push(0);
            cases.push(long);
            let mut more = good.clone();
            more.sample_count += 1;
            cases.push(more);
            let mut huge = good.clone();
            huge.sample_count = u32::MAX;
            cases.push(huge);
            let mut empty_claims_one = good.clone();
            empty_claims_one.data.clear();
            empty_claims_one.sample_count = 1;
            cases.push(empty_claims_one);
            for bad in cases {
                for to in AudioEncoding::ALL {
                    let expected = bad.format.data_len(bad.sample_count);
                    assert_eq!(
                        bad.to_encoding(to),
                        Err(ConversionError::DataLength {
                            expected,
                            actual: bad.data.len() as u64
                        }),
                        "{from:?} x{channels} -> {to:?}, {} samples in {} bytes",
                        bad.sample_count,
                        bad.data.len()
                    );
                }
            }
        }
    }
}
