//! Auth: the key goes in the `Authorization` header of the handshake as `Token <key>` and nowhere
//! else. A handshake refused with HTTP 401 or 403 is a typed refusal that does not contain the
//! key, and no `Debug` output or error of the binding contains it.

mod support;

use std::sync::Arc;

use inference_deepgram::{
    CredentialError, DeepgramError, DeepgramRecogniser, Endpoint, ListenConfig,
};
use inference_stt::TranscriptEvent;
use support::{FixtureServer, KEY, NoKey, Script, recognise, tone};

#[test]
fn the_key_travels_as_a_token_in_the_authorization_header_only() {
    let server = FixtureServer::start(Script::fixture("conversation.jsonl"));
    let (handle, _) = recognise(&mut server.recogniser(), tone(4));
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
    assert_eq!(
        server.seen()[0].authorization.as_deref(),
        Some(format!("Token {KEY}").as_str())
    );
    assert!(!server.seen()[0].query.contains(KEY));
    assert_eq!(server.violations(), Vec::<String>::new());
}

#[test]
fn a_handshake_refused_with_401_or_403_is_a_typed_refusal_without_the_key() {
    for status in [401, 403] {
        let server = FixtureServer::start(Script::Refuse(status));
        let mut recogniser = server.recogniser();
        let (handle, events) = recognise(&mut recogniser, tone(2));
        let Some(DeepgramError::Connect(refusal)) = handle.failure() else {
            panic!("{status}: not a connect failure: {:?}", handle.failure());
        };
        assert_eq!(refusal.status, Some(status));
        assert!(refusal.is_refusal());
        let error = handle.failure().unwrap();
        for shown in [
            error.to_string(),
            format!("{error:?}"),
            format!("{handle:?}"),
            format!("{recogniser:?}"),
        ] {
            assert!(!shown.contains(KEY), "the key shows in {shown}");
        }
        assert!(
            matches!(events.as_slice(), [event] if matches!(&event.payload, TranscriptEvent::Usage(record) if !record.r#final))
        );
        assert_eq!(
            server.seen()[0].authorization.as_deref(),
            Some(format!("Token {KEY}").as_str())
        );
    }
}

#[test]
fn a_source_without_the_key_fails_before_any_connection() {
    let server = FixtureServer::start(Script::fixture("conversation.jsonl"));
    let mut recogniser = DeepgramRecogniser::new(
        server.endpoint(),
        ListenConfig::new(),
        Arc::new(NoKey),
        "deepgram-api-key",
    )
    .unwrap();
    let (handle, _) = recognise(&mut recogniser, tone(2));
    assert_eq!(
        handle.failure(),
        Some(&DeepgramError::Credential(CredentialError::Missing))
    );
    assert!(server.seen().is_empty(), "nothing connected without a key");
}

#[test]
fn the_environment_source_reads_deepgram_api_key() {
    assert_eq!(
        inference_deepgram::DEEPGRAM_KEY_VARIABLE,
        "DEEPGRAM_API_KEY"
    );
    let recogniser = DeepgramRecogniser::from_env(Endpoint::new(), ListenConfig::new()).unwrap();
    assert!(format!("{recogniser:?}").contains("deepgram-api-key"));
}

#[test]
fn plaintext_to_a_remote_host_is_refused_without_naming_the_key() {
    let refused = DeepgramRecogniser::new(
        Endpoint::new().with_url("ws://api.deepgram.com"),
        ListenConfig::new(),
        Arc::new(support::FixedKey(KEY)),
        "deepgram-api-key",
    )
    .unwrap_err();
    assert!(matches!(
        refused,
        inference_deepgram::ConfigError::InvalidUrl(_)
    ));
    assert!(!refused.to_string().contains(KEY));
}
