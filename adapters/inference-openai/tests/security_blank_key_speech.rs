//! Conformance check of the credential contract: a blank key is refused as
//! `CredentialError::Invalid` before any request instead of a bare `Bearer ` header.

mod speech_support;

use std::sync::Arc;

use inference_openai::CredentialError;
use inference_openai::speech::{OpenAiSpeech, SpeechConfig, SpeechError, SpeechVoice};
use inference_tts::inference_audio::AudioEncoding;
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};
use speech_support::{FixedKey, FixtureServer, Script, VOICE, committed, format, within_deadline};

#[test]
fn a_blank_key_is_refused_as_invalid_before_any_request() {
    within_deadline(|| {
        for key in ["", "   ", "\t"] {
            let server = FixtureServer::start(Script::Serve);
            let mut synthesiser = OpenAiSpeech::new(
                server.endpoint(),
                SpeechConfig::new(
                    SpeechVoice::BuiltIn(VOICE.to_owned()),
                    format(AudioEncoding::PcmMulaw, 8_000),
                ),
                Arc::new(FixedKey(key)),
                "openai-api-key",
            )
            .unwrap();
            let mut handle = synthesiser.start().unwrap();
            handle
                .offer_segment(committed(&["Blank key."]).remove(0))
                .unwrap();
            handle.end_utterance().unwrap();
            let _: Vec<SynthesisEvent> = handle.by_ref().collect();
            assert!(
                matches!(
                    handle.failure(),
                    Some(SpeechError::Credential(CredentialError::Invalid))
                ),
                "key {key:?}: {:?}",
                handle.failure()
            );
            assert!(
                server.seen().is_empty(),
                "key {key:?}: a request was made with a blank key"
            );
        }
    });
}
