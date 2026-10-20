//! Recognition of recorded human speech by the pinned checkpoint, offered in 20 ms frames: the
//! LibriSpeech clip `test_wavs/0.wav` published with the sherpa-onnx streaming Zipformer
//! checkpoint the sibling adapter pins, read from the path `INFERENCE_WHISPERCPP_STT_CLIP` and
//! checked against its sha256. The conformance tests use the synthetic fixture, two of them with
//! this clip; these show the recogniser on real speech alone, and the usage record of its
//! recognition: `input_audio_ms` the clip's length and `local_compute_ms` measured above 0.
//!
//! Needs the pinned checkpoint file at the path `INFERENCE_WHISPERCPP_STT_MODEL` and the clip at
//! the path `INFERENCE_WHISPERCPP_STT_CLIP`; ignored by default, and fails naming the variable
//! that is not set. Run them with
//! `INFERENCE_WHISPERCPP_STT_MODEL=<ggml-base.en.bin> INFERENCE_WHISPERCPP_STT_CLIP=<0.wav> cargo test -p inference-whispercpp --test stt_live -- --ignored`.
//! No test downloads weights or audio.

mod support;

use inference_stt::inference_core::usage::{Quantity, QuantityKind, QuantitySource, UsageRecorded};
use inference_stt::inference_text::SpanState;
use inference_stt::{Recogniser, RecognitionHandle, TranscriptEvent};

use support::{CLIP_RATE, PCM16_16K_MONO, frames, open, request, speech_clip, stream};

#[test]
#[ignore = "needs ggml-base.en.bin at INFERENCE_WHISPERCPP_STT_MODEL and the speech clip at INFERENCE_WHISPERCPP_STT_CLIP"]
fn recorded_speech_is_committed_as_one_span_after_the_end_of_input() {
    let mut recogniser = open(1);
    let clip = speech_clip();
    let mut handle = recogniser.start(request()).expect("nothing else runs");
    let feed = stream(&mut handle, frames(&clip.samples, PCM16_16K_MONO));
    feed.print("speech");

    assert!(
        feed.deliveries.iter().all(|delivery| !delivery.before_end),
        "nothing reaches the stream before the end of input"
    );
    let kinds: Vec<&str> = feed
        .deliveries
        .iter()
        .map(|delivery| delivery.envelope.payload.name())
        .collect();
    assert_eq!(
        kinds,
        [
            "inference.stream.TranscriptHypothesis",
            "inference.stream.TranscriptCommitted",
            "inference.usage.UsageRecorded"
        ],
        "one hypothesis, committed at once, then the usage record"
    );
    assert!(matches!(
        feed.deliveries[1].envelope.payload,
        TranscriptEvent::Committed(_)
    ));
    assert_eq!(handle.output().spans().len(), 1);
    assert_eq!(handle.output().spans()[0].state(), SpanState::Committed);
    let committed = feed.committed();
    for word in clip.words {
        assert!(
            committed.contains(word),
            "the committed text reads {word:?} of the transcript {:?}: {committed:?}",
            clip.transcript
        );
    }
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
#[ignore = "needs ggml-base.en.bin at INFERENCE_WHISPERCPP_STT_MODEL and the speech clip at INFERENCE_WHISPERCPP_STT_CLIP"]
fn recorded_speech_is_recorded_as_its_length_with_the_compute_measured() {
    let mut recogniser = open(1);
    let clip = speech_clip();
    let clip_ms =
        i64::try_from(clip.samples.len() * 1_000 / CLIP_RATE as usize).expect("a short clip");
    let mut handle = recogniser.start(request()).expect("nothing else runs");
    let feed = stream(&mut handle, frames(&clip.samples, PCM16_16K_MONO));

    let usage: Vec<&UsageRecorded> = feed
        .deliveries
        .iter()
        .filter_map(|delivery| match &delivery.envelope.payload {
            TranscriptEvent::Usage(recorded) => Some(recorded),
            _ => None,
        })
        .collect();
    let [recorded] = usage.as_slice() else {
        panic!("one usage record: {:?}", feed.deliveries.len())
    };
    eprintln!("usage of the {clip_ms} ms clip: {recorded:?}");
    assert!(
        matches!(
            feed.deliveries
                .last()
                .map(|delivery| &delivery.envelope.payload),
            Some(TranscriptEvent::Usage(_))
        ),
        "the usage record is the last event"
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
        "transcribing the clip took time: {compute:?}"
    );
    assert_eq!(recorded.adapter_id, recogniser.adapter().adapter_id());
    assert_eq!(recorded.model_id, recogniser.model().model_id());
    assert!(handle.next().is_none(), "nothing follows the usage record");
}
