//! A Phonic conversation through the `inference-duplex` contract, against the hosted service.
//!
//! Ignored and behind feature `live`: they talk to Phonic and are billed. The key is
//! `PHONIC_API_KEY`, or the keychain secret `phonic-api-key` when the variable is unset. Run them
//! with `cargo test -p inference-phonic --features live --test live -- --ignored`.
//!
//! The clips are the Nova Sonic adapter's committed ones, 16-bit PCM at 16 kHz: the question
//! "What is the capital of France?", then during the answer "And what about Portugal?". They
//! print what the session delivered, and nothing about the account.

use std::num::{NonZeroU16, NonZeroU32};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use inference_phonic::inference_duplex::conformance::{NativeDuplexProbe, native_duplex_evidence};
use inference_phonic::inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_phonic::inference_duplex::{
    NativeSpeechModel, SessionConfig, SessionEvent, SpeechSessionHandle, TurnDetection, TurnTuning,
};
use inference_phonic::{
    CredentialSource, EnvironmentSource, KeychainSource, PHONIC_KEY_NAME, PHONIC_KEY_VARIABLE,
    PhonicConfig, PhonicEndpoint, PhonicSpeech, native_duplex_verdict,
};

/// Samples in one 20 ms frame at 16 kHz.
const FRAME_SAMPLES: usize = 320;
const FRAME_PERIOD: Duration = Duration::from_millis(20);

fn pcm16k() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).expect("a positive rate"),
        channels: NonZeroU16::MIN,
    }
}

fn config() -> SessionConfig {
    SessionConfig {
        voice: String::new(),
        instructions: "Answer in one short sentence.".to_owned(),
        input_format: pcm16k(),
        output_format: pcm16k(),
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt: true,
        tools: Vec::new(),
        idle_limit: Duration::from_secs(30),
    }
}

/// The model, its key from `PHONIC_API_KEY` or else the keychain.
fn model() -> PhonicSpeech {
    let source: Arc<dyn CredentialSource> = if std::env::var_os(PHONIC_KEY_VARIABLE).is_some() {
        Arc::new(EnvironmentSource::phonic())
    } else {
        Arc::new(KeychainSource::native().expect("the keychain opens"))
    };
    PhonicSpeech::new(
        PhonicEndpoint::new(),
        PhonicConfig::new(),
        source,
        PHONIC_KEY_NAME,
    )
    .expect("the default endpoint is valid")
}

/// The samples of the Nova Sonic adapter's committed clip `name`.
fn clip(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../inference-aws/tests/fixtures")
        .join(name);
    let wav = std::fs::read(&path).expect("the speech clip is committed");
    let mut at = 12;
    while at + 8 <= wav.len() {
        let size =
            u32::from_le_bytes([wav[at + 4], wav[at + 5], wav[at + 6], wav[at + 7]]) as usize;
        if &wav[at..at + 4] == b"data" {
            return wav[at + 8..(at + 8 + size).min(wav.len())].to_vec();
        }
        at += 8 + size + size % 2;
    }
    panic!("the speech clip has no data chunk");
}

/// `samples` as 20 ms frames from `start`, then `silence` frames of silence.
fn frames(samples: &[u8], start: u64, silence: usize) -> Vec<AudioFrame> {
    let mut data: Vec<Vec<u8>> = samples
        .chunks(FRAME_SAMPLES * 2)
        .map(<[u8]>::to_vec)
        .collect();
    data.extend(std::iter::repeat_n(vec![0; FRAME_SAMPLES * 2], silence));
    let mut position = start;
    data.into_iter()
        .map(|data| {
            let sample_count = u32::try_from(data.len() / 2).expect("a short frame");
            let frame = AudioFrame {
                format: pcm16k(),
                position: SampleClockPosition::new(position).expect("a position"),
                sample_count,
                data,
            };
            position += u64::from(sample_count);
            frame
        })
        .collect()
}

#[test]
#[ignore = "billed: needs PHONIC_API_KEY or the keychain secret phonic-api-key"]
fn phonic_answers_a_question_and_the_session_closes() {
    let mut model = model();
    let mut session = model
        .open(config())
        .unwrap_or_else(|error| panic!("{error}: {:?}", model.open_error()));
    let started = Instant::now();
    let mut answered = false;
    for frame in frames(&clip("question-16k.wav"), 0, 400) {
        session.append_audio(frame).expect("the frame is taken");
        let paced = Instant::now() + FRAME_PERIOD;
        while let Some(event) = session.next_event(paced.saturating_duration_since(Instant::now()))
        {
            match &event {
                SessionEvent::OutputTranscriptDelta(delta) => {
                    println!("{:?} assistant: {}", started.elapsed(), delta.text);
                    answered = true;
                }
                SessionEvent::InputTranscribed(heard) => {
                    println!("{:?} user: {}", started.elapsed(), heard.text);
                }
                SessionEvent::SpeechSessionFailed(failed) => panic!("{}", failed.failure),
                _ => {}
            }
        }
        if answered {
            break;
        }
    }
    session.close().expect("the session closes");
    assert!(answered, "Phonic said nothing");
}

#[test]
#[ignore = "billed: needs PHONIC_API_KEY or the keychain secret phonic-api-key"]
fn native_duplex_evidence_agrees_with_the_claims() {
    let opening = frames(&clip("question-16k.wav"), 0, 50);
    let start = opening
        .iter()
        .map(|frame| u64::from(frame.sample_count))
        .sum();
    let probe = NativeDuplexProbe {
        config: config(),
        opening,
        during: frames(&clip("during-16k.wav"), start, 50),
        marker: "portugal".to_owned(),
        wait: Duration::from_millis(50),
        patience: 400,
    };
    let outcome = native_duplex_evidence(&mut model(), &probe);
    println!("{outcome:?}");
    assert_eq!(native_duplex_verdict(&outcome), Ok(true));
}
