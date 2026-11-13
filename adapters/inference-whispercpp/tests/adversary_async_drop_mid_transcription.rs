//! Adversary, story:async-speech-handles: dropping a recognition while its async form transcribes
//! on a thread of its own. A recogniser opened for one recognition at a time must not then
//! transcribe the next recognition beside the dropped one's transcription.
//!
//! Concurrency is measured by the threads named like the transcription thread,
//! `inference-whispercpp` as Linux keeps it, cut to 15 bytes. whisper.cpp's compute threads
//! inherit that name, so one transcription alone shows several; the case first records the
//! largest count one lone transcription shows, then drops a recognition mid-transcription,
//! transcribes the next one to its end and requires that count never to be exceeded by more
//! than the one thread a handover from one transcription to the next may briefly show. Two
//! transcriptions at once show about twice it.
//!
//! Needs the pinned ggml-base.en.bin at the path `INFERENCE_WHISPERCPP_STT_MODEL`; ignored by
//! default, as the conformance tests are.

mod support;

use std::future::poll_fn;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::Poll;
use std::time::Duration;

use inference_stt::{Recogniser, RecognitionHandle, TranscriptEvent};
use inference_whispercpp::stt::{WhisperCppRecogniser, WhisperCppRecognition};
use support::{PCM16_16K_MONO, fixture, frames, open, request};

/// The threads of this process named like the transcription thread.
fn transcribing() -> usize {
    std::fs::read_dir("/proc/self/task")
        .expect("the threads of this process")
        .filter_map(Result::ok)
        .filter_map(|task| std::fs::read_to_string(task.path().join("comm")).ok())
        .filter(|name| name.trim_end() == "inference-whisp")
        .count()
}

/// Samples [`transcribing`] every millisecond on a thread of its own while `during` runs;
/// returns the largest count seen and what `during` returned.
fn largest_while<T>(during: impl FnOnce() -> T) -> (usize, T) {
    let stop = Arc::new(AtomicBool::new(false));
    let largest = Arc::new(AtomicUsize::new(0));
    let sampler = {
        let stop = Arc::clone(&stop);
        let largest = Arc::clone(&largest);
        std::thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                largest.fetch_max(transcribing(), Ordering::AcqRel);
                std::thread::sleep(Duration::from_millis(1));
            }
        })
    };
    let returned = during();
    stop.store(true, Ordering::Release);
    sampler.join().expect("the sampler ends without a panic");
    (largest.load(Ordering::Acquire), returned)
}

/// A recognition of the fixture with input ended.
fn ended(recogniser: &mut WhisperCppRecogniser) -> WhisperCppRecognition {
    let mut handle = recogniser.start(request()).expect("a slot is free");
    for frame in frames(&fixture(), PCM16_16K_MONO) {
        handle.offer_frame(frame).expect("input is open");
    }
    handle.end_input().expect("input ends once");
    handle
}

/// Every event of `handle` through the async form, until it ends; whether a usage record came.
fn drained(runtime: &tokio::runtime::Runtime, handle: &mut WhisperCppRecognition) -> bool {
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(60), async {
            let mut usage = false;
            while let Some(envelope) = handle.event().await {
                usage |= matches!(envelope.payload, TranscriptEvent::Usage(_));
            }
            usage
        })
        .await
        .expect("the transcription ends within 60 s")
    })
}

#[test]
#[ignore = "needs the pinned ggml-base.en.bin at the path INFERENCE_WHISPERCPP_STT_MODEL"]
fn a_dropped_recognitions_transcription_does_not_run_beside_the_next_one_admitted() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("a current-thread runtime");
    let mut recogniser = open(1);

    let (alone, usage) = largest_while(|| drained(&runtime, &mut ended(&mut recogniser)));
    assert!(usage, "the lone transcription ends with its usage record");
    assert!(alone > 0, "a lone transcription shows its threads");

    let (largest, usage) = largest_while(|| {
        let mut first = ended(&mut recogniser);
        let pending =
            runtime.block_on(poll_fn(|cx| Poll::Ready(first.poll_event(cx).is_pending())));
        assert!(pending, "the first poll starts the transcription and waits");
        drop(first);
        drained(&runtime, &mut ended(&mut recogniser))
    });
    assert!(usage, "the next transcription ends with its usage record");
    assert!(
        largest <= alone + 1,
        "one transcription alone shows at most {alone} threads; after a recognition was dropped \
         mid-transcription the next one admitted ran beside it, {largest} threads at once"
    );
}
