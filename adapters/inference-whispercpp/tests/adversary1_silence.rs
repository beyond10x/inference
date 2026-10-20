//! Adversary pass 1: an utterance with no speech. The story's scope says "An utterance with no
//! text emits nothing"; whisper.cpp writes non-speech annotations such as `[BLANK_AUDIO]` or
//! `(silence)` for audio without speech, which are not text of the utterance.

mod support;

use inference_stt::{Recogniser, TranscriptEvent};

use support::{PCM16_16K_MONO, frames, open, request, stream};

#[test]
#[ignore = "needs the pinned ggml-base.en.bin at the path INFERENCE_WHISPERCPP_STT_MODEL"]
fn two_seconds_of_silence_emit_no_span() {
    let mut recogniser = open(1);
    let mut handle = recogniser.start(request()).expect("nothing else runs");
    let feed = stream(&mut handle, frames(&vec![0_i16; 32_000], PCM16_16K_MONO));
    feed.print("silence");
    let texts: Vec<String> = feed
        .deliveries
        .iter()
        .map(|delivery| match &delivery.envelope.payload {
            TranscriptEvent::Hypothesis(event) => event.text.clone(),
            TranscriptEvent::Revision(event) => event.text.clone(),
            TranscriptEvent::Committed(event) => event.text.clone(),
        })
        .collect();
    assert!(
        texts.is_empty(),
        "an utterance with no speech emits no span: {texts:?}"
    );
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
}

#[test]
#[ignore = "needs the pinned ggml-base.en.bin at the path INFERENCE_WHISPERCPP_STT_MODEL"]
fn low_noise_emits_no_bracketed_annotation() {
    let mut recogniser = open(1);
    let mut handle = recogniser.start(request()).expect("nothing else runs");
    let mut state = 0x1234_5678_u32;
    let noise: Vec<i16> = (0..32_000)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            i16::try_from(state % 41).expect("small") - 20
        })
        .collect();
    let feed = stream(&mut handle, frames(&noise, PCM16_16K_MONO));
    feed.print("noise");
    let committed = feed.committed();
    assert!(
        !committed.contains('[') && !committed.contains('('),
        "near-silent noise commits no non-speech annotation as transcript: {committed:?}"
    );
}
