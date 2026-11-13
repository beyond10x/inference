//! The recognition's async form decodes off the caller's runtime: a 10 ms ticker on the same
//! current-thread runtime keeps its largest gap under 50 ms while one recognition of the fixture
//! is transcribed, and the transcript is the one iterating gives.

mod support;

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use inference_stt::{Recogniser, RecognitionHandle, TranscriptEvent};
use support::{FIXTURE_WORDS, PCM16_16K_MONO, fixture, frames, open, request};

const TICK: Duration = Duration::from_millis(10);
const GAP_BOUND: Duration = Duration::from_millis(50);

#[test]
#[ignore = "needs the pinned ggml-base.en.bin at the path INFERENCE_WHISPERCPP_STT_MODEL"]
fn a_ticker_keeps_time_while_the_async_form_transcribes() {
    let mut recogniser = open(1);
    let mut handle = recogniser.start(request()).expect("a slot is free");
    for frame in frames(&fixture(), PCM16_16K_MONO) {
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
            }
        }
        done.set(true);
        (text, ticker.await.expect("the ticker ends"))
    });
    assert!(handle.failure().is_none(), "{:?}", handle.failure());
    let text = text.to_lowercase();
    for word in FIXTURE_WORDS {
        assert!(text.contains(word), "{word} in {text:?}");
    }
    assert!(
        ticks > 0,
        "the ticker ticked while the fixture was transcribed"
    );
    assert!(
        largest.get() < GAP_BOUND,
        "the ticker's largest gap {:?} over {ticks} ticks",
        largest.get()
    );
}
