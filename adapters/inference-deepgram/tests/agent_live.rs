//! A Deepgram Voice Agent session through the `inference-duplex` contract, against the hosted
//! service.
//!
//! Ignored and behind the features `live` and `duplex`: they talk to the Voice Agent and are
//! billed per minute. They need a key with Voice Agent access in `DEEPGRAM_API_KEY` (or the
//! keychain secret `deepgram-api-key`). Run them with
//! `cargo test -p inference-deepgram --features live,duplex --test agent_live -- --ignored --nocapture`.
//!
//! The session takes and gives `linear16` at 16 kHz with the default providers of
//! `AgentConfig::new`. The evidence probe streams the Nova Sonic adapter's committed clips: the
//! question "What is the capital of France?", then during the answer "And what about
//! Portugal?". They print what the session delivered, and nothing about the account or the key.

use std::num::{NonZeroU16, NonZeroU32};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use inference_deepgram::agent::AgentConfig;
use inference_deepgram::inference_duplex::conformance::{
    NativeDuplexProbe, native_duplex_evidence,
};
use inference_deepgram::inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_deepgram::inference_duplex::{
    NativeSpeechModel, SessionConfig, SessionEvent, SpeechSessionHandle, TurnDetection, TurnTuning,
};
use inference_deepgram::{
    AgentEndpoint, CredentialSource, DEEPGRAM_KEY_NAME, DeepgramAgent, EnvironmentSource,
    KeychainSource, agent_native_duplex_verdict,
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
    if std::env::var_os(inference_deepgram::DEEPGRAM_KEY_VARIABLE).is_some() {
        Arc::new(EnvironmentSource::deepgram())
    } else {
        Arc::new(KeychainSource::native().expect("a keychain"))
    }
}

fn agent() -> DeepgramAgent {
    DeepgramAgent::new(
        AgentEndpoint::new(),
        AgentConfig::new(),
        source(),
        DEEPGRAM_KEY_NAME,
    )
    .expect("the default configuration is valid")
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
#[ignore = "talks to the hosted Voice Agent and is billed; needs a key with Voice Agent access in DEEPGRAM_API_KEY"]
fn the_voice_agent_answers_a_question_and_the_session_closes() {
    let mut model = agent();
    let mut session = model.open(config()).expect("the session opens");
    let started = Instant::now();
    let mut answered = false;
    for frame in frames(&clip("question-16k.wav"), 0, 400) {
        session.append_audio(frame).expect("the frame is taken");
        let paced = Instant::now() + FRAME_PERIOD;
        while let Some(event) = session.next_event(paced.saturating_duration_since(Instant::now()))
        {
            match &event {
                SessionEvent::OutputTranscriptDelta(delta) => {
                    println!("{:?} agent: {}", started.elapsed(), delta.text);
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
    println!("warnings: {:?}", session.warnings());
    session.close().expect("the session closes");
    assert!(answered, "the agent said nothing");
}

#[test]
#[ignore = "talks to the hosted Voice Agent and is billed; needs a key with Voice Agent access in DEEPGRAM_API_KEY"]
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
    let outcome = native_duplex_evidence(&mut agent(), &probe);
    println!("{outcome:?}");
    assert_eq!(agent_native_duplex_verdict(&outcome), Ok(true));
}
