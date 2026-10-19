//! Amazon Nova Sonic on Amazon Bedrock through the `inference-duplex` contract.
//!
//! Ignored and behind the `live` feature: they call the service, which needs credentials from the
//! AWS SDK's default provider chain, model access in the region named by `AWS_REGION`, and costs
//! money. The model id is `INFERENCE_NOVA_SONIC_MODEL`, or the default of
//! [`inference_aws::nova_sonic::DEFAULT_MODEL_ID`]. Run them with
//! `cargo test -p inference-aws --features live --test nova_sonic_duplex_live -- --ignored`.
//!
//! Both stream `tests/fixtures/question-16k.wav` ("What is the capital of France?", the command
//! that made it is in `tests/fixtures/question-16k.txt`); the evidence probe sends
//! `tests/fixtures/during-16k.wav` ("And what about Portugal?", `during-16k.txt`) during the
//! answer, its marker said by neither the question nor the answer to it. They print what the
//! session delivered and the usage it recorded, and nothing about the account.
#![cfg(feature = "live")]

use std::num::{NonZeroU16, NonZeroU32};
use std::time::{Duration, Instant};

use inference_aws::duplex::NovaSonicSpeech;
use inference_aws::nova_sonic::redact;
use inference_duplex::conformance::{NativeDuplexProbe, NoNativeDuplex, native_duplex_evidence};
use inference_duplex::inference_audio::{
    AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition,
};
use inference_duplex::inference_core::model::Capability;
use inference_duplex::{
    NativeSpeechModel, ResponseId, ResponseState, SessionConfig, SessionEvent, SpeechSessionHandle,
    TurnDetection, TurnTuning,
};

/// Samples in one 20 ms frame at 16 kHz.
const FRAME_SAMPLES: usize = 320;
const FRAME_PERIOD: Duration = Duration::from_millis(20);

fn pcm(sample_rate: u32) -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(sample_rate).expect("a positive rate"),
        channels: NonZeroU16::MIN,
    }
}

