//! Auth: the key goes in the `xi-api-key` header of the handshake and nowhere else. A handshake
//! refused with HTTP 401 is a typed error that does not contain the key, and no `Debug` output
//! or error of the binding contains it.

mod support;

use std::sync::Arc;

use inference_elevenlabs::{
    CredentialError, ElevenLabsError, ElevenLabsSynthesiser, OutputFormat, VoiceConfig,
};
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use support::{FixtureServer, KEY, NoKey, Script, VOICE, committed};

const FORMAT: OutputFormat = OutputFormat::Ulaw8000;

#[test]
fn the_key_travels_in_the_xi_api_key_header_and_in_no_message() {
    let server = FixtureServer::start(FORMAT, Script::Serve);
    let mut synthesiser = server.synthesiser(FORMAT);
    let mut handle = synthesiser.start().unwrap();
    handle
        .offer_segment(committed(&["Authorised."]).remove(0))
        .unwrap();
    handle.end_utterance().unwrap();
    let _: Vec<SynthesisEvent> = handle.by_ref().collect();
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
    let seen = server.seen();
    assert_eq!(seen[0].key.as_deref(), Some(KEY));
    assert!(
        seen[0]
            .messages
            .iter()
            .all(|message| !message.contains(KEY))
    );
    assert_eq!(server.violations(), Vec::<String>::new());
}

#[test]
fn a_handshake_refused_with_401_is_a_typed_refusal_without_the_key() {
    let server = FixtureServer::start(FORMAT, Script::Refuse);
    let mut synthesiser = server.synthesiser(FORMAT);
    let mut handle = synthesiser.start().unwrap();
    handle
        .offer_segment(committed(&["Refused."]).remove(0))
        .unwrap();
    handle.end_utterance().unwrap();
    let events: Vec<SynthesisEvent> = handle.by_ref().take(100).collect();
    let Some(ElevenLabsError::Connect(refusal)) = handle.failure() else {
        panic!("not a connect failure: {:?}", handle.failure());
    };
    assert_eq!(refusal.status, Some(401));
    assert!(refusal.is_refusal());
    let error = handle.failure().unwrap();
    for shown in [
        error.to_string(),
        format!("{error:?}"),
        format!("{handle:?}"),
        format!("{synthesiser:?}"),
    ] {
        assert!(!shown.contains(KEY), "the key shows in {shown}");
    }
    assert!(matches!(events.as_slice(), [SynthesisEvent::Usage(record)] if !record.r#final));
    assert_eq!(server.seen()[0].key.as_deref(), Some(KEY));
}

#[test]
fn a_source_without_the_key_fails_before_any_connection() {
    let server = FixtureServer::start(FORMAT, Script::Serve);
    let mut synthesiser = ElevenLabsSynthesiser::new(
        server.endpoint(),
        VoiceConfig::new(VOICE, FORMAT),
        Arc::new(NoKey),
        "elevenlabs-api-key",
    )
    .unwrap();
    let mut handle = synthesiser.start().unwrap();
    handle
        .offer_segment(committed(&["No key."]).remove(0))
        .unwrap();
    let _: Vec<SynthesisEvent> = handle.by_ref().take(100).collect();
    assert_eq!(
        handle.failure(),
        Some(&ElevenLabsError::Credential(CredentialError::Missing))
    );
    assert!(server.seen().is_empty(), "nothing connected without a key");
}

#[test]
fn the_environment_source_reads_elevenlabs_api_key() {
    assert_eq!(
        inference_elevenlabs::ELEVENLABS_KEY_VARIABLE,
        "ELEVENLABS_API_KEY"
    );
    let synthesiser = ElevenLabsSynthesiser::from_env(
        inference_elevenlabs::Endpoint::new(),
        VoiceConfig::new(VOICE, FORMAT),
    )
    .unwrap();
    assert!(format!("{synthesiser:?}").contains("elevenlabs-api-key"));
}
