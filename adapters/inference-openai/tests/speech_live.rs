//! The live test against OpenAI's hosted speech endpoint. It is ignored and needs the `live` and
//! `speech` features:
//!
//! ```text
//! cargo test -p inference-openai --features live,speech --test speech_live -- --ignored --nocapture
//! ```
//!
//! The key is resolved through the crate's credential sources, never read by this test itself:
//! from the OS keychain, secret `openai-api-key` (another name in `INFERENCE_OPENAI_SECRET_NAME`),
//! or with `INFERENCE_OPENAI_KEY_SOURCE=env` from `OPENAI_API_KEY`. It speaks one short utterance
//! as μ-law at 8 kHz, checks the format, length and positions of every frame and prints the time
//! to the first frame. The key is never printed or logged.

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::time::Instant;

use inference_openai::speech::{OpenAiSpeech, SpeechConfig, SpeechEndpoint, SpeechVoice};
use inference_openai::{
    CredentialSource, EnvironmentSource, KeychainSource, OPENAI_KEY_NAME, OPENAI_KEY_VARIABLE,
};
use inference_tts::inference_audio::{AudioEncoding, AudioFormat};
use inference_tts::inference_core::generation::{Generation, StartGeneration};
use inference_tts::inference_core::id::{NodeId, RunId};
use inference_tts::inference_text::OutputSegment;
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};

const SECRET_NAME_VARIABLE: &str = "INFERENCE_OPENAI_SECRET_NAME";
const KEY_SOURCE_VARIABLE: &str = "INFERENCE_OPENAI_KEY_SOURCE";

/// The source and the secret name the key comes from: the keychain unless the key source
/// variable is `env`.
fn credential() -> (Arc<dyn CredentialSource>, String) {
    let name = std::env::var(SECRET_NAME_VARIABLE)
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| OPENAI_KEY_NAME.to_owned());
    let source = std::env::var(KEY_SOURCE_VARIABLE).unwrap_or_default();
    if source == "env" {
        let bindings = [(name.clone(), OPENAI_KEY_VARIABLE.to_owned())];
        (Arc::new(EnvironmentSource::new(bindings)), name)
    } else {
        let keychain = KeychainSource::native().expect("the OS keychain opens");
        (Arc::new(keychain), name)
    }
}

#[test]
#[ignore = "billed: needs OPENAI_API_KEY or the keychain secret openai-api-key"]
fn speaks_an_utterance_as_mulaw_8k() {
    let wanted = AudioFormat {
        encoding: AudioEncoding::PcmMulaw,
        sample_rate: NonZeroU32::new(8_000).unwrap(),
        channels: NonZeroU16::MIN,
    };
    let (source, name) = credential();
    let mut synthesiser = OpenAiSpeech::new(
        SpeechEndpoint::new(),
        SpeechConfig::new(SpeechVoice::BuiltIn("marin".to_owned()), wanted),
        source,
        name,
    )
    .expect("the configuration is valid");
    let (generation, _) = Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    });
    let (tentative, _) = OutputSegment::emit(
        &generation,
        1,
        "Please hold, your call is important.".to_owned(),
    )
    .unwrap();
    let (segment, _) = tentative.commit().unwrap();
    let started = Instant::now();
    let mut handle = synthesiser.start().unwrap();
    handle.offer_segment(segment).unwrap();
    handle.end_utterance().unwrap();
    let mut first_frame = None;
    let mut samples = 0_u64;
    let mut usage = None;
    for event in handle.by_ref() {
        match event {
            SynthesisEvent::Audio(audio) => {
                let frame = audio.frame();
                if first_frame.is_none() {
                    first_frame = Some(started.elapsed());
                }
                assert_eq!(frame.format, wanted);
                assert_eq!(frame.data.len() as u64, wanted.data_len(frame.sample_count));
                assert_eq!(frame.position.get(), samples);
                samples += u64::from(frame.sample_count);
            }
            SynthesisEvent::Usage(record) => usage = Some(record),
            SynthesisEvent::Completed(_) => {}
        }
    }
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
    assert!(samples > 8_000 / 2, "at least half a second of speech");
    let usage = usage.expect("one usage record");
    assert!(usage.r#final);
    println!(
        "first frame after {:?}; {samples} samples at 8 kHz; usage {:?}",
        first_frame.expect("at least one frame"),
        usage.quantities
    );
}
