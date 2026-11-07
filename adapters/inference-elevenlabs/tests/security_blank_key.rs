//! Conformance check of the credential contract: a blank key is refused as
//! `CredentialError::Invalid` before any connection, as the Deepgram, Deepslate and Phonic
//! bindings already refuse it. A blank key cannot authenticate, and sending it puts an empty
//! `xi-api-key` header on the wire instead of a typed refusal.

mod support;

use std::sync::Arc;

use inference_elevenlabs::{
    CredentialError, ElevenLabsError, ElevenLabsSynthesiser, OutputFormat, VoiceConfig,
};
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use support::{FixedKey, FixtureServer, Script, VOICE, committed};

const FORMAT: OutputFormat = OutputFormat::Ulaw8000;

#[test]
fn a_blank_key_is_refused_as_invalid_before_any_connection() {
    for key in ["", "   ", "\t"] {
        let server = FixtureServer::start(FORMAT, Script::Serve);
        let mut synthesiser = ElevenLabsSynthesiser::new(
            server.endpoint(),
            VoiceConfig::new(VOICE, FORMAT),
            Arc::new(FixedKey(key)),
            "elevenlabs-api-key",
        )
        .unwrap();
        let mut handle = synthesiser.start().unwrap();
        handle
            .offer_segment(committed(&["Blank key."]).remove(0))
            .unwrap();
        handle.end_utterance().unwrap();
        let _: Vec<SynthesisEvent> = handle.by_ref().take(100).collect();
        assert_eq!(
            handle.failure(),
            Some(&ElevenLabsError::Credential(CredentialError::Invalid)),
            "key {key:?}: expected a typed credential refusal"
        );
        assert!(
            server.seen().is_empty(),
            "key {key:?}: a handshake was made with a blank key"
        );
    }
}
