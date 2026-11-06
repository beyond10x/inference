//! Output formats: a run that asks for μ-law at 8 kHz sends that format in every request and
//! receives μ-law frames at 8 kHz that decode back to what the server sent; the PCM encodings
//! and A-law arrive byte for byte, a sample split across chunks put back together; a rate the
//! endpoint does not take is refused before anything connects.

mod support;

use std::num::{NonZeroU16, NonZeroU32};

use inference_cartesia::{ConfigError, OutputFormat};
use inference_tts::inference_audio::{AudioEncoding, AudioFormat, decode_mulaw};
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use serde_json::json;
use support::{FixtureServer, Script, committed, tone, tone_bytes, within};

/// Speaks one segment in `format` and returns every frame's format and the bytes of them all.
fn speak(format: OutputFormat) -> (Vec<AudioFormat>, Vec<u8>, FixtureServer) {
    let server = FixtureServer::start(format, Script::Serve);
    let mut synthesiser = server.synthesiser(format);
    let mut handle = synthesiser.start().unwrap();
    handle
        .offer_segment(committed(&["Your code is four four two."]).remove(0))
        .unwrap();
    handle.end_utterance().unwrap();
    let mut formats = Vec::new();
    let mut bytes = Vec::new();
    for event in handle.by_ref() {
        if let SynthesisEvent::Audio(audio) = event {
            formats.push(audio.frame().format);
            bytes.extend_from_slice(&audio.frame().data);
        }
    }
    assert!(
        handle.failure().is_none(),
        "{format}: {:?}",
        handle.failure()
    );
    drop(handle);
    (formats, bytes, server)
}

#[test]
fn a_run_asking_for_mulaw_at_8_khz_receives_mulaw_at_8_khz() {
    within(|| {
        let (formats, bytes, server) = speak(OutputFormat::MULAW_8000);
        assert_eq!(server.violations(), Vec::<String>::new());
        let request = &server.seen()[0].requests[0];
        assert_eq!(
            request["output_format"],
            json!({"container": "raw", "encoding": "pcm_mulaw", "sample_rate": 8000})
        );
        let expected = AudioFormat {
            encoding: AudioEncoding::PcmMulaw,
            sample_rate: NonZeroU32::new(8_000).unwrap(),
            channels: NonZeroU16::new(1).unwrap(),
        };
        assert_eq!(formats.len(), 4);
        assert!(
            formats.iter().all(|format| *format == expected),
            "{formats:?}"
        );
        assert_eq!(bytes, tone_bytes(OutputFormat::MULAW_8000));
        let tone = tone(8_000);
        assert_eq!(
            bytes.len(),
            tone.len(),
            "one byte per sample, 200 ms at 8 kHz"
        );
        for (code, sample) in bytes.iter().zip(&tone) {
            let error = (i32::from(decode_mulaw(*code)) - i32::from(*sample)).abs();
            assert!(error <= 256, "{sample} decodes {error} away");
        }
    });
}

#[test]
fn alaw_and_the_pcm_encodings_arrive_byte_for_byte_across_split_samples() {
    within(|| {
        for format in [
            OutputFormat::ALAW_8000,
            OutputFormat::PCM_S16LE_16000,
            OutputFormat::PCM_S16LE_24000,
            OutputFormat::new(AudioEncoding::PcmF32le, 48_000).unwrap(),
            OutputFormat::new(AudioEncoding::PcmS16le, 22_050).unwrap(),
        ] {
            let (formats, bytes, server) = speak(format);
            assert_eq!(server.violations(), Vec::<String>::new(), "{format}");
            assert!(
                formats.iter().all(|seen| *seen == format.audio_format()),
                "{format}: {formats:?}"
            );
            assert_eq!(bytes, tone_bytes(format), "{format}");
        }
    });
}

#[test]
fn a_rate_the_endpoint_does_not_take_is_refused() {
    for rate in [0, 11_025, 32_000, 96_000] {
        assert_eq!(
            OutputFormat::new(AudioEncoding::PcmMulaw, rate),
            Err(ConfigError::UnsupportedSampleRate(rate))
        );
    }
    for rate in inference_cartesia::SAMPLE_RATES {
        assert!(OutputFormat::new(AudioEncoding::PcmS16le, rate).is_ok());
    }
}
