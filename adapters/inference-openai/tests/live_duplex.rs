//! The live tests of the Realtime binding through the duplex contract. They are ignored and need
//! the `live` feature:
//!
//! ```text
//! cargo test -p inference-openai --features live --test live_duplex -- --ignored --nocapture
//! ```
//!
//! The key is resolved as `tests/live.rs` resolves it: the OS keychain through the `secrets`
//! library, secret `openai-api-key` (another name in `INFERENCE_OPENAI_SECRET_NAME`), or with
//! `INFERENCE_OPENAI_KEY_SOURCE=env` the variable `OPENAI_API_KEY`. The model is
//! `INFERENCE_OPENAI_REALTIME_MODEL`, or the default model. The key is never printed or logged.
//!
//! `one_audio_in_audio_out_session_through_the_contract` sends the speech of
//! `fixtures/speech.wav` and expects one response with audio and transcript.
//! `native_duplex_evidence_of_the_hosted_model` runs `conformance::native_duplex_evidence`: it
//! sends `fixtures/during.wav` ("And what about Portugal?") while the answer to `speech.wav` plays
//! and looks for "Portugal" in the same response. The hosted model is not expected to pass: a
//! Realtime response is generated from the conversation as it stood when the response was
//! created, and input that arrives later is answered by a later response. The test fails when the
//! model was never put to the test (the session did not open, failed, or never answered), and
//! otherwise passes when the binding's claims agree with the outcome. It prints the outcome either
//! way.

use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_duplex::conformance::{NativeDuplexProbe, native_duplex_evidence};
use inference_duplex::{
    NativeSpeechModel, SessionConfig, SessionEvent, SpeechSessionHandle, TurnDetection, TurnTuning,
};
use inference_openai::duplex::{OpenAiRealtime, native_duplex_verdict};
use inference_openai::{
    CredentialSource, Endpoint, EnvironmentSource, KeychainSource, OPENAI_KEY_NAME,
    OPENAI_KEY_VARIABLE, encode_pcm16_24k,
};

const SECRET_NAME_VARIABLE: &str = "INFERENCE_OPENAI_SECRET_NAME";
const KEY_SOURCE_VARIABLE: &str = "INFERENCE_OPENAI_KEY_SOURCE";

const SPEECH: &[u8] = include_bytes!("fixtures/speech.wav");
const DURING: &[u8] = include_bytes!("fixtures/during.wav");

fn credential() -> (Box<dyn CredentialSource>, String) {
    let name = std::env::var(SECRET_NAME_VARIABLE)
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| OPENAI_KEY_NAME.to_owned());
    let source = std::env::var(KEY_SOURCE_VARIABLE).unwrap_or_default();
    if source == "env" {
        let bindings = [(name.clone(), OPENAI_KEY_VARIABLE.to_owned())];
        (Box::new(EnvironmentSource::new(bindings)), name)
    } else {
        let keychain = KeychainSource::native().expect("the OS keychain opens");
        (Box::new(keychain), name)
    }
}

fn model() -> OpenAiRealtime {
    let (source, name) = credential();
    OpenAiRealtime::new(Endpoint::from_env(), source, name)
}

fn pcm24() -> AudioFormat {
    AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(24_000).unwrap(),
        channels: NonZeroU16::MIN,
    }
}

/// The samples, sample rate and channel count of a PCM16 WAV file.
fn read_wav(bytes: &[u8]) -> (Vec<u8>, u32, u16) {
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    let mut at = 12;
    let mut format = None;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = &bytes[at + 8..(at + 8 + size).min(bytes.len())];
        if id == b"fmt " {
            let channels = u16::from_le_bytes([body[2], body[3]]);
            let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
            format = Some((rate, channels));
        } else if id == b"data" {
            let (rate, channels) = format.expect("fmt before data");
            let whole = body.len() - body.len() % (2 * usize::from(channels));
            return (body[..whole].to_vec(), rate, channels);
        }
        at += 8 + size + size % 2;
    }
    panic!("no data chunk");
}

