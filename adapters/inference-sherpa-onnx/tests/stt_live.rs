//! Recognition by the pinned checkpoint, streamed in 20 ms frames.
//!
//! Every test needs the unpacked pinned checkpoint in the directory
//! `INFERENCE_SHERPA_ONNX_STT_DIR`: the four files `stt::FILES` names, and the first and the
//! third also `stt::REFERENCE_CLIP`, the checkpoint's own LibriSpeech clip `test_wavs/0.wav`.
//! They are ignored by default and fail naming that variable when run without it. Run them with
//! `INFERENCE_SHERPA_ONNX_STT_DIR=<dir> cargo test -p inference-sherpa-onnx --all-features -- --ignored`.
//! No test downloads weights.
//!
//! The second streams `tests/fixtures/question-16k.wav` ("What is the capital of France?",
//! synthesised with eSpeak NG, provenance in `question-16k.txt`). The checkpoint does not
//! recognise synthetic speech (the known limitation in the `stt` module documentation), so that
//! test asserts only that the recognition runs to the end of input.
//!
//! The third streams the reference clip again and reads the recognition's usage record, its last
//! event: `input_audio_ms` the clip's length and `local_compute_ms` measured above 0.
#![cfg(feature = "stt")]

use std::ffi::OsString;
use std::num::{NonZeroU16, NonZeroU32};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use inference_sherpa_onnx::stt::{REFERENCE_CLIP, SherpaOnnxRecogniser};
use inference_stt::inference_audio::{AudioEncoding, AudioFormat, AudioFrame, SampleClockPosition};
use inference_stt::inference_core::envelope::EventEnvelope;
use inference_stt::inference_core::id::{NodeId, RunId, StreamId};
use inference_stt::inference_core::usage::{Quantity, QuantityKind, QuantitySource, UsageRecorded};
use inference_stt::inference_text::{SpanState, TurnId};
use inference_stt::{Recogniser, RecognitionHandle, RecognitionRequest, TranscriptEvent};

const CHECKPOINT_DIR: &str = "INFERENCE_SHERPA_ONNX_STT_DIR";

/// 20 ms at 16 kHz.
const FRAME_SAMPLES: usize = 320;

fn checkpoint_dir() -> OsString {
    std::env::var_os(CHECKPOINT_DIR).unwrap_or_else(|| {
        panic!("set {CHECKPOINT_DIR} to the directory of the unpacked pinned checkpoint")
    })
}

/// The `pcm_s16le` samples of the `data` chunk of the WAV file at `path`.
fn wav_pcm(path: &Path) -> Vec<u8> {
    let wav = std::fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let mut at = 12;
    while at + 8 <= wav.len() {
        let size = u32::from_le_bytes([wav[at + 4], wav[at + 5], wav[at + 6], wav[at + 7]]);
        let size = usize::try_from(size).expect("a chunk size fits usize");
        if &wav[at..at + 4] == b"data" {
            return wav[at + 8..(at + 8 + size).min(wav.len())].to_vec();
        }
        at += 8 + size + size % 2;
    }
    panic!("{} has no data chunk", path.display());
}

fn frames(pcm: &[u8]) -> Vec<AudioFrame> {
    let format = AudioFormat {
        encoding: AudioEncoding::PcmS16le,
        sample_rate: NonZeroU32::new(16_000).expect("16000 is not zero"),
        channels: NonZeroU16::new(1).expect("1 is not zero"),
    };
    let mut position = 0_u64;
    pcm.chunks(FRAME_SAMPLES * 2)
        .map(|chunk| {
            let sample_count = u32::try_from(chunk.len() / 2).expect("a frame fits u32");
            let frame = AudioFrame {
                format,
                position: SampleClockPosition::new(position).expect("the clip is short"),
                sample_count,
                data: chunk.to_vec(),
            };
            position += u64::from(sample_count);
            frame
        })
        .collect()
}

