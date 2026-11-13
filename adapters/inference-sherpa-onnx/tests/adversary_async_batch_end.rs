//! Adversary, story:async-speech-handles: the end of input arriving while a batch the async form
//! started is still decoding on its thread. When that batch is done, `advance` settles it and
//! returns, and `poll_event` then hands out whatever the batch showed; a batch of silence shows
//! nothing, so the async form returns `Ready(None)`, which from the end of input on means nothing
//! more is delivered at all. The end of input, the flush, the last commit and the usage record
//! never come. Iterating after the same batch reaches the same `None`.
//!
//! Needs the unpacked pinned checkpoint in the directory `INFERENCE_SHERPA_ONNX_STT_DIR`; ignored
//! by default, as the conformance tests are.
#![cfg(feature = "stt")]

mod support;

use std::future::poll_fn;
use std::task::Poll;
use std::time::Duration;

use inference_sherpa_onnx::stt::{SherpaOnnxRecogniser, SherpaOnnxRecognition};
use inference_stt::{Recogniser, RecognitionHandle, TranscriptEvent};
use support::{PCM16_16K_MONO, frames, open, request};

/// Two seconds of silence at 16 kHz, as 20 ms frames.
fn silence() -> Vec<inference_stt::inference_audio::AudioFrame> {
    frames(&[0_i16; 32_000], PCM16_16K_MONO)
}

fn kind(event: &TranscriptEvent) -> &'static str {
    match event {
        TranscriptEvent::Hypothesis(_) => "hypothesis",
        TranscriptEvent::Revision(_) => "revision",
        TranscriptEvent::Committed(_) => "commit",
        TranscriptEvent::Usage(_) => "usage",
    }
}

/// A recognition whose async form has a batch of silence decoding on its thread, input ended
/// while it decodes.
fn ended_mid_batch(
    runtime: &tokio::runtime::Runtime,
) -> (SherpaOnnxRecogniser, SherpaOnnxRecognition) {
    let mut recogniser = open(1);
    let mut handle = recogniser.start(request()).expect("a slot is free");
    for frame in silence() {
        handle.offer_frame(frame).expect("input is open");
    }
    let first = runtime.block_on(poll_fn(|cx| {
        Poll::Ready(handle.poll_event(cx).is_pending())
    }));
    assert!(
        first,
        "the first poll starts the batch on its thread and waits for it"
    );
    handle.end_input().expect("input ends once");
    (recogniser, handle)
}

/// The control: the same silence with input ended before the first poll, one batch that is the
/// end, delivers the usage record through the async form.
#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_STT_DIR"]
fn control_the_async_form_delivers_the_usage_when_input_ends_before_the_first_poll() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("a current-thread runtime");
    let mut recogniser = open(1);
    let mut handle = recogniser.start(request()).expect("a slot is free");
    for frame in silence() {
        handle.offer_frame(frame).expect("input is open");
    }
    handle.end_input().expect("input ends once");
    let kinds = runtime.block_on(async {
        let mut kinds = Vec::new();
        while let Some(envelope) = handle.event().await {
            kinds.push(kind(&envelope.payload));
        }
        kinds
    });
    assert_eq!(kinds.last(), Some(&"usage"), "{kinds:?}");
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_STT_DIR"]
fn the_async_form_delivers_the_usage_when_input_ends_while_a_batch_decodes() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("a current-thread runtime");
    let (_recogniser, mut handle) = ended_mid_batch(&runtime);
    let kinds = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(30), async {
            let mut kinds = Vec::new();
            while let Some(envelope) = handle.event().await {
                kinds.push(kind(&envelope.payload));
            }
            kinds
        })
        .await
        .expect("the async form ends within 30 s")
    });
    assert_eq!(
        kinds.last(),
        Some(&"usage"),
        "input ended, so the async form's None means nothing more at all; the usage record is \
         its last event: {kinds:?}"
    );
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_STT_DIR"]
fn iterating_delivers_the_usage_when_input_ended_while_an_async_batch_decoded() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("a current-thread runtime");
    let (_recogniser, mut handle) = ended_mid_batch(&runtime);
    let kinds: Vec<_> = handle
        .by_ref()
        .take(1_000)
        .map(|envelope| kind(&envelope.payload))
        .collect();
    assert_eq!(
        kinds.last(),
        Some(&"usage"),
        "input ended, so iterating's None means nothing more at all; the usage record is its \
         last event: {kinds:?}"
    );
}
