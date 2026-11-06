//! What the binding sends, and what it refuses before sending: the model and voice configured,
//! the text as given, the request's 4096-character limit as a typed error, and a configuration
//! or endpoint the endpoint cannot take.

mod speech_support;

use std::sync::Arc;

use inference_openai::speech::{
    DEFAULT_MODEL, DEFAULT_URL, MAX_INPUT_CHARACTERS, OpenAiSpeech, SpeechConfig,
    SpeechConfigError, SpeechEndpoint, SpeechError, SpeechVoice,
};
use inference_tts::inference_audio::{AudioEncoding, AudioFormat};
use inference_tts::inference_text::SegmentState;
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use serde_json::json;
use speech_support::{
    FixedKey, FixtureServer, NoKey, Script, VOICE, committed, format, within_deadline,
};

fn mulaw_8k() -> AudioFormat {
    format(AudioEncoding::PcmMulaw, 8_000)
}

/// Speaks `texts` in one synthesis and returns its events.
fn speak(synthesiser: &mut OpenAiSpeech, texts: &[&str]) -> Vec<SynthesisEvent> {
    let mut handle = synthesiser.start().unwrap();
    for segment in committed(texts) {
        handle.offer_segment(segment).unwrap();
    }
    handle.end_utterance().unwrap();
    handle.by_ref().collect()
}

#[test]
fn the_request_names_the_default_model_the_voice_and_the_text_as_given() {
    within_deadline(|| {
        let server = FixtureServer::start(Script::Serve);
        let text = "  Ihre Nummer ist 4 7 1 1.\nBitte warten…  ";
        speak(&mut server.synthesiser(mulaw_8k()), &[text]);
        assert_eq!(server.violations(), Vec::<String>::new());
        let seen = server.seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            seen[0].body,
            Some(json!({
                "model": DEFAULT_MODEL,
                "voice": VOICE,
                "input": text,
                "response_format": "pcm",
                "stream_format": "audio",
            }))
        );
        assert_eq!(DEFAULT_MODEL, "gpt-4o-mini-tts");
    });
}

#[test]
fn model_voice_instructions_and_speed_are_configurable() {
    within_deadline(|| {
        let server = FixtureServer::start(Script::Serve);
        let config = SpeechConfig::new(SpeechVoice::Custom("voice_1234".to_owned()), mulaw_8k())
            .with_model("tts-1")
            .with_instructions("Speak calmly.")
            .with_speed(1.25);
        speak(&mut server.synthesiser_with(config), &["Hello."]);
        assert_eq!(server.violations(), Vec::<String>::new());
        let body = server.seen()[0].body.clone().unwrap();
        assert_eq!(body["model"], "tts-1");
        assert_eq!(body["voice"], json!({ "id": "voice_1234" }));
        assert_eq!(body["instructions"], "Speak calmly.");
        assert_eq!(body["speed"], 1.25);
    });
}

