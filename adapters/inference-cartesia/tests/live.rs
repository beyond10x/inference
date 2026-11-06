//! The live check against Cartesia's hosted endpoint, behind feature `live` and ignored: it
//! needs a key in `CARTESIA_API_KEY` (or the keychain secret `cartesia-api-key`) and a voice in
//! `CARTESIA_VOICE_ID`. It is billed. Run it with
//! `cargo test -p inference-cartesia --features live --test live -- --ignored --nocapture`.

use std::sync::Arc;
use std::time::Instant;

use inference_cartesia::{
    CARTESIA_KEY_NAME, CARTESIA_KEY_VARIABLE, CartesiaSynthesiser, CredentialSource, Endpoint,
    EnvironmentSource, KeychainSource, OutputFormat, VoiceConfig,
};
use inference_tts::inference_core::generation::{Generation, StartGeneration};
use inference_tts::inference_core::id::{NodeId, RunId};
use inference_tts::inference_text::OutputSegment;
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};

/// The key from the environment when it is set, else from the keychain.
fn source() -> Arc<dyn CredentialSource> {
    if std::env::var_os(CARTESIA_KEY_VARIABLE).is_some() {
        Arc::new(EnvironmentSource::cartesia())
    } else {
        Arc::new(KeychainSource::native().expect("a keychain"))
    }
}

#[test]
#[ignore = "billed: needs CARTESIA_API_KEY and CARTESIA_VOICE_ID"]
fn one_utterance_in_mulaw_at_8_khz() {
    let voice = std::env::var("CARTESIA_VOICE_ID").expect("CARTESIA_VOICE_ID names a voice");
    let format = OutputFormat::MULAW_8000;
    let mut synthesiser = CartesiaSynthesiser::new(
        Endpoint::new(),
        VoiceConfig::new(voice.trim(), format).with_language("en"),
        source(),
        CARTESIA_KEY_NAME,
    )
    .expect("a valid configuration");
    let mut handle = synthesiser.start().expect("a free slot");
    let (generation, _) = Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    });
    for (sequence, text) in
        (1..).zip(["Hello, this is a short test.", "The weather is fine today."])
    {
        let (tentative, _) = OutputSegment::emit(&generation, sequence, text.to_owned())
            .expect("an Active generation");
        let committed = tentative.commit().expect("a Tentative segment commits").0;
        handle
            .offer_segment(committed)
            .expect("a Committed segment");
    }
    handle.end_utterance().expect("the utterance ends once");
    let started = Instant::now();
    let mut first_audio = None;
    let mut samples = 0_u64;
    let mut next_position = 0_u64;
    let mut usage = false;
    for event in handle.by_ref() {
        match event {
            SynthesisEvent::Audio(audio) => {
                let frame = audio.frame();
                first_audio.get_or_insert_with(|| started.elapsed());
                assert_eq!(frame.format, format.audio_format());
                assert_eq!(
                    frame.data.len() as u64,
                    frame.format.data_len(frame.sample_count)
                );
                assert_eq!(frame.position.get(), next_position);
                next_position += u64::from(frame.sample_count);
                samples += u64::from(frame.sample_count);
            }
            SynthesisEvent::Usage(_) => usage = true,
            _ => {}
        }
    }
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
    assert!(usage, "the utterance ended with a usage record");
    assert!(samples > 8_000, "more than a second of speech");
    println!(
        "first audio after {:?}, {} ms of speech",
        first_audio.expect("audio arrived"),
        samples / 8
    );
}
