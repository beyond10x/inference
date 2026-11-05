//! The live check against the hosted endpoint, billed:
//!
//! ```text
//! cargo run -p inference-elevenlabs --features live --example live
//! ```
//!
//! It needs a key with text-to-speech access, in `ELEVENLABS_API_KEY` or else in the keychain
//! secret `elevenlabs-api-key`, and a voice in `ELEVENLABS_VOICE_ID`. Without either it exits
//! with status 2 and says what is missing; it is never skipped. It speaks one short utterance in
//! `ulaw_8000` and one in `pcm_24000`, checks that every frame has the format's encoding, rate
//! and one channel, carries the bytes its samples make and follows the frame before it, that the
//! utterance completed with a final usage record, and prints the time to first audio. Any check
//! that fails exits with status 1.

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use inference_elevenlabs::{
    CredentialSource, ELEVENLABS_KEY_NAME, ELEVENLABS_KEY_VARIABLE, ElevenLabsSynthesiser,
    Endpoint, EnvironmentSource, KeychainSource, OutputFormat, VoiceConfig,
};
use inference_tts::inference_core::generation::{Generation, StartGeneration};
use inference_tts::inference_core::id::{NodeId, RunId};
use inference_tts::inference_text::OutputSegment;
use inference_tts::{SynthesisEvent, SynthesisHandle, Synthesiser};

const VOICE_VARIABLE: &str = "ELEVENLABS_VOICE_ID";

const SENTENCES: [&str; 2] = ["Hello, this is a short test.", "The weather is fine today."];

fn main() -> ExitCode {
    let Some(voice) = std::env::var(VOICE_VARIABLE)
        .ok()
        .filter(|voice| !voice.trim().is_empty())
    else {
        eprintln!("live: set {VOICE_VARIABLE} to a voice id the key can use");
        return ExitCode::from(2);
    };
    let source: Arc<dyn CredentialSource> = if std::env::var_os(ELEVENLABS_KEY_VARIABLE).is_some() {
        Arc::new(EnvironmentSource::elevenlabs())
    } else {
        match KeychainSource::native() {
            Ok(source) => Arc::new(source),
            Err(error) => {
                eprintln!(
                    "live: {ELEVENLABS_KEY_VARIABLE} is unset and the keychain cannot be opened ({error})"
                );
                return ExitCode::from(2);
            }
        }
    };
    let mut failed = false;
    for format in [OutputFormat::Ulaw8000, OutputFormat::Pcm24000] {
        match speak(&voice, format, Arc::clone(&source)) {
            Ok(report) => println!("{format}: {report}"),
            Err(Problem::Missing(why)) => {
                eprintln!("live: {why}");
                return ExitCode::from(2);
            }
            Err(Problem::Failed(why)) => {
                eprintln!("live: {format}: {why}");
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

enum Problem {
    Missing(String),
    Failed(String),
}

fn speak(
    voice: &str,
    format: OutputFormat,
    source: Arc<dyn CredentialSource>,
) -> Result<String, Problem> {
    let mut synthesiser = ElevenLabsSynthesiser::new(
        Endpoint::new(),
        VoiceConfig::new(voice, format),
        source,
        ELEVENLABS_KEY_NAME,
    )
    .map_err(|error| Problem::Failed(error.to_string()))?;
    let expected = synthesiser.format();
    let mut handle = synthesiser
        .start()
        .map_err(|error| Problem::Failed(error.to_string()))?;
    let started = Instant::now();
    let (generation, _) = Generation::start(StartGeneration {
        run_id: RunId::generate(),
        node_id: NodeId::generate(),
        epoch: 1,
    });
    for (sentence, sequence) in SENTENCES.iter().zip(1..) {
        let (tentative, _) = OutputSegment::emit(&generation, sequence, (*sentence).to_owned())
            .map_err(|error| Problem::Failed(format!("{error:?}")))?;
        let (segment, _) = tentative
            .commit()
            .map_err(|error| Problem::Failed(format!("{error:?}")))?;
        handle
            .offer_segment(segment)
            .map_err(|error| Problem::Failed(error.to_string()))?;
    }
    handle
        .end_utterance()
        .map_err(|error| Problem::Failed(error.to_string()))?;

    let mut first_audio = None;
    let mut frames = 0_usize;
    let mut samples = 0_u64;
    let mut usage_final = None;
    for event in handle.by_ref() {
        match event {
            SynthesisEvent::Audio(audio) => {
                let frame = audio.frame();
                first_audio.get_or_insert_with(|| started.elapsed());
                if frame.format != expected {
                    return Err(Problem::Failed(format!(
                        "frame {frames} is {:?}, expected {expected:?}",
                        frame.format
                    )));
                }
                let length = frame.format.data_len(frame.sample_count);
                if frame.data.len() as u64 != length || frame.sample_count == 0 {
                    return Err(Problem::Failed(format!(
                        "frame {frames} carries {} bytes for {} samples, which make {length}",
                        frame.data.len(),
                        frame.sample_count
                    )));
                }
                if frame.position.get() != samples {
                    return Err(Problem::Failed(format!(
                        "frame {frames} starts at {} where the last ended at {samples}",
                        frame.position.get()
                    )));
                }
                samples += u64::from(frame.sample_count);
                frames += 1;
            }
            SynthesisEvent::Completed(_) => {}
            SynthesisEvent::Usage(record) => usage_final = Some(record.r#final),
        }
    }
    if let Some(failure) = handle.failure() {
        if let inference_elevenlabs::ElevenLabsError::Credential(error) = failure {
            return Err(Problem::Missing(format!(
                "no key: {ELEVENLABS_KEY_VARIABLE} is unset and the keychain secret {ELEVENLABS_KEY_NAME} gave {error}"
            )));
        }
        return Err(Problem::Failed(failure.to_string()));
    }
    if usage_final != Some(true) {
        return Err(Problem::Failed(
            "the utterance ended without a final usage record".to_owned(),
        ));
    }
    let Some(first_audio) = first_audio else {
        return Err(Problem::Failed("no audio".to_owned()));
    };
    let seconds = samples as f64 / f64::from(expected.sample_rate.get());
    Ok(format!(
        "first audio after {} ms, {frames} frames, {seconds:.2} s of audio, all {} at {} Hz mono",
        first_audio.as_millis(),
        match expected.encoding {
            inference_tts::inference_audio::AudioEncoding::PcmMulaw => "mu-law",
            inference_tts::inference_audio::AudioEncoding::PcmAlaw => "A-law",
            _ => "16-bit little-endian PCM",
        },
        expected.sample_rate.get()
    ))
}
