//! Recognition of recorded human speech by the pinned checkpoint, offered in 20 ms frames: the
//! LibriSpeech clip `test_wavs/0.wav` published with the sherpa-onnx streaming Zipformer
//! checkpoint the sibling adapter pins, read from the path `INFERENCE_WHISPERCPP_STT_CLIP` and
//! checked against its sha256. The conformance tests use the synthetic fixture, two of them with
//! this clip; this one shows the recogniser on real speech alone.
//!
//! Needs the pinned checkpoint file at the path `INFERENCE_WHISPERCPP_STT_MODEL` and the clip at
//! the path `INFERENCE_WHISPERCPP_STT_CLIP`; ignored by default, and fails naming the variable
//! that is not set. Run it with
//! `INFERENCE_WHISPERCPP_STT_MODEL=<ggml-base.en.bin> INFERENCE_WHISPERCPP_STT_CLIP=<0.wav> cargo test -p inference-whispercpp --test stt_live -- --ignored`.
//! No test downloads weights or audio.

mod support;

use inference_stt::inference_text::SpanState;
use inference_stt::{Recogniser, RecognitionHandle, TranscriptEvent};

use support::{PCM16_16K_MONO, frames, open, request, speech_clip, stream};

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
            "inference.stream.TranscriptCommitted"
        ],
        "one hypothesis, committed at once"
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
