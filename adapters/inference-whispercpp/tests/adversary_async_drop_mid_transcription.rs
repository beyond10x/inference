//! Adversary, story:async-speech-handles: dropping a recognition while its async form transcribes
//! on a thread of its own. The drop frees the recognition's slot at once, but nothing stops the
//! thread: it transcribes to the end with its own reference to the model. A recogniser opened
//! for one recognition at a time then admits the next, whose transcription runs beside the
//! dropped one's, two at once. Iterating could not be dropped mid-transcription, so this is new
//! with the async form.
//!
//! Needs the pinned ggml-base.en.bin at the path `INFERENCE_WHISPERCPP_STT_MODEL`; ignored by
//! default, as the conformance tests are.

mod support;

use std::future::poll_fn;
use std::task::Poll;

use inference_stt::{Recogniser, RecognitionHandle};
use inference_whispercpp::stt::WhisperCppRecogniser;
use support::{PCM16_16K_MONO, fixture, frames, open, request};

/// The threads of this process that transcribe for a recognition, by the name the adapter gives
/// them, as Linux keeps it, cut to 15 bytes.
fn transcribing() -> usize {
    std::fs::read_dir("/proc/self/task")
        .expect("the threads of this process")
        .filter_map(Result::ok)
        .filter_map(|task| std::fs::read_to_string(task.path().join("comm")).ok())
        .filter(|name| name.trim_end() == "inference-whisp")
        .count()
}

#[test]
#[ignore = "needs the pinned ggml-base.en.bin at the path INFERENCE_WHISPERCPP_STT_MODEL"]
fn a_dropped_recognitions_transcription_does_not_run_beside_the_next_one_admitted() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("a current-thread runtime");
    let mut recogniser = open(1);
    let started = |recogniser: &mut WhisperCppRecogniser| {
        let mut handle = recogniser.start(request()).expect("a slot is free");
        for frame in frames(&fixture(), PCM16_16K_MONO) {
            handle.offer_frame(frame).expect("input is open");
        }
        handle.end_input().expect("input ends once");
        let pending = runtime.block_on(poll_fn(|cx| {
            Poll::Ready(handle.poll_event(cx).is_pending())
        }));
        assert!(pending, "the first poll starts the transcription and waits");
        handle
    };
    let first = started(&mut recogniser);
    assert_eq!(transcribing(), 1, "one transcription runs");
    drop(first);
    let second = started(&mut recogniser);
    let running = transcribing();
    drop(second);
    assert!(
        running <= 1,
        "a recogniser opened for one recognition at a time runs {running} transcriptions at once \
         after the first was dropped mid-transcription"
    );
}