#[test]
fn a_segment_over_the_character_limit_fails_with_a_typed_error_and_sends_nothing() {
    within_deadline(|| {
        let server = FixtureServer::start(Script::Serve);
        let mut synthesiser = server.synthesiser(mulaw_8k());
        let mut handle = synthesiser.start().unwrap();
        let long = "a".repeat(MAX_INPUT_CHARACTERS + 1);
        let segment = committed(&[&long]).remove(0);
        let id = segment.segment_id();
        handle.offer_segment(segment).unwrap();
        handle.end_utterance().unwrap();
        let events: Vec<SynthesisEvent> = handle.by_ref().collect();
        match handle.failure() {
            Some(SpeechError::InputTooLong { characters, limit }) => {
                assert_eq!(*characters, 4_097);
                assert_eq!(*limit, 4_096);
            }
            other => panic!("not InputTooLong: {other:?}"),
        }
        assert!(server.seen().is_empty(), "nothing was sent");
        assert!(
            events
                .iter()
                .all(|event| !matches!(event, SynthesisEvent::Audio(_)))
        );
        assert_eq!(events.len(), 1, "the usage record alone: {events:?}");
        let Some(SynthesisEvent::Usage(usage)) = events.last() else {
            panic!("no usage record");
        };
        assert!(!usage.r#final);
        assert_eq!(
            handle.output().segment(id).map(|s| s.state()),
            Some(SegmentState::Queued)
        );
    });
}

#[test]
fn the_limit_counts_characters_not_bytes() {
    within_deadline(|| {
        let server = FixtureServer::start(Script::Serve);
        let mut synthesiser = server.synthesiser(mulaw_8k());
        let mut handle = synthesiser.start().unwrap();
        let long = "é".repeat(MAX_INPUT_CHARACTERS);
        handle.offer_segment(committed(&[&long]).remove(0)).unwrap();
        handle.end_utterance().unwrap();
        let _: Vec<SynthesisEvent> = handle.by_ref().collect();
        assert!(handle.failure().is_none(), "{:?}", handle.failure());
        assert_eq!(server.inputs(), [long]);
        assert_eq!(server.violations(), Vec::<String>::new());
    });
}

#[test]
fn a_refused_key_fails_the_synthesis_with_the_status() {
    within_deadline(|| {
        let server = FixtureServer::start(Script::Refuse);
        let mut synthesiser = server.synthesiser(mulaw_8k());
        let mut handle = synthesiser.start().unwrap();
        handle
            .offer_segment(committed(&["Hello."]).remove(0))
            .unwrap();
        handle.end_utterance().unwrap();
        let events: Vec<SynthesisEvent> = handle.by_ref().collect();
        match handle.failure() {
            Some(error @ SpeechError::Status(401)) => {
                assert!(!error.to_string().contains(speech_support::KEY));
            }
            other => panic!("not Status(401): {other:?}"),
        }
        assert!(matches!(events.last(), Some(SynthesisEvent::Usage(_))));
    });
}

#[test]
fn a_missing_key_fails_the_synthesis_before_anything_is_sent() {
    within_deadline(|| {
        let server = FixtureServer::start(Script::Serve);
        let mut synthesiser = OpenAiSpeech::new(
            server.endpoint(),
            SpeechConfig::new(SpeechVoice::BuiltIn(VOICE.to_owned()), mulaw_8k()),
            Arc::new(NoKey),
            "openai-api-key",
        )
        .unwrap();
        let mut handle = synthesiser.start().unwrap();
        handle
            .offer_segment(committed(&["Hello."]).remove(0))
            .unwrap();
        handle.end_utterance().unwrap();
        let _: Vec<SynthesisEvent> = handle.by_ref().collect();
        assert!(
            matches!(handle.failure(), Some(SpeechError::Credential(_))),
            "{:?}",
            handle.failure()
        );
        assert!(server.seen().is_empty());
    });
}

#[test]
fn plain_http_is_taken_only_to_a_loopback_host() {
    let config = || SpeechConfig::new(SpeechVoice::BuiltIn(VOICE.to_owned()), mulaw_8k());
    let build = |url: &str| {
        OpenAiSpeech::new(
            SpeechEndpoint::new().with_url(url),
            config(),
            Arc::new(FixedKey(speech_support::KEY)),
            "openai-api-key",
        )
    };
    assert_eq!(SpeechEndpoint::new().url, DEFAULT_URL);
    assert_eq!(DEFAULT_URL, "https://api.openai.com/v1/audio/speech");
    assert!(build(DEFAULT_URL).is_ok());
    assert!(build("http://127.0.0.1:9/v1/audio/speech").is_ok());
    assert!(build("http://localhost:9/v1/audio/speech").is_ok());
    for refused in [
        "http://api.openai.com/v1/audio/speech",
        "http://user@127.0.0.1@example.com/v1/audio/speech",
        "ftp://127.0.0.1/v1/audio/speech",
        "api.openai.com/v1/audio/speech",
    ] {
        assert!(
            matches!(build(refused), Err(SpeechConfigError::InvalidEndpoint(_))),
            "{refused} was taken"
        );
    }
}

#[test]
fn a_configuration_the_endpoint_cannot_take_is_refused() {
    let voice = || SpeechVoice::BuiltIn(VOICE.to_owned());
    let refused = |config: SpeechConfig| config.validate().unwrap_err();
    for unsupported in [
        format(AudioEncoding::PcmF32le, 24_000),
        format(AudioEncoding::PcmS16le, 44_100),
        format(AudioEncoding::PcmMulaw, 22_050),
        AudioFormat {
            channels: std::num::NonZeroU16::new(2).unwrap(),
            ..format(AudioEncoding::PcmS16le, 24_000)
        },
    ] {
        assert_eq!(
            refused(SpeechConfig::new(voice(), unsupported)),
            SpeechConfigError::Output(unsupported)
        );
    }
    for speed in [0.2, 4.5, f64::NAN] {
        assert!(matches!(
            refused(SpeechConfig::new(voice(), mulaw_8k()).with_speed(speed)),
            SpeechConfigError::Speed(_)
        ));
    }
    assert_eq!(
        refused(SpeechConfig::new(
            SpeechVoice::BuiltIn(String::new()),
            mulaw_8k()
        )),
        SpeechConfigError::Voice
    );
    assert_eq!(
        refused(SpeechConfig::new(voice(), mulaw_8k()).with_model(" ")),
        SpeechConfigError::Model
    );
    assert_eq!(
        refused(SpeechConfig::new(voice(), mulaw_8k()).with_instructions("x".repeat(4_097))),
        SpeechConfigError::Instructions { characters: 4_097 }
    );
    for accepted in [
        format(AudioEncoding::PcmS16le, 8_000),
        format(AudioEncoding::PcmS16le, 16_000),
        format(AudioEncoding::PcmS16le, 24_000),
        format(AudioEncoding::PcmS16le, 48_000),
        format(AudioEncoding::PcmAlaw, 8_000),
        format(AudioEncoding::PcmMulaw, 16_000),
    ] {
        assert_eq!(SpeechConfig::new(voice(), accepted).validate(), Ok(()));
    }
}