/// The clip as 24 kHz PCM16 frames of 100 ms from sample `from`, then `silence` of zeros. The
/// caller converts its audio to the session's format; the session never resamples.
fn frames(clip: &[u8], from: u64, silence: Duration) -> Vec<AudioFrame> {
    let (data, rate, channels) = read_wav(clip);
    let source = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(rate).unwrap(),
        channels: NonZeroU16::new(channels).unwrap(),
    };
    let step = (rate / 10) as usize * 2 * usize::from(channels);
    let mut pcm = Vec::new();
    for chunk in data.chunks(step) {
        let frame = AudioFrame {
            format: source,
            position: SampleClockPosition::new(0).unwrap(),
            sample_count: (chunk.len() / (2 * usize::from(channels))) as u32,
            data: chunk.to_vec(),
        };
        pcm.extend(encode_pcm16_24k(&frame).expect("PCM16 converts"));
    }
    let silent = (silence.as_millis() as usize) * 24 * 2;
    pcm.extend(std::iter::repeat_n(0_u8, silent));
    let mut at = from;
    pcm.chunks(4_800)
        .map(|chunk| {
            let frame = AudioFrame {
                format: pcm24(),
                position: SampleClockPosition::new(at).unwrap(),
                sample_count: (chunk.len() / 2) as u32,
                data: chunk.to_vec(),
            };
            at += u64::from(frame.sample_count);
            frame
        })
        .collect()
}

fn config(turn_detection: TurnDetection, auto_response: bool) -> SessionConfig {
    SessionConfig {
        voice: "marin".to_owned(),
        instructions: "Answer in one short sentence.".to_owned(),
        input_format: pcm24(),
        output_format: pcm24(),
        turn_detection,
        turn_tuning: TurnTuning::default(),
        auto_response,
        auto_interrupt: false,
        tools: Vec::new(),
        idle_limit: Duration::from_secs(30),
    }
}

#[test]
#[ignore = "calls the hosted Realtime API; needs the live feature and a key"]
fn one_audio_in_audio_out_session_through_the_contract() {
    let mut model = model();
    let mut session = model
        .open(config(TurnDetection::Disabled, false))
        .expect("the session opens");
    for frame in frames(SPEECH, 0, Duration::ZERO) {
        session.append_audio(frame).expect("PCM16 at 24 kHz");
    }
    session.commit_input().expect("a manual commit");
    session.create_response().expect("none in progress");
    let mut audio = 0_u64;
    let mut transcript = String::new();
    let mut heard = String::new();
    let mut idle = 0;
    let done = loop {
        match session.next_event(Duration::from_millis(500)) {
            Some(SessionEvent::OutputAudio(output)) => {
                audio += u64::from(output.frame.sample_count)
            }
            Some(SessionEvent::OutputTranscriptDelta(delta)) => transcript.push_str(&delta.text),
            Some(SessionEvent::InputTranscribed(input)) => heard = input.text,
            Some(SessionEvent::ResponseUsageRecorded(usage)) => {
                println!("usage: {:?}", usage.usage);
            }
            Some(SessionEvent::ResponseCompleted(done)) => break Some(done.response_id),
            Some(SessionEvent::SpeechSessionFailed(failed)) => panic!("failed: {}", failed.failure),
            Some(_) => {}
            None => {
                idle += 1;
                assert!(idle < 120, "no response within a minute");
            }
        }
    };
    session.close().expect("open");
    println!("heard: {heard}");
    println!("answered: {transcript} ({audio} samples at 24 kHz)");
    for error in session.server_errors() {
        println!("server error: {error}");
    }
    assert!(done.is_some());
    assert!(audio > 0, "audio out");
    assert!(!transcript.is_empty(), "a transcript of the audio");
}

#[test]
#[ignore = "calls the hosted Realtime API; needs the live feature and a key"]
fn native_duplex_evidence_of_the_hosted_model() {
    let opening = frames(SPEECH, 0, Duration::from_millis(1_500));
    let start = opening.last().map_or(0, |frame| {
        frame.position.get() + u64::from(frame.sample_count)
    });
    let probe = NativeDuplexProbe {
        config: config(TurnDetection::ServerVad, true),
        opening,
        during: frames(DURING, start, Duration::from_millis(1_500)),
        marker: "portugal".to_owned(),
        wait: Duration::from_millis(500),
        patience: 60,
    };
    let outcome = native_duplex_evidence(&mut model(), &probe);
    println!("native_duplex evidence: {outcome:?}");
    assert_eq!(
        native_duplex_verdict(&outcome),
        Ok(true),
        "the binding claims native_duplex exactly when the hosted model gives the evidence, and \
         only an outcome that put the model to the test decides it"
    );
}
