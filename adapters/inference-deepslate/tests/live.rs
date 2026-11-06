//! An Opal session through the `inference-duplex` contract, against the hosted service.
//!
//! Ignored and behind feature `live`: it talks to Deepslate and is billed. It needs a key in
//! `DEEPSLATE_API_KEY` (or the keychain secret `deepslate-api-key`), the vendor and organization in
//! `DEEPSLATE_VENDOR_ID` and `DEEPSLATE_ORGANIZATION_ID`, and a hosted voice in
//! `DEEPSLATE_VOICE_ID`. Run it with
//! `cargo test -p inference-deepslate --features live --test live -- --ignored --nocapture`.
//!
//! The session takes and gives 16-bit PCM at 16 kHz and streams the Nova Sonic adapter's committed
//! question "What is the capital of France?". It prints what the session delivered, and nothing
//! about the account or the key.

use std::num::{NonZeroU16, NonZeroU32};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use inference_deepslate::inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_deepslate::inference_duplex::{
    NativeSpeechModel, SessionConfig, SessionEvent, SpeechSessionHandle, TurnDetection, TurnTuning,
};
use inference_deepslate::{
    CredentialSource, DEEPSLATE_KEY_NAME, DEEPSLATE_KEY_VARIABLE, DeepslateOpal, Endpoint,
    EnvironmentSource, KeychainSource, OpalConfig,
};

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
        voice: std::env::var("DEEPSLATE_VOICE_ID")
            .expect("DEEPSLATE_VOICE_ID names a hosted voice"),
        instructions: "You are a friendly voice assistant. Answer in one short sentence."
            .to_owned(),
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

/// The key from the environment when it is set, else from the keychain.
fn source() -> Arc<dyn CredentialSource> {
    if std::env::var_os(DEEPSLATE_KEY_VARIABLE).is_some() {
        Arc::new(EnvironmentSource::deepslate())
    } else {
        Arc::new(KeychainSource::native().expect("a keychain"))
    }
}

/// The samples of the Nova Sonic adapter's committed clip `name`, 16-bit PCM at 16 kHz.
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

/// `samples` as 20 ms frames, then `silence` frames of silence.
fn frames(samples: &[u8], silence: usize) -> Vec<AudioFrame> {
    let mut data: Vec<Vec<u8>> = samples
        .chunks(FRAME_SAMPLES * 2)
        .map(<[u8]>::to_vec)
        .collect();
    data.extend(std::iter::repeat_n(vec![0; FRAME_SAMPLES * 2], silence));
    let mut position = 0;
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
#[ignore = "billed: needs DEEPSLATE_API_KEY, DEEPSLATE_VENDOR_ID, DEEPSLATE_ORGANIZATION_ID and DEEPSLATE_VOICE_ID"]
fn opal_answers_a_question_and_the_session_closes() {
    let endpoint = Endpoint::from_env().expect("the vendor and organization are set");
    let mut model = DeepslateOpal::new(endpoint, OpalConfig::new(), source(), DEEPSLATE_KEY_NAME)
        .expect("the endpoint is valid");
    let mut session = model.open(config()).expect("the session opens");
    let started = Instant::now();
    let mut answered = false;
    for frame in frames(&clip("question-16k.wav"), 400) {
        session.append_audio(frame).expect("the frame is taken");
        let paced = Instant::now() + FRAME_PERIOD;
        while let Some(event) = session.next_event(paced.saturating_duration_since(Instant::now()))
        {
            match &event {
                SessionEvent::OutputTranscriptDelta(delta) => {
                    println!("{:?} opal: {}", started.elapsed(), delta.text);
                }
                SessionEvent::OutputAudio(_) => answered = true,
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
    assert!(answered, "no audio came back");
    session.close().expect("the session closes");
}
