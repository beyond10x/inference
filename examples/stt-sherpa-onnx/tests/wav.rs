//! The example reads WAV files of the encodings the recogniser takes into 20 ms frames, and
//! refuses what it cannot read. Needs no weights.

use std::num::{NonZeroU16, NonZeroU32};

use inference_stt::inference_audio::{AudioEncoding, AudioFormat};
use stt_sherpa_onnx::{Wav, WavRefused, frames, read_wav};

/// A WAV file with a `fmt ` chunk of format `tag`, `channels`, `rate` and `bits`, an unknown
/// chunk of odd size before it, and a `data` chunk of `data`.
fn wav(tag: u16, channels: u16, rate: u32, bits: u16, data: &[u8]) -> Vec<u8> {
    let block = channels * bits / 8;
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&tag.to_le_bytes());
    fmt.extend_from_slice(&channels.to_le_bytes());
    fmt.extend_from_slice(&rate.to_le_bytes());
    fmt.extend_from_slice(&(rate * u32::from(block)).to_le_bytes());
    fmt.extend_from_slice(&block.to_le_bytes());
    fmt.extend_from_slice(&bits.to_le_bytes());
    let mut body = b"WAVE".to_vec();
    for (id, chunk) in [(b"LIST", &b"abc"[..]), (b"fmt ", &fmt[..]), (b"data", data)] {
        body.extend_from_slice(id);
        body.extend_from_slice(&u32::try_from(chunk.len()).unwrap().to_le_bytes());
        body.extend_from_slice(chunk);
        if chunk.len() % 2 == 1 {
            body.push(0);
        }
    }
    let mut file = b"RIFF".to_vec();
    file.extend_from_slice(&u32::try_from(body.len()).unwrap().to_le_bytes());
    file.extend_from_slice(&body);
    file
}

fn format(encoding: AudioEncoding, rate: u32, channels: u16) -> AudioFormat {
    AudioFormat {
        encoding,
        sample_rate: NonZeroU32::new(rate).unwrap(),
        channels: NonZeroU16::new(channels).unwrap(),
    }
}

#[test]
fn stereo_pcm16_at_8_khz_is_read_into_20_ms_frames() {
    let data: Vec<u8> = (0..=255).cycle().take(170 * 4 + 3).collect();
    let read = read_wav(&wav(1, 2, 8_000, 16, &data)).expect("a PCM WAV file");
    assert_eq!(read.format, format(AudioEncoding::PcmS16le, 8_000, 2));
    assert_eq!(read.data, data[..170 * 4], "a partial sample group is cut");

    let frames = frames(&read);
    let counts: Vec<(u64, u32)> = frames
        .iter()
        .map(|frame| (frame.position.get(), frame.sample_count))
        .collect();
    assert_eq!(
        counts,
        [(0, 160), (160, 10)],
        "160 samples are 20 ms at 8 kHz"
    );
    assert_eq!(frames[1].data, data[160 * 4..170 * 4]);
}

#[test]
fn float_mulaw_and_alaw_are_read_and_extensible_names_its_subformat() {
    for (tag, bits, encoding) in [
        (3, 32, AudioEncoding::PcmF32le),
        (7, 8, AudioEncoding::PcmMulaw),
        (6, 8, AudioEncoding::PcmAlaw),
    ] {
        let read = read_wav(&wav(tag, 1, 16_000, bits, &[0; 8])).expect("a supported WAV file");
        assert_eq!(read.format, format(encoding, 16_000, 1));
    }

    let mut extensible = wav(0xFFFE, 1, 16_000, 16, &[0; 4]);
    let fmt = extensible
        .windows(4)
        .position(|window| window == b"fmt ")
        .unwrap();
    let mut chunk = extensible[fmt + 8..fmt + 24].to_vec();
    chunk.extend_from_slice(&22_u16.to_le_bytes());
    chunk.extend_from_slice(&16_u16.to_le_bytes());
    chunk.extend_from_slice(&0_u32.to_le_bytes());
    chunk.extend_from_slice(&1_u16.to_le_bytes());
    chunk.extend_from_slice(&[0; 14]);
    extensible.splice(fmt + 4..fmt + 24, {
        let mut sized = u32::try_from(chunk.len()).unwrap().to_le_bytes().to_vec();
        sized.extend_from_slice(&chunk);
        sized
    });
    assert_eq!(
        read_wav(&extensible),
        Ok(Wav {
            format: format(AudioEncoding::PcmS16le, 16_000, 1),
            data: vec![0; 4],
        })
    );
}

#[test]
fn what_is_not_a_supported_wav_file_is_refused() {
    assert_eq!(read_wav(b"not a wav file"), Err(WavRefused::NotWav));
    assert_eq!(
        read_wav(&wav(1, 1, 16_000, 24, &[0; 6])),
        Err(WavRefused::UnsupportedEncoding { tag: 1, bits: 24 })
    );
    assert_eq!(
        read_wav(&wav(1, 0, 16_000, 16, &[])),
        Err(WavRefused::ZeroRateOrChannels)
    );
    let mut no_data = wav(1, 1, 16_000, 16, &[]);
    let data = no_data
        .windows(4)
        .position(|window| window == b"data")
        .unwrap();
    no_data.truncate(data);
    assert_eq!(read_wav(&no_data), Err(WavRefused::MissingChunk("data")));
}
