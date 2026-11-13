//! The recognition's async form decodes off the caller's runtime: a 10 ms ticker on the same
//! current-thread runtime keeps its largest gap under 50 ms while one recognition of the
//! reference clip decodes, and the transcript reads the clip's words; and a recognition dropped
//! while its batch decodes does not have that batch decode beside the next recognition's.
//!
//! The cases run one at a time, as the second counts the decode threads of the whole process.
//!
//! Needs the unpacked pinned checkpoint in the directory `INFERENCE_SHERPA_ONNX_STT_DIR`; ignored
//! by default, as the conformance tests are.
#![cfg(feature = "stt")]

mod support;

use std::cell::Cell;
use std::future::poll_fn;
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::task::Poll;
use std::time::{Duration, Instant};

use inference_sherpa_onnx::stt::{SherpaOnnxRecogniser, SherpaOnnxRecognition};
use inference_stt::{Recogniser, RecognitionHandle, TranscriptEvent};
use support::{FIRST_PART_WORDS, PCM16_16K_MONO, frames, open, reference_clip, request};

const TICK: Duration = Duration::from_millis(10);
const GAP_BOUND: Duration = Duration::from_millis(50);

/// Held by each case for its whole run, so the cases of this file run one at a time.
static ALONE: Mutex<()> = Mutex::new(());

fn alone() -> MutexGuard<'static, ()> {
    ALONE.lock().unwrap_or_else(PoisonError::into_inner)
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_STT_DIR"]
fn a_ticker_keeps_time_while_the_async_form_decodes() {
    let _alone = alone();
    let mut recogniser = open(1);
    let mut handle = recogniser.start(request()).expect("a slot is free");
    for frame in frames(&reference_clip(), PCM16_16K_MONO) {
        handle.offer_frame(frame).expect("input is open");
    }
    handle.end_input().expect("input ends once");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("a current-thread runtime");
    let local = tokio::task::LocalSet::new();
    let done = Rc::new(Cell::new(false));
    let largest = Rc::new(Cell::new(Duration::ZERO));
    let (text, ticks) = local.block_on(&runtime, async {
        let ticker = {
            let done = Rc::clone(&done);
            let largest = Rc::clone(&largest);
            tokio::task::spawn_local(async move {
                let mut ticks = 0_u32;
                let mut last = Instant::now();
                while !done.get() {
                    tokio::time::sleep(TICK).await;
                    let now = Instant::now();
                    largest.set(largest.get().max(now - last));
                    last = now;
                    ticks += 1;
                }
                ticks
            })
        };
        let mut text = String::new();
        while let Some(envelope) = handle.event().await {
            if let TranscriptEvent::Committed(committed) = &envelope.payload {
                text.push_str(&committed.text);
                text.push(' ');
            }
        }
        done.set(true);
        (text, ticker.await.expect("the ticker ends"))
    });
    let text = text.to_lowercase();
    for word in FIRST_PART_WORDS {
        assert!(text.contains(word), "{word} in {text:?}");
    }
    assert!(ticks > 0, "the ticker ticked while the clip was decoded");
    assert!(
        largest.get() < GAP_BOUND,
        "the ticker's largest gap {:?} over {ticks} ticks",
        largest.get()
    );
}

/// The threads of this process that decode a batch, by the name the adapter gives them, as Linux
/// keeps it, cut to 15 bytes.
fn decoding() -> usize {
    std::fs::read_dir("/proc/self/task")
        .expect("the threads of this process")
        .filter_map(Result::ok)
        .filter_map(|task| std::fs::read_to_string(task.path().join("comm")).ok())
        .filter(|name| name.trim_end() == "inference-sherp")
        .count()
}

/// A recognition of eight reference clips in a row, one batch that decodes for a while, its
/// first poll made: the batch is decoding, or waiting for a place to.
fn started(
    runtime: &tokio::runtime::Runtime,
    recogniser: &mut SherpaOnnxRecogniser,
) -> SherpaOnnxRecognition {
    let mut handle = recogniser.start(request()).expect("a slot is free");
    let clip = reference_clip();
    for _ in 0..8 {
        for frame in frames(&clip, PCM16_16K_MONO) {
            handle.offer_frame(frame).expect("input is open");
        }
    }
    let pending = runtime.block_on(poll_fn(|cx| {
        Poll::Ready(handle.poll_event(cx).is_pending())
    }));
    assert!(
        pending,
        "the first poll starts the batch, or waits for a place, and waits"
    );
    handle
}

#[test]
#[ignore = "needs the pinned checkpoint unpacked in the directory INFERENCE_SHERPA_ONNX_STT_DIR"]
fn a_dropped_recognitions_batch_does_not_decode_beside_the_next_one_admitted() {
    let _alone = alone();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("a current-thread runtime");
    let mut recogniser = open(1);
    let first = started(&runtime, &mut recogniser);
    assert_eq!(decoding(), 1, "one batch decodes");
    drop(first);
    let mut second = started(&runtime, &mut recogniser);
    let running = decoding();
    assert!(
        running <= 1,
        "a recogniser opened for one recognition at a time decodes {running} batches at once \
         after the first was dropped mid-batch"
    );
    second.end_input().expect("input ends once");
    let last = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(120), async {
            let mut last = None;
            while let Some(envelope) = second.event().await {
                last = Some(matches!(envelope.payload, TranscriptEvent::Usage(_)));
            }
            last
        })
        .await
        .expect("the second recognition ends once the first batch is done")
    });
    assert_eq!(
        last,
        Some(true),
        "the usage record is the second recognition's last event"
    );
}
