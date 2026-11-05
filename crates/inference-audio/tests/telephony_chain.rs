//! The chain case of story `g711-codec-and-anti-aliased-resampler`: 16-bit speech-like audio at
//! 24 kHz, as a hosted voice model speaks it, resampled to 8 kHz and carried as G.711 μ-law, as a
//! phone leg carries it, then decoded, stays within a stated error of a reference computed here.
//!
//! The fixture is a sum of harmonics of a 125 Hz voice: 24 inside the telephone band, up to
//! 3 kHz, and 9 between 5 and 6 kHz that the 8 kHz leg cannot carry. The reference is the in-band
//! harmonics alone, evaluated on the 8 kHz clock and delayed by the resampler's stated delay.

use std::f64::consts::PI;
use std::num::{NonZeroU16, NonZeroU32};

use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, Resampler, SampleClockPosition};

/// The least signal-to-error ratio, in decibels, of the decoded 8 kHz μ-law leg against the
/// reference.
const CHAIN_SNR_DB: f64 = 33.0;

const F0: f64 = 125.0;

/// The harmonics of the fixture: (number, amplitude, phase).
fn harmonics() -> Vec<(u32, f64, f64)> {
    let in_band = (1..=24).map(|k| (k, 3_000.0 / f64::from(k), 0.7 * f64::from(k * k)));
    let out_of_band = (40..=48).map(|k| (k, 600.0, 0.3 * f64::from(k)));
    in_band.chain(out_of_band).collect()
}

fn voice(t: f64, harmonics: &[(u32, f64, f64)]) -> f64 {
    harmonics
        .iter()
        .map(|&(k, amplitude, phase)| amplitude * (2.0 * PI * F0 * f64::from(k) * t + phase).sin())
        .sum()
}

fn format(encoding: AudioEncoding, rate: u32) -> AudioFormat {
    AudioFormat {
        encoding,
        sample_rate: NonZeroU32::new(rate).expect("non-zero"),
        channels: NonZeroU16::new(1).expect("non-zero"),
    }
}

fn samples_of(frame: &AudioFrame) -> impl Iterator<Item = i16> + '_ {
    frame
        .data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| i16::from_le_bytes(*pair))
}

fn snr_db(signal: &[f64], error: &[f64]) -> f64 {
    let power = |values: &[f64]| values.iter().map(|v| v * v).sum::<f64>();
    10.0 * (power(signal) / power(error)).log10()
}

#[test]
fn a_24_khz_voice_carried_as_8_khz_mulaw_stays_within_the_stated_error_of_its_reference() {
    let all = harmonics();
    let in_band: Vec<_> = all.iter().copied().filter(|&(k, _, _)| k <= 24).collect();

    let source: Vec<i16> = (0..24_000)
        .map(|n| voice(f64::from(n) / 24_000.0, &all).round() as i16)
        .collect();

    let mut resampler = Resampler::new(24_000, 8_000).expect("supported rates");
    let delay = resampler.delay() as usize;
    let mut leg = Vec::new();
    let mut resampled = Vec::new();
    for (index, chunk) in source.chunks(480).enumerate() {
        let frame = AudioFrame {
            format: format(AudioEncoding::PcmS16le, 24_000),
            position: SampleClockPosition::new(index as u64 * 480).expect("in range"),
            sample_count: chunk.len() as u32,
            data: chunk.iter().flat_map(|s| s.to_le_bytes()).collect(),
        };
        let narrow = resampler.resample_frame(&frame).expect("24 kHz mono s16");
        let mulaw = narrow
            .to_encoding(AudioEncoding::PcmMulaw)
            .expect("s16 to mu-law");
        assert_eq!(mulaw.format, format(AudioEncoding::PcmMulaw, 8_000));
        let decoded = mulaw
            .to_encoding(AudioEncoding::PcmS16le)
            .expect("mu-law to s16");
        resampled.extend(samples_of(&narrow));
        leg.extend(samples_of(&decoded));
    }
    assert_eq!(leg.len(), 8_000);

    let start = 2 * delay;
    let reference: Vec<f64> = (start..leg.len())
        .map(|n| voice((n - delay) as f64 / 8_000.0, &in_band))
        .collect();
    let error_of = |samples: &[i16]| -> Vec<f64> {
        samples[start..]
            .iter()
            .zip(&reference)
            .map(|(&s, r)| f64::from(s) - r)
            .collect()
    };
    let filter_snr = snr_db(&reference, &error_of(&resampled));
    let chain_snr = snr_db(&reference, &error_of(&leg));
    println!("24 kHz -> 8 kHz: resampled SNR {filter_snr:.1} dB; through mu-law {chain_snr:.1} dB");

    assert!(
        chain_snr >= CHAIN_SNR_DB,
        "the mu-law leg is {chain_snr} dB from the reference"
    );
}
