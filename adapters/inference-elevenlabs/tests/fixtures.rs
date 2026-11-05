//! The recorded fixtures under `tests/fixtures/` are the synthesized tone and nothing else: each
//! file is exactly what `support::fixture_text` makes of it. Setting
//! `INFERENCE_ELEVENLABS_WRITE_FIXTURES=1` writes them instead of checking them.

mod support;

use std::fs;
use std::path::Path;

use inference_tts::inference_audio::{AudioEncoding, decode_alaw, decode_mulaw};

#[test]
fn every_fixture_is_the_synthesized_tone_in_its_encoding() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let write = std::env::var_os("INFERENCE_ELEVENLABS_WRITE_FIXTURES").is_some();
    for (name, encoding) in [
        ("tone-ulaw.jsonl", AudioEncoding::PcmMulaw),
        ("tone-alaw.jsonl", AudioEncoding::PcmAlaw),
        ("tone-pcm16.jsonl", AudioEncoding::PcmS16le),
    ] {
        let path = directory.join(name);
        let expected = support::fixture_text(encoding);
        if write {
            fs::write(&path, &expected).unwrap();
        }
        let actual =
            fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        assert_eq!(actual, expected, "{name} is not the synthesized tone");
    }
}

#[test]
fn the_g711_fixtures_decode_back_to_the_tone_within_companding_error() {
    use inference_elevenlabs::OutputFormat;
    let tone = support::tone(8_000);
    for (format, decode) in [
        (OutputFormat::Ulaw8000, decode_mulaw as fn(u8) -> i16),
        (OutputFormat::Alaw8000, decode_alaw as fn(u8) -> i16),
    ] {
        let bytes: Vec<u8> = support::fixture_chunks(format).concat();
        assert_eq!(bytes.len(), tone.len());
        for (code, sample) in bytes.iter().zip(&tone) {
            let error = (i32::from(decode(*code)) - i32::from(*sample)).abs();
            assert!(error <= 256, "{format}: {sample} decodes {error} away");
        }
    }
}
