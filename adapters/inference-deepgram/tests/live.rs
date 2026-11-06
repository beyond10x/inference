//! The live check against Deepgram's hosted endpoint, behind feature `live` and ignored: it needs
//! a key with speech-to-text access in `DEEPGRAM_API_KEY` (or the keychain secret
//! `deepgram-api-key`) and a 16-bit mono PCM WAV file of speech in `INFERENCE_DEEPGRAM_STT_CLIP`.
//! It is billed. Run it with
//! `cargo test -p inference-deepgram --features live --test live -- --ignored --nocapture`.

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::time::{Duration, Instant};

use inference_deepgram::{
    CredentialSource, DEEPGRAM_KEY_NAME, DeepgramRecogniser, Endpoint, EnvironmentSource,
    KeychainSource, ListenConfig,
};
use inference_stt::inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_stt::inference_core::id::{NodeId, RunId, StreamId};
use inference_stt::inference_text::TurnId;
use inference_stt::{Recogniser, RecognitionHandle, RecognitionRequest, TranscriptEvent};

/// The key from the environment when it is set, else from the keychain.
fn source() -> Arc<dyn CredentialSource> {
    if std::env::var_os(inference_deepgram::DEEPGRAM_KEY_VARIABLE).is_some() {
        Arc::new(EnvironmentSource::deepgram())
    } else {
        Arc::new(KeychainSource::native().expect("a keychain"))
    }
}

/// The samples of a 16-bit mono PCM WAV file and its rate.
fn wav(path: &str) -> (Vec<u8>, u32) {
    let bytes = std::fs::read(path).unwrap_or_else(|error| panic!("{path}: {error}"));
    assert_eq!(&bytes[0..4], b"RIFF", "{path} is not a WAV file");
    let mut at = 12;
    let mut rate = None;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = &bytes[at + 8..(at + 8 + size).min(bytes.len())];
        if id == b"fmt " {
            assert_eq!(u16::from_le_bytes([body[0], body[1]]), 1, "PCM");
            assert_eq!(u16::from_le_bytes([body[2], body[3]]), 1, "mono");
            assert_eq!(u16::from_le_bytes([body[14], body[15]]), 16, "16-bit");
            rate = Some(u32::from_le_bytes(body[4..8].try_into().unwrap()));
        }
        if id == b"data" {
            return (body.to_vec(), rate.expect("fmt before data"));
        }
        at += 8 + size + size % 2;
    }
    panic!("{path} has no data chunk");
}

#[test]
#[ignore = "billed: needs DEEPGRAM_API_KEY with speech-to-text access and INFERENCE_DEEPGRAM_STT_CLIP"]
fn a_clip_is_transcribed_with_interim_results_and_a_committed_span() {
    let clip = std::env::var("INFERENCE_DEEPGRAM_STT_CLIP").expect("INFERENCE_DEEPGRAM_STT_CLIP");
    let (data, rate) = wav(&clip);
    let format = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(rate).expect("a rate"),
        channels: NonZeroU16::MIN,
    };
    let samples_per_frame = (rate / 50) as usize;
    let mut recogniser = DeepgramRecogniser::new(
        Endpoint::new(),
        ListenConfig {
            utterance_end_ms: Some(1_000),
            ..ListenConfig::new()
        },
        source(),
        DEEPGRAM_KEY_NAME,
    )
    .expect("the default configuration is valid");
    let mut handle = recogniser
        .start(RecognitionRequest::new(
            RunId::generate(),
            TurnId::generate(),
            StreamId::generate(),
            NodeId::generate(),
            "text_out",
        ))
        .expect("a slot");
    let started = Instant::now();
    let mut first = None;
    let mut events = Vec::new();
    for (index, chunk) in data.chunks(samples_per_frame * 2).enumerate() {
        let frame = AudioFrame {
            format,
            position: SampleClockPosition::new((index * samples_per_frame) as u64).unwrap(),
            sample_count: (chunk.len() / 2) as u32,
            data: chunk.to_vec(),
        };
        handle.offer_frame(frame).unwrap();
        for event in handle.by_ref() {
            first.get_or_insert(started.elapsed());
            events.push(event);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    handle.end_input().unwrap();
    events.extend(handle.by_ref());
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
    let committed: Vec<String> = handle
        .output()
        .spans()
        .iter()
        .map(|span| span.text().to_owned())
        .collect();
    println!("first event after {first:?}; committed {committed:?}");
    println!("provider duration {:?}", handle.provider_duration());
    assert!(first.is_some(), "an interim result arrived while streaming");
    assert!(!committed.is_empty(), "a span was committed");
    assert!(matches!(
        events.last().map(|event| &event.payload),
        Some(TranscriptEvent::Usage(record)) if record.r#final
    ));
}