/// The samples of the committed clip `tests/fixtures/<name>`.
fn clip_pcm(name: &str) -> Vec<u8> {
    let wav = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .expect("the speech clip is committed");
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

/// `pcm` as 20 ms frames at 16 kHz from sample `from`, the last padded with silence.
fn frames(pcm_bytes: &[u8], from: u64) -> Vec<AudioFrame> {
    pcm_bytes
        .chunks(FRAME_SAMPLES * 2)
        .enumerate()
        .map(|(index, chunk)| {
            let mut data = chunk.to_vec();
            data.resize(FRAME_SAMPLES * 2, 0);
            AudioFrame {
                format: pcm(16_000),
                position: SampleClockPosition::new(from + (index * FRAME_SAMPLES) as u64)
                    .expect("a position"),
                sample_count: FRAME_SAMPLES as u32,
                data,
            }
        })
        .collect()
}

fn silence(seconds: usize, from: u64) -> Vec<AudioFrame> {
    frames(&vec![0; seconds * 16_000 * 2], from)
}

fn config(auto_interrupt: bool) -> SessionConfig {
    SessionConfig {
        voice: "matthew".to_owned(),
        instructions:
            "You are a helpful assistant in a spoken conversation. Answer in one short sentence."
                .to_owned(),
        input_format: pcm(16_000),
        output_format: pcm(24_000),
        turn_detection: TurnDetection::ServerVad,
        turn_tuning: TurnTuning::default(),
        auto_response: true,
        auto_interrupt,
        tools: Vec::new(),
        idle_limit: Duration::from_secs(30),
    }
}

fn model() -> NovaSonicSpeech {
    assert!(
        std::env::var("AWS_REGION").is_ok_and(|region| !region.is_empty()),
        "set AWS_REGION to a region where the model is available"
    );
    let model = NovaSonicSpeech::from_env().expect("the binding's runtime starts");
    println!("backend: {}", model.adapter().backend());
    model
}

#[test]
#[ignore = "calls Amazon Bedrock: needs credentials, model access and AWS_REGION, and costs money"]
fn one_audio_in_audio_out_session_through_the_contract() {
    let mut model = model();
    let mut session = model
        .open(config(true))
        .unwrap_or_else(|error| panic!("the session opens: {error}"));

    let clip = clip_pcm("question-16k.wav");
    let mut input = frames(&clip, 0);
    let after = (input.len() * FRAME_SAMPLES) as u64;
    input.extend(silence(30, after));
    let mut events = Vec::new();
    let started = Instant::now();
    let mut completed: Option<ResponseId> = None;
    for frame in input {
        session
            .append_audio(frame)
            .unwrap_or_else(|error| panic!("the input is accepted: {error}"));
        let sent = Instant::now();
        while let Some(event) = session.next_event(Duration::ZERO) {
            if let SessionEvent::ResponseCompleted(done) = &event {
                completed = Some(done.response_id);
            }
            if let SessionEvent::SpeechSessionFailed(failed) = &event {
                panic!("the session failed: {}", failed.failure);
            }
            events.push(event);
        }
        if completed.is_some() {
            break;
        }
        std::thread::sleep(FRAME_PERIOD.saturating_sub(sent.elapsed()));
    }
    while let Some(event) = session.next_event(Duration::from_millis(500)) {
        events.push(event);
    }
    session.close().expect("the session is open");
    println!("elapsed: {:?}", started.elapsed());

    let response = completed.expect("a response completed within the 30 s of silence");
    let heard: String = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::InputTranscribed(heard) => Some(heard.text.as_str()),
            _ => None,
        })
        .collect();
    let said: String = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::OutputTranscriptDelta(delta) if delta.response_id == response => {
                Some(delta.text.as_str())
            }
            _ => None,
        })
        .collect();
    let audio: u64 = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::OutputAudio(audio) if audio.response_id == response => {
                Some(u64::from(audio.frame.sample_count))
            }
            _ => None,
        })
        .sum();
    let responses = events
        .iter()
        .filter(|event| matches!(event, SessionEvent::ResponseCreated(_)))
        .count();
    println!("user transcript: {}", redact(heard.trim()));
    println!("assistant transcript: {}", redact(said.trim()));
    println!("assistant audio: {audio} samples at 24 kHz");
    println!("responses created: {responses}");
    println!("usage: {:?}", session.ledger().usage(response));

    assert!(!heard.trim().is_empty(), "a user transcript");
    assert!(
        said.chars().any(char::is_alphabetic),
        "an assistant transcript"
    );
    assert!(audio > 0, "assistant audio on the response");
    assert_eq!(
        session.ledger().response(response).map(|r| r.state()),
        Some(ResponseState::Completed)
    );
}

/// Nova Sonic is not expected to give the evidence: it takes no caller-decided barge-in, so the
/// probe runs with the provider's own, and input during output cancels the response and is
/// answered in a new one. The adapter claims `native_duplex` only if it gave the evidence.
#[test]
#[ignore = "calls Amazon Bedrock: needs credentials, model access and AWS_REGION, and costs money"]
fn native_duplex_evidence_agrees_with_the_claim() {
    let mut model = model();
    let question = clip_pcm("question-16k.wav");
    let mut opening = frames(&question, 0);
    let after = (opening.len() * FRAME_SAMPLES) as u64;
    opening.extend(silence(2, after));
    let start = after + 32_000;
    let follow_up = clip_pcm("during-16k.wav");
    let mut during = frames(&follow_up, start);
    let after = start + (during.len() * FRAME_SAMPLES) as u64;
    during.extend(silence(2, after));
    let probe = NativeDuplexProbe {
        config: config(true),
        opening,
        during,
        marker: "portugal".to_owned(),
        wait: Duration::from_millis(500),
        patience: 60,
    };
    let result = native_duplex_evidence(&mut model, &probe);
    println!("evidence: {result:?}");
    let claimed = model
        .adapter()
        .capabilities()
        .contains(&Capability::NativeDuplex);
    assert_eq!(result.is_ok(), claimed, "{result:?}");
    assert!(
        matches!(
            result,
            Err(NoNativeDuplex::Restarted { .. }
                | NoNativeDuplex::Ended {
                    state: ResponseState::Cancelled,
                    ..
                })
        ),
        "the provider reached output and restarted on input during it: {result:?}"
    );
}