/// Streams the WAV file at `clip` through a recognition of the pinned checkpoint in 20 ms
/// frames, collecting the events delivered after each frame and after the end of input, and
/// checks they are delivered in sequence order with nothing after the end and every span
/// committed; returns the texts of the committed spans.
fn recognise(clip: &Path) -> Vec<String> {
    let dir = checkpoint_dir();
    let mut recogniser = SherpaOnnxRecogniser::open(&dir).unwrap_or_else(|refused| {
        panic!("{CHECKPOINT_DIR} holds the pinned checkpoint: {refused}")
    });
    let mut handle = recogniser
        .start(RecognitionRequest::new(
            RunId::generate(),
            TurnId::generate(),
            StreamId::generate(),
            NodeId::generate(),
            "text_out",
        ))
        .expect("nothing else runs");

    let mut events: Vec<EventEnvelope<TranscriptEvent>> = Vec::new();
    for frame in frames(&wav_pcm(clip)) {
        handle.offer_frame(frame).expect("input has not ended");
        events.extend(handle.by_ref());
    }
    let before_end = events.len();
    handle.end_input().expect("input ends once");
    events.extend(handle.by_ref());

    for event in &events {
        eprintln!("{} {} {:?}", event.sequence, event.kind, event.payload);
    }
    eprintln!(
        "{before_end} of {} events before the end of input",
        events.len()
    );

    let sequences: Vec<i64> = events.iter().map(|event| event.sequence).collect();
    let expected: Vec<i64> = (1..=i64::try_from(events.len()).expect("few events")).collect();
    assert_eq!(
        sequences, expected,
        "events are delivered in sequence order"
    );
    assert!(handle.next().is_none(), "nothing follows the end of input");
    assert!(handle.output().input_ended());
    assert!(
        handle
            .output()
            .spans()
            .iter()
            .all(|span| span.state() == SpanState::Committed),
        "every span is committed once input has ended"
    );
    let committed: Vec<String> = handle
        .output()
        .spans()
        .iter()
        .map(|span| span.text().to_owned())
        .collect();
    eprintln!("committed: {committed:?}");
    committed
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_STT_DIR"]
fn the_reference_clip_is_recognised_into_a_committed_span() {
    let clip = PathBuf::from(checkpoint_dir()).join(REFERENCE_CLIP.name);
    let bytes = std::fs::read(&clip).unwrap_or_else(|error| {
        panic!(
            "{CHECKPOINT_DIR} holds {}: {}: {error}",
            REFERENCE_CLIP.name,
            clip.display()
        )
    });
    let sha256: String = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(
        sha256,
        REFERENCE_CLIP.sha256,
        "{} is the pinned clip",
        clip.display()
    );

    let committed = recognise(&clip);
    assert!(
        committed.iter().any(|text| {
            let text = text.to_lowercase();
            text.contains("nightfall") && text.contains("lamps") && text.contains("brothels")
        }),
        "a committed span reads the clip's transcript, {}: {committed:?}",
        REFERENCE_CLIP.transcript
    );
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_STT_DIR"]
fn the_synthetic_question_runs_to_the_end_of_input() {
    let clip = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/question-16k.wav");
    recognise(&clip);
}

/// The one quantity of `kind` in `recorded`.
fn quantity(recorded: &UsageRecorded, kind: QuantityKind) -> Quantity {
    let found: Vec<&Quantity> = recorded
        .quantities
        .iter()
        .filter(|quantity| quantity.kind() == kind)
        .collect();
    let [quantity] = found.as_slice() else {
        panic!("one {kind:?} quantity in {recorded:?}")
    };
    **quantity
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_STT_DIR"]
fn the_reference_clip_is_recorded_as_its_length_with_the_compute_measured() {
    let dir = checkpoint_dir();
    let pcm = wav_pcm(&PathBuf::from(&dir).join(REFERENCE_CLIP.name));
    let clip_ms = i64::try_from(pcm.len() / 2 * 1_000 / 16_000).expect("a short clip");
    let mut recogniser = SherpaOnnxRecogniser::open(&dir).unwrap_or_else(|refused| {
        panic!("{CHECKPOINT_DIR} holds the pinned checkpoint: {refused}")
    });
    let mut handle = recogniser
        .start(RecognitionRequest::new(
            RunId::generate(),
            TurnId::generate(),
            StreamId::generate(),
            NodeId::generate(),
            "text_out",
        ))
        .expect("nothing else runs");
    let mut events: Vec<EventEnvelope<TranscriptEvent>> = Vec::new();
    for frame in frames(&pcm) {
        handle.offer_frame(frame).expect("input has not ended");
        events.extend(handle.by_ref());
    }
    assert!(
        events
            .iter()
            .all(|event| !matches!(event.payload, TranscriptEvent::Usage(_))),
        "no usage is recorded before the end of input"
    );
    handle.end_input().expect("input ends once");
    events.extend(handle.by_ref());

    let Some(TranscriptEvent::Usage(recorded)) = events.last().map(|event| &event.payload) else {
        panic!("the usage record is the last event: {events:?}")
    };
    eprintln!("usage of the {clip_ms} ms clip: {recorded:?}");
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.payload, TranscriptEvent::Usage(_)))
            .count(),
        1
    );
    assert_eq!(
        quantity(recorded, QuantityKind::InputAudioMs),
        Quantity::measured(QuantityKind::InputAudioMs, clip_ms),
        "the clip's length"
    );
    assert_eq!(
        quantity(recorded, QuantityKind::Requests),
        Quantity::measured(QuantityKind::Requests, 1)
    );
    let compute = quantity(recorded, QuantityKind::LocalComputeMs);
    assert_eq!(compute.source(), QuantitySource::Measured, "{compute:?}");
    assert!(
        compute.value().is_some_and(|ms| ms > 0),
        "decoding the clip took time: {compute:?}"
    );
    assert_eq!(recorded.adapter_id, recogniser.adapter().adapter_id());
    assert_eq!(recorded.model_id, recogniser.model().model_id());
    assert!(handle.next().is_none(), "nothing follows the usage record");
}
