//! Output formats: each mapped `output_format` gives frames of its encoding and rate, mono, each
//! as long as its samples make it, carrying every byte the server sent; MP3, Opus and WAV are
//! refused when the configuration is parsed; and the configuration's ranges are held.

mod support;

use std::sync::Arc;

use inference_elevenlabs::{
    ConfigError, ElevenLabsSynthesiser, Endpoint, OutputFormat, VoiceConfig, VoiceSettings,
};
use inference_tts::inference_audio::AudioEncoding;
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use serde_json::{Value, json};
use support::{FixedKey, FixtureServer, KEY, Script, VOICE, committed};

#[test]
fn each_mapped_format_gives_frames_of_its_encoding_and_rate() {
    for (format, name, encoding, rate) in [
        (
            OutputFormat::Ulaw8000,
            "ulaw_8000",
            AudioEncoding::PcmMulaw,
            8_000,
        ),
        (
            OutputFormat::Alaw8000,
            "alaw_8000",
            AudioEncoding::PcmAlaw,
            8_000,
        ),
        (
            OutputFormat::Pcm16000,
            "pcm_16000",
            AudioEncoding::PcmS16le,
            16_000,
        ),
        (
            OutputFormat::Pcm22050,
            "pcm_22050",
            AudioEncoding::PcmS16le,
            22_050,
        ),
        (
            OutputFormat::Pcm24000,
            "pcm_24000",
            AudioEncoding::PcmS16le,
            24_000,
        ),
        (
            OutputFormat::Pcm44100,
            "pcm_44100",
            AudioEncoding::PcmS16le,
            44_100,
        ),
    ] {
        assert_eq!(format.as_str(), name);
        assert_eq!(name.parse::<OutputFormat>(), Ok(format));
        let server = FixtureServer::start(format, Script::Serve);
        let mut synthesiser = server.synthesiser(format);
        let mut handle = synthesiser.start().unwrap();
        handle
            .offer_segment(committed(&["A tone in every format."]).remove(0))
            .unwrap();
        handle.end_utterance().unwrap();
        let mut bytes = Vec::new();
        let mut next_position = 0;
        for event in handle.by_ref() {
            let SynthesisEvent::Audio(audio) = event else {
                continue;
            };
            let frame = audio.frame();
            assert_eq!(frame.format.encoding, encoding, "{name}");
            assert_eq!(frame.format.sample_rate.get(), rate, "{name}");
            assert_eq!(frame.format.channels.get(), 1, "{name}");
            assert_eq!(
                frame.data.len() as u64,
                frame.format.data_len(frame.sample_count),
                "{name}"
            );
            assert_eq!(frame.position.get(), next_position, "{name}");
            next_position += u64::from(frame.sample_count);
            bytes.extend_from_slice(&frame.data);
        }
        assert!(handle.failure().is_none(), "{name}: {:?}", handle.failure());
        assert_eq!(
            bytes,
            support::fixture_chunks(format).concat(),
            "{name}: every byte sent, in order"
        );
        assert!(
            server.seen()[0]
                .query
                .contains(&format!("output_format={name}"))
        );
        assert_eq!(server.violations(), Vec::<String>::new(), "{name}");
    }
}

#[test]
fn pcm_chunks_that_split_a_sample_are_joined_into_whole_samples() {
    let format = OutputFormat::Pcm24000;
    let server = FixtureServer::start(format, Script::Serve);
    let mut synthesiser = server.synthesiser(format);
    let mut handle = synthesiser.start().unwrap();
    handle
        .offer_segment(committed(&["Split samples."]).remove(0))
        .unwrap();
    handle.end_utterance().unwrap();
    let counts: Vec<u32> = handle
        .by_ref()
        .filter_map(|event| match event {
            SynthesisEvent::Audio(audio) => Some(audio.frame().sample_count),
            _ => None,
        })
        .collect();
    assert_eq!(counts, [1_200, 1_200, 1_200, 1_200]);
}

