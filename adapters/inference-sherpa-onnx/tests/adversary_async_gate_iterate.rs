//! Adversary, story:async-speech-handles: the decode gate and a recognition iterated with nothing
//! offered. Iterating takes a batch of whatever was offered, even nothing, and waits on the
//! calling thread for a place in the recogniser's gate before feeding it. So while a place is
//! held, here by the batch of a recognition dropped mid-decode, a fresh recognition that was
//! offered nothing blocks its caller on `next` for as long as that batch decodes, where it used
//! to return `None` at once. The async form skips such an empty batch; iterating does not.
//!
//! Needs the unpacked pinned checkpoint in the directory `INFERENCE_SHERPA_ONNX_STT_DIR`; ignored
//! by default, as the conformance tests are.
#![cfg(feature = "stt")]

mod support;

use std::future::poll_fn;
use std::task::Poll;
use std::time::{Duration, Instant};

use inference_stt::{Recogniser, RecognitionHandle};
use support::{PCM16_16K_MONO, frames, open, reference_clip, request};

/// How long `next` on a recognition offered nothing may take.
const BOUND: Duration = Duration::from_millis(200);

/// The control: with no batch decoding anywhere, `next` on a recognition offered nothing
/// returns `None` within [`BOUND`].
#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_STT_DIR"]
fn control_iterating_a_recognition_offered_nothing_returns_at_once() {
    let mut recogniser = open(1);
    let mut handle = recogniser.start(request()).expect("a slot is free");
    let asked = Instant::now();
    let next = handle.next();
    let took = asked.elapsed();
    assert!(next.is_none());
    assert!(took < BOUND, "{took:?}");
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_STT_DIR"]
fn iterating_a_recognition_offered_nothing_does_not_wait_for_another_recognitions_batch() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("a current-thread runtime");
    let mut recogniser = open(1);
    let mut first = recogniser.start(request()).expect("a slot is free");
    let clip = reference_clip();
    for _ in 0..8 {
        for frame in frames(&clip, PCM16_16K_MONO) {
            first.offer_frame(frame).expect("input is open");
        }
    }
    let pending = runtime.block_on(poll_fn(|cx| Poll::Ready(first.poll_event(cx).is_pending())));
    assert!(pending, "the first poll starts the batch on its thread");
    drop(first);
    let mut second = recogniser
        .start(request())
        .expect("the dropped one's slot is free");
    let asked = Instant::now();
    let next = second.next();
    let took = asked.elapsed();
    assert!(next.is_none(), "nothing was offered: {next:?}");
    assert!(
        took < BOUND,
        "next on a recognition offered nothing blocked its caller for {took:?}, waiting for the \
         dropped recognition's batch to give back its place"
    );
}
