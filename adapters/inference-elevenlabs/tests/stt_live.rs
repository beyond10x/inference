//! The live check of the realtime speech-to-text binding against ElevenLabs' hosted endpoint,
//! behind features `live` and `stt` and ignored: it needs a key with speech-to-text access in
//! `ELEVENLABS_API_KEY` (or the keychain secret `elevenlabs-api-key`) and a German speech clip of
//! about 30 s in G.711 μ-law at 8 kHz, mono, in `INFERENCE_ELEVENLABS_STT_CLIP`: a WAV file of
//! format 7 (μ-law), or the bare μ-law bytes. It is billed. Run it with
//! `cargo test -p inference-elevenlabs --features live,stt --test stt_live -- --ignored --nocapture`.

use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::time::Duration;

use inference_elevenlabs::{
    CommitStrategy, CredentialSource, ELEVENLABS_KEY_NAME, ELEVENLABS_KEY_VARIABLE,
    ElevenLabsRecogniser, Endpoint, EnvironmentSource, InputFormat, KeychainSource,
    TranscriptionConfig, VadSettings,
};
use inference_stt::inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_stt::inference_core::id::{NodeId, RunId, StreamId};
use inference_stt::inference_core::usage::QuantityKind;
use inference_stt::inference_text::TurnId;
use inference_stt::{Recogniser, RecognitionHandle, RecognitionRequest, TranscriptEvent};

/// The variable naming the clip.
const CLIP_VARIABLE: &str = "INFERENCE_ELEVENLABS_STT_CLIP";

/// Samples in one 20 ms frame at 8 kHz.
const FRAME_SAMPLES: usize = 160;

/// The key from the environment when it is set, else from the keychain, as the live example
/// reads it.
fn source() -> Arc<dyn CredentialSource> {
    if std::env::var_os(ELEVENLABS_KEY_VARIABLE).is_some() {
        Arc::new(EnvironmentSource::elevenlabs())
    } else {
        Arc::new(KeychainSource::native().expect("a keychain"))
    }
}

/// The μ-law samples of `path`: the data chunk of a WAV file of format 7, mono, 8 kHz, or the
/// whole file when it is not a WAV file.
fn mulaw(path: &str) -> Vec<u8> {
    let bytes = std::fs::read(path).unwrap_or_else(|error| panic!("{path}: {error}"));
    if bytes.get(0..4) != Some(b"RIFF".as_slice()) {
        return bytes;
    }
    let mut at = 12;
    let mut format_seen = false;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = &bytes[at + 8..(at + 8 + size).min(bytes.len())];
        if id == b"fmt " {
            assert_eq!(u16::from_le_bytes([body[0], body[1]]), 7, "μ-law");
            assert_eq!(u16::from_le_bytes([body[2], body[3]]), 1, "mono");
            assert_eq!(
                u32::from_le_bytes(body[4..8].try_into().unwrap()),
                8_000,
                "8 kHz"
            );
            format_seen = true;
        }
        if id == b"data" {
            assert!(format_seen, "{path}: fmt before data");
            return body.to_vec();
        }
        at += 8 + size + size % 2;
    }
    panic!("{path} has no data chunk");
}

#[test]
#[ignore = "billed: needs ELEVENLABS_API_KEY with speech-to-text access and INFERENCE_ELEVENLABS_STT_CLIP"]
fn a_german_mulaw_clip_is_committed_in_spans_with_timestamps_and_a_usage_record() {
    let clip = std::env::var(CLIP_VARIABLE).unwrap_or_else(|_| panic!("{CLIP_VARIABLE} is unset"));
    let data = mulaw(&clip);
    let format = AudioFormat {
        encoding: AudioEncoding::PcmMulaw,
        sample_rate: NonZeroU32::new(8_000).expect("a rate"),
        channels: NonZeroU16::MIN,
    };
    let config = TranscriptionConfig::new(InputFormat::Ulaw8000)
        .with_language("de")
        .with_commit_strategy(CommitStrategy::Vad(VadSettings::default()));
    let mut recogniser =
        ElevenLabsRecogniser::new(Endpoint::new(), config, source(), ELEVENLABS_KEY_NAME)
            .expect("the configuration is valid");
    let mut handle = recogniser
        .start(RecognitionRequest::new(
            RunId::generate(),
            TurnId::generate(),
            StreamId::generate(),
            NodeId::generate(),
            "text_out",
        ))
        .expect("a slot");
    let mut events = Vec::new();
    for (index, chunk) in data.chunks(FRAME_SAMPLES).enumerate() {
        let frame = AudioFrame {
            format,
            position: SampleClockPosition::new((index * FRAME_SAMPLES) as u64).unwrap(),
            sample_count: chunk.len() as u32,
            data: chunk.to_vec(),
        };
        handle.offer_frame(frame).unwrap();
        events.extend(handle.by_ref());
        std::thread::sleep(Duration::from_millis(20));
    }
    handle.end_input().unwrap();
    events.extend(handle.by_ref());
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
    let committed: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.payload {
            TranscriptEvent::Committed(committed) => Some(committed.clone()),
            _ => None,
        })
        .collect();
    for span in &committed {
        println!("{:?} {}", handle.span_timing(span.span_id), span.text);
    }
    assert!(!committed.is_empty(), "a span was committed");
    assert!(
        committed
            .iter()
            .any(|span| handle.span_timing(span.span_id).is_some()),
        "a committed span carries its timestamps"
    );
    let Some(TranscriptEvent::Usage(record)) = events.last().map(|event| &event.payload) else {
        panic!("the last event is the usage record");
    };
    assert!(record.r#final);
    let sent_ms = (data.len() as i64) * 1_000 / 8_000;
    let audio = record
        .quantities
        .iter()
        .find(|quantity| quantity.kind() == QuantityKind::InputAudioMs)
        .and_then(|quantity| quantity.value());
    println!(
        "usage: {audio:?} ms of input audio; warnings {:?}",
        handle.warnings()
    );
    assert_eq!(audio, Some(sent_ms));
}