#[test]
fn mp3_opus_and_wav_are_refused_at_configuration() {
    for refused in [
        "mp3_44100_128",
        "mp3_22050_32",
        "opus_48000_64",
        "wav_16000",
        "pcm_48000",
    ] {
        match refused.parse::<OutputFormat>() {
            Err(ConfigError::UnsupportedOutputFormat { name, .. }) => assert_eq!(name, refused),
            other => panic!("{refused}: {other:?}"),
        }
    }
    let Err(error) = "mp3_44100_128".parse::<OutputFormat>() else {
        unreachable!()
    };
    assert!(error.to_string().contains("compressed"), "{error}");
}

fn configured(config: VoiceConfig) -> Result<ElevenLabsSynthesiser, ConfigError> {
    ElevenLabsSynthesiser::new(
        Endpoint::new(),
        config,
        Arc::new(FixedKey(KEY)),
        "elevenlabs-api-key",
    )
}

#[test]
fn the_configuration_is_held_to_the_documented_ranges() {
    let base = VoiceConfig::new(VOICE, OutputFormat::Ulaw8000);
    assert_eq!(base.model_id, "eleven_flash_v2_5");
    assert!(configured(base.clone()).is_ok());
    let mut config = base.clone();
    config.voice_id = "../other".to_owned();
    assert_eq!(configured(config).err(), Some(ConfigError::InvalidVoiceId));
    let mut config = base.clone();
    config.model_id = "model&x=1".to_owned();
    assert_eq!(configured(config).err(), Some(ConfigError::InvalidModelId));
    let mut config = base.clone();
    config.inactivity_timeout = Some(181);
    assert_eq!(
        configured(config).err(),
        Some(ConfigError::InactivityTimeout(181))
    );
    let mut config = base.clone();
    config.voice_settings = Some(VoiceSettings {
        speed: Some(1.5),
        ..VoiceSettings::default()
    });
    assert_eq!(configured(config).err(), Some(ConfigError::Speed(1.5)));
    assert!(matches!(
        ElevenLabsSynthesiser::new(
            Endpoint::new().with_url("https://api.example"),
            base,
            Arc::new(FixedKey(KEY)),
            "elevenlabs-api-key",
        ),
        Err(ConfigError::InvalidUrl(_))
    ));
}

#[test]
fn the_options_reach_the_query_and_the_voice_settings_the_first_message() {
    let format = OutputFormat::Ulaw8000;
    let server = FixtureServer::start(format, Script::Serve);
    let mut config = VoiceConfig::new(VOICE, format).with_model("eleven_flash_v2");
    config.auto_mode = Some(true);
    config.inactivity_timeout = Some(60);
    config.voice_settings = Some(VoiceSettings {
        stability: Some(0.5),
        similarity_boost: Some(0.75),
        ..VoiceSettings::default()
    });
    let mut synthesiser = ElevenLabsSynthesiser::new(
        server.endpoint(),
        config,
        Arc::new(FixedKey(KEY)),
        "elevenlabs-api-key",
    )
    .unwrap();
    let mut handle = synthesiser.start().unwrap();
    handle
        .offer_segment(committed(&["Configured."]).remove(0))
        .unwrap();
    handle.end_utterance().unwrap();
    let _: Vec<SynthesisEvent> = handle.by_ref().collect();
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
    let seen = &server.seen()[0];
    assert_eq!(
        seen.query,
        "model_id=eleven_flash_v2&output_format=ulaw_8000&auto_mode=true&inactivity_timeout=60"
    );
    let first: Value = serde_json::from_str(&seen.messages[0]).unwrap();
    assert_eq!(
        first,
        json!({"text": " ", "voice_settings": {"stability": 0.5, "similarity_boost": 0.75}})
    );
    assert_eq!(server.violations(), Vec::<String>::new());
}
